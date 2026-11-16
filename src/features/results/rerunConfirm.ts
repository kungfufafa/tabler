import { create } from "zustand";

/**
 * The answer to "Discard N uncommitted result changes and run?": commit them first and then run,
 * discard them and run, or neither (false: Cancel, Esc, a newer question).
 */
export type RerunAnswer = "commit" | "discard" | false;

/**
 * "Discard N uncommitted result changes and run?": asked before a run from the SQL editor
 * replaces results that hold staged edits (`runQuery`). `RerunConfirmDialog` shows it.
 */
export interface RerunConfirmRequest {
  tabId: string;
  /** Staged changes the run would discard. */
  count: number;
  resolve: (answer: RerunAnswer) => void;
}

export const useRerunConfirmStore = create<{ request: RerunConfirmRequest | null }>()(() => ({
  request: null,
}));

/** Asks; resolves with the button chosen. A newer question cancels an older one. */
export function confirmRerunDiscard(tabId: string, count: number): Promise<RerunAnswer> {
  return new Promise((resolve) => {
    useRerunConfirmStore.getState().request?.resolve(false);
    useRerunConfirmStore.setState({ request: { tabId, count, resolve } });
  });
}

/** Answers the open question (no-op without one). */
export function answerRerunConfirm(answer: RerunAnswer): void {
  const request = useRerunConfirmStore.getState().request;
  if (!request) return;
  useRerunConfirmStore.setState({ request: null });
  request.resolve(answer);
}
