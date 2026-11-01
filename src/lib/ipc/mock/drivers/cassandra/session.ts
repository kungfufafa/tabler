import { classifyDialect } from "@/features/drivers/portable/classify";
import { split } from "@/features/drivers/portable/scanner";
import type { ApplyRequest } from "../../../bindings/ApplyRequest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { DialectLex } from "../../../bindings/DialectLex";
import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { FilterGroup } from "../../../bindings/FilterGroup";
import type { RowKey } from "../../../bindings/RowKey";
import type { RowResult } from "../../../bindings/RowResult";
import type { SafeConfirm } from "../../../bindings/SafeConfirm";
import type { SessionCaps } from "../../../bindings/SessionCaps";
import type { StagedChange } from "../../../bindings/StagedChange";
import type { TableRef } from "../../../bindings/TableRef";
import type { Value } from "../../../bindings/Value";
import CQL_LEX from "../../../bindings/lex/cql.json";
import { matchesGroup } from "../../rows";
import { mockError } from "../../transport";
import type { MockSession } from "../types";
import { type Editor, runStatement } from "./cql";
import {
  columnsOf,
  type Keyspace,
  metaOf,
  qualified,
  quoted,
  rowsOf,
  same,
  SYSTEM_KEYSPACES,
  type Table,
} from "./data";
import { cell, dbPayload, literal, payload, ServerFailure } from "./values";

/** An error `mockError` threw. */
const isRefusal = (e: unknown): e is { code: string; message: string } =>
  typeof e === "object" && e !== null && "code" in e && "message" in e;

/**
 * A session of the mock's Cassandra, mirroring `drivers/cassandra/session.rs`: the same caps
 * (`tests/fixtures/cassandra/caps.json` holds the backend's), keyspaces as databases with the
 * current one as the only schema, pages by a cursor bound to the statement that made it, the size
 * estimate as the count until an exact count is asked for, AND-only filters with ALLOW FILTERING,
 * edits as lightweight transactions by the full primary key sent one by one (a change that does
 * not apply stops the rest, and those before it stay applied), and the editor with cqlsh's
 * commands. A read-only session refuses what the CQL classifier calls a write (or does not know)
 * before anything runs.
 */

export const LEX = CQL_LEX as unknown as DialectLex;

/** The backend's `READ_ONLY_MESSAGE`. */
export const READ_ONLY_MESSAGE = "This connection is read-only";

/** `session::refuse_added_statement`'s words. */
export const ADDED_STATEMENT_MESSAGE =
  "A filter can only read rows: this one would run another statement";

/** `sql::writes`: what the CQL classifier calls a write, or does not know. */
export const writes = (sql: string) => classifyDialect(LEX, sql) === "write";

/** `session::FEATURES`, in `Feature`'s order. */
export const FEATURES: SessionCaps["features"] = [
  "editor",
  "editRows",
  "insertRows",
  "deleteRows",
  "serverExpressions",
  "exactCount",
  "structureView",
  "tableDdl",
  "viewDefinition",
  "switchDatabase",
  "startupCommands",
];

/** `CassandraSession::capabilities`. */
export function cassandraCaps(keyspace: string | null, confirm: SafeConfirm): SessionCaps {
  return {
    driver: "cassandra",
    flavor: null,
    dialect: "cql",
    language: "cql",
    features: FEATURES,
    namespaces: {
      databases: true,
      schemas: "databaseIsSchema",
      defaultSchema: keyspace,
      labels: { database: "Keyspace", schema: "Keyspace", table: "Table", tables: "Tables" },
    },
    data: {
      paging: "cursor",
      count: "costly",
      sort: false,
      filterOps: ["eq", "gt", "lt", "ge", "le", "in", "like", "between"],
      rawFilter: "sqlWhere",
      transactions: "sequential",
      affected: "exact",
      defaultKeyword: false,
      upsert: false,
      serverExprs: ["now", "currentTimestamp", "currentDate"],
      maxParams: 1000,
    },
    safety: { readOnly: "classifierOnly", hardened: false, confirm },
    session: { keepaliveSecs: 30, pingBilled: false },
  };
}

/** What a session was opened with. */
export interface CassandraOptions {
  keyspaces: Map<string, Keyspace>;
  readOnly: boolean;
  confirm: SafeConfirm;
  editor: Editor;
}

// --- the grid's statements (`dialect::cql`) -----------------------------------------------

const refused = (why: string): never => mockError("INVALID_INPUT", `CQL ${why}`);

