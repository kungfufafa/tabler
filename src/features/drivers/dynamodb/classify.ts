import type { StatementRange } from "@/features/editor/scanner";
import type { StatementClass } from "@/features/safety/classify";

/**
 * The console's splitter and classifier, a mirror of the backend's (`sqlgen/native/dynamo.rs`:
 * `split_console`, `classify_console`, `parse_command`, `partiql_class`, `api_class`, and
 * `drivers/dynamodb/attr.rs` `parse_json`), run on the same case file
 * (`src-tauri/tests/fixtures/dynamodb/console.cases.json`). Text is read by code point, as Rust
 * reads `char`s; whitespace is Unicode's `White_Space`, as `char::is_whitespace`.
 *
 * A command is a PartiQL statement (sent with `ExecuteStatement`) or an API request
 * (`Action {json}`). A statement reads only when it is one `SELECT` every lexer reads the same
 * way; a request only when its action reads, or (the statement actions) when every statement it
 * carries does and its body holds only the members the API reference names. Anything else,
 * unknown actions included, writes.
 */

/** The actions that only read (the API reference, October 2026). */
export const READ_ACTIONS: readonly string[] = [
  "BatchGetItem",
  "DescribeBackup",
  "DescribeContinuousBackups",
  "DescribeContributorInsights",
  "DescribeEndpoints",
  "DescribeExport",
  "DescribeGlobalTable",
  "DescribeGlobalTableSettings",
  "DescribeImport",
  "DescribeKinesisStreamingDestination",
  "DescribeLimits",
  "DescribeTable",
  "DescribeTableReplicaAutoScaling",
  "DescribeTimeToLive",
  "GetItem",
  "GetResourcePolicy",
  "ListBackups",
  "ListContributorInsights",
  "ListExports",
  "ListGlobalTables",
  "ListImports",
  "ListTables",
  "ListTagsOfResource",
  "Query",
  "Scan",
  "SearchVectors",
  "TransactGetItems",
];

/** The other actions of the reference: writes, or (the statement actions) as their statements. */
export const OTHER_ACTIONS: readonly string[] = [
  "BatchExecuteStatement",
  "BatchWriteItem",
  "CreateBackup",
  "CreateGlobalTable",
  "CreateTable",
  "DeleteBackup",
  "DeleteItem",
  "DeleteResourcePolicy",
  "DeleteTable",
  "DisableKinesisStreamingDestination",
  "EnableKinesisStreamingDestination",
  "ExecuteStatement",
  "ExecuteTransaction",
  "ExportTableToPointInTime",
  "ImportTable",
  "PutItem",
  "PutResourcePolicy",
  "RestoreTableFromBackup",
  "RestoreTableToPointInTime",
  "TagResource",
  "TransactWriteItems",
  "UntagResource",
  "UpdateContinuousBackups",
  "UpdateContributorInsights",
  "UpdateGlobalTable",
  "UpdateGlobalTableSettings",
  "UpdateItem",
  "UpdateKinesisStreamingDestination",
  "UpdateTable",
  "UpdateTableReplicaAutoScaling",
  "UpdateTimeToLive",
];

export const isReadAction = (action: string): boolean => READ_ACTIONS.includes(action);
export const isKnownAction = (action: string): boolean =>
  isReadAction(action) || OTHER_ACTIONS.includes(action);

const EXECUTE_STATEMENT_MEMBERS = [
  "ConsistentRead",
  "Limit",
  "NextToken",
  "Parameters",
  "ReturnConsumedCapacity",
  "ReturnValuesOnConditionCheckFailure",
  "Statement",
];
const BATCH_EXECUTE_STATEMENT_MEMBERS = ["ReturnConsumedCapacity", "Statements"];
const BATCH_STATEMENT_MEMBERS = [
  "ConsistentRead",
  "Parameters",
  "ReturnValuesOnConditionCheckFailure",
  "Statement",
];
const EXECUTE_TRANSACTION_MEMBERS = [
  "ClientRequestToken",
  "ReturnConsumedCapacity",
  "TransactStatements",
];
const TRANSACT_STATEMENT_MEMBERS = [
  "Parameters",
  "ReturnValuesOnConditionCheckFailure",
  "Statement",
];

