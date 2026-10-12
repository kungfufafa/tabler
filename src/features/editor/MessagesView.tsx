import { useVirtualizer } from "@tanstack/react-virtual";
import { Ban, CircleCheck, CircleX, Info, Loader2 } from "lucide-react";
import { memo, useState } from "react";
import { formatDuration } from "@/lib/format/time";
import { cn } from "@/lib/utils";
import { locateSubmitted } from "./submissions";
import { focusEditor } from "./editorStates";
import { ErrorBlock } from "./ErrorBlock";
import { errorRange, spanRange } from "./errorPos";
import { firstLine, statementOutcome } from "./labels";
import type { RunState, StatementRun, StatementStatus } from "./runModel";

/** From this many statements on, only the visible rows are rendered. */
export const VIRTUALIZE_FROM = 200;
const ROW_ESTIMATE = 29;
const ROW_CLASS = "border-gridline border-b px-3 py-1.5";

function StatusIcon({ status }: { status: StatementStatus }) {
  switch (status) {
    case "running":
      return <Loader2 className="text-brand size-3.5 shrink-0 animate-spin" aria-label="Running" />;
    case "ok":
      return <CircleCheck className="text-success size-3.5 shrink-0" aria-label="Succeeded" />;
    case "error":
      return <CircleX className="text-error size-3.5 shrink-0" aria-label="Failed" />;
    case "cancelled":
      return <Ban className="text-warning size-3.5 shrink-0" aria-label="Cancelled" />;
  }
}

interface RowProps {
  tabId: string;
  /** The run's queryId (its document anchor). */
  runId: string;
  /** The submitted text and its offset in the document. */
  sql: string;
  baseOffset: number;
  st: StatementRun;
}

/**
 * One statement of the log. Memoized on primitive props plus the statement object, which the run
 * model only replaces when that statement changes: a new event re-renders one row, not all of them.
 */
const StatementRow = memo(function StatementRow({ tabId, runId, sql, baseOffset, st }: RowProps) {
  const sub = { sql, baseOffset };
  const outcome =
    st.status === "running"
      ? "Running…"
      : st.status === "cancelled"
        ? "Cancelled"
        : st.status === "error"
          ? "Failed"
          : st.result
            ? statementOutcome(st.result)
            : "OK";
  const notices = st.result?.notices ?? [];
  return (
    <>
      <button
        type="button"
        className="hover:text-foreground flex w-full items-center gap-2 text-left"
        title="Select this statement in the editor"
        onClick={() => {
          const at = locateSubmitted(tabId, runId, spanRange(st.span, sub));
          if (at) focusEditor(at.key, at.range);
        }}
      >
        <StatusIcon status={st.status} />
        <span className="text-muted-foreground w-5 shrink-0 text-right text-[10px] tabular-nums">
          {st.index + 1}
        </span>
        <span className="min-w-0 flex-1 truncate font-mono text-[11px]">{firstLine(st.sql)}</span>
        <span
          className={cn(
            "shrink-0 tabular-nums",
            st.status === "error" ? "text-error" : "text-muted-foreground",
            st.status === "cancelled" && "text-warning",
          )}
        >
          {outcome}
        </span>
        <span className="text-muted-foreground w-16 shrink-0 text-right tabular-nums">
          {st.result ? formatDuration(st.result.durationMs) : ""}
        </span>
      </button>
      {st.status === "error" && st.error && (
        <ErrorBlock
          tabId={tabId}
          submission={runId}
          error={st.error}
          range={errorRange(st.error, sub, st.span)}
          className="mt-1 ml-[46px]"
        />
      )}
      {st.status === "cancelled" && st.error && (
        <div className="text-warning mt-0.5 ml-[46px] text-[11px]">{st.error.message}</div>
      )}
      {st.result?.lastInsertId && (
        <div className="text-muted-foreground mt-0.5 ml-[46px] text-[11px]">
          Last insert id: <span className="selectable font-mono">{st.result.lastInsertId}</span>
        </div>
      )}
      {notices.map((n, i) => (
        <div
          key={i}
          className="text-muted-foreground mt-0.5 ml-[46px] flex items-start gap-1.5 text-[11px]"
        >
          <Info className="mt-0.5 size-3 shrink-0" />
          <span className="selectable break-words whitespace-pre-wrap">{n}</span>
        </div>
      ))}
    </>
  );
});

function rowProps(tabId: string, run: RunState, st: StatementRun): RowProps {
  return { tabId, runId: run.id, sql: run.sql, baseOffset: run.baseOffset, st };
}

function PlainList({ tabId, run }: { tabId: string; run: RunState }) {
  return (
    <ol className="py-1">
      {run.statements.map((st) => (
        <li key={st.index} className={cn(ROW_CLASS, "last:border-b-0")}>
          <StatementRow {...rowProps(tabId, run, st)} />
        </li>
      ))}
    </ol>
  );
}

/** Long scripts (dumps, migrations): only the rows in view exist in the DOM. */
function VirtualList({
  tabId,
  run,
  scroller,
}: {
  tabId: string;
  run: RunState;
  scroller: HTMLDivElement | null;
}) {
  const statements = run.statements;
  const virtualizer = useVirtualizer({
    count: statements.length,
    getScrollElement: () => scroller,
    estimateSize: () => ROW_ESTIMATE,
    getItemKey: (i) => statements[i]?.index ?? i,
    paddingStart: 4,
    paddingEnd: 4,
    overscan: 12,
  });
  return (
    <ol className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
      {virtualizer.getVirtualItems().map((item) => {
        const st = statements[item.index];
        if (!st) return null;
        return (
          <li
            key={item.key}
            ref={virtualizer.measureElement}
            data-index={item.index}
            className={cn(
              ROW_CLASS,
              "absolute top-0 left-0 w-full",
              item.index === statements.length - 1 && "border-b-0",
            )}
            style={{ transform: `translateY(${item.start}px)` }}
          >
            <StatementRow {...rowProps(tabId, run, st)} />
          </li>
        );
      })}
    </ol>
  );
}

/** Per-statement log of a run: outcome, rows / affected, duration, notices and errors. */
export function MessagesView({ tabId, run }: { tabId: string; run: RunState }) {
  // State, not a ref: the virtualizer needs the element on the render after it is attached.
  const [scroller, setScroller] = useState<HTMLDivElement | null>(null);
  const waiting = run.statements.length === 0 && run.status === "running";
  return (
    <div className="flex h-full min-h-0 flex-col text-xs" role="log" aria-label="Messages">
      {run.error && (
        <div className="border-gridline shrink-0 border-b px-3 py-1.5">
          {run.error.code === "CANCELLED" ? (
            <div className="text-warning text-[11px]">{run.error.message}</div>
          ) : (
            <ErrorBlock
              tabId={tabId}
              submission={run.id}
              error={run.error}
              range={errorRange(run.error, run)}
            />
          )}
        </div>
      )}
      {waiting && (
        <div className="text-muted-foreground flex shrink-0 items-center gap-2 px-3 py-2">
          <Loader2 className="size-3.5 animate-spin" /> Waiting for the server…
        </div>
      )}
      <div ref={setScroller} className="min-h-0 flex-1 overflow-auto">
        {run.statements.length >= VIRTUALIZE_FROM ? (
          <VirtualList tabId={tabId} run={run} scroller={scroller} />
        ) : (
          <PlainList tabId={tabId} run={run} />
        )}
      </div>
    </div>
  );
}
