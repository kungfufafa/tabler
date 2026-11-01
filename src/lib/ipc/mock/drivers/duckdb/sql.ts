import { EXTERNAL_ACCESS_HINT } from "@/features/drivers/duckdb/params";
import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { ResultColumn } from "../../../bindings/ResultColumn";
import type { Value } from "../../../bindings/Value";
import { sortRows } from "../../rows";
import { type Database, columnsOf, NULL, type Table, VERSION, viewRows } from "./data";

/**
 * The mock's DuckDB SQL: enough of it for the editor to feel real. It runs `SELECT` (or
 * FROM-first `FROM t …`) over one table (columns or `*`, `count(*)`, one `WHERE column =
 * literal`, `ORDER BY`, `LIMIT`), `SELECT` of literals and of `version()`, `current_database()`
 * and `current_schema()`, `SHOW TABLES`, `DESCRIBE t`, `EXPLAIN [ANALYZE]` (DuckDB's
 * `explain_key` / `explain_value` rows), and `INSERT … VALUES`, `UPDATE … SET … [WHERE column =
 * literal]` and `DELETE … [WHERE …]` on one table, which report their count. A file named where a
 * table goes is refused as the engine refuses it: in a hardened session for its external access,
 * elsewhere because the mock has no files. Anything else finishes without rows. Errors are
 * DuckDB's, without SQLSTATE or code (`errors::from_message`).
 */

/** A DuckDB error: its kind (`Catalog`, `Binder`…) heads the message, with the driver's hint. */
export class EngineFailure extends Error {
  constructor(
    message: string,
    readonly hint: string | null = null,
  ) {
    super(message);
  }
}

/** The error the backend reports for an engine failure of statement `index`. */
export const engineError = (f: EngineFailure, index: number | null): ErrorPayload => ({
  code: "DB_ERROR",
  message: f.message,
  detail: null,
  hint: f.hint,
  sqlstate: null,
  vendorCode: null,
  position: null,
  statementIndex: index,
  data: null,
});

export type Outcome =
  | { kind: "rows"; columns: ResultColumn[]; rows: Value[][] }
  | { kind: "affected"; affected: number }
  | { kind: "empty" };

/** What a statement runs against: the database and the session's engine settings. */
export interface SqlContext {
  db: Database;
  /** External access off (read-only and MCP sessions). */
  hardened: boolean;
  /** `access_mode=READ_ONLY`, or an encrypted file attached read-only. */
  fileReadOnly: boolean;
}

const NAME = String.raw`(?:"((?:[^"]|"")+)"|([A-Za-z_][\w$]*))`;
/** `[db.][schema.]table`: three optional parts, six groups. */
const QUALIFIED = String.raw`${NAME}(?:\s*\.\s*${NAME})?(?:\s*\.\s*${NAME})?`;

const nameOf = (quoted: string | undefined, bare: string | undefined): string =>
  quoted !== undefined ? quoted.replaceAll('""', '"') : (bare ?? "");

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/** A table by its qualified name's six groups (DuckDB matches names in any case). */
function tableOf(ctx: SqlContext, g: (string | undefined)[], at: number): Table {
  const parts = [0, 2, 4]
    .filter((k) => g[at + k] !== undefined || g[at + k + 1] !== undefined)
    .map((k) => nameOf(g[at + k], g[at + k + 1]));
  const name = parts.at(-1)!;
  const schema = parts.length >= 2 ? parts.at(-2)! : "main";
  if (parts.length === 3 && !same(parts[0]!, ctx.db.name))
    throw new EngineFailure(`Catalog Error: Catalog "${parts[0]}" does not exist!`);
  const t = ctx.db.tables.find((x) => same(x.schema, schema) && same(x.name, name));
  if (!t) throw new EngineFailure(`Catalog Error: Table with name ${name} does not exist!`);
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
  throw new EngineFailure(`Parser Error: syntax error at or near "${t.split(/\s/)[0]}"`);
}

/** The values of a comma-separated list, commas inside strings kept. */
function literals(list: string): Value[] {
  const parts = list.match(/'(?:[^']|'')*'|[^,]+/g) ?? [];
  return parts.map((p) => literal(p));
}

/** A cell's text as a comparison sees it. */
const cellText = (v: Value): string | null =>
  v.t === "null" || v.t === "default"
    ? null
    : typeof v.v === "object"
      ? JSON.stringify(v.v)
      : String(v.v);

