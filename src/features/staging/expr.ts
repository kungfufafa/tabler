import type { ServerExpr, StagedChange, Value } from "@/lib/ipc";

/**
 * A staged cell can hold a server-side SQL expression instead of a value (TablePlus: Set Value ▸
 * NOW(), typing NOW() in a date cell). Staged cells are `Value`s everywhere (grid, sidebar, undo),
 * so the expression rides along as an `other` value of this marker type: no database names a type
 * with a NUL character, so a fetched value never looks like one. It shows as its keyword
 * ("NOW()"), and `withServerExprs` turns it into the backend's whitelisted `exprs` when the
 * change is sent; the server never receives SQL text from it.
 */
export const EXPR_TYPE = "\u0000expr";

/** The expressions without a precision: the keyword as it reads in a cell and in the SQL. */
export type PlainExpr = Exclude<ServerExpr, `${string}(${number})`>;

/** How each expression without a precision reads in a cell and in the SQL. */
export const EXPR_TEXT: Readonly<Record<PlainExpr, string>> = {
  now: "NOW()",
  currentTimestamp: "CURRENT_TIMESTAMP",
  currentDate: "CURRENT_DATE",
  currentTime: "CURRENT_TIME",
  localTimestamp: "LOCALTIMESTAMP",
  default: "DEFAULT",
};

/** The clocks that take a fractional-seconds precision, by the keyword that names them. */
const CLOCKS = {
  now: "now",
  current_timestamp: "currentTimestamp",
  current_time: "currentTime",
  localtimestamp: "localTimestamp",
} as const;
type ClockName = (typeof CLOCKS)[keyof typeof CLOCKS];

/** The largest fractional-seconds precision (MySQL, MariaDB and PostgreSQL stop at 6). */
export const MAX_FSP = 6;

/** `NOW(6)`, `CURRENT_TIMESTAMP(3)`: the keyword, then the digits as typed. */
const FSP_TEXT = /^(now|currentTimestamp|currentTime|localTimestamp)\((\d)\)$/;
const KEYWORD_OF: Record<ClockName, string> = {
  now: "NOW",
  currentTimestamp: "CURRENT_TIMESTAMP",
  currentTime: "CURRENT_TIME",
  localTimestamp: "LOCALTIMESTAMP",
};

/** A clock with `digits` fractional-seconds digits (0-6), or null for any other precision. */
export function clockWithPrecision(clock: ClockName, digits: number): ServerExpr | null {
  return Number.isInteger(digits) && digits >= 0 && digits <= MAX_FSP
    ? (`${clock}(${digits})` as ServerExpr)
    : null;
}

/** How an expression reads in a cell and in the SQL: `NOW()`, `NOW(6)`, `CURRENT_TIMESTAMP(3)`. */
export function exprText(expr: ServerExpr): string {
  const m = FSP_TEXT.exec(expr);
  if (m) return `${KEYWORD_OF[m[1] as ClockName]}(${m[2]})`;
  return EXPR_TEXT[expr as PlainExpr];
}

/** The staged value of an expression. */
export function exprValue(expr: ServerExpr): Value {
  return { t: "other", v: { ty: EXPR_TYPE, text: exprText(expr) } };
}

/** The expression a staged value holds, or null for a plain value. */
export function exprOf(v: Value | undefined): ServerExpr | null {
  if (v?.t !== "other" || v.v.ty !== EXPR_TYPE) return null;
  const text = v.v.text;
  const expr = text === EXPR_TEXT.default ? "default" : parseServerExpr(text);
  // Only the exact text exprValue writes: a value of this type never carries anything else.
  return expr !== null && exprText(expr) === text ? expr : null;
}

/**
 * Typed text that names a date / time expression: `NOW()`, `CURRENT_TIMESTAMP`, `CURRENT_DATE`,
 * `CURRENT_TIME`, `LOCALTIMESTAMP`, in any case and spacing; the clocks also with a
 * fractional-seconds precision (`NOW(6)`, `CURRENT_TIMESTAMP(3)`), which is kept: MySQL's bare
 * `CURRENT_TIMESTAMP` has 0 digits whatever the column's precision. A precision above 6 is no
 * expression (null: plain text, which the server rejects as a date). Only these keywords: never
 * arbitrary SQL.
 */
export function parseServerExpr(text: string): ServerExpr | null {
  const t = text.trim().toLowerCase().replace(/\s+/g, "");
  const keyword = /^(now|current_timestamp|current_time|localtimestamp)(?:\((\d*)\))?$/.exec(t);
  if (keyword) {
    const clock = CLOCKS[keyword[1] as keyof typeof CLOCKS];
    const digits = keyword[2];
    // A bare keyword (and `()`, which MySQL reads the same) has no precision; NOW needs the
    // parentheses.
    if (digits === undefined || digits === "") {
      if (clock === "now") return digits === undefined ? null : "now";
      return clock;
    }
    return clockWithPrecision(clock, Number(digits));
  }
  return t === "current_date" ? "currentDate" : null;
}

function split(cells: Record<string, Value>): {
  values: Record<string, Value>;
  exprs: Record<string, ServerExpr> | undefined;
} {
  let exprs: Record<string, ServerExpr> | undefined;
  const values: Record<string, Value> = {};
  for (const [name, v] of Object.entries(cells)) {
    const e = exprOf(v);
    if (e === null) values[name] = v;
    else (exprs ??= {})[name] = e;
  }
  return { values, exprs };
}

/** `change` with its expression cells moved from the values to `exprs` (the backend payload). */
export function withServerExprs(change: StagedChange): StagedChange {
  if (change.op === "delete") return change;
  if (change.op === "update") {
    const { values, exprs } = split(change.changes);
    return exprs ? { ...change, changes: values, exprs } : change;
  }
  const { values, exprs } = split(change.values);
  return exprs ? { ...change, values, exprs } : change;
}
