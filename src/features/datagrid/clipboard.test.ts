import { describe, expect, it, vi } from "vitest";
import type { Value } from "@/lib/ipc";
import {
  copiedMarkers,
  formatCells,
  formatColumnValues,
  INSERT_CHUNK_BYTES,
  insertLiteral,
  mysqlLongLat,
  parseCsvPaste,
  parseTsv,
  parseTsvPaste,
  rememberCopy,
  resetCopyMarkersForTests,
  sniffCsvPaste,
} from "./clipboard";
import { DEFAULT_CSV_OPTIONS, type CsvOptions } from "./csv";
import type { GridColumn } from "./types";

const col = (name: string, kind: GridColumn["kind"]): GridColumn => ({
  id: name,
  name,
  kind,
  dataType: kind,
  nullable: true,
  isPk: false,
  hasDefault: false,
  isAutoInc: false,
  editable: true,
  align: "left",
});
const columns = [col("id", "int"), col("name", "text"), col("big", "bigInt"), col("j", "json")];
const rows: Value[][] = [
  [
    { t: "int", v: 1 },
    { t: "text", v: "a\tb" },
    { t: "bigInt", v: "9007199254740993" },
    { t: "json", v: '{"x":1}' },
  ],
  [{ t: "int", v: 2 }, { t: "null" }, { t: "bigInt", v: "5" }, { t: "null" }],
];

