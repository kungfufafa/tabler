import { create } from "zustand";
import type { DdlOp, TableRef } from "@/lib/ipc";
import { retargetOps } from "./model";

/** Staged DDL of one structure tab: the op list plus the earlier lists for undo / redo. */
export interface StagedSlice {
  ops: DdlOp[];
  past: DdlOp[][];
  /** Lists undone since the last edit (redo pops from the end). */
  future?: DdlOp[][];
  /**
   * Set when the last apply stopped partway on a non-transactional plan (MySQL): the number of
   * statements that already ran, so the staged ops no longer match the table. Cleared with the
   * slice (discard, successful apply).
   */
  partial?: number;
  /**
   * Set when the last apply failed with nothing applied (a rolled-back transaction, or a single
   * `ALTER TABLE`, which fails as a whole): "All changes were reverted", the table is as it was
   * and the staged ops are still here. Cleared by the next edit, undo or redo.
   */
  reverted?: boolean;
}

export const EMPTY_SLICE: StagedSlice = Object.freeze({ ops: [], past: [] }) as StagedSlice;
const NO_OPS: DdlOp[] = EMPTY_SLICE.ops;
const MAX_HISTORY = 200;

/** Records `next` as the staged list (no-op when the reducer returned the same list). */
export function pushOps(slice: StagedSlice, next: DdlOp[]): StagedSlice {
  if (next === slice.ops) return slice;
  const { reverted: _reverted, ...rest } = slice;
  return {
    ...rest,
    ops: next,
    past: [...slice.past, slice.ops].slice(-MAX_HISTORY),
    future: [],
  };
}

/** Steps back to the op list before the last edit. */
export function undoOps(slice: StagedSlice): StagedSlice {
  const prev = slice.past[slice.past.length - 1];
  if (!prev) return slice;
  const { reverted: _reverted, ...rest } = slice;
  return {
    ...rest,
    ops: prev,
    past: slice.past.slice(0, -1),
    future: [...(slice.future ?? []), slice.ops],
  };
}

/** Steps forward again to the op list an undo stepped back from. */
export function redoOps(slice: StagedSlice): StagedSlice {
  const future = slice.future ?? [];
  const next = future[future.length - 1];
  if (!next) return slice;
  const { reverted: _reverted, ...rest } = slice;
  return {
    ...rest,
    ops: next,
    past: [...slice.past, slice.ops].slice(-MAX_HISTORY),
    future: future.slice(0, -1),
  };
}

interface StructureState {
  byTab: Record<string, StagedSlice>;
  /** Replaces the staged ops of `tabId` with `update(current)`, remembering the old list. */
  stage: (tabId: string, update: (ops: DdlOp[]) => DdlOp[]) => void;
  undo: (tabId: string) => void;
  redo: (tabId: string) => void;
  /**
   * Replaces the staged ops of `tabId` without recording a history entry (a view definition
   * being typed: the editor keeps its own undo history).
   */
  replace: (tabId: string, ops: DdlOp[]) => void;
  /** Drops every staged op (and the undo history) of `tabId`. */
  discard: (tabId: string) => void;
  /** Records that an apply of `tabId` stopped after `ran` statements had run (see `partial`). */
  markPartial: (tabId: string, ran: number) => void;
  /** Records that an apply of `tabId` failed with nothing applied (see `reverted`). */
  markReverted: (tabId: string) => void;
  /** Points the staged ops (and undo history) of `tabIds` at the renamed table `to`. */
  retarget: (tabIds: readonly string[], from: TableRef, to: TableRef) => void;
  /** Forgets the slices of closed tabs. */
  remove: (tabIds: readonly string[]) => void;
  /** Keeps only the slices of `tabIds` (lazy cleanup after tabs were closed). */
  retain: (tabIds: ReadonlySet<string>) => void;
}

