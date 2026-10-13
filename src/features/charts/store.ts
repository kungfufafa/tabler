import { create } from "zustand";
import { getEditorState } from "@/features/editor/editorStates";
import { spanRange } from "@/features/editor/errorPos";
import { charToUtf16 } from "@/features/editor/offsets";
import type { RunState } from "@/features/editor/runModel";
import { locateSubmitted } from "@/features/editor/submissions";
import { directiveAbove, type ChartDirective } from "./directive";

/** What a result with a chart directive shows: its chart (the default) or its grid. */
export type ResultView = "chart" | "grid";

interface ChartViews {
  /** By result buffer id; unset: the default. */
  views: Record<string, ResultView>;
}

export const useChartViews = create<ChartViews>()(() => ({ views: {} }));

export function setResultView(resultId: string, view: ResultView): void {
  useChartViews.setState((s) =>
    s.views[resultId] === view ? s : { views: { ...s.views, [resultId]: view } },
  );
}

/** The result's view: the chart when it has a directive, unless the user picked the grid. */
export function useResultView(resultId: string, hasChart: boolean): ResultView {
  const picked = useChartViews((s) => s.views[resultId]);
  return hasChart ? (picked ?? "chart") : "grid";
}

/** Directives found per result id, kept for pinned copies (whose run text is gone). */
const found = new Map<string, ChartDirective | null>();
const MAX_REMEMBERED = 500;

/** How far above a statement the editor document is searched for its directive. */
const LOOK_BACK = 4000;

/**
 * The chart directive of the statement that produced result set `statementIndex` of `run`: the
 * comment right above the statement, in the submitted text or else in the editor (⌘↩ submits
 * the statement without the comments before it). Undefined while the statement is not known.
 */
export function runDirective(
  tabId: string,
  run: RunState,
  statementIndex: number,
): ChartDirective | null | undefined {
  const st = run.statements.find((s) => s.index === statementIndex);
  if (!st || st.sql === "") return undefined;
  const submitted = directiveAbove(run.sql, charToUtf16(run.sql, st.span.start));
  if (submitted) return submitted;
  const at = locateSubmitted(tabId, run.id, spanRange(st.span, run));
  const doc = at ? getEditorState(at.key)?.doc : undefined;
  if (!at || !doc || at.range.from > doc.length) return null;
  const from = Math.max(0, at.range.from - LOOK_BACK);
  return directiveAbove(doc.sliceString(from, at.range.from), at.range.from - from);
}

/**
 * The directive of a result: from the run that holds it, or (a pinned copy) the one remembered
 * for the result it was pinned from.
 */
export function directiveFor(
  resultId: string,
  source: { tabId: string; run: RunState; statementIndex: number } | { pinnedFrom: string },
): ChartDirective | null {
  if ("pinnedFrom" in source) return found.get(source.pinnedFrom) ?? null;
  if (found.has(resultId)) return found.get(resultId) ?? null;
  const d = runDirective(source.tabId, source.run, source.statementIndex);
  if (d === undefined) return null;
  if (found.size >= MAX_REMEMBERED) {
    const oldest = found.keys().next().value;
    if (oldest !== undefined) found.delete(oldest);
  }
  found.set(resultId, d);
  return d;
}
