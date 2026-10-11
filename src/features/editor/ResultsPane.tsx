import { Loader2, Play, TriangleAlert } from "lucide-react";
import type { KeyboardEvent, ReactNode } from "react";
import { EmptyState } from "@/components/EmptyState";
import { formatDuration } from "@/lib/format/time";
import type { Dialect } from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import {
  updateQueryTab,
  useQueryTabField,
  type PlanState,
  type ResultPane,
} from "@/stores/queryTabs";
import { useResultRowCount } from "@/stores/resultData";
import { ElapsedTimer } from "./ElapsedTimer";
import { resultTabLabel, runStatusText } from "./labels";
import { MessagesView } from "./MessagesView";
import { PlanView } from "./PlanView";
import { ResultGrid } from "./ResultSetGrid";
import { summarize, type ResultSetInfo, type RunState } from "./runModel";

function PaneTab({
  active,
  onSelect,
  children,
  title,
}: {
  active: boolean;
  onSelect: () => void;
  children: ReactNode;
  title?: string;
}) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      tabIndex={active ? 0 : -1}
      title={title}
      onClick={onSelect}
      className={cn(
        "border-border relative flex h-full shrink-0 items-center gap-1.5 border-r px-3 text-[11px] whitespace-nowrap outline-none",
        "focus-visible:ring-brand/40 focus-visible:ring-1 focus-visible:ring-inset",
        active
          ? "bg-surface text-foreground"
          : "text-muted-foreground hover:bg-accent/60 hover:text-foreground",
      )}
    >
      {children}
      {active && <span className="bg-brand absolute inset-x-0 bottom-0 h-0.5" aria-hidden />}
    </button>
  );
}

function ResultTab({
  info,
  n,
  active,
  onSelect,
}: {
  info: ResultSetInfo;
  n: number;
  active: boolean;
  onSelect: () => void;
}) {
  const rows = useResultRowCount(info.id);
  return (
    <PaneTab active={active} onSelect={onSelect}>
      {!info.done && <Loader2 className="text-brand size-3 animate-spin" />}
      <span className="tabular-nums">{resultTabLabel(n, rows, info.durationMs)}</span>
      {info.truncated && <TriangleAlert className="text-warning size-3" aria-label="Truncated" />}
    </PaneTab>
  );
}

function RunStatus({
  run,
  plan,
  showPlan,
}: {
  run: RunState | null;
  plan: PlanState | null;
  showPlan: boolean;
}) {
  if (run?.status === "running") {
    const n = run.statements.length;
    return (
      <span className="flex items-center gap-1.5">
        <Loader2 className="text-brand size-3 animate-spin" />
        {run.cancelling ? "Stopping…" : n > 1 ? `Running statement ${n}…` : "Running…"}
        <ElapsedTimer since={run.startedAt} />
      </span>
    );
  }
  if (plan?.status === "running") {
    return (
      <span className="flex items-center gap-1.5">
        <Loader2 className="text-brand size-3 animate-spin" />
        Explaining…
      </span>
    );
  }
  if (showPlan && plan) {
    const title = plan.mode === "analyze" ? "Explain analyze" : "Explain";
    if (plan.status === "error") return <span className="text-error">{title} failed</span>;
    if (plan.status === "cancelled") return <span className="text-warning">{title} cancelled</span>;
    return (
      <span className="tabular-nums">
        {title}
        {plan.result ? ` · ${formatDuration(plan.result.durationMs)}` : ""}
      </span>
    );
  }
  if (!run) return null;
  const s = summarize(run);
  return (
    <span
      className={cn(
        "tabular-nums",
        s.errors > 0 && "text-error",
        s.cancelled && s.errors === 0 && "text-warning",
      )}
    >
      {runStatusText(s)}
    </span>
  );
}

function resolvePane(
  pane: ResultPane,
  results: readonly ResultSetInfo[],
  plan: PlanState | null,
): ResultPane {
  if (pane === "plan") return plan ? "plan" : (results[0]?.id ?? "messages");
  if (pane === "messages") return "messages";
  return results.some((r) => r.id === pane) ? pane : (results[0]?.id ?? "messages");
}

function ResultsEmpty() {
  const k = (id: Parameters<typeof shortcutFor>[0]) => formatCombo(shortcutFor(id) ?? "");
  return (
    <EmptyState
      icon={Play}
      title="Run a query"
      description={`${k("query.run")} runs the selection or the whole editor, ${k("query.run-current")} the statement under the cursor, ${k("query.explain")} explains it.`}
    />
  );
}

/** Result sets, messages and plan of the tab's latest run, as sub-tabs below the editor. */
export function ResultsPane({ tabId, dialect }: { tabId: string; dialect: Dialect }) {
  const run = useQueryTabField(tabId, "run");
  const plan = useQueryTabField(tabId, "plan");
  const pane = useQueryTabField(tabId, "pane");
  if (!run && !plan) return <ResultsEmpty />;

  const results = run?.results ?? [];
  const active = resolvePane(pane, results, plan);
  const panes: ResultPane[] = [
    ...results.map((r) => r.id),
    ...(run ? (["messages"] as const) : []),
    ...(plan ? (["plan"] as const) : []),
  ];
  const select = (p: ResultPane) => updateQueryTab(tabId, { pane: p });
  const errors = run ? summarize(run).errors : 0;
  const activeIndex = results.findIndex((r) => r.id === active);
  const activeResult = activeIndex >= 0 ? results[activeIndex] : undefined;

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    const i = panes.indexOf(active);
    const next = panes[(i + (e.key === "ArrowRight" ? 1 : -1) + panes.length) % panes.length];
    if (next === undefined) return;
    e.preventDefault();
    select(next);
    const tabs = e.currentTarget.querySelectorAll<HTMLElement>("[role=tab]");
    requestAnimationFrame(() => tabs[panes.indexOf(next)]?.focus());
  };

  return (
    <div className="bg-surface flex h-full min-h-0 flex-col">
      <div className="border-border bg-surface-2 flex h-8 shrink-0 items-stretch border-b">
        <div
          role="tablist"
          aria-label="Results"
          className="flex min-w-0 flex-1 [scrollbar-width:none] items-stretch overflow-x-auto"
          onKeyDown={onKeyDown}
        >
          {results.map((r, i) => (
            <ResultTab
              key={r.id}
              info={r}
              n={i + 1}
              active={active === r.id}
              onSelect={() => select(r.id)}
            />
          ))}
          {run && (
            <PaneTab active={active === "messages"} onSelect={() => select("messages")}>
              Messages
              {errors > 0 && (
                <span className="bg-error/15 text-error rounded-full px-1.5 text-[10px] font-medium tabular-nums">
                  {errors}
                </span>
              )}
            </PaneTab>
          )}
          {plan && (
            <PaneTab active={active === "plan"} onSelect={() => select("plan")}>
              Plan
            </PaneTab>
          )}
        </div>
        <div className="text-muted-foreground flex shrink-0 items-center px-3 text-[11px]">
          <RunStatus run={run} plan={plan} showPlan={active === "plan"} />
        </div>
      </div>
      <div className="min-h-0 flex-1" role="tabpanel">
        {active === "plan" && plan ? (
          <PlanView tabId={tabId} plan={plan} dialect={dialect} />
        ) : activeResult ? (
          <ResultGrid
            key={activeResult.id}
            info={activeResult}
            label={`Result ${activeIndex + 1}`}
            dialect={dialect}
          />
        ) : run ? (
          <MessagesView tabId={tabId} run={run} />
        ) : null}
      </div>
    </div>
  );
}
