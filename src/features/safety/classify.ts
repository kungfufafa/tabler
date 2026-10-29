import type { Dialect } from "@/lib/ipc";

/**
 * Safe-mode statement classifier: is a statement a READ (Alert Mode 2 / Safe Mode 2 let it through
 * without asking) or a WRITE? Mirrors `src-tauri/src/sqlgen/classify.rs` rule for rule; both run
 * the shared cases in `classify.cases.json`.
 *
 * READ: SELECT / VALUES / TABLE (unless they hold an `INTO`), SHOW, USE <db>, EXPLAIN / DESCRIBE /
 * DESC (an EXPLAIN ANALYZE is classified by the statement it runs, an EXPLAIN of a PRAGMA as the
 * PRAGMA, which SQLite applies while preparing it), WITH … without a data-modifying keyword, and
 * SQLite PRAGMAs that only read. Everything else, including empty and unknown statements, is a
 * WRITE, and so is any statement that calls `set_config(…)` (it changes settings,
 * `default_transaction_read_only` too; on PostgreSQL also in attribute notation,
 * `(…).set_config`). Comments, strings and quoted identifiers are ignored, so
 * `-- delete` or `'DROP'` change nothing; a `;` outside them starts another statement (all of them
 * must be reads). The text is read under every quoting / comment mode the server may be in, and is
 * a READ only under all of them. The check is deliberately coarse in the safe direction: a doubtful
 * statement is a WRITE (`WITH … FOR UPDATE` asks too).
 */
export type StatementClass = "read" | "write";

/** Words that make a `WITH …` statement a write wherever they appear in its code. */
const WITH_WRITE_WORDS = new Set([
  "INSERT",
  "UPDATE",
  "DELETE",
  "MERGE",
  "CREATE",
  "DROP",
  "ALTER",
  "TRUNCATE",
  "INTO",
]);
/** EXPLAIN / DESCRIBE options that come before the explained statement. */
const EXPLAIN_OPTION_WORDS = new Set(["VERBOSE", "EXTENDED", "PARTITIONS", "QUERY", "PLAN"]);
/** PRAGMAs that change the database although they take no `= value`. */
const ACTION_PRAGMAS = new Set(["OPTIMIZE", "WAL_CHECKPOINT", "INCREMENTAL_VACUUM"]);
/** PRAGMAs whose `(argument)` names what to read rather than a value to set. */
const READ_ARG_PRAGMAS = new Set([
  "TABLE_INFO",
  "TABLE_XINFO",
  "TABLE_LIST",
  "INDEX_LIST",
  "INDEX_INFO",
  "INDEX_XINFO",
  "FOREIGN_KEY_LIST",
  "FOREIGN_KEY_CHECK",
  "INTEGRITY_CHECK",
  "QUICK_CHECK",
]);

/**
 * How the server may read quotes and comments (it depends on server settings the classifier
 * cannot see); a statement is a READ only under every reading of its dialect.
 */
export interface Reading {
  /**
   * `'…'` strings take backslash escapes: MySQL unless `NO_BACKSLASH_ESCAPES`, PostgreSQL with
   * `standard_conforming_strings = off`.
   */
  singleBackslash: boolean;
  /**
   * `"…"` strings take backslash escapes: MySQL unless `ANSI_QUOTES` (an identifier then) or
   * `NO_BACKSLASH_ESCAPES`.
   */
  doubleBackslash: boolean;
  /**
   * MySQL / MariaDB conditional comments run: versioned `/*!NNNNN … *\/`, MariaDB's `/*M! … *\/`
   * and optimizer hints `/*+ … *\/` are code. Otherwise they are plain comments, as on a server
   * older than the version, on MySQL for `/*M!`, or for a hint out of place. An unversioned
   * `/*! … *\/` is code under every reading.
   */
  execComments: boolean;
}

const reading = (
  singleBackslash: boolean,
  doubleBackslash: boolean,
  execComments: boolean,
): Reading => ({ singleBackslash, doubleBackslash, execComments });

const READINGS: Record<Dialect, readonly Reading[]> = {
  postgres: [reading(false, false, false), reading(true, false, false)],
  // Default, NO_BACKSLASH_ESCAPES, ANSI_QUOTES (backslash escapes in '…' only); each with and
  // without the conditional comments.
  mysql: [
    reading(true, true, true),
    reading(false, false, true),
    reading(true, false, true),
    reading(true, true, false),
    reading(false, false, false),
    reading(true, false, false),
  ],
  sqlite: [reading(false, false, false)],
};

