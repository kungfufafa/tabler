import type { TableRef } from "@/lib/ipc";
import type { AutoSaveQueryTabs } from "./prefs";
import type { Tab, TableView } from "./tabs";

/**
 * The saved session: the workspaces (connections) that were open, each with its database, schema
 * and tabs, so the next start can reopen them (General ▸ "Reopen closed workspaces at startup") and
 * SQL Query tabs keep their text (Editor ▸ "Auto Save SQL Query Tab"). Stored as JSON under the
 * `session` key of the app DB settings. Staged edits are never part of it.
 *
 * This module is pure (serialise, parse, filter); `sessionPersist.ts` / `sessionRestore.ts` wire it
 * to the stores.
 */

export const SESSION_KEY = "session";
export const SESSION_VERSION = 1;

export interface SessionTableTab {
  kind: "table";
  table: TableRef;
  view: TableView;
  title: string;
}

export interface SessionQueryTab {
  kind: "query";
  title: string;
  /**
   * The buffer. Null when it is not kept: a clean file tab (its file has the text) or one the
   * auto-save setting leaves out. A file tab with text had unsaved edits.
   */
  sql: string | null;
  filePath: string | null;
  /** Caret offset (UTF-16). */
  cursor: number;
}

export type SessionTab = SessionTableTab | SessionQueryTab;

export interface SessionWorkspace {
  profileId: string;
  /**
   * Open when the session was written (reopened at startup). A closed one is a workspace that was
   * open at the last quit but has not been reconnected since: its tabs come back on connect.
   */
  open: boolean;
  database: string | null;
  schema: string | null;
  tabs: SessionTab[];
  /** Index in `tabs` of the tab the workspace showed last. */
  activeTab: number | null;
}

export interface Session {
  version: typeof SESSION_VERSION;
  /** Rail order: the open workspaces first. */
  workspaces: SessionWorkspace[];
  /** Profile of the workspace that was on screen. */
  activeProfileId: string | null;
}

export const EMPTY_SESSION: Readonly<Session> = Object.freeze({
  version: SESSION_VERSION,
  workspaces: [],
  activeProfileId: null,
});

// --- serialise -------------------------------------------------------------------------------

/** A query tab's live buffer (from its editor state) and file. */
export interface LiveQueryTab {
  text: string;
  cursor: number;
  filePath: string | null;
}

export interface LiveWorkspace {
  profileId: string;
  database: string | null;
  schema: string | null;
  /** The workspace's tabs, in strip order. */
  tabs: readonly Tab[];
  /** The tab it shows (or showed last). */
  activeTabId: string | null;
  query: (tabId: string) => LiveQueryTab;
}

/** Whether the auto-save setting keeps the text of the `index`-th query tab of a workspace. */
export function keepsQueryText(mode: AutoSaveQueryTabs, index: number): boolean {
  return mode === "all" || (mode === "first" && index === 0);
}

/**
 * Applies the auto-save setting to a workspace's tabs (when saving and again when restoring, so a
 * changed setting counts): a query tab whose text is not kept loses it; one without a file then
 * has nothing to restore and is left out. Returns the kept tabs and where `active` went (null when
 * that tab was left out).
 */
export function applyAutoSave(
  tabs: readonly SessionTab[],
  mode: AutoSaveQueryTabs,
  active: number | null = null,
): { tabs: SessionTab[]; activeTab: number | null } {
  const out: SessionTab[] = [];
  let activeTab: number | null = null;
  let queryIndex = 0;
  tabs.forEach((tab, i) => {
    let kept: SessionTab | null = tab;
    if (tab.kind === "query") {
      if (!keepsQueryText(mode, queryIndex++) && tab.sql !== null)
        kept = tab.filePath ? { ...tab, sql: null } : null;
    }
    if (!kept) return;
    if (i === active) activeTab = out.length;
    out.push(kept);
  });
  return { tabs: out, activeTab };
}

/** One live workspace as saved. Preview tabs are saved as regular ones. */
export function serializeWorkspace(ws: LiveWorkspace, mode: AutoSaveQueryTabs): SessionWorkspace {
  let active: number | null = null;
  const tabs = ws.tabs.map((tab, i): SessionTab => {
    if (tab.id === ws.activeTabId) active = i;
    if (tab.kind !== "query")
      return { kind: "table", table: { ...tab.table }, view: tab.kind, title: tab.title };
    const q = ws.query(tab.id);
    const keep = !q.filePath || tab.dirty === true;
    return {
      kind: "query",
      title: tab.title,
      sql: keep ? q.text : null,
      filePath: q.filePath,
      cursor: Math.max(0, Math.min(q.cursor, q.text.length)),
    };
  });
  const kept = applyAutoSave(tabs, mode, active);
  return {
    profileId: ws.profileId,
    open: true,
    database: ws.database,
    schema: ws.schema,
    tabs: kept.tabs,
    activeTab: kept.activeTab,
  };
}

