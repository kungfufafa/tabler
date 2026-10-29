import type { StatementClass } from "@/features/safety/classify";
import type { ClassifyRules } from "@/lib/ipc/bindings/ClassifyRules";
import type { DialectLex } from "@/lib/ipc/bindings/DialectLex";
import type { LexProfile } from "@/lib/ipc/bindings/LexProfile";
import {
  firstReading,
  identName,
  isIdentStart,
  isPunct,
  listed,
  PLAIN,
  scan,
  statements,
  text,
  upper,
  type Scan,
  type Token,
  type TokenRange,
} from "./scanner";

/**
 * Safe Mode's classifier of the extension dialects: a port of the backend's
 * `sqlgen/portable/classify.rs` over the dialect's `LexProfile` and `ClassifyRules`
 * (`src/lib/ipc/bindings/lex/<d>.json`), rule for rule. It is an allow-list: a statement is a READ
 * only when its first keyword is a query or an inspection of the rules and nothing in it writes;
 * empty and unknown statements are WRITEs. On top of the statement kind, a query holding a word of
 * `writes` (anywhere, with an optional terminator), a call of a `writeCalls` function (also in
 * attribute notation where the profile has `attributeCalls`), a table function outside
 * `tableFunctions`, a keyword a number may hide (`1DELETE`), or a text a server may read otherwise
 * (`Scan.ambiguous`) makes it a write; the text is read under every `Reading` of the dialect. The
 * backend's module comment has the reasons of each rule; the shared cases (`cases.ts`) and every
 * dialect's generated fixture run on both sides.
 */

/**
 * A token of code: a word (upper case), a quoted name (quotes removed), a name whose spelling the
 * scanner cannot know (`U&"…"`, or a quoted name holding a backslash where quoted names take
 * escapes), a string or a punctuation character.
 */
type Code =
  | { t: "word"; w: string }
  | { t: "name"; n: string }
  | { t: "anyName" }
  | { t: "str" }
  | { t: "punct"; c: string };

const is = (code: Code | undefined, c: string): boolean => code?.t === "punct" && code.c === c;
const wordOf = (code: Code | undefined): string | null => (code?.t === "word" ? code.w : null);
/** Whether a call could name it: a word or a quoted name. */
const isName = (code: Code | undefined): boolean =>
  code?.t === "word" || code?.t === "name" || code?.t === "anyName";
const startsWithDigit = (w: string): boolean => w[0] !== undefined && w[0] >= "0" && w[0] <= "9";

/** A part of a qualified name: a name token's index, or nothing between two dots (`db..p`). */
type Part = number | null;

/**
 * The parts of the qualified name that ends at the name token `i`, first to last: the names
 * before it with a run of dots between each two (every dot past the first is an empty part).
 */
function qualifiedParts(code: readonly Code[], i: number): Part[] {
  const parts: Part[] = [i];
  let q = i;
  for (;;) {
    let d = q;
    while (d > 0 && is(code[d - 1], ".")) d--;
    if (d === q || d === 0 || !isName(code[d - 1])) break;
    for (let k = 0; k < q - d - 1; k++) parts.push(null);
    parts.push(d - 1);
    q = d - 1;
  }
  return parts.reverse();
}

/** Whether the word at `i` may be read as part of a number: it starts with a digit, or it follows
 * one and a `.` (`1.e5DELETE` is `1`, `.`, `E5DELETE` to the scanner). */
function inNumber(code: readonly Code[], i: number): boolean {
  const digits = (c: Code | undefined) => {
    const w = wordOf(c);
    return w !== null && startsWithDigit(w);
  };
  return digits(code[i]) || (i >= 2 && is(code[i - 1], ".") && digits(code[i - 2]));
}

/** The suffixes of `w` after its first character that start like a name, when `number`. */
function suffixes(w: string, number: boolean): string[] {
  if (!number) return [];
  const chars = Array.from(w);
  const out: string[] = [];
  for (let k = 1; k < chars.length; k++)
    if (isIdentStart(chars[k])) out.push(chars.slice(k).join(""));
  return out;
}

/** The words the word at `i` may hide from the scanner where it may be part of a number:
 * every suffix after its first character that starts like a name (`1e0DELETE`, `0xINTO`). */
