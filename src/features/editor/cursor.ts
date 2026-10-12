import type { EditorState } from "@codemirror/state";
import { useCallback, useSyncExternalStore } from "react";
import { formatCount } from "@/lib/format/time";
import { getEditorState, subscribeEditor } from "./editorStates";

/** "Line 4, Column 13", or "12 characters selected" while text is selected. */
export function cursorLabel(state: EditorState | null): string {
  if (!state) return "Line 1, Column 1";
  let selected = 0;
  for (const r of state.selection.ranges) selected += r.to - r.from;
  if (selected > 0) {
    return `${formatCount(selected)} ${selected === 1 ? "character" : "characters"} selected`;
  }
  const head = state.selection.main.head;
  const line = state.doc.lineAt(head);
  return `Line ${line.number}, Column ${head - line.from + 1}`;
}

/** The tab's cursor position / selection size, live while its editor is mounted. */
export function useCursorLabel(tabId: string): string {
  const subscribe = useCallback((fn: () => void) => subscribeEditor(tabId, fn), [tabId]);
  return useSyncExternalStore(subscribe, () => cursorLabel(getEditorState(tabId)));
}