/** DynamoDB refuses longer PartiQL statements. */
export const MAX_STATEMENT_CHARS = 8192;

// --- JSON as the backend reads it -------------------------------------------------------------

/** JSON as written (`attr::Json`): numbers keep their text, objects their entries in order. */
export type Json =
  | { k: "null" }
  | { k: "bool"; v: boolean }
  | { k: "num"; v: string }
  | { k: "str"; v: string }
  | { k: "arr"; v: Json[] }
  | { k: "obj"; v: [string, Json][] };

/** As the backend's `attr::MAX_DEPTH`. */
const MAX_DEPTH = 64;

class JsonError extends Error {}

/**
 * The backend's `parse_json`, then what `serde_json` adds: a repeated key, nesting past 64, a
 * lone surrogate or a number beyond a double's range is an error, so what is sent is what was
 * classified. Throws a `JsonError`.
 */
export function parseJson(text: string): Json {
  let i = 0;
  const fail = (why: string): never => {
    throw new JsonError(`invalid JSON at character ${i + 1}: ${why}`);
  };
  const ws = () => {
    while (i < text.length && " \t\n\r".includes(text[i]!)) i += 1;
  };
  const eat = (word: string): boolean => {
    if (!text.startsWith(word, i)) return false;
    i += word.length;
    return true;
  };
  const digits = (): number => {
    const start = i;
    while (i < text.length && text[i]! >= "0" && text[i]! <= "9") i += 1;
    return i - start;
  };
  const hex4 = (): number => {
    const hex = text.slice(i, i + 4);
    if (!/^[0-9a-fA-F]{4}$/.test(hex)) fail("a \\u escape needs four hex digits");
    i += 4;
    return parseInt(hex, 16);
  };
  const string = (): string => {
    i += 1;
    let out = "";
    for (;;) {
      const start = i;
      while (i < text.length && text[i] !== '"' && text[i] !== "\\" && text.charCodeAt(i) >= 0x20)
        i += 1;
      out += text.slice(start, i);
      if (i >= text.length) fail("a string is not closed");
      const c = text[i];
      if (c === '"') {
        i += 1;
        return out;
      }
      if (c !== "\\") fail("a control character in a string");
      i += 1;
      if (i >= text.length) fail("a string is not closed");
      const e = text[i]!;
      i += 1;
      const simple: Record<string, string> = {
        '"': '"',
        "\\": "\\",
        "/": "/",
        b: "\b",
        f: "\f",
        n: "\n",
        r: "\r",
        t: "\t",
      };
      if (e in simple) out += simple[e];
      else if (e === "u") {
        const hi = hex4();
        let code = hi;
        if (hi >= 0xd800 && hi < 0xdc00) {
          if (!eat("\\u")) fail("a lone surrogate");
          const lo = hex4();
          if (lo < 0xdc00 || lo >= 0xe000) fail("a lone surrogate");
          code = 0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00);
        } else if (hi >= 0xdc00 && hi < 0xe000) fail("a lone surrogate");
        out += String.fromCodePoint(code);
      } else fail("bad escape");
    }
  };
  const number = (): Json => {
    const start = i;
    eat("-");
    const intStart = i;
    if (digits() === 0) fail("a number needs digits");
    if (text[intStart] === "0" && i - intStart > 1) fail("a number cannot start with 0");
    if (eat(".") && digits() === 0) fail("a fraction needs digits");
    if (text[i] === "e" || text[i] === "E") {
      i += 1;
      if (text[i] === "+" || text[i] === "-") i += 1;
      if (digits() === 0) fail("an exponent needs digits");
    }
    const v = text.slice(start, i);
    // serde_json refuses a number a double cannot hold.
    if (!Number.isFinite(Number(v))) fail("a number out of range");
    return { k: "num", v };
  };
  const value = (depth: number): Json => {
    if (depth > MAX_DEPTH) fail("nested too deep");
    const c = text[i];
    if (c === undefined) return fail("a value is missing");
    if (c === "{") {
      i += 1;
      const entries: [string, Json][] = [];
      ws();
      if (eat("}")) return { k: "obj", v: entries };
      for (;;) {
        ws();
        if (text[i] !== '"') fail("a key must be a string");
        const key = string();
        if (entries.some(([k]) => k === key)) fail(`the key “${key}” is repeated`);
        ws();
        if (!eat(":")) fail("expected ':'");
        ws();
        entries.push([key, value(depth + 1)]);
        ws();
        if (eat(",")) continue;
        if (eat("}")) return { k: "obj", v: entries };
        fail("expected ',' or '}'");
      }
    }
    if (c === "[") {
      i += 1;
      const items: Json[] = [];
      ws();
      if (eat("]")) return { k: "arr", v: items };
      for (;;) {
        ws();
        items.push(value(depth + 1));
        ws();
        if (eat(",")) continue;
        if (eat("]")) return { k: "arr", v: items };
        fail("expected ',' or ']'");
      }
    }
    if (c === '"') return { k: "str", v: string() };
    if (c === "t" && eat("true")) return { k: "bool", v: true };
    if (c === "f" && eat("false")) return { k: "bool", v: false };
    if (c === "n" && eat("null")) return { k: "null" };
    if (c === "-" || (c >= "0" && c <= "9")) return number();
    return fail("unexpected character");
  };
  ws();
  const v = value(0);
  ws();
  if (i < text.length) fail("unexpected text after the JSON value");
  return v;
}

