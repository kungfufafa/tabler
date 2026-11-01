import {
  asciiUpper,
  identName,
  isPunct,
  PLAIN,
  scan,
  text,
  upper,
} from "@/features/drivers/portable/scanner";
import type { DialectLex } from "@/lib/ipc/bindings/DialectLex";
import DUCKDB_LEX from "@/lib/ipc/bindings/lex/duckdb.json";

/**
 * `dialect::duckdb::switches_logger`, for the form's startup SQL and the mock: a statement that
 * names DuckDB's logger switches in its code, or calls dynamic SQL with the letters `LOGGING` in
 * it, is refused before the engine sees it (file storage DuckDB cannot write aborts the app).
 */

export const LEX = DUCKDB_LEX as unknown as DialectLex;

/** `session::LOGGER_REFUSED`, in the backend's words. */
export const LOGGER_REFUSED =
  "DuckDB's logger cannot be switched from Tabler: file storage it cannot write stops the database and closes the app. Use the startup SQL of another DuckDB to log.";

const LOGGER_SWITCHES = ["ENABLE_LOGGING", "LOGGING_STORAGE"];
const DYNAMIC_SQL = ["QUERY", "QUERY_TABLE", "JSON_EXECUTE_SERIALIZED_SQL"];

export function switchesLogger(sql: string): boolean {
  if (!asciiUpper(sql).includes("LOGGING")) return false;
  const s = scan(LEX.lex, PLAIN, sql);
  // A text the scanner may read otherwise than DuckDB is judged by its letters alone.
  if (s.ambiguous) return true;
  const code = s.tokens.filter((t) => t.kind !== "space" && t.kind !== "comment");
  const dynamic = code.some((t, i) => {
    const next = code[i + 1];
    const name =
      t.kind === "word" ? upper(s, t) : t.kind === "ident" ? asciiUpper(identName(s, t)) : null;
    return (
      name !== null && next !== undefined && DYNAMIC_SQL.includes(name) && isPunct(s, next, "(")
    );
  });
  if (dynamic) return true;
  return code.some((t) =>
    t.kind === "word"
      ? LOGGER_SWITCHES.includes(upper(s, t))
      : t.kind === "ident" && LOGGER_SWITCHES.some((w) => asciiUpper(text(s, t)).includes(w)),
  );
}
