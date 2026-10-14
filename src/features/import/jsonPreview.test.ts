import { describe, expect, it } from "vitest";
import type { JsonPreview } from "@/lib/ipc";
import { fromJsonPreview, layoutLabel } from "./jsonPreview";
import { statusLine, tableNameFromFile } from "./mapping";

describe("JSON in the import wizard", () => {
  it("shapes a JSON preview like a CSV one, keeping its keys", () => {
    const json: JsonPreview = {
      path: "/x/people.json",
      fileName: "people.json",
      size: 120,
      encoding: "utf8",
      layout: "array",
      keys: ["", "name"],
      columns: [
        {
          name: "column_1",
          detected: { name: "column_1", ty: "integer", precision: null, scale: null },
        },
        { name: "name", detected: { name: "name", ty: "text", precision: null, scale: null } },
      ],
      rows: [["1", null]],
      sampledRows: 1,
      more: false,
    };
    const p = fromJsonPreview(json);
    expect(p.keys).toEqual(["", "name"]);
    expect(p.columns.map((c) => c.name)).toEqual(["column_1", "name"]);
    expect(p.delimiter).toBe("");
    expect(p.rows).toEqual([["1", null]]);
    expect(layoutLabel("lines")).toBe("one object per line");
    expect(layoutLabel(null)).toBeNull();
  });

  it("words the status line and the new table's name for JSON", () => {
    expect(statusLine(3, 4, 2, "JSON")).toBe("JSON 3 keys, table 4 columns, matched 2 columns");
    expect(statusLine(1, 1, 1)).toBe("CSV 1 column, table 1 column, matched 1 column");
    expect(tableNameFromFile("Sales 2026.json")).toBe("sales_2026");
  });
});