const IDENT_START = /[\p{L}_]/u;
const WORD_CHAR = /[\p{L}\p{N}_$]/u;
const isIdentStart = (c: string | undefined) => c !== undefined && IDENT_START.test(c);
const isWordChar = (c: string | undefined) => c !== undefined && WORD_CHAR.test(c);
/**
 * Whether PostgreSQL reads `c` as part of an identifier: every non-ASCII character is one (so `€$$`
 * is a single name, not a name and a dollar quote), as are the letters, digits, `_` and `$`.
 */
const isPgWordChar = (c: string | undefined) =>
  c !== undefined && (c.codePointAt(0)! > 0x7f || isWordChar(c));
const isSpace = (c: string | undefined) => c !== undefined && /\s/u.test(c);
/** Unicode Cc (Rust's `char::is_control`): U+0000–U+001F and U+007F–U+009F. */
const isControl = (c: string | undefined) => {
  const code = c?.codePointAt(0);
  return code !== undefined && (code <= 0x1f || (code >= 0x7f && code <= 0x9f));
};

/** Index just past the quote that closes the one at `i` (`qq` doubled = escaped). */
function skipQuoted(chars: string[], i: number, quote: string, backslash: boolean): number {
  let j = i + 1;
  while (j < chars.length) {
    const c = chars[j];
    if (backslash && c === "\\") {
      j += 2;
    } else if (c === quote) {
      if (chars[j + 1] === quote) j += 2;
      else return j + 1;
    } else {
      j += 1;
    }
  }
  return chars.length;
}

/**
 * End of the line comment that starts at `i`. `crEnds`: PostgreSQL ends one at a lone CR as well as
 * at LF (`SELECT 1 --x<CR>, pg_sleep(100)` calls `pg_sleep`); MySQL and SQLite read to the LF.
 */
function skipLine(chars: string[], i: number, crEnds: boolean): number {
  let j = i;
  while (j < chars.length && chars[j] !== "\n" && !(crEnds && chars[j] === "\r")) j += 1;
  return j;
}

function skipBlockComment(chars: string[], i: number, nested: boolean): number {
  let depth = 0;
  let j = i;
  while (j < chars.length) {
    if (chars[j] === "/" && chars[j + 1] === "*") {
      depth = nested ? depth + 1 : 1;
      j += 2;
    } else if (chars[j] === "*" && chars[j + 1] === "/") {
      depth -= 1;
      j += 2;
      if (depth <= 0) return j;
    } else {
      j += 1;
    }
  }
  return chars.length;
}

/** Length of the PostgreSQL dollar-quote tag (`$$`, `$body$`) at `i`, or 0. */
function dollarTag(chars: string[], i: number): number {
  if (isPgWordChar(chars[i - 1])) return 0;
  let j = i + 1;
  if (isIdentStart(chars[j])) {
    while (j < chars.length && isWordChar(chars[j]) && chars[j] !== "$") j += 1;
  }
  return chars[j] === "$" ? j - i + 1 : 0;
}

function skipDollar(chars: string[], i: number, len: number): number {
  const tag = chars.slice(i, i + len).join("");
  for (let j = i + len; j + len <= chars.length; j += 1) {
    if (chars[j] === "$" && chars.slice(j, j + len).join("") === tag) return j + len;
  }
  return chars.length;
}

/**
 * Length of the opening of the MySQL / MariaDB comment at `i` when its body is code under
 * `reading` (`/*!`, `/*M!`, `/*+`; version digits stay in the code as a number), else 0.
 */
function codeCommentOpen(chars: string[], i: number, reading: Reading): number {
  const exec = reading.execComments;
  const c = chars[i + 2];
  const next = chars[i + 3];
  if (c === "!") return exec || next === undefined || !/[0-9]/.test(next) ? 3 : 0;
  if (c === "M" && next === "!") return exec ? 4 : 0;
  if (c === "+") return exec ? 3 : 0;
  return 0;
}

/** The PostgreSQL function that changes settings from a query. */
const SET_CONFIG = "set_config";

/**
 * `sql` with every comment, string literal, quoted identifier and dollar-quoted body replaced by
 * one space, so keyword checks see only code. MySQL executable comments and optimizer hints are
 * code as `reading` says (see `Reading`). A PostgreSQL `"set_config"` stays a name.
 */