function findColumn(columns: ColumnMeta[], name: string): ColumnMeta {
  return (
    columns.find((c) => c.name === name) ?? mockError("INVALID_INPUT", `unknown column "${name}"`)
  );
}

/** `Debug` of a `FilterOp` (`IsNull`), as the backend's message prints it. */
const debugOp = (op: string) => op[0]!.toUpperCase() + op.slice(1);

/** `cql::conditions`: the AND-ed relations of a filter, values inlined for display. */
function conditions(columns: ColumnMeta[], group: FilterGroup, out: string[]): void {
  const counted = group.items.filter((i) => !(i.kind === "raw" && i.sql.trim() === "")).length;
  if (group.conjunction === "or" && counted > 1) refused("has no OR: combine the filters with AND");
  for (const item of group.items) {
    if (item.kind === "group") {
      conditions(columns, item, out);
      continue;
    }
    if (item.kind === "raw") {
      if (item.sql.trim() !== "") out.push(item.sql.trim());
      continue;
    }
    if (item.kind === "anyColumn") refused("cannot filter on any column: pick one");
    if (item.kind !== "condition") continue;
    const id = quoted(findColumn(columns, item.column).name);
    const one = (values: Value[]): Value => {
      const v = values[0];
      if (v === undefined)
        return mockError("INVALID_INPUT", `filter on "${item.column}" needs a value`);
      if (v.t === "null") refused("cannot match NULL");
      return v;
    };
    const signs: Partial<Record<string, string>> = {
      eq: "=",
      gt: ">",
      lt: "<",
      ge: ">=",
      le: "<=",
      like: "LIKE",
    };
    const sign = signs[item.op];
    if (sign) out.push(`${id} ${sign} ${literal(one(item.values))}`);
    else if (item.op === "in") {
      if (item.values.length === 0 || item.values.some((v) => v.t === "null"))
        refused("IN needs values and cannot match NULL");
      out.push(`${id} IN (${item.values.map(literal).join(", ")})`);
    } else if (item.op === "between") {
      const low = one(item.values);
      const high = one(item.values.slice(1));
      out.push(`${id} >= ${literal(low)}`, `${id} <= ${literal(high)}`);
    } else mockError("INVALID_INPUT", `CQL cannot filter with ${debugOp(item.op)}`);
  }
}

/** `cql::where_clause`; null without relations. */
export function whereClause(columns: ColumnMeta[], filter: FilterGroup | null): string | null {
  if (!filter) return null;
  const parts: string[] = [];
  conditions(columns, filter, parts);
  return parts.length > 0 ? parts.join(" AND ") : null;
}

/** The FNV-1a fingerprint of a statement (`cursor::fingerprint`), as 16 hex digits. */
function fingerprint(sql: string): string {
  let h = 0xcbf29ce484222325n;
  for (const b of new TextEncoder().encode(sql))
    h = ((h ^ BigInt(b)) * 0x100000001b3n) & 0xffffffffffffffffn;
  return h.toString(16).padStart(16, "0");
}

const b64url = (s: string) => btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

/** `cursor::encode`: the next page's start, bound to the statement. */
export const encodeCursor = (sql: string, at: number) =>
  `cql1.${b64url(`${fingerprint(sql)}:${at}`)}`;

/** `cursor::decode`, with its refusals. */
export function decodeCursor(cursor: string | null | undefined, sql: string): number {
  if (!cursor) return 0;
  const invalid = () => mockError("INVALID_INPUT", "not a Cassandra page cursor");
  if (!cursor.startsWith("cql1.")) return invalid();
  let body: string;
  try {
    body = atob(cursor.slice(5).replace(/-/g, "+").replace(/_/g, "/"));
  } catch {
    return invalid();
  }
  const m = /^([0-9a-f]{16}):(\d+)$/.exec(body);
  if (!m) return invalid();
  if (m[1] !== fingerprint(sql))
    mockError(
      "INVALID_INPUT",
      "the page cursor belongs to another query (the filter or sort changed): reload from the first page",
    );
  return Number(m[2]);
}

