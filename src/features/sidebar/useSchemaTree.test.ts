import { describe, expect, it } from "vitest";
import type { TableSummary } from "@/lib/ipc";
import { buildTreeRows } from "./useSchemaTree";

function table(
  name: string,
  kind: TableSummary["kind"] = "table",
  schema = "public",
): TableSummary {
  return { schema, name, kind, rowEstimate: null, sizeBytes: null, comment: null };
}

const tables = [
  table("users"),
  table("posts"),
  table("user_stats", "view"),
  table("orders", "table", "inventory"),
];
const keys = (rows: ReturnType<typeof buildTreeRows>) => rows.map((r) => r.key);

describe("buildTreeRows", () => {
  it("groups by kind and filters by search", () => {
    expect(keys(buildTreeRows(tables, ""))).toEqual([
      "h:Tables",
      "t:public.users",
      "t:public.posts",
      "t:inventory.orders",
      "h:Views",
      "t:public.user_stats",
    ]);
    expect(keys(buildTreeRows(tables, " USER "))).toEqual([
      "h:Tables",
      "t:public.users",
      "h:Views",
      "t:public.user_stats",
    ]);
  });

  it("lists matching pinned tables first and keeps them in their group", () => {
    const pinned = new Set(["public.posts", "public.user_stats", "inventory.orders", "x.gone"]);
    const rows = buildTreeRows(tables, "", pinned);
    expect(keys(rows)).toEqual([
      "h:Pinned",
      "p:public.posts",
      "p:public.user_stats",
      "p:inventory.orders",
      "h:Tables",
      "t:public.users",
      "t:public.posts",
      "t:inventory.orders",
      "h:Views",
      "t:public.user_stats",
    ]);
    expect(rows[0]).toMatchObject({ type: "header", label: "Pinned", count: 3 });
  });

  it("applies the search to pinned tables and drops the section when none match", () => {
    const pinned = new Set(["public.posts", "public.user_stats"]);
    expect(keys(buildTreeRows(tables, "user", pinned))).toEqual([
      "h:Pinned",
      "p:public.user_stats",
      "h:Tables",
      "t:public.users",
      "h:Views",
      "t:public.user_stats",
    ]);
    expect(keys(buildTreeRows(tables, "order", pinned))).toEqual([
      "h:Tables",
      "t:inventory.orders",
    ]);
  });

  it("lists recently opened tables first, most recent first, above the pinned ones", () => {
    const pinned = new Set(["public.posts"]);
    const recent = ["public.user_stats", "x.gone", "public.posts"];
    const rows = buildTreeRows(tables, "", pinned, recent);
    expect(keys(rows)).toEqual([
      "h:Recent",
      "r:public.user_stats",
      "r:public.posts",
      "h:Pinned",
      "p:public.posts",
      "h:Tables",
      "t:public.users",
      "t:public.posts",
      "t:inventory.orders",
      "h:Views",
      "t:public.user_stats",
    ]);
    expect(rows[0]).toMatchObject({ type: "header", label: "Recent", count: 2 });
    expect(new Set(keys(rows)).size).toBe(rows.length);
  });

  it("hides the recent section while searching or when none of its tables are listed", () => {
    const recent = ["public.users"];
    expect(keys(buildTreeRows(tables, "user", undefined, recent))).toEqual([
      "h:Tables",
      "t:public.users",
      "h:Views",
      "t:public.user_stats",
    ]);
    expect(keys(buildTreeRows(tables, "  ", undefined, recent)).slice(0, 2)).toEqual([
      "h:Recent",
      "r:public.users",
    ]);
    expect(keys(buildTreeRows(tables, "", undefined, ["x.gone"]))[0]).toBe("h:Tables");
  });

  it("keeps the pinned section apart from same-named tables of another schema", () => {
    const rows = buildTreeRows(
      [table("t", "table", "a"), table("t", "table", "b")],
      "",
      new Set(["b.t"]),
    );
    expect(keys(rows)).toEqual(["h:Pinned", "p:b.t", "h:Tables", "t:a.t", "t:b.t"]);
  });

  it("keeps a collapsed section's header and count but leaves its rows out", () => {
    const pinned = new Set(["public.posts"]);
    const recent = ["public.users"];
    const rows = buildTreeRows(tables, "", pinned, recent, new Set(["Recent", "Tables"]));
    expect(keys(rows)).toEqual([
      "h:Recent",
      "h:Pinned",
      "p:public.posts",
      "h:Tables",
      "h:Views",
      "t:public.user_stats",
    ]);
    expect(rows[0]).toMatchObject({ label: "Recent", count: 1, collapsed: true });
    expect(rows[1]).toMatchObject({ label: "Pinned", count: 1, collapsed: false });
    expect(rows[3]).toMatchObject({ label: "Tables", count: 3, collapsed: true });
    // The search still counts the matches of a collapsed section.
    expect(buildTreeRows(tables, "user", undefined, undefined, new Set(["Views"]))).toEqual([
      { type: "header", key: "h:Tables", label: "Tables", count: 1, collapsed: false },
      { type: "table", key: "t:public.users", table: tables[0] },
      { type: "header", key: "h:Views", label: "Views", count: 1, collapsed: true },
    ]);
  });
});
