import { callsListedFunction, classifyDialect } from "@/features/drivers/portable/classify";
import {
  isIdentStart,
  isPunct,
  PLAIN,
  scan,
  text,
  upper,
} from "@/features/drivers/portable/scanner";
import type { ApplyRequest } from "../../../bindings/ApplyRequest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { DialectLex } from "../../../bindings/DialectLex";
import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { FilterGroup } from "../../../bindings/FilterGroup";
import type { ProcessRow } from "../../../bindings/ProcessRow";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { RowResult } from "../../../bindings/RowResult";
import type { SessionCaps } from "../../../bindings/SessionCaps";
import type { StagedChange } from "../../../bindings/StagedChange";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { TableRef } from "../../../bindings/TableRef";
import type { Value } from "../../../bindings/Value";
import { DRIVER_CATALOG } from "../../../bindings/driverCatalog";
import TSQL_LEX from "../../../bindings/lex/tsql.json";
import { matchesGroup, sortRows } from "../../rows";
import { mockError } from "../../transport";
import type { MockSession } from "../types";
import {
  coerce,
  columnsOf,
  constraintError,
  type Database,
  display,
  findRow,
  GUARD_MESSAGE,
  NULL,
  rowsOf,
  serverError,
  ServerFailure,
  type Table,
  tableIn,
} from "./data";
import { type Batch, runBatch } from "./sql";

/**
 * A session of the mock's SQL Server, mirroring `drivers/sqlserver/session.rs`: the same caps,
 * databases with their schemas (`dbo`, `sales`), offset pages ordered by the key (or
 * `(SELECT NULL)`), exact counts, staged commits in one transaction that a failing change rolls
 * back whole (each statement guarded by `@@ROWCOUNT`), the editor's GO batches, Running Queries
 * with Kill Connection only, routines, Show DDL and session details. A read-only session refuses
 * what the backend's `refused_read_only` refuses, before anything runs, and every session refuses
 * a raw filter that adds a statement (`refuse_added_statement`).
 */

const LEX = TSQL_LEX as DialectLex;

/** The backend's `READ_ONLY_MESSAGE`. */
export const READ_ONLY_MESSAGE = "This connection is read-only";

/** `session::refuse_added_statement`'s words. */
export const ADDED_STATEMENT_MESSAGE =
  "A filter or an exported query can only read rows: this one would run another statement";

/** `tsql.rs` `SPEC.knobs.side_effect_functions`. */
export const SIDE_EFFECT_FUNCTIONS = [
  "OPENQUERY",
  "OPENROWSET",
  "OPENDATASOURCE",
  "XP_*",
  "SP_*",
  "NEXT VALUE FOR",
] as const;

/**
 * `tsql::hides_statement`, token for token: a text another reader may scan otherwise, an ODBC
 * `{? = call p}` escape, or the words CALL, DUMP and LOAD in code, also hidden in a number (any
 * tail of a word that starts with a digit, or follows one and a dot, from a letter on).
 */
export function hidesStatement(sql: string): boolean {
  const s = scan(LEX.lex, PLAIN, sql);
  if (s.ambiguous) return true;
  const code = s.tokens.filter((t) => t.kind !== "space" && t.kind !== "comment");
  const digits = (i: number) => {
    const t = code[i];
    return t !== undefined && t.kind === "word" && /^[0-9]/.test(text(s, t));
  };
  return code.some((t, i) => {
    if (isPunct(s, t, "{")) {
      const next = code[i + 1];
      return next !== undefined && isPunct(s, next, "?");
    }
    if (t.kind !== "word") return false;
    const words = [upper(s, t), text(s, t).toUpperCase()];
    const dot = code[i - 1];
    const number =
      digits(i) || (i >= 2 && dot !== undefined && isPunct(s, dot, ".") && digits(i - 2));
    if (number) words.push(...words.flatMap(hiddenInNumber));
    return words.some((w) => HIDDEN_WORDS.includes(w));
  });
}

const HIDDEN_WORDS = ["CALL", "DUMP", "LOAD"];

