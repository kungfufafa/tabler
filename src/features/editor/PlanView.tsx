import { Copy, Loader2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { writeClipboard } from "@/features/datagrid/clipboard";
import { formatDuration } from "@/lib/format/time";
import { appliesPartially, useCaps } from "@/lib/caps";
import type { Dialect } from "@/lib/ipc";
import type { PlanState } from "@/stores/queryTabs";
import { useTabsStore } from "@/stores/tabs";
import { ElapsedTimer } from "./ElapsedTimer";
import { ErrorBlock } from "./ErrorBlock";
import { errorRange } from "./errorPos";
import { firstLine } from "./labels";
import { StaticGrid } from "./ResultSetGrid";

/** The "Plan" pane: EXPLAIN output as text (when the dialect has a textual plan) or rows. */
export function PlanView({
  tabId,
  plan,
  dialect,
}: {
  tabId: string;
  plan: PlanState;
  /** Null: another engine's dialect (no core copy formats). */
  dialect: Dialect | null;
}) {
  const title = plan.mode === "analyze" ? "EXPLAIN ANALYZE" : "EXPLAIN";
  // Whether the session rolls an analyzed statement back: it runs in a transaction only where
  // the session's writes are all or nothing (`TxMode`).
  const connectionId = useTabsStore((s) => s.tabs.find((t) => t.id === tabId)?.connectionId);
  const caps = useCaps(connectionId ?? null);
  const rollsBack = caps === null || !appliesPartially(caps.data.transactions);
  const text = plan.result?.text ?? null;
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="border-gridline text-muted-foreground flex h-7 shrink-0 items-center gap-2 border-b px-3 text-[11px]">
        <span className="text-foreground/80 font-semibold tracking-wide">{title}</span>
        <span className="min-w-0 flex-1 truncate font-mono">{firstLine(plan.sql)}</span>
        {plan.status === "running" && (
          <span className="flex items-center gap-1.5">
            <Loader2 className="size-3 animate-spin" />
            <ElapsedTimer since={plan.startedAt} />
          </span>
        )}
        {plan.result && (
          <span className="tabular-nums">{formatDuration(plan.result.durationMs)}</span>
        )}
        {text && (
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label="Copy plan"
            onClick={() =>
              void writeClipboard(text).then(() => toast.success("Plan copied to the clipboard"))
            }
          >
            <Copy />
          </Button>
        )}
      </div>
      {/* The backend wraps PostgreSQL / MySQL analyzes in a transaction (or savepoint) it rolls
          back; SQLite's plan never executes anything. */}
      {plan.mode === "analyze" && dialect !== "sqlite" && plan.status === "done" && (
        <p className="border-gridline text-muted-foreground shrink-0 border-b px-3 py-1 text-[11px] leading-4">
          {rollsBack
            ? "Ran inside a transaction that was rolled back — no changes were kept (non-transactional MySQL tables excepted)."
            : "This connection cannot roll a statement back: whatever the analyzed statement changed was kept."}
        </p>
      )}
      <div className="min-h-0 flex-1">
        {plan.status === "running" ? (
          <div className="text-muted-foreground flex items-center gap-2 p-3 text-xs">
            <Loader2 className="size-3.5 animate-spin" /> Explaining…
          </div>
        ) : plan.status === "cancelled" ? (
          <div className="text-warning p-3 text-xs">
            {plan.error?.message ?? "The explain was cancelled."}
          </div>
        ) : plan.error ? (
          <div className="p-3">
            <ErrorBlock
              tabId={tabId}
              submission={plan.id}
              error={plan.error}
              range={errorRange(plan.error, plan)}
            />
          </div>
        ) : text !== null ? (
          <pre className="selectable h-full overflow-auto p-3 font-mono text-[11.5px] leading-[1.55] whitespace-pre">
            {text}
          </pre>
        ) : plan.result ? (
          <StaticGrid
            columns={plan.result.columns}
            rows={plan.result.rows}
            label="Plan"
            dialect={dialect}
          />
        ) : null}
      </div>
    </div>
  );
}
