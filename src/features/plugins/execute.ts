import { coreDialect } from "@/features/drivers/catalog";
import { statementTexts } from "@/features/editor/scanner";
import { guardExecution } from "@/features/safety/guard";
import { ipc, type StatementResult, type Value } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import type { ExecResult } from "./types";

/** The user (Safe Mode, a read-only connection) did not let the SQL go out. */
export class PluginCancelled extends Error {
  constructor(message = "The query was not sent") {
    super(message);
    this.name = "PluginCancelled";
  }
}

export interface ExecuteOptions {
  /** Called with the id of the run before it starts (to cancel it on a time-out). */
  onStart?: (queryId: string) => void;
  /** Wraps the Safe Mode prompt (the plugin's time-out pauses while the user answers). */
  userWait?: <T>(p: Promise<T>) => Promise<T>;
  /** True once the command was stopped: a query the user confirmed late is not sent. */
  isStopped?: () => boolean;
}

/**
 * A plugin's `context.execute(sql)`: the SQL goes through the connection's Safe Mode first
 * (`guardExecution`, which also refuses writes on a read-only connection), then runs like the SQL
 * editor's (`run_sql`: logged, recorded in the history, the query time-out applies). Resolves to the
 * rows of the last statement that returned rows; rejects with the first statement's error.
 */
export async function executeSql(
  connectionId: string | null,
  sql: string,
  options: ExecuteOptions = {},
): Promise<ExecResult> {
  const conn = connectionId ? useConnectionsStore.getState().byId[connectionId] : undefined;
  if (!connectionId || !conn) throw new Error("Not connected: open a connection first");
  const statements = statementTexts(sql, coreDialect(conn.info.driver)).filter(
    (s) => s.trim() !== "",
  );
  if (statements.length === 0) throw new Error("Nothing to execute");
  const wait = options.userWait ?? (<T>(p: Promise<T>) => p);
  if (!(await wait(guardExecution(connectionId, { kind: "query", statements }))))
    throw new PluginCancelled();
  // The command may have been stopped while the Safe Mode prompt was open (its dialogs are not
  // dismissed with the run): confirming it late must not send the SQL.
  if (options.isStopped?.()) throw new PluginCancelled("The command was stopped");
  const queryId = `plugin-${crypto.randomUUID()}`;
  options.onStart?.(queryId);
  /** First result set of each statement. */
  const sets = new Map<number, { columns: string[]; rows: Value[][] }>();
  const results: StatementResult[] = await ipc.query.run(
    connectionId,
    queryId,
    sql,
    { stopOnError: true },
    (e) => {
      if (e.event === "columns" && e.data.set === 0)
        sets.set(e.data.index, { columns: e.data.columns.map((c) => c.name), rows: [] });
      else if (e.event === "rows" && e.data.set === 0) {
        const set = sets.get(e.data.index);
        if (set) for (const row of e.data.rows) set.rows.push(row);
      }
    },
  );
  const failed = results.find((r) => r.error);
  if (failed?.error) throw new Error(failed.error.message);
  const withRows = [...results].reverse().find((r) => sets.has(r.index));
  const set = (withRows && sets.get(withRows.index)) ?? { columns: [], rows: [] };
  const last = results[results.length - 1];
  return {
    columns: set.columns,
    rows: set.rows,
    rowsAffected: last?.affected ?? null,
    statements: results.length,
  };
}