/** `tsql::hidden_in_number`: every tail of a word from a letter or `_` after its first character. */
function hiddenInNumber(w: string): string[] {
  const chars = [...w];
  return chars.flatMap((c, k) => (k > 0 && isIdentStart(c) ? [chars.slice(k).join("")] : []));
}

/** `session::adds_a_statement`: a write to the classifier, or a statement it has no word for. */
export const addsAStatement = (sql: string): boolean =>
  classifyDialect(LEX, sql) === "write" || hidesStatement(sql);

/** `session::refused_read_only`. */
export const refusedReadOnly = (sql: string): boolean =>
  addsAStatement(sql) || callsListedFunction(LEX.lex, SIDE_EFFECT_FUNCTIONS, sql);

/** The features `session::FEATURES` grants, in `Feature`'s order. */
export const FEATURES: SessionCaps["features"] = [
  "editor",
  "explain",
  "explainAnalyze",
  "cancelOnServer",
  "editRows",
  "insertRows",
  "deleteRows",
  "serverExpressions",
  "exactCount",
  "structureView",
  "tableDdl",
  "viewDefinition",
  "routines",
  "switchDatabase",
  "processList",
  "killProcess",
  "sessionDetails",
  "startupCommands",
];

/** `MssqlSession::capabilities`. */
export function sqlserverCaps(defaultSchema: string, commandPassword: boolean): SessionCaps {
  const base = DRIVER_CATALOG.baseCaps.sqlserver as SessionCaps;
  return {
    ...base,
    features: FEATURES,
    namespaces: { ...base.namespaces, databases: true, schemas: "schemas", defaultSchema },
    data: {
      paging: "offset",
      count: "estimate",
      sort: true,
      filterOps: [
        "eq",
        "ne",
        "gt",
        "lt",
        "ge",
        "le",
        "like",
        "notLike",
        "in",
        "notIn",
        "isNull",
        "isNotNull",
        "between",
        "notBetween",
        "contains",
        "notContains",
        "containsCase",
        "notContainsCase",
        "hasPrefix",
        "hasSuffix",
        "hasPrefixCase",
        "hasSuffixCase",
        "ilike",
      ],
      rawFilter: "sqlWhere",
      transactions: "atomic",
      affected: "exact",
      defaultKeyword: true,
      upsert: false,
      serverExprs: [
        "now",
        "currentTimestamp",
        "currentDate",
        "currentTime",
        "localTimestamp",
        "default",
      ],
      maxParams: 2098,
    },
    safety: {
      readOnly: "classifierOnly",
      hardened: false,
      confirm: commandPassword ? "passcode" : "password",
    },
    session: { keepaliveSecs: 30, pingBilled: false },
  };
}

/** `[name]` with `]` doubled (`tsql.rs` `bracketed`). */
export const bracket = (name: string) => `[${name.replaceAll("]", "]]")}]`;

/** A value as T-SQL text, for Preview SQL (`tsql.rs` literals: `N''`, `0x…`). */
export function literal(v: Value): string {
  switch (v.t) {
    case "null":
      return "NULL";
    case "default":
      return "DEFAULT";
    case "bool":
      return v.v ? "1" : "0";
    case "int":
    case "float":
    case "bigInt":
    case "decimal":
      return display(v);
    case "bytes":
      return display(v);
    default:
      return `N'${display(v).replaceAll("'", "''")}'`;
  }
}

/** T-SQL's clocks for the server expressions (`tsql.rs` `clock`). */
function clock(e: string, tz: boolean): string {
  if (e === "default") return "DEFAULT";
  if (e.startsWith("currentDate")) return "CAST(SYSDATETIME() AS DATE)";
  if (e.startsWith("currentTime")) return "CAST(SYSDATETIME() AS TIME)";
  if (tz) return "SYSDATETIMEOFFSET()";
  return e.startsWith("currentTimestamp") ? "CURRENT_TIMESTAMP" : "SYSDATETIME()";
}

