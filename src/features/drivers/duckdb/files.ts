import { callsListedFunction } from "@/features/drivers/portable/classify";
import {
  asciiUpper,
  identName,
  isPunct,
  listed,
  PLAIN,
  scan,
  upper,
  type Scan,
  type Token,
} from "@/features/drivers/portable/scanner";
import { LEX } from "./logger";

/**
 * `dialect::duckdb::reads_files`, for the mock's `admitPluginRun`: what a session that is not
 * hardened refuses of a plugin's run, because a hardened one's engine would (files named where a
 * table goes, the scanners, file statements, extensions, secrets, settings). Both run the cases
 * of `tests/fixtures/duckdb/plugin-files.json`.
 */

/** `session::PLUGIN_FILES_REFUSED`, in the backend's words. */
export const PLUGIN_FILES_REFUSED =
  "A plugin cannot read files, extensions, secrets or settings through this DuckDB connection: open it read-only, or run the statement in the SQL editor.";

const FILE_WORDS = [
  "COPY",
  "ATTACH",
  "INSTALL",
  "LOAD",
  "EXPORT",
  "IMPORT",
  "PRAGMA",
  "SECRET",
  "EXTENSIONS",
];
const SETTING_STATEMENTS = ["SET", "RESET", "CALL", "FORCE"];
const SESSION_CALLS = ["NEXTVAL", "SETSEED"];
const FILE_SUFFIXES = [
  "CSV",
  "TSV",
  "TBL",
  "TXT",
  "DAT",
  "PSV",
  "PARQUET",
  "PARQ",
  "PQ",
  "JSON",
  "JSONL",
  "NDJSON",
  "GEOJSON",
  "GZ",
  "GZIP",
  "ZST",
  "ZSTD",
  "BZ2",
  "XZ",
  "LZ4",
  "ZIP",
  "XLSX",
  "XLS",
  "ARROW",
  "IPC",
  "FEATHER",
  "AVRO",
  "ORC",
  "SHP",
  "GPKG",
  "FGB",
  "SQLITE",
  "SQLITE3",
  "DB",
  "DUCKDB",
  "DDB",
  "VORTEX",
];
const TABLE_AT = [
  "FROM",
  "JOIN",
  "TABLE",
  "DESCRIBE",
  "DESC",
  "SUMMARIZE",
  "SHOW",
  "PIVOT",
  "UNPIVOT",
  "PIVOT_WIDER",
  "PIVOT_LONGER",
];
const NAME_AT = ["USING", "INTO", "UPDATE"];
const TABLE_DEFINED_AFTER = [
  "CREATE",
  "TEMP",
  "TEMPORARY",
  "REPLACE",
  "UNLOGGED",
  "ALTER",
  "DROP",
  "TRUNCATE",
];
const FROM_LIST_ENDS = [
  "WHERE",
  "GROUP",
  "HAVING",
  "QUALIFY",
  "WINDOW",
  "ORDER",
  "LIMIT",
  "OFFSET",
  "UNION",
  "EXCEPT",
  "INTERSECT",
  "RETURNING",
  "SELECT",
];
const QUERY_AT = [
  "SELECT",
  "VALUES",
  "FROM",
  "WITH",
  "TABLE",
  "PIVOT",
  "UNPIVOT",
  "PIVOT_WIDER",
  "PIVOT_LONGER",
  "SUMMARIZE",
  "DESCRIBE",
  "DESC",
  "SHOW",
];
const MAINTENANCE_STATEMENTS = ["TRUNCATE", "VACUUM", "ANALYZE", "ANALYSE"];
const MAINTENANCE_OPTIONS = ["TABLE", "ONLY", "FULL", "FREEZE", "VERBOSE", "ANALYZE", "ANALYSE"];
const FROM_IN_ARGUMENTS = ["EXTRACT", "SUBSTRING", "SUBSTR", "TRIM", "OVERLAY", "POSITION"];

/** The word lists, for the test that keeps them equal to the backend's. */
export const LISTS = {
  FILE_WORDS,
  SETTING_STATEMENTS,
  SESSION_CALLS,
  FILE_SUFFIXES,
  TABLE_AT,
  NAME_AT,
  TABLE_DEFINED_AFTER,
  FROM_LIST_ENDS,
  QUERY_AT,
  MAINTENANCE_STATEMENTS,
  MAINTENANCE_OPTIONS,
  FROM_IN_ARGUMENTS,
};