describe("formatCells", () => {
  it("tsv quotes tabs and leaves NULL empty", () => {
    expect(formatCells("tsv", columns, rows)).toBe(
      '1\t"a\tb"\t9007199254740993\t{"x":1}\n2\t\t5\t',
    );
  });
  it("csv names the columns only with a header", () => {
    expect(formatCells("csv", columns, rows, { headers: true }).split("\n")[0]).toBe(
      "id,name,big,j",
    );
    expect(formatCells("csv", columns, rows)).toBe('1,a\tb,9007199254740993,"{""x"":1}"\n2,,5,');
  });
  it("csv follows the CSV default options (delimiter, quotes, line break, decimal)", () => {
    const money = [col("id", "int"), col("price", "decimal"), col("note", "text")];
    const data: Value[][] = [
      [
        { t: "int", v: 1 },
        { t: "decimal", v: "12.50" },
        { t: "text", v: "it's; fine" },
      ],
      [{ t: "int", v: 2 }, { t: "null" }, { t: "text", v: "plain" }],
    ];
    const csv = (o: Partial<CsvOptions>) =>
      formatCells("csv", money, data, { headers: true, csv: { ...DEFAULT_CSV_OPTIONS, ...o } });
    expect(csv({ delimiter: ";", decimal: ",", lineBreak: "\r\n" })).toBe(
      'id;price;note\r\n1;12,50;"it\'s; fine"\r\n2;;plain',
    );
    // Every value quoted (NULL stays empty), with the chosen quote doubled inside.
    expect(csv({ quote: "single" })).toBe(
      "'id','price','note'\n'1','12.50','it''s; fine'\n'2',,'plain'",
    );
    expect(csv({ quote: "double", delimiter: "\t", lineBreak: "\r" })).toBe(
      '"id"\t"price"\t"note"\r"1"\t"12.50"\t"it\'s; fine"\r"2"\t\t"plain"',
    );
    // A decimal comma under a comma delimiter needs quotes.
    expect(csv({ decimal: "," }).split("\n")[1]).toBe('1,"12,50",it\'s; fine');
  });
  it("tsv adds a header line on request (plain text has none)", () => {
    expect(formatCells("tsv", columns, rows, { headers: true }).split("\n")[0]).toBe(
      "id\tname\tbig\tj",
    );
  });
  it("html writes an escaped table, NULL as an empty cell", () => {
    const html = formatCells(
      "html",
      [col("a", "text"), col("b", "int")],
      [
        [
          { t: "text", v: "<b>&'\"\nx" },
          { t: "int", v: 1 },
        ],
        [{ t: "null" }, { t: "int", v: 2 }],
      ],
    );
    expect(html).toBe(
      "<table>\n<thead>\n<tr><th>a</th><th>b</th></tr>\n</thead>\n<tbody>\n" +
        "<tr><td>&lt;b&gt;&amp;&#39;&quot;<br>x</td><td>1</td></tr>\n" +
        "<tr><td></td><td>2</td></tr>\n</tbody>\n</table>",
    );
    expect(formatCells("html", [col("a", "text")], [])).toBe(
      "<table>\n<thead>\n<tr><th>a</th></tr>\n</thead>\n<tbody>\n</tbody>\n</table>",
    );
  });
  it("json keeps unsafe ints as strings and parses json cells", () => {
    const parsed = JSON.parse(formatCells("json", columns, rows)) as Array<Record<string, unknown>>;
    expect(parsed[0]).toEqual({ id: 1, name: "a\tb", big: "9007199254740993", j: { x: 1 } });
    expect(parsed[1]?.big).toBe(5);
    expect(parsed[1]?.name).toBeNull();
  });
  it("insert quotes per dialect, in one multi-row statement", () => {
    const pg = formatCells("insert", columns, rows, {
      dialect: "postgres",
      schema: "public",
      table: "t",
    });
    expect(pg).toBe(
      `INSERT INTO "public"."t" ("id", "name", "big", "j") VALUES\n` +
        `(1, 'a\tb', 9007199254740993, '{"x":1}'),\n(2, NULL, 5, NULL);`,
    );
    const my = formatCells("insert", columns, rows, { dialect: "mysql", schema: null, table: "t" });
    expect(my.split("\n")[0]).toBe("INSERT INTO `t` (`id`, `name`, `big`, `j`) VALUES");
    expect(my.split("\n")[2]).toBe("(2, NULL, 5, NULL);");
    expect(formatCells("insert", columns, [])).toBe("");
  });
  it("insert splits the rows into statements under the size cap (~250 KB)", () => {
    const one = [col("s", "text")];
    const many: Value[][] = Array.from({ length: 10 }, (_, i) => [{ t: "text", v: `v${i}` }]);
    const ctx = { dialect: "sqlite" as const, schema: null, table: "t" };
    // Head `INSERT INTO "t" ("s") VALUES\n` is 29 bytes, each tuple ('vN') 6 plus ",\n".
    const out = formatCells("insert", one, many, { ...ctx, insertChunkBytes: 60 });
    const statements = out.split(";\n").map((st) => (st.endsWith(";") ? st : `${st};`));
    expect(statements).toHaveLength(3);
    for (const st of statements)
      expect(new TextEncoder().encode(st).length).toBeLessThanOrEqual(60);
    expect(statements[0]).toBe(`INSERT INTO "t" ("s") VALUES\n('v0'),\n('v1'),\n('v2'),\n('v3');`);
    expect(out.match(/\('v\d'\)/g)).toHaveLength(10);
    // A row larger than the cap still gets a statement of its own.
    const big: Value[][] = [[{ t: "text", v: "é".repeat(40) }], [{ t: "text", v: "x" }]];
    expect(
      formatCells("insert", one, big, { ...ctx, insertChunkBytes: 60 }).split(";\n"),
    ).toHaveLength(2);
    // The default cap keeps every statement under 250 KB.
    const wide: Value[][] = Array.from({ length: 3000 }, () => [{ t: "text", v: "x".repeat(200) }]);
    const chunks = formatCells("insert", one, wide, ctx).split(";\n");
    expect(chunks.length).toBeGreaterThan(1);
    for (const st of chunks) expect(st.length).toBeLessThanOrEqual(INSERT_CHUNK_BYTES);
  });
  it("markdown escapes pipes", () => {
    const md = formatCells("markdown", [col("a", "text")], [[{ t: "text", v: "x|y" }]]);
    expect(md).toBe("| a |\n| --- |\n| x\\|y |");
  });
});

