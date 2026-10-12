import { describe, expect, it, vi } from "vitest";
import type { ConnectionProfile, TableSummary } from "@/lib/ipc";
import { KEYMAP } from "@/lib/keys/keymap";
import type { Tab } from "@/stores/tabs";
import type { ActionId } from "./actions";
import { paletteCommandItems } from "./commands";
import {
  actionApplies,
  actionIcon,
  browseOrder,
  buildOpenAnything,
  buildSections,
  commandQuery,
  completionOf,
  connectionItems,
  databaseItems,
  flattenSections,
  isCommandQuery,
  isListedAction,
  keymapActionItems,
  rankStrict,
  RECENT_LIMIT,
  schemaItems,
  TABLE_LIMIT,
  tabItems,
  tableItemKey,
  tableItems,
  type PaletteGroupId,
  type PaletteItem,
} from "./items";

function item(key: string, label: string, group: PaletteGroupId = "actions"): PaletteItem {
  return {
    key,
    group,
    label,
    icon: { type: "tab", kind: "query" },
    run: () => {},
    verb: "Run",
  };
}

function table(
  name: string,
  rowEstimate: number | null = null,
  kind: TableSummary["kind"] = "table",
): TableSummary {
  return { name, schema: "public", kind, rowEstimate, sizeBytes: null, comment: null };
}

const keys = (items: { item: PaletteItem }[]) => items.map((r) => r.item.key);

describe("keymapActionItems", () => {
  it("lists only actions that currently have a handler, once each", () => {
    const has = (id: ActionId) =>
      ["tab.next", "ui.toggle-sidebar", "palette.open", "tab.goto-1"].includes(id);
    const run = vi.fn();
    const items = keymapActionItems(KEYMAP, has, run);
    expect(items.map((i) => i.key)).toEqual(["action:tab.next", "action:ui.toggle-sidebar"]);
    // The first binding is the one shown.
    expect(items[0]!.combo).toBe("Mod+]");
    expect(items[1]).toMatchObject({ label: "Toggle sidebar", combo: "Mod+0" });
    items[1]!.run();
    expect(run).toHaveBeenCalledWith("ui.toggle-sidebar");
  });

  it("does not list the palette's own openers", () => {
    const openers: ActionId[] = ["palette.open", "palette.databases", "palette.connections"];
    for (const id of openers) expect(isListedAction(id)).toBe(false);
    expect(keymapActionItems(KEYMAP, (id) => openers.includes(id), vi.fn())).toEqual([]);
  });

  it("gives every listed action an icon of its own", () => {
    const fallback = actionIcon("tab.goto-1");
    const missing = [...new Set(KEYMAP.map((d) => d.id))].filter(
      (id) => isListedAction(id) && actionIcon(id) === fallback,
    );
    expect(missing).toEqual([]);
  });

  it("hides everything when nothing is registered", () => {
    expect(keymapActionItems(KEYMAP, () => false, vi.fn())).toEqual([]);
  });

  it("hides tab actions that would do nothing", () => {
    const idle = { hasConnection: false, tabCount: 0, closedCount: 0 };
    expect(actionApplies("tab.new-query", idle)).toBe(false);
    expect(actionApplies("tab.close", idle)).toBe(false);
    expect(actionApplies("tab.reopen", idle)).toBe(false);
    expect(actionApplies("export.open", idle)).toBe(false);
    expect(actionApplies("ui.toggle-sidebar", idle)).toBe(true);
    const busy = { hasConnection: true, tabCount: 1, closedCount: 1 };
    expect(actionApplies("tab.new-query", busy)).toBe(true);
    expect(actionApplies("tab.close", busy)).toBe(true);
    expect(actionApplies("export.open", busy)).toBe(true);
    expect(actionApplies("tab.next", busy)).toBe(false);
    expect(actionApplies("tab.next", { ...busy, tabCount: 2 })).toBe(true);
  });
});

