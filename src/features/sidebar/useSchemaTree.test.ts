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

  it("keeps the pinned section apart from same-named tables of another schema", () => {
    const rows = buildTreeRows(
      [table("t", "table", "a"), table("t", "table", "b")],
      "",
      new Set(["b.t"]),
    );
    expect(keys(rows)).toEqual(["h:Pinned", "p:b.t", "h:Tables", "t:a.t", "t:b.t"]);
  });
});
