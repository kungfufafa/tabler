import { describe, expect, it } from "vitest";
import type { RoutineSummary, SequenceSummary, TableSummary } from "@/lib/ipc";
import { buildTreeRows, mergeObjectRows } from "./useSchemaTree";

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

function routine(
  name: string,
  kind: RoutineSummary["kind"] = "function",
  args = "",
): RoutineSummary {
  return {
    schema: "public",
    name,
    kind,
    arguments: args,
    identityArguments: args,
    returnType: kind === "procedure" ? null : "integer",
    language: "sql",
    comment: null,
  };
}

function sequence(name: string): SequenceSummary {
  return { schema: "public", name, dataType: "bigint", ownedBy: null, comment: null };
}

describe("buildTreeRows search syntax", () => {
  it("supports anchors, alternatives and exact quotes", () => {
    expect(keys(buildTreeRows(tables, "^users$"))).toEqual(["h:Tables", "t:public.users"]);
    expect(keys(buildTreeRows(tables, "^posts$ | orders"))).toEqual([
      "h:Tables",
      "t:public.posts",
      "t:inventory.orders",
    ]);
    expect(keys(buildTreeRows(tables, '"r_s"'))).toEqual(["h:Views", "t:public.user_stats"]);
    // Fuzzy by default.
    expect(keys(buildTreeRows(tables, "pst"))).toEqual(["h:Tables", "t:public.posts"]);
  });
});

describe("mergeObjectRows", () => {
  const routines = [
    routine("add", "function", "a integer, b integer"),
    routine("add", "function", "a numeric, b numeric"),
    routine("archive_posts", "procedure"),
  ];
  const sequences = [sequence("users_id_seq")];

  it("puts Functions before the tables and Sequences last", () => {
    const pinned = new Set(["public.posts"]);
    const rows = mergeObjectRows(buildTreeRows(tables, "", pinned), routines, sequences, "");
    expect(keys(rows)).toEqual([
      "h:Pinned",
      "p:public.posts",
      "h:Functions",
      "f:public.add(a integer, b integer)",
      "f:public.add(a numeric, b numeric)",
      "f:public.archive_posts()",
      "h:Tables",
      "t:public.users",
      "t:public.posts",
      "t:inventory.orders",
      "h:Views",
      "t:public.user_stats",
      "h:Sequences",
      "s:public.users_id_seq",
    ]);
    expect(new Set(keys(rows)).size).toBe(rows.length);
  });

  it("filters with the search and collapses like the other sections", () => {
    const rows = mergeObjectRows(buildTreeRows(tables, "arch"), routines, sequences, "arch");
    expect(keys(rows)).toEqual(["h:Functions", "f:public.archive_posts()"]);
    const collapsed = new Set(["Functions"] as const);
    const folded = mergeObjectRows(
      buildTreeRows(tables, "", undefined, undefined, collapsed),
      routines,
      undefined,
      "",
      collapsed,
    );
    expect(folded[0]).toEqual({
      type: "header",
      key: "h:Functions",
      label: "Functions",
      count: 3,
      collapsed: true,
    });
    expect(folded[1]?.key).toBe("h:Tables");
  });

  it("adds nothing for turned-off or empty sections", () => {
    const base = buildTreeRows(tables, "");
    expect(mergeObjectRows(base, undefined, undefined, "")).toBe(base);
    expect(mergeObjectRows(base, [], [], "")).toBe(base);
    // No tables at all: the routines still show.
    expect(keys(mergeObjectRows([], routines.slice(2), undefined, ""))).toEqual([
      "h:Functions",
      "f:public.archive_posts()",
    ]);
  });
});
