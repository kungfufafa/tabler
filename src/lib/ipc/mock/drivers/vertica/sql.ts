import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { ResultColumn } from "../../../bindings/ResultColumn";
import type { Value } from "../../../bindings/Value";
import { sortRows } from "../../rows";
import { type Cluster, columnsOf, NULL, type Table, VERSION_TEXT } from "./data";

/**
 * The mock's Vertica SQL: enough of it for the editor to feel real. It runs `SELECT` over one
 * table (columns or `*`, `COUNT(*)`, one `WHERE column = literal`, `ORDER BY`, `LIMIT`), `SELECT`
 * of literals and of `version()` / `current_user` / `current_database()`, `EXPLAIN` (a plan as
 * Vertica prints it), and `INSERT … VALUES`, `UPDATE … SET … [WHERE column = literal]` and
 * `DELETE … [WHERE column = literal]` on one table, which report their count (the `OUTPUT` row).
 * `COMMIT`, `ROLLBACK`, `SET` and DDL finish without rows; a missing table fails as the server
 * does (`Relation "x" does not exist`, 42V01 / 4566).
 */

/** A server error: Vertica's SQLSTATE and its own code (`V`). */
export class ServerFailure extends Error {
  constructor(
    readonly sqlstate: string,
    readonly vendor: number,
    message: string,
  ) {
    super(message);
  }
}

/** The error the backend reports for a server failure of statement `index`. */
export const serverError = (f: ServerFailure, index: number | null): ErrorPayload => ({
  code: "DB_ERROR",
  message: f.message,
  detail: null,
  hint: null,
  sqlstate: f.sqlstate,
  vendorCode: f.vendor,
  position: null,
  statementIndex: index,
  data: null,
});

export type Outcome =
  | { kind: "rows"; columns: ResultColumn[]; rows: Value[][] }
  | { kind: "affected"; affected: number }
  | { kind: "empty" };

/** What a statement runs against: the cluster and who is logged in. */
export interface SqlContext {
  cluster: Cluster;
  user: string;
  defaultSchema: string;
}

const NAME = String.raw`(?:"((?:[^"]|"")+)"|([A-Za-z_][\w$]*))`;
const QUALIFIED = String.raw`${NAME}(?:\s*\.\s*${NAME})?`;

/** The name of `NAME`'s two groups: quoted as written, bare in lower case (Vertica folds). */
const nameOf = (quoted: string | undefined, bare: string | undefined): string =>
  quoted !== undefined ? quoted.replaceAll('""', '"') : (bare ?? "").toLowerCase();

/** A table by its qualified name's four groups. */
function tableOf(ctx: SqlContext, g: (string | undefined)[], at: number): Table {
  const first = nameOf(g[at], g[at + 1]);
  const second = g[at + 2] !== undefined || g[at + 3] !== undefined;
  const schema = second ? first : ctx.defaultSchema;
  const name = second ? nameOf(g[at + 2], g[at + 3]) : first;
  const t = ctx.cluster.tables.find((x) => x.schema === schema && x.name === name);
  if (!t) throw new ServerFailure("42V01", 4566, `Relation "${name}" does not exist`);
  return t;
}

/** A literal of the mock's SQL: a number, a string, TRUE / FALSE or NULL. */
function literal(raw: string): Value {
  const t = raw.trim();
  if (/^null$/i.test(t)) return NULL;
  if (/^(true|false)$/i.test(t)) return { t: "bool", v: t.toLowerCase() === "true" };
  if (/^-?\d+$/.test(t)) return { t: "int", v: Number(t) };
  if (/^-?\d+\.\d+$/.test(t)) return { t: "decimal", v: t };
  const s = /^'((?:[^']|'')*)'$/s.exec(t);
  if (s) return { t: "text", v: s[1]!.replaceAll("''", "'") };
  throw new ServerFailure("42601", 4856, `Syntax error at or near "${t.split(/\s/)[0]}"`);
}

/** The values of a comma-separated list, commas inside strings kept. */
function literals(list: string): Value[] {
  const parts = list.match(/'(?:[^']|'')*'|[^,]+/g) ?? [];
  return parts.map((p) => literal(p));
}

/** A cell's text as the comparison sees it. */
const cellText = (v: Value): string | null =>
  v.t === "null" || v.t === "default"
    ? null
    : typeof v.v === "object"
      ? JSON.stringify(v.v)
      : String(v.v);

/** The rows `WHERE col = literal` keeps (all of them without a condition). */
function where(t: Table, condition: string | undefined): number[] {
  const all = t.rows.map((_, i) => i);
  if (!condition) return all;
  const m = new RegExp(String.raw`^${NAME}\s*=\s*(.+)$`, "s").exec(condition.trim());
  if (!m) throw new ServerFailure("0A000", 9999, "The mock compares one column with a literal");
  const column = nameOf(m[1], m[2]);
  const at = t.cols.findIndex((c) => c.name === column);
  if (at < 0) throw new ServerFailure("42703", 2624, `Column "${column}" does not exist`);
  const want = cellText(literal(m[3]!));
  return all.filter((i) => cellText(t.rows[i]![at] ?? NULL) === want);
}

