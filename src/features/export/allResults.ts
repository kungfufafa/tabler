import { getQueryTab } from "@/stores/queryTabs";
import { resultData } from "@/stores/resultData";
import { useTabsStore } from "@/stores/tabs";
import { resultTable } from "./resultTable";
import type { ResultsExportItem } from "./options";

/** A finished result set of a run, as the "All results" export takes it. */
export interface RunResult extends ResultsExportItem {
  /** The row cap cut the result: more rows exist than the ones held. */
  truncated: boolean;
  /** The statement was cancelled or failed after some rows arrived (null when it completed). */
  stopped: "cancelled" | "error" | null;
}

/**
 * The finished result sets of the run a query tab shows, in tab order, as export items. Modelled
 * on the controller's `shownResultSource`: the same buffers, labels ("Result 2") and tables, and
 * the same `stopped` reading of the statement's status. Rows are the buffers' own (not copied):
 * the request is built and sent right away.
 */
export function runResultsOf(tabId: string): RunResult[] {
  const { run } = getQueryTab(tabId);
  if (!run) return [];
  const out: RunResult[] = [];
  run.results.forEach((info, index) => {
    if (!info.done) return;
    const buffer = resultData.get(info.id);
    if (!buffer) return;
    const columns = buffer.columns.length > 0 ? buffer.columns : info.columns;
    const statement = run.statements.find((s) => s.index === info.statementIndex);
    // Without its statement (the command itself failed), the run's outcome says how it ended.
    const status = statement?.status ?? (run.status === "done" ? "ok" : run.status);
    out.push({
      label: `Result ${index + 1}`,
      columns: columns.map((c) => ({ ...c })),
      rows: buffer.rows,
      table: resultTable(columns, statement?.sql ?? null),
      truncated: info.truncated,
      stopped: status === "ok" ? null : status === "cancelled" ? "cancelled" : "error",
    });
  });
  return out;
}

/**
 * The results "All results" exports when the dialog was opened for `held` (a shown result of the
 * active query tab): every finished result of that tab's run, when it holds two or more and `held`
 * is one of them (a pinned result of an earlier run is not); else null.
 */
export function allResultsFor(
  connectionId: string,
  held: { label: string; columns: readonly { name: string }[]; rows: readonly unknown[] },
): RunResult[] | null {
  const { tabs, activeId } = useTabsStore.getState();
  const tab = tabs.find((t) => t.id === activeId);
  if (!tab || tab.kind !== "query" || tab.connectionId !== connectionId) return null;
  const results = runResultsOf(tab.id);
  if (results.length < 2) return null;
  const same = results.some(
    (r) =>
      r.label === held.label &&
      r.rows.length === held.rows.length &&
      r.columns.length === held.columns.length &&
      r.columns.every((c, i) => c.name === held.columns[i]?.name),
  );
  return same ? results : null;
}
