import { toast } from "sonner";
import { refreshOpenTableTabs } from "@/features/commit/refreshTables";
import type { RunState } from "@/features/editor/runModel";
import { charToUtf16 } from "@/features/editor/offsets";
import { skippedSpans, type SqlDialect } from "@/features/editor/scanner";
import { canRedo, discardAll, redo, setRowError } from "@/features/staging/model";
import type { ApplyRequest, RowResult } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { getQueryTab } from "@/stores/queryTabs";
import { resultData } from "@/stores/resultData";
import { useTabsStore } from "@/stores/tabs";
import { applyCommitted, toResultChanges } from "./changes";
import { TARGET_CHANGED, sameTarget, type ResultEditTarget } from "./editability";
import {
  dirtyResultOf,
  pinsOf,
  setResultStaging,
  stagedTargetOf,
  stagingOf,
  useResultsStore,
} from "./store";

/**
 * The staged edits of a SQL editor result are part of the workspace commit (⌘S commits every
 * pending change of the connection, `features/commit/workspace.ts`): this module plans a tab's
 * part (`planResultCommit`) and settles it afterwards. Committed rows are updated in place;
 * re-running the statement is offered.
 */

/** A statement to run again in the editor (`runQuery`'s submission). */
export interface Rerun {
  sql: string;
  baseOffset: number;
}

type RerunHandler = (tabId: string, sub: Rerun) => void;
let rerunHandler: RerunHandler | null = null;

/** Set by the results pane (which may import the editor controller; this module must not). */
export function setResultRerun(handler: RerunHandler | null): void {
  rerunHandler = handler;
}

function connectionOf(tabId: string): string | null {
  const tab = useTabsStore.getState().tabs.find((t) => t.id === tabId);
  return tab?.kind === "query" ? tab.connectionId : null;
}

/** Whether ⌘S in this query tab commits result edits (rather than saving the file). */
export function hasPendingResultEdits(tabId: string): boolean {
  return dirtyResultOf(useResultsStore.getState(), tabId) !== null;
}

export interface PlannedResult {
  tabId: string;
  resultId: string;
  connectionId: string;
  target: ResultEditTarget;
  req: ApplyRequest;
}

/** A tab's part of the workspace commit, or why it cannot be committed. */
export type ResultPlan =
  { ok: true; planned: PlannedResult } | { ok: false; problem: string; detail?: string };

/**
 * What committing the tab's dirty result would send; null when it has nothing. The edits go to
 * the target they were staged against, and only while the result still has that target: if the
 * table's structure changed (its key, a failed reload…), the staged row keys may name other rows,
 * so nothing is sent.
 */
export function planResultCommit(tabId: string): ResultPlan | null {
  const st = useResultsStore.getState();
  const resultId = dirtyResultOf(st, tabId);
  const connectionId = connectionOf(tabId);
  if (!resultId || !connectionId) return null;
  const editability = st.editability[resultId];
  if (editability?.kind !== "editable")
    return {
      ok: false,
      problem: "These changes can't be committed",
      detail: "The result's source table is no longer known. Discard them.",
    };
  const target = stagedTargetOf(st, resultId);
  if (!target || !sameTarget(target, editability.target))
    return { ok: false, problem: TARGET_CHANGED };
  const changes = toResultChanges(stagingOf(st, resultId), target);
  if (changes.length === 0) return null;
  return {
    ok: true,
    planned: {
      tabId,
      resultId,
      connectionId,
      target,
      req: { table: target.table, changes, returning: true },
    },
  };
}

/** The statement a result came from, when running it again cannot change anything. */
export function rerunOf(run: RunState | null, resultId: string, dialect: SqlDialect): Rerun | null {
  const info = run?.results.find((r) => r.id === resultId);
  const st = info && run?.statements.find((s) => s.index === info.statementIndex);
  if (!run || !st) return null;
  const start = charToUtf16(run.sql, st.span.start);
  const end = charToUtf16(run.sql, st.span.end);
  const sql = run.sql.slice(start, end);
  return isReadOnlyQuery(sql, dialect) ? { sql, baseOffset: run.baseOffset + start } : null;
}

/** A plain read (SELECT / TABLE / VALUES / SHOW, or a WITH that writes nothing). */
export function isReadOnlyQuery(sql: string, dialect: SqlDialect): boolean {
  let code = sql;
  for (const span of skippedSpans(sql, dialect).reverse())
    code = code.slice(0, span.start) + " ".repeat(span.end - span.start) + code.slice(span.end);
  const lower = code.toLowerCase();
  const first = /[a-z]+/.exec(lower)?.[0];
  if (!first || !["select", "with", "table", "values", "show"].includes(first)) return false;
  return !/\b(insert|update|delete|merge|call|into|for\s+update|for\s+share)\b/.test(lower);
}

/** After a failed commit: the failed rows carry their error. */
export function failResultCommit(p: PlannedResult, rows: readonly RowResult[]): void {
  setResultStaging(p.tabId, p.resultId, (s) =>
    rows.reduce((acc, r) => (r.error ? setRowError(acc, r.rowId, r.error.message) : acc), s),
  );
}

/**
 * After a commit went through: the rows as they are now (edited in place, deleted ones gone, new
 * ones appended), the staged edits gone; re-running a plain read is offered.
 */
export function finishResultCommit(p: PlannedResult, rows: RowResult[]): void {
  const staging = stagingOf(useResultsStore.getState(), p.resultId);
  const current = resultData.get(p.resultId)?.rows ?? [];
  resultData.replaceRows(p.resultId, applyCommitted(current, staging, p.target, rows));
  setResultStaging(p.tabId, p.resultId, () => discardAll());
  // The table was written: its open table tabs (a split view, another tab) reload too.
  refreshOpenTableTabs(p.connectionId, p.target.table);
  const driver = useConnectionsStore.getState().byId[p.connectionId]?.info.driver ?? "postgres";
  const rerun = p.resultId.startsWith("pin:")
    ? null
    : rerunOf(getQueryTab(p.tabId).run, p.resultId, driver);
  if (rerun && rerunHandler)
    toast.message("The result's rows were updated in place", {
      action: { label: "Re-run", onClick: () => rerunHandler?.(p.tabId, rerun) },
    });
}

/** ⇧⌘Z in a result grid: redoes the last undone edit of the tab's dirty result. */
export function redoResultEdit(tabId: string, resultId: string): void {
  setResultStaging(tabId, resultId, (s) => (canRedo(s) ? redo(s) : s));
}

/** Whether the result `resultId` (a run result or a pin of `tabId`) still exists. */
export function resultExists(tabId: string, resultId: string): boolean {
  if (getQueryTab(tabId).run?.results.some((r) => r.id === resultId)) return true;
  return pinsOf(useResultsStore.getState(), tabId).some((p) => p.id === resultId);
}