describe("buildSections (⌘K, ⇧⌘K)", () => {
  const groups = {
    tables: [item("t:users", "users", "tables"), item("t:orders", "orders", "tables")],
    tabs: [item("tab:1", "Query 1", "tabs")],
    connections: [] as PaletteItem[],
    actions: [item("a:sidebar", "Toggle sidebar"), item("a:close", "Close tab")],
  };

  it("keeps the fixed group order and drops empty groups", () => {
    const sections = buildSections(groups, "");
    expect(sections.map((s) => s.id)).toEqual(["tables", "tabs", "actions"]);
    expect(sections.map((s) => s.heading)).toEqual(["Tables", "Open tabs", "Actions"]);
  });

  it("keeps the natural order of an empty query, even with recently used items", () => {
    const sections = buildSections(groups, "", { mru: ["a:close", "t:orders"] });
    expect(flattenSections(sections).map((i) => i.key)).toEqual([
      "t:users",
      "t:orders",
      "tab:1",
      "a:sidebar",
      "a:close",
    ]);
  });

  it("filters with the fuzzy matcher and highlights labels", () => {
    const sections = buildSections(groups, "ord");
    expect(sections.map((s) => s.id)).toEqual(["tables"]);
    expect(sections[0]!.items[0]).toMatchObject({ item: { key: "t:orders" }, ranges: [[0, 3]] });
    expect(buildSections(groups, "zzz")).toEqual([]);
  });

  it("breaks score ties by recency", () => {
    const twins = [item("a:1", "Refresh"), item("a:2", "Refresh")];
    expect(keys(buildSections({ actions: twins }, "ref")[0]!.items)).toEqual(["a:1", "a:2"]);
    expect(keys(buildSections({ actions: twins }, "ref", { mru: ["a:2"] })[0]!.items)).toEqual([
      "a:2",
      "a:1",
    ]);
  });

  it("only counts detail and keyword hits that start words", () => {
    const theme = { ...item("a:1", "Theme: Light"), keywords: ["appearance color scheme"] };
    expect(buildSections({ actions: [theme] }, "pt")).toEqual([]);
    expect(buildSections({ actions: [theme] }, "colsch")).toHaveLength(1);
    const conn = { ...item("a:2", "Disconnect"), detail: "Local Postgres" };
    expect(buildSections({ actions: [conn] }, "lpg")).toEqual([]);
    expect(buildSections({ actions: [conn] }, "lpost")).toHaveLength(1);
  });

  it("matches details and keywords below label matches, without highlights", () => {
    const a = { ...item("a:1", "Disconnect"), detail: "Local Postgres" };
    const b = item("a:2", "Local backup");
    const [section] = buildSections({ actions: [a, b] }, "local");
    expect(keys(section!.items)).toEqual(["a:2", "a:1"]);
    expect(section!.items[1]!.ranges).toEqual([]);
  });

  it("follows the given group order", () => {
    const sections = buildSections(
      { databases: [item("db:app", "app", "databases")], schemas: [item("s:x", "x", "schemas")] },
      "",
      { mru: ["s:x"], order: ["databases", "schemas"] },
    );
    expect(sections.map((s) => s.id)).toEqual(["databases", "schemas"]);
    // Groups outside the order are not shown.
    expect(buildSections(groups, "", { order: ["tabs"] }).map((s) => s.id)).toEqual(["tabs"]);
  });

  it("puts a loading placeholder in the place of an empty group that is still loading", () => {
    const sections = buildSections({ ...groups, tables: [] }, "", { loading: ["tables"] });
    expect(sections.map((s) => [s.id, s.loading ?? null])).toEqual([
      ["tables", "tables"],
      ["tabs", null],
      ["actions", null],
    ]);
    expect(sections[0]!.items).toEqual([]);
    // Items that already arrived win over the placeholder.
    expect(buildSections(groups, "", { loading: ["tables"] })[0]).not.toHaveProperty("loading");
  });

  it("caps tables and reports how many more matched", () => {
    const many = Array.from({ length: TABLE_LIMIT + 50 }, (_, i) =>
      item(`t:${i}`, `table_${i}`, "tables"),
    );
    const [empty] = buildSections({ tables: many }, "");
    expect(empty!.items).toHaveLength(TABLE_LIMIT);
    expect(empty!.more).toBe(50);
    const [searched] = buildSections({ tables: many }, "table_1");
    const expected = many.filter((t) => /1/.test(t.label.slice("table_".length))).length;
    expect(searched!.items).toHaveLength(expected);
    expect(searched!.items[0]!.item.label).toBe("table_1");
    expect(searched!.more).toBe(0);
  });
});

