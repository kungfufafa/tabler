import { describe, expect, it } from "vitest";
import type { RoutineSummary, TableSummary } from "@/lib/ipc";
import type { SidebarGroup } from "./groups";
import { buildLazyRows, nodeKey, type LazyNode } from "./lazyTree";
import { nodeSchemaOfRow } from "./useSchemaTree";

function table(name: string, schema: string, kind: TableSummary["kind"] = "table"): TableSummary {
  return { schema, name, kind, rowEstimate: null, sizeBytes: null, comment: null };
}

function routine(name: string, schema: string): RoutineSummary {
  return {
    schema,
    name,
    kind: "function",
    arguments: "",
    identityArguments: "",
    returnType: "integer",
    language: "sql",
    comment: null,
  };
}

function node(schema: string, o: Partial<LazyNode> = {}): LazyNode {
  return {
    schema,
    system: false,
    expanded: false,
    tables: undefined,
    loading: false,
    error: null,
    ...o,
  };
}

const PUBLIC = [table("users", "public"), table("posts", "public"), table("v", "public", "view")];
const keys = (rows: ReturnType<typeof buildLazyRows>) => rows.map((r) => r.key);

describe("buildLazyRows", () => {
  it("lists one node per schema; an expanded one lists its sections under its prefix", () => {
    const rows = buildLazyRows(
      [
        node("public", { expanded: true, tables: PUBLIC, routines: [routine("add", "public")] }),
        node("sales"),
      ],
      { search: "" },
    );
    expect(keys(rows)).toEqual([
      "n:public",
      "n:public:h:Functions",
      "n:public:f:public.add()",
      "n:public:h:Tables",
      "n:public:t:public.users",
      "n:public:t:public.posts",
      "n:public:h:Views",
      "n:public:t:public.v",
      "n:sales",
    ]);
    expect(rows[0]).toMatchObject({ type: "schemaNode", schema: "public", count: 4 });
    // A node not loaded has no count yet.
    expect(rows.at(-1)).toMatchObject({ type: "schemaNode", schema: "sales", count: null });
    // Section headers know their schema (Item Overview, New ▸ …).
    expect(rows[3]).toMatchObject({ type: "header", label: "Tables", schema: "public" });
    expect(nodeSchemaOfRow(rows[4]!.key)).toBe("public");
  });

  it("shows a loading or error row in an expanded node until its items are in", () => {
    const loading = buildLazyRows([node("sales", { expanded: true, loading: true })], {
      search: "",
    });
    expect(loading).toEqual([
      expect.objectContaining({ type: "schemaNode", loading: true }),
      {
        type: "nodeStatus",
        key: "n:sales#status",
        parentKey: "n:sales",
        status: "loading",
        message: null,
      },
    ]);
    const failed = buildLazyRows([node("sales", { expanded: true, error: "permission denied" })], {
      search: "",
    });
    expect(failed[1]).toMatchObject({ status: "error", message: "permission denied" });
  });

  it("keeps a collapsed node's count and leaves its rows out", () => {
    const rows = buildLazyRows([node("public", { tables: PUBLIC })], { search: "" });
    expect(rows).toEqual([
      {
        type: "schemaNode",
        key: nodeKey("public"),
        schema: "public",
        expanded: false,
        count: 3,
        loading: false,
        system: false,
      },
    ]);
  });

  it("while searching, drops loaded nodes without a match and keeps unloaded ones", () => {
    const rows = buildLazyRows(
      [
        node("public", { expanded: true, tables: PUBLIC }),
        node("sales", { expanded: true, tables: [table("orders", "sales")] }),
        node("hr"),
      ],
      { search: "ord" },
    );
    expect(keys(rows)).toEqual(["n:sales", "n:sales:h:Tables", "n:sales:t:sales.orders", "n:hr"]);
    expect(rows[0]).toMatchObject({ count: 1 });
    // The Matching Strategy applies.
    const prefix = buildLazyRows([node("public", { expanded: true, tables: PUBLIC })], {
      search: "sts",
      strategy: "prefix",
    });
    expect(prefix).toEqual([]);
  });

  it("lists Recent from the loaded nodes, and pins and groups inside each node", () => {
    const manual: SidebarGroup = {
      id: "m",
      name: "Mine",
      rules: null,
      members: ["t:sales.orders"],
      collapsed: false,
    };
    const empty: SidebarGroup = { ...manual, id: "e", name: "Empty", members: [] };
    const rows = buildLazyRows(
      [
        node("public", { expanded: true, tables: PUBLIC }),
        node("sales", { expanded: true, tables: [table("orders", "sales"), table("x", "sales")] }),
      ],
      {
        search: "",
        recent: ["sales.orders", "gone.t"],
        pinned: new Set(["public.posts"]),
        groups: [manual, empty],
      },
    );
    expect(keys(rows)).toEqual([
      "h:Recent",
      "r:sales.orders",
      "n:public",
      "n:public:h:Pinned",
      "n:public:p:public.posts",
      "n:public:h:Tables",
      "n:public:t:public.users",
      "n:public:h:Views",
      "n:public:t:public.v",
      "n:sales",
      // A group shows only in the node that holds its items.
      "n:sales:g:m",
      "n:sales:g:m:t:sales.orders",
      "n:sales:h:Tables",
      "n:sales:t:sales.x",
    ]);
    expect(rows.find((r) => r.key === "n:sales")).toMatchObject({ count: 2 });
  });

  it("encodes schema names in the row keys", () => {
    const rows = buildLazyRows([node("a:b", { expanded: true, tables: [table("t", "a:b")] })], {
      search: "",
    });
    expect(keys(rows)).toEqual(["n:a%3Ab", "n:a%3Ab:h:Tables", "n:a%3Ab:t:a:b.t"]);
    expect(nodeSchemaOfRow(rows[2]!.key)).toBe("a:b");
  });
});
