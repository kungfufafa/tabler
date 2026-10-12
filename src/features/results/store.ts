import { toast } from "sonner";
import { create } from "zustand";
import type { ResultSetInfo } from "@/features/editor/runModel";
import { emptyStaging, pendingCount } from "@/features/staging/model";
import type { StagingState } from "@/features/staging/types";
import { PREVIEW_CLOSED, type PreviewState } from "@/features/table-tab/previewState";
import { useQueryTabsStore } from "@/stores/queryTabs";
import { dropResults, resultData } from "@/stores/resultData";
import { registerUnsavedSource, useTabsStore } from "@/stores/tabs";
import { sameTarget, type ResultEditability, type ResultEditTarget } from "./editability";

/**
 * Per-result state of the SQL editor's results that outlives the grid (only the visible tab is
 * mounted): staged edits, what the grid found out about editability, pinned results and the
 * Preview SQL sheet of a result commit. Keys are `resultData` buffer ids.
 */

export interface ResultEdits {
  tabId: string;
  staging: StagingState;
  /**
   * The edit target the first change was staged against. Every later change and the commit must
   * see the same one (`sameTarget`): staged row keys are only valid for that table.
   */
  target: ResultEditTarget | null;
}

/** A result kept across re-runs (Pin Result): a copy of the buffer under its own id. */
export interface PinnedResult {
  /** Buffer id of the copy: `pin:<uuid>`. */
  id: string;
  /** The run result it was pinned from (hidden from the strip while that run is shown). */
  sourceId: string;
  label: string;
  info: ResultSetInfo;
  /** The statement that produced it. */
  sql: string | null;
}

export interface ResultPreview extends PreviewState {
  tabId: string | null;
  resultId: string | null;
}

interface ResultsState {
  edits: Record<string, ResultEdits>;
  editability: Record<string, ResultEditability>;
  /** By query tab id, in pin order. */
  pins: Record<string, PinnedResult[]>;
  preview: ResultPreview;
}

export const CLOSED_PREVIEW: ResultPreview = { ...PREVIEW_CLOSED, tabId: null, resultId: null };

export const useResultsStore = create<ResultsState>()(() => ({
  edits: {},
  editability: {},
  pins: {},
  preview: CLOSED_PREVIEW,
}));

const EMPTY_STAGING = emptyStaging();
const NO_PINS: readonly PinnedResult[] = Object.freeze([]);

export const isPinId = (id: string): boolean => id.startsWith("pin:");

/** The staged edits of a result (a shared empty state when it has none). */
export function stagingOf(s: ResultsState, resultId: string): StagingState {
  return s.edits[resultId]?.staging ?? EMPTY_STAGING;
}

/** Staged changes over every result of a query tab. */
export function pendingForTab(s: ResultsState, tabId: string): number {
  let n = 0;
  for (const e of Object.values(s.edits)) if (e.tabId === tabId) n += pendingCount(e.staging);
  return n;
}

/** The result of the tab that holds staged edits (one at a time), if any. */
export function dirtyResultOf(s: ResultsState, tabId: string): string | null {
  for (const [id, e] of Object.entries(s.edits))
    if (e.tabId === tabId && pendingCount(e.staging) > 0) return id;
  return null;
}

export function pinsOf(s: ResultsState, tabId: string): readonly PinnedResult[] {
  return s.pins[tabId] ?? NO_PINS;
}

/** The edit target the result's staged changes were made against (null without changes). */
export function stagedTargetOf(s: ResultsState, resultId: string): ResultEditTarget | null {
  return s.edits[resultId]?.target ?? null;
}

/** Nothing staged that a new target could be confused with (no changes, nothing to undo). */
const nothingStaged = (staging: StagingState) =>
  pendingCount(staging) === 0 && staging.undo.length === 0;

/**
 * Whether changes against `target` may be staged on the result: it holds none (nor any to undo),
 * or they were made against the same target.
 */
export function canStageAgainst(
  s: ResultsState,
  resultId: string,
  target: ResultEditTarget,
): boolean {
  const staged = s.edits[resultId];
  return !staged?.target || nothingStaged(staged.staging) || sameTarget(staged.target, target);
}

/**
 * Applies `fn` to the result's staged changes. `target`: the edit target a new change is made
 * against (callers check `canStageAgainst` first); it is recorded when nothing was staged yet,
 * and kept for every later change.
 */