describe("Open Anything (buildOpenAnything)", () => {
  const open = vi.fn();
  const relations = tableItems(
    [
      table("users"),
      table("posts"),
      table("post_tags"),
      table("post_counts"),
      table("order_items"),
      table("orders"),
      table("active_users", null, "view"),
      table("daily_totals", null, "materializedView"),
      table("repost_log"),
    ],
    { profileId: "p1", open },
  );
  const key = (name: string) => tableItemKey("p1", { schema: "public", name });
  const groups = {
    tables: relations,
    tabs: [{ ...item("tab:1", "orders", "tabs"), kind: "tab" }],
    schemas: [{ ...item("schema:public", "public", "schemas"), tags: ["schema"] }],
    databases: [{ ...item("db:postgres", "postgres", "databases"), noRecent: true }],
    connections: [item("conn:p1", "Local PG", "connections")],
    actions: [
      item("action:dup", "Duplicate row"),
      item("action:fk", "Open referenced row"),
      { ...item("action:inspector", "Toggle row detail panel"), keywords: ["ui toggle inspector"] },
      { ...item("cmd:theme.dark", "Theme: Dark"), keywords: ["dark mode"] },
      item("action:export", "Export…"),
    ],
  };
  const labels = (query: string, options = {}) =>
    flattenSections(buildOpenAnything(groups, query, options)).map((i) => i.label);

  it("lists only the schema's relations for an empty query, alphabetically, without headings", () => {
    const sections = buildOpenAnything(groups, "");
    expect(sections).toHaveLength(1);
    expect(sections[0]).toMatchObject({ id: "objects", showKind: true });
    expect(sections[0]!.heading).toBeUndefined();
    expect(labels("")).toEqual([
      "active_users",
      "daily_totals",
      "order_items",
      "orders",
      "post_counts",
      "post_tags",
      "posts",
      "repost_log",
      "users",
    ]);
  });

  it("lists recently opened relations first, and only relations", () => {
    const mru = ["action:dup", key("users"), "conn:p1", "tab:1", key("posts")];
    expect(labels("", { mru }).slice(0, 3)).toEqual(["users", "posts", "active_users"]);
    expect(labels("", { mru })).toHaveLength(relations.length);
    // At most RECENT_LIMIT of them; the rest stays alphabetical.
    const all = relations.map((r) => r.key).reverse();
    const listed = labels("", { mru: all });
    expect(listed.slice(0, RECENT_LIMIT)).toEqual(
      relations
        .slice(-RECENT_LIMIT)
        .map((r) => r.label)
        .reverse(),
    );
  });

  it("lists the browse groups it is told to (connections without a connection)", () => {
    expect(labels("", { browse: ["connections"] })).toEqual(["Local PG"]);
  });

  it("ranks objects of every kind in one flat list: prefix, word starts, then substrings", () => {
    expect(labels("po")).toEqual(["post_counts", "post_tags", "postgres", "posts", "repost_log"]);
    // Actions never show up without the > prefix, nor through loose subsequences.
    expect(labels("po")).not.toContain("Duplicate row");
    expect(labels("oi")).toEqual(["order_items"]);
    expect(labels("ord")).toEqual(["order_items", "orders", "orders"]);
    // Equal labels: the table before its open tab.
    expect(flattenSections(buildOpenAnything(groups, "orders")).map((i) => i.key)).toEqual([
      key("orders"),
      "tab:1",
    ]);
    expect(labels("users")).toEqual(["users", "active_users"]);
    expect(labels("local")).toEqual(["Local PG"]);
    expect(labels("qqq")).toEqual([]);
  });

  it("breaks ties by recency, then alphabetically", () => {
    expect(labels("post", { mru: [key("posts"), key("post_tags")] })).toEqual([
      "posts",
      "post_tags",
      "post_counts",
      "postgres",
      "repost_log",
    ]);
  });

  it("highlights the matched runs", () => {
    const [section] = buildOpenAnything(groups, "oi");
    expect(section!.items[0]!.ranges).toEqual([
      [0, 1],
      [6, 7],
    ]);
  });

  it("lists every relation of a kind when the whole query names it", () => {
    expect(labels("view")).toEqual(["active_users", "daily_totals"]);
    expect(labels("materialized view")).toEqual(["daily_totals"]);
    expect(labels("schema")).toEqual(["public"]);
    // A partial kind is not enough.
    expect(labels("vie")).toEqual([]);
  });

  it("lists commands only after >, with recently used ones first", () => {
    const sections = buildOpenAnything(groups, ">", { mru: ["cmd:theme.dark"] });
    expect(sections.map((s) => [s.id, s.heading, s.showKind])).toEqual([
      ["commands", undefined, undefined],
    ]);
    expect(labels(">", { mru: ["cmd:theme.dark"] })).toEqual([
      "Theme: Dark",
      "Duplicate row",
      "Export…",
      "Open referenced row",
      "Toggle row detail panel",
    ]);
    expect(labels("> dup")).toEqual(["Duplicate row"]);
    expect(labels(">ro")).toEqual([
      "Duplicate row",
      "Open referenced row",
      "Toggle row detail panel",
    ]);
    // Keywords that start words count, below label hits.
    expect(labels("> inspector")).toEqual(["Toggle row detail panel"]);
    expect(labels("> dark mode")).toEqual(["Theme: Dark"]);
    expect(labels(">users")).toEqual([]);
  });

  it("follows the list with a placeholder while tables load", () => {
    const loading = buildOpenAnything({ ...groups, tables: [] }, "", { loading: ["tables"] });
    expect(loading).toEqual([
      { id: "objects", items: [], more: 0, showKind: true, loading: "tables" },
    ]);
    // A query lists what already matched, the placeholder after it.
    const [section] = buildOpenAnything({ ...groups, tables: [] }, "lo", { loading: ["tables"] });
    expect(keys(section!.items)).toEqual(["conn:p1"]);
    expect(section!.loading).toBe("tables");
    // Not for commands.
    expect(buildOpenAnything(groups, "> dup", { loading: ["tables"] })[0]).not.toHaveProperty(
      "loading",
    );
  });

  it("caps the list and reports how many more matched", () => {
    const many = Array.from({ length: TABLE_LIMIT + 50 }, (_, i) =>
      item(`t:${i}`, `table_${i}`, "tables"),
    );
    const [empty] = buildOpenAnything({ tables: many }, "");
    expect(empty!.items).toHaveLength(TABLE_LIMIT);
    expect(empty!.more).toBe(50);
    // Natural sort: table_2 before table_10.
    expect(empty!.items.slice(0, 3).map((r) => r.item.label)).toEqual([
      "table_0",
      "table_1",
      "table_2",
    ]);
    const [searched] = buildOpenAnything({ tables: many }, "table_1");
    expect(searched!.items[0]!.item.label).toBe("table_1");
    expect(searched!.more).toBe(0);
  });

  it("tells command queries apart", () => {
    expect(isCommandQuery(">")).toBe(true);
    expect(isCommandQuery("  > theme")).toBe(true);
    expect(isCommandQuery("theme >")).toBe(false);
    expect(commandQuery(" > theme")).toBe(" theme");
  });
});

