import { describe, expect, it } from "vitest";
import type { Value } from "@/lib/ipc";
import { formatCells } from "./clipboard";
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
  it("csv includes a header", () => {
    expect(formatCells("csv", columns, rows).split("\r\n")[0]).toBe("id,name,big,j");
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