function hiddenWords(code: readonly Code[], i: number): string[] {
  return suffixes(wordOf(code[i]) ?? "", inNumber(code, i));
}

const isAsciiText = (s: string): boolean => [...s].every((c) => c.charCodeAt(0) < 0x80);

/**
 * The word at `i` and the words it may hide, for the deny-lists; a word with non-ASCII letters
 * also counts upper-cased by Unicode rules (`ſystem$x` as `SYSTEM$X`).
 */
function spellings(code: readonly Code[], i: number): string[] {
  const w = wordOf(code[i]);
  if (w === null) return [];
  const out = [w, ...hiddenWords(code, i)];
  if (!isAsciiText(w)) {
    const folded = w.toUpperCase();
    out.push(...suffixes(folded, inNumber(code, i)), folded);
  }
  return out;
}

/**
 * Whether the name ending at the token `i` names an entry of a deny-list, in any spelling: an
 * entry of several parts matches the last parts of a qualified name, a one-part entry its last
 * part. A quoted name counts upper-cased and split at its dots; a name whose spelling cannot be
 * known is on every list.
 */
function namesListed(code: readonly Code[], i: number, list: readonly string[]): boolean {
  if (list.length === 0 || !isName(code[i])) return false;
  const slots: string[][] = [];
  for (const p of qualifiedParts(code, i)) {
    if (p === null) {
      slots.push([""]);
      continue;
    }
    const c = code[p]!;
    if (c.t === "anyName") return true;
    if (c.t === "name") for (const s of c.n.toUpperCase().split(".")) slots.push([s]);
    else slots.push(spellings(code, p));
  }
  return list.some((entry) => {
    const want = entry.split(".");
    if (want.length > slots.length) return false;
    const tail = slots.slice(slots.length - want.length);
    return tail.every((slot, k) => slot.some((s) => listed([want[k]!], s)));
  });
}

/** Words of the grammar that read like a call where a table goes: quoted, a user's object. */
const CLAUSE_WORDS = ["TABLE", "VALUES", "SEMANTIC_VIEW"];

/** Whether the name token `code` is the part `entry` of an allow-list entry: a word on its ASCII
 * upper case, a quoted name on its exact text, never a quoted clause word. */
function partAllowed(code: Code, entry: string): boolean {
  if (code.t === "word") return listed([entry], code.w);
  if (code.t === "name") return !CLAUSE_WORDS.includes(entry) && listed([entry], code.n);
  return false;
}

/**
 * Whether the table function named by `parts` (a method after `!` when `method`) is on the
 * allow-list: a one-part entry matches an unqualified call only, an entry of several parts the
 * call's last parts, a `!METHOD` entry a method of that name.
 */
function tableFunctionAllowed(
  code: readonly Code[],
  parts: readonly Part[],
  method: boolean,
  allowed: readonly string[],
): boolean {
  const token = (p: Part | undefined) => (p === null || p === undefined ? null : code[p]!);
  if (parts.some((p) => token(p)?.t === "anyName")) return false;
  return allowed.some((entry) => {
    if (entry.startsWith("!")) {
      const last = token(parts[parts.length - 1]);
      return method && last !== null && partAllowed(last, entry.slice(1));
    }
    if (method) return false;
    const want = entry.split(".");
    const fits = want.length === 1 ? parts.length === 1 : want.length <= parts.length;
    if (!fits) return false;
    return parts.slice(parts.length - want.length).every((p, k) => {
      const c = token(p);
      return c !== null && partAllowed(c, want[k]!);
    });
  });
}

