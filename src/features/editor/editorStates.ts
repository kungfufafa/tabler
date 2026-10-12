import { diagnosticCount, setDiagnostics } from "@codemirror/lint";
import {
  EditorSelection,
  EditorState,
  Text,
  type StateEffect,
  type TransactionSpec,
} from "@codemirror/state";
import type { EditorView, ViewUpdate } from "@codemirror/view";
import { subscribeKeymap } from "@/lib/keys/keymap";
import { getPrefs } from "@/stores/prefs";
import { useTabsStore } from "@/stores/tabs";
import {
  actionKeymap,
  actionKeymapConf,
  editorExtensions,
  editorPrefsOf,
  type LanguageConfig,
} from "./cmExtensions";
import { dropAnchors, recordDocChanges } from "./docAnchors";
import { isSplitKey } from "./split";

/**
 * Each query tab keeps one CodeMirror `EditorState` for its whole life, so the document, the undo
 * history, the selection and folds survive tab switches (the view is destroyed when the tab is not
 * shown). While a tab is visible its live `EditorView` is the source of truth. States are keyed by
 * editor key: the tab id, or `splitKey(tabId)` for the second pane of a split editor.
 */

interface Stored {
  state: EditorState;
  /** `EditorView.scrollSnapshot()` taken when the view unmounted. */
  scroll: StateEffect<unknown> | null;
}

const stored = new Map<string, Stored>();
const views = new Map<string, EditorView>();
/** Text of the last save / open, to decide whether the tab is dirty. */
const savedDocs = new Map<string, Text>();
/** Who wants to hear about a tab's selection / document changes (the editor bar's position). */
const listeners = new Map<string, Set<() => void>>();

/** Calls `fn` whenever the tab's selection or document changes (or its view mounts). */
export function subscribeEditor(tabId: string, fn: () => void): () => void {
  let set = listeners.get(tabId);
  if (!set) listeners.set(tabId, (set = new Set()));
  set.add(fn);
  return () => {
    set.delete(fn);
    if (set.size === 0 && listeners.get(tabId) === set) listeners.delete(tabId);
  };
}

function notify(tabId: string): void {
  listeners.get(tabId)?.forEach((fn) => fn());
}

/**
 * A remapped or removed shortcut (Settings ▸ Shortcuts) applies to the editors already open too:
 * every live view and every stored state gets the query keymap rebuilt from KEYMAP.
 */
function refreshActionKeymaps(): void {
  const effects = actionKeymapConf.reconfigure(actionKeymap());
  for (const view of views.values()) view.dispatch({ effects });
  for (const [key, entry] of stored)
    if (!views.has(key)) entry.state = entry.state.update({ effects }).state;
}
subscribeKeymap(refreshActionKeymaps);

/**
 * Recomputes the tab's dirty flag from its document. A split pane (`splitKey`) is a scratch
 * editor: it never makes the tab dirty.
 */
export function syncDirty(tabId: string, doc: Text): void {
  if (isSplitKey(tabId)) return;
  const saved = savedDocs.get(tabId) ?? Text.empty;
  useTabsStore.getState().setDirty(tabId, !doc.eq(saved));
}

function onUpdate(tabId: string) {
  return (u: ViewUpdate) => {
    if (u.docChanged || u.selectionSet) notify(tabId);
    if (!u.docChanged) return;
    recordDocChanges(tabId, u.changes.desc);
    syncDirty(tabId, u.state.doc);
    // Error squiggles describe the text that ran; editing makes them stale.
    if (diagnosticCount(u.state) > 0) {
      const view = u.view;
      queueMicrotask(() => {
        if (diagnosticCount(view.state) > 0) view.dispatch(setDiagnostics(view.state, []));
      });
    }
  };
}

