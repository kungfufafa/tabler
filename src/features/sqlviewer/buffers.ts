import { EditorSelection, EditorState, Text } from "@codemirror/state";
import type { EditorView, ViewUpdate } from "@codemirror/view";
import {
  actionKeymap,
  actionKeymapConf,
  editorExtensions,
  editorPrefsOf,
  languageConf,
  languageExtension,
  languagePrefsOf,
} from "@/features/editor/cmExtensions";
import type { SqlDialect } from "@/features/editor/scanner";
import { subscribeKeymap } from "@/lib/keys/keymap";
import { getPrefs } from "@/stores/prefs";
import { patchDoc } from "./store";

/**
 * The SQL Viewers' CodeMirror documents, by viewer key. Like the query tabs' (editorStates.ts), a
 * viewer keeps one `EditorState` (text, undo history, selection) while its view comes and goes
 * with the tab; `savedTexts` is what the file holds, to tell whether the buffer is dirty.
 */
const states = new Map<string, EditorState>();
const views = new Map<string, EditorView>();
const savedTexts = new Map<string, Text>();

const toText = (s: string): Text => Text.of(s.split(/\r\n?|\n/));

/** The SQL language without a server: keywords and functions of the dialect complete. */
export function viewerLanguage(dialect: SqlDialect) {
  return languageExtension({ dialect, ...languagePrefsOf(getPrefs()) });
}

function syncDirty(key: string, doc: Text): void {
  patchDoc(key, { dirty: !doc.eq(savedTexts.get(key) ?? Text.empty) });
}

export function createViewerState(key: string, text: string, dialect: SqlDialect): EditorState {
  const p = getPrefs();
  return EditorState.create({
    doc: text,
    selection: EditorSelection.cursor(0),
    extensions: editorExtensions({
      language: { dialect, ...languagePrefsOf(p) },
      prefs: editorPrefsOf(p),
      onUpdate: (u: ViewUpdate) => {
        if (u.docChanged) syncDirty(key, u.state.doc);
      },
    }),
  });
}

/** The viewer's state (an empty one on first use). */
export function takeViewerState(key: string, dialect: SqlDialect): EditorState {
  let state = states.get(key);
  if (!state) {
    state = createViewerState(key, "", dialect);
    states.set(key, state);
  }
  return state;
}

export function stashViewerState(key: string, state: EditorState): void {
  states.set(key, state);
}

export function registerViewerView(key: string, view: EditorView): () => void {
  views.set(key, view);
  return () => {
    if (views.get(key) === view) views.delete(key);
  };
}

export function getViewerView(key: string): EditorView | null {
  return views.get(key) ?? null;
}

/** The buffer's text (live view first). */
export function viewerText(key: string): string {
  return (views.get(key)?.state ?? states.get(key))?.doc.toString() ?? "";
}

/**
 * Replaces the buffer with a file's `text` (a fresh undo history) and records it as saved. A
 * mounted view takes the new state at once.
 */
export function loadViewerText(key: string, text: string, dialect: SqlDialect): void {
  const state = createViewerState(key, text, dialect);
  states.set(key, state);
  savedTexts.set(key, state.doc);
  views.get(key)?.setState(state);
  syncDirty(key, state.doc);
}

/** Records `text` as what the file now holds (after a save). */
export function markViewerSaved(key: string, text: string): void {
  savedTexts.set(key, toText(text));
  const state = views.get(key)?.state ?? states.get(key);
  if (state) syncDirty(key, state.doc);
}

/** Switches the highlighting / comment syntax of the viewer. */
export function setViewerDialect(key: string, dialect: SqlDialect): void {
  const effects = languageConf.reconfigure(viewerLanguage(dialect));
  const view = views.get(key);
  if (view) view.dispatch({ effects });
  else {
    const state = states.get(key);
    if (state) states.set(key, state.update({ effects }).state);
  }
}

export function focusViewer(key: string): void {
  views.get(key)?.focus();
}

/** Forgets a viewer (its tab closed for good). */
export function dropViewerBuffer(key: string): void {
  states.delete(key);
  savedTexts.delete(key);
  views.delete(key);
}

/** Viewer keys with a buffer (for cleanup). */
export function viewerBufferKeys(): string[] {
  return [...new Set([...states.keys(), ...views.keys(), ...savedTexts.keys()])];
}

// A remapped shortcut (Settings ▸ Shortcuts: Toggle Line Comment…) reaches open viewers too.
subscribeKeymap(() => {
  const effects = actionKeymapConf.reconfigure(actionKeymap());
  for (const view of views.values()) view.dispatch({ effects });
  for (const [key, state] of states)
    if (!views.has(key)) states.set(key, state.update({ effects }).state);
});