describe("Copy Rows As ▸ SQL Insert with spatial values (as the export writer)", () => {
  const geom = (text: string, ty = "geometry"): Value => ({ t: "other", v: { ty, text } });

  it("MySQL: (E)WKT through ST_GeomFromText with the SRID, hex as a binary literal", () => {
    expect(insertLiteral("mysql", geom("LINESTRING(0 0,1 2)"))).toBe(
      "ST_GeomFromText('LINESTRING(0 0,1 2)', 0)",
    );
    expect(insertLiteral("mysql", geom("SRID=4326;POINT(2 1)"))).toBe(
      "ST_GeomFromText('POINT(2 1)', 4326)",
    );
    // MySQL 8 reads geographic WKT latitude first unless told longitude first.
    expect(insertLiteral("mysql", geom("SRID=4326;POINT(2 1)"), true)).toBe(
      "ST_GeomFromText('POINT(2 1)', 4326, 'axis-order=long-lat')",
    );
    expect(insertLiteral("mysql", geom("POINT(2 1)"), true)).toBe(
      "ST_GeomFromText('POINT(2 1)', 0)",
    );
    expect(insertLiteral("mysql", geom("srid=3857; MULTIPOINT((1 2),(3 -4.5e3)) "))).toBe(
      "ST_GeomFromText('MULTIPOINT((1 2),(3 -4.5e3))', 3857)",
    );
    expect(insertLiteral("mysql", geom("00000000AB"))).toBe("X'00000000AB'");
  });

  it("PostGIS: EWKT or hex EWKB cast to the value's type", () => {
    expect(insertLiteral("postgres", geom("SRID=4326;POINT(1 2)"))).toBe(
      "'SRID=4326;POINT(1 2)'::geometry",
    );
    expect(insertLiteral("postgres", geom("0101000020E6100000", "geometry(Point,4326)"))).toBe(
      "'0101000020E6100000'::geometry",
    );
    expect(insertLiteral("postgres", geom("POINT(1 2)", "public.geography"))).toBe(
      "'POINT(1 2)'::geography",
    );
  });

  it("never turns text that is not (E)WKT into SQL", () => {
    // Quoted like any other string (MySQL escapes the quote with a backslash), never executed.
    expect(insertLiteral("mysql", geom("POINT(1 2)'); DROP"))).toBe("'POINT(1 2)\\'); DROP'");
    expect(insertLiteral("postgres", geom("POINT(1 2)'); DROP"))).toBe(
      "'POINT(1 2)''); DROP'::geometry",
    );
    expect(insertLiteral("mysql", geom("SRID=x;POINT(1 2)"))).toBe("'SRID=x;POINT(1 2)'");
    expect(insertLiteral("mysql", geom("POINT(1 2)", "set"))).toBe("'POINT(1 2)'");
    expect(insertLiteral("postgres", geom("10.0.0.1", "inet"))).toBe("'10.0.0.1'");
    expect(insertLiteral("sqlite", geom("POINT(1 2)"))).toBe("'POINT(1 2)'");
    // Other values are the plain literals.
    expect(insertLiteral("postgres", { t: "int", v: 3 })).toBe("3");
  });

  it("formatCells writes them into the INSERT, MySQL 8's long-lat order included", () => {
    const cols = [col("id", "int"), col("g", "geometry")];
    const data: Value[][] = [[{ t: "int", v: 1 }, geom("SRID=4326;POINT(2 1)")]];
    const ctx = { dialect: "mysql" as const, schema: null, table: "places" };
    expect(formatCells("insert", cols, data, ctx)).toBe(
      "INSERT INTO `places` (`id`, `g`) VALUES\n(1, ST_GeomFromText('POINT(2 1)', 4326));",
    );
    expect(formatCells("insert", cols, data, { ...ctx, mysqlLongLat: true })).toContain(
      "'axis-order=long-lat'",
    );
    expect(
      formatCells("insert", cols, data, { dialect: "postgres", schema: "public", table: "places" }),
    ).toBe(
      `INSERT INTO "public"."places" ("id", "g") VALUES\n(1, 'SRID=4326;POINT(2 1)'::geometry);`,
    );
  });

  it("long-lat only for MySQL 8 and later (not 5.7, not MariaDB)", () => {
    expect(mysqlLongLat("mysql", "8.0.35")).toBe(true);
    expect(mysqlLongLat("mysql", "9.6.0")).toBe(true);
    expect(mysqlLongLat("mysql", "5.7.44")).toBe(false);
    expect(mysqlLongLat("mysql", "5.5.5-10.6.12-MariaDB")).toBe(false);
    expect(mysqlLongLat("mysql", "10.11.6-MariaDB")).toBe(false);
    expect(mysqlLongLat("postgres", "17.2")).toBe(false);
    expect(mysqlLongLat("mysql", "")).toBe(false);
  });
});

