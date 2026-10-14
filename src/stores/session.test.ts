import { describe, expect, it } from "vitest";
import {
  applyAutoSave,
  buildSession,
  clampCursor,
  keepsQueryText,
  parseSession,
  parseTab,
  queryTabContent,
  serializeWorkspace,
  SESSION_VERSION,
  workspaceKey,
  type LiveQueryTab,
  type SessionQueryTab,
  type SessionTab,
  type SessionWorkspace,
} from "./session";
import type { Tab } from "./tabs";

const users = { schema: "public", name: "users" };

const tabs: Tab[] = [
  { id: "t1", kind: "table", connectionId: "c1", table: users, title: "users", preview: true },
  { id: "q1", kind: "query", connectionId: "c1", title: "Query 1", dirty: true },
  { id: "q2", kind: "query", connectionId: "c1", title: "report.sql" },
  { id: "q3", kind: "query", connectionId: "c1", title: "Scratch", dirty: true },
  { id: "s1", kind: "structure", connectionId: "c1", table: users, title: "Users!" },
];

const buffers: Record<string, LiveQueryTab> = {
  q1: { text: "select 1;", cursor: 7, filePath: null },
  q2: { text: "select * from users;", cursor: 99, filePath: "/q/report.sql" },
  q3: { text: "select 3;", cursor: 0, filePath: null },
};

const live = (activeTabId: string | null = "q2") => ({
  profileId: "p1",
  database: "shop",
  schema: "public",
  tabs,
  activeTabId,
  query: (id: string) => buffers[id]!,
});

const query = (over: Partial<SessionQueryTab> = {}): SessionQueryTab => ({
  kind: "query",
  title: "Query 1",
  sql: "select 1;",
  filePath: null,
  cursor: 0,
  ...over,
});

describe("serializeWorkspace", () => {
  it("saves every tab in order with its view, title, text and caret (auto save: all)", () => {
    const ws = serializeWorkspace(live(), "all");
    expect(ws).toEqual({
      profileId: "p1",
      open: true,
      database: "shop",
      schema: "public",
      activeTab: 2,
      tabs: [
        { kind: "table", table: users, view: "table", title: "users" },
        { kind: "query", title: "Query 1", sql: "select 1;", filePath: null, cursor: 7 },
        // A clean file tab: its file has the text. The caret is clamped to the document.
        { kind: "query", title: "report.sql", sql: null, filePath: "/q/report.sql", cursor: 20 },
        { kind: "query", title: "Scratch", sql: "select 3;", filePath: null, cursor: 0 },
        // A renamed tab keeps its name; structure view is remembered.
        { kind: "table", table: users, view: "structure", title: "Users!" },
      ],
    });
  });

  it("keeps a dirty file tab's unsaved text", () => {
    const dirty = tabs.map((t) => (t.id === "q2" ? { ...t, dirty: true } : t));
    const ws = serializeWorkspace({ ...live(), tabs: dirty }, "all");
    expect(ws.tabs[2]).toMatchObject({ sql: "select * from users;", filePath: "/q/report.sql" });
  });

  it("only saves the first query tab's text with 'first'", () => {
    const ws = serializeWorkspace(live("q3"), "first");
    expect(ws.tabs.map((t) => t.title)).toEqual(["users", "Query 1", "report.sql", "Users!"]);
    expect(ws.tabs[1]).toMatchObject({ sql: "select 1;" });
    // The active tab (Scratch) was left out.
    expect(ws.activeTab).toBeNull();
  });

  it("saves no query text with 'none' (file tabs still reopen from disk)", () => {
    const ws = serializeWorkspace(live("s1"), "none");
    expect(ws.tabs).toEqual([
      { kind: "table", table: users, view: "table", title: "users" },
      { kind: "query", title: "report.sql", sql: null, filePath: "/q/report.sql", cursor: 20 },
      { kind: "table", table: users, view: "structure", title: "Users!" },
    ]);
    expect(ws.activeTab).toBe(2);
  });
});

