import { Loader2, Play, TriangleAlert } from "lucide-react";
import type { KeyboardEvent, ReactNode } from "react";
import { EmptyState } from "@/components/EmptyState";
import type { Dialect } from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { updateQueryTab, useQueryTabField, type ResultPane } from "@/stores/queryTabs";
import { useResultRowCount } from "@/stores/resultData";
import { resultTabLabel } from "./labels";
import { MessagesView } from "./MessagesView";
import { resolvePane } from "./panes";
import { PlanView } from "./PlanView";
import { ResultGrid } from "./ResultSetGrid";
import { summarize, type ResultSetInfo } from "./runModel";

/** One segment of the results strip. */
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
        "flex h-full shrink-0 items-center gap-1.5 rounded-[4px] px-2 text-[11px] whitespace-nowrap transition-colors outline-none",
        // Tailwind v4: `outline-none` sets --tw-outline-style:none, so the ring needs its style.
        "focus-visible:outline-ring/70 focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-solid",
        active
          ? "bg-surface text-foreground ring-border-strong/70 dark:bg-accent shadow-xs ring-1"
          : "text-muted-foreground hover:text-foreground",
      )}
    >
      {children}
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

function ResultsEmpty() {
  const k = (id: Parameters<typeof shortcutFor>[0]) => formatCombo(shortcutFor(id) ?? "");
  return (
    <EmptyState
      icon={Play}
      title="Run a query"
      description={`${k("query.run-current")} runs the statement under the cursor (or the selection), ${k("query.run")} runs everything, ${k("query.explain")} explains it.`}
    />
  );
}

/**
 * Result sets, messages and plan of the tab's latest run, below the editor bar: a compact
 * segmented strip (Result 1 / Result 2 / Messages / Plan) over the selected pane. The run's
 * outcome and timer show in the editor bar.
 */
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
      <div className="border-border bg-surface flex h-8 shrink-0 items-center border-b px-1.5">
        <div
          role="tablist"
          aria-label="Results"
          className="border-border bg-surface-2 dark:bg-background flex h-6.5 max-w-full min-w-0 [scrollbar-width:none] items-stretch gap-0.5 overflow-x-auto rounded-md border p-0.5"
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
                <span className="bg-error/15 text-error rounded-full px-1.5 text-[10px] leading-4 font-medium tabular-nums">
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