describe("formatColumnValues", () => {
  it("lists values comma-separated, strings single-quoted, numbers and NULL bare", () => {
    expect(
      formatColumnValues([
        { t: "int", v: 1 },
        { t: "text", v: "it's" },
        { t: "null" },
        { t: "decimal", v: "1.50" },
        { t: "bool", v: true },
        { t: "uuid", v: "a-b" },
        { t: "date", v: "2024-01-02" },
        { t: "default" },
      ]),
    ).toBe("1, 'it''s', NULL, 1.50, true, 'a-b', '2024-01-02', DEFAULT");
    expect(formatColumnValues([])).toBe("");
  });
});

describe("parseTsv", () => {
  it("round-trips formatCells('tsv') with tabs, quotes and line breaks", () => {
    const tricky: Value[][] = [
      [
        { t: "text", v: "a\tb" },
        { t: "text", v: 'say "hi"' },
        { t: "text", v: "line1\nline2\r\nline3" },
        { t: "null" },
      ],
      [
        { t: "text", v: '"quoted"' },
        { t: "text", v: "" },
        { t: "text", v: '"a\tb" and "c"' },
        { t: "int", v: 3 },
      ],
    ];
    const cols = ["a", "b", "c", "d"].map((n) => col(n, "text"));
    const text = formatCells("tsv", cols, tricky);
    expect(parseTsv(text)).toEqual([
      ["a\tb", 'say "hi"', "line1\nline2\r\nline3", ""],
      ['"quoted"', "", '"a\tb" and "c"', "3"],
    ]);
    const withHeader = formatCells("tsv", cols, tricky, { headers: true });
    expect(parseTsv(withHeader)[0]).toEqual(["a", "b", "c", "d"]);
  });

  it("accepts CRLF and drops one trailing line break", () => {
    expect(parseTsv("1\tx\r\n2\ty\r\n")).toEqual([
      ["1", "x"],
      ["2", "y"],
    ]);
    expect(parseTsv("a\nb\n")).toEqual([["a"], ["b"]]);
    expect(parseTsv("a\n\n")).toEqual([["a"], [""]]);
    expect(parseTsv("a\t")).toEqual([["a", ""]]);
  });

  it("reads a copied single NULL / empty cell back as one empty field", () => {
    const cols = [col("a", "int")];
    for (const v of [{ t: "null" }, { t: "text", v: "" }] as Value[]) {
      const text = formatCells("tsv", cols, [[v]]);
      expect(text).toBe("");
      expect(parseTsv(text)).toEqual([[""]]);
    }
    expect(parseTsv("\n")).toEqual([[""]]);
    expect(parseTsv("\r\n")).toEqual([[""]]);
  });

  it("keeps fields that only look quoted", () => {
    expect(parseTsv('"a"b\t5" screen')).toEqual([['"a"b', '5" screen']]);
    expect(parseTsv('"unterminated\tx')).toEqual([['"unterminated', "x"]]);
  });
});