describe("applyAutoSave", () => {
  const saved: SessionTab[] = [
    query({ title: "A" }),
    query({ title: "B", sql: "edited", filePath: "/b.sql" }),
    query({ title: "C" }),
  ];

  it("counts query tabs only, and maps the active index", () => {
    expect(keepsQueryText("first", 0)).toBe(true);
    expect(keepsQueryText("first", 1)).toBe(false);
    expect(keepsQueryText("all", 7)).toBe(true);
    expect(keepsQueryText("none", 0)).toBe(false);
    expect(applyAutoSave(saved, "all", 2)).toEqual({ tabs: saved, activeTab: 2 });
    const first = applyAutoSave(saved, "first", 1);
    expect(first.tabs.map((t) => t.title)).toEqual(["A", "B"]);
    expect(first.tabs[1]).toMatchObject({ sql: null, filePath: "/b.sql" });
    expect(first.activeTab).toBe(1);
    expect(applyAutoSave(saved, "none", 2)).toEqual({
      tabs: [{ ...saved[1]!, sql: null }],
      activeTab: null,
    });
  });
});

describe("buildSession", () => {
  const ws = (profileId: string, open = true): SessionWorkspace => ({
    profileId,
    open,
    database: null,
    schema: null,
    tabs: [],
    activeTab: null,
  });

  it("lists live workspaces first, then remembered ones (closed), each workspace once", () => {
    const s = buildSession([ws("p1"), ws("p2")], [ws("p2"), ws("p3")], "p2");
    expect(s.version).toBe(SESSION_VERSION);
    expect(s.workspaces.map((w) => [w.profileId, w.open])).toEqual([
      ["p1", true],
      ["p2", true],
      ["p3", false],
    ]);
    expect(s.activeProfileId).toBe("p2");
    expect(buildSession([ws("p1")], [], "gone").activeProfileId).toBeNull();
  });

  it("keeps a workspace per database of a profile, and which one was on screen", () => {
    const on = (database: string, open = true) => ({ ...ws("p1", open), database });
    const s = buildSession([on("app"), on("shop")], [on("shop"), on("logs")], "p1", "shop");
    expect(s.workspaces.map((w) => [w.database, w.open])).toEqual([
      ["app", true],
      ["shop", true],
      ["logs", false],
    ]);
    expect(s.activeDatabase).toBe("shop");
    expect(parseSession(JSON.parse(JSON.stringify(s)))).toEqual(s);
    expect(workspaceKey(on("app"))).not.toBe(workspaceKey(on("shop")));
  });
});

describe("tool tabs", () => {
  const overview: Tab = {
    id: "o",
    kind: "tool",
    connectionId: "c1",
    tool: "overview",
    title: "Overview: public",
    params: { schema: "public" },
  };

  it("are saved only when their tool keeps them, and read back", () => {
    const base = { ...live(), tabs: [overview] };
    expect(serializeWorkspace(base, "all").tabs).toEqual([]);
    const kept = serializeWorkspace({ ...base, keepsTool: (t) => t === "overview" }, "all");
    expect(kept.tabs).toEqual([
      { kind: "tool", tool: "overview", title: "Overview: public", params: { schema: "public" } },
    ]);
    expect(parseTab(kept.tabs[0])).toEqual(kept.tabs[0]);
    expect(parseTab({ kind: "tool", tool: "" })).toBeNull();
    expect(parseTab({ kind: "tool", tool: "metrics" })).toEqual({
      kind: "tool",
      tool: "metrics",
      title: "metrics",
      params: null,
    });
  });
});