/** The values a change writes, server expressions as T-SQL's clocks. */
function changeSql(t: Table, c: StagedChange): [string, string][] {
  if (c.op === "delete") return [];
  const values = c.op === "insert" ? c.values : c.changes;
  const out = new Map<string, string>();
  for (const [k, v] of Object.entries(values)) out.set(k, literal(v));
  for (const [k, e] of Object.entries(c.exprs ?? {})) {
    const col = t.cols.find((x) => x.name === k);
    out.set(k, clock(e, col?.kind === "timestampTz"));
  }
  return [...out].sort(([a], [b]) => a.localeCompare(b));
}

function keySql(c: StagedChange): string {
  if (c.op === "insert") return "";
  return Object.entries(c.key.value as Record<string, Value>)
    .map(([k, v]) => (v.t === "null" ? `${bracket(k)} IS NULL` : `${bracket(k)} = ${literal(v)}`))
    .join(" AND ");
}

/** Every raw filter of a group, nested groups included. */
function rawFilters(group: FilterGroup): string[] {
  return group.items.flatMap((n) =>
    n.kind === "raw" ? [n.sql] : n.kind === "group" ? rawFilters(n) : [],
  );
}

/** A `varbinary` as the grid's preview of it: 64 KiB at most (`Value::from_bytes`). */
const PREVIEW_CAP = 64 * 1024;
function preview(v: Value): Value {
  if (v.t !== "bytes" || v.v.len <= PREVIEW_CAP) return v;
  const head = atob(v.v.base64).slice(0, PREVIEW_CAP);
  return { t: "bytes", v: { base64: btoa(head), len: v.v.len, truncated: true } };
}

const now = () => new Date().toISOString().replace("T", " ").replace("Z", "0000");

/** A session's own state. */
export interface SqlServerOptions {
  databases: Map<string, Database>;
  database: string;
  readOnly: boolean;
  commandPassword: boolean;
  tls: boolean;
  version: string;
  login: string;
  application: string;
  spid: number;
}