describe("parseCsvPaste", () => {
  it("splits on the delimiter, with CRLF, LF or CR line breaks and one trailing break dropped", () => {
    expect(parseCsvPaste("a,b\r\n1,2\r\n")).toEqual([
      ["a", "b"],
      ["1", "2"],
    ]);
    expect(parseCsvPaste("a;b\n1;2", ";")).toEqual([
      ["a", "b"],
      ["1", "2"],
    ]);
    expect(parseCsvPaste("a,b\r1,2")).toEqual([
      ["a", "b"],
      ["1", "2"],
    ]);
    // Empty fields, a trailing delimiter, an empty text.
    expect(parseCsvPaste("a,,c,")).toEqual([["a", "", "c", ""]]);
    expect(parseCsvPaste("")).toEqual([[""]]);
  });

  it("reads quoted fields: delimiters, doubled quotes and line breaks inside", () => {
    expect(parseCsvPaste('1,"Smith, John","say ""hi""","two\nlines"\n2,x,y,z')).toEqual([
      ["1", "Smith, John", 'say "hi"', "two\nlines"],
      ["2", "x", "y", "z"],
    ]);
    expect(parseCsvPaste('"a;b";c', ";")).toEqual([["a;b", "c"]]);
    expect(parseCsvPaste("'it''s',b", ",", "'")).toEqual([["it's", "b"]]);
    // A quoted field that ends the text, and an empty quoted field.
    expect(parseCsvPaste('a,"b"')).toEqual([["a", "b"]]);
    expect(parseCsvPaste('a,"",c')).toEqual([["a", "", "c"]]);
    // CRLF inside quotes stays.
    expect(parseCsvPaste('"x\r\ny",z')).toEqual([["x\r\ny", "z"]]);
  });

  it("keeps fields that only look quoted, as parseTsv does", () => {
    expect(parseCsvPaste('"a"b,c')).toEqual([['"a"b', "c"]]);
    expect(parseCsvPaste('say "hi",c')).toEqual([['say "hi"', "c"]]);
    // An unterminated quote is plain text.
    expect(parseCsvPaste('"oops,c')).toEqual([['"oops', "c"]]);
  });

  it("round-trips Copy as CSV (formatCells) with its quoting", () => {
    const text = formatCells("csv", columns, rows);
    expect(parseCsvPaste(text)).toEqual([
      ["1", "a\tb", "9007199254740993", '{"x":1}'],
      ["2", "", "5", ""],
    ]);
    const semicolon = formatCells("csv", columns, rows, {
      csv: { ...DEFAULT_CSV_OPTIONS, delimiter: ";", quote: "double", lineBreak: "\r\n" },
    });
    expect(parseCsvPaste(semicolon, ";")[0]).toEqual(["1", "a\tb", "9007199254740993", '{"x":1}']);
  });
});

