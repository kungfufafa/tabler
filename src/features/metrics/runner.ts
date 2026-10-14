import { splitStatements } from "@/features/editor/scanner";
import { allReads } from "@/features/safety/classify";
import { guardExecution } from "@/features/safety/guard";
import { ipc, type Dialect, type MetricsResult, type Value } from "@/lib/ipc";
import { bindVariables, neutralSql, splitTemplate } from "./bind";

/** Rows a board object keeps (the backend caps them too). */
export const MAX_ROWS = 5_000;

export const WRITE_REFUSED =
  'This query changes data. Turn on "Allow data changes" in the object\'s settings to run it.';

/** Shown in place of a write that a refresh (the board opening, a timer, an event) tried to run. */
export const AUTOMATIC_WRITE_REFUSED =
  "This query changes data, so it only runs when you click Refresh or a button wired to it.";

/** Why a board query did not run (shown inside the object). */
export class BoardQueryError extends Error {
  constructor(
    message: string,
    readonly reason: "refused" | "declined" | "variables" | "statements",
  ) {
    super(message);
    this.name = "BoardQueryError";
  }
}

export interface BoardQuery {
  connectionId: string;
  dialect: Dialect;
  sql: string;
  /** The board's variables. */
  values: Readonly<Record<string, Value>>;
  /** The object's "Allow data changes". */
  allowWrites: boolean;
  /** Cancels through `ipc.query.cancel(connectionId, queryId)`. */
  queryId: string;
  /** Not logged in the console (automatic refreshes). */
  quiet?: boolean;
  /**
   * The run was not asked for by the user: the board opened, an interval timer fired or an input
   * event arrived. A statement that writes never runs then.
   */
  automatic?: boolean;
  /** The object's "Query timeout" in seconds (the backend keeps it between 5 and 600). */
  timeoutSecs?: number;
}

/**
 * Runs one board object's query. Boards are read-only by default: a statement that writes is
 * refused unless the object allows data changes, and then the connection's safe mode applies
 * (`guardExecution`) like any query from the editor. Reads run without a prompt (a board refreshes
 * on its own). One statement per object; `{{variables}}` are bound, never pasted in.
 *
 * A statement that writes only runs from a user action (Refresh, a button): an automatic run of
 * it (`automatic`: the board opened, a timer, an event) is refused before the safe-mode guard and
 * before anything is sent. Safe Mode's default is Silent, so without this a board file that
 * switches "Allow data changes" on would run its DELETE on open and again on every interval.
 */
export async function runBoardQuery(q: BoardQuery): Promise<MetricsResult> {
  const sql = q.sql.trim();
  if (splitStatements(sql, q.dialect).length > 1)
    throw new BoardQueryError(
      "A board object runs one statement. Remove the others.",
      "statements",
    );
  const template = splitTemplate(sql, q.dialect);
  const { query, missing } = bindVariables(template, q.values);
  if (missing.length > 0)
    throw new BoardQueryError(
      `No input or dropdown sets ${missing.map((m) => `{{${m}}}`).join(", ")}.`,
      "variables",
    );
  const writes = !allReads([neutralSql(template)], q.dialect);
  if (writes && !q.allowWrites) throw new BoardQueryError(WRITE_REFUSED, "refused");
  if (writes && q.automatic) throw new BoardQueryError(AUTOMATIC_WRITE_REFUSED, "refused");
  if (writes && !(await guardExecution(q.connectionId, { kind: "query", statements: [sql] })))
    throw new BoardQueryError("Not sent: the query was cancelled.", "declined");
  return ipc.metrics.run(
    q.connectionId,
    q.queryId,
    {
      ...query,
      allowWrites: q.allowWrites,
      maxRows: MAX_ROWS,
      ...(q.timeoutSecs !== undefined ? { timeoutSecs: q.timeoutSecs } : {}),
    },
    q.quiet,
  );
}
