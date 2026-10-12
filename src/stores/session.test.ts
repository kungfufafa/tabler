import { describe, expect, it } from "vitest";
import {
  applyAutoSave,
  buildSession,
  clampCursor,
  keepsQueryText,
  parseSession,
  queryTabContent,
  serializeWorkspace,
  SESSION_VERSION,
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

  it("lists live workspaces first, then remembered ones (closed), each profile once", () => {
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
        { profileId: "p1", open: true, tabs: [] },
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
