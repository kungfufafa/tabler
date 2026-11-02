import { codeOnly, isSingleStatement } from "@/features/drivers/portable/classify";
import { classifySnowflake, LEX, quotesNextval } from "@/features/drivers/snowflake/classify";

/**
 * The session's refusals (`drivers/snowflake/session.rs`, `editor.rs`) for the mock, over the
 * renderer's copy of the backend's classifier (`classify_snowflake`, which Safe Mode reads too).
 */

export { classifySnowflake, LEX, quotesNextval };

/** `sql::writes` on the Snowflake dialect: what a read-only session refuses. */
export const writes = (sql: string): boolean => classifySnowflake(sql) === "write";

/** `sql::is_single_statement`: `;` and the flow operator `->>` make two. */
export const isSingle = (sql: string): boolean => isSingleStatement(LEX.lex, LEX.classify, sql);

/** The first `n` words of the code, upper case (`split::leading_keywords`, near enough). */
export function leadingKeywords(sql: string, n: number): string[] {
  return (codeOnly(LEX.lex, sql).match(/[A-Za-z_][A-Za-z0-9_$]*/g) ?? [])
    .slice(0, n)
    .map((w) => w.toUpperCase());
}

/** `drivers::contains_word` on upper-cased text. */
function containsWord(upper: string, word: string): boolean {
  return new RegExp(`(?<![\\p{L}\\p{N}_$])${word}(?![\\p{L}\\p{N}_$])`, "u").test(upper);
}

/**
 * `editor::sets_autocommit`: a statement not known to only read that names AUTOCOMMIT, except
 * `ALTER USER` / `ALTER ACCOUNT` (new sessions' default).
 */
export const setsAutocommit = (sql: string): boolean => setsParameter(sql, "AUTOCOMMIT");

/** `editor::sets_ignore_case`: the same for QUOTED_IDENTIFIERS_IGNORE_CASE. */
export const setsIgnoreCase = (sql: string): boolean =>
  setsParameter(sql, "QUOTED_IDENTIFIERS_IGNORE_CASE");

/** `sf::APP_PARAMETERS`: what the app's session keeps at Tabler's values. */
export const APP_PARAMETERS = [
  "QUOTED_IDENTIFIERS_IGNORE_CASE",
  "DATE_INPUT_FORMAT",
  "TIME_INPUT_FORMAT",
  "TIMESTAMP_INPUT_FORMAT",
] as const;

/**
 * `editor::sets_app_parameter`: the first of them a statement names, the same coarse way. The
 * text only: what it misses (`EXECUTE IMMEDIATE $q`, a procedure) the backend sets back after
 * the statement, which the mock, keeping no parameters, has nothing to mirror of.
 */
export const setsAppParameter = (sql: string): string | null =>
  APP_PARAMETERS.find((name) => setsParameter(sql, name)) ?? null;

function setsParameter(sql: string, name: string): boolean {
  const [first, second] = leadingKeywords(sql, 2);
  const defaults = first === "ALTER" && (second === "USER" || second === "ACCOUNT");
  return !defaults && writes(sql) && containsWord(sql.toUpperCase(), name);
}
