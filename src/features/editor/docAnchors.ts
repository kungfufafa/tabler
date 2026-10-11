import type { ChangeDesc } from "@codemirror/state";
import { useCallback, useSyncExternalStore } from "react";
import { clampRange, type DocRange } from "./errorPos";

/**
 * Offsets that come back from a run (error positions, statement spans) describe the document as it
 * was when the SQL was submitted. The editor stays editable while a query runs (and afterwards), so
 * each submission records the document changes made since, and ranges are mapped through them
 * before they become squiggles, "Line X, column Y" labels or a selection.
 */

export type SubmissionKind = "run" | "plan";

interface Anchor {
  /** The queryId of the run / explain. */
  id: string;
  /** Changes since the submission (null: none yet). */
  changes: ChangeDesc | null;
}

/** Per tab, the latest run and the latest explain (the only submissions the UI shows). */
const anchors = new Map<string, Partial<Record<SubmissionKind, Anchor>>>();
const versions = new Map<string, number>();
const listeners = new Map<string, Set<() => void>>();

/** Starts tracking edits for a submission made against the tab's current document. */
export function anchorSubmission(tabId: string, kind: SubmissionKind, id: string): void {
  anchors.set(tabId, { ...anchors.get(tabId), [kind]: { id, changes: null } });
}

/** Called for every document change of the tab (view updates and off-screen transactions). */
export function recordDocChanges(tabId: string, changes: ChangeDesc): void {
  if (changes.empty) return;
  const tracked = anchors.get(tabId);
  if (tracked) {
    for (const a of Object.values(tracked)) {
      if (a) a.changes = a.changes ? a.changes.composeDesc(changes) : changes;
    }
  }
  versions.set(tabId, (versions.get(tabId) ?? 0) + 1);
  listeners.get(tabId)?.forEach((l) => l());
}

export function dropAnchors(tabId: string): void {
  anchors.delete(tabId);
  versions.delete(tabId);
}

function findAnchor(tabId: string, id: string): Anchor | null {
  const tracked = anchors.get(tabId);
  if (tracked?.run?.id === id) return tracked.run;
  if (tracked?.plan?.id === id) return tracked.plan;
  return null;
}

/** True when a change overlaps the inside of [from, to] (insertions at its edges do not count). */
function editsInside(changes: ChangeDesc, from: number, to: number): boolean {
  let hit = false;
  changes.iterChangedRanges((fromA, toA) => {
    if (fromA < to && toA > from) hit = true;
  });
  return hit;
}

/**
 * Where a range of the submitted document is now. Null when the submission is no longer tracked,
 * when its text was deleted, or (with `unlessEdited`) when the text inside it was edited.
 */
export function toCurrentDoc(
  tabId: string,
  id: string,
  range: DocRange,
  opts: { unlessEdited?: boolean } = {},
): DocRange | null {
  const anchor = findAnchor(tabId, id);
  if (!anchor) return null;
  const changes = anchor.changes;
  if (!changes) return range;
  const r = clampRange(range, changes.length);
  if (opts.unlessEdited && editsInside(changes, r.from, r.to)) return null;
  const from = changes.mapPos(r.from, 1);
  const to = Math.max(from, changes.mapPos(r.to, -1));
  if (r.to > r.from && to === from) return null;
  return { from, to };
}

function subscribe(tabId: string, cb: () => void): () => void {
  let set = listeners.get(tabId);
  if (!set) {
    set = new Set();
    listeners.set(tabId, set);
  }
  set.add(cb);
  return () => {
    set.delete(cb);
    if (set.size === 0 && listeners.get(tabId) === set) listeners.delete(tabId);
  };
}

/** Re-renders the caller whenever the tab's document changes. */
export function useDocVersion(tabId: string): number {
  const sub = useCallback((cb: () => void) => subscribe(tabId, cb), [tabId]);
  return useSyncExternalStore(sub, () => versions.get(tabId) ?? 0);
}