export function codeOnly(sql: string, dialect: Dialect, reading: Reading): string[] {
  const chars = Array.from(sql);
  const mysql = dialect === "mysql";
  const pg = dialect === "postgres";
  const out: string[] = [];
  let i = 0;
  while (i < chars.length) {
    const c = chars[i]!;
    const next = chars[i + 1];
    let end: number | null = null;
    if (c === "'") {
      end = skipQuoted(chars, i, c, reading.singleBackslash);
    } else if (c === '"') {
      end = skipQuoted(chars, i, c, reading.doubleBackslash);
      if (pg && chars.slice(i + 1, end - 1).join("") === SET_CONFIG) {
        out.push(...SET_CONFIG);
        i = end;
        continue;
      }
    } else if (c === "`" && !pg) {
      end = skipQuoted(chars, i, "`", false);
    } else if (c === "[" && dialect === "sqlite") {
      const close = chars.indexOf("]", i + 1);
      end = close < 0 ? chars.length : close + 1;
    } else if (
      c === "-" &&
      next === "-" &&
      (!mysql || chars[i + 2] === undefined || isSpace(chars[i + 2]) || isControl(chars[i + 2]))
    ) {
      end = skipLine(chars, i, pg);
    } else if (c === "#" && mysql) {
      end = skipLine(chars, i, false);
    } else if (c === "/" && next === "*") {
      const len = mysql ? codeCommentOpen(chars, i, reading) : 0;
      if (len > 0) {
        out.push(...Array<string>(len).fill(" "));
        i += len;
        continue;
      }
      end = skipBlockComment(chars, i, pg);
    } else if (c === "$" && pg) {
      const len = dollarTag(chars, i);
      if (len > 0) end = skipDollar(chars, i, len);
    } else if (isIdentStart(c)) {
      let j = i;
      while (j < chars.length && isWordChar(chars[j])) j += 1;
      out.push(...chars.slice(i, j));
      // PostgreSQL escape strings: E'…' with backslash escapes (`€e'…'` is a name and a plain
      // string: every non-ASCII character is part of a name).
      if (
        pg &&
        j === i + 1 &&
        !isPgWordChar(chars[i - 1]) &&
        (c === "e" || c === "E") &&
        chars[j] === "'"
      ) {
        out.push(" ");
        i = skipQuoted(chars, j, "'", true);
      } else {
        i = j;
      }
      continue;
    }
    if (end === null) {
      out.push(c);
      i += 1;
    } else {
      out.push(" ");
      i = Math.max(end, i + 1);
    }
  }
  return out;
}

/** A word (upper-cased) or a single punctuation character of code-only text. */
interface Token {
  word: boolean;
  text: string;
}

/** Splits code-only text at `;` into the tokens of each statement (empty ones dropped). */
function statementTokens(code: string[]): Token[][] {
  const statements: Token[][] = [];
  let current: Token[] = [];
  let i = 0;
  while (i < code.length) {
    const c = code[i]!;
    if (c === ";") {
      if (current.length > 0) statements.push(current);
      current = [];
      i += 1;
    } else if (isSpace(c)) {
      i += 1;
    } else if (isWordChar(c)) {
      let j = i;
      while (j < code.length && isWordChar(code[j])) j += 1;
      // Numbers count as words too: `50000` is just not a keyword.
      current.push({ word: true, text: code.slice(i, j).join("").toUpperCase() });
      i = j;
    } else {
      current.push({ word: false, text: c });
      i += 1;
    }
  }
  if (current.length > 0) statements.push(current);
  return statements;
}

const hasWord = (tokens: Token[], words: ReadonlySet<string>) =>
  tokens.some((t) => t.word && words.has(t.text));
const INTO = new Set(["INTO"]);

function classifyExplain(tokens: Token[], attributes: boolean): StatementClass {
  let analyze = false;
  let i = 0;
  if (tokens[0]?.text === "(") {
    // PostgreSQL option list: EXPLAIN (ANALYZE, BUFFERS) … — any ANALYZE counts, even `false`.
    let depth = 0;
    for (; i < tokens.length; i += 1) {
      const t = tokens[i]!;
      if (t.text === "(") depth += 1;
      else if (t.text === ")") depth -= 1;
      else if (t.word && (t.text === "ANALYZE" || t.text === "ANALYSE")) analyze = true;
      if (depth === 0) {
        i += 1;
        break;
      }
    }
  }
  for (;;) {
    const t = tokens[i];
    if (!t?.word) break;
    if (t.text === "ANALYZE" || t.text === "ANALYSE") {
      analyze = true;
      i += 1;
    } else if (EXPLAIN_OPTION_WORDS.has(t.text)) {
      i += 1;
    } else if (t.text === "FORMAT") {
      i += 1;
      if (tokens[i]?.text === "=") i += 1;
      if (tokens[i]?.word) i += 1;
    } else {
      break;
    }
  }
  // A plain EXPLAIN only plans; EXPLAIN ANALYZE runs the statement. SQLite applies a setting
  // PRAGMA while it prepares it, so `EXPLAIN [QUERY PLAN] PRAGMA query_only = 0` changes the
  // setting: it is the PRAGMA. A lone `pragma` is a table to MySQL's EXPLAIN / DESCRIBE.
  const pragma = tokens[i]?.text === "PRAGMA" && tokens.length > i + 1;
  return analyze || pragma ? classifyTokens(tokens.slice(i), attributes) : "read";
}

