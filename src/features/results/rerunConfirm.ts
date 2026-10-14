import { create } from "zustand";

/**
 * "Discard N uncommitted result changes and run?": asked before a run from the SQL editor
 * replaces results that hold staged edits (`runQuery`). `RerunConfirmDialog` shows it.
 */
export interface RerunConfirmRequest {
  tabId: string;
  /** Staged changes the run would discard. */
  count: number;
  resolve: (discard: boolean) => void;
}

export const useRerunConfirmStore = create<{ request: RerunConfirmRequest | null }>()(() => ({
  request: null,
}));

/** Asks; resolves true for Discard & Run, false for Cancel. A newer question cancels an older one. */
export function confirmRerunDiscard(tabId: string, count: number): Promise<boolean> {
  return new Promise((resolve) => {
    useRerunConfirmStore.getState().request?.resolve(false);
    useRerunConfirmStore.setState({ request: { tabId, count, resolve } });
  });
}

/** Answers the open question (no-op without one). */
export function answerRerunConfirm(discard: boolean): void {
  const request = useRerunConfirmStore.getState().request;
  if (!request) return;
  useRerunConfirmStore.setState({ request: null });
  request.resolve(discard);
}