/** One session to the mock's server. */
export function sqlserverSession(o: SqlServerOptions): MockSession {
  const caps = sqlserverCaps("dbo", o.commandPassword);
  const batch: Batch = {
    databases: o.databases,
    current: o.database,
    trancount: 0,
    snapshot: null,
    version: o.version,
    login: o.login,
    spid: o.spid,
  };
  const db = (): Database =>
    o.databases.get(o.database) ?? mockError("NOT_FOUND", `not found: database ${o.database}`);
  const tableOf = (ref: TableRef): Table =>
    tableIn(db(), ref.schema, ref.name) ??
    mockError("DB_ERROR", `Invalid object name '${ref.schema ?? "dbo"}.${ref.name}'.`);
  const qualified = (ref: TableRef) => `${bracket(ref.schema ?? "dbo")}.${bracket(ref.name)}`;

  /** `refuse_write`: a read-only session refuses before anything is sent. */
  const refuseWrite = (sql: string) => {
    if (o.readOnly && refusedReadOnly(sql)) mockError("INVALID_INPUT", READ_ONLY_MESSAGE);
  };

  /** The checks of the grid's reads (`query`): `refuse_write`, then `refuse_added_statement`. */
  const checkRead = (ref: TableRef, filter: FilterGroup | null) => {
    const raw = filter ? rawFilters(filter) : [];
    if (raw.length === 0) return;
    const sql = `SELECT COUNT_BIG(*) FROM ${qualified(ref)} WHERE ${raw.map((r) => `(${r})`).join(" AND ")}`;
    refuseWrite(sql);
    if (addsAStatement(sql)) mockError("INVALID_INPUT", ADDED_STATEMENT_MESSAGE);
  };

  const previewChanges = (req: ApplyRequest): string[] => {
    const t = tableOf(req.table);
    const name = qualified(req.table);
    return req.changes.map((c) => {
      if (c.op === "delete") return `DELETE FROM ${name} WHERE ${keySql(c)}`;
      const values = changeSql(t, c);
      if (c.op === "update")
        return `UPDATE ${name} SET ${values.map(([k, v]) => `${bracket(k)} = ${v}`).join(", ")} WHERE ${keySql(c)}`;
      return values.length === 0
        ? `INSERT INTO ${name} DEFAULT VALUES`
        : `INSERT INTO ${name} (${values.map(([k]) => bracket(k)).join(", ")}) VALUES (${values.map(([, v]) => v).join(", ")})`;
    });
  };

  return {
    caps,
    listDatabases: () =>
      [...o.databases.values()].map((d) => ({
        name: d.name,
        owner: d.owner,
        encoding: "SQL_Latin1_General_CP1_CI_AS",
        isCurrent: d.name === o.database,
      })),
    listSchemas: () => db().schemas.map((name) => ({ name, isDefault: name === "dbo" })),
    listTables: (schema) =>
      db()
        .tables.filter((t) => t.schema === schema)
        .sort((a, b) => a.name.localeCompare(b.name))
        .map((t) => ({
          name: t.name,
          schema: t.schema,
          kind: t.kind,
          rowEstimate: t.kind === "table" ? t.data.length : null,
          sizeBytes: t.kind === "table" ? 8192 * (2 + Math.ceil(t.data.length / 40)) : null,
          comment: t.comment,
        })),
    tableMeta: (ref) => {
      const t = tableOf(ref);
      return {
        table: { schema: t.schema, name: t.name },
        kind: t.kind,
        columns: columnsOf(t),
        primaryKey: t.pk,
        indexes: t.indexes,
        foreignKeys: t.foreignKeys,
        triggers: t.triggers,
        comment: t.comment,
        rowEstimate: t.kind === "table" ? t.data.length : null,
        ddl: null,
        checks: t.cols.flatMap((c) =>
          c.check
            ? [{ name: c.check.name, expression: c.check.expression, columns: [c.name] }]
            : [],
        ),
        options: null,
      };
    },
    tableDdl: (ref) => {
      const t = tableOf(ref);
      if (t.kind === "view") return t.viewSql ?? "";
      const name = qualified(ref);
      const cols = t.cols.map((c) => {
        const parts = [`    ${bracket(c.name)} ${c.type}`];
        if (c.identity) parts.push("IDENTITY(1,1)");
        if (c.collation) parts.push(`COLLATE ${c.collation}`);
        parts.push(c.nullable ? "NULL" : "NOT NULL");
        if (c.default) parts.push(`CONSTRAINT [DF_${t.name}_${c.name}] DEFAULT (${c.default})`);
        return parts.join(" ");
      });
      const pk = t.pk.length
        ? [`    CONSTRAINT [PK_${t.name}] PRIMARY KEY CLUSTERED (${t.pk.map(bracket).join(", ")})`]
        : [];
      const fks = t.foreignKeys.map(
        (fk) =>
          `    CONSTRAINT ${bracket(fk.name)} FOREIGN KEY (${fk.columns.map(bracket).join(", ")}) REFERENCES ${bracket(fk.refTable.schema ?? "dbo")}.${bracket(fk.refTable.name)} (${fk.refColumns.map(bracket).join(", ")})`,
      );
      const checks = t.cols.flatMap((c) =>
        c.check ? [`    CONSTRAINT ${bracket(c.check.name)} CHECK (${c.check.expression})`] : [],
      );
      const out = [
        `CREATE TABLE ${name} (\n${[...cols, ...pk, ...fks, ...checks].join(",\n")}\n);`,
      ];
      for (const i of t.indexes.filter((x) => !x.primary))
        out.push(
          `CREATE ${i.unique ? "UNIQUE " : ""}${i.method ?? "NONCLUSTERED"} INDEX ${bracket(i.name)} ON ${name} (${i.columns.map(bracket).join(", ")})${i.include?.length ? ` INCLUDE (${i.include.map(bracket).join(", ")})` : ""}${i.predicate ? ` WHERE ${i.predicate}` : ""};`,
        );
      for (const tr of t.triggers) out.push(`${tr.definition};`);
      return out.join("\nGO\n\n");
    },
    browse: (req): BrowsePage => {
      const t = tableOf(req.table);
      checkRead(req.table, req.filter);
      const columns = columnsOf(t);
      let rows = rowsOf(db(), t);
      if (req.filter) rows = rows.filter((row) => matchesGroup(req.filter!, row, columns));
      rows = sortRows(rows, req.sort, columns);
      const page = rows.slice(req.offset, req.offset + req.limit);
      const picked = req.columns ? columns.filter((c) => req.columns!.includes(c.name)) : columns;
      const at = picked.map((c) => columns.indexOf(c));
      const order = req.sort.length
        ? req.sort.map((s) => `${bracket(s.column)} ${s.dir.toUpperCase()}`).join(", ")
        : t.pk.length
          ? t.pk.map((k) => `${bracket(k)} ASC`).join(", ")
          : "(SELECT NULL)";
      return {
        columns: picked,
        rows: page.map((row) => at.map((i) => preview(row[i] ?? NULL))),
        pkColumns: t.pk,
        // Without a primary key a row cannot be edited (`RowLocator::KeyOnly`).
        identity: t.pk.length > 0 ? "pk" : "none",
        rowIds: null,
        offset: req.offset,
        limit: req.limit,
        sql: `SELECT ${picked.map((c) => (c.dataType === "money" ? `CONVERT(NVARCHAR(40), ${bracket(c.name)}, 2) AS ${bracket(c.name)}` : bracket(c.name))).join(", ")}\nFROM ${qualified(req.table)} ORDER BY ${order}\nOFFSET ${req.offset} ROWS FETCH NEXT ${req.limit} ROWS ONLY`,
        durationMs: 12,
      };
    },
    count: (ref, filter) => {
      const t = tableOf(ref);
      checkRead(ref, filter);
      const columns = columnsOf(t);
      const rows = rowsOf(db(), t).filter((row) => !filter || matchesGroup(filter, row, columns));
      return { count: rows.length, exact: true };
    },
    fetchCell: (ref, key, column) => {
      const t = tableOf(ref);
      const at = findRow(t, key);
      const col = t.cols.findIndex((c) => c.name === column);
      if (at < 0 || col < 0) mockError("NOT_FOUND", "not found: the row is gone");
      // The one statement that loads a binary value whole (`tsql::reads_one_cell`).
      return t.data[at]![col] ?? NULL;
    },
    preview: previewChanges,
    apply: (req): ApplyResult => {
      const sql = previewChanges(req);
      for (const s of sql) refuseWrite(s);
      const t = tableOf(req.table);
      if (t.kind === "view")
        mockError(
          "DB_ERROR",
          `View or function '${t.schema}.${t.name}' is not updatable because the modification affects multiple base tables.`,
        );
      if (req.changes.some((c) => c.op !== "insert" && c.key.kind !== "pk"))
        mockError(
          "UNSUPPORTED",
          "unsupported: Transact-SQL: a row of a table without a primary key cannot be edited",
        );
      const started = performance.now();
      const before = t.data.map((r) => r.slice());
      const nextId = t.nextId;
      const results: RowResult[] = [];
      const fail = (i: number, error: ErrorPayload): ApplyResult => {
        // One transaction: what ran before the failure is rolled back with it.
        t.data.splice(0, t.data.length, ...before);
        t.nextId = nextId;
        return {
          success: false,
          results: req.changes.map((x, j) => ({
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
      };
      for (const [i, c] of req.changes.entries()) {
        const values: Record<string, Value> =
          c.op === "delete" ? {} : { ...(c.op === "insert" ? c.values : c.changes) };
        for (const [k, e] of Object.entries(c.op === "delete" ? {} : (c.exprs ?? {})))
          values[k] =
            e === "default"
              ? { t: "default" }
              : ({
                  t:
                    t.cols.find((x) => x.name === k)?.kind === "timestampTz"
                      ? "timestampTz"
                      : "timestamp",
                  v: now(),
                } as Value);
        let broken: ServerFailure | null = null;
        if (c.op === "insert") {
          const row = t.cols.map((col) => {
            if (col.identity) return { t: "int", v: t.nextId } as Value;
            const v = values[col.name];
            return v === undefined || v.t === "default" ? defaultValue(col) : coerce(col, v);
          });
          broken = constraintError(db(), t, row, -1);
          if (!broken) {
            t.data.push(row);
            if (t.cols.some((col) => col.identity)) t.nextId++;
          }
        } else {
          const at = findRow(t, c.key);
          // `@@ROWCOUNT` after the statement is 0: the guard fails the transaction.
          if (at < 0)
            return fail(i, {
              ...serverError(0, GUARD_MESSAGE, 1, i),
              detail: null,
              vendorCode: null,
            });
          if (c.op === "delete") {
            for (const other of db().tables)
              for (const fk of other.foreignKeys) {
                if (fk.refTable.schema !== t.schema || fk.refTable.name !== t.name) continue;
                const ref = t.cols.findIndex((x) => x.name === fk.refColumns[0]);
                const own = other.cols.findIndex((x) => x.name === fk.columns[0]);
                if (other.data.some((r) => display(r[own]!) === display(t.data[at]![ref]!)))
                  broken = new ServerFailure(
                    547,
                    `The DELETE statement conflicted with the REFERENCE constraint "${fk.name}". The conflict occurred in database "${o.database}", table "${other.schema}.${other.name}", column '${fk.columns[0]}'.`,
                  );
              }
            if (!broken) t.data.splice(at, 1);
          } else {
            const row = t.data[at]!.slice();
            for (const [k, v] of Object.entries(values)) {
              const col = t.cols.findIndex((x) => x.name === k);
              if (col >= 0)
                row[col] = v.t === "default" ? defaultValue(t.cols[col]!) : coerce(t.cols[col]!, v);
            }
            broken = constraintError(db(), t, row, at);
            if (!broken) t.data[at] = row;
          }
        }
        if (broken) return fail(i, serverError(broken.code, broken.message, 1, i));
        results.push({ rowId: c.rowId, op: c.op, affected: 1, returned: null, error: null });
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
      // `run_statement`: only a read-only session checks what the editor sends.
      refuseWrite(statement);
      // A USE moves only the editor's own connection: the catalog, the grid and the other
      // workers log in to the profile's database (`batch.current` is the editor's).
      runOne(statement, batch, emit);
    },
    listProcesses: () => processes(o),
    killProcesses: (ids, scope) =>
      ids.map((id) => {
        if (o.readOnly) return { id, ok: false, message: READ_ONLY_MESSAGE };
        if (scope === "query")
          return {
            id,
            ok: false,
            message:
              "unsupported: SQL Server ends a whole session, not one of its queries: use Kill Connection",
          };
        // `kill_one`: the id as an i64 (`AppError::InvalidInput` prints its text alone), then
        // `tsql::kill`'s smallint range.
        const spid = /^[+-]?\d+$/.test(id.trim()) ? BigInt(id.trim()) : null;
        if (spid === null || spid > 2n ** 63n - 1n || spid < -(2n ** 63n))
          return { id, ok: false, message: `${id} is not a session id` };
        if (spid < 1n || spid > 32767n)
          return { id, ok: false, message: `${spid} is not a user session id` };
        const at = OTHERS.findIndex((p) => p.id === String(spid));
        if (Number(spid) === o.spid)
          return { id, ok: false, message: "Cannot use KILL to kill your own process." };
        if (at < 0)
          return { id, ok: false, message: `Process ID ${spid} is not an active process ID.` };
        OTHERS.splice(at, 1);
        return { id, ok: true, message: null };
      }),
    listRoutines: (schema) =>
      db()
        .routines.filter((r) => r.schema === schema)
        .map(({ definition: _definition, ...summary }) => summary),
    routineDefinition: (ref) => {
      const r = db().routines.find((x) => x.schema === ref.schema && x.name === ref.name);
      if (!r) return mockError("NOT_FOUND", `not found: routine ${ref.name}`);
      return (
        r.definition ??
        "-- The definition is not available: the object was created WITH ENCRYPTION or is a CLR one."
      );
    },
    sessionDetails: () => ({ tls: o.tls, tlsVersion: null, tlsCipher: null, encoding: "UTF-16" }),
  };
}

/** A column's default for a staged insert. */
function defaultValue(c: Table["cols"][number]): Value {
  if (c.default === undefined) return NULL;
  if (c.default === "newid()") return { t: "uuid", v: crypto.randomUUID().toUpperCase() };
  if (c.default === "sysdatetime()") return { t: "timestamp", v: now() };
  const s = /^N?'(.*)'$/.exec(c.default);
  return coerce(c, { t: "text", v: s ? s[1]! : c.default });
}

/** Other sessions of the server, for Running Queries (a kill ends one). */
const OTHERS: ProcessRow[] = [];
function seedOthers() {
  if (OTHERS.length > 0) return;
  OTHERS.push(
    {
      id: "61",
      user: "app_shop",
      database: "shop",
      application: "Core Microsoft SqlClient Data Provider",
      client: "10.0.4.17",
      state: "running",
      command: "SELECT",
      startedAt: "2026-10-31 09:12:04.117",
      durationMs: 4210,
      waitEvent: "PAGEIOLATCH_SH",
      query: "SELECT o.id, o.total FROM dbo.orders AS o WHERE o.status = @P1",
      current: false,
      own: false,
    },
    {
      id: "64",
      user: "etl",
      database: "reporting",
      application: "SQLAgent - TSQL JobStep",
      client: "10.0.4.30",
      state: "suspended",
      command: "INSERT",
      startedAt: "2026-10-31 09:10:41.003",
      durationMs: 87_120,
      waitEvent: "LCK_M_IX",
      query: "INSERT INTO dbo.daily_sales (day, orders, revenue) SELECT …",
      current: false,
      own: false,
    },
    {
      id: "70",
      user: "sa",
      database: "master",
      application: "Microsoft SQL Server Management Studio",
      client: "10.0.4.2",
      state: "sleeping",
      command: "AWAITING COMMAND",
      startedAt: null,
      durationMs: null,
      waitEvent: null,
      query: null,
      current: false,
      own: false,
    },
  );
}

/** Tests: the other sessions as seeded again. */
export function resetProcesses(): void {
  OTHERS.splice(0);
}

function processes(o: SqlServerOptions): ProcessRow[] {
  seedOthers();
  return [
    {
      id: String(o.spid),
      user: o.login,
      database: o.database,
      application: o.application,
      client: "127.0.0.1",
      state: "running",
      command: "SELECT",
      startedAt: now().slice(0, 23),
      durationMs: 0,
      waitEvent: null,
      query: "SELECT s.session_id, … FROM sys.dm_exec_sessions AS s …",
      current: true,
      own: true,
    },
    ...OTHERS.map((p) => ({ ...p })),
  ];
}

/** Runs one GO batch as the editor's connection does, streaming its events. */
function runOne(sql: string, batch: Batch, emit: (e: QueryEvent) => void) {
  const span = { start: 0, end: sql.length, line: 1 };
  const started = performance.now();
  emit({ event: "started", data: { index: 0, sql, span } });
  const base: StatementResult = {
    index: 0,
    sql,
    span,
    kind: "empty",
    columns: [],
    resultSets: 0,
    rowCount: 0,
    truncated: false,
    affected: null,
    lastInsertId: null,
    startedAt: new Date().toISOString(),
    durationMs: 1,
    error: null,
    notices: [],
  };
  let outcome: ReturnType<typeof runBatch>;
  try {
    outcome = runBatch(sql, batch);
  } catch (err) {
    if (!(err instanceof ServerFailure)) throw err;
    emit({ event: "failed", data: { index: 0, error: serverError(err.code, err.message, 1, 0) } });
    return;
  }
  const durationMs = Math.max(1, Math.round(performance.now() - started));
  if (outcome.kind === "rows") {
    outcome.sets.forEach((set, i) => {
      emit({ event: "columns", data: { index: 0, set: i, columns: set.columns } });
      emit({
        event: "rows",
        data: { index: 0, set: i, rows: set.rows.map((r) => r.map(preview)) },
      });
    });
    emit({
      event: "finished",
      data: {
        index: 0,
        result: {
          ...base,
          kind: "rows",
          columns: outcome.sets[0]!.columns,
          resultSets: outcome.sets.length,
          rowCount: outcome.sets.reduce((n, s) => n + s.rows.length, 0),
          durationMs,
        },
      },
    });
    return;
  }
  emit({
    event: "finished",
    data: {
      index: 0,
      result:
        outcome.kind === "affected"
          ? { ...base, kind: "affected", affected: outcome.affected, durationMs }
          : { ...base, durationMs },
    },
  });
}