describe("tool tabs that decide per tab", () => {
  const viewer = (id: string, path: string | null): Tab => ({
    id,
    kind: "tool",
    connectionId: "c1",
    tool: "sqlviewer",
    title: path ?? "SQL Viewer",
    params: { path },
  });
  // What the SQL Viewer registers: only a viewer of a file can be reopened.
  const keepsViewer = (tool: string, params: unknown) =>
    tool === "sqlviewer" && typeof (params as { path?: unknown })?.path === "string";

  it("asks the tool with the tab's params: an untitled viewer is not serialised, one with a path is", () => {
    const ws = serializeWorkspace(
      {
        ...live(),
        tabs: [viewer("v1", null), viewer("v2", "/q/a.sql"), viewer("v3", null)],
        activeTabId: "v2",
        keepsTool: keepsViewer,
      },
      "all",
    );
    expect(ws.tabs).toEqual([
      { kind: "tool", tool: "sqlviewer", title: "/q/a.sql", params: { path: "/q/a.sql" } },
    ]);
    // The active index follows the tabs that were left out.
    expect(ws.activeTab).toBe(0);
  });

  it("gives a tab without params to the tool as null", () => {
    const seen: unknown[] = [];
    serializeWorkspace(
      {
        ...live(),
        tabs: [{ id: "x", kind: "tool", connectionId: "c1", tool: "t", title: "T" }],
        keepsTool: (_tool, params) => {
          seen.push(params);
          return false;
        },
      },
      "all",
    );
    expect(seen).toEqual([null]);
  });
});

describe("parseSession", () => {
  it("round-trips a session", () => {
    const s = buildSession([serializeWorkspace(live(), "all")], [], "p1");
    expect(parseSession(JSON.parse(JSON.stringify(s)))).toEqual(s);
  });

  it("drops what it cannot read instead of failing", () => {
    expect(parseSession(null).workspaces).toEqual([]);
    expect(parseSession({ version: 99, workspaces: [] }).workspaces).toEqual([]);
    const s = parseSession({
      version: SESSION_VERSION,
      activeProfileId: "nope",
      workspaces: [
        "junk",
        { profileId: "" },
        {
          profileId: "p1",
          open: true,
          database: "shop",
          schema: 4,
          activeTab: 3,
          tabs: [
            { kind: "table", table: { name: "users", schema: "public" }, view: "weird" },
            { kind: "table", table: { schema: "public" } },
            { kind: "query", title: "", sql: null, filePath: null },
            { kind: "query", sql: "select 1", cursor: -4 },
          ],
        },
        // The same workspace twice (profile and database): the first one counts.
        { profileId: "p1", open: true, database: "shop", tabs: [] },
        { kind: "tool" },
      ],
    });
    expect(s.activeProfileId).toBeNull();
    expect(s.workspaces).toEqual([
      {
        profileId: "p1",
        open: true,
        database: "shop",
        schema: null,
        activeTab: 1,
        tabs: [
          { kind: "table", table: users, view: "table", title: "users" },
          { kind: "query", title: "Query", sql: "select 1", filePath: null, cursor: 0 },
        ],
      },
    ]);
  });
});

