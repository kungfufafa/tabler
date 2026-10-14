import type { SqlDialect } from "./scanner";

/**
 * Which tables a statement uses (FROM / JOIN / UPDATE / INTO, with their aliases), for the
 * completions that need them: `alias.*` → every column, `JOIN … ON` from the foreign keys. A
 * light tokenizer, not a parser: strings and comments are skipped, subqueries are simply scanned
 * as part of the text.
 */

export interface TableUse {
  /** The reference as written (`public.users`, `"Orders"`). */
  text: string;
  /** Unquoted name parts: `[name]` or `[schema, name]`. */
  parts: string[];
  alias: string | null;
  /** From a JOIN clause (otherwise FROM, UPDATE or INTO). */
  joined: boolean;
  /** Offset of the reference in the scanned text. */
  from: number;
}

interface Tok {
  kind: "word" | "quoted" | "dot" | "comma" | "open" | "close" | "other";
  /** Unquoted text. */
  value: string;
  from: number;
  to: number;
}

const WORD = /[A-Za-z_\u0080-￿][\w$\u0080-￿]*/y;

function tokenize(sql: string): Tok[] {
  const out: Tok[] = [];
  const n = sql.length;
  let i = 0;
  while (i < n) {
    const c = sql[i]!;
    if (/\s/.test(c)) {
      i++;
    } else if (c === "-" && sql[i + 1] === "-") {
      const nl = sql.indexOf("\n", i);
      i = nl < 0 ? n : nl + 1;
    } else if (c === "/" && sql[i + 1] === "*") {
      const end = sql.indexOf("*/", i + 2);
      i = end < 0 ? n : end + 2;
    } else if (c === "'") {
      let j = i + 1;
      while (j < n) {
        if (sql[j] === "\\") j += 2;
        else if (sql[j] === "'" && sql[j + 1] === "'") j += 2;
        else if (sql[j] === "'") break;
        else j++;
      }
      i = j + 1;
    } else if (c === "$" && /^\$[A-Za-z_]*\$/.test(sql.slice(i, i + 64))) {
      const tag = /^\$[A-Za-z_]*\$/.exec(sql.slice(i, i + 64))![0];
      const end = sql.indexOf(tag, i + tag.length);
      i = end < 0 ? n : end + tag.length;
    } else if (c === '"' || c === "`" || c === "[") {
      const close = c === "[" ? "]" : c;
      let j = i + 1;
      let value = "";
      while (j < n) {
        if (sql[j] === close && sql[j + 1] === close && close !== "]") {
          value += close;
          j += 2;
        } else if (sql[j] === close) break;
        else value += sql[j++];
      }
      out.push({ kind: "quoted", value, from: i, to: Math.min(n, j + 1) });
      i = j + 1;
    } else {
      WORD.lastIndex = i;
      const m = WORD.exec(sql);
      if (m) {
        out.push({ kind: "word", value: m[0], from: i, to: i + m[0].length });
        i += m[0].length;
        continue;
      }
      const kind =
        c === "."
          ? "dot"
          : c === ","
            ? "comma"
            : c === "("
              ? "open"
              : c === ")"
                ? "close"
                : "other";
      out.push({ kind, value: c, from: i, to: i + 1 });
      i++;
    }
  }
  return out;
}

const STARTERS = new Set(["from", "join", "update", "into"]);
/** Words that end a table reference (never an alias). */
const STOP = new Set(
  (
    "on using where join inner left right full outer cross natural group order having limit " +
    "offset union intersect except window set values select returning lateral straight_join " +
    "for fetch partition use force ignore with when then else end and or tablesample default"
  ).split(" "),
);

const isName = (t: Tok | undefined): t is Tok =>
  !!t && (t.kind === "quoted" || (t.kind === "word" && !STOP.has(t.value.toLowerCase())));

