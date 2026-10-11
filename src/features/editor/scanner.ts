/**
 * Client-side statement scanner, a port of the backend splitter (`src-tauri/src/sqlgen/split.rs`)
 * so "Run current statement" and the current-statement highlight agree with what `run_sql`
 * executes.
 *
 * One pass over the text with these states: normal text, `'…'` strings (`''` escapes; backslash
 * escapes in MySQL and in PostgreSQL `E'…'`), `"…"` (identifiers; strings with backslash escapes in
 * MySQL), `` `…` `` (MySQL / SQLite), `[…]` (SQLite), `--` line comments (MySQL: only when followed
 * by whitespace), `#` line comments (MySQL), `/* … *\/` block comments (nested in PostgreSQL),
 * `$tag$ … $tag$` dollar quoting (PostgreSQL), a `BEGIN … END` depth counter inside
 * `CREATE TRIGGER / FUNCTION / PROCEDURE / EVENT` bodies, the MySQL `DELIMITER xx` directive, psql
 * meta-command lines, and (PostgreSQL) parenthesis depth.
 *
 * Offsets are UTF-16 code units (what CodeMirror uses). Spans are trimmed, exclude the terminator
 * and leading comments; empty and comment-only statements are skipped.
 */

export type SqlDialect = "postgres" | "mysql" | "sqlite";

export interface StatementRange {
  /** First character of the statement (leading whitespace and comments excluded). */
  start: number;
  /** End of the trimmed statement, terminator excluded. */
  end: number;
  /** 1-based line `start` is on. */
  line: number;
}

const IDENT_START = /[\p{L}_]/u;
const WORD_CHAR = /[\p{L}\p{N}_$]/u;
const ALNUM = /[\p{L}\p{N}_]/u;
const WS = /\s/;
const HEAD_MAX = 16;

const isIdentStart = (c: string | undefined): boolean => c !== undefined && IDENT_START.test(c);
const isWordChar = (c: string | undefined): boolean => c !== undefined && WORD_CHAR.test(c);
const isWs = (c: string | undefined): boolean => c !== undefined && WS.test(c);

class Splitter {
  readonly out: StatementRange[] = [];
  private contentStart: number | null = null;
  private delimiter = ";";
  private head: string[] = [];
  private routine: boolean | null = null;
  private depth = 0;
  private parens = 0;
  private linePos = 0;
  private lineCount = 0;
  private readonly n: number;

  constructor(
    private readonly s: string,
    private readonly dialect: SqlDialect,
  ) {
    this.n = s.length;
  }

  private get mysql(): boolean {
    return this.dialect === "mysql";
  }

  private get pg(): boolean {
    return this.dialect === "postgres";
  }

  private defaultDelimiter(): boolean {
    return this.delimiter === ";";
  }

  private mark(i: number): void {
    if (this.contentStart === null) this.contentStart = i;
  }

  private atDelimiter(i: number): boolean {
    if (!this.s.startsWith(this.delimiter, i)) return false;
    if (this.pg && this.parens > 0) return false;
    return !(this.defaultDelimiter() && this.routine === true && this.depth > 0);
  }

  private lineOf(pos: number): number {
    if (pos < this.linePos) {
      this.linePos = 0;
      this.lineCount = 0;
    }
    for (let i = this.linePos; i < pos; i++) if (this.s.charCodeAt(i) === 10) this.lineCount++;
    this.linePos = pos;
    return this.lineCount + 1;
  }

  private finish(to: number): void {
    const from = this.contentStart;
    if (from !== null) {
      let end = to;
      while (end > from && isWs(this.s[end - 1])) end--;
      if (end > from) this.out.push({ start: from, end, line: this.lineOf(from) });
    }
    this.contentStart = null;
    this.head = [];
    this.routine = null;
    this.depth = 0;
    this.parens = 0;
  }

  run(): void {
    const s = this.s;
    let i = 0;
    while (i < this.n) {
      if (this.atDelimiter(i)) {
        this.finish(i);
        i += this.delimiter.length;
        continue;
      }
      const c = s[i]!;
      const next = s[i + 1];
      if (c === "'") {
        this.mark(i);
        i = this.skipQuoted(i, "'", this.mysql);
      } else if (c === '"') {
        this.mark(i);
        i = this.skipQuoted(i, '"', this.mysql);
      } else if (c === "`" && !this.pg) {
        this.mark(i);
        i = this.skipQuoted(i, "`", false);
      } else if (c === "[" && this.dialect === "sqlite") {
        this.mark(i);
        i = this.skipUntil(i + 1, "]");
      } else if (c === "-" && next === "-" && this.dashCommentOk(i)) {
        i = this.skipLine(i);
      } else if (c === "#" && this.mysql) {
        i = this.skipLine(i);
      } else if (c === "/" && next === "*") {
        // MySQL executable comments (`/*! … */`) and optimizer hints are code.
        if (this.mysql && (s[i + 2] === "!" || s[i + 2] === "+")) this.mark(i);
        i = this.skipBlockComment(i);
      } else if (c === "$" && this.pg) {
        const tag = this.dollarTag(i);
        this.mark(i);
        i = tag === null ? i + 1 : this.skipDollar(i, tag);
      } else if (c === "\\" && this.pg && this.atLineStart(i)) {
        i = this.metaCommand(i);
      } else if (isIdentStart(c)) {
        i = this.word(i);
      } else if (isWs(c)) {
        i++;
      } else if (c === "(") {
        this.mark(i);
        if (this.pg) this.parens++;
        if (this.head.length < HEAD_MAX) this.head.push("(");
        this.decideRoutine();
        i++;
      } else if (c === ")") {
        this.mark(i);
        this.parens = Math.max(0, this.parens - 1);
        i++;
      } else {
        this.mark(i);
        i++;
      }
    }
    this.finish(this.n);
  }