const columnAt = (t: Table, name: string): number => {
  const at = t.cols.findIndex((c) => same(c.name, name));
  if (at < 0)
    throw new EngineFailure(`Binder Error: Referenced column "${name}" not found in FROM clause!`);
  return at;
};

/** The rows `WHERE col = literal` keeps (all of them without a condition). */
function where(t: Table, rows: Value[][], condition: string | undefined): number[] {
  const all = rows.map((_, i) => i);
  if (!condition) return all;
  const m = new RegExp(String.raw`^${NAME}\s*=\s*(.+)$`, "s").exec(condition.trim());
  if (!m)
    throw new EngineFailure("Not implemented Error: the mock compares one column with a literal");
  const at = columnAt(t, nameOf(m[1], m[2]));
  const want = cellText(literal(m[3]!));
  return all.filter((i) => cellText(rows[i]![at] ?? NULL) === want);
}

/** A value for a column: numbers and booleans from their text, the rest as written. */
function coerce(t: Table, at: number, v: Value): Value {
  const c = t.cols[at]!;
  if (v.t === "null") return v;
  const raw = cellText(v) ?? "";
  if ((c.kind === "int" || c.kind === "bigInt") && /^-?\d+$/.test(raw))
    return Number.isSafeInteger(Number(raw))
      ? { t: "int", v: Number(raw) }
      : { t: "bigInt", v: raw };
  if (c.kind === "decimal" && /^-?\d+(\.\d+)?$/.test(raw)) {
    const scale = Number(/,(\d+)\)$/.exec(c.type)?.[1] ?? 0);
    return { t: "decimal", v: Number(raw).toFixed(scale) };
  }
  if (c.kind === "bool") return { t: "bool", v: raw === "true" || raw === "t" || raw === "1" };
  if (c.kind === "enum") {
    if (c.enumValues && !c.enumValues.includes(raw))
      throw new EngineFailure(`Conversion Error: Could not convert string '${raw}' to UINT8`);
    return { t: "enum", v: raw };
  }
  if (c.kind === "date" || c.kind === "uuid" || c.kind === "json") return { t: c.kind, v: raw };
  return { t: "text", v: raw };
}

