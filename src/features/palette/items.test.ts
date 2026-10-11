import { describe, expect, it, vi } from "vitest";
import type { ConnectionProfile, TableSummary } from "@/lib/ipc";
import { KEYMAP } from "@/lib/keys/keymap";
import type { Tab } from "@/stores/tabs";
import type { ActionId } from "./actions";
import { paletteCommandItems } from "./commands";
import {
  actionApplies,
  buildSections,
  connectionItems,
  flattenSections,
  keymapActionItems,
  RECENT_LIMIT,
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
    expect(items[0]!.combo).toBe("Ctrl+Tab");
    expect(items[1]).toMatchObject({ label: "Toggle sidebar", combo: "Mod+B" });
    items[1]!.run();
    expect(run).toHaveBeenCalledWith("ui.toggle-sidebar");
  });

  it("hides everything when nothing is registered", () => {
    expect(keymapActionItems(KEYMAP, () => false, vi.fn())).toEqual([]);
  });

  it("hides tab actions that would do nothing", () => {
    const idle = { hasConnection: false, tabCount: 0, closedCount: 0 };
    expect(actionApplies("tab.new-query", idle)).toBe(false);
    expect(actionApplies("tab.close", idle)).toBe(false);
    expect(actionApplies("tab.reopen", idle)).toBe(false);
    expect(actionApplies("ui.toggle-sidebar", idle)).toBe(true);
    const busy = { hasConnection: true, tabCount: 1, closedCount: 1 };
    expect(actionApplies("tab.new-query", busy)).toBe(true);
    expect(actionApplies("tab.close", busy)).toBe(true);
    expect(actionApplies("tab.next", busy)).toBe(false);
    expect(actionApplies("tab.next", { ...busy, tabCount: 2 })).toBe(true);
  });
});

describe("buildSections", () => {
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

  it("floats recently used items into a leading Recent section when the query is empty", () => {
    const mru = ["a:close", "gone", "t:orders"];
    const sections = buildSections(groups, "", { mru });
    expect(sections[0]!.id).toBe("recent");
    expect(keys(sections[0]!.items)).toEqual(["a:close", "t:orders"]);
    // Recent items are not repeated in their own groups.
    expect(flattenSections(sections).map((i) => i.key)).toEqual([
      "a:close",
      "t:orders",
      "t:users",
      "tab:1",
      "a:sidebar",
    ]);
  });

  it("limits the Recent section", () => {
    const many = Array.from({ length: 10 }, (_, i) => item(`a:${i}`, `Action ${i}`));
    const mru = many.map((i) => i.key);
    const sections = buildSections({ actions: many }, "", { mru });
    expect(sections[0]!.items).toHaveLength(RECENT_LIMIT);
    expect(sections[1]!.items).toHaveLength(10 - RECENT_LIMIT);
  });

  it("filters with the fuzzy matcher, highlights labels and drops Recent while searching", () => {
    const sections = buildSections(groups, "ord", { mru: ["a:close"] });
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

  it("never floats noRecent items (Disconnect) into Recent", () => {
    const disconnect = { ...item("cmd:disconnect", "Disconnect"), noRecent: true };
    const sections = buildSections({ actions: [disconnect, ...groups.actions] }, "", {
      mru: ["cmd:disconnect", "a:close"],
    });
    expect(keys(sections[0]!.items)).toEqual(["a:close"]);
    expect(flattenSections(sections)[0]!.key).toBe("a:close");
    // Still listed in its own group.
    expect(sections.at(-1)!.items.some((r) => r.item.key === "cmd:disconnect")).toBe(true);
  });

  it("caps tables and reports how many more matched", () => {
    const many = Array.from({ length: TABLE_LIMIT + 50 }, (_, i) =>
      item(`t:${i}`, `table_${i}`, "tables"),
    );
    const [empty] = buildSections({ tables: many }, "");
    expect(empty!.items).toHaveLength(TABLE_LIMIT);
    expect(empty!.more).toBe(50);
    const [all] = buildSections({ tables: many }, "table");
    expect(all!.items).toHaveLength(TABLE_LIMIT);
    expect(all!.more).toBe(50);
    const [searched] = buildSections({ tables: many }, "table_1");
    const expected = many.filter((t) => /1/.test(t.label.slice("table_".length))).length;
    expect(searched!.items).toHaveLength(expected);
    expect(searched!.items[0]!.item.label).toBe("table_1");
    expect(searched!.more).toBe(0);
  });
});

describe("table kind tags", () => {
  const open = vi.fn();
  const many = tableItems(
    Array.from({ length: 2000 }, (_, i) => table(`customer_${i}`)),
    { profileId: "p1", open },
  );
  const tabActions = [item("action:tab.close", "Close tab"), item("action:tab.next", "Next tab")];

  it.each(["ta", "tab", "tabl", "table"])(
    "does not list every table for %j (the tab actions come first)",
    (query) => {
      const sections = buildSections({ tables: many, actions: tabActions }, query);
      expect(sections.find((s) => s.id === "tables")).toBeUndefined();
    },
  );

  it("lists views (and only views) when the whole query names the kind", () => {
    const mixed = tableItems(
      [
        table("users"),
        table("active_users", null, "view"),
        table("daily_totals", null, "materializedView"),
        table("pg_stats", null, "systemView"),
      ],
      { profileId: "p1", open },
    );
    const names = (q: string) =>
      flattenSections(buildSections({ tables: mixed }, q)).map((i) => i.label);
    expect(names("view")).toEqual(["active_users", "daily_totals", "pg_stats"]);
    expect(names("materialized view")).toEqual(["daily_totals"]);
    // A partial kind is not enough; label matches still work as usual.
    expect(names("vi")).toEqual([]);
    expect(names("users")).toEqual(["users", "active_users"]);
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
    expect(buildSections({ tabs: items }, "tab")).toEqual([]);
  });
});

describe("item builders", () => {
  it("opens data on run and structure on the alternate action", () => {
    const open = vi.fn();
    const [users] = tableItems([table("users", 12_345)], { profileId: "p1", open });
    expect(users).toMatchObject({
      key: tableItemKey("p1", { schema: "public", name: "users" }),
      label: "users",
      altLabel: "Structure",
    });
    expect(users!.meta).toMatch(/^~12/);
    users!.run();
    users!.runAlt!();
    expect(open.mock.calls.map((c) => c[1])).toEqual(["table", "structure"]);
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
    expect(items.map((i) => [i.label, i.detail, i.current, i.dirty])).toEqual([
      ["users", "Data · public", false, true],
      ["Query 1", undefined, true, false],
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
    expect(items.map((i) => [i.verb, i.meta])).toEqual([
      ["Connect", undefined],
      ["Switch", "Connected"],
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
