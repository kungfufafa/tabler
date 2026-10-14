import type {
  ErrorPayload,
  QueryEvent,
  ResultColumn,
  StatementResult,
  StatementSpan,
  Value,
} from "@/lib/ipc";

/**
 * Pure model of one `run_sql` call: statements, result sets and the overall status. Streamed
 * events are folded in with `applyEvent`; row chunks go to a `ResultSink` (the non-reactive
 * buffers in `stores/resultData`) and leave the model untouched, so the store only changes on
 * structural events.
 */

export type StatementStatus = "running" | "ok" | "error" | "cancelled";

/** A server notice (PostgreSQL `RAISE NOTICE` / `WARNING`…), as `QueryEvent::Notice` sends it. */
export type RunNotice = Omit<Extract<QueryEvent, { event: "notice" }>["data"], "index">;

export interface StatementRun {
  index: number;
  sql: string;
  /** Character offsets into the submitted text (see `offsets.ts`). */
  span: StatementSpan;
  status: StatementStatus;
  result: StatementResult | null;
  error: ErrorPayload | null;
  /** Notices the statement raised, in arrival order (streamed while it runs). */
  notices?: RunNotice[];
}

export interface ResultSetInfo {
  /** Buffer id in `resultData`: `${runId}:${statement}:${set}`. */
  id: string;
  statementIndex: number;
  set: number;
  columns: ResultColumn[];
  /** The statement finished (no more rows will arrive for this set). */
  done: boolean;
  /** The row cap was hit for this set. */
  truncated: boolean;
  durationMs: number | null;
}

export type RunStatus = "running" | "done" | "cancelled" | "error";

export interface RunState {
  /** The queryId passed to `run_sql`. */
  id: string;
  /** Exactly the text that was submitted. */
  sql: string;
  /** UTF-16 offset of the submitted text in the editor document (selection / statement start). */
  baseOffset: number;
  startedAt: number;
  finishedAt: number | null;
  totalMs: number | null;
  status: RunStatus;
  statements: StatementRun[];
  results: ResultSetInfo[];
  /** The command itself failed (not connected, cancelled before the first statement…). */
  error: ErrorPayload | null;
  /**
   * Stop was pressed and the run has not ended yet. Only drives the "Stopping…" label: whether the
   * run was cancelled is decided by what the backend reports (a CANCELLED statement or error), since
   * Stop can arrive after the last statement already finished.
   */
  cancelling: boolean;
  /**
   * Notices raised outside the run's statements (bootstrap SQL, the session's activity between
   * runs), handed over when the run started. They go to the console log.
   */
  outsideNotices?: RunNotice[];
  /**
   * Result sets past the first `MAX_RESULT_TABS` (TablePlus: very large result counts fall back
   * to plain text). Their rows are buffered like the others; Messages lists them as text.
   */
  overflow?: ResultSetInfo[];
}

/** Result sets that get a result tab / grid; the rest are listed as text in Messages. */
export const MAX_RESULT_TABS = 100;

/** Adds a result set: a grid while there is room, else to the plain-text overflow. */
function withSet(run: RunState, set: ResultSetInfo): RunState {
  if (run.results.length < MAX_RESULT_TABS) return { ...run, results: [...run.results, set] };
  return { ...run, overflow: [...(run.overflow ?? []), set] };
}

/** Maps the result sets matching `pick` (grids and overflow alike). */
function mapSets(
  run: RunState,
  pick: (r: ResultSetInfo) => boolean,
  f: (r: ResultSetInfo) => ResultSetInfo,
): Pick<RunState, "results" | "overflow"> {
  const map = (list: ResultSetInfo[]) => list.map((r) => (pick(r) ? f(r) : r));
  return run.overflow
    ? { results: map(run.results), overflow: map(run.overflow) }
    : { results: map(run.results) };
}

/** Every result set of the run: grids first, then the overflow. */
export function allResultSets(run: RunState): ResultSetInfo[] {
  return run.overflow ? [...run.results, ...run.overflow] : run.results;
}

export interface ResultSink {
  setColumns: (id: string, columns: ResultColumn[]) => void;
  append: (id: string, rows: Value[][]) => void;
}

export function resultId(runId: string, index: number, set: number): string {
  return `${runId}:${index}:${set}`;
}

export function newRun(id: string, sql: string, baseOffset: number, now = Date.now()): RunState {
  return {
    id,
    sql,
    baseOffset,
    startedAt: now,
    finishedAt: null,
    totalMs: null,
    status: "running",
    statements: [],
    results: [],
    error: null,
    cancelling: false,
  };
}