/** The constraints DuckDB checks on a row a statement writes: NOT NULL, the keys, the CHECK. */
export function rowProblem(
  t: Table,
  row: Value[],
  skip: number | null = null,
): EngineFailure | null {
  const at = t.cols.findIndex((c, i) => !c.nullable && (row[i] ?? NULL).t === "null");
  if (at >= 0)
    return new EngineFailure(
      `Constraint Error: NOT NULL constraint failed: ${t.name}.${t.cols[at]!.name}`,
    );
  if (t.pk.length > 0) {
    const keyOf = (r: Value[]) =>
      t.pk.map((k) => cellText(r[columnAt(t, k)] ?? NULL)).join("\u0000");
    const mine = keyOf(row);
    if (t.rows.some((r, i) => i !== skip && keyOf(r) === mine))
      return new EngineFailure(
        `Constraint Error: Duplicate key "${t.pk.map((k) => `${k}: ${cellText(row[columnAt(t, k)] ?? NULL)}`).join(", ")}" violates primary key constraint.`,
      );
  }
  for (const check of t.checks) {
    // The mock knows the one shape its seed uses: `(column >= 0)`.
    const m = /^\((\w+) >= 0\)$/.exec(check.expression);
    const v = m ? row[columnAt(t, m[1]!)] : undefined;
    if (v && (v.t === "decimal" || v.t === "int") && Number(v.v) < 0)
      return new EngineFailure(
        `Constraint Error: CHECK constraint failed on table ${t.name} with expression CHECK(${check.expression})`,
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

const text = (v: string): Value => ({ t: "text", v });

/** A literal's result column. */
const literalColumn = (name: string, v: Value): ResultColumn =>
  v.t === "int"
    ? column(name, "INTEGER", "int")
    : v.t === "decimal"
      ? column(name, "DECIMAL(18,3)", "decimal")
      : v.t === "bool"
        ? column(name, "BOOLEAN", "bool")
        : column(name, "VARCHAR", "text");

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

/** EXPLAIN's rows, as DuckDB prints a plan (`explain_key`, `explain_value`). */
function plan(ctx: SqlContext, sql: string, analyze: boolean): Outcome {
  const from = new RegExp(String.raw`\bfrom\s+${QUALIFIED}`, "i").exec(sql);
  const t = from ? tableOf(ctx, [...from].slice(1), 0) : null;
  const rows = t ? (t.kind === "view" ? viewRows(ctx.db, t) : t.rows).length : 1;
  const box = (title: string, lines: string[]) => {
    const width = Math.max(title.length, ...lines.map((l) => l.length)) + 2;
    const pad = (s: string) => {
      const left = Math.floor((width - s.length) / 2);
      return `│${" ".repeat(left)}${s}${" ".repeat(width - s.length - left)}│`;
    };
    return [
      `┌${"─".repeat(width)}┐`,
      pad(title),
      pad("─".repeat(width - 2)),
      ...lines.map(pad),
      `└${"─".repeat(width)}┘`,
    ];
  };
  const tree = [
    ...box("PROJECTION", ["__internal_decompress", `~${rows} rows`]),
    ...box(
      t ? "SEQ_SCAN " : "DUMMY_SCAN",
      t ? [`Table: ${t.name}`, `Type: Sequential Scan`, `~${rows} rows`] : ["0 columns"],
    ),
  ].join("\n");
  const head = analyze
    ? "┌─────────────────────────────────────┐\n│┌───────────────────────────────────┐│\n││    Query Profiling Information    ││\n│└───────────────────────────────────┘│\n└─────────────────────────────────────┘\n"
    : "";
  return {
    kind: "rows",
    columns: [column("explain_key", "VARCHAR", "text"), column("explain_value", "VARCHAR", "text")],
    rows: [[text(analyze ? "analyzed_plan" : "physical_plan"), text(head + tree)]],
  };
}

/** `FROM 'x.csv'` or `FROM "C:/x.csv"`: a replacement scan of a file. */
const FILE_SCAN =
  /\bfrom\s+(?:'((?:[^']|'')+)'|"((?:[^"]|"")+\.(?:csv|tsv|parquet|json|ndjson|jsonl|txt)(?:\.gz)?)")/i;

/** Runs one statement of the editor on the mock's database. */
export function runSql(ctx: SqlContext, statement: string): Outcome {
  let sql = statement.trim().replace(/;\s*$/, "").trim();
  const explain = /^explain\s+(analyze\s+)?/i.exec(sql);
  if (explain) return plan(ctx, sql.slice(explain[0].length), explain[1] !== undefined);

  const file = FILE_SCAN.exec(sql);
  if (file) {
    const path = file[1] ?? file[2]!;
    if (ctx.hardened)
      throw new EngineFailure(
        `Permission Error: Cannot access file "${path}" - file system operations are disabled by configuration`,
        EXTERNAL_ACCESS_HINT,
      );
    throw new EngineFailure(`IO Error: No files found that match the pattern "${path}"`);
  }

  const single = (name: string, v: Value): Outcome => ({
    kind: "rows",
    columns: [column(name, "VARCHAR", "text")],
    rows: [[v]],
  });
  if (/^select\s+version\s*\(\s*\)$/i.test(sql)) return single("version()", text(VERSION));
  if (/^select\s+current_database\s*\(\s*\)$/i.test(sql))
    return single("current_database()", text(ctx.db.name));
  if (/^select\s+current_schema\s*\(\s*\)$/i.test(sql))
    return single("current_schema()", text("main"));
  if (/^show\s+tables$/i.test(sql))
    return {
      kind: "rows",
      columns: [column("name", "VARCHAR", "text")],
      rows: ctx.db.tables
        .filter((t) => t.schema === "main")
        .map((t) => [text(t.name)])
        .sort((a, b) => cellText(a[0]!)!.localeCompare(cellText(b[0]!)!)),
    };
  const describe = new RegExp(String.raw`^(?:describe|desc)\s+${QUALIFIED}$`, "i").exec(sql);
  if (describe) {
    const t = tableOf(ctx, [...describe], 1);
    return {
      kind: "rows",
      columns: ["column_name", "column_type", "null", "key", "default", "extra"].map((n) =>
        column(n, "VARCHAR", "text"),
      ),
      rows: t.cols.map((c) => [
        text(c.name),
        text(c.type),
        text(c.nullable ? "YES" : "NO"),
        t.pk.includes(c.name) ? text("PRI") : NULL,
        c.default !== undefined ? text(c.default) : NULL,
        NULL,
      ]),
    };
  }

  // FROM-first: `FROM t …` is `SELECT * FROM t …`.
  if (/^from\s/i.test(sql)) sql = `SELECT * ${sql}`;
  const select = new RegExp(
    String.raw`^select\s+(.+?)\s+from\s+${QUALIFIED}(?:\s+where\s+(.+?))?(?:\s+order\s+by\s+${NAME}(?:\s+(asc|desc))?)?(?:\s+limit\s+(\d+))?$`,
    "is",
  ).exec(sql);
  if (select) {
    const g = [...select];
    const t = tableOf(ctx, g, 2);
    const source = t.kind === "view" ? viewRows(ctx.db, t) : t.rows;
    const list = g[1]!.trim();
    const kept = where(t, source, g[8]);
    if (/^count\s*\(\s*\*\s*\)$/i.test(list))
      return {
        kind: "rows",
        columns: [column("count_star()", "BIGINT", "bigInt")],
        rows: [[{ t: "int", v: kept.length }]],
      };
    const meta = columnsOf(t);
    const picked =
      list === "*"
        ? meta
        : list.split(",").map((part) => {
            const m = new RegExp(String.raw`^\s*${NAME}\s*$`).exec(part);
            const name = m ? nameOf(m[1], m[2]) : part.trim();
            return meta[columnAt(t, name)]!;
          });
    let rows = kept.map((i) => source[i]!);
    if (g[9] !== undefined || g[10] !== undefined)
      rows = sortRows(
        rows,
        [
          {
            column: meta[columnAt(t, nameOf(g[9], g[10]))]!.name,
            dir: g[11]?.toLowerCase() === "desc" ? "desc" : "asc",
            nulls: null,
          },
        ],
        meta,
      );
    if (g[12] !== undefined) rows = rows.slice(0, Number(g[12]));
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
      columns: cells.map((v, i) =>
        literalColumn(cellText(v) ?? `NULL${i === 0 ? "" : `_${i}`}`, v),
      ),
      rows: [cells],
    };
  }

  const write = /^(insert|update|delete)\b/i.exec(sql);
  if (write && ctx.fileReadOnly)
    throw new EngineFailure(
      `Invalid Input Error: Cannot execute statement of type "${write[1]!.toUpperCase()}" on database "${ctx.db.name}" which is attached in read-only mode!`,
    );
  const insert = new RegExp(
    String.raw`^insert\s+into\s+${QUALIFIED}\s*\(([^)]*)\)\s*values\s*\((.*)\)$`,
    "is",
  ).exec(sql);
  if (insert) {
    const t = tableOf(ctx, [...insert], 1);
    const names = insert[7]!.split(",").map((n) => n.trim().replace(/^"|"$/g, ""));
    const given = literals(insert[8]!);
    const row = t.cols.map((c, i) => {
      const k = names.findIndex((n) => same(n, c.name));
      if (k >= 0) return coerce(t, i, given[k] ?? NULL);
      return defaultOf(ctx.db, t, i);
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
    const sets = (update[7]!.match(/(?:[^,']|'(?:[^']|'')*')+/g) ?? []).map((s) => {
      const m = new RegExp(String.raw`^\s*${NAME}\s*=\s*(.+)$`, "s").exec(s);
      if (!m) throw new EngineFailure('Parser Error: syntax error at or near "SET"');
      return [columnAt(t, nameOf(m[1], m[2])), literal(m[3]!)] as const;
    });
    const hit = where(t, t.rows, update[8]);
    const next = hit.map((i) => {
      const row = t.rows[i]!.slice();
      for (const [k, v] of sets) row[k] = coerce(t, k, v);
      const problem = rowProblem(t, row, i);
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
    const hit = new Set(where(t, t.rows, del[7]));
    t.rows = t.rows.filter((_, i) => !hit.has(i));
    return { kind: "affected", affected: hit.size };
  }
  return { kind: "empty" };
}

/** A column's default, as DuckDB fills it for an insert that leaves it out. */
export function defaultOf(db: Database, t: Table, at: number): Value {
  const c = t.cols[at]!;
  const d = c.default;
  if (d === undefined) return NULL;
  const seq = /^nextval\('(\w+)'\)$/.exec(d);
  if (seq) {
    const name = `${t.schema}.${seq[1]}`;
    const next = db.sequences.get(name) ?? 1;
    db.sequences.set(name, next + 1);
    return { t: "int", v: next };
  }
  if (/^current_timestamp$/i.test(d) || /^now\(\)$/i.test(d)) return clockOf(c.kind);
  return coerce(t, at, literal(d));
}

/** `CURRENT_TIMESTAMP` as the column's kind shows it. */
export function clockOf(kind: Table["cols"][number]["kind"]): Value {
  const now = new Date().toISOString().replace("T", " ").replace("Z", "");
  return kind === "timestampTz" ? { t: "timestampTz", v: `${now}+00` } : { t: "timestamp", v: now };
}
