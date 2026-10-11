import { toIpcError, type DdlOp, type DdlPlan, type Dialect, type ErrorPayload } from "@/lib/ipc";

export interface DdlErrorInfo {
  code: string;
  message: string;
  detail: string | null;
  hint: string | null;
}

/** State of the "Review structure changes" sheet. */
export interface DdlPreviewState {
  open: boolean;
  loading: boolean;
  applying: boolean;
  /** The ops that were previewed (and that Apply sends). */
  ops: DdlOp[];
  plan: DdlPlan | null;
  error: DdlErrorInfo | null;
  /** Statement (index into `plan.statements`) that failed during apply. */
  failedIndex: number | null;
  /** After a failed apply: statements whose effects persist (not rolled back). */
  executed: string[] | null;
}

export const PREVIEW_CLOSED: DdlPreviewState = {
  open: false,
  loading: false,
  applying: false,
  ops: [],
  plan: null,
  error: null,
  failedIndex: null,
  executed: null,
};

export function errorInfo(e: unknown): DdlErrorInfo {
  const err = toIpcError(e);
  return {
    code: err.code,
    message: err.message,
    detail: err.payload.detail ?? null,
    hint: err.payload.hint ?? null,
  };
}

export function payloadInfo(p: ErrorPayload): DdlErrorInfo {
  return { code: p.code, message: p.message, detail: p.detail, hint: p.hint };
}

/**
 * Marks which statements of a plan ran (and persisted) given `DdlResult.executed`: each executed
 * entry claims the first not-yet-claimed identical statement, in order.
 */
export function executedFlags(
  statements: readonly string[],
  executed: readonly string[],
): boolean[] {
  const flags = statements.map(() => false);
  let from = 0;
  for (const sql of executed) {
    let i = statements.indexOf(sql, from);
    if (i < 0) i = statements.findIndex((s, j) => !flags[j] && s === sql);
    if (i < 0) continue;
    flags[i] = true;
    from = i + 1;
  }
  return flags;
}

/** False for session / transaction control statements (PRAGMA, SET, BEGIN, COMMIT…). */
export function changesSchema(sql: string): boolean {
  return !/^\s*(pragma|set|begin|commit|rollback|start\s+transaction)\b/i.test(sql);
}

/** Why a plan is not wrapped in a single transaction, per dialect. */
export function nonTransactionalNote(dialect: Dialect): string {
  if (dialect === "mysql")
    return "MySQL commits every DDL statement on its own: if one fails, the statements before it stay applied.";
  if (dialect === "sqlite")
    return "The table rebuild runs in its own BEGIN … COMMIT block, with foreign-key enforcement switched off around it.";
  return "Some statements cannot run inside a transaction block, so the plan runs statement by statement.";
}