function classifyPragma(tokens: Token[]): StatementClass {
  if (tokens.some((t) => t.text === "=")) return "write";
  // PRAGMA [schema.]name[(argument)]
  let i = 0;
  if (tokens[1]?.text === ".") i = 2;
  const name = tokens[i];
  if (!name?.word) return "write";
  if (ACTION_PRAGMAS.has(name.text)) return "write";
  if (tokens[i + 1]?.text === "(" && !READ_ARG_PRAGMAS.has(name.text)) return "write";
  return "read";
}

/**
 * Whether the word at `i` is selected from a value: `(expr).name`, `arr[1].name`, or a later name
 * of such a chain (`(expr).a.name`, `(expr)."a".name`). PostgreSQL calls `name(value)` when the
 * value has no field of that name (attribute notation). A name after a plain name (`t.name`,
 * `t."a".name`) is a column or a qualified name, never such a call.
 */
function inAttributeNotation(tokens: Token[], i: number): boolean {
  let k = i;
  while (k >= 2 && tokens[k - 1]!.text === ".") {
    const before = tokens[k - 2]!;
    if (before.text === ")" || before.text === "]") return true;
    if (before.word) {
      k -= 2;
    } else if (before.text === ".") {
      // Two dots in a row: a quoted name of the chain stood between them, blanked to a space
      // (`(x)."f".set_config`), so the walk goes on from the first dot.
      k -= 1;
    } else {
      return false;
    }
  }
  return false;
}

/**
 * `set_config(…)`, `pg_catalog.set_config(…)`: changes settings (even
 * `default_transaction_read_only`) although it is called from a SELECT. With `attributes`
 * (PostgreSQL) also in attribute notation (`(…).set_config`).
 */
const callsSetConfig = (tokens: Token[], attributes: boolean) =>
  tokens.some(
    (t, k) =>
      t.word &&
      t.text === "SET_CONFIG" &&
      (tokens[k + 1]?.text === "(" || (attributes && inAttributeNotation(tokens, k))),
  );

function classifyTokens(tokens: Token[], attributes: boolean): StatementClass {
  if (callsSetConfig(tokens, attributes)) return "write";
  let i = 0;
  while (tokens[i]?.text === "(") i += 1;
  const first = tokens[i];
  if (!first?.word) return "write";
  const rest = tokens.slice(i + 1);
  switch (first.text) {
    case "SELECT":
    case "VALUES":
    case "TABLE":
      return hasWord(rest, INTO) ? "write" : "read";
    case "WITH":
      return hasWord(rest, WITH_WRITE_WORDS) ? "write" : "read";
    case "SHOW":
    case "USE":
      return "read";
    case "EXPLAIN":
    case "DESCRIBE":
    case "DESC":
      return classifyExplain(rest, attributes);
    case "PRAGMA":
      return classifyPragma(rest);
    default:
      return "write";
  }
}

function readsUnder(sql: string, dialect: Dialect, reading: Reading): boolean {
  const statements = statementTokens(codeOnly(sql, dialect, reading));
  const attributes = dialect === "postgres";
  return (
    statements.length > 0 &&
    statements.every((tokens) => classifyTokens(tokens, attributes) === "read")
  );
}

/**
 * Classifies one statement (several separated by `;` are a READ only when each one is). The text
 * is read under every quoting / comment mode of the dialect (see `Reading`: MySQL with and without
 * backslash escapes, `ANSI_QUOTES`, conditional comments; PostgreSQL with and without
 * `standard_conforming_strings`) and is a READ only under all of them.
 */
export function classifyStatement(sql: string, dialect: Dialect): StatementClass {
  return READINGS[dialect].every((r) => readsUnder(sql, dialect, r)) ? "read" : "write";
}

/** True when there is at least one statement and every one of them is a READ. */
export function allReads(statements: readonly string[], dialect: Dialect): boolean {
  return statements.length > 0 && statements.every((s) => classifyStatement(s, dialect) === "read");
}

/**
 * A classifier of the statements of a session the core classifier cannot read: the portable
 * classifier over an extension SQL dialect's lexer profile, or a native store's console
 * classifier (`ConsoleUi.classify`). Unknown commands must be writes.
 */
export type Classifier = (statement: string) => StatementClass;

/** `allReads` with another classifier: at least one statement, and every one a READ. */
export function allReadsWith(statements: readonly string[], classify: Classifier): boolean {
  return statements.length > 0 && statements.every((s) => classify(s) === "read");
}
