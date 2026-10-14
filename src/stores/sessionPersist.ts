import { getEditorState, subscribeEditor } from "@/features/editor/editorStates";
import { persistsToolTab } from "@/features/tabs/toolTabs";
import { railOrder } from "@/features/tabs/workspaces";
import { closeWindowNow } from "@/hooks/useUnsavedGuard";
import { ipc, isMock } from "@/lib/ipc";
import { lastShownTab } from "./activeSync";
import { useConnectionsStore } from "./connections";
import { getPrefs, usePrefsStore } from "./prefs";
import { getQueryTab, useQueryTabsStore } from "./queryTabs";
import {
  buildSession,
  keepsQueryText,
  parseSession,
  serializeWorkspace,
  SESSION_KEY,
  workspaceKey,
  type LiveQueryTab,
  type Session,
  type SessionWorkspace,
} from "./session";
import { setWindowCloseHooks, useTabsStore, workspaceTabs, type Tab } from "./tabs";

/**
 * Keeps the saved session (see `session.ts`) in step with the app: every change to the open
 * workspaces, their tabs, a query buffer or its caret schedules a debounced write to the app DB
 * settings (`session`); leaving the page or closing the window writes right away.
 */

export const SESSION_SAVE_DEBOUNCE_MS = 500;
/** How long closing the window waits for the session write before closing anyway. */
export const SESSION_CLOSE_TIMEOUT_MS = 2_000;
/** The browser mock's settings live in memory; this mirror lets `pnpm dev:mock` restore too. */
export const MOCK_SESSION_LOCAL_KEY = "tabler.mock.session";

/** Workspaces from the stored session that are not live (not reconnected yet, or failed). */
let remembered: SessionWorkspace[] = [];

/**
 * A saved workspace being restored: until it is done, its saved entry stands in for the live
 * workspace being rebuilt (`connectionId` once known, else a connection of the profile that was
 * not open when the restore began).
 */
export interface RestoreToken {
  readonly ws: SessionWorkspace;
  readonly known: ReadonlySet<string>;
  connectionId: string | null;
}
const restoring: RestoreToken[] = [];
let lastWritten: string | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;
let teardown: (() => void) | null = null;

function readMockLocal(): unknown {
  try {
    const raw = globalThis.localStorage?.getItem(MOCK_SESSION_LOCAL_KEY);
    return raw ? (JSON.parse(raw) as unknown) : null;
  } catch {
    return null;
  }
}

function writeMockLocal(json: string): void {
  try {
    globalThis.localStorage?.setItem(MOCK_SESSION_LOCAL_KEY, json);
  } catch {
    // Storage unavailable or full: the mock only loses its restore.
  }
}

/** Reads the stored session (an empty one when there is none or it cannot be read). */
export async function loadSession(): Promise<Session> {
  let raw: unknown = null;
  try {
    raw = (await ipc.settings.get())[SESSION_KEY] ?? null;
  } catch (err) {
    console.warn("[session] load failed", err);
  }
  if (raw === null && isMock) raw = readMockLocal();
  return parseSession(raw);
}

/**
 * The remembered (not live) workspace of a profile: the one on `database` when there is one (a
 * profile can have a workspace per database), else its first. With `exact` only the one on
 * `database` counts, never another database's: a profile's first connection may land on another
 * database than the saved one and take its tabs, but a further workspace of an already open
 * profile must not consume (and so lose) a workspace of a database it is not on.
 */
export function rememberedWorkspace(
  profileId: string,
  database?: string | null,
  opts: { exact?: boolean } = {},
): SessionWorkspace | null {
  const own = remembered.filter((w) => w.profileId === profileId);
  if (database !== undefined) {
    const same = own.find((w) => workspaceKey(w) === workspaceKey({ profileId, database }));
    if (same) return same;
  }
  return opts.exact ? null : (own[0] ?? null);
}

/** Replaces the remembered workspaces (startup). */
export function setRemembered(workspaces: readonly SessionWorkspace[]): void {
  remembered = workspaces.map((w) => ({ ...w }));
  scheduleSave();
}

