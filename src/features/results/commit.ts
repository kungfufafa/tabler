import { toast } from "sonner";
import type { RunState } from "@/features/editor/runModel";
import { charToUtf16 } from "@/features/editor/offsets";
import { skippedSpans, type SqlDialect } from "@/features/editor/scanner";
import { guardExecution } from "@/features/safety/guard";
import { canRedo, discardAll, pendingCount, redo, setRowError } from "@/features/staging/model";
import { ipc, toIpcError, type ApplyRequest } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { getPrefs } from "@/stores/prefs";
import { getQueryTab } from "@/stores/queryTabs";
import { resultData } from "@/stores/resultData";
import { useTabsStore } from "@/stores/tabs";
import { applyCommitted, toResultChanges } from "./changes";
import { TARGET_CHANGED, sameTarget, type ResultEditTarget } from "./editability";
import {
  closeResultPreview,
  dirtyResultOf,
  discardResultEdits,
  openResultPreview,
  pinsOf,
  setResultStaging,
  stagedTargetOf,
  stagingOf,
  useResultsStore,
} from "./store";

/**
 * Committing the staged edits of a SQL editor result: the same Preview SQL / ⌘S flow as a table
 * tab (`ipc.mutate.preview` / `apply`, one transaction), behind the connection's safe-mode check.
 * Afterwards the edited rows are updated in place; re-running the statement is offered.
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

interface Planned {
  tabId: string;
  resultId: string;
  connectionId: string;
  target: ResultEditTarget;
  req: ApplyRequest;
}

/**
 * What committing the tab's dirty result would send; null (with a message) when it cannot. The
 * edits go to the target they were staged against, and only while the result still has that
 * target: if the table's structure changed (its key, a failed reload…), the staged row keys may
 * name other rows, so nothing is sent.
 */
function plan(tabId: string): Planned | null {
  const st = useResultsStore.getState();
  const resultId = dirtyResultOf(st, tabId);
  const connectionId = connectionOf(tabId);
  if (!resultId || !connectionId) return null;
  const editability = st.editability[resultId];
  if (editability?.kind !== "editable") {
    toast.error("These changes can't be committed", {
      description: "The result's source table is no longer known. Discard them.",
    });
    return null;
  }
  const target = stagedTargetOf(st, resultId);
  if (!target || !sameTarget(target, editability.target)) {
    toast.error(TARGET_CHANGED, { id: "result-target-changed" });
    return null;
  }
  const changes = toResultChanges(stagingOf(st, resultId), target);
  if (changes.length === 0) return null;
  return {
    tabId,
    resultId,
    connectionId,
    target,
    req: { table: target.table, changes, returning: true },
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

function failedRows(
  p: Planned,
  result: { results: { rowId: string; error: { message: string } | null }[] },
) {
  setResultStaging(p.tabId, p.resultId, (s) =>
    result.results.reduce(
      (acc, r) => (r.error ? setRowError(acc, r.rowId, r.error.message) : acc),
      s,
    ),
  );
}

/** Applies a planned commit; true when it went through. */
async function apply(p: Planned, statements: string[]): Promise<boolean> {
  const result = await ipc.mutate.apply(p.connectionId, p.req);
  if (!result.success) {
    failedRows(p, result);
    const failed = result.results.findIndex((r) => r.error);
    // Show what failed in the Preview SQL sheet (opened here when ⌘S skipped it).
    openResultPreview({
      open: true,
      tabId: p.tabId,
      resultId: p.resultId,
      statements: statements.length > 0 ? statements : result.sql,
      loading: false,
      applying: false,
      error: result.error?.message ?? "Failed",
      failedIndex: failed >= 0 ? failed : null,
    });
    return false;
  }
  // The rows as they are now: edited in place, deleted ones gone, new ones appended.
  const staging = stagingOf(useResultsStore.getState(), p.resultId);
  const rows = resultData.get(p.resultId)?.rows ?? [];
  resultData.replaceRows(p.resultId, applyCommitted(rows, staging, p.target, result.results));
  setResultStaging(p.tabId, p.resultId, () => discardAll());
  closeResultPreview();
  const n = result.results.length;
  const driver = useConnectionsStore.getState().byId[p.connectionId]?.info.driver ?? "postgres";
  const rerun = p.resultId.startsWith("pin:")
    ? null
    : rerunOf(getQueryTab(p.tabId).run, p.resultId, driver);
  toast.success(`Applied ${n} change${n === 1 ? "" : "s"} · ${result.durationMs} ms`, {
    description: rerun ? "The rows were updated in place." : undefined,
    action:
      rerun && rerunHandler
        ? { label: "Re-run", onClick: () => rerunHandler?.(p.tabId, rerun) }
        : undefined,
  });
  return true;
}

/**
 * ⌘S on a query tab with staged result edits: commits them (after the safe-mode check), or opens
 * the Preview SQL sheet when "Confirm before commit" is on. Resolves true when it committed.
 */
export async function commitResultEdits(tabId: string): Promise<boolean> {
  if (getPrefs().confirmCommit) {
    await previewResultEdits(tabId);
    return false;
  }
  const p = plan(tabId);
  if (!p) return false;
  try {
    const statements = await ipc.mutate.preview(p.connectionId, p.req);
    if (!(await guardExecution(p.connectionId, { kind: "commit", statements }))) return false;
    return await apply(p, statements);
  } catch (e) {
    toast.error("Could not apply changes", { description: toIpcError(e).message });
    return false;
  }
}

/** ⇧⌘P: the Preview SQL sheet for the tab's staged result edits. */
export async function previewResultEdits(tabId: string): Promise<void> {
  const p = plan(tabId);
  if (!p) return;
  openResultPreview({
    open: true,
    tabId,
    resultId: p.resultId,
    statements: [],
    loading: true,
    applying: false,
    error: null,
    failedIndex: null,
  });
  try {
    const statements = await ipc.mutate.preview(p.connectionId, p.req);
    openResultPreview({ statements, loading: false });
  } catch (e) {
    openResultPreview({ loading: false, error: toIpcError(e).message });
  }
}

/** The Preview SQL sheet's Apply (⌘↩). */
export async function applyPreviewedResultEdits(): Promise<void> {
  const preview = useResultsStore.getState().preview;
  if (!preview.tabId || preview.applying || preview.loading) return;
  const p = plan(preview.tabId);
  if (!p) return closeResultPreview();
  openResultPreview({ applying: true, error: null, failedIndex: null });
  try {
    const statements = await ipc.mutate.preview(p.connectionId, p.req);
    if (!(await guardExecution(p.connectionId, { kind: "commit", statements }))) {
      openResultPreview({ applying: false });
      return;
    }
    await apply(p, statements);
  } catch (e) {
    openResultPreview({ applying: false, error: toIpcError(e).message });
  }
}

/** ⇧⌘⌫: drops the staged edits of the tab's results. */
export function discardResultChanges(tabId: string): void {
  const st = useResultsStore.getState();
  const ids = Object.entries(st.edits)
    .filter(([, e]) => e.tabId === tabId && pendingCount(e.staging) > 0)
    .map(([id]) => id);
  if (ids.length === 0) return;
  for (const id of ids) discardResultEdits(id);
  toast.message("Changes discarded");
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
