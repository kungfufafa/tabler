import { EditorSelection } from "@codemirror/state";
import { applyToEditor, focusEditor, getEditorState } from "@/features/editor/editorStates";
import { languageOf, splitWith } from "@/features/editor/language";
import { statementAt } from "@/features/editor/scanner";
import { activeEditorKey } from "@/features/editor/split";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import type { PluginSql } from "./types";

/** The active tab when it is a SQL query tab. */
export function activeQueryTabId(): string | null {
  const { tabs, activeId } = useTabsStore.getState();
  const tab = tabs.find((t) => t.id === activeId);
  return tab?.kind === "query" ? tab.id : null;
}

/** The query tab's editor text, selection and statement at the cursor (its focused pane). */
export function editorSql(tabId: string): PluginSql | null {
  const state = getEditorState(activeEditorKey(tabId));
  if (!state) return null;
  const tab = useTabsStore.getState().tabs.find((t) => t.id === tabId);
  const info = tab ? useConnectionsStore.getState().byId[tab.connectionId]?.info : undefined;
  // Split as the editor splits it (`languageOf`): a core dialect's scanner, or the lane's
  // splitter for an extension dialect or a console. None without an editor, or while it loads.
  const language = languageOf(info);
  if (!language) return null;
  const text = state.doc.toString();
  const sel = state.selection.main;
  const st = statementAt(splitWith(language, text), text, sel.head, true);
  return {
    text,
    selected: state.sliceDoc(sel.from, sel.to),
    statement: st ? text.slice(st.start, st.end) : "",
  };
}

/**
 * `context.insertToEditor(text)` (at the cursor, the selection stays) and
 * `context.replaceSelection(text)` in the query tab `tabId` (else the active query tab).
 */
export function writeToEditor(tabId: string | null, text: string, mode: "insert" | "replace") {
  const id = tabId ?? activeQueryTabId();
  if (!id) throw new Error("Open a SQL query tab first");
  const key = activeEditorKey(id);
  const applied = applyToEditor(key, (s) => {
    const sel = s.selection.main;
    const from = mode === "replace" ? sel.from : sel.head;
    const to = mode === "replace" ? sel.to : sel.head;
    return {
      changes: { from, to, insert: text },
      selection: EditorSelection.cursor(from + text.length),
      scrollIntoView: true,
      userEvent: "input.paste",
    };
  });
  if (!applied) throw new Error("The SQL editor is not open");
  focusEditor(key);
}