const member = (j: Json, key: string): Json | undefined =>
  j.k === "obj" ? j.v.find(([k]) => k === key)?.[1] : undefined;

// --- PartiQL lexing ---------------------------------------------------------------------------

const WHITESPACE = /^\p{White_Space}$/u;
const isWs = (c: string | undefined): boolean => c !== undefined && WHITESPACE.test(c);
const isAsciiAlpha = (c: string | undefined): boolean => c !== undefined && /^[A-Za-z]$/.test(c);
const isWordChar = (c: string | undefined): boolean => c !== undefined && /^[A-Za-z0-9_]$/.test(c);

/** Line breaks PartiQL keeps inside a `--` comment that other lexers may end it at. */
const OTHER_BREAKS = ["\v", "\f", "\x1c", "\x1d", "\x1e", "\u0085", " ", " "];

type Chars = readonly string[];

/** The comment at `i` as PartiQL reads it: where it ends, and whether a lexer could end it elsewhere. */
function commentAt(c: Chars, i: number): [number, boolean] | null {
  if (c[i] === "-" && c[i + 1] === "-") {
    let j = i + 2;
    let otherBreak = false;
    let unclear = false;
    while (j < c.length && c[j] !== "\n" && c[j] !== "\r") {
      if (otherBreak && !isWs(c[j])) unclear = true;
      otherBreak ||= OTHER_BREAKS.includes(c[j]!);
      j += 1;
    }
    // Text after a lone `\r` before the next `\n`; the look ahead stops at the first text.
    if (!unclear && (otherBreak || c[j] === "\r")) {
      for (let k = j; k < c.length && c[k] !== "\n"; k++)
        if (!isWs(c[k])) {
          unclear = true;
          break;
        }
    }
    return [j, unclear];
  }
  if (c[i] === "/" && c[i + 1] === "*") {
    let j = i + 2;
    let unclear = false;
    for (;;) {
      if (j >= c.length) return [c.length, true];
      if (c[j] === "*" && c[j + 1] === "/") return [j + 2, unclear];
      if (c[j] === "/" && c[j + 1] === "*") unclear = true;
      j += 1;
    }
  }
  return null;
}

/** Where whitespace and comments from `i` end, and whether a comment among them is unclear. */
function trivia(c: Chars, from: number): [number, boolean] {
  let i = from;
  let unclear = false;
  for (;;) {
    while (i < c.length && isWs(c[i])) i += 1;
    const comment = commentAt(c, i);
    if (!comment) return [i, unclear];
    unclear ||= comment[1];
    i = comment[0];
  }
}

const skipTrivia = (c: Chars, i: number): number => trivia(c, i)[0];