export const useStructureStore = create<StructureState>()((set) => ({
  byTab: {},
  stage: (tabId, update) =>
    set((s) => {
      const slice = s.byTab[tabId] ?? EMPTY_SLICE;
      const next = pushOps(slice, update(slice.ops));
      return next === slice ? s : { byTab: { ...s.byTab, [tabId]: next } };
    }),
  undo: (tabId) =>
    set((s) => {
      const slice = s.byTab[tabId];
      if (!slice) return s;
      const next = undoOps(slice);
      return next === slice ? s : { byTab: { ...s.byTab, [tabId]: next } };
    }),
  redo: (tabId) =>
    set((s) => {
      const slice = s.byTab[tabId];
      if (!slice) return s;
      const next = redoOps(slice);
      return next === slice ? s : { byTab: { ...s.byTab, [tabId]: next } };
    }),
  replace: (tabId, ops) =>
    set((s) => {
      const slice = s.byTab[tabId] ?? EMPTY_SLICE;
      if (slice.ops === ops) return s;
      if (ops.length === 0 && slice.past.length === 0) {
        if (!s.byTab[tabId]) return s;
        const byTab = { ...s.byTab };
        delete byTab[tabId];
        return { byTab };
      }
      return { byTab: { ...s.byTab, [tabId]: { ...slice, ops } } };
    }),
  discard: (tabId) =>
    set((s) => {
      if (!s.byTab[tabId]) return s;
      const byTab = { ...s.byTab };
      delete byTab[tabId];
      return { byTab };
    }),
  markPartial: (tabId, ran) =>
    set((s) => {
      const slice = s.byTab[tabId] ?? EMPTY_SLICE;
      if (slice.partial === ran) return s;
      return { byTab: { ...s.byTab, [tabId]: { ...slice, partial: ran } } };
    }),
  markReverted: (tabId) =>
    set((s) => {
      const slice = s.byTab[tabId] ?? EMPTY_SLICE;
      if (slice.reverted) return s;
      return { byTab: { ...s.byTab, [tabId]: { ...slice, reverted: true } } };
    }),
  retarget: (tabIds, from, to) =>
    set((s) => {
      const ids = tabIds.filter((id) => id in s.byTab);
      if (ids.length === 0) return s;
      const byTab = { ...s.byTab };
      for (const id of ids) {
        const slice = byTab[id]!;
        byTab[id] = {
          ...slice,
          ops: retargetOps(slice.ops, from, to),
          past: slice.past.map((ops) => retargetOps(ops, from, to)),
          ...(slice.future && {
            future: slice.future.map((ops) => retargetOps(ops, from, to)),
          }),
        };
      }
      return { byTab };
    }),
  remove: (tabIds) =>
    set((s) => {
      if (!tabIds.some((id) => id in s.byTab)) return s;
      const byTab = { ...s.byTab };
      for (const id of tabIds) delete byTab[id];
      return { byTab };
    }),
  retain: (tabIds) =>
    set((s) => {
      const stale = Object.keys(s.byTab).filter((id) => !tabIds.has(id));
      if (stale.length === 0) return s;
      const byTab = { ...s.byTab };
      for (const id of stale) delete byTab[id];
      return { byTab };
    }),
}));

/** Staged ops of a tab (a stable empty array when nothing is staged). */
export function useStagedOps(tabId: string): DdlOp[] {
  return useStructureStore((s) => s.byTab[tabId]?.ops ?? NO_OPS);
}

/** Statements that ran before the last apply of the tab stopped partway; 0 when none. */
export function usePartialApply(tabId: string): number {
  return useStructureStore((s) => s.byTab[tabId]?.partial ?? 0);
}

/** Whether the last apply of the tab failed with everything reverted (see `reverted`). */
export function useRevertedApply(tabId: string): boolean {
  return useStructureStore((s) => s.byTab[tabId]?.reverted ?? false);
}

export function useCanUndo(tabId: string): boolean {
  return useStructureStore((s) => (s.byTab[tabId]?.past.length ?? 0) > 0);
}

export function useCanRedo(tabId: string): boolean {
  return useStructureStore((s) => (s.byTab[tabId]?.future?.length ?? 0) > 0);
}

export function stagedOps(tabId: string): DdlOp[] {
  return useStructureStore.getState().byTab[tabId]?.ops ?? NO_OPS;
}

/**
 * Number of staged structure changes of a tab, for the unsaved-changes guard (tab close, window
 * close). Meant for `unsavedCount` in `src/stores/tabs.ts`.
 */
export function structurePendingCount(tabId: string): number {
  return stagedOps(tabId).length;
}