describe("a table tab's filter bar (a window handoff)", () => {
  const row = (over: Record<string, unknown> = {}) => ({
    id: "r1",
    column: "id",
    op: "eq",
    value: "5",
    value2: "",
    enabled: true,
    ...over,
  });
  const table = (over: Record<string, unknown> = {}) => ({
    kind: "table",
    table: users,
    view: "table",
    title: "users",
    ...over,
  });

  it("is kept with its visibility and conjunction", () => {
    expect(parseTab(table({ filters: [row()], filtersVisible: true, conjunction: "or" }))).toEqual({
      kind: "table",
      table: users,
      view: "table",
      title: "users",
      filters: [row()],
      filtersVisible: true,
      conjunction: "or",
    });
    // The bar is shown unless the handoff says it is hidden; "and" is the default conjunction.
    expect(parseTab(table({ filters: [row()] }))).toMatchObject({ filtersVisible: true });
    expect(parseTab(table({ filters: [row()], filtersVisible: false }))).toMatchObject({
      filtersVisible: false,
    });
    expect(parseTab(table({ filters: [row()], conjunction: "xor" }))).not.toHaveProperty(
      "conjunction",
    );
  });

  it("is validated: rows with missing or mistyped fields and unknown operators are dropped", () => {
    const parsed = parseTab(
      table({
        filters: [
          row({ id: "ok1" }),
          row({ id: 7 }),
          row({ id: "bad-col", column: null }),
          row({ id: "bad-value", value: 5 }),
          row({ id: "bad-value2", value2: undefined }),
          row({ id: "bad-enabled", enabled: "yes" }),
          row({ id: "bad-op", op: "regexp-ish" }),
          row({ id: "raw", op: "like", column: "__raw_sql__", enabled: false }),
          "junk",
          null,
        ],
      }),
    );
    expect(parsed?.kind === "table" && parsed.filters?.map((f) => f.id)).toEqual(["ok1", "raw"]);
  });

  it("gives a repeated or empty row id a new one", () => {
    const parsed = parseTab(
      table({ filters: [row({ id: "a" }), row({ id: "a" }), row({ id: "" })] }),
    );
    const ids = parsed?.kind === "table" ? (parsed.filters ?? []).map((f) => f.id) : [];
    expect(ids).toHaveLength(3);
    expect(new Set(ids).size).toBe(3);
    expect(ids[0]).toBe("a");
  });

  it("is left out when no row survives, or when it is not a list", () => {
    for (const filters of [[], [row({ op: "nope" })], "x", 4, null, {}]) {
      const parsed = parseTab(table({ filters, filtersVisible: true, conjunction: "or" }));
      expect(parsed).toEqual({ kind: "table", table: users, view: "table", title: "users" });
    }
  });

  it("is capped", () => {
    const many = Array.from({ length: 80 }, (_, i) => row({ id: `r${i}` }));
    const parsed = parseTab(table({ filters: many }));
    expect(parsed?.kind === "table" && parsed.filters).toHaveLength(50);
  });

  it("is never part of the saved session itself", () => {
    const saved = serializeWorkspace(live(), "all");
    expect(saved.tabs.filter((t) => t.kind === "table")).toEqual([
      { kind: "table", table: users, view: "table", title: "users" },
      { kind: "table", table: users, view: "structure", title: "Users!" },
    ]);
  });

  it("keeps two tabs of one table as two tabs", () => {
    const twice: Tab[] = [
      { id: "a", kind: "table", connectionId: "c1", table: users, title: "users" },
      { id: "b", kind: "table", connectionId: "c1", table: users, title: "users" },
    ];
    const saved = serializeWorkspace({ ...live(null), tabs: twice }, "all");
    expect(saved.tabs).toHaveLength(2);
    const back = parseSession(JSON.parse(JSON.stringify(buildSession([saved], [], "p1"))));
    expect(back.workspaces[0]!.tabs).toEqual(saved.tabs);
  });
});

describe("restoring a query tab", () => {
  it("starts untitled tabs from their text, dirty while not empty", () => {
    expect(queryTabContent(query(), undefined)).toEqual({ doc: "select 1;", saved: "" });
    expect(queryTabContent(query({ sql: null }), undefined)).toBeNull();
  });

  it("reloads clean file tabs from disk and keeps a dirty one's edits against it", () => {
    const file = query({ sql: null, filePath: "/r.sql" });
    expect(queryTabContent(file, "on disk")).toEqual({ doc: "on disk", saved: "on disk" });
    expect(queryTabContent(file, null)).toBeNull();
    const edited = query({ sql: "edited", filePath: "/r.sql" });
    expect(queryTabContent(edited, "on disk")).toEqual({ doc: "edited", saved: "on disk" });
    // The file is gone: the edits stay, unsaved.
    expect(queryTabContent(edited, null)).toEqual({ doc: "edited", saved: "" });
  });

  it("clamps the caret", () => {
    expect(clampCursor(50, 10)).toBe(10);
    expect(clampCursor(-1, 10)).toBe(0);
    expect(clampCursor(4, 10)).toBe(4);
  });
});