export function createEditorState(
  tabId: string,
  doc = "",
  language: LanguageConfig = { dialect: "postgres" },
): EditorState {
  const p = getPrefs();
  return EditorState.create({
    doc,
    selection: EditorSelection.cursor(0),
    extensions: editorExtensions({
      language: { upperCaseKeywords: p.editorUppercaseKeywords, ...language },
      prefs: editorPrefsOf(p),
      onUpdate: onUpdate(tabId),
    }),
  });
}

/** The stored state of a tab (created empty on first use) plus its last scroll position. */
export function takeEditorState(tabId: string, language?: LanguageConfig): Stored {
  let entry = stored.get(tabId);
  if (!entry) {
    entry = { state: createEditorState(tabId, "", language), scroll: null };
    stored.set(tabId, entry);
  }
  return entry;
}

/** Keeps a view's state (and scroll position) when it unmounts. */
export function stashEditorState(
  tabId: string,
  state: EditorState,
  scroll: StateEffect<unknown> | null,
): void {
  stored.set(tabId, { state, scroll });
}

export function registerView(tabId: string, view: EditorView): () => void {
  views.set(tabId, view);
  notify(tabId);
  return () => {
    if (views.get(tabId) === view) views.delete(tabId);
  };
}

export function getEditorView(tabId: string): EditorView | null {
  return views.get(tabId) ?? null;
}

/** Live state (mounted view) or the stored one. */
export function getEditorState(tabId: string): EditorState | null {
  return views.get(tabId)?.state ?? stored.get(tabId)?.state ?? null;
}

export function hasEditorState(tabId: string): boolean {
  return views.has(tabId) || stored.has(tabId);
}

/**
 * Applies a transaction to the tab's editor whether or not it is mounted (e.g. diagnostics that
 * arrive after the user switched to another tab).
 */
export function applyToEditor(
  tabId: string,
  make: (state: EditorState) => TransactionSpec | null,
): boolean {
  const view = views.get(tabId);
  if (view) {
    const spec = make(view.state);
    if (spec) view.dispatch(spec);
    return spec !== null;
  }
  const entry = stored.get(tabId);
  if (!entry) return false;
  const spec = make(entry.state);
  if (!spec) return false;
  const tr = entry.state.update(spec);
  entry.state = tr.state;
  if (tr.docChanged || tr.selection) notify(tabId);
  if (tr.docChanged) {
    recordDocChanges(tabId, tr.changes.desc);
    syncDirty(tabId, tr.state.doc);
  }
  return true;
}

/** Records `text` as the saved / opened content (the tab is clean while the doc equals it). */
export function markSaved(tabId: string, text: string | Text): void {
  savedDocs.set(tabId, typeof text === "string" ? Text.of(text.split(/\r\n?|\n/)) : text);
  const state = getEditorState(tabId);
  if (state) syncDirty(tabId, state.doc);
}

/** Prepares a tab's document before it mounts (opening a file into a new tab). */
export function seedEditor(tabId: string, doc: string, language?: LanguageConfig): void {
  stored.set(tabId, { state: createEditorState(tabId, doc, language), scroll: null });
  markSaved(tabId, doc);
  notify(tabId);
}

/** Forgets a tab's editor (it was closed for good). */
export function dropEditor(tabId: string): void {
  stored.delete(tabId);
  savedDocs.delete(tabId);
  views.delete(tabId);
  dropAnchors(tabId);
}

/** Ids of every tab with editor state (for cleanup). */
export function editorTabIds(): string[] {
  return [...new Set([...stored.keys(), ...views.keys(), ...savedDocs.keys()])];
}

/** Focuses the tab's editor, optionally selecting a range and scrolling it into view. */
export function focusEditor(tabId: string, range?: { from: number; to: number }): void {
  const view = views.get(tabId);
  if (!view) return;
  if (range) {
    const len = view.state.doc.length;
    const from = Math.max(0, Math.min(range.from, len));
    const to = Math.max(from, Math.min(range.to, len));
    view.dispatch({ selection: EditorSelection.range(from, to), scrollIntoView: true });
  }
  view.focus();
}