describe("sniffCsvPaste", () => {
  it("reads comma and semicolon separated lines of one width, the first line being data", () => {
    expect(sniffCsvPaste("a,b\n1,2", 5)).toEqual([
      ["a", "b"],
      ["1", "2"],
    ]);
    expect(sniffCsvPaste("a;b\n1;2\n", 5)).toEqual([
      ["a", "b"],
      ["1", "2"],
    ]);
    expect(sniffCsvPaste('1,"Smith, John",x\n2,Doe,y', 3)).toEqual([
      ["1", "Smith, John", "x"],
      ["2", "Doe", "y"],
    ]);
    // Quoted line breaks do not split the line.
    expect(sniffCsvPaste('a,"x\ny"\nb,z', 2)).toEqual([
      ["a", "x\ny"],
      ["b", "z"],
    ]);
  });

  it("rejects ragged lines, a single value with a comma and text without a delimiter", () => {
    expect(sniffCsvPaste("a,b\n1,2,3", 5)).toBeNull();
    expect(sniffCsvPaste("a,b\n1", 5)).toBeNull();
    expect(sniffCsvPaste("a,b\n\n1,2", 5)).toBeNull();
    // A lone text value with a comma stays one value, even when the columns would fit.
    expect(sniffCsvPaste("Smith, John", 5)).toBeNull();
    expect(sniffCsvPaste("Smith, John", 2)).toBeNull();
    expect(sniffCsvPaste("Smith,John", 5)).toBeNull();
    expect(sniffCsvPaste("hello world", 2)).toBeNull();
    expect(sniffCsvPaste("justone", 2)).toBeNull();
    expect(sniffCsvPaste("", 2)).toBeNull();
    // One column per line is no CSV either.
    expect(sniffCsvPaste("a\nb\nc", 3)).toBeNull();
  });

  it("takes a single line only when it has one field per visible column", () => {
    expect(sniffCsvPaste("1,Ann,ann@x.io", 3)).toEqual([["1", "Ann", "ann@x.io"]]);
    expect(sniffCsvPaste("1;Ann;ann@x.io", 3)).toEqual([["1", "Ann", "ann@x.io"]]);
    expect(sniffCsvPaste("1,Ann,ann@x.io", 4)).toBeNull();
    expect(sniffCsvPaste("1,Ann,ann@x.io", 2)).toBeNull();
  });

  it("leaves tab separated text to the TSV parser", () => {
    expect(sniffCsvPaste("a,b\tc\n1,2\t3", 2)).toBeNull();
    expect(sniffCsvPaste("a\tb\n1\t2", 2)).toBeNull();
  });

  it("tries the CSV default options' delimiter first, and the split that fits the columns", () => {
    // Decimal commas: ';' gives the two columns of the grid, ',' would give three.
    expect(sniffCsvPaste("1,5;2,5\n3,5;4,5", 2)).toEqual([
      ["1,5", "2,5"],
      ["3,5", "4,5"],
    ]);
    expect(sniffCsvPaste("1,5;2,5\n3,5;4,5", 3)).toEqual([
      ["1", "5;2", "5"],
      ["3", "5;4", "5"],
    ]);
    // Both split the same width: the preference's delimiter wins.
    expect(sniffCsvPaste("a,b;c\nd,e;f", 9, { delimiter: ";", quote: "ifNeeded" })).toEqual([
      ["a,b", "c"],
      ["d,e", "f"],
    ]);
    expect(sniffCsvPaste("a,b;c\nd,e;f", 9, { delimiter: ",", quote: "ifNeeded" })).toEqual([
      ["a", "b;c"],
      ["d", "e;f"],
    ]);
    // A tab preference leaves ',' and ';' to the defaults.
    expect(sniffCsvPaste("a,b\n1,2", 2, { delimiter: "\t", quote: "ifNeeded" })).toEqual([
      ["a", "b"],
      ["1", "2"],
    ]);
  });

  it("reads single-quoted fields when the quote option says so", () => {
    expect(sniffCsvPaste("'a,b',c\n'd',e", 2, { delimiter: ",", quote: "single" })).toEqual([
      ["a,b", "c"],
      ["d", "e"],
    ]);
    // With double quotes only, the first line is three fields and the second two: ragged.
    expect(sniffCsvPaste("'a,b',c\n'd',e", 2)).toBeNull();
  });

  it("leaves what Tabler copied as plain text alone: a column of 'Last, First' names is one column", () => {
    resetCopyMarkersForTests();
    const names: Value[][] = [[{ t: "text", v: "Smith, John" }], [{ t: "text", v: "Doe, Jane" }]];
    const text = formatCells("tsv", [col("name", "text")], names);
    expect(text).toBe("Smith, John\nDoe, Jane");
    // Text from elsewhere would read as two columns...
    expect(sniffCsvPaste(text, 3)).not.toBeNull();
    // ...but Tabler remembers it copied this one.
    rememberCopy(text, names);
    expect(sniffCsvPaste(text, 3)).toBeNull();
    resetCopyMarkersForTests();
  });
});

describe("Copy as Raw", () => {
  it("writes values as stored: nothing quoted, NULL empty", () => {
    expect(formatCells("raw", columns, rows)).toBe('1\ta\tb\t9007199254740993\t{"x":1}\n2\t\t5\t');
  });
});

describe("copy markers (NULL vs EMPTY through ⌘C / ⌘V)", () => {
  it("remembers which copied fields were NULL or DEFAULT, for that text only", () => {
    resetCopyMarkersForTests();
    const copied: Value[][] = [
      [{ t: "null" }, { t: "text", v: "" }],
      [{ t: "default" }, { t: "int", v: 1 }],
    ];
    const text = "\t\n\t1";
    rememberCopy(text, copied);
    expect(copiedMarkers(text)).toEqual(
      new Map([
        ["0:0", "null"],
        ["1:0", "default"],
      ]),
    );
    expect(copiedMarkers("something else")).toBeNull();
    // Another window: only localStorage knows.
    expect(JSON.parse(localStorage.getItem("tabler.clipboard.markers")!).marks).toHaveLength(2);
    // A copy without NULLs forgets the markers.
    rememberCopy("a\tb", [
      [
        { t: "text", v: "a" },
        { t: "text", v: "b" },
      ],
    ]);
    expect(copiedMarkers(text)).toBeNull();
    expect(localStorage.getItem("tabler.clipboard.markers")).toBeNull();
    resetCopyMarkersForTests();
  });
});