export function readsFiles(sql: string): boolean {
  const s = scan(LEX.lex, PLAIN, sql);
  if (s.ambiguous || splitsNames(s)) return true;
  const calls = LEX.classify.writeCalls.filter((c) => !SESSION_CALLS.includes(c));
  if (callsListedFunction(LEX.lex, calls, sql)) return true;
  const code = s.tokens.filter((t) => t.kind !== "space" && t.kind !== "comment");
  const word = (i: number): string | null => {
    const t = code[i];
    return t?.kind === "word" ? upper(s, t) : null;
  };
  const punct = (i: number, c: string) => code[i] !== undefined && isPunct(s, code[i]!, c);
  const parens: boolean[] = [];
  const froms: number[] = [];
  const pivots: number[] = [];
  const head: string[] = [];
  let verb = statementVerb(s, code, 0);
  for (let i = 0; i < code.length; i++) {
    const w = word(i);
    if (w !== null && gluedNumber(w)) return true;
    const leading = i <= verb;
    if (w !== null && i >= verb && head.length < 3 && parens.length === 0) head.push(w);
    const h = head.join(" ");
    const index = h.startsWith("CREATE INDEX") || h === "CREATE UNIQUE INDEX";
    if (leading && w !== null && MAINTENANCE_STATEMENTS.includes(w)) {
      if (maintenanceReadsFiles(s, code, i + 1)) return true;
    }
    if (leading && w !== null && SETTING_STATEMENTS.includes(w)) return true;
    if (w === "U" && punct(i + 1, "&")) return true;
    const inFrom = froms.length > 0 && froms[froms.length - 1] === parens.length;
    let reaches = false;
    if (punct(i, ";")) {
      parens.length = 0;
      froms.length = 0;
      pivots.length = 0;
      head.length = 0;
      verb = statementVerb(s, code, i + 1);
    } else if (punct(i, "(")) {
      const args = i > 0 ? word(i - 1) : null;
      const query = punct(i + 1, "(") || QUERY_AT.includes(word(i + 1) ?? "");
      parens.push(!query && args !== null && FROM_IN_ARGUMENTS.includes(args));
    } else if (punct(i, ")")) {
      parens.pop();
      while (froms.length > 0 && froms[froms.length - 1]! > parens.length) froms.pop();
      while (pivots.length > 0 && pivots[pivots.length - 1]! > parens.length) pivots.pop();
    } else if (punct(i, ",")) {
      reaches = inFrom && tableReadsFiles(s, code, i + 1, true);
    } else if (w !== null) {
      const distinct =
        i > 0 && word(i - 1) === "DISTINCT" && i > 1 && ["IS", "NOT"].includes(word(i - 2) ?? "");
      if (FILE_WORDS.includes(w)) reaches = true;
      else if (w === "FROM") {
        if (!distinct && !(parens[parens.length - 1] ?? false)) {
          if (!inFrom) froms.push(parens.length);
          reaches = tableReadsFiles(s, code, i + 1, true);
        }
      } else if (w === "TABLE") {
        const defined = i > 0 && TABLE_DEFINED_AFTER.includes(word(i - 1) ?? "");
        reaches = tableReadsFiles(s, code, i + 1, !defined);
      } else if (TABLE_AT.includes(w)) {
        if (w.includes("PIVOT")) pivots.push(parens.length);
        reaches = tableReadsFiles(s, code, i + 1, true);
      } else if (w === "ON" && index && parens.length === 0) {
        reaches = tableReadsFiles(s, code, i + 1, false);
      } else if (w === "USING") {
        const aggregates = index || pivots[pivots.length - 1] === parens.length;
        reaches = tableReadsFiles(s, code, i + 1, !aggregates);
      } else if (NAME_AT.includes(w)) reaches = tableReadsFiles(s, code, i + 1, false);
      else if (inFrom && FROM_LIST_ENDS.includes(w)) froms.pop();
    }
    if (reaches) return true;
  }
  return false;
}

/**
 * `splits_names`: DuckDB reads every non-ASCII character as part of a name (`r€where` is one
 * alias), so punctuation or a Unicode space the scanner splits a word at may hide a word.
 */
function splitsNames(s: Scan): boolean {
  return s.tokens.some(
    (t) =>
      (t.kind === "punct" || t.kind === "space") &&
      s.chars.slice(t.start, t.end).some((c) => (c.codePointAt(0) ?? 0) > 0x7f),
  );
}

/** `glued_number`: a word that starts with a digit runs into letters other than a number's. */
function gluedNumber(w: string): boolean {
  if (!/^[0-9]/.test(w)) return false;
  return !/^(?:0X[0-9A-F_]*|0B[01_]*|[0-9][0-9_]*(?:E[0-9_]*)?)$/.test(w);
}

/**
 * `statement_verb`: where the statement's verb stands, past `(`, `EXPLAIN [( options )]
 * [ANALYZE] [VERBOSE]` and `PREPARE name [( types )] AS`.
 */