/** One session to the mock's cluster. */
export function cassandraSession(o: CassandraOptions): MockSession {
  const keyspaceOf = (ref: TableRef): Keyspace => {
    const name = ref.schema || o.editor.keyspace;
    if (!name) return mockError("INVALID_INPUT", "Cassandra: choose a keyspace first");
    return o.keyspaces.get(name) ?? mockError("NOT_FOUND", `not found: table ${name}.${ref.name}`);
  };
  const tableOf = (ref: TableRef): { ks: Keyspace; t: Table } => {
    const ks = keyspaceOf(ref);
    const t = ks.tables.find((x) => x.name === ref.name);
    return t ? { ks, t } : mockError("NOT_FOUND", `not found: table ${ks.name}.${ref.name}`);
  };

  /** `refuse_write` and `refuse_added_statement`: the checks of every read the app builds. */
  const checkRead = (sql: string) => {
    if (o.readOnly && writes(sql)) mockError("INVALID_INPUT", READ_ONLY_MESSAGE);
    if (writes(sql) || split(LEX.lex, sql).length > 1)
      mockError("INVALID_INPUT", ADDED_STATEMENT_MESSAGE);
  };

  const filtered = (ks: Keyspace, t: Table, filter: FilterGroup | null) => {
    const columns = columnsOf(t);
    const rows = rowsOf(ks, t);
    return filter ? rows.filter((r) => matchesGroup(filter, r, columns)) : rows;
  };

  /** `cql::key_condition`: the row a full primary key names. */
  const keyed = (
    t: Table,
    key: RowKey,
    columns: ColumnMeta[],
  ): [string, (row: Value[]) => boolean] => {
    if (key.kind !== "pk") return refused("addresses a row by its primary key only");
    const keyCols = columns.filter((c) => c.isPk);
    const map = key.value as Record<string, Value>;
    const whole = "addresses a row by its whole primary key (partition and clustering columns)";
    if (keyCols.length === 0 || Object.keys(map).length !== keyCols.length) refused(whole);
    const parts = keyCols.map((c) => {
      const v = map[c.name];
      if (v === undefined) return refused(whole);
      if (v.t === "null") return refused("key columns are never NULL");
      return [c, v] as const;
    });
    const at = (name: string) => t.cols.findIndex((c) => c.name === name);
    return [
      parts.map(([c, v]) => `${quoted(c.name)} = ${literal(v)}`).join(" AND "),
      (row) => parts.every(([c, v]) => same(row[at(c.name)] ?? { t: "null" }, v)),
    ];
  };

  /** `cql::change_hook` of one change: its statement and how it applies. */
  const plan = (t: Table, c: StagedChange) => {
    const columns = columnsOf(t);
    if (columns.some((x) => x.dataType === "counter"))
      refused(
        "counter tables cannot be edited in the grid: run UPDATE … SET c = c + n in the editor",
      );
    const name = qualified(t);
    const cells = (values: Record<string, Value>, exprs: Record<string, string> | undefined) => {
      const all = new Map<string, Value | string>(Object.entries(values));
      for (const [k, e] of Object.entries(exprs ?? {})) all.set(k, e);
      return [...all].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
    };
    const cellSql = (col: ColumnMeta, v: Value | string): string => {
      if (v === "default" || (typeof v !== "string" && v.t === "default"))
        return refused("has no DEFAULT: enter the value explicitly");
      if (typeof v === "string") {
        const clock =
          v === "now" && col.kind === "uuid"
            ? "now()"
            : (v === "now" || v === "currentTimestamp") &&
                (col.kind === "timestamp" || col.kind === "timestampTz")
              ? "toTimestamp(now())"
              : v === "currentDate" && col.kind === "date"
                ? "toDate(now())"
                : null;
        return clock ?? mockError("UNSUPPORTED", `unsupported: CQL has no ${v} for "${col.name}"`);
      }
      return v.t === "null" ? "NULL" : literal(v);
    };
    if (c.op === "delete") {
      const [cond, match] = keyed(t, c.key, columns);
      return { sql: `DELETE FROM ${name} WHERE ${cond} IF EXISTS`, match, change: c };
    }
    if (c.op === "update") {
      const list = cells(c.changes, c.exprs);
      if (list.length === 0) mockError("INVALID_INPUT", "update has no changed columns");
      const sets = list.map(([k, v]) => {
        const col = findColumn(columns, k);
        if (col.isPk)
          refused("cannot change a primary key column: delete the row and insert it again");
        return `${quoted(k)} = ${cellSql(col, v)}`;
      });
      const [cond, match] = keyed(t, c.key, columns);
      return {
        sql: `UPDATE ${name} SET ${sets.join(", ")} WHERE ${cond} IF EXISTS`,
        match,
        change: c,
      };
    }
    const list = cells(c.values, c.exprs).filter(
      ([, v]) => v !== "default" && (typeof v === "string" || v.t !== "default"),
    );
    const names: string[] = [];
    const values: string[] = [];
    for (const [k, v] of list) {
      const col = findColumn(columns, k);
      names.push(quoted(k));
      values.push(cellSql(col, v));
    }
    const given = new Set([...Object.keys(c.values), ...Object.keys(c.exprs ?? {})]);
    const missing = columns.find((x) => x.isPk && !given.has(x.name));
    if (missing) mockError("INVALID_INPUT", `a new row needs its key column "${missing.name}"`);
    return {
      sql: `INSERT INTO ${name} (${names.join(", ")}) VALUES (${values.join(", ")}) IF NOT EXISTS`,
      match: null,
      change: c,
    };
  };

  /** `plan_rows`: deletes, then updates, then inserts (`mutate::ordered`). */
  const planAll = (req: ApplyRequest) => {
    if (o.readOnly) mockError("UNSUPPORTED", "unsupported: this connection is read-only");
    if (req.changes.some((c) => c.op !== "insert" && c.key.kind !== "pk"))
      mockError(
        "UNSUPPORTED",
        "unsupported: Cassandra: editing rows without a primary key is not available",
      );
    const { t } = tableOf(req.table);
    const order = [
      ...req.changes.filter((c) => c.op === "delete"),
      ...req.changes.filter((c) => c.op === "update"),
      ...req.changes.filter((c) => c.op === "insert"),
    ];
    return { t, planned: order.map((c) => plan(t, c)) };
  };

  /** The clock a server expression writes. */
  const clockValue = (col: ColumnMeta): Value => {
    const now = new Date();
    if (col.kind === "uuid") return { t: "uuid", v: crypto.randomUUID() };
    if (col.kind === "date") return { t: "date", v: now.toISOString().slice(0, 10) };
    return cell(col, { t: "text", v: now.toISOString() });
  };

  /** One change, as the node runs its lightweight transaction; false: it did not apply. */
  const applyOne = (
    t: Table,
    c: StagedChange,
    match: ((row: Value[]) => boolean) | null,
  ): boolean => {
    // The node refuses it once the statement reaches it (the backend builds it like any table's).
    if (t.view) throw new ServerFailure(8704, "Cannot directly modify a materialized view");
    const columns = columnsOf(t);
    const at = match ? t.data.findIndex(match) : -1;
    if (c.op === "delete") {
      if (at < 0) return false;
      t.data.splice(at, 1);
      return true;
    }
    const written = (values: Record<string, Value>, exprs: Record<string, string> | undefined) => {
      const out = new Map<string, Value>();
      for (const [k, v] of Object.entries(values)) out.set(k, cell(findColumn(columns, k), v));
      for (const k of Object.keys(exprs ?? {})) out.set(k, clockValue(findColumn(columns, k)));
      return out;
    };
    if (c.op === "update") {
      if (at < 0) return false;
      const row = t.data[at]!.slice();
      for (const [k, v] of written(c.changes, c.exprs))
        row[t.cols.findIndex((x) => x.name === k)] = v;
      t.data[at] = row;
      return true;
    }
    const values = written(c.values, c.exprs);
    const row = t.cols.map((col) => values.get(col.name) ?? ({ t: "null" } as Value));
    const keyAt = columns.flatMap((col, i) => (col.isPk ? [i] : []));
    if (t.data.some((r) => keyAt.every((i) => same(r[i]!, row[i]!)))) return false;
    t.data.push(row);
    return true;
  };

  return {
    get caps() {
      return cassandraCaps(o.editor.keyspace, o.confirm);
    },
    listDatabases: () =>
      [...o.keyspaces.values()]
        .filter((k) => !SYSTEM_KEYSPACES.includes(k.name) || k.name === o.editor.keyspace)
        .map((k) => ({
          name: k.name,
          owner: null,
          encoding: null,
          isCurrent: k.name === o.editor.keyspace,
        })),
    listSchemas: () => (o.editor.keyspace ? [{ name: o.editor.keyspace, isDefault: true }] : []),
    listTables: (schema) =>
      (o.keyspaces.get(schema)?.tables ?? [])
        .map((t) => ({
          name: t.name,
          schema,
          kind: t.kind,
          rowEstimate: null,
          sizeBytes: null,
          comment: t.comment,
        }))
        .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0)),
    tableMeta: (ref) => metaOf(tableOf(ref).t),
    tableDdl: (ref) => metaOf(tableOf(ref).t).ddl ?? "",
    browse: (req): BrowsePage => {
      if (!req.cursor && req.offset > 0)
        mockError(
          "INVALID_INPUT",
          "Cassandra pages follow one another: there is no offset to start from",
        );
      if (req.sort.length > 0)
        refused(
          "sorts only by clustering columns within one partition: the grid keeps the table's order",
        );
      const { ks, t } = tableOf(req.table);
      const columns = columnsOf(t);
      const names = req.columns ?? columns.map((c) => c.name);
      if (names.length === 0) mockError("INVALID_INPUT", "no columns to select");
      const picked = names.map((n) => findColumn(columns, n));
      const where = whereClause(columns, req.filter);
      const sql = `SELECT ${picked.map((c) => quoted(c.name)).join(", ")} FROM ${qualified(t)}${where ? ` WHERE ${where} ALLOW FILTERING` : ""}`;
      const start = decodeCursor(req.cursor, sql);
      checkRead(sql);
      const rows = filtered(ks, t, req.filter);
      const size = Math.max(1, req.limit);
      const page = rows.slice(start, start + size);
      const at = picked.map((c) => columns.indexOf(c));
      return {
        columns: picked,
        rows: page.map((r) => at.map((i) => r[i] ?? { t: "null" })),
        pkColumns: columns.filter((c) => c.isPk).map((c) => c.name),
        identity: "pk",
        rowIds: null,
        offset: req.offset,
        limit: req.limit,
        sql,
        durationMs: 3,
        nextCursor: start + size < rows.length ? encodeCursor(sql, start + size) : null,
      };
    },
    count: (ref, filter) => {
      const { ks, t } = tableOf(ref);
      // The size estimate (partitions) stands in until a filter or Count asks for rows.
      if (filter === null && t.estimate !== null) return { count: t.estimate, exact: false };
      const where = whereClause(columnsOf(t), filter);
      checkRead(
        `SELECT count(*) FROM ${qualified(t)}${where ? ` WHERE ${where} ALLOW FILTERING` : ""}`,
      );
      return { count: filtered(ks, t, filter).length, exact: true };
    },
    fetchCell: (ref, key, column) => {
      const { ks, t } = tableOf(ref);
      const columns = columnsOf(t);
      const col = findColumn(columns, column);
      const [cond, match] = keyed(t, key, columns);
      checkRead(`SELECT ${quoted(col.name)} FROM ${qualified(t)} WHERE ${cond}`);
      const row = rowsOf(ks, t).find(match);
      if (!row) return mockError("NOT_FOUND", "not found: the row no longer exists");
      return row[columns.indexOf(col)] ?? { t: "null" };
    },
    preview: (req) => planAll(req).planned.map((p) => p.sql),
    apply: (req): ApplyResult => {
      const { t, planned } = planAll(req);
      const sql = planned.map((p) => p.sql);
      const started = performance.now();
      const ms = () => Math.max(1, Math.round(performance.now() - started));
      for (const [i, p] of planned.entries()) {
        let error: ErrorPayload | null = null;
        try {
          if (!applyOne(t, p.change, p.match))
            error = payload(
              "DB_ERROR",
              p.change.op === "insert"
                ? "a row with this primary key already exists, so it was not inserted"
                : "the row no longer exists (it was deleted or its key changed), so the change was not applied",
              { statementIndex: i },
            );
        } catch (e) {
          if (!(e instanceof ServerFailure)) throw e;
          error = dbPayload(e, i);
        }
        if (!error) continue;
        // `partial_results`: those before stay applied, the failed one says why, the rest were
        // not sent.
        const notSent = payload("INVALID_INPUT", "not applied: an earlier change failed");
        const results: RowResult[] = planned.map((q, j) => ({
          rowId: q.change.rowId,
          op: q.change.op,
          affected: 0,
          returned: null,
          error: j < i ? null : j === i ? error : notSent,
        }));
        return { success: false, results, sql, error, durationMs: ms() };
      }
      return {
        success: true,
        results: planned.map((p) => ({
          rowId: p.change.rowId,
          op: p.change.op,
          affected: 1,
          returned: null,
          error: null,
        })),
        sql,
        error: null,
        durationMs: ms(),
      };
    },
    run: (statement, emit) => {
      const ctx = { keyspaces: o.keyspaces, readOnly: o.readOnly, editor: o.editor };
      try {
        runStatement(statement, ctx, emit, writes);
      } catch (e) {
        // A server error is `Db` with its CQL code; the session's own refusals keep theirs.
        const error =
          e instanceof ServerFailure
            ? dbPayload(e, 0)
            : isRefusal(e)
              ? payload(e.code, e.message)
              : null;
        if (!error) throw e;
        emit({ event: "failed", data: { index: 0, error } });
      }
    },
  };
}
