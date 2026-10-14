import { describe, expect, it } from "vitest";
import type { Value } from "@/lib/ipc";
import { formatCells, formatColumnValues, INSERT_CHUNK_BYTES, parseTsv } from "./clipboard";
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