function statementVerb(s: Scan, code: Token[], at: number): number {
  const punct = (i: number, c: string) => code[i] !== undefined && isPunct(s, code[i]!, c);
  const word = (i: number): string | null => {
    const t = code[i];
    return t?.kind === "word" ? upper(s, t) : null;
  };
  const pastGroup = (at: number): number => {
    if (!punct(at, "(")) return at;
    let depth = 0;
    while (code[at] !== undefined) {
      if (punct(at, "(")) depth++;
      else if (punct(at, ")")) depth = Math.max(0, depth - 1);
      at++;
      if (depth === 0) break;
    }
    return at;
  };
  for (;;) {
    if (punct(at, "(")) {
      at++;
      continue;
    }
    const w = word(at);
    if (w === "EXPLAIN") {
      at = pastGroup(at + 1);
      while (["ANALYZE", "ANALYSE", "VERBOSE"].includes(word(at) ?? "")) at++;
    } else if (w === "PREPARE") {
      at = pastGroup(at + 2);
      if (word(at) === "AS") at++;
    } else return at;
  }
}

/** `maintenance_reads_files`:the table of `TRUNCATE`, `VACUUM` or `ANALYZE` past its options. */
function maintenanceReadsFiles(s: Scan, code: Token[], at: number): boolean {
  const punct = (i: number, c: string) => code[i] !== undefined && isPunct(s, code[i]!, c);
  for (;;) {
    if (tableReadsFiles(s, code, at, false)) return true;
    const t = code[at];
    const option = t?.kind === "word" && MAINTENANCE_OPTIONS.includes(upper(s, t));
    if (option && !punct(at + 1, ".") && !punct(at + 1, ":")) at++;
    else if (punct(at, "(")) {
      let depth = 0;
      do {
        if (code[at] === undefined) return false;
        if (punct(at, "(")) depth++;
        else if (punct(at, ")")) depth = Math.max(0, depth - 1);
        at++;
      } while (depth > 0);
    } else return false;
  }
}

/** `pivot_for`: the group opened at `open` holds `FOR` at its own depth. */
function pivotFor(s: Scan, code: Token[], open: number): boolean {
  let depth = 0;
  for (const t of code.slice(open)) {
    if (isPunct(s, t, "(")) depth++;
    else if (isPunct(s, t, ")")) {
      depth = Math.max(0, depth - 1);
      if (depth === 0) return false;
    } else if (depth === 1 && t.kind === "word" && upper(s, t) === "FOR") return true;
  }
  return false;
}

/** `query_at`: `VALUES` and `SUMMARIZE` are unreserved (`FROM values.csv` names a file). */
function queryAt(s: Scan, code: Token[], at: number): boolean {
  const next = (c: string) => code[at + 1] !== undefined && isPunct(s, code[at + 1]!, c);
  const t = code[at];
  if (t === undefined) return false;
  const w = upper(s, t);
  if (w === "VALUES") return next("(");
  return QUERY_AT.includes(w) && !next(".") && !next(":");
}

/** `table_reads_files`. */
function tableReadsFiles(s: Scan, code: Token[], at: number, calls: boolean): boolean {
  const punct = (i: number, c: string) => code[i] !== undefined && isPunct(s, code[i]!, c);
  const word = (i: number): string | null => {
    const t = code[i];
    return t?.kind === "word" ? upper(s, t) : null;
  };
  const pivot =
    punct(at, "(") && at > 0 && (word(at - 1) ?? "").includes("PIVOT") && pivotFor(s, code, at);
  while (punct(at, "(") || word(at) === "LATERAL" || word(at) === "ONLY") at++;
  const first = code[at];
  if (first === undefined) return false;
  if (first.kind === "str") return true;
  if (first.kind === "word" && queryAt(s, code, at)) return false;
  if (first.kind !== "word" && first.kind !== "ident") return false;
  const parts: { kind: "word" | "ident"; name: string }[] = [];
  let i = at;
  for (;;) {
    const t = code[i];
    if (t?.kind === "word") parts.push({ kind: "word", name: upper(s, t) });
    else if (t?.kind === "ident") parts.push({ kind: "ident", name: identName(s, t) });
    else break;
    if (punct(i + 1, ".") && code[i + 2] !== undefined) i += 2;
    else break;
  }
  if (parts.some((p) => p.kind === "ident" && /[./\\:*?~[{%]/.test(p.name))) return true;
  const last = parts[parts.length - 1];
  if (parts.length > 1 && last && FILE_SUFFIXES.includes(asciiUpper(last.name))) return true;
  // A prefix alias (`FROM f: 'x.csv'`): the table follows the colon.
  if (punct(i + 1, ":")) return tableReadsFiles(s, code, i + 2, calls);
  if (!(calls && punct(i + 1, "("))) return false;
  const allowed = LEX.classify.tableFunctions ?? [];
  const only = parts.length === 1 ? parts[0] : undefined;
  if (only?.kind !== "word") return true;
  return !allowed.some((entry) =>
    entry.startsWith("%") ? pivot && entry.slice(1) === only.name : listed([entry], only.name),
  );
}