/** Where the PartiQL statement from `i` ends (its `;`, or the end), and whether it is unclear. */
function partiqlEnd(c: Chars, i: number): [number, boolean] {
  let unclear = false;
  let j = i;
  while (j < c.length) {
    const ch = c[j]!;
    if (ch === ";") return [j, unclear];
    if (ch === "'" || ch === '"') {
      j += 1;
      for (;;) {
        const d = c[j];
        if (d === undefined) return [c.length, true];
        if (d === ch) {
          if (c[j + 1] === ch) {
            j += 2;
            continue;
          }
          break;
        }
        if (d === "\\" && c[j + 1] === ch) unclear = true;
        j += 1;
      }
    } else if (ch === "`") {
      unclear = true;
      j += 1;
      while (j < c.length && c[j] !== "`") j += 1;
      if (j >= c.length) return [c.length, true];
    } else if (ch === "-" || ch === "/") {
      const comment = commentAt(c, j);
      if (comment) {
        unclear ||= comment[1];
        j = comment[0];
        continue;
      }
    }
    j += 1;
  }
  return [c.length, unclear];
}

/** The action of an API command at `i`: a word followed by `{`, or a known action alone. */
function apiAction(c: Chars, i: number): string | null {
  if (!isAsciiAlpha(c[i])) return null;
  let j = i;
  while (j < c.length && isWordChar(c[j])) j += 1;
  const word = c.slice(i, j).join("");
  let k = j;
  while (k < c.length && isWs(c[k])) k += 1;
  if (c[k] === "{") return word;
  if ((k >= c.length || c[k] === ";") && isKnownAction(word)) return word;
  return null;
}

/** Where the command at `i` ends: an API command's JSON with its escapes, else PartiQL's. */
function commandEnd(c: Chars, i: number): number {
  if (apiAction(c, i) === null) return partiqlEnd(c, i)[0];
  let j = i;
  while (j < c.length) {
    if (c[j] === ";") return j;
    if (c[j] === '"') {
      j += 1;
      while (j < c.length && c[j] !== '"') {
        if (c[j] === "\\") j += 1;
        j += 1;
      }
    }
    j += 1;
  }
  return c.length;
}

/** One command of console text: its text and code point offsets, and its 1-based line. */
export interface ConsoleStatement {
  sql: string;
  start: number;
  end: number;
  line: number;
}

/**
 * The backend's `split_console`: commands split on `;` outside strings, quoted names, comments
 * and an API command's JSON (code point offsets); empty and comment-only ones are skipped.
 */
export function splitConsole(text: string): ConsoleStatement[] {
  const c = Array.from(text);
  const out: ConsoleStatement[] = [];
  let i = 0;
  let counted = 0;
  let line = 1;
  for (;;) {
    i = skipTrivia(c, i);
    while (c[i] === ";") i = skipTrivia(c, i + 1);
    if (i >= c.length) return out;
    const end = commandEnd(c, i);
    let e = Math.min(end, c.length);
    while (e > i && isWs(c[e - 1])) e -= 1;
    if (e > i) {
      for (let k = counted; k < i; k++) if (c[k] === "\n") line += 1;
      counted = i;
      out.push({ sql: c.slice(i, e).join(""), start: i, end: e, line });
    }
    i = end + 1;
  }
}

/** The commands as the editor's ranges (UTF-16 offsets). */
export function consoleRanges(text: string): StatementRange[] {
  const statements = splitConsole(text);
  if (statements.length === 0) return [];
  // Code point offset → UTF-16 offset.
  const utf16: number[] = [0];
  for (const ch of text) utf16.push(utf16[utf16.length - 1]! + ch.length);
  return statements.map((s) => ({ start: utf16[s.start]!, end: utf16[s.end]!, line: s.line }));
}

/** Whether the statement from `i` is clear and only whitespace, comments and `;` follow it. */
function oneStatement(c: Chars, i: number): boolean {
  const [end, unclear] = partiqlEnd(c, i);
  if (unclear) return false;
  let k = end;
  while (k < c.length) {
    const [next, u] = trivia(c, k + 1);
    if (u || (next < c.length && c[next] !== ";")) return false;
    k = next;
  }
  return true;
}

