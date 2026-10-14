import type { EditorState } from "@codemirror/state";
import { useCallback, useSyncExternalStore } from "react";
import { formatCount } from "@/lib/format/time";
import { getEditorState, subscribeEditor } from "./editorStates";

/**
 * TablePlus' bottom bar: "Line 4, Column 13, Location 40", plus " (12 characters selected)" while
 * text is selected. Line, column and location are all 1-based, so the document's first character
 * reads "Line 1, Column 1, Location 1"; Location is the absolute offset of the caret (the head of
 * the main selection range) in the document.
 */
export function cursorLabel(state: EditorState | null): string {
  if (!state) return "Line 1, Column 1, Location 1";
  const head = state.selection.main.head;
  const line = state.doc.lineAt(head);
  const where = `Line ${line.number}, Column ${head - line.from + 1}, Location ${head + 1}`;
  let selected = 0;
  for (const r of state.selection.ranges) selected += r.to - r.from;
  if (selected === 0) return where;
  return `${where} (${formatCount(selected)} ${selected === 1 ? "character" : "characters"} selected)`;
}

/** The tab's cursor position / selection size, live while its editor is mounted. */
export function useCursorLabel(tabId: string): string {
  const subscribe = useCallback((fn: () => void) => subscribeEditor(tabId, fn), [tabId]);
  return useSyncExternalStore(subscribe, () => cursorLabel(getEditorState(tabId)));
}