/** Fills the optional fields of an error (IPC errors may omit them). */
export function normalizeError(e: {
  code: string;
  message: string;
  detail?: string | null;
  hint?: string | null;
  sqlstate?: string | null;
  vendorCode?: number | null;
  position?: ErrorPayload["position"] | null;
  statementIndex?: number | null;
  data?: unknown;
}): ErrorPayload {
  return {
    code: e.code,
    message: e.message,
    detail: e.detail ?? null,
    hint: e.hint ?? null,
    sqlstate: e.sqlstate ?? null,
    vendorCode: e.vendorCode ?? null,
    position: e.position ?? null,
    statementIndex: e.statementIndex ?? null,
    data: e.data ?? null,
  };
}

export const isCancelled = (e: { code: string } | null | undefined): boolean =>
  e?.code === "CANCELLED";

/**
 * Replaces (or adds) one statement; every other entry keeps its identity so memoized rows can skip
 * re-rendering. Events arrive in index order, so look from the end and append without sorting.
 */
function upsertStatement(
  list: StatementRun[],
  index: number,
  patch: (prev: StatementRun | undefined) => StatementRun,
): StatementRun[] {
  let i = list.length - 1;
  while (i >= 0 && list[i]!.index !== index) i--;
  if (i < 0) {
    const next = [...list, patch(undefined)];
    const last = list[list.length - 1];
    return last && last.index > index ? next.sort((a, b) => a.index - b.index) : next;
  }
  const out = list.slice();
  out[i] = patch(list[i]);
  return out;
}

/** The notices of a statement being replaced, for its replacement. */
const keptNotices = (prev: StatementRun | undefined): Pick<StatementRun, "notices"> =>
  prev?.notices ? { notices: prev.notices } : {};

/** Appends a notice to statement `index` (a placeholder when it has not started yet). */
function addNotice(run: RunState, index: number, notice: RunNotice): RunState {
  return {
    ...run,
    statements: upsertStatement(run.statements, index, (prev) => ({
      index,
      sql: prev?.sql ?? "",
      span: prev?.span ?? { start: 0, end: 0, line: 1 },
      status: prev?.status ?? "running",
      result: prev?.result ?? null,
      error: prev?.error ?? null,
      notices: [...(prev?.notices ?? []), notice],
    })),
  };
}

function newSet(id: string, index: number, set: number, columns: ResultColumn[]): ResultSetInfo {
  return {
    id,
    statementIndex: index,
    set,
    columns,
    done: false,
    truncated: false,
    durationMs: null,
  };
}

function finishStatement(run: RunState, result: StatementResult, sink: ResultSink): RunState {
  const { index } = result;
  const failed = result.error !== null;
  const statements = upsertStatement(run.statements, index, (prev) => ({
    index,
    sql: prev?.sql ?? result.sql,
    span: prev?.span ?? result.span,
    status: failed ? (isCancelled(result.error) ? "cancelled" : "error") : "ok",
    result,
    error: result.error,
    ...keptNotices(prev),
  }));
  let next: RunState = { ...run, statements };
  let own = allResultSets(next).filter((r) => r.statementIndex === index);
  if (own.length === 0 && !failed && (result.kind === "rows" || result.kind === "explain")) {
    // A statement that produced no row events (e.g. zero rows) still gets an (empty) result tab.
    const id = resultId(run.id, index, 0);
    sink.setColumns(id, result.columns);
    const set = newSet(id, index, 0, result.columns);
    next = withSet(next, set);
    own = [set];
  }
  // Statements without result sets (DML, DDL) leave the lists untouched.
  if (own.length === 0) return next;
  const last = Math.max(...own.map((r) => r.set));
  return {
    ...next,
    ...mapSets(
      next,
      (r) => r.statementIndex === index,
      (r) => ({
        ...r,
        done: true,
        durationMs: result.durationMs,
        truncated: r.truncated || (result.truncated && r.set === last),
      }),
    ),
  };
}

