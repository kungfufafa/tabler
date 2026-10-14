import { toast } from "sonner";
import { ipc, toIpcError } from "@/lib/ipc";
import { getPrefs, usePrefsStore } from "@/stores/prefs";
import { getQueryTab, useQueryTabsStore } from "@/stores/queryTabs";
import { useTabsStore } from "@/stores/tabs";
import { editorTabIds, getEditorState, markSaved, onDocChanged } from "./editorStates";
import { baseName } from "./files";
import { isSplitKey } from "./split";

/**
 * "Automatically save queries while editing" (TablePlus, on by default): a query tab backed by a
 * `.sql` file (opened from it, or saved to it once) writes the file shortly after each edit, so
 * the tab never stays dirty. Off: ⌘S saves, as before.
 */

/** Quiet time after the last keystroke before the file is written. */
export const FILE_AUTOSAVE_DELAY_MS = 800;

const timers = new Map<string, ReturnType<typeof setTimeout>>();
/** Paths whose last auto-save failed (warned once until a save works again). */
const failed = new Set<string>();
/** Per tab: the auto-save running now (with the passes that follow it) until it is done. */
const writing = new Map<string, Promise<boolean>>();
/** Tabs asked to save while one of their writes was running: they write once more after it. */
const rerun = new Set<string>();

function isDirty(tabId: string): boolean {
  return useTabsStore.getState().tabs.some((t) => t.id === tabId && t.kind === "query" && t.dirty);
}

/**
 * Writes the tab's document to its file now, when it has one and has unsaved edits. A tab writes
 * one at a time: asked again while its write runs (a second timer, a flush), it writes once more
 * after that one ended, with the text the document has by then, so two writes of one file never
 * overlap and the last one holds the latest text. Resolves when the tab has nothing left to
 * write; true when anything was written.
 */
export function autoSaveFile(tabId: string): Promise<boolean> {
  const pending = timers.get(tabId);
  if (pending) clearTimeout(pending);
  timers.delete(tabId);
  const running = writing.get(tabId);
  if (running) {
    rerun.add(tabId);
    return running;
  }
  const done = (async () => {
    let wrote = await writeOnce(tabId);
    while (rerun.delete(tabId)) wrote = (await writeOnce(tabId)) || wrote;
    return wrote;
  })().finally(() => {
    writing.delete(tabId);
  });
  writing.set(tabId, done);
  return done;
}

/** One write of the tab's current document (nothing when it has no file or no unsaved edits). */
async function writeOnce(tabId: string): Promise<boolean> {
  const path = getQueryTab(tabId).filePath;
  const state = getEditorState(tabId);
  if (!path || !state || !isDirty(tabId)) return false;
  const doc = state.doc;
  try {
    await ipc.sqlFiles.write(path, doc.toString());
  } catch (e) {
    if (!failed.has(path)) {
      failed.add(path);
      toast.error(`Could not auto-save ${baseName(path)}`, {
        description: `${toIpcError(e).message} — ⌘S saves (or Save As… elsewhere).`,
      });
    }
    return false;
  }
  failed.delete(path);
  // The text that was written; edits made meanwhile keep the tab dirty (and schedule again).
  if (getQueryTab(tabId).filePath === path) markSaved(tabId, doc);
  return true;
}

export function scheduleFileAutoSave(tabId: string, delay = FILE_AUTOSAVE_DELAY_MS): void {
  if (isSplitKey(tabId) || !getPrefs().editorAutoSaveFiles) return;
  if (!getQueryTab(tabId).filePath) return;
  const pending = timers.get(tabId);
  if (pending) clearTimeout(pending);
  timers.set(
    tabId,
    setTimeout(() => {
      timers.delete(tabId);
      void autoSaveFile(tabId);
    }, delay),
  );
}

/** Writes every pending auto-save at once (the window is going away). */
export function flushFileAutoSaves(): void {
  for (const tabId of [...timers.keys()]) void autoSaveFile(tabId);
}

let installed = false;

/** Hooks the auto-save to every query tab's edits (once). */
export function installFileAutoSave(): void {
  if (installed) return;
  installed = true;
  onDocChanged((tabId) => scheduleFileAutoSave(tabId));
  // Turned on: the file-backed tabs with unsaved edits save now.
  usePrefsStore.subscribe((s, prev) => {
    if (!s.prefs.editorAutoSaveFiles || prev.prefs.editorAutoSaveFiles) return;
    for (const tabId of editorTabIds()) if (isDirty(tabId)) scheduleFileAutoSave(tabId, 0);
  });
  // A tab that just got its file (Save As…, opened into it) has nothing pending.
  useQueryTabsStore.subscribe((s, prev) => {
    if (s.slices === prev.slices) return;
    for (const [tabId, slice] of Object.entries(s.slices))
      if (slice.filePath !== prev.slices[tabId]?.filePath && isDirty(tabId))
        scheduleFileAutoSave(tabId);
  });
  if (typeof window !== "undefined") window.addEventListener("pagehide", flushFileAutoSaves);
}

/** Test helper: forgets pending saves and warnings. */
export function resetFileAutoSaveForTests(): void {
  for (const t of timers.values()) clearTimeout(t);
  timers.clear();
  failed.clear();
  writing.clear();
  rerun.clear();
}