/** A value for a column: numbers and booleans from their text, the rest as written. */
function coerce(t: Table, column: string, v: Value): Value {
  const c = t.cols.find((x) => x.name === column);
  if (!c || v.t === "null") return v;
  const raw = cellText(v) ?? "";
  if (c.kind === "int" && /^-?\d+$/.test(raw)) return { t: "int", v: Number(raw) };
  if (c.kind === "decimal" && /^-?\d+(\.\d+)?$/.test(raw)) {
    const scale = Number(/,(\d+)\)$/.exec(c.type)?.[1] ?? 0);
    return { t: "decimal", v: Number(raw).toFixed(scale) };
  }
  if (c.kind === "bool") return { t: "bool", v: raw === "true" || raw === "t" || raw === "1" };
  if (c.kind === "date" || c.kind === "time" || c.kind === "uuid") return { t: c.kind, v: raw };
  return { t: "text", v: raw };
}

/** The checks of a row a statement writes: NOT NULL and the enabled CHECK constraints. */
export function rowProblem(t: Table, row: Value[]): ServerFailure | null {
  const at = t.cols.findIndex((c, i) => !c.nullable && (row[i] ?? NULL).t === "null");
  if (at >= 0)
    return new ServerFailure(
      "23502",
      2501,
      `Cannot set a NOT NULL column (${t.cols[at]!.name}) to a NULL value in INSERT/UPDATE statement`,
    );
  for (const check of t.checks) {
    if (!check.enabled) continue;
    // The mock knows the one shape its seed uses: `(table.column >= 0)`.
    const m = /^\(\w+\.(\w+) >= 0\)$/.exec(check.expression);
    const k = m ? t.cols.findIndex((c) => c.name === m[1]) : -1;
    const v = k >= 0 ? row[k] : undefined;
    if (v && v.t === "int" && v.v < 0)
      return new ServerFailure(
        "23514",
        7230,
        `Check constraint '${t.schema}.${t.name}.${check.name}' ${check.expression} violation in table '${t.name}': '${t.cols[k]!.name}=${v.v}'`,
      );
  }
  return null;
}

const column = (name: string, dataType: string, kind: ResultColumn["kind"]): ResultColumn => ({
  name,
  dataType,
  kind,
  nullable: true,
  table: null,
});

/** A literal's result column. */
const literalColumn = (name: string, v: Value): ResultColumn =>
  v.t === "int"
    ? column(name, "int", "int")
    : v.t === "decimal"
      ? column(name, "numeric", "decimal")
      : v.t === "bool"
        ? column(name, "boolean", "bool")
        : column(name, "varchar", "text");

/** EXPLAIN's text, as Vertica prints a plan (one row per line of `QUERY PLAN`). */
function plan(ctx: SqlContext, sql: string): Outcome {
  const from = new RegExp(String.raw`\bfrom\s+${QUALIFIED}`, "i").exec(sql);
  const t = from ? tableOf(ctx, [...from].slice(1), 0) : null;
  const lines = [
    "------------------------------",
    `QUERY PLAN DESCRIPTION:`,
    "------------------------------",
    "",
    `EXPLAIN ${sql}`,
    "",
    " Access Path:",
    t
      ? ` +-STORAGE ACCESS for ${t.name} [Cost: ${t.rows.length * 4}, Rows: ${t.rows.length} (NO STATISTICS)] (PATH ID: 1)`
      : " +-SELECT  [Cost: 1, Rows: 1] (PATH ID: 0)",
    t ? ` |  Projection: ${t.schema}.${t.projections[0]?.name ?? `${t.name}_super`}` : "",
    t ? ` |  Materialize: ${t.cols.map((c) => `${t.name}.${c.name}`).join(", ")}` : "",
  ].filter((l, i) => l !== "" || i < 6);
  return {
    kind: "rows",
    columns: [column("QUERY PLAN", "varchar", "text")],
    rows: lines.map((l) => [{ t: "text", v: l }]),
  };
}

