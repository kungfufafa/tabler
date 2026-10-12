/**
 * The first word of one SQL statement, upper-cased: what follows leading whitespace, `--` / `#`
 * line comments, `/* … *\/` block comments and opening parentheses. Null when the statement is
 * empty, only comments, or starts with something other than a word.
 */
export function leadingKeyword(sql: string): string | null {
  let i = 0;
  const n = sql.length;
  while (i < n) {
    const c = sql[i]!;
    if (/\s/.test(c) || c === "(") {
      i++;
    } else if ((c === "-" && sql[i + 1] === "-") || c === "#") {
      const end = sql.indexOf("\n", i);
      i = end < 0 ? n : end + 1;
    } else if (c === "/" && sql[i + 1] === "*") {
      const end = sql.indexOf("*/", i + 2);
      i = end < 0 ? n : end + 2;
    } else {
      break;
    }
  }
  const word = /^[\p{L}_][\p{L}\p{N}_$]*/u.exec(sql.slice(i))?.[0];
  return word === undefined ? null : word.toUpperCase();
}

/** First words of statements that only read. */
const READ_KEYWORDS = new Set([
  "SELECT",
  "WITH",
  "VALUES",
  "TABLE",
  "SHOW",
  "EXPLAIN",
  "DESCRIBE",
  "DESC",
]);

/**
 * Words that make a read statement write, wherever they appear: data-modifying CTEs
 * (`WITH d AS (DELETE … RETURNING *) SELECT …`), `EXPLAIN ANALYZE` (runs the statement),
 * `SELECT … INTO` (creates a table or writes a file), sequence calls. They are matched in the raw
 * text, strings and comments included, so a literal such as `'delete'` errs on the safe side.
 */
const WRITE_WORDS = /\b(?:INSERT|UPDATE|DELETE|MERGE|INTO|ANALYZE|ANALYSE|NEXTVAL|SETVAL)\b/i;

/** Row locks (`FOR UPDATE`, `FOR NO KEY UPDATE`) end with the statement; they write nothing. */
const ROW_LOCK = /\bFOR\s+(?:NO\s+KEY\s+)?UPDATE\b/gi;

/**
 * Whether running `sql` (one statement) a second time only reads, so a result can be exported in
 * full by running its statement again. Conservative: DML (also with `RETURNING`), `CALL`, `DO`,
 * `EXPLAIN ANALYZE`, `SELECT … INTO` and anything that is not a plain query are refused. A
 * function with side effects called from a `SELECT` cannot be told apart.
 */
export function isReadOnlyStatement(sql: string): boolean {
  const first = leadingKeyword(sql);
  if (first === null || !READ_KEYWORDS.has(first)) return false;
  return !WRITE_WORDS.test(sql.replace(ROW_LOCK, " "));
}
