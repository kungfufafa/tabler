import { callsListedFunction } from "@/features/drivers/portable/classify";
import {
  isIdentStart,
  isPunct,
  PLAIN,
  scan,
  text,
  upper,
} from "@/features/drivers/portable/scanner";
import type { DialectLex } from "@/lib/ipc/bindings/DialectLex";
import type { StatementClass } from "@/features/safety/classify";
import TSQL_LEX from "@/lib/ipc/bindings/lex/tsql.json";

/**
 * T-SQL's own Safe Mode checks beside the portable classifier (`DialectUi.classify`): what the
 * backend's session refuses on a read-only connection beyond the shared rules, so a writable
 * session confirms it too. The mock's session refuses with the same functions.
 */

const LEX = TSQL_LEX as DialectLex;

/** `tsql.rs` `SPEC.knobs.side_effect_functions`. */
export const SIDE_EFFECT_FUNCTIONS = [
  "OPENQUERY",
  "OPENROWSET",
  "OPENDATASOURCE",
  "XP_*",
  "SP_*",
  "NEXT VALUE FOR",
] as const;

const HIDDEN_WORDS = ["CALL", "DUMP", "LOAD"];

/**
 * `tsql::hides_statement`, token for token: a text another reader may scan otherwise, an ODBC
 * `{? = call p}` escape, or the words CALL, DUMP and LOAD in code, also hidden in a number (any
 * tail of a word that starts with a digit, or follows one and a dot, from a letter on).
 */
export function hidesStatement(sql: string): boolean {
  const s = scan(LEX.lex, PLAIN, sql);
  if (s.ambiguous) return true;
  const code = s.tokens.filter((t) => t.kind !== "space" && t.kind !== "comment");
  const digits = (i: number) => {
    const t = code[i];
    return t !== undefined && t.kind === "word" && /^[0-9]/.test(text(s, t));
  };
  return code.some((t, i) => {
    if (isPunct(s, t, "{")) {
      const next = code[i + 1];
      return next !== undefined && isPunct(s, next, "?");
    }
    if (t.kind !== "word") return false;
    const words = [upper(s, t), text(s, t).toUpperCase()];
    const dot = code[i - 1];
    const number =
      digits(i) || (i >= 2 && dot !== undefined && isPunct(s, dot, ".") && digits(i - 2));
    if (number) words.push(...words.flatMap(hiddenInNumber));
    return words.some((w) => HIDDEN_WORDS.includes(w));
  });
}

/** `tsql::hidden_in_number`: every tail of a word from a letter or `_` after its first character. */
function hiddenInNumber(w: string): string[] {
  const chars = [...w];
  return chars.flatMap((c, k) => (k > 0 && isIdentStart(c) ? [chars.slice(k).join("")] : []));
}

/** Whether `sql` calls a side-effect function (`NEXT VALUE FOR` takes a sequence's value for good). */
export const callsSideEffect = (sql: string): boolean =>
  callsListedFunction(LEX.lex, SIDE_EFFECT_FUNCTIONS, sql);

/**
 * `tsqlUi.classify`: a write when the statement hides one (`hidesStatement`) or calls a
 * side-effect function, as a read-only session refuses both; otherwise the portable classifier
 * decides alone.
 */
export const classifyTsql = (statement: string): StatementClass =>
  hidesStatement(statement) || callsSideEffect(statement) ? "write" : "read";