/** Marks every remembered workspace closed: it is reopened on connect, not at startup. */
export function closeRemembered(): void {
  remembered = remembered.map((w) => (w.open ? { ...w, open: false } : w));
  scheduleSave();
}

/** A saved workspace is being restored: keep writing that, not the half-built live one. */
export function beginRestoring(ws: SessionWorkspace): RestoreToken {
  const connections = useConnectionsStore.getState();
  const known = new Set(
    connections.order.filter((id) => connections.byId[id]?.info.profileId === ws.profileId),
  );
  const token: RestoreToken = { ws, known, connectionId: null };
  restoring.push(token);
  return token;
}

/** The connection the restored workspace lives in, once it is open. */
export function bindRestoring(token: RestoreToken, connectionId: string): void {
  token.connectionId = connectionId;
}

export function isRestoring(profileId: string): boolean {
  return restoring.some((t) => t.ws.profileId === profileId);
}

/**
 * Restoring is over. `consumed`: the live workspace now stands for the saved one (it is forgotten);
 * otherwise (the connect failed) it stays remembered, closed.
 */
export function endRestoring(token: RestoreToken, consumed: boolean): void {
  const i = restoring.indexOf(token);
  if (i >= 0) restoring.splice(i, 1);
  const key = workspaceKey(token.ws);
  if (consumed) remembered = remembered.filter((w) => workspaceKey(w) !== key);
  else remembered = remembered.map((w) => (workspaceKey(w) === key ? { ...w, open: false } : w));
  scheduleSave();
}

/** The restore a live connection is being rebuilt by, if any. */
function restoreOf(connectionId: string, profileId: string): RestoreToken | null {
  return (
    restoring.find(
      (t) =>
        t.ws.profileId === profileId &&
        (t.connectionId === connectionId ||
          (t.connectionId === null && !t.known.has(connectionId))),
    ) ?? null
  );
}

function liveQuery(tabId: string): LiveQueryTab {
  const state = getEditorState(tabId);
  return {
    text: state?.doc.toString() ?? "",
    cursor: state?.selection.main.head ?? 0,
    filePath: getQueryTab(tabId).filePath,
  };
}

/** The session as it would be written now. */
export function currentSession(): Session {
  const connections = useConnectionsStore.getState();
  const { tabs } = useTabsStore.getState();
  const mode = getPrefs().autoSaveQueryTabs;
  const live: SessionWorkspace[] = [];
  const standIns = new Set<RestoreToken>();
  // Rail order (the user may have dragged it).
  for (const id of railOrder()) {
    const conn = connections.byId[id];
    if (!conn) continue;
    const { profileId, currentDatabase } = conn.info;
    const token = restoreOf(id, profileId);
    if (token) {
      if (!standIns.has(token)) live.push({ ...token.ws, open: true });
      standIns.add(token);
      continue;
    }
    live.push(
      serializeWorkspace(
        {
          profileId,
          database: currentDatabase,
          schema: conn.currentSchema,
          tabs: workspaceTabs(tabs, id),
          activeTabId: lastShownTab(id),
          query: liveQuery,
          keepsTool: persistsToolTab,
        },
        mode,
      ),
    );
  }
  const active = connections.activeId ? connections.byId[connections.activeId] : undefined;
  return buildSession(
    live,
    remembered,
    active?.info.profileId ?? null,
    active?.info.currentDatabase ?? null,
  );
}

async function writeNow(): Promise<void> {
  if (!teardown) return;
  const session = currentSession();
  const json = JSON.stringify(session);
  if (json === lastWritten) return;
  lastWritten = json;
  if (isMock) writeMockLocal(json);
  try {
    await ipc.settings.set(SESSION_KEY, session);
  } catch (err) {
    // Try again with the next change.
    if (lastWritten === json) lastWritten = null;
    console.warn("[session] save failed", err);
  }
}

/** Schedules a debounced write (no-op until persistence is installed). */
export function scheduleSave(): void {
  if (!teardown) return;
  if (timer !== null) clearTimeout(timer);
  timer = setTimeout(() => {
    timer = null;
    void writeNow();
  }, SESSION_SAVE_DEBOUNCE_MS);
}

