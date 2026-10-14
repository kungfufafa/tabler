import { actions, type ActionId } from "@/features/palette/actions";

/** How a tab's own commit (its staged rows, structure or result edits) ended. */
export type TabCommitOutcome = "committed" | "failed" | "cancelled";

type OutcomeListener = (tabId: string, outcome: TabCommitOutcome) => void;

const outcomeListeners = new Set<OutcomeListener>();

/**
 * Reported by the commit paths of a tab (table rows, structure, query results): "committed" once
 * its staged changes were applied, "failed" when the apply failed, "cancelled" when the commit
 * stopped before anything ran (its Preview SQL dismissed, the safe mode said no, nothing to send).
 */
export function reportTabCommit(tabId: string, outcome: TabCommitOutcome): void {
  for (const listener of [...outcomeListeners]) listener(tabId, outcome);
}

export interface CommitChainOptions {
  /** The tab whose own commit goes first. */
  tabId: string;
  /** Its staged changes: any change other than the commit's own ends the wait. */
  pending: () => number;
  /** False once the tab is closed or no longer the active one. */
  alive: () => boolean;
  /** Calls its argument whenever `pending` or `alive` may have changed. */
  subscribe: (onChange: () => void) => () => void;
  /** Runs once, after the tab reported "committed". */
  then: () => void;
  /** Gives up after this long (the tab's preview sheet may stay open a while). */
  timeoutMs?: number;
}

/** Actions after which the waiting stops: the tab's changes were dropped or edited instead. */
const STOP_ON: readonly ActionId[] = ["grid.discard", "grid.undo", "grid.redo"];

let current: (() => void) | null = null;

/**
 * "Commit the active tab, THEN the pending object ops": waits for the tab's own commit (its
 * handler, maybe through its Preview SQL sheet) and fires `then` only when that tab reports
 * "committed". Anything else stops the wait without firing: a "failed" / "cancelled" report, the
 * tab's changes changing without a commit (a discard by any path, a reverted row, new edits), the
 * tab closing or losing focus, discard / undo / redo, a newer chain, or `timeoutMs`. Returns the
 * cancel function.
 */
export function armCommitChain(o: CommitChainOptions): () => void {
  current?.();
  const armed = o.pending();
  let stopped = false;
  const offs: Array<() => void> = [];
  const stop = () => {
    if (stopped) return;
    stopped = true;
    for (const off of offs) off();
    if (current === stop) current = null;
  };
  const onOutcome: OutcomeListener = (tabId, outcome) => {
    if (stopped || tabId !== o.tabId) return;
    stop();
    if (outcome === "committed") o.then();
  };
  const check = () => {
    if (stopped) return;
    if (!o.alive()) return stop();
    const now = o.pending();
    if (now === armed) return;
    if (now > 0) return stop();
    // A successful commit clears the changes in the same task as its report (just before or
    // after it): wait for the report until the task ends, then give up.
    const timer = setTimeout(stop, 0);
    offs.push(() => clearTimeout(timer));
  };
  outcomeListeners.add(onOutcome);
  offs.push(() => outcomeListeners.delete(onOutcome));
  offs.push(o.subscribe(check));
  for (const id of STOP_ON) offs.push(actions.observe(id, stop));
  const timer = setTimeout(stop, o.timeoutMs ?? 10 * 60_000);
  offs.push(() => clearTimeout(timer));
  current = stop;
  return stop;
}

/** Whether a chain is waiting for a tab's commit. */
export function commitChainArmed(): boolean {
  return current !== null;
}

/** Cancels the armed chain, if any. */
export function cancelCommitChain(): void {
  current?.();
}
