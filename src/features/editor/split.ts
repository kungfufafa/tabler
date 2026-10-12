import { create } from "zustand";

/**
 * Split pane (⇧⌘D, TablePlus's "Split Pane Horizontally"): a second, independent SQL editor under
 * the first one in the same query tab. It has its own document, undo history and selection (its
 * editor state is keyed `splitKey(tabId)`) and shares the tab's editor bar and results. Runs,
 * Beautify, Explain… act on the pane that had the focus last.
 */

const SUFFIX = "::split";

/** Editor-state key of a tab's second pane. */
export function splitKey(tabId: string): string {
  return `${tabId}${SUFFIX}`;
}

export function isSplitKey(key: string): boolean {
  return key.endsWith(SUFFIX);
}

/** The query tab an editor key belongs to. */
export function baseTabId(key: string): string {
  return isSplitKey(key) ? key.slice(0, -SUFFIX.length) : key;
}

export type EditorPane = "main" | "split";

interface SplitState {
  /** Tabs whose editor is split. */
  open: Readonly<Record<string, true>>;
  /** Tabs whose second pane had the focus last. */
  focused: Readonly<Record<string, true>>;
}

export const useSplitStore = create<SplitState>()(() => ({ open: {}, focused: {} }));

function without(map: Readonly<Record<string, true>>, key: string): Record<string, true> {
  const next = { ...map };
  delete next[key];
  return next;
}

export function isSplitOpen(tabId: string): boolean {
  return useSplitStore.getState().open[tabId] === true;
}

export function useSplitOpen(tabId: string): boolean {
  return useSplitStore((s) => s.open[tabId] === true);
}

/** The editor the tab's commands act on: the pane focused last. */
export function activeEditorKey(tabId: string): string {
  const s = useSplitStore.getState();
  return s.open[tabId] && s.focused[tabId] ? splitKey(tabId) : tabId;
}

export function useActiveEditorKey(tabId: string): string {
  return useSplitStore((s) => (s.open[tabId] && s.focused[tabId] ? splitKey(tabId) : tabId));
}

export function setFocusedPane(tabId: string, pane: EditorPane): void {
  const s = useSplitStore.getState();
  if ((s.focused[tabId] === true) === (pane === "split")) return;
  useSplitStore.setState({
    focused: pane === "split" ? { ...s.focused, [tabId]: true } : without(s.focused, tabId),
  });
}

export function openSplit(tabId: string): void {
  const s = useSplitStore.getState();
  if (s.open[tabId]) return;
  useSplitStore.setState({
    open: { ...s.open, [tabId]: true },
    focused: { ...s.focused, [tabId]: true },
  });
}

/** Closes the second pane; `onClosed` frees its editor state. */
export function closeSplit(tabId: string, onClosed?: (key: string) => void): void {
  const s = useSplitStore.getState();
  if (!s.open[tabId]) return;
  useSplitStore.setState({ open: without(s.open, tabId), focused: without(s.focused, tabId) });
  onClosed?.(splitKey(tabId));
}

/** Forgets a tab's split state (it was closed for good). */
export function dropSplit(tabId: string): void {
  const s = useSplitStore.getState();
  if (s.open[tabId] || s.focused[tabId])
    useSplitStore.setState({ open: without(s.open, tabId), focused: without(s.focused, tabId) });
}
