import { splitStatements } from "@/features/editor/scanner";
import { classifyStatement } from "@/features/safety/classify";
import type { ApplyRequest } from "../../../bindings/ApplyRequest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { FilterGroup } from "../../../bindings/FilterGroup";
import type { RowResult } from "../../../bindings/RowResult";
import type { SessionCaps } from "../../../bindings/SessionCaps";
import type { StagedChange } from "../../../bindings/StagedChange";
import type { TableRef } from "../../../bindings/TableRef";
import type { Value } from "../../../bindings/Value";
import { DRIVER_CATALOG } from "../../../bindings/driverCatalog";
import { matchesGroup, sortRows } from "../../rows";
import { mockError } from "../../transport";
import type { MockSession } from "../types";
import {
  columnsOf,
  constraintError,
  findRow,
  GUARD_MESSAGE,
  newRow,
  pkOf,
  serverExprValue,
  sqliteError,
  tableOf,
  type Engine,
  type Table,
} from "./data";
import { runStatement } from "./sql";

/**
 * A session of the mock's LibSQL or D1 server, mirroring `sqlite_http/libsql/session.rs` and
 * `d1/session.rs`: the same caps, SQLite's catalog under `main`, offset paging, rows counted on
 * request, staged commits as one atomic batch (a failing change rolls back the others), and a
 * read-only session that refuses what the SQLite classifier does not call a read, before running.
 */

/** The backend's `READ_ONLY_MESSAGE`. */
export const READ_ONLY_MESSAGE = "This connection is read-only";

/** What `sqlite_http/session.rs::hides_code_in_a_variable` finds: see there. */
export function hidesCodeInAVariable(sql: string): boolean {
  const ident = (c: string) => /[A-Za-z0-9_$]/.test(c) || c.charCodeAt(0) >= 0x80;
  let start = 0;
  while (start < sql.length) {
    if (!"$@:#".includes(sql[start]!)) {
      start++;
      continue;
    }
    let i = start + 1;
    let named = false;
    for (;;) {
      if (i < sql.length && ident(sql[i]!)) {
        named = true;
        i++;
      } else if (sql[i] === ":" && sql[i + 1] === ":") i += 2;
      else break;
    }
    if (named && sql[i] === "(") {
      let end = i + 1;
      // `)` or ASCII whitespace, as SQLite: JS `\s` would also stop at U+00A0 and the like.
      while (end < sql.length && !/[)\t\n\v\f\r ]/.test(sql[end]!)) end++;
      const suffix = sql.slice(i + 1, end);
      if (suffix.includes("--") || suffix.includes("/*") || /['"[`]/.test(suffix)) return true;
      i = end;
    }
    start = Math.max(i, start + 1);
  }
  return false;
}

/** What a read-only session refuses: a statement the classifier does not call a read. */
export const mayWrite = (sql: string): boolean =>
  classifyStatement(sql, "sqlite") === "write" || hidesCodeInAVariable(sql);

/** The session's refusal of a write on a read-only connection (the backend's `refuse_write`). */
export function refuseWrite(readOnly: boolean, sql: string): void {
  if (readOnly && mayWrite(sql)) mockError("INVALID_INPUT", READ_ONLY_MESSAGE);
}

const ENGINE: Record<Engine, string> = { libsql: "LibSQL", d1: "Cloudflare D1" };

/** Every raw filter of a group, nested groups included. */
function rawFilters(group: FilterGroup): string[] {
  return group.items.flatMap((n) =>
    n.kind === "raw" ? [n.sql] : n.kind === "group" ? rawFilters(n) : [],
  );
}

/**
 * The caps each session reports (`libsql::session::capabilities`, `d1::session::capabilities`):
 * SQLite's base, the features the lane implements in the backend's order, counts on request.
 * Read-only is the classifier's, and LibSQL's `perRequest` once a read-only session's server took
 * `PRAGMA query_only = 1` (`queryOnly`).
 */
export function remoteCaps(engine: Engine, queryOnly = false): SessionCaps {
  const base = DRIVER_CATALOG.baseCaps[engine] as SessionCaps;
  return {
    ...base,
    features:
      engine === "libsql"
        ? [
            "editor",
            "explain",
            "format",
            "editRows",
            "insertRows",
            "deleteRows",
            "editWithoutKey",
            "returning",
            "serverExpressions",
            "exactCount",
            "copyAsInsert",
            "structureView",
            "tableDdl",
            "viewDefinition",
            "startupCommands",
            "sessionDetails",
            "mcp",
          ]
        : [
            "editor",
            "explain",
            "format",
            "editRows",
            "insertRows",
            "deleteRows",
            "serverExpressions",
            "exactCount",
            "copyAsInsert",
            "structureView",
            "tableDdl",
            "viewDefinition",
            "switchDatabase",
            "sessionDetails",
            "mcp",
          ],
    namespaces: { ...base.namespaces, databases: engine === "d1" },
    data: { ...base.data, count: "costly" },
    safety: {
      ...base.safety,
      readOnly: engine === "libsql" && queryOnly ? "perRequest" : "classifierOnly",
    },
  };
}

const quote = (name: string) => `"${name.replaceAll('"', '""')}"`;

/** A value as SQLite text, for Preview SQL. */
function literal(v: Value): string {
  switch (v.t) {
    case "null":
    case "default":
      return "NULL";
    case "bool":
      return v.v ? "1" : "0";
    case "int":
    case "float":
      return String(v.v);
    case "bigInt":
    case "decimal":
      return v.v;
    case "bytes":
      return "X'…'";
    case "other":
      return `'${v.v.text.replaceAll("'", "''")}'`;
    case "array":
      return `'${JSON.stringify(v.v).replaceAll("'", "''")}'`;
    default:
      return `'${v.v.replaceAll("'", "''")}'`;
  }
}

/** A staged change's values, server expressions written as SQLite's clock functions. */
function changeSql(c: StagedChange): Record<string, string> {
  const values = c.op === "insert" ? c.values : c.op === "update" ? c.changes : {};
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(values)) if (v.t !== "default") out[k] = literal(v);
  const exprs = c.op === "delete" ? {} : (c.exprs ?? {});
  for (const [k, e] of Object.entries(exprs)) {
    if (e === "default") delete out[k];
    else
      out[k] = e.startsWith("currentDate")
        ? "CURRENT_DATE"
        : e.startsWith("currentTime")
          ? "CURRENT_TIME"
          : "CURRENT_TIMESTAMP";
  }
  return out;
}

