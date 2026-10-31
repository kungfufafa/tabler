import { StreamLanguage, type StringStream } from "@codemirror/language";
import type { ConsoleUi } from "../types";
import { classifyConsole, consoleRanges, isKnownAction } from "./classify";

/**
 * The console of a DynamoDB session: PartiQL statements and API commands (`Scan {"TableName":
 * "t"}`), split and classified as the backend does (`classify.ts`), highlighted by a small
 * tokenizer of its own (PartiQL's comments, strings, quoted names and Ion literals; the actions
 * and their JSON).
 */

const KEYWORDS = new Set(
  [
    "SELECT",
    "FROM",
    "WHERE",
    "AND",
    "OR",
    "NOT",
    "IN",
    "IS",
    "BETWEEN",
    "INSERT",
    "INTO",
    "VALUE",
    "UPDATE",
    "SET",
    "REMOVE",
    "DELETE",
    "RETURNING",
    "ALL",
    "OLD",
    "NEW",
    "MODIFIED",
    "ORDER",
    "BY",
    "ASC",
    "DESC",
    "EXISTS",
  ].map((w) => w.toLowerCase()),
);
const FUNCTIONS = new Set([
  "begins_with",
  "contains",
  "size",
  "attribute_type",
  "attribute_exists",
  "attribute_not_exists",
  "list_append",
  "set_add",
  "set_delete",
]);
const ATOMS = new Set(["true", "false", "null", "missing"]);

interface State {
  /** Inside a block comment. */
  comment: boolean;
}

function quoted(stream: StringStream, quote: string): void {
  for (let c = stream.next(); c !== undefined; c = stream.next()) {
    if (c === "\\" && quote === '"') stream.next();
    else if (c === quote) {
      if (stream.peek() === quote && quote === "'") stream.next();
      else return;
    }
  }
}

function token(stream: StringStream, state: State): string | null {
  if (state.comment) {
    if (stream.skipTo("*/")) {
      stream.match("*/");
      state.comment = false;
    } else stream.skipToEnd();
    return "comment";
  }
  if (stream.eatSpace()) return null;
  if (stream.match("--")) {
    stream.skipToEnd();
    return "comment";
  }
  if (stream.match("/*")) {
    state.comment = true;
    return token(stream, state) ?? "comment";
  }
  const ch = stream.next();
  if (ch === undefined) return null;
  if (ch === "'") {
    quoted(stream, "'");
    return "string";
  }
  if (ch === '"') {
    quoted(stream, '"');
    // A JSON member's name.
    return stream.match(/^\s*:/, false) ? "propertyName" : "string";
  }
  if (ch === "`") {
    if (stream.skipTo("`")) stream.next();
    else stream.skipToEnd();
    return "string.special";
  }
  if (ch === "?") return "variableName.special";
  if (/\d/.test(ch) || (ch === "-" && stream.match(/^\d/, false))) {
    stream.eatWhile(/[\d.eE+-]/);
    return "number";
  }
  if (/[A-Za-z_]/.test(ch)) {
    stream.eatWhile(/[\w$]/);
    const word = stream.current();
    const lower = word.toLowerCase();
    if (isKnownAction(word)) return "typeName";
    if (KEYWORDS.has(lower)) return "keyword";
    if (ATOMS.has(lower)) return "atom";
    if (FUNCTIONS.has(lower)) return "function";
    return "variableName";
  }
  return /[{}[\]()]/.test(ch) ? "bracket" : "punctuation";
}

const partiqlLanguage = StreamLanguage.define<State>({
  name: "partiql",
  startState: () => ({ comment: false }),
  token,
  languageData: { commentTokens: { line: "--", block: { open: "/*", close: "*/" } } },
});

export const dynamoConsole: ConsoleUi = {
  language: () => partiqlLanguage,
  split: consoleRanges,
  classify: classifyConsole,
  fileExtension: "partiql",
  placeholder: `SELECT * FROM "Music" WHERE "Artist" = 'Acme'   or   Scan {"TableName": "Music"}`,
};
