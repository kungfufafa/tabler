import { skippedSpans, type SqlDialect } from "@/features/editor/scanner";

/**
 * What the text of the statement behind an editable result says about the tables it reads: the
 * checks that keep an edit from landing on another row than the one shown (`editability.ts`).
 * Strings and comments are skipped; names compare case-insensitively, so a doubtful reading only
 * ever counts a reference too many (a result that stays read-only), never one too few.
 */

/**
 * A name (bare word or quoted identifier, lower-cased), a symbol, or a literal. `maybeString`: a
 * MySQL `"…"`, a string unless the server runs with `ANSI_QUOTES`: it counts as a reference
 * (`references`) but never as the place a table is named or aliased.
 */
export type SqlToken =
  | { k: "name"; v: string; quoted: boolean; maybeString?: boolean }
  | { k: "sym"; v: string }
  | { k: "lit" };

const LEX = /([\p{L}_][\p{L}\p{N}_$]*)|(\p{N}[\p{L}\p{N}_.]*)|(\s+)|([\s\S])/gu;

function lexCode(code: string, out: SqlToken[]): void {
  for (const m of code.matchAll(LEX)) {
    if (m[1] !== undefined) out.push({ k: "name", v: m[1].toLowerCase(), quoted: false });
    else if (m[2] !== undefined) out.push({ k: "lit" });
    else if (m[4] !== undefined) out.push({ k: "sym", v: m[4] });
  }
}

/** The tokens of one statement's code. */
export function sqlTokens(sql: string, dialect: SqlDialect): SqlToken[] {
  const out: SqlToken[] = [];
  let pos = 0;
  for (const span of skippedSpans(sql, dialect)) {
    if (span.start < pos) continue;
    lexCode(sql.slice(pos, span.start), out);
    pos = span.end;
    if (span.kind !== "quoted") continue;
    const raw = sql.slice(span.start, span.end);
    const q = raw[0];
    if (raw.startsWith("/*")) {
      // A MySQL executable comment / optimizer hint: its body is code.
      const body = raw.replace(/^\/\*(?:M?!\d*|\+)/, "").replace(/\*\/$/, "");
      out.push(...sqlTokens(body, dialect));
    } else if (q === '"' || q === "`" || q === "[") {
      const close = q === "[" ? "]" : q;
      const inner = raw.slice(1, raw.endsWith(close) && raw.length > 1 ? -1 : undefined);
      const v = q === "[" ? inner : inner.split(q + q).join(q);
      const maybeString = dialect === "mysql" && q === '"';
      out.push({
        k: "name",
        v: v.toLowerCase(),
        quoted: true,
        ...(maybeString && { maybeString }),
      });
    } else {
      out.push({ k: "lit" });
    }
  }
  lexCode(sql.slice(pos), out);
  return out;
}

const isSym = (t: SqlToken | undefined, v: string): boolean => t?.k === "sym" && t.v === v;
const isWord = (t: SqlToken | undefined, v: string): boolean =>
  t?.k === "name" && !t.quoted && t.v === v;

/** References to `name` (a table): the name, unless it qualifies a column (`users.id`). */
export function references(tokens: readonly SqlToken[], name: string): number {
  const want = name.toLowerCase();
  let n = 0;
  tokens.forEach((t, i) => {
    if (t.k === "name" && t.v === want && !isSym(tokens[i + 1], ".")) n++;
  });
  return n;
}

/** Whether the statement names the table `name` (as a table, not only as a column's qualifier). */
export function names(tokens: readonly SqlToken[], name: string): boolean {
  const want = name.toLowerCase();
  return tokens.some(
    (t, i) => t.k === "name" && !t.maybeString && t.v === want && !isSym(tokens[i + 1], "."),
  );
}

/** A set operation (`UNION`, `EXCEPT`, `INTERSECT`) anywhere in the statement. */
export function isCompound(tokens: readonly SqlToken[]): boolean {
  return tokens.some((t) => ["union", "except", "intersect"].some((w) => isWord(t, w)));
}

/** Index just past the `)` that closes the `(` at `i`. */
function closeParen(tokens: readonly SqlToken[], i: number): number {
  let depth = 0;
  for (let j = i; j < tokens.length; j++) {
    if (isSym(tokens[j], "(")) depth++;
    else if (isSym(tokens[j], ")") && --depth === 0) return j + 1;
  }
  return tokens.length;
}

/**
 * Names the statement defines as `name [(columns)] AS [[NOT] MATERIALIZED] (…)`: its CTEs (and
 * named windows, which only make the check below stricter).
 */
export function cteNames(tokens: readonly SqlToken[]): string[] {
  const out: string[] = [];
  tokens.forEach((t, i) => {
    if (t.k !== "name") return;
    let j = i + 1;
    if (isSym(tokens[j], "(")) j = closeParen(tokens, j);
    if (!isWord(tokens[j], "as")) return;
    j++;
    if (isWord(tokens[j], "not")) j++;
    if (isWord(tokens[j], "materialized")) j++;
    if (isSym(tokens[j], "(") && !out.includes(t.v)) out.push(t.v);
  });
  return out;
}

/**
 * A CTE read more than once (`WITH c AS (…) … FROM c a JOIN c b`): the tables it reads then have
 * several references in the result without being named twice.
 */
export function readsCteTwice(tokens: readonly SqlToken[]): boolean {
  // The definition is a reference too: more than two means two reads.
  return cteNames(tokens).some((name) => references(tokens, name) > 2);
}

/** Whether the statement defines `alias` on the table `table` (`table [AS] alias`). */
export function aliasedAs(tokens: readonly SqlToken[], table: string, alias: string): boolean {
  const [t, a] = [table.toLowerCase(), alias.toLowerCase()];
  return tokens.some((tok, i) => {
    if (tok.k !== "name" || tok.maybeString || tok.v !== t || isSym(tokens[i + 1], "."))
      return false;
    const next = isWord(tokens[i + 1], "as") ? tokens[i + 2] : tokens[i + 1];
    return next?.k === "name" && !next.maybeString && next.v === a;
  });
}
