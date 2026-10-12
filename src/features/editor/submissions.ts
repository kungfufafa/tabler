import { useCallback, useSyncExternalStore } from "react";
import { toCurrentDoc } from "./docAnchors";
import type { DocRange } from "./errorPos";
import { originalOffset, type Replacement } from "./params";

/**
 * Where each run / explain of a tab came from: the editor pane (`key`, see `split.ts`) and the
 * query parameters substituted into its text. Offsets that come back from the backend refer to
 * the substituted text; `locateSubmitted` maps them to that pane's current document.
 */
interface SubmissionInfo {
  key: string;
  /** Offset of the submitted text in the document. */
  baseOffset: number;
  replacements: readonly Replacement[];
}

const infos = new Map<string, SubmissionInfo>();
/** `${tabId}:${kind}` → the latest submission id (older ones are forgotten). */
const latest = new Map<string, string>();
const listeners = new Set<() => void>();
let version = 0;

export function recordSubmission(
  tabId: string,
  kind: "run" | "plan",
  id: string,
  info: SubmissionInfo,
): void {
  const slot = `${tabId}:${kind}`;
  const prev = latest.get(slot);
  if (prev !== undefined) infos.delete(prev);
  latest.set(slot, id);
  infos.set(id, info);
  version++;
  listeners.forEach((l) => l());
}

/** The editor key submission `id` came from (the tab's main editor when unknown). */
export function submissionEditor(tabId: string, id: string | null | undefined): string {
  return (id && infos.get(id)?.key) || tabId;
}

/** Maps a range of the substituted text (in document offsets) back to the variables. */
export function unsubstituteRange(
  range: DocRange,
  baseOffset: number,
  replacements: readonly Replacement[],
): DocRange {
  if (replacements.length === 0) return range;
  return {
    from: baseOffset + originalOffset(replacements, range.from - baseOffset, -1),
    to: baseOffset + originalOffset(replacements, range.to - baseOffset, 1),
  };
}

/**
 * Where a range of submission `id` (document offsets of the text as submitted) is now, and in
 * which editor; null when it can no longer be located (see `toCurrentDoc`).
 */
export function locateSubmitted(
  tabId: string,
  id: string,
  range: DocRange,
  opts: { unlessEdited?: boolean } = {},
): { key: string; range: DocRange } | null {
  const info = infos.get(id);
  const key = info?.key ?? tabId;
  const original = info ? unsubstituteRange(range, info.baseOffset, info.replacements) : range;
  const at = toCurrentDoc(key, id, original, opts);
  return at ? { key, range: at } : null;
}

/** Re-renders the caller when a tab submits again (its editor may have changed). */
export function useSubmissionsVersion(): number {
  const subscribe = useCallback((cb: () => void) => {
    listeners.add(cb);
    return () => void listeners.delete(cb);
  }, []);
  return useSyncExternalStore(subscribe, () => version);
}

/** Forgets a tab's submissions (it was closed for good). */
export function dropSubmissions(tabId: string): void {
  for (const kind of ["run", "plan"]) {
    const slot = `${tabId}:${kind}`;
    const id = latest.get(slot);
    if (id !== undefined) infos.delete(id);
    latest.delete(slot);
  }
}