function keySql(c: StagedChange): string {
  if (c.op === "insert") return "";
  if (c.key.kind === "rowid") return `rowid = ${c.key.value}`;
  return Object.entries(c.key.value)
    .map(([k, v]) => (v.t === "null" ? `${quote(k)} IS NULL` : `${quote(k)} = ${literal(v)}`))
    .join(" AND ");
}

/** Deletes, then updates, then inserts: the order the batch runs them in. */
function ordered(req: ApplyRequest): StagedChange[] {
  return [
    ...req.changes.filter((c) => c.op === "delete"),
    ...req.changes.filter((c) => c.op === "update"),
    ...req.changes.filter((c) => c.op === "insert"),
  ];
}

/** One session of `engine` on the database `tables`. */
export function remoteSession(
  engine: Engine,
  tables: Map<string, Table>,
  options: {
    readOnly: boolean;
    tls: boolean;
    /** LibSQL: the server took `PRAGMA query_only = 1` at connect. */
    queryOnly?: boolean;
    databases: () => { name: string; isCurrent: boolean }[];
  },
): MockSession {
  const caps = remoteCaps(engine, options.queryOnly);
  const returning = caps.features.includes("returning");
  const visible = (name: string) =>
    tables.get(name) ?? mockError("NOT_FOUND", `not found: table ${name}`);
  const tableOfRef = (ref: TableRef) => visible(ref.name);

  const preview = (req: ApplyRequest): string[] => {
    const t = `${quote("main")}.${quote(req.table.name)}`;
    const back = req.returning && returning ? " RETURNING *" : "";
    return ordered(req).map((c) => {
      if (c.op === "delete") return `DELETE FROM ${t} WHERE ${keySql(c)}`;
      const values = changeSql(c);
      if (c.op === "update")
        return `UPDATE ${t} SET ${Object.entries(values)
          .map(([k, v]) => `${quote(k)} = ${v}`)
          .join(", ")} WHERE ${keySql(c)}${back}`;
      const cols = Object.keys(values);
      return cols.length === 0
        ? `INSERT INTO ${t} DEFAULT VALUES${back}`
        : `INSERT INTO ${t} (${cols.map(quote).join(", ")}) VALUES (${Object.values(values).join(", ")})${back}`;
    });
  };

  /** D1 runs every statement of a text, so each call sends one (`api::one_statement`). */
  const oneStatement = (sql: string) => {
    if (engine === "d1" && (splitStatements(sql, "sqlite").length > 1 || hidesCodeInAVariable(sql)))
      mockError(
        "INVALID_INPUT",
        `${ENGINE[engine]} would run every statement of this text: send one statement at a time`,
      );
  };

  /**
   * The checks of the backend's `worker_query` on the SELECT a browse or a count sends, which
   * holds every raw filter the user typed, in groups too: refused on a read-only session when it
   * is not a read, and on D1 when it is more than one statement.
   */
  const checkQuery = (table: TableRef, filter: FilterGroup | null) => {
    const raw = filter ? rawFilters(filter) : [];
    if (raw.length === 0) return;
    const sql = `SELECT * FROM ${quote("main")}.${quote(table.name)} WHERE ${raw.map((r) => `(${r})`).join(" AND ")}`;
    refuseWrite(options.readOnly, sql);
    oneStatement(sql);
  };

  return {
    caps,
    listDatabases: () => options.databases().map((d) => ({ ...d, owner: null, encoding: null })),
    listSchemas: () => [{ name: "main", isDefault: true }],
    listTables: (schema) =>
      schema !== "main"
        ? []
        : [...tables.values()].map((t) => ({
            name: t.name,
            schema: "main",
            kind: t.kind,
            rowEstimate: null,
            sizeBytes: null,
            comment: null,
          })),
    tableMeta: (ref) => {
      const t = tableOfRef(ref);
      return {
        table: { schema: "main", name: t.name },
        kind: t.kind,
        columns: columnsOf(t),
        primaryKey: pkOf(t),
        indexes: t.indexes,
        foreignKeys: t.foreignKeys,
        triggers: t.triggers,
        comment: null,
        rowEstimate: null,
        ddl: t.ddl,
        checks: [],
        options: null,
      };
    },
    tableDdl: (ref) => {
      const t = tableOfRef(ref);
      return [
        t.ddl,
        ...t.indexes.flatMap((i) => (i.definition ? [i.definition] : [])),
        ...t.triggers.map((tr) => tr.definition),
      ]
        .map((s) => `${s};`)
        .join("\n\n");
    },
    browse: (req): BrowsePage => {
      const t = tableOfRef(req.table);
      checkQuery(req.table, req.filter);
      const columns = columnsOf(t);
      const all = t.rows();
      const kept = all
        .map((row, i) => ({ row, rowid: t.rowids[i] ?? i + 1 }))
        .filter(({ row }) => !req.filter || matchesGroup(req.filter, row, columns));
      const sorted = sortRows(
        kept.map(({ row, rowid }) => [...row, { t: "int", v: rowid } as Value]),
        req.sort,
        columns,
      );
      const page = sorted.slice(req.offset, req.offset + req.limit);
      const picked = req.columns ? columns.filter((c) => req.columns!.includes(c.name)) : columns;
      const at = picked.map((c) => columns.indexOf(c));
      const pk = pkOf(t);
      // A table without a key: LibSQL edits it by rowid, D1 not at all (no rowid identity).
      const identity =
        pk.length > 0 ? "pk" : t.kind === "table" && engine === "libsql" ? "rowid" : "none";
      const order = req.sort.length
        ? ` ORDER BY ${req.sort.map((s) => `${quote(s.column)} ${s.dir.toUpperCase()}`).join(", ")}`
        : "";
      return {
        columns: picked,
        rows: page.map((row) => at.map((i) => row[i] ?? { t: "null" })),
        pkColumns: pk,
        identity,
        rowIds:
          identity === "rowid"
            ? page.map((row) => {
                const id = row[columns.length];
                return id?.t === "int" ? id.v : 0;
              })
            : null,
        offset: req.offset,
        limit: req.limit,
        sql: `SELECT ${picked.map((c) => quote(c.name)).join(", ")} FROM ${quote("main")}.${quote(t.name)}${order} LIMIT ${req.limit} OFFSET ${req.offset}`,
        durationMs: engine === "d1" ? 38 : 21,
      };
    },
    count: (ref, filter) => {
      const t = tableOfRef(ref);
      checkQuery(ref, filter);
      const columns = columnsOf(t);
      const rows = t.rows().filter((row) => !filter || matchesGroup(filter, row, columns));
      return { count: rows.length, exact: true };
    },
    fetchCell: (ref, key, column) => {
      const t = tableOfRef(ref);
      const at = findRow(t, key);
      const col = t.cols.findIndex((c) => c.name === column);
      if (at < 0 || col < 0) mockError("NOT_FOUND", "not found: the row is gone");
      return t.data[at]![col] ?? { t: "null" };
    },
    preview,
    apply: (req): ApplyResult => {
      if (options.readOnly) mockError("INVALID_INPUT", READ_ONLY_MESSAGE);
      const t = tableOf(tables, req.table.name);
      if (t.kind === "view") mockError("DB_ERROR", `cannot modify ${t.name} because it is a view`);
      const changes = ordered(req);
      if (engine === "d1" && changes.some((c) => c.op !== "insert" && c.key.kind !== "pk"))
        mockError("UNSUPPORTED", "unsupported: Cloudflare D1 edits rows by their primary key only");
      const sql = preview(req);
      const started = performance.now();
      const data = t.data.map((row) => row.slice());
      const rowids = [...t.rowids];
      const nextRowid = t.nextRowid;
      const results: RowResult[] = [];
      const rowMap = (row: Value[]) =>
        Object.fromEntries(t.cols.map((c, k) => [c.name, row[k] ?? ({ t: "null" } as Value)]));
      for (const [i, c] of changes.entries()) {
        let broken: { message: string; code: number } | null = null;
        let row: Value[] | null = null;
        const values = c.op === "delete" ? {} : { ...(c.op === "insert" ? c.values : c.changes) };
        for (const [k, e] of Object.entries(c.op === "delete" ? {} : (c.exprs ?? {})))
          values[k] = e === "default" ? { t: "default" } : serverExprValue(e);
        if (c.op === "insert") {
          row = newRow(t, values);
          broken = constraintError(t, row, -1);
          if (!broken) {
            t.data.push(row);
            t.rowids.push(t.nextRowid++);
          }
        } else {
          const at = findRow(t, c.key);
          if (at < 0) broken = { message: GUARD_MESSAGE, code: 0 };
          else if (c.op === "delete") {
            t.data.splice(at, 1);
            t.rowids.splice(at, 1);
          } else {
            row = t.data[at]!.slice();
            for (const [k, v] of Object.entries(values)) {
              const col = t.cols.findIndex((x) => x.name === k);
              if (col >= 0) row[col] = v.t === "default" ? { t: "null" } : v;
            }
            broken = constraintError(t, row, at);
            if (!broken) t.data[at] = row;
          }
        }
        if (broken) {
          // One batch: what ran before the failure is rolled back with it.
          t.data.splice(0, t.data.length, ...data);
          t.rowids.splice(0, t.rowids.length, ...rowids);
          t.nextRowid = nextRowid;
          const error =
            broken.code === 0
              ? { ...sqliteError(engine, broken.message, 0, i), vendorCode: null }
              : sqliteError(engine, broken.message, broken.code, i);
          return {
            success: false,
            results: changes.map((x, j) => ({
              rowId: x.rowId,
              op: x.op,
              affected: 0,
              returned: null,
              error: j === i ? error : null,
            })),
            sql,
            error,
            durationMs: Math.max(1, Math.round(performance.now() - started)),
          };
        }
        results.push({
          rowId: c.rowId,
          op: c.op,
          affected: 1,
          returned: row && req.returning && returning ? rowMap(row) : null,
          error: null,
        });
      }
      return {
        success: true,
        results,
        sql,
        error: null,
        durationMs: Math.max(1, Math.round(performance.now() - started)),
      };
    },
    run: (statement, emit) => {
      refuseWrite(options.readOnly, statement);
      oneStatement(statement);
      // A LibSQL server runs one statement per request and refuses a text of several.
      if (engine === "libsql" && splitStatements(statement, "sqlite").length > 1)
        mockError("DB_ERROR", "SQL string contains more than one statement");
      runStatement(statement, tables, engine, emit);
    },
    sessionDetails: () => ({
      tls: options.tls,
      tlsVersion: null,
      tlsCipher: null,
      encoding: "UTF-8",
    }),
  };
}
