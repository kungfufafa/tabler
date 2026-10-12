import { describe, expect, it } from "vitest";
import type { OverviewItem } from "@/lib/ipc";
import { commentEditable, formatBytes, nextSort, overviewColumns, sortOverview } from "./overview";

function item(name: string, extra: Partial<OverviewItem> = {}): OverviewItem {
  return {
    name,
    schema: "public",
    kind: "table",
    estimatedRows: null,
    totalSize: null,
    dataSize: null,
    indexSize: null,
    engine: null,
    charset: null,
    collation: null,
    owner: null,
    comment: null,
    ...extra,
  };
}

describe("item overview", () => {
  it("shows MySQL's engine / charset / collation and PostgreSQL's owner", () => {
    const ids = (d: "postgres" | "mysql" | "sqlite") => overviewColumns(d).map((c) => c.label);
    expect(ids("mysql")).toEqual([
      "name",
      "kind",
      "schema",
      "estimated_rows",
      "total_size",
      "data_size",
      "index_size",
      "engine",
      "charset",
      "collation",
      "comment",
    ]);
    expect(ids("postgres")).toContain("owner");
    expect(ids("postgres")).not.toContain("engine");
    expect(ids("sqlite")).not.toContain("owner");
  });

  it("sorts numbers numerically, empty cells last, ties by name", () => {
    const items = [
      item("b", { totalSize: 10 }),
      item("a", { totalSize: 9000 }),
      item("v", { kind: "view" }),
      item("c", { totalSize: 10 }),
    ];
    const names = (desc: boolean) =>
      sortOverview(items, { column: "totalSize", desc }).map((i) => i.name);
    expect(names(false)).toEqual(["b", "c", "a", "v"]);
    expect(names(true)).toEqual(["a", "b", "c", "v"]);
    expect(sortOverview(items, { column: "kind", desc: false }).map((i) => i.name)).toEqual([
      "a",
      "b",
      "c",
      "v",
    ]);
    expect(
      sortOverview([item("t10"), item("t9"), item("T1")], { column: "name", desc: false }).map(
        (i) => i.name,
      ),
    ).toEqual(["T1", "t9", "t10"]);
  });

  it("flips the direction on the same header, numbers start descending", () => {
    const [name, , , rows] = overviewColumns("postgres");
    const s = nextSort({ column: "name", desc: false }, name!);
    expect(s).toEqual({ column: "name", desc: true });
    expect(nextSort(s, rows!)).toEqual({ column: "estimatedRows", desc: true });
  });

  it("formats sizes", () => {
    expect(formatBytes(null)).toBe("");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(8192)).toBe("8 KB");
    expect(formatBytes(250 * 1024 * 1024)).toBe("250 MB");
    expect(formatBytes(3 * 1024 ** 3)).toBe("3 GB");
  });

  it("allows comments where the server keeps them", () => {
    expect(commentEditable("postgres", item("v", { kind: "view" }))).toBe(true);
    expect(commentEditable("mysql", item("t"))).toBe(true);
    expect(commentEditable("mysql", item("v", { kind: "view" }))).toBe(false);
    expect(commentEditable("sqlite", item("t"))).toBe(false);
    expect(commentEditable("postgres", item("s", { kind: "systemView" }))).toBe(false);
  });
});