describe("rankStrict / browseOrder", () => {
  it("ranks exact above prefix above word starts above substrings", () => {
    const items = ["app_users", "users_archive", "rusers", "users"].map((l) => item(l, l));
    expect(keys(rankStrict(items, "users").items)).toEqual([
      "users",
      "users_archive",
      "app_users",
      "rusers",
    ]);
  });

  it("never lists noRecent items first", () => {
    const items = [{ ...item("a", "a"), noRecent: true }, item("b", "b"), item("c", "c")];
    expect(browseOrder(items, { mru: ["a", "c"] }).map((i) => i.key)).toEqual(["c", "b", "a"]);
  });
});

describe("completionOf", () => {
  it("completes a case-insensitive prefix with the rest of the label", () => {
    expect(completionOf("pos", "posts")).toBe("ts");
    expect(completionOf("POS", "posts")).toBe("ts");
    expect(completionOf("posts", "posts")).toBe("");
    expect(completionOf("", "posts")).toBe("");
    expect(completionOf("ost", "posts")).toBe("");
    expect(completionOf(">", ">x")).toBe("");
  });
});

describe("table kinds", () => {
  const open = vi.fn();

  it("shows the kind of every relation and of the other objects", () => {
    const kinds = tableItems(
      [
        table("users"),
        table("active_users", null, "view"),
        table("daily_totals", null, "materializedView"),
        table("pg_stats", null, "systemView"),
        table("events", null, "partitioned"),
      ],
      { profileId: "p1", open },
    ).map((i) => i.kind);
    expect(kinds).toEqual([
      "table",
      "view",
      "materialized view",
      "system view",
      "partitioned table",
    ]);
  });

  it("does not match schema-less table tabs on a hidden 'table' keyword", () => {
    const tabs: Tab[] = [
      {
        id: "1",
        kind: "table",
        connectionId: "c1",
        table: { schema: null, name: "x" },
        title: "x",
      },
    ];
    const items = tabItems(tabs, { activeId: null, activate: vi.fn() });
    expect(buildOpenAnything({ tabs: items }, "tab")).toEqual([]);
    expect(buildOpenAnything({ tabs: items }, "table")).toEqual([]);
  });
});