/** Runs one statement of the editor on the mock's cluster. */
export function runSql(ctx: SqlContext, statement: string): Outcome {
  const sql = statement.trim().replace(/;\s*$/, "").trim();
  if (/^explain\b/i.test(sql)) return plan(ctx, sql.replace(/^explain\s+(verbose\s+)?/i, ""));

  if (/^select\s+version\s*\(\s*\)$/i.test(sql))
    return {
      kind: "rows",
      columns: [column("version", "varchar", "text")],
      rows: [[{ t: "text", v: VERSION_TEXT }]],
    };
  if (/^select\s+current_user(\s*\(\s*\))?$/i.test(sql))
    return {
      kind: "rows",
      columns: [column("current_user", "varchar", "text")],
      rows: [[{ t: "text", v: ctx.user }]],
    };
  if (/^select\s+current_database\s*\(\s*\)$/i.test(sql))
    return {
      kind: "rows",
      columns: [column("current_database", "varchar", "text")],
      rows: [[{ t: "text", v: ctx.cluster.database }]],
    };

  const select = new RegExp(
    String.raw`^select\s+(.+?)\s+from\s+${QUALIFIED}(?:\s+where\s+(.+?))?(?:\s+order\s+by\s+${NAME}(?:\s+(asc|desc))?)?(?:\s+limit\s+(\d+))?$`,
    "is",
  ).exec(sql);
  if (select) {
    const g = [...select];
    const t = tableOf(ctx, g, 2);
    if (t.kind === "view")
      // The mock keeps no rows of a view: it reads what it would select.
      return { kind: "rows", columns: columnsOf(t).map(asResult(t)), rows: [] };
    const list = g[1]!.trim();
    const kept = where(t, g[6]);
    if (/^count\s*\(\s*\*\s*\)$/i.test(list))
      return {
        kind: "rows",
        columns: [column("count", "int", "int")],
        rows: [[{ t: "int", v: kept.length }]],
      };
    const meta = columnsOf(t);
    const picked =
      list === "*"
        ? meta
        : list.split(",").map((part) => {
            const m = new RegExp(String.raw`^\s*${NAME}\s*$`).exec(part);
            const name = m ? nameOf(m[1], m[2]) : part.trim();
            const c = meta.find((x) => x.name === name);
            if (!c) throw new ServerFailure("42703", 2624, `Column "${name}" does not exist`);
            return c;
          });
    let rows = kept.map((i) => t.rows[i]!);
    if (g[7] !== undefined || g[8] !== undefined)
      rows = sortRows(
        rows,
        [
          {
            column: nameOf(g[7], g[8]),
            dir: g[9]?.toLowerCase() === "desc" ? "desc" : "asc",
            nulls: null,
          },
        ],
        meta,
      );
    if (g[10] !== undefined) rows = rows.slice(0, Number(g[10]));
    const at = picked.map((c) => meta.indexOf(c));
    return {
      kind: "rows",
      columns: picked.map(asResult(t)),
      rows: rows.map((r) => at.map((i) => r[i] ?? NULL)),
    };
  }
  const values = /^select\s+(.+)$/is.exec(sql);
  if (values && !/\bfrom\b/i.test(values[1]!)) {
    const cells = literals(values[1]!);
    return {
      kind: "rows",
      columns: cells.map((v, i) => literalColumn(i === 0 ? "?column?" : `?column?${i}`, v)),
      rows: [cells],
    };
  }

  const insert = new RegExp(
    String.raw`^insert\s+into\s+${QUALIFIED}\s*\(([^)]*)\)\s*values\s*\((.*)\)$`,
    "is",
  ).exec(sql);
  if (insert) {
    const t = tableOf(ctx, [...insert], 1);
    const names = insert[5]!.split(",").map((n) => n.trim().replace(/^"|"$/g, "").toLowerCase());
    const given = literals(insert[6]!);
    const row = t.cols.map((c) => {
      const k = names.indexOf(c.name);
      if (k >= 0) return coerce(t, c.name, given[k] ?? NULL);
      return c.default !== undefined ? coerce(t, c.name, literal(c.default)) : NULL;
    });
    const problem = rowProblem(t, row);
    if (problem) throw problem;
    t.rows.push(row);
    return { kind: "affected", affected: 1 };
  }
  const update = new RegExp(
    String.raw`^update\s+${QUALIFIED}\s+set\s+(.+?)(?:\s+where\s+(.+))?$`,
    "is",
  ).exec(sql);
  if (update) {
    const t = tableOf(ctx, [...update], 1);
    const sets = (update[5]!.match(/(?:[^,']|'(?:[^']|'')*')+/g) ?? []).map((s) => {
      const m = new RegExp(String.raw`^\s*${NAME}\s*=\s*(.+)$`, "s").exec(s);
      if (!m) throw new ServerFailure("42601", 4856, 'Syntax error at or near "SET"');
      return [nameOf(m[1], m[2]), literal(m[3]!)] as const;
    });
    const hit = where(t, update[6]);
    const next = hit.map((i) => {
      const row = t.rows[i]!.slice();
      for (const [name, v] of sets) {
        const k = t.cols.findIndex((c) => c.name === name);
        if (k < 0) throw new ServerFailure("42703", 2624, `Column "${name}" does not exist`);
        row[k] = coerce(t, name, v);
      }
      const problem = rowProblem(t, row);
      if (problem) throw problem;
      return row;
    });
    hit.forEach((i, k) => (t.rows[i] = next[k]!));
    return { kind: "affected", affected: hit.length };
  }
  const del = new RegExp(String.raw`^delete\s+from\s+${QUALIFIED}(?:\s+where\s+(.+))?$`, "is").exec(
    sql,
  );
  if (del) {
    const t = tableOf(ctx, [...del], 1);
    const hit = new Set(where(t, del[5]));
    t.rows = t.rows.filter((_, i) => !hit.has(i));
    return { kind: "affected", affected: hit.size };
  }
  return { kind: "empty" };
}

/** A table's column as a result column of a statement that reads it. */
const asResult =
  (t: Table) =>
  (c: ReturnType<typeof columnsOf>[number]): ResultColumn => ({
    name: c.name,
    dataType: c.dataType,
    kind: c.kind,
    nullable: c.nullable,
    table: t.name,
  });