/**
 * The session to write: the live workspaces (rail order), then the remembered closed ones whose
 * profile has no live workspace (each profile once).
 */
export function buildSession(
  live: readonly SessionWorkspace[],
  remembered: readonly SessionWorkspace[],
  activeProfileId: string | null,
): Session {
  const seen = new Set<string>();
  const workspaces: SessionWorkspace[] = [];
  for (const ws of live) {
    if (seen.has(ws.profileId)) continue;
    seen.add(ws.profileId);
    workspaces.push(ws);
  }
  for (const ws of remembered) {
    if (seen.has(ws.profileId)) continue;
    seen.add(ws.profileId);
    workspaces.push({ ...ws, open: false });
  }
  return {
    version: SESSION_VERSION,
    workspaces,
    activeProfileId: activeProfileId !== null && seen.has(activeProfileId) ? activeProfileId : null,
  };
}

// --- parse -----------------------------------------------------------------------------------

type Json = Record<string, unknown>;

const isObject = (v: unknown): v is Json =>
  v !== null && typeof v === "object" && !Array.isArray(v);
const str = (v: unknown): string | null => (typeof v === "string" ? v : null);
const nonEmpty = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);

function parseTab(raw: unknown): SessionTab | null {
  if (!isObject(raw)) return null;
  if (raw.kind === "table") {
    const table = raw.table;
    if (!isObject(table)) return null;
    const name = nonEmpty(table.name);
    if (!name) return null;
    const view: TableView = raw.view === "structure" ? "structure" : "table";
    return {
      kind: "table",
      table: { schema: str(table.schema), name },
      view,
      title: nonEmpty(raw.title) ?? name,
    };
  }
  if (raw.kind === "query") {
    const sql = str(raw.sql);
    const filePath = nonEmpty(raw.filePath);
    if (sql === null && filePath === null) return null;
    const cursor =
      typeof raw.cursor === "number" && Number.isInteger(raw.cursor) && raw.cursor > 0
        ? raw.cursor
        : 0;
    return { kind: "query", title: nonEmpty(raw.title) ?? "Query", sql, filePath, cursor };
  }
  return null;
}

function parseWorkspace(raw: unknown): SessionWorkspace | null {
  if (!isObject(raw)) return null;
  const profileId = nonEmpty(raw.profileId);
  if (!profileId) return null;
  const rawTabs = Array.isArray(raw.tabs) ? raw.tabs : [];
  const tabs: SessionTab[] = [];
  let activeTab: number | null = null;
  rawTabs.forEach((t, i) => {
    const tab = parseTab(t);
    if (!tab) return;
    if (i === raw.activeTab) activeTab = tabs.length;
    tabs.push(tab);
  });
  return {
    profileId,
    open: raw.open === true,
    database: str(raw.database),
    schema: str(raw.schema),
    tabs,
    activeTab,
  };
}

/**
 * Reads a stored session leniently: unknown versions and malformed parts are dropped (an invalid
 * tab is skipped, not the whole workspace), so stale or hand-edited data never breaks startup.
 */
export function parseSession(raw: unknown): Session {
  if (!isObject(raw) || raw.version !== SESSION_VERSION || !Array.isArray(raw.workspaces))
    return { ...EMPTY_SESSION, workspaces: [] };
  const workspaces: SessionWorkspace[] = [];
  const seen = new Set<string>();
  for (const w of raw.workspaces) {
    const ws = parseWorkspace(w);
    if (!ws || seen.has(ws.profileId)) continue;
    seen.add(ws.profileId);
    workspaces.push(ws);
  }
  const active = nonEmpty(raw.activeProfileId);
  return {
    version: SESSION_VERSION,
    workspaces,
    activeProfileId: active && seen.has(active) ? active : null,
  };
}

// --- restore ---------------------------------------------------------------------------------

/** Clamps a saved caret into a document of `length` characters. */
export function clampCursor(cursor: number, length: number): number {
  return Math.max(0, Math.min(cursor, length));
}

/**
 * What a restored query tab starts with. `fileText`: the file's current content (undefined when
 * the tab has no file, null when it could not be read). `doc` is the buffer; `saved` the text it
 * counts as saved against (the tab is dirty while they differ: the editor's own rule). Null when
 * there is nothing to show (a clean file tab whose file is gone).
 */
export function queryTabContent(
  tab: SessionQueryTab,
  fileText: string | null | undefined,
): { doc: string; saved: string } | null {
  if (tab.filePath === null) return tab.sql === null ? null : { doc: tab.sql, saved: "" };
  if (tab.sql !== null) return { doc: tab.sql, saved: fileText ?? "" };
  return fileText == null ? null : { doc: fileText, saved: fileText };
}