/** A PartiQL statement reads only when it is one `SELECT` that every lexer reads the same way. */
export function partiqlClass(statement: string): StatementClass {
  const c = Array.from(statement);
  const [i, leadUnclear] = trivia(c, 0);
  let j = i;
  while (j < c.length && isAsciiAlpha(c[j])) j += 1;
  const word = c.slice(i, j).join("");
  const ends = j >= c.length || (!isWordChar(c[j]) && c[j] !== "$");
  return word.toUpperCase() === "SELECT" && ends && !leadUnclear && oneStatement(c, i)
    ? "read"
    : "write";
}

/** The `Statement` of an object holding only `members`. */
function statementOf(o: Json, members: readonly string[]): string | null {
  if (o.k !== "obj" || !o.v.every(([k]) => members.includes(k))) return null;
  const s = member(o, "Statement");
  return s?.k === "str" ? s.v : null;
}

/** Whether an API request only reads (the backend's `api_class`). */
export function apiClass(action: string, body: Json): StatementClass {
  const reads = (text: string | null) => text !== null && partiqlClass(text) === "read";
  const list = (members: readonly string[], key: string, itemMembers: readonly string[]) => {
    if (body.k !== "obj" || !body.v.every(([k]) => members.includes(k))) return false;
    const items = member(body, key);
    return (
      items?.k === "arr" &&
      items.v.length > 0 &&
      items.v.every((s) => reads(statementOf(s, itemMembers)))
    );
  };
  let read: boolean;
  if (isReadAction(action)) read = true;
  else if (action === "ExecuteStatement")
    read = reads(statementOf(body, EXECUTE_STATEMENT_MEMBERS));
  else if (action === "BatchExecuteStatement")
    read = list(BATCH_EXECUTE_STATEMENT_MEMBERS, "Statements", BATCH_STATEMENT_MEMBERS);
  else if (action === "ExecuteTransaction")
    read = list(EXECUTE_TRANSACTION_MEMBERS, "TransactStatements", TRANSACT_STATEMENT_MEMBERS);
  else read = false;
  return read ? "read" : "write";
}

/** One console command as it is sent. */
export type ConsoleCommand =
  | { kind: "partiql"; statement: string }
  | { kind: "api"; action: string; body: Json; text: string };

/** Unicode's whitespace at the end, as Rust's `trim_end` takes it. */
const trimEnd = (s: string) => s.replace(/\p{White_Space}+$/u, "");
const trim = (s: string) => trimEnd(s).replace(/^\p{White_Space}+/u, "");

/**
 * One console command (the backend's `parse_command`); throws with the backend's reason when it
 * is not one (an API body that is not one JSON object, an empty or too long statement).
 */
export function parseCommand(text: string): ConsoleCommand {
  const c = Array.from(text);
  const i = skipTrivia(c, 0);
  let rest = trimEnd(c.slice(i).join(""));
  rest = trimEnd(rest.replace(/;+$/, ""));
  const action = apiAction(c, i);
  if (action !== null) {
    const raw = trim(rest.slice(action.length));
    const json = raw === "" ? "{}" : raw;
    const body = parseJson(json);
    if (body.k !== "obj") throw new Error(`${action}: the request is a JSON object`);
    return { kind: "api", action, body, text: json };
  }
  if (rest === "") throw new Error("an empty statement");
  if (Array.from(rest).length > MAX_STATEMENT_CHARS)
    throw new Error(
      `DynamoDB runs PartiQL statements of at most ${MAX_STATEMENT_CHARS} characters`,
    );
  return { kind: "partiql", statement: rest };
}

/** Whether console text only reads: every command it holds must (one that does not parse writes). */
export function classifyConsole(text: string): StatementClass {
  const commands = splitConsole(text);
  if (commands.length === 0) return "write";
  const read = commands.every((s) => {
    try {
      const cmd = parseCommand(s.sql);
      return (
        (cmd.kind === "partiql" ? partiqlClass(cmd.statement) : apiClass(cmd.action, cmd.body)) ===
        "read"
      );
    } catch {
      return false;
    }
  });
  return read ? "read" : "write";
}