  /** MySQL only treats `--` as a comment when whitespace or a control character follows. */
  private dashCommentOk(i: number): boolean {
    if (!this.mysql) return true;
    const c = this.s[i + 2];
    if (c === undefined) return true;
    const code = c.charCodeAt(0);
    return isWs(c) || code < 32 || (code >= 127 && code < 160);
  }

  private skipLine(i: number): number {
    const nl = this.s.indexOf("\n", i);
    return nl < 0 ? this.n : nl;
  }

  private skipUntil(i: number, close: string): number {
    const at = this.s.indexOf(close, i);
    return at < 0 ? this.n : at + 1;
  }

  private skipQuoted(i: number, quote: string, backslash: boolean): number {
    const s = this.s;
    let j = i + 1;
    while (j < this.n) {
      const c = s[j];
      if (backslash && c === "\\") {
        j += 2;
        continue;
      }
      if (c === quote) {
        if (s[j + 1] === quote) {
          j += 2;
          continue;
        }
        return j + 1;
      }
      j++;
    }
    return this.n;
  }

  private skipBlockComment(i: number): number {
    const s = this.s;
    const nested = this.pg;
    let depth = 1;
    let j = i + 2;
    while (j < this.n) {
      const c = s[j];
      const next = s[j + 1];
      if (nested && c === "/" && next === "*") {
        depth++;
        j += 2;
      } else if (c === "*" && next === "/") {
        depth--;
        j += 2;
        if (depth === 0) return j;
      } else {
        j++;
      }
    }
    return this.n;
  }

  /** Length of a `$tag$` opener at `i` (both `$` included), or null. */
  private dollarTag(i: number): number | null {
    const s = this.s;
    // `$` inside an identifier (`a$b`) or a positional parameter (`$1`) is not a quote.
    if (i > 0 && isWordChar(s[i - 1])) return null;
    let j = i + 1;
    const first = s[j];
    if (first === "$") return 2;
    if (!isIdentStart(first)) return null;
    j++;
    while (j < this.n) {
      const c = s[j]!;
      if (c === "$") return j + 1 - i;
      if (!ALNUM.test(c)) return null;
      j++;
    }
    return null;
  }

  private skipDollar(i: number, tagLen: number): number {
    const tag = this.s.slice(i, i + tagLen);
    const at = this.s.indexOf(tag, i + tagLen);
    return at < 0 ? this.n : at + tagLen;
  }

  private atLineStart(i: number): boolean {
    let j = i;
    while (j > 0) {
      const c = this.s[j - 1];
      if (c === "\n") return true;
      if (c === " " || c === "\t" || c === "\r") j--;
      else return false;
    }
    return true;
  }

  /** A psql meta-command ends the current statement and becomes a statement of its own. */
  private metaCommand(i: number): number {
    this.finish(i);
    const eol = this.skipLine(i);
    this.mark(i);
    this.finish(eol);
    return eol;
  }

  private peekWord(from: number): [string, number] | null {
    const s = this.s;
    let i = from;
    while (i < this.n && isWs(s[i])) i++;
    if (i >= this.n || !isIdentStart(s[i])) return null;
    const start = i;
    while (i < this.n && isWordChar(s[i])) i++;
    return [s.slice(start, i).toUpperCase(), i];
  }

  private nextIsDelimiter(from: number): boolean {
    let i = from;
    while (i < this.n && isWs(this.s[i])) i++;
    return i >= this.n || this.s.startsWith(this.delimiter, i);
  }

  private delimiterDirective(j: number): number | null {
    const c = this.s[j];
    if (c !== " " && c !== "\t") return null;
    const eol = this.skipLine(j);
    const tok = this.s.slice(j, eol).trim().split(/\s+/)[0];
    if (tok) this.delimiter = tok;
    // The directive is a client command: it never becomes part of a statement.
    return eol;
  }

