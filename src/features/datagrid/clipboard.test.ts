import { describe, expect, it } from "vitest";
import type { Value } from "@/lib/ipc";
import { formatCells, formatColumnValues, parseTsv } from "./clipboard";
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
    expect(formatCells("csv", columns, rows, { headers: true }).split("\r\n")[0]).toBe(
      "id,name,big,j",
    );
    expect(formatCells("csv", columns, rows)).toBe('1,a\tb,9007199254740993,"{""x"":1}"\r\n2,,5,');
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
  it("insert quotes per dialect", () => {
    const pg = formatCells("insert", columns, rows, {
      dialect: "postgres",
      schema: "public",
      table: "t",
    });
    expect(pg.split("\n")[0]).toBe(
      `INSERT INTO "public"."t" ("id", "name", "big", "j") VALUES (1, 'a\tb', 9007199254740993, '{"x":1}');`,
    );
    const my = formatCells("insert", columns, rows, { dialect: "mysql", schema: null, table: "t" });
    expect(my.split("\n")[1]).toBe(
      "INSERT INTO `t` (`id`, `name`, `big`, `j`) VALUES (2, NULL, 5, NULL);",
    );
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