describe("pasting a copied column that ends in NULL or empty cells", () => {
  const one = [col("v", "text")];
  const text = (v: string | null): Value => (v === null ? { t: "null" } : { t: "text", v });
  /** ⌘C of a single column, as the grid does it. */
  function copyColumn(values: Array<string | null>): string {
    resetCopyMarkersForTests();
    const block = values.map((v) => [text(v)]);
    const copied = formatCells("tsv", one, block);
    rememberCopy(copied, block);
    return copied;
  }

  it("gives back the last row that parseTsv reads as the terminator", () => {
    const cases: Array<[Array<string | null>, string[][]]> = [
      [
        ["x", "y", null],
        [["x"], ["y"], [""]],
      ],
      [
        ["x", null, null],
        [["x"], [""], [""]],
      ],
      [
        [null, null],
        [[""], [""]],
      ],
      [
        ["x", ""],
        [["x"], [""]],
      ],
      [
        ["x", "y", ""],
        [["x"], ["y"], [""]],
      ],
    ];
    for (const [values, rows] of cases) {
      const copied = copyColumn(values);
      expect(parseTsvPaste(copied), JSON.stringify(values)).toEqual(rows);
      // The clipboard text itself is unchanged: other apps read it.
      expect(copied).toBe(values.map((v) => v ?? "").join("\n"));
    }
    // Which cells were NULL stays keyed by the copied row.
    copyColumn(["x", "y", null]);
    expect(copiedMarkers("x\ny\n")).toEqual(new Map([["2:0", "null"]]));
  });

  it("parses text that Tabler did not copy as before", () => {
    copyColumn(["x", "y", null]);
    // Same shape from elsewhere (or after another copy): the old reading.
    expect(parseTsvPaste("x\ny\nz\n")).toEqual(parseTsv("x\ny\nz\n"));
    expect(parseTsv("x\ny\n")).toEqual([["x"], ["y"]]);
    resetCopyMarkersForTests();
    expect(parseTsvPaste("x\ny\n")).toEqual([["x"], ["y"]]);
    // Another copy replaces what was remembered.
    copyColumn(["x", "y", "z"]);
    expect(parseTsvPaste("x\ny\n")).toEqual([["x"], ["y"]]);
    expect(parseTsvPaste("x\ny\nz")).toEqual([["x"], ["y"], ["z"]]);
  });

  it("changes nothing when no row is lost", () => {
    // A last cell with a value, one row, or several columns (the last row keeps its tab).
    for (const values of [["x", "y"], [null, "y"], [null], [""], ["x"]]) {
      expect(parseTsvPaste(copyColumn(values))).toEqual(parseTsv(copyColumn(values)));
    }
    resetCopyMarkersForTests();
    const two = [col("a", "text"), col("b", "text")];
    const block: Value[][] = [
      [text("x"), text("y")],
      [text("z"), text(null)],
    ];
    const copied = formatCells("tsv", two, block);
    rememberCopy(copied, block);
    expect(parseTsvPaste(copied)).toEqual([
      ["x", "y"],
      ["z", ""],
    ]);
    // A cell holding a line break renders quoted: not empty.
    expect(parseTsvPaste(copyColumn(["x", "a\n"]))).toEqual([["x"], ["a\n"]]);
    resetCopyMarkersForTests();
  });

  it("is found by another window through localStorage", async () => {
    const copied = copyColumn(["x", null]);
    expect(JSON.parse(localStorage.getItem("tabler.clipboard.markers")!).trailingEmptyRow).toBe(
      true,
    );
    // Another window runs its own copy of the module: only localStorage is shared.
    vi.resetModules();
    const other = await import("./clipboard");
    expect(other.parseTsvPaste(copied)).toEqual([["x"], [""]]);
    expect(other.copiedMarkers(copied)).toEqual(new Map([["1:0", "null"]]));
    resetCopyMarkersForTests();
  });
});