/** Functions whose arguments may hold a `FROM` (`EXTRACT(YEAR FROM ts)`). */
const FROM_IN_ARGUMENTS = ["EXTRACT", "SUBSTRING", "SUBSTR", "TRIM", "OVERLAY", "POSITION"];
/** What may follow a set operator that ends a `FROM` list, besides `(`. */
const AFTER_SET_OPERATOR = ["ALL", "DISTINCT", "SELECT", "VALUES", "TABLE", "FROM", "WITH", "BY"];
/** Words a name or an operand follows, besides `TABLE_WORDS` and `TABLE_PREFIXES`. */
const NAME_AFTER = [
  "AS",
  "FROM",
  "USING",
  "ON",
  "AND",
  "OR",
  "NOT",
  "IS",
  "IN",
  "LIKE",
  "ILIKE",
  "REGEXP",
  "BETWEEN",
  "CASE",
  "WHEN",
  "THEN",
  "ELSE",
  "INTERVAL",
  "DIV",
  "MOD",
  "COLLATE",
];
/** The options of EXPLAIN that may stand before the statement it explains. */
const EXPLAIN_OPTIONS = [
  "ANALYZE",
  "ANALYSE",
  "VERBOSE",
  "EXTENDED",
  "COSTS",
  "BUFFERS",
  "TIMING",
  "SUMMARY",
  "SETTINGS",
  "FORMAT",
  "TEXT",
  "JSON",
  "XML",
  "YAML",
  "TREE",
  "USING",
  "TABULAR",
];
/** Words between a table position and the table itself. */
const TABLE_PREFIXES = ["LATERAL", "ONLY"];
/** Words a table follows, besides `FROM` (with DuckDB's spellings of PIVOT and UNPIVOT). */
const TABLE_WORDS = [
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
/** Words after which a query keyword goes on the same statement (`UNION ALL SELECT`). */
const SET_OPERATORS = ["UNION", "EXCEPT", "INTERSECT", "MINUS", "ALL", "DISTINCT"];

/** The readings of the dialect; with none, nothing is a read (`with`), and splitting uses PLAIN. */
const readingsOf = (lex: LexProfile) => lex.readings;

/** Classifies one statement or several (a READ only when each one is). */
export function classifyWith(lex: LexProfile, rules: ClassifyRules, sql: string): StatementClass {
  const readings = readingsOf(lex);
  const reads =
    readings.length > 0 &&
    readings.every((r) => {
      const s = scan(lex, r, sql);
      return !s.ambiguous && readsUnder(lex, rules, s);
    });
  return reads ? "read" : "write";
}

/** `classifyWith` over a dialect's exported lexer profile and rules. */
export function classifyDialect(dialect: DialectLex, sql: string): StatementClass {
  return classifyWith(dialect.lex, dialect.classify, sql);
}

/** `code` cut at its `;`s, empty parts left out. */
function bySemicolon(code: readonly Code[]): Code[][] {
  const parts: Code[][] = [[]];
  for (const c of code) {
    if (is(c, ";")) parts.push([]);
    else parts[parts.length - 1]!.push(c);
  }
  return parts.filter((p) => p.length > 0);
}

function readsUnder(lex: LexProfile, rules: ClassifyRules, s: Scan): boolean {
  const ranges = statements(lex, s);
  return (
    ranges.length > 0 &&
    ranges.every((r) => {
      // A batch may hold several `;`-separated statements: each one must read, and so must each
      // one the flow operator chains on.
      const parts = bySemicolon(codeOf(s, r));
      return (
        parts.length > 0 &&
        parts.every((p) => reads(lex, rules, p) && chained(p).every((c) => reads(lex, rules, c)))
      );
    })
  );
}

/**
 * The statements Snowflake's flow operator `->>` chains after the first one: the code from each
 * `->>` that a word follows to the next one.
 */
function chained(code: readonly Code[]): Code[][] {
  const starts: number[] = [];
  for (let i = 3; i < code.length; i++) {
    const w = wordOf(code[i]);
    if (
      is(code[i - 3], "-") &&
      is(code[i - 2], ">") &&
      is(code[i - 1], ">") &&
      w !== null &&
      !startsWithDigit(w)
    )
      starts.push(i);
  }
  return starts.map((s, k) =>
    code.slice(s, k + 1 < starts.length ? starts[k + 1]! - 3 : code.length),
  );
}

function codeOf(s: Scan, r: TokenRange): Code[] {
  const toks = s.tokens.slice(r.start, r.end);
  const out: Code[] = [];
  let i = 0;
  while (i < toks.length) {
    const next = unicodeName(s, toks, i);
    if (next !== null) {
      out.push({ t: "anyName" });
      i = next;
      continue;
    }
    const t = toks[i]!;
    switch (t.kind) {
      case "word":
        out.push({ t: "word", w: upper(s, t) });
        break;
      case "ident":
        // Where quoted names take a string's escapes, one holding a backslash may spell any name.
        out.push(
          s.reading.identBackslash && s.chars.slice(t.start, t.end).includes("\\")
            ? { t: "anyName" }
            : { t: "name", n: identName(s, t) },
        );
        break;
      case "str":
        out.push({ t: "str" });
        break;
      case "punct":
        out.push({ t: "punct", c: s.chars[t.start]! });
        break;
      default:
        break;
    }
    i++;
  }
  return out;
}

/**
 * When `toks[i..]` starts with a Unicode-escaped name (`U&"…"`, glued, then an optional
 * `UESCAPE 'c'`), the index after it: its escapes may spell any name.
 */
function unicodeName(s: Scan, toks: readonly Token[], i: number): number | null {
  const [u, amp, name] = [toks[i], toks[i + 1], toks[i + 2]];
  if (!u || !amp || !name) return null;
  const glued = u.end === amp.start && amp.end === name.start;
  if (!(
    u.kind === "word" &&
    upper(s, u) === "U" &&
    isPunct(s, amp, "&") &&
    name.kind === "ident" &&
    glued
  ))
    return null;
  // `UESCAPE 'c'` after it, past any space or comment.
  const codeFrom = (from: number) => {
    for (let j = from; j < toks.length; j++)
      if (toks[j]!.kind !== "space" && toks[j]!.kind !== "comment") return j;
    return null;
  };
  const e = codeFrom(i + 3);
  if (e !== null && toks[e]!.kind === "word" && upper(s, toks[e]!) === "UESCAPE") {
    const lit = codeFrom(e + 1);
    if (lit !== null && toks[lit]!.kind === "str") return lit + 1;
  }
  return i + 3;
}

/** Whether `code` holds a word of `words`, or a punctuation character one of them is
 * (Snowflake's `@`). */
function hasWord(code: readonly Code[], words: readonly string[]): boolean {
  for (let i = 0; i < code.length; i++) {
    const c = code[i]!;
    if (c.t === "punct") {
      if (words.includes(c.c)) return true;
    } else if (spellings(code, i).some((s) => listed(words, s))) return true;
  }
  return false;
}

function reads(lex: LexProfile, rules: ClassifyRules, code: readonly Code[]): boolean {
  return (
    !(lex.terminatorOptional && hasWord(code, rules.writes)) &&
    !calls(code, rules.writeCalls, lex.attributeCalls ?? false) &&
    tablesReadOnly(rules, code) &&
    statementReads(rules, code)
  );
}

function statementReads(rules: ClassifyRules, code: readonly Code[]): boolean {
  let start = 0;
  while (is(code[start], "(")) start++;
  const first = wordOf(code[start]);
  if (first === null) return false;
  const rest = code.slice(start + 1);
  if (listed(rules.queries, first)) return !hasWord(rest, rules.writes) && !locksRows(rest);
  if (first === "EXPLAIN" && listed(rules.inspections, first))
    // With ANALYZE it runs the statement it explains, which must then be a query.
    return !explainAnalyzes(rest) || explainedQueryReads(rules, rest);
  return listed(rules.inspections, first);
}

/** Whether the EXPLAIN whose options are `rest` runs its statement: ANALYZE among them, or a
 * quoted name or a string in its option list, which may spell ANALYZE. */
function explainAnalyzes(rest: readonly Code[]): boolean {
  if (hasWord(rest, ["ANALYZE", "ANALYSE"])) return true;
  if (!is(rest[0], "(")) return false;
  let depth = 0;
  for (const c of rest) {
    if (is(c, "(")) depth++;
    else if (is(c, ")")) {
      depth--;
      if (depth === 0) return false;
    } else if (c.t === "name" || c.t === "anyName" || c.t === "str") return true;
  }
  return false;
}

/** Whether a query locks the rows it reads (`FOR UPDATE`, `FOR NO KEY UPDATE`, `FOR SHARE`,
 * `FOR KEY SHARE`), which a read-only transaction refuses like a write. */
function locksRows(code: readonly Code[]): boolean {
  const word = (i: number) => wordOf(code[i]);
  for (let i = 0; i < code.length; i++) {
    if (word(i) !== "FOR") continue;
    const next = word(i + 1);
    if (next === "UPDATE" || next === "SHARE") return true;
    if (next === "KEY" && word(i + 2) === "SHARE") return true;
    if (next === "NO" && word(i + 2) === "KEY" && word(i + 3) === "UPDATE") return true;
  }
  return false;
}

/** Whether the statement an `EXPLAIN ANALYZE` runs is a query that reads: the first word after
 * the known options must start a query. */
function explainedQueryReads(rules: ClassifyRules, rest: readonly Code[]): boolean {
  let i = 0;
  for (;;) {
    const c = rest[i];
    if (is(c, "(")) {
      // A parenthesized option list: `(ANALYZE, BUFFERS)`.
      let depth = 0;
      while (i < rest.length) {
        if (is(rest[i], "(")) depth++;
        else if (is(rest[i], ")")) {
          depth--;
          if (depth === 0) break;
        }
        i++;
      }
      i++;
    } else if (is(c, "=")) i++;
    else if (c?.t === "word" && EXPLAIN_OPTIONS.includes(c.w)) i++;
    else break;
  }
  const w = wordOf(rest[i]);
  return w !== null && listed(rules.queries, w) && statementReads(rules, rest.slice(i));
}

/** Whether `code` calls a function of `names`: a name before `(`, and with `attributes` a name
 * in attribute notation too. */
function calls(code: readonly Code[], names: readonly string[], attributes: boolean): boolean {
  for (let i = 0; i < code.length; i++) {
    const called = is(code[i + 1], "(") || (attributes && inAttributeNotation(code, i));
    if (called && namesListed(code, i, names)) return true;
  }
  return false;
}

/** Whether the name at `i` is selected from a value: `(expr).name`, `arr[1].name`, or a later
 * name of such a chain. The PostgreSQL family calls `name(value)` for it. */
function inAttributeNotation(code: readonly Code[], i: number): boolean {
  let k = i;
  while (k >= 2 && isName(code[k]) && is(code[k - 1], ".")) {
    if (is(code[k - 2], ")") || is(code[k - 2], "]")) return true;
    k -= 2;
  }
  return false;
}

/** Whether every table function `code` reads from is allowed. */
function tablesReadOnly(rules: ClassifyRules, code: readonly Code[]): boolean {
  const allowed = rules.tableFunctions;
  if (allowed === null) return true;
  return tablePositions(code).every((at) => {
    // The first table of a parenthesized join goes here too (`FROM (url(…) CROSS JOIN t)`).
    let p = at;
    let nested = false;
    for (;;) {
      const c = code[p];
      if (is(c, "(")) nested = true;
      else if (!(c?.t === "word" && TABLE_PREFIXES.includes(c.w))) break;
      p++;
    }
    const c = code[p];
    if (c?.t === "word" && nested && listed(rules.queries, c.w)) return true;
    if (!isName(c)) return true;
    // A qualified name: the function is its last part, past every run of dots (`db..p(`) and a
    // method after `!` (Snowflake's `m!PREDICT(`).
    let q = p;
    let method = false;
    for (;;) {
      let d = q + 1;
      while (is(code[d], ".")) d++;
      if (d > q + 1 && isName(code[d])) q = d;
      else if (d === q + 1 && is(code[d], "!") && isName(code[d + 1])) {
        q = d + 1;
        method = true;
        break;
      } else break;
    }
    const called = is(code[q + 1], "(");
    return !called || tableFunctionAllowed(code, qualifiedParts(code, q), method, allowed);
  });
}

/** Whether the `FROM` at `i` ends the comparison `IS [NOT] DISTINCT FROM`, which no table follows. */
function comparesDistinct(code: readonly Code[], i: number): boolean {
  const word = (back: number) => (i >= back ? wordOf(code[i - back]) : null);
  return word(1) === "DISTINCT" && (word(2) === "IS" || (word(2) === "NOT" && word(3) === "IS"));
}

/**
 * Indexes of the tokens where a table goes: after `FROM` (but `IS DISTINCT FROM`), `JOIN`,
 * `TABLE`, `DESCRIBE` / `DESC` / `SUMMARIZE` / `SHOW` and `PIVOT` / `UNPIVOT`, after a word that
 * may hide one of them, and after a comma of a `FROM` list until a clause ends it.
 */
function tablePositions(code: readonly Code[]): number[] {
  const out: number[] = [];
  // Per open parenthesis: whether it holds a function's arguments that may say FROM.
  const parens: boolean[] = [];
  // The parenthesis depths of the open FROM lists, innermost last.
  const froms: number[] = [];
  code.forEach((c, i) => {
    const prev = i > 0 ? wordOf(code[i - 1]) : null;
    const inFrom = froms.length > 0 && froms[froms.length - 1] === parens.length;
    if (is(c, "(")) parens.push(prev !== null && FROM_IN_ARGUMENTS.includes(prev));
    else if (is(c, ")")) {
      parens.pop();
      while (froms.length > 0 && froms[froms.length - 1]! > parens.length) froms.pop();
    } else if (is(c, ",")) {
      if (inFrom) out.push(i + 1);
    } else if (c.t === "word") {
      const w = c.w;
      // After a number, a hidden word has the number before it (`SELECT 1e0FROM t`).
      const hides = (words: readonly string[]) =>
        hiddenWords(code, i).some((h) => words.includes(h));
      if ((w === "FROM" && !comparesDistinct(code, i)) || hides(["FROM"])) {
        const inArguments = parens[parens.length - 1] ?? false;
        if (!inArguments) {
          // A second FROM at the same depth (T-SQL `DELETE FROM t FROM …`) continues the list.
          if (!inFrom) froms.push(parens.length);
          out.push(i + 1);
        }
      } else if (
        TABLE_WORDS.includes(w) ||
        hides(TABLE_WORDS) ||
        (w === "FUNCTION" && prev === "TABLE")
      ) {
        out.push(i + 1);
      } else if (inFrom && endsFromList(code, i, w)) {
        froms.pop();
      }
    }
  });
  return out;
}

/**
 * Whether the word `w` at `i` ends the `FROM` list it stands in: only the clauses whose own comma
 * lists may hold calls, where a clause can start and followed by what the clause takes.
 */
function endsFromList(code: readonly Code[], i: number, w: string): boolean {
  const next = code[i + 1];
  const nextWord = wordOf(next);
  const paren = is(next, "(");
  const value = (c: Code | undefined) =>
    (c?.t === "word" && startsWithDigit(c.w)) || (c?.t === "punct" && "?$:@{".includes(c.c));
  const query = (c: Code | undefined) => {
    const cw = wordOf(c);
    return (
      is(c, "(") || (cw !== null && ["SELECT", "WITH", "VALUES", "TABLE", "FROM"].includes(cw))
    );
  };
  const afterSetOperator = nextWord !== null && AFTER_SET_OPERATOR.includes(nextWord);
  let takesNext = false;
  if (w === "GROUP" || w === "ORDER") takesNext = nextWord === "BY";
  else if (w === "UNION" || w === "INTERSECT") takesNext = paren || afterSetOperator;
  else if (w === "EXCEPT") takesNext = afterSetOperator || (paren && query(code[i + 2]));
  else if (w === "LIMIT")
    takesNext = value(next) || (paren && (value(code[i + 2]) || query(code[i + 2])));
  const prev = i > 0 ? code[i - 1] : undefined;
  let clauseCanStart = true;
  if (prev?.t === "punct") clauseCanStart = prev.c === ")";
  else if (prev?.t === "word")
    clauseCanStart = !(
      NAME_AFTER.includes(prev.w) ||
      TABLE_WORDS.includes(prev.w) ||
      TABLE_PREFIXES.includes(prev.w)
    );
  return takesNext && clauseCanStart;
}

/**
 * Whether `sql` calls a function of `names` (upper case, `PREFIX*` allowed) under any reading:
 * the Metrics Board's check of a dialect's side-effect functions. An entry of several words is
 * those words in a row; a text a server may read otherwise counts as a call.
 */
export function callsListedFunction(
  lex: LexProfile,
  names: readonly string[],
  sql: string,
): boolean {
  return (
    names.length > 0 &&
    readingsOf(lex).some((r) => {
      const s = scan(lex, r, sql);
      const code = codeOf(s, { start: 0, end: s.tokens.length });
      return (
        s.ambiguous ||
        calls(code, names, lex.attributeCalls ?? false) ||
        names.some((n) => says(code, n))
      );
    })
  );
}

/** Whether `code` holds the words of `phrase` (several, upper case) in a row. */
function says(code: readonly Code[], phrase: string): boolean {
  const words = phrase.split(" ");
  if (words.length < 2) return false;
  return code.some(
    (_, i) =>
      spellings(code, i).some((s) => s === words[0]) &&
      words.slice(1).every((w, k) => wordOf(code[i + 1 + k]) === w),
  );
}

/**
 * Whether the statement `r` of `s` is one statement for the server: no `;` between two
 * statements, no flow operator chaining one, and where terminators are optional no word that may
 * start a second one.
 */
function oneStatement(lex: LexProfile, rules: ClassifyRules, s: Scan, r: TokenRange): boolean {
  const parts = bySemicolon(codeOf(s, r));
  if (parts.length > 1) return false;
  const part = parts[0];
  if (!part) return true;
  return chained(part).length === 0 && !(lex.terminatorOptional && startsAnother(rules, part));
}

/**
 * Whether a word after the first one of `code` may start a second statement where none needs a
 * terminator: a word of `writes` anywhere, or a query or inspection keyword outside parentheses
 * (but after a set operator, as a leading `WITH`'s query, or a `WITH` that defines no table).
 */
function startsAnother(rules: ClassifyRules, code: readonly Code[]): boolean {
  let depth = 0;
  let cteQuery = wordOf(code[0]) === "WITH";
  const starts = (s: string) => listed(rules.queries, s) || listed(rules.inspections, s);
  for (let i = 0; i < code.length; i++) {
    const c = code[i]!;
    if (is(c, "(")) depth++;
    else if (is(c, ")")) depth = Math.max(0, depth - 1);
    else if (c.t === "word" && i > 0) {
      const w = c.w;
      if (listed(rules.writes, w)) return true;
      // A keyword hidden after a number (`SELECT 1SELECT 2`) follows the number.
      if (hiddenWords(code, i).some((h) => listed(rules.writes, h) || (depth === 0 && starts(h))))
        return true;
      if (depth > 0 || !starts(w)) continue;
      const prev = wordOf(code[i - 1]);
      if (prev !== null && SET_OPERATORS.includes(prev)) continue;
      if (w === "WITH" && !definesTable(code.slice(i + 1))) continue;
      if (cteQuery) {
        cteQuery = false;
        continue;
      }
      return true;
    }
  }
  return false;
}

/** Whether the code after a `WITH` defines a table: `name AS (` or `name (columns) AS (`. */
function definesTable(rest: readonly Code[]): boolean {
  return isName(rest[0]) && (is(rest[1], "(") || wordOf(rest[1]) === "AS");
}

/**
 * Whether `sql` holds at most one statement under every reading of the dialect (EXPLAIN wraps
 * one, only a single read is cancelled): an unsure answer is no.
 */
export function isSingleStatement(lex: LexProfile, rules: ClassifyRules, sql: string): boolean {
  const readings = lex.readings.length > 0 ? lex.readings : [PLAIN];
  return readings.every((r) => {
    const s = scan(lex, r, sql);
    if (s.ambiguous) return false;
    const ranges = statements(lex, s);
    if (ranges.length === 0) return true;
    return ranges.length === 1 && oneStatement(lex, rules, s, ranges[0]!);
  });
}

/** `sql` with strings, quoted names and comments replaced by a space, so keyword checks see only
 * code (first reading). */
export function codeOnly(lex: LexProfile, sql: string): string {
  const s = scan(lex, firstReading(lex), sql);
  return s.tokens
    .map((t) => (t.kind === "str" || t.kind === "ident" || t.kind === "comment" ? " " : text(s, t)))
    .join("");
}

const LEX_FILES = import.meta.glob<DialectLex>("../../../lib/ipc/bindings/lex/*.json", {
  import: "default",
});

/**
 * The lexer profile and classifier rules of a dialect (`bindings/lex/<name>.json`, written by the
 * backend's tests), loaded on first use; null for a dialect without one (the core dialects).
 */
export async function loadDialectLex(name: string): Promise<DialectLex | null> {
  const load = LEX_FILES[`../../../lib/ipc/bindings/lex/${name}.json`];
  return load ? load() : null;
}
