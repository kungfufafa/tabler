import { useVirtualizer } from "@tanstack/react-virtual";
import { Ban, CircleCheck, CircleX, Info, Loader2, TriangleAlert } from "lucide-react";
import { memo, useMemo, useState } from "react";
import { formatDuration } from "@/lib/format/time";
import { cn } from "@/lib/utils";
import { locateSubmitted } from "./submissions";
import { focusEditor } from "./editorStates";
import { ErrorBlock } from "./ErrorBlock";
import { errorRange, spanRange } from "./errorPos";
import { firstLine, statementOutcome } from "./labels";
import { isWarning, noticeDetails, noticeText } from "./notices";
import { formatPlainResult } from "./plainResult";
import {
  MAX_RESULT_TABS,
  type ResultSetInfo,
  type RunNotice,
  type RunState,
  type StatementRun,
  type StatementStatus,
} from "./runModel";
import { resultData } from "@/stores/resultData";

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

/** A server notice under its statement: `NOTICE: …`, then its DETAIL / HINT lines. */
function NoticeLine({ notice }: { notice: RunNotice }) {
  const warning = isWarning(notice);
  const Icon = warning ? TriangleAlert : Info;
  return (
    <div
      className={cn(
        "mt-0.5 ml-[46px] flex items-start gap-1.5 text-[11px]",
        warning ? "text-warning" : "text-muted-foreground",
      )}
      title={notice.context ? `CONTEXT: ${notice.context}` : undefined}
      data-slot="notice"
    >
      <Icon className="mt-0.5 size-3 shrink-0" />
      <span className="selectable break-words whitespace-pre-wrap">
        {[noticeText(notice), ...noticeDetails(notice)].join("\n")}
      </span>
    </div>
  );
}

/**
 * A result set past the first `MAX_RESULT_TABS`, as text under its statement (TablePlus: very
 * large result counts fall back to plain text).
 */
function PlainResult({ set }: { set: ResultSetInfo }) {
  const buffer = resultData.get(set.id);
  const columns = (buffer && buffer.columns.length > 0 ? buffer.columns : set.columns).map(
    (c) => c.name,
  );
  const text = set.done
    ? formatPlainResult(columns, buffer?.rows ?? [], { truncated: set.truncated })
    : "…";
  return (
    <pre
      className="selectable bg-muted/40 text-foreground/90 mt-1 ml-[46px] max-h-64 overflow-auto rounded px-2 py-1 font-mono text-[11px] leading-4"
      data-slot="plain-result"
    >
      {text}
    </pre>
  );
}

interface RowProps {
  tabId: string;
  /** The run's queryId (its document anchor). */
  runId: string;
  /** The submitted text and its offset in the document. */
  sql: string;
  baseOffset: number;
  st: StatementRun;
  /** The statement's result sets past the result tabs, shown as text. */
  overflow?: readonly ResultSetInfo[];
}

/**
 * One statement of the log. Memoized on primitive props plus the statement object, which the run
 * model only replaces when that statement changes: a new event re-renders one row, not all of them.
 */
const StatementRow = memo(function StatementRow({
  tabId,
  runId,
  sql,
  baseOffset,
  st,
  overflow,
}: RowProps) {
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
      {st.notices?.map((n, i) => (
        <NoticeLine key={`live-${i}`} notice={n} />
      ))}
      {notices.map((n, i) => (
        <div
          key={i}
          className="text-muted-foreground mt-0.5 ml-[46px] flex items-start gap-1.5 text-[11px]"
        >
          <Info className="mt-0.5 size-3 shrink-0" />
          <span className="selectable break-words whitespace-pre-wrap">{n}</span>
        </div>
      ))}
      {overflow?.map((set) => (
        <PlainResult key={set.id} set={set} />
      ))}
    </>
  );
});

type OverflowByStatement = ReadonlyMap<number, readonly ResultSetInfo[]>;

/** The overflow result sets of each statement (the same arrays while the overflow is unchanged). */
function useOverflow(run: RunState): OverflowByStatement {
  const overflow = run.overflow;
  return useMemo(() => {
    const map = new Map<number, ResultSetInfo[]>();
    for (const set of overflow ?? []) {
      const list = map.get(set.statementIndex);
      if (list) list.push(set);
      else map.set(set.statementIndex, [set]);
    }
    return map;
  }, [overflow]);
}

function rowProps(
  tabId: string,
  run: RunState,
  st: StatementRun,
  overflow: OverflowByStatement,
): RowProps {
  return {
    tabId,
    runId: run.id,
    sql: run.sql,
    baseOffset: run.baseOffset,
    st,
    overflow: overflow.get(st.index),
  };
}

function PlainList({
  tabId,
  run,
  overflow,
}: {
  tabId: string;
  run: RunState;
  overflow: OverflowByStatement;
}) {
  return (
    <ol className="py-1">
      {run.statements.map((st) => (
        <li key={st.index} className={cn(ROW_CLASS, "last:border-b-0")}>
          <StatementRow {...rowProps(tabId, run, st, overflow)} />
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
  overflow,
}: {
  tabId: string;
  run: RunState;
  scroller: HTMLDivElement | null;
  overflow: OverflowByStatement;
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
            <StatementRow {...rowProps(tabId, run, st, overflow)} />
          </li>
        );
      })}
    </ol>
  );
}

/**
 * Per-statement log of a run: outcome, rows / affected, duration, notices (the server's, streamed
 * while it runs, then the run's own) and errors.
 */
export function MessagesView({ tabId, run }: { tabId: string; run: RunState }) {
  // State, not a ref: the virtualizer needs the element on the render after it is attached.
  const [scroller, setScroller] = useState<HTMLDivElement | null>(null);
  const waiting = run.statements.length === 0 && run.status === "running";
  const overflow = useOverflow(run);
  const extra = run.overflow?.length ?? 0;
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
      {extra > 0 && (
        <div
          className="border-gridline text-muted-foreground flex shrink-0 items-center gap-1.5 border-b px-3 py-1.5 text-[11px]"
          data-slot="overflow-note"
        >
          <Info className="size-3 shrink-0" />
          {`${MAX_RESULT_TABS} result sets have tabs; the other ${extra.toLocaleString()} are listed here as text.`}
        </div>
      )}
      <div ref={setScroller} className="min-h-0 flex-1 overflow-auto">
        {run.statements.length >= VIRTUALIZE_FROM ? (
          <VirtualList tabId={tabId} run={run} scroller={scroller} overflow={overflow} />
        ) : (
          <PlainList tabId={tabId} run={run} overflow={overflow} />
        )}
      </div>
    </div>
  );
}