/** Folds one streamed event into the run. Returns `run` itself when nothing structural changed. */
export function applyEvent(
  run: RunState,
  e: QueryEvent,
  sink: ResultSink,
  now = Date.now(),
): RunState {
  switch (e.event) {
    case "started": {
      const { index, sql, span } = e.data;
      return {
        ...run,
        statements: upsertStatement(run.statements, index, (prev) => ({
          index,
          sql,
          span,
          status: "running",
          result: null,
          error: null,
          ...keptNotices(prev),
        })),
      };
    }
    case "columns": {
      const { index, set, columns } = e.data;
      const id = resultId(run.id, index, set);
      sink.setColumns(id, columns);
      const existing = allResultSets(run).some((r) => r.id === id);
      return existing
        ? {
            ...run,
            ...mapSets(
              run,
              (r) => r.id === id,
              (r) => ({ ...r, columns }),
            ),
          }
        : withSet(run, newSet(id, index, set, columns));
    }
    case "rows": {
      const { index, set, rows } = e.data;
      const id = resultId(run.id, index, set);
      sink.append(id, rows);
      if (allResultSets(run).some((r) => r.id === id)) return run;
      return withSet(run, newSet(id, index, set, []));
    }
    case "finished":
      return finishStatement(run, e.data.result, sink);
    case "failed": {
      const { index, error } = e.data;
      return {
        ...run,
        statements: upsertStatement(run.statements, index, (prev) => ({
          index,
          sql: prev?.sql ?? "",
          span: prev?.span ?? { start: 0, end: 0, line: 1 },
          status: isCancelled(error) ? "cancelled" : "error",
          result: prev?.result ?? null,
          error,
          ...keptNotices(prev),
        })),
        ...mapSets(
          run,
          (r) => r.statementIndex === index,
          (r) => ({ ...r, done: true }),
        ),
      };
    }
    case "notice": {
      const { index, ...notice } = e.data;
      if (index === null)
        return { ...run, outsideNotices: [...(run.outsideNotices ?? []), notice] };
      return addNotice(run, index, notice);
    }
    case "done": {
      // A statement still "running" here never reported back; after Stop, it was the one stopped.
      const unfinished: StatementStatus = run.cancelling ? "cancelled" : "ok";
      const statements = run.statements.map((s) =>
        s.status === "running" ? { ...s, status: unfinished } : s,
      );
      const cancelled = statements.some((s) => s.status === "cancelled");
      return {
        ...run,
        statements,
        ...mapSets(
          run,
          (r) => !r.done,
          (r) => ({ ...r, done: true }),
        ),
        status: cancelled ? "cancelled" : "done",
        totalMs: e.data.totalMs,
        finishedAt: now,
        cancelling: false,
      };
    }
  }
}

/** The `run_sql` command rejected (not connected, cancelled, internal error…). */
export function failRun(run: RunState, error: ErrorPayload, now = Date.now()): RunState {
  if (run.status !== "running") return run;
  const cancelled = isCancelled(error);
  return {
    ...run,
    statements: run.statements.map((s) =>
      s.status === "running"
        ? { ...s, status: cancelled ? "cancelled" : "error", error: s.error ?? error }
        : s,
    ),
    ...mapSets(
      run,
      (r) => !r.done,
      (r) => ({ ...r, done: true }),
    ),
    status: cancelled ? "cancelled" : "error",
    // A statement already reported this error: do not show it twice.
    error: run.statements.some((s) => s.error?.message === error.message) ? null : error,
    finishedAt: now,
    totalMs: now - run.startedAt,
    cancelling: false,
  };
}

/**
 * `run_sql` resolved. Normally `done` already arrived; if channel messages lag behind the command
 * response, reconcile from the returned summaries so the run never stays "running".
 */
export function finalizeRun(
  run: RunState,
  results: readonly StatementResult[],
  sink: ResultSink,
  now = Date.now(),
): RunState {
  if (run.status !== "running") return run;
  let next = run;
  for (const r of results) {
    const st = next.statements.find((s) => s.index === r.index);
    if (!st || st.status === "running") next = finishStatement(next, r, sink);
  }
  return applyEvent(next, { event: "done", data: { totalMs: now - run.startedAt } }, sink, now);
}

export interface RunSummary {
  statements: number;
  errors: number;
  cancelled: boolean;
  totalMs: number | null;
  affected: number;
}

export function summarize(run: RunState): RunSummary {
  let errors = 0;
  let affected = 0;
  for (const s of run.statements) {
    if (s.status === "error") errors++;
    affected += s.result?.affected ?? 0;
  }
  if (run.error && !isCancelled(run.error) && errors === 0) errors = 1;
  return {
    statements: run.statements.length,
    errors,
    cancelled: run.status === "cancelled",
    totalMs: run.totalMs,
    affected,
  };
}

/** First failed statement (or the command error), for the inline message and diagnostics. */
export function firstError(
  run: RunState,
): { error: ErrorPayload; statement: StatementRun | null } | null {
  const st = run.statements.find((s) => s.status === "error" && s.error);
  if (st?.error) return { error: st.error, statement: st };
  if (run.error && !isCancelled(run.error)) return { error: run.error, statement: null };
  return null;
}
