import { describe, expect, it } from "vitest";
import type { RoutineSummary, SequenceSummary, TableMeta, TableSummary } from "@/lib/ipc";
import type { SidebarGroup } from "./groups";
import {
  buildTreeRows,
  groupIdOfRow,
  isItemRow,
  mergeObjectRows,
  nodePrefix,
  nodeSchemaOfRow,
  pruneEmptyGroups,
  withColumnRows,
  type ColumnState,
} from "./useSchemaTree";

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

  it("lists matching pinned tables first; they leave the alphabetical sections", () => {
    const pinned = new Set(["public.posts", "public.user_stats", "inventory.orders", "x.gone"]);
    const rows = buildTreeRows(tables, "", pinned);
    expect(keys(rows)).toEqual([
      "h:Pinned",
      "p:public.posts",
      "p:public.user_stats",
      "p:inventory.orders",
      "h:Tables",
      "t:public.users",
    ]);
    expect(rows[0]).toMatchObject({ type: "header", label: "Pinned", count: 3 });
    expect(rows[4]).toMatchObject({ type: "header", label: "Tables", count: 1 });
  });

  it("applies the search to pinned tables and drops the section when none match", () => {
    const pinned = new Set(["public.posts", "public.user_stats"]);
    expect(keys(buildTreeRows(tables, "user", pinned))).toEqual([
      "h:Pinned",
      "p:public.user_stats",
      "h:Tables",
      "t:public.users",
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
    expect(keys(rows)).toEqual(["h:Pinned", "p:b.t", "h:Tables", "t:a.t"]);
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
    expect(rows[3]).toMatchObject({ label: "Tables", count: 2, collapsed: true });
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

  it("matches with the search menu's Matching Strategy", () => {
    const build = (q: string, strategy: "contains" | "prefix" | "regex") =>
      keys(buildTreeRows(tables, q, undefined, undefined, undefined, undefined, strategy));
    expect(build("pst", "contains")).toEqual([]);
    expect(build("ost", "contains")).toEqual(["h:Tables", "t:public.posts"]);
    expect(build("ost", "prefix")).toEqual([]);
    expect(build("ord", "prefix")).toEqual(["h:Tables", "t:inventory.orders"]);
    expect(build("^(users|orders)$", "regex")).toEqual([
      "h:Tables",
      "t:public.users",
      "t:inventory.orders",
    ]);
    // The same strategy filters the routines, sequences and groups.
    const rows = mergeObjectRows(
      [],
      [routine("slugify"), routine("add")],
      [sequence("users_id_seq")],
      "^s",
      undefined,
      undefined,
      "regex",
    );
    expect(keys(rows)).toEqual(["h:Functions", "f:public.slugify()"]);
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

describe("sidebar groups in the tree", () => {
  const views: SidebarGroup = {
    id: "v",
    name: "Views",
    rules: [{ field: "kind", value: "view" }],
    members: [],
    collapsed: false,
  };
  const manual: SidebarGroup = {
    id: "m",
    name: "Mine",
    rules: null,
    members: ["t:public.posts", "f:public.slugify(value text)"],
    collapsed: false,
  };
  const empty: SidebarGroup = { ...manual, id: "e", name: "Empty", members: [] };
  const fn = (name: string, args = ""): RoutineSummary => ({
    schema: "public",
    name,
    kind: "function",
    arguments: args,
    identityArguments: args,
    returnType: "text",
    language: "sql",
    comment: null,
  });

  it("lists groups (even empty ones) above Tables; grouped items leave their section", () => {
    const rows = buildTreeRows(tables, "", undefined, undefined, undefined, [views, manual, empty]);
    expect(keys(rows)).toEqual([
      "g:v",
      "g:v:t:public.user_stats",
      "g:m",
      "g:m:t:public.posts",
      "g:e",
      "h:Tables",
      "t:public.users",
      "t:inventory.orders",
    ]);
    expect(
      rows.filter((r) => r.type === "groupHeader").map((r) => r.type === "groupHeader" && r.count),
    ).toEqual([1, 1, 0]);
    expect(rows.filter(isItemRow).map((r) => r.key)).not.toContain("g:v");
    expect(groupIdOfRow("g:m:t:public.posts")).toBe("m");
    expect(groupIdOfRow("t:public.posts")).toBeNull();
    // Inside a lazy-loaded schema node.
    expect(groupIdOfRow("n:public:g:m:t:public.posts")).toBe("m");
    expect(groupIdOfRow("n:public:t:public.posts")).toBeNull();
    expect(nodeSchemaOfRow("n:my%3Aschema:t:x.y")).toBe("my:schema");
    expect(nodeSchemaOfRow("t:public.posts")).toBeNull();
    expect(nodePrefix("my:schema")).toBe("n:my%3Aschema:");
    // Collapsed: the header (and its count) stays.
    const collapsed = buildTreeRows(tables, "", undefined, undefined, undefined, [
      { ...views, collapsed: true },
    ]);
    expect(keys(collapsed).slice(0, 2)).toEqual(["g:v", "h:Tables"]);
  });

  it("puts a group's routines after its tables and drops empty groups while searching", () => {
    const routines = [fn("slugify", "value text"), fn("add")];
    const rows = mergeObjectRows(
      buildTreeRows(tables, "", undefined, undefined, undefined, [manual, empty]),
      routines,
      undefined,
      "",
      undefined,
      [manual, empty],
    );
    expect(keys(rows)).toEqual([
      "g:m",
      "g:m:t:public.posts",
      "g:m:f:public.slugify(value text)",
      "g:e",
      "h:Functions",
      "f:public.add()",
      "h:Tables",
      "t:public.users",
      "t:inventory.orders",
      "h:Views",
      "t:public.user_stats",
    ]);
    expect(rows[0]).toMatchObject({ type: "groupHeader", count: 2 });

    const searched = pruneEmptyGroups(
      mergeObjectRows(
        buildTreeRows(tables, "slug", undefined, undefined, undefined, [manual, empty]),
        routines,
        undefined,
        "slug",
        undefined,
        [manual, empty],
      ),
      "slug",
    );
    expect(keys(searched)).toEqual(["g:m", "g:m:f:public.slugify(value text)"]);
    expect(pruneEmptyGroups(rows, "")).toBe(rows);
  });
});

describe("withColumnRows", () => {
  const meta = {
    table: { schema: "public", name: "posts" },
    kind: "table",
    columns: [
      { name: "title", ordinal: 3, dataType: "text", isPk: false },
      { name: "id", ordinal: 1, dataType: "bigint", isPk: true },
      { name: "user_id", ordinal: 2, dataType: "bigint", isPk: false },
    ],
    primaryKey: ["id"],
    foreignKeys: [{ columns: ["user_id"] }],
  } as unknown as TableMeta;

  it("lists the columns of expanded table rows only", () => {
    const rows = buildTreeRows(tables, "");
    const states: Record<string, ColumnState> = {
      posts: { status: "ready", meta },
      users: { status: "loading" },
      orders: { status: "error", message: "no access" },
    };
    const out = withColumnRows(
      rows,
      new Set(["t:public.posts", "t:public.users", "t:inventory.orders"]),
      (t) => states[t.name]!,
    );
    expect(keys(out)).toEqual([
      "h:Tables",
      "t:public.users",
      "t:public.users#status",
      "t:public.posts",
      "t:public.posts#c:id",
      "t:public.posts#c:user_id",
      "t:public.posts#c:title",
      "t:inventory.orders",
      "t:inventory.orders#status",
      "h:Views",
      "t:public.user_stats",
    ]);
    const cols = out.filter((r) => r.type === "column");
    expect(cols.map((c) => c.type === "column" && [c.name, c.dataType, c.pk, c.fk])).toEqual([
      ["id", "bigint", true, false],
      ["user_id", "bigint", false, true],
      ["title", "text", false, false],
    ]);
    expect(out.find((r) => r.key === "t:inventory.orders#status")).toMatchObject({
      status: "error",
      message: "no access",
    });
    // Columns are not items: ↑/↓ and the selection skip them.
    expect(out.filter(isItemRow)).toHaveLength(4);
    expect(withColumnRows(rows, new Set(), () => ({ status: "loading" }))).toBe(rows);
  });
});
