import { StreamLanguage, type StringStream } from "@codemirror/language";
import type { StatementRange } from "@/features/editor/scanner";
import type { ConsoleUi } from "../types";
import { classifyConsole } from "./classify";
import { METHODS, SQL_WORDS, split } from "./request";
import { utf16Offsets } from "./rust";

/**
 * The console of an Elasticsearch or OpenSearch session: Kibana Dev Tools requests and SQL
 * statements, split and classified as the backend does (`request.ts`, `classify.ts`), and
 * highlighted by a small tokenizer of its own.
 */

/** The statements of `text` as the editor's ranges (UTF-16 offsets, 1-based lines). */
export function consoleRanges(text: string): StatementRange[] {
  const at = utf16Offsets(text);
  return split(text).map((s) => ({ start: at[s.start]!, end: at[s.end]!, line: s.line }));
}

const METHOD_SET = new Set<string>(METHODS);
const SQL_KEYWORDS = new Set([
  ...SQL_WORDS,
  "FROM",
  "WHERE",
  "AND",
  "OR",
  "NOT",
  "IN",
  "AS",
  "BY",
  "GROUP",
  "ORDER",
  "HAVING",
  "LIMIT",
  "ASC",
  "DESC",
  "LIKE",
  "IS",
  "NULL",
  "TABLES",
  "COLUMNS",
  "FUNCTIONS",
  "BETWEEN",
  "MATCH",
  "QUERY",
  "DISTINCT",
  "JOIN",
  "ON",
  "CASE",
  "WHEN",
  "THEN",
  "ELSE",
  "END",
]);

interface State {
  /** Inside a `/* *\/` comment or a `"""` string that runs over lines. */
  block: "comment" | "triple" | null;
  /** The line's first word was a method: the path comes next. */
  path: boolean;
  /** Inside a SQL statement. */
  sql: boolean;
}

function token(stream: StringStream, state: State): string | null {
  if (state.block === "comment") {
    if (stream.skipTo("*/")) {
      stream.match("*/");
      state.block = null;
    } else stream.skipToEnd();
    return "comment";
  }
  if (state.block === "triple") {
    if (stream.skipTo('"""')) {
      stream.match('"""');
      state.block = null;
    } else stream.skipToEnd();
    return "string";
  }
  if (stream.sol()) {
    state.path = false;
    const word = stream.match(/^\s*([A-Za-z]+)(?=\s|$)/, false) as RegExpMatchArray | null;
    const first = word?.[1]?.toUpperCase();
    if (first && METHOD_SET.has(first)) state.sql = false;
    else if (first && SQL_WORDS.includes(first)) state.sql = true;
  }
  if (stream.eatSpace()) return null;
  if (stream.match("#") || stream.match("//")) {
    stream.skipToEnd();
    return "comment";
  }
  if (stream.match("/*")) {
    state.block = "comment";
    return token(stream, state) ?? "comment";
  }
  if (state.path) {
    stream.eatWhile(/\S/);
    state.path = false;
    return "string.special";
  }
  if (stream.match('"""')) {
    state.block = "triple";
    return token(stream, state) ?? "string";
  }
  const ch = stream.next();
  if (ch === undefined) return null;
  if (ch === '"' || (state.sql && ch === "'")) {
    let escaped = false;
    for (let c = stream.next(); c !== undefined; c = stream.next()) {
      if (c === ch && !escaped) break;
      escaped = !escaped && c === "\\";
    }
    return !state.sql && stream.match(/^\s*:/, false) ? "propertyName" : "string";
  }
  if (/[\d-]/.test(ch)) {
    stream.eatWhile(/[\w.+-]/);
    return "number";
  }
  if (/[A-Za-z_]/.test(ch)) {
    stream.eatWhile(/[\w]/);
    const word = stream.current();
    const up = word.toUpperCase();
    if (stream.column() === stream.indentation() && METHOD_SET.has(up)) {
      state.path = true;
      return "keyword";
    }
    if (state.sql) return SQL_KEYWORDS.has(up) ? "keyword" : "variableName";
    if (word === "true" || word === "false" || word === "null") return "atom";
    return "variableName";
  }
  return /[{}[\]()]/.test(ch) ? "bracket" : "punctuation";
}

const esLanguage = StreamLanguage.define<State>({
  name: "elasticsearch",
  startState: () => ({ block: null, path: false, sql: false }),
  token,
  languageData: { commentTokens: { line: "#", block: { open: "/*", close: "*/" } } },
});

export const esConsole: ConsoleUi = {
  language: () => esLanguage,
  split: consoleRanges,
  classify: classifyConsole,
  fileExtension: "es",
  placeholder: 'GET /_cat/indices?v\n\nPOST /logs/_search\n{"query": {"match_all": {}}}',
};