export function setResultStaging(
  tabId: string,
  resultId: string,
  fn: (s: StagingState) => StagingState,
  target: ResultEditTarget | null = null,
): void {
  useResultsStore.setState((st) => {
    const cur = stagingOf(st, resultId);
    const next = fn(cur);
    if (next === cur) return st;
    const edits = { ...st.edits };
    const prev = st.edits[resultId]?.target ?? null;
    if (pendingCount(next) === 0 && next.undo.length === 0 && next.redo.length === 0)
      delete edits[resultId];
    else
      edits[resultId] = {
        tabId,
        staging: next,
        target: target && (!prev || nothingStaged(cur)) ? target : prev,
      };
    return { edits };
  });
}

export function setResultEditability(resultId: string, e: ResultEditability): void {
  useResultsStore.setState((st) =>
    st.editability[resultId] === e ? st : { editability: { ...st.editability, [resultId]: e } },
  );
}

function dropKeys(pred: (resultId: string, tabId: string | null) => boolean): number {
  let pending = 0;
  useResultsStore.setState((st) => {
    const edits = { ...st.edits };
    const editability = { ...st.editability };
    let changed = false;
    for (const [id, e] of Object.entries(st.edits)) {
      if (!pred(id, e.tabId)) continue;
      pending += pendingCount(e.staging);
      delete edits[id];
      changed = true;
    }
    for (const id of Object.keys(st.editability)) {
      if (!pred(id, null)) continue;
      delete editability[id];
      changed = true;
    }
    return changed ? { edits, editability } : st;
  });
  return pending;
}

/** Forgets a result's staged edits (Discard). */
export function discardResultEdits(resultId: string): void {
  dropKeys((id) => id === resultId);
}

/**
 * Pins a result of the tab's run: its rows are copied to a buffer of its own, which a re-run
 * leaves alone. Staged edits move along. Returns the pinned result's id.
 */
export function pinResult(
  tabId: string,
  info: ResultSetInfo,
  label: string,
  sql: string | null,
): string | null {
  const id = `pin:${crypto.randomUUID()}`;
  if (!resultData.copy(info.id, id)) return null;
  useResultsStore.setState((st) => {
    const pin: PinnedResult = { id, sourceId: info.id, label, info: { ...info, id }, sql };
    const edits = { ...st.edits };
    const moved = edits[info.id];
    if (moved) {
      delete edits[info.id];
      edits[id] = moved;
    }
    return { pins: { ...st.pins, [tabId]: [...pinsOf(st, tabId), pin] }, edits };
  });
  return id;
}

/** Removes a pinned result (and its rows). */
export function unpinResult(tabId: string, pinId: string): void {
  useResultsStore.setState((st) => ({
    pins: { ...st.pins, [tabId]: pinsOf(st, tabId).filter((p) => p.id !== pinId) },
  }));
  dropKeys((id) => id === pinId);
  dropResults((id) => id === pinId);
}

export function openResultPreview(patch: Partial<ResultPreview>): void {
  useResultsStore.setState((st) => ({ preview: { ...st.preview, ...patch } }));
}

export function closeResultPreview(): void {
  useResultsStore.setState({ preview: CLOSED_PREVIEW });
}

// --- lifecycle ------------------------------------------------------------------------------

let installed = false;

/**
 * Keeps the per-result state in step with the tabs: a new run drops the staged edits of the
 * results it replaced (saying so when some were pending), a closed query tab its pins and edits.
 */
export function installResultsLifecycle(): void {
  if (installed) return;
  installed = true;
  // Closing a query tab (or quitting) with uncommitted result edits asks first, like a table tab.
  registerUnsavedSource((tab) =>
    tab.kind === "query" ? pendingForTab(useResultsStore.getState(), tab.id) : 0,
  );
  useQueryTabsStore.subscribe((s, prev) => {
    if (s.slices === prev.slices) return;
    for (const [tabId, before] of Object.entries(prev.slices)) {
      const old = before.run?.id;
      if (!old || s.slices[tabId]?.run?.id === old) continue;
      const lost = dropKeys((id) => id.startsWith(`${old}:`));
      if (lost > 0)
        toast.message(
          `${lost} uncommitted result change${lost === 1 ? " was" : "s were"} discarded`,
          {
            description: "Running the query again replaced the result they were made in.",
          },
        );
    }
  });
  useTabsStore.subscribe((s, prev) => {
    if (s.tabs === prev.tabs) return;
    const open = new Set(s.tabs.map((t) => t.id));
    for (const t of prev.tabs) {
      if (t.kind !== "query" || open.has(t.id)) continue;
      const pins = pinsOf(useResultsStore.getState(), t.id);
      dropKeys((id, tabId) => tabId === t.id || pins.some((p) => p.id === id));
      if (pins.length > 0) {
        const ids = new Set(pins.map((p) => p.id));
        dropResults((id) => ids.has(id));
        useResultsStore.setState((st) => {
          const next = { ...st.pins };
          delete next[t.id];
          return { pins: next };
        });
      }
    }
  });
}