/** Writes the session right away (pending changes included). */
export function flushSession(): Promise<void> {
  if (timer !== null) clearTimeout(timer);
  timer = null;
  return writeNow();
}

/**
 * Whether closing the window may drop this tab's unsaved work: an untitled SQL Query tab whose text
 * the auto-save setting keeps (it comes back on the next start). File tabs keep their unsaved-file
 * prompt.
 */
export function survivesClose(tab: Tab, tabs: readonly Tab[]): boolean {
  if (tab.kind !== "query" || getQueryTab(tab.id).filePath !== null) return false;
  const index = workspaceTabs(tabs, tab.connectionId)
    .filter((t) => t.kind === "query")
    .findIndex((t) => t.id === tab.id);
  return keepsQueryText(getPrefs().autoSaveQueryTabs, index);
}

function withTimeout(p: Promise<void>, ms: number): Promise<void> {
  return new Promise((resolve) => {
    const t = setTimeout(resolve, ms);
    void p.finally(() => {
      clearTimeout(t);
      resolve();
    });
  });
}

/** Re-subscribes to the editors of the open query tabs (buffer and caret changes). */
function syncEditorSubscriptions(subs: Map<string, () => void>): void {
  const open = new Set(
    useTabsStore
      .getState()
      .tabs.filter((t) => t.kind === "query")
      .map((t) => t.id),
  );
  for (const [id, unsubscribe] of subs)
    if (!open.has(id)) {
      unsubscribe();
      subs.delete(id);
    }
  for (const id of open) if (!subs.has(id)) subs.set(id, subscribeEditor(id, scheduleSave));
}

/**
 * Starts writing the session (call once the stored one was read, so startup never overwrites it
 * with an empty app). Returns the uninstall function.
 */
export function installSessionPersistence(): () => void {
  if (teardown) return teardown;
  const editorSubs = new Map<string, () => void>();
  const unsubscribers = [
    useTabsStore.subscribe((s, prev) => {
      if (s.tabs !== prev.tabs) syncEditorSubscriptions(editorSubs);
      if (s.tabs !== prev.tabs || s.activeId !== prev.activeId) scheduleSave();
    }),
    useConnectionsStore.subscribe(scheduleSave),
    useQueryTabsStore.subscribe((s, prev) => {
      const moved = Object.entries(s.slices).some(
        ([id, slice]) => slice.filePath !== (prev.slices[id]?.filePath ?? null),
      );
      if (moved) scheduleSave();
    }),
    usePrefsStore.subscribe((s, prev) => {
      if (s.prefs.autoSaveQueryTabs !== prev.prefs.autoSaveQueryTabs) scheduleSave();
    }),
    setWindowCloseHooks({
      survivesClose,
      save: () => void flushSession(),
      closeAfterSave: () => {
        void withTimeout(flushSession(), SESSION_CLOSE_TIMEOUT_MS).then(() => closeWindowNow());
      },
    }),
  ];
  const flush = () => void flushSession();
  const onHidden = () => {
    if (document.visibilityState === "hidden") flush();
  };
  window.addEventListener("pagehide", flush);
  window.addEventListener("beforeunload", flush);
  window.addEventListener("blur", flush);
  document.addEventListener("visibilitychange", onHidden);
  syncEditorSubscriptions(editorSubs);

  const uninstall = () => {
    unsubscribers.forEach((u) => u());
    for (const u of editorSubs.values()) u();
    editorSubs.clear();
    window.removeEventListener("pagehide", flush);
    window.removeEventListener("beforeunload", flush);
    window.removeEventListener("blur", flush);
    document.removeEventListener("visibilitychange", onHidden);
    if (timer !== null) clearTimeout(timer);
    timer = null;
    if (teardown === uninstall) teardown = null;
  };
  teardown = uninstall;
  scheduleSave();
  return uninstall;
}

/** Test-only: forget everything (uninstalls persistence). */
export function resetSessionForTests(): void {
  teardown?.();
  teardown = null;
  remembered = [];
  restoring.splice(0);
  lastWritten = null;
}