describe("item builders", () => {
  it("opens tables in the preview tab, and in a new tab on the alternate action", () => {
    const open = vi.fn();
    const [users] = tableItems([table("users", 12_345)], { profileId: "p1", open });
    expect(users).toMatchObject({
      key: tableItemKey("p1", { schema: "public", name: "users" }),
      label: "users",
      kind: "table",
      altLabel: "New tab",
    });
    expect(users!.meta).toMatch(/^~12/);
    users!.run();
    users!.runAlt!();
    expect(open.mock.calls.map((c) => c[1])).toEqual([{ preview: true }, { preview: false }]);
  });

  it("switches schemas and marks the current and the default one", () => {
    const select = vi.fn();
    const items = schemaItems(
      [
        { name: "public", isDefault: true },
        { name: "inventory", isDefault: false },
      ],
      { profileId: "p1", database: "app", current: "inventory", select },
    );
    expect(items.map((i) => [i.key, i.group, i.meta, i.kind, i.current, i.verb])).toEqual([
      ["schema:p1:app:public", "schemas", "default", "schema", false, "Switch"],
      ["schema:p1:app:inventory", "schemas", undefined, "schema", true, "Switch"],
    ]);
    items[0]!.run();
    expect(select).toHaveBeenCalledWith("public");
    // Typing the whole word lists them all.
    expect(flattenSections(buildSections({ schemas: items }, "schema"))).toHaveLength(2);
  });

  it("switches databases, never as a Recent default", () => {
    const switchTo = vi.fn();
    const db = (name: string, isCurrent = false) => ({
      name,
      owner: null,
      encoding: null,
      isCurrent,
    });
    const items = databaseItems([db("app", true), db("shop"), db("sys")], {
      profileId: "p1",
      // The connection's info wins over a stale `isCurrent`.
      current: "shop",
      switching: "sys",
      switchTo,
    });
    expect(items.map((i) => [i.key, i.current, i.meta, i.noRecent])).toEqual([
      ["db:p1:app", false, undefined, true],
      ["db:p1:shop", true, undefined, true],
      ["db:p1:sys", false, "Switching…", true],
    ]);
    items[0]!.run();
    expect(switchTo).toHaveBeenCalledWith("app");
    expect(items.every((i) => i.kind === "database")).toBe(true);
    // Never listed first as recently used.
    expect(browseOrder(items, { mru: ["db:p1:sys"] })[0]!.key).toBe("db:p1:app");
  });

  it("encodes keys so whitespace in names survives cmdk's trimming", () => {
    expect(tableItemKey("p1", { schema: null, name: " odd name " })).toBe(
      "table:p1::%20odd%20name%20",
    );
  });

  it("marks the active tab and dirty tabs", () => {
    const tabs: Tab[] = [
      {
        id: "1",
        kind: "table",
        connectionId: "c1",
        table: { schema: "public", name: "users" },
        title: "users",
      },
      { id: "2", kind: "query", connectionId: "c1", title: "Query 1" },
    ];
    const activate = vi.fn();
    const items = tabItems(tabs, { activeId: "2", isDirty: (t) => t.id === "1", activate });
    expect(items.map((i) => [i.label, i.kind, i.current, i.dirty])).toEqual([
      ["users", "tab", false, true],
      ["Query 1", "tab", true, false],
    ]);
    items[0]!.run();
    expect(activate).toHaveBeenCalledWith(tabs[0]);
  });

  it("connects saved profiles or switches to an already open connection", () => {
    const profile = (id: string, name: string) =>
      ({ id, name, color: null, driver: "postgres", group: null }) as ConnectionProfile;
    const connect = vi.fn();
    const switchTo = vi.fn();
    const items = connectionItems([profile("p1", "Local"), profile("p2", "Prod")], {
      openByProfile: new Map([["p2", "c2"]]),
      activeConnectionId: "c9",
      summary: () => "localhost",
      connect,
      switchTo,
    });
    expect(items.map((i) => [i.verb, i.meta, i.kind])).toEqual([
      ["Connect", undefined, "connection"],
      ["Switch", "Connected", "connection"],
    ]);
    items[0]!.run();
    items[1]!.run();
    expect(connect).toHaveBeenCalledWith(expect.objectContaining({ id: "p1" }));
    expect(switchTo).toHaveBeenCalledWith("c2");
  });

  it("shows a connect in flight and does not start another", () => {
    const profile = { id: "p1", name: "Local", color: null, driver: "postgres", group: null };
    const connect = vi.fn();
    const [p1] = connectionItems([profile as ConnectionProfile], {
      openByProfile: new Map(),
      connecting: new Set(["p1"]),
      activeConnectionId: null,
      summary: () => "",
      connect,
      switchTo: vi.fn(),
    });
    expect(p1).toMatchObject({ verb: "Connecting…", meta: "Connecting…" });
    p1!.run();
    expect(connect).not.toHaveBeenCalled();
  });

  it("offers connection commands only with an active connection", () => {
    const commands = {
      setTheme: vi.fn(),
      newConnection: vi.fn(),
      openShortcuts: vi.fn(),
      disconnect: vi.fn(),
      reloadSchema: vi.fn(),
    };
    const none = paletteCommandItems({ themePref: "dark", active: null, commands });
    expect(none.map((i) => i.key)).toEqual([
      "cmd:theme.light",
      "cmd:theme.dark",
      "cmd:theme.system",
      "cmd:connection.new",
      "cmd:settings.shortcuts",
    ]);
    expect(none.find((i) => i.current)?.key).toBe("cmd:theme.dark");
    none.find((i) => i.key === "cmd:connection.new")!.run();
    none.find((i) => i.key === "cmd:settings.shortcuts")!.run();
    expect(commands.newConnection).toHaveBeenCalledOnce();
    expect(commands.openShortcuts).toHaveBeenCalledOnce();

    const withConn = paletteCommandItems({
      themePref: "system",
      active: { connectionId: "c1", name: "Local", schema: "public" },
      commands,
    });
    const byKey = new Map(withConn.map((i) => [i.key, i]));
    byKey.get("cmd:schema.reload")!.run();
    byKey.get("cmd:connection.disconnect")!.run();
    byKey.get("cmd:theme.light")!.run();
    expect(commands.reloadSchema).toHaveBeenCalledWith("c1");
    expect(commands.disconnect).toHaveBeenCalledWith("c1");
    expect(commands.setTheme).toHaveBeenCalledWith("light");
    // Destructive: never becomes a "Recent" default.
    expect(byKey.get("cmd:connection.disconnect")!.noRecent).toBe(true);
  });
});