  private decideRoutine(): void {
    if (this.routine !== null || this.head.length === 0) return;
    if (this.head[0] !== "CREATE") {
      this.routine = false;
      return;
    }
    for (const tok of this.head.slice(1)) {
      switch (tok) {
        case "TRIGGER":
        case "FUNCTION":
        case "PROCEDURE":
        case "EVENT":
          this.routine = true;
          return;
        case "TABLE":
        case "VIEW":
        case "INDEX":
        case "(":
        case "SCHEMA":
        case "DATABASE":
        case "SEQUENCE":
        case "TYPE":
        case "DOMAIN":
        case "MATERIALIZED":
        case "ROLE":
        case "USER":
        case "EXTENSION":
        case "AS":
          this.routine = false;
          return;
      }
    }
    if (this.head.length >= HEAD_MAX) this.routine = false;
  }

  private word(i: number): number {
    const s = this.s;
    const custom = !this.defaultDelimiter();
    let j = i;
    while (j < this.n && isWordChar(s[j])) {
      // `END$$` with `DELIMITER $$`: the delimiter ends the word.
      if (custom && s.startsWith(this.delimiter, j)) break;
      j++;
    }
    const word = s.slice(i, j).toUpperCase();

    // PostgreSQL escape strings: E'…' with backslash escapes.
    if (this.pg && word === "E" && s[j] === "'") {
      this.mark(i);
      return this.skipQuoted(j, "'", true);
    }

    const first = this.contentStart === null;
    if (first && this.mysql && word === "DELIMITER") {
      const end = this.delimiterDirective(j);
      if (end !== null) return end;
    }

    this.mark(i);
    const qualified = i > 0 && s[i - 1] === ".";
    if (this.head.length < HEAD_MAX) this.head.push(word);
    if (this.routine === null) {
      // MariaDB anonymous compound statement: `BEGIN NOT ATOMIC … END`.
      if (first && this.mysql && word === "BEGIN") {
        const w1 = this.peekWord(j);
        const w2 = w1?.[0] === "NOT" ? this.peekWord(w1[1]) : null;
        if (w2?.[0] === "ATOMIC") this.routine = true;
      }
      this.decideRoutine();
    }
    if (qualified || this.routine !== true || !this.defaultDelimiter()) return j;
    switch (word) {
      case "BEGIN": {
        const next = this.peekWord(j)?.[0];
        const opens = this.pg
          ? next === "ATOMIC"
          : !(
              next !== undefined &&
              ["TRANSACTION", "WORK", "DEFERRED", "IMMEDIATE", "EXCLUSIVE"].includes(next)
            ) && !this.nextIsDelimiter(j);
        if (opens) this.depth++;
        return j;
      }
      case "CASE":
        this.depth++;
        return j;
      case "END": {
        const next = this.peekWord(j);
        if (next && ["IF", "LOOP", "WHILE", "REPEAT", "FOR"].includes(next[0])) return next[1];
        this.depth = Math.max(0, this.depth - 1);
        return next?.[0] === "CASE" ? next[1] : j;
      }
      default:
        return j;
    }
  }
}

/** Splits editor text into statements (see the module comment for the rules). */
export function splitStatements(text: string, dialect: SqlDialect): StatementRange[] {
  const splitter = new Splitter(text, dialect);
  splitter.run();
  return splitter.out;
}

/** First index whose `start` is greater than `pos` (statements are sorted by `start`). */
function firstAfter(statements: readonly StatementRange[], pos: number): number {
  let lo = 0;
  let hi = statements.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (statements[mid]!.start > pos) hi = mid;
    else lo = mid + 1;
  }
  return lo;
}

function sameLine(text: string, from: number, to: number): boolean {
  const nl = text.indexOf("\n", from);
  return nl < 0 || nl >= to;
}

/**
 * The statement under the cursor at `pos`: the one containing it (its end included, so a cursor
 * right after the last character counts), else the statement whose terminator sits on the cursor's
 * line before it, else one starting later on the cursor's line. With `loose`, a cursor on a blank
 * line between statements picks the closest previous statement (then the next one).
 */
export function statementAt(
  statements: readonly StatementRange[],
  text: string,
  pos: number,
  loose = false,
): StatementRange | null {
  if (statements.length === 0) return null;
  const k = firstAfter(statements, pos);
  const prev = k > 0 ? statements[k - 1]! : null;
  const next = k < statements.length ? statements[k]! : null;
  if (prev && pos <= prev.end) return prev;
  if (prev && sameLine(text, prev.end, pos)) return prev;
  if (next && sameLine(text, pos, next.start)) return next;
  return loose ? (prev ?? next) : null;
}

/** Convenience for tests and callers that want the statement texts. */
export function statementTexts(text: string, dialect: SqlDialect): string[] {
  return splitStatements(text, dialect).map((s) => text.slice(s.start, s.end));
}