/** The tables of `sql` in order of appearance. */
export function tablesInStatement(sql: string): TableUse[] {
  const toks = tokenize(sql);
  const out: TableUse[] = [];
  for (let i = 0; i < toks.length; i++) {
    const t = toks[i]!;
    if (t.kind !== "word" || !STARTERS.has(t.value.toLowerCase())) continue;
    const joined = t.value.toLowerCase() === "join";
    let j = i + 1;
    for (;;) {
      if (toks[j]?.kind === "word" && /^(only|lateral)$/i.test(toks[j]!.value)) j++;
      const first = toks[j];
      if (!isName(first)) break;
      const parts = [first.value];
      let last = first;
      while (toks[j + 1]?.kind === "dot" && isName(toks[j + 2])) {
        last = toks[j + 2]!;
        parts.push(last.value);
        j += 2;
      }
      j++;
      let alias: string | null = null;
      const next = toks[j];
      if (next?.kind === "word" && next.value.toLowerCase() === "as" && isName(toks[j + 1])) {
        alias = toks[j + 1]!.value;
        j += 2;
      } else if (isName(next) && next.value.toLowerCase() !== "as") {
        alias = next.value;
        j++;
      }
      out.push({
        text: sql.slice(first.from, last.to),
        parts: parts.slice(-2),
        alias,
        joined,
        from: first.from,
      });
      // FROM a, b
      if (!joined && toks[j]?.kind === "comma") {
        j++;
        continue;
      }
      break;
    }
    i = Math.max(i, j - 1);
  }
  return out;
}

/** Splits `public."My Table"` into unquoted parts (`["public", "My Table"]`). */
export function identParts(text: string): string[] {
  return tokenize(text)
    .filter((t) => t.kind === "word" || t.kind === "quoted")
    .map((t) => t.value);
}

const PLAIN: Record<SqlDialect, RegExp> = {
  postgres: /^[a-z_][a-z0-9_$]*$/,
  mysql: /^[A-Za-z_][A-Za-z0-9_$]*$/,
  sqlite: /^[A-Za-z_][A-Za-z0-9_]*$/,
};

/** An identifier as the dialect needs it written: quoted only when it has to be. */
export function quoteIdent(name: string, dialect: SqlDialect): string {
  if (PLAIN[dialect].test(name)) return name;
  const q = dialect === "mysql" ? "`" : '"';
  return q + name.split(q).join(q + q) + q;
}

/** schema → table → columns, as the completion metadata holds them. */
export type TableColumns = Readonly<Record<string, Readonly<Record<string, readonly string[]>>>>;

export interface FoundTable {
  schema: string;
  name: string;
  columns: readonly string[];
}

function lookup<T>(map: Readonly<Record<string, T>>, key: string): [string, T] | null {
  if (Object.hasOwn(map, key)) return [key, map[key]!];
  const lower = key.toLowerCase();
  const hit = Object.keys(map).find((k) => k.toLowerCase() === lower);
  return hit === undefined ? null : [hit, map[hit]!];
}

/**
 * The table `parts` names (`[name]` in the default schema first, then in any schema; or
 * `[schema, name]`), matched exactly, else ignoring case.
 */
export function findTable(
  tables: TableColumns | undefined,
  parts: readonly string[],
  defaultSchema: string | undefined,
): FoundTable | null {
  if (!tables || parts.length === 0) return null;
  const name = parts[parts.length - 1]!;
  const schemas =
    parts.length > 1
      ? [parts[parts.length - 2]!]
      : [...(defaultSchema ? [defaultSchema] : []), ...Object.keys(tables)];
  for (const s of schemas) {
    const schema = lookup(tables, s);
    if (!schema) continue;
    const table = lookup(schema[1], name);
    if (table) return { schema: schema[0], name: table[0], columns: table[1] };
  }
  return null;
}

/**
 * The table a qualifier (`u`, `users`, `public.users`) stands for in a statement: an alias of
 * one of its tables, else a table it names, else the qualifier itself as a table name.
 */
export function resolveQualifier(statement: string, qualifier: readonly string[]): string[] {
  if (qualifier.length === 1) {
    const q = qualifier[0]!.toLowerCase();
    const uses = tablesInStatement(statement);
    const byAlias = uses.find((u) => u.alias?.toLowerCase() === q);
    if (byAlias) return byAlias.parts;
    const byName = uses.find((u) => u.parts[u.parts.length - 1]!.toLowerCase() === q);
    if (byName) return byName.parts;
  }
  return [...qualifier];
}
