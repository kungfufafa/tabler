import { getEditorState, subscribeEditor } from "@/features/editor/editorStates";
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
/** Profiles whose saved workspace is being restored: their saved entry stands in until done. */
const restoring = new Set<string>();
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

/** The remembered (not live) workspace of a profile. */
export function rememberedWorkspace(profileId: string): SessionWorkspace | null {
  return remembered.find((w) => w.profileId === profileId) ?? null;
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

/** Its saved workspace is being restored: keep writing that, not the half-built live one. */
export function beginRestoring(profileId: string): void {
  restoring.add(profileId);
}

export function isRestoring(profileId: string): boolean {
  return restoring.has(profileId);
}

/**
 * Restoring is over. `consumed`: the live workspace now stands for the saved one (it is forgotten);
 * otherwise (the connect failed) it stays remembered, closed.
 */
export function endRestoring(profileId: string, consumed: boolean): void {
  restoring.delete(profileId);
  if (consumed) remembered = remembered.filter((w) => w.profileId !== profileId);
  else remembered = remembered.map((w) => (w.profileId === profileId ? { ...w, open: false } : w));
  scheduleSave();
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
  for (const id of connections.order) {
    const conn = connections.byId[id];
    if (!conn) continue;
    const { profileId, currentDatabase } = conn.info;
    const saved = restoring.has(profileId) ? rememberedWorkspace(profileId) : null;
    if (saved) {
      live.push({ ...saved, open: true });
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
        },
        mode,
      ),
    );
  }
  const active = connections.activeId ? connections.byId[connections.activeId] : undefined;
  return buildSession(live, remembered, active?.info.profileId ?? null);
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
  restoring.clear();
  lastWritten = null;
}
