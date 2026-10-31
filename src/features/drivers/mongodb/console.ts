import { StreamLanguage, type StringStream } from "@codemirror/language";
import type { ConsoleUi } from "../types";
import { classifyShell, shellRanges } from "./shell";

/**
 * The console of a MongoDB session: the mongosh subset the backend runs (`shell.rs`), split and
 * classified as the backend does (`shell.ts`), highlighted by a small tokenizer of its own.
 */

const KEYWORDS = new Set(["db", "rs", "use", "show", "new"]);
const ATOMS = new Set([
  "true",
  "false",
  "null",
  "undefined",
  "NaN",
  "Infinity",
  "MinKey",
  "MaxKey",
]);
const HELPERS = new Set([
  "ObjectId",
  "ISODate",
  "Date",
  "NumberLong",
  "NumberInt",
  "NumberDecimal",
  "Long",
  "Int32",
  "Double",
  "Decimal128",
  "UUID",
  "BinData",
  "HexData",
  "MD5",
  "Timestamp",
  "Code",
  "RegExp",
  "DBRef",
]);

interface State {
  /** Inside a block comment. */
  comment: boolean;
  /** The last code character, for telling a regex from a division. */
  last: string;
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
  if (stream.match("//")) {
    stream.skipToEnd();
    return "comment";
  }
  if (stream.match("/*")) {
    state.comment = true;
    return token(stream, state) ?? "comment";
  }
  const ch = stream.next();
  if (ch === undefined) return null;
  if (ch === '"' || ch === "'" || ch === "`") {
    let escaped = false;
    for (let c = stream.next(); c !== undefined; c = stream.next()) {
      if (c === ch && !escaped) break;
      escaped = !escaped && c === "\\";
    }
    state.last = ch;
    return "string";
  }
  if (ch === "/" && (state.last === "" || "(,:[{=!&|?;".includes(state.last))) {
    let cls = false;
    let escaped = false;
    for (let c = stream.next(); c !== undefined; c = stream.next()) {
      if (!escaped && c === "[") cls = true;
      else if (!escaped && c === "]") cls = false;
      else if (!escaped && !cls && c === "/") break;
      escaped = !escaped && c === "\\";
    }
    stream.eatWhile(/[a-z]/);
    state.last = "/";
    return "string.special";
  }
  if (/[\d]/.test(ch) || (ch === "-" && stream.match(/^\d/, false))) {
    stream.eatWhile(/[\w.+-]/);
    state.last = "0";
    return "number";
  }
  if (/[\p{L}_$]/u.test(ch)) {
    stream.eatWhile(/[\p{L}\p{N}_$]/u);
    const word = stream.current();
    state.last = "a";
    if (KEYWORDS.has(word)) return "keyword";
    if (ATOMS.has(word)) return "atom";
    if (HELPERS.has(word)) return "typeName";
    if (word.startsWith("$")) return "operatorKeyword";
    if (stream.match(/^\s*\(/, false)) return "function";
    if (stream.match(/^\s*:/, false)) return "propertyName";
    return "variableName";
  }
  state.last = ch;
  return /[{}[\]()]/.test(ch) ? "bracket" : "punctuation";
}

const shellLanguage = StreamLanguage.define<State>({
  name: "mongosh",
  startState: () => ({ comment: false, last: "" }),
  token,
  languageData: { commentTokens: { line: "//", block: { open: "/*", close: "*/" } } },
});

export const mongoConsole: ConsoleUi = {
  language: () => shellLanguage,
  split: shellRanges,
  classify: classifyShell,
  fileExtension: "js",
  placeholder: "db.collection.find({ status: 'active' }).limit(20)",
};
