import { classifyDialect, codeOnly, isSingleStatement } from "@/features/drivers/portable/classify";
import { scan, split } from "@/features/drivers/portable/scanner";
import { classifyStatement } from "@/features/safety/classify";
import type { ApplyRequest } from "../../../bindings/ApplyRequest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import type { DialectLex } from "../../../bindings/DialectLex";
import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { ResultColumn } from "../../../bindings/ResultColumn";
import type { RowKey } from "../../../bindings/RowKey";
import type { RowResult } from "../../../bindings/RowResult";
import type { StagedChange } from "../../../bindings/StagedChange";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { TableRef } from "../../../bindings/TableRef";
import type { UserSummary } from "../../../bindings/UserSummary";
import type { Value } from "../../../bindings/Value";
import REDSHIFT_LEX from "../../../bindings/lex/redshift.json";
import { matchesGroup, sortRows } from "../../rows";
import { mockError } from "../../transport";
import type { MockSession } from "../types";
import { type Engine, flavorCaps, notAvailable } from "./caps";
import { columnMeta, engineData, NULL, pkOf, type Table } from "./data";

/**
 * A session of the mock's CockroachDB, Greenplum (6 or 7) or Redshift, mirroring a PostgreSQL
 * session with this lane's hooks: the caps of `flavorCaps`, the catalog of `data.ts`, offset pages,
 * exact counts, commits in one transaction that a failing change (a NULL in a NOT NULL column, a
 * row someone else removed) rolls back whole, rows read back only where the caps grant
 * `Returning`, and Redshift SQL (no RETURNING, no SAVEPOINT) on Redshift. A read-only profile is
 * refused where the backend refuses it: grid edits always, editor writes by the session's
 * classifier (Redshift SQL on Redshift, PostgreSQL's elsewhere). What the caps leave out has no
 * method, so the mock's handlers refuse it.
 */

const LEX = REDSHIFT_LEX as DialectLex;

/** The backend's `READ_ONLY_MESSAGE` (`commands::safety`). */
export const READ_ONLY_MESSAGE = "This connection is read-only";

/** `mutate::plan_rows`' refusal on a read-only connection. */
export const READ_ONLY_EDIT = "unsupported: this connection is read-only";

/** `imp/redshift.rs::check_init`'s refusal of a read-only profile's writing startup commands. */
export const READ_ONLY_STARTUP =
  "This connection is read-only, and Redshift has no read-only setting to stop a startup command that may write. Remove the commands that write from Startup commands, or turn read-only off.";

/** `check_init`'s refusal of a session that only its encoding made read-only. */
export const readOnlyByEncoding = (encoding: string): string =>
  `Redshift speaks only UTF-8, and the encoding ${encoding} would make this connection read-only with nothing on the server to enforce it. Set Encoding to UTF-8, or turn read-only on.`;

/** `conn_options::is_utf8_encoding`'s names. */
const UTF8_ENCODINGS = ["utf8", "utf-8", "utf8mb3", "utf8mb4", "unicode"];

/**
 * The encoding that makes a PostgreSQL-driver session read-only (`connect::read_only_session`):
 * the profile's, when it is a valid name that is not UTF-8.
 */
export function nonUtf8Encoding(profile: ConnectionProfile): string | null {
  const encoding = profile.options.encoding?.trim() ?? "";
  const valid = encoding.length > 0 && encoding.length <= 40 && /^[A-Za-z0-9_-]+$/.test(encoding);
  return valid && !UTF8_ENCODINGS.includes(encoding.toLowerCase()) ? encoding : null;
}

/** `imp/redshift.rs::startup_lexes`: Redshift SQL's lexer, and the same without dollar quotes. */
const STARTUP_LEXES: DialectLex[] = [
  LEX,
  { ...LEX, lex: { ...LEX.lex, dollarQuotes: false, dollarTags: false } },
];

/**
 * Whether a Redshift startup command writes (`imp/redshift.rs::startup_writes`): SET and RESET
 * stay where every reading sees one statement; anything else is the classifier's. The text is
 * split under each reading on its own, since a `;` one reading sees inside a string ends a
 * statement to the other. A comment a server may end elsewhere is looked for in the whole text,
 * since `split` leaves the comments around a statement out of its text.
 */
export function startupWrites(text: string): boolean {
  return STARTUP_LEXES.some((dialect) =>
    dialect.lex.readings.some(
      (r) =>
        scan(dialect.lex, r, text).ambiguous ||
        // `split` cuts under a lexer's first reading: one lexer per reading.
        split({ ...dialect.lex, readings: [r] }, text).some(({ sql }) => {
          const first = codeOnly(dialect.lex, sql).trim().split(/\s+/)[0]?.toUpperCase();
          const setting =
            (first === "SET" || first === "RESET") &&
            isSingleStatement(dialect.lex, dialect.classify, sql);
          return !setting && classifyDialect(dialect, sql) === "write";
        }),
    ),
  );
}

/** `drivers::check_affected`'s words. */
export const GUARD_MESSAGE =
  "expected to affect 1 row(s) but matched 0; the row may have been changed or deleted by someone else";

/** What `SELECT version()` answers on each engine (`imp/testkit.rs`). */
export const VERSION_TEXT: Record<Engine, string> = {
  cockroachdb: "CockroachDB CCL v24.2.4 (x86_64-pc-linux-gnu, built 2024/10/14, go1.22.5)",
  greenplum6:
    "PostgreSQL 9.4.26 (Greenplum Database 6.27.1 build commit:1d9f0a3) on x86_64-unknown-linux-gnu, compiled by gcc",
  greenplum7: "PostgreSQL 12.12 (Greenplum Database 7.1.0 build commit:e7c2b1f) on x86_64",
  redshift:
    "PostgreSQL 8.0.2 on i686-pc-linux-gnu, compiled by GCC gcc (GCC) 3.4.2 20041017 (Red Hat 3.4.2-6.fc3), Redshift 1.0.77467",
};

const quote = (name: string) => `"${name.replaceAll('"', '""')}"`;
const qualified = (t: Table) => `${quote(t.schema)}.${quote(t.name)}`;

/** A value as SQL text for Preview (Redshift reads backslashes in strings, so they double). */
function literal(engine: Engine, v: Value, type: string): string {
  switch (v.t) {
    case "null":
      return "NULL";
    case "default":
      return "DEFAULT";
    case "bool":
      return v.v ? "TRUE" : "FALSE";
    case "int":
    case "float":
      return String(v.v);
    case "bigInt":
    case "decimal":
      return v.v;
    default: {
      const raw = typeof v.v === "string" ? v.v : JSON.stringify(v.v);
      const escaped = raw.replaceAll("'", "''");
      if (engine !== "redshift") return `'${escaped}'`;
      const s = `'${escaped.replaceAll("\\", "\\\\")}'`;
      return type === "super" ? `JSON_PARSE(${s})` : s;
    }
  }
}

const same = (a: Value | undefined, b: Value) => JSON.stringify(a) === JSON.stringify(b);

function serverError(
  message: string,
  sqlstate: string,
  statementIndex: number | null,
): ErrorPayload {
  return {
    code: "DB_ERROR",
    message,
    detail: null,
    hint: null,
    sqlstate,
    vendorCode: null,
    position: null,
    statementIndex,
    data: null,
  };
}

/** The session of `profile` on `engine`. */
export function flavorSession(engine: Engine, profile: ConnectionProfile): MockSession {
  const caps = flavorCaps(engine);
  const has = (f: (typeof caps.features)[number]) => caps.features.includes(f);
  const data = engineData(engine);
  const readOnly = profile.options.readOnly;
  // The backend refuses the connect rather than leave a command out (`check_init`). A non-UTF-8
  // encoding makes the session read-only (`connect::read_only_session`) with nothing on Redshift
  // to enforce it, so that is refused whatever the startup commands are.
  if (engine === "redshift" && !readOnly) {
    const encoding = nonUtf8Encoding(profile);
    if (encoding !== null) mockError("INVALID_INPUT", readOnlyByEncoding(encoding));
  }
  if (engine === "redshift" && readOnly && startupWrites(profile.options.startupSql ?? ""))
    mockError("INVALID_INPUT", READ_ONLY_STARTUP);
  const database = profile.database ?? data.databases[0]!;

  const table = (ref: TableRef): Table =>
    data.tables.find((t) => t.name === ref.name && t.schema === (ref.schema ?? "public")) ??
    mockError("NOT_FOUND", `not found: table ${ref.schema ?? "public"}.${ref.name}`);

  const meta = (t: Table): TableMeta => {
    const pk = pkOf(t);
    return {
      table: { schema: t.schema, name: t.name },
      kind: t.kind,
      columns: t.cols.map((c, i) => columnMeta(c, i, pk)),
      primaryKey: pk,
      indexes: pk.length
        ? [
            {
              name: `${t.name}_pkey`,
              columns: pk,
              unique: true,
              primary: true,
              method: engine === "redshift" ? null : "btree",
              predicate: null,
              definition: null,
              comment: null,
            },
          ]
        : [],
      foreignKeys: [],
      triggers: [],
      comment: t.comment,
      rowEstimate: t.rows.length,
      ddl: null,
      checks: [],
      options: t.options,
    };
  };

  const rowAt = (t: Table, key: RowKey): number => {
    if (key.kind === "rowid") return -1;
    return t.rows.findIndex((row) =>
      Object.entries(key.value).every(([name, v]) =>
        same(row[t.cols.findIndex((c) => c.name === name)], v),
      ),
    );
  };

  const where = (t: Table, key: RowKey) =>
    key.kind === "rowid"
      ? "ctid = …"
      : Object.entries(key.value)
          .map(([name, v]) => {
            const type = t.cols.find((c) => c.name === name)?.type ?? "";
            return v.t === "null"
              ? `${quote(name)} IS NULL`
              : `${quote(name)} = ${literal(engine, v, type)}`;
          })
          .join(" AND ");

  const typeOf = (t: Table, name: string) => t.cols.find((c) => c.name === name)?.type ?? "";

  const returning = (req: ApplyRequest) => req.returning && has("returning");

  const statement = (t: Table, c: StagedChange, back: boolean): string => {
    const tail = back ? " RETURNING *" : "";
    if (c.op === "delete") return `DELETE FROM ${qualified(t)} WHERE ${where(t, c.key)}`;
    if (c.op === "update") {
      const set = Object.entries(c.changes)
        .map(([k, v]) => `${quote(k)} = ${literal(engine, v, typeOf(t, k))}`)
        .join(", ");
      return `UPDATE ${qualified(t)} SET ${set} WHERE ${where(t, c.key)}${tail}`;
    }
    const cols = Object.entries(c.values);
    return `INSERT INTO ${qualified(t)} (${cols.map(([k]) => quote(k)).join(", ")}) VALUES (${cols
      .map(([k, v]) => literal(engine, v, typeOf(t, k)))
      .join(", ")})${tail}`;
  };

  /** `plan_rows`: refused on a read-only connection, and by the features the changes need. */
  const plan = (req: ApplyRequest): { t: Table; sql: string[] } => {
    if (readOnly) mockError("UNSUPPORTED", READ_ONLY_EDIT);
    const t = table(req.table);
    for (const c of req.changes) {
      const need = c.op === "insert" ? "insertRows" : c.op === "delete" ? "deleteRows" : "editRows";
      if (!has(need)) mockError("UNSUPPORTED", notAvailable(engine, need));
      if (c.op !== "insert" && c.key.kind !== "pk" && !has("editWithoutKey"))
        mockError("UNSUPPORTED", notAvailable(engine, "editing rows without a primary key"));
    }
    return { t, sql: req.changes.map((c) => statement(t, c, returning(req))) };
  };

  const notNull = (t: Table, row: Value[]): string | null => {
    const at = t.cols.findIndex((c, i) => c.nullable === false && !c.pk && row[i]?.t === "null");
    return at < 0
      ? null
      : `null value in column "${t.cols[at]!.name}" violates not-null constraint`;
  };

  const apply = (req: ApplyRequest): ApplyResult => {
    const { t, sql } = plan(req);
    const before = t.rows.map((r) => r.slice());
    const results: RowResult[] = [];
    const back = returning(req);
    const asRecord = (row: Value[]) =>
      back ? Object.fromEntries(t.cols.map((c, i) => [c.name, row[i] ?? NULL])) : null;
    for (const [i, c] of req.changes.entries()) {
      let failure: string | null = null;
      let sqlstate = "23502";
      let row: Value[] | null = null;
      if (c.op === "insert") {
        row = t.cols.map((col, k) => {
          const v = c.values[col.name];
          if (v && v.t !== "default") return v;
          if (col.auto)
            return col.kind === "uuid"
              ? { t: "uuid", v: crypto.randomUUID() }
              : { t: "int", v: t.rows.length + 1000 + k };
          return NULL;
        });
        failure = notNull(t, row);
        if (!failure) t.rows.push(row);
      } else {
        const at = rowAt(t, c.key);
        if (at < 0) {
          failure = GUARD_MESSAGE;
          sqlstate = "";
        } else if (c.op === "delete") t.rows.splice(at, 1);
        else {
          row = t.rows[at]!.slice();
          for (const [name, v] of Object.entries(c.changes)) {
            const k = t.cols.findIndex((col) => col.name === name);
            if (k >= 0) row[k] = v.t === "default" ? NULL : v;
          }
          failure = notNull(t, row);
          if (!failure) t.rows[at] = row;
        }
      }
      if (failure) {
        // One transaction: the failing change rolls back the ones before it.
        t.rows = before;
        const error = serverError(failure, sqlstate, i);
        return {
          success: false,
          results: req.changes.map((ch, j) => ({
            rowId: ch.rowId,
            op: ch.op,
            affected: 0,
            returned: null,
            error: j === i ? error : null,
          })),
          sql,
          error,
          durationMs: 3,
        };
      }
      results.push({
        rowId: c.rowId,
        op: c.op,
        affected: 1,
        returned: row && c.op !== "delete" ? asRecord(row) : null,
        error: null,
      });
    }
    return { success: true, results, sql, error: null, durationMs: 3 };
  };

  const writes = (sql: string) =>
    engine === "redshift"
      ? classifyDialect(LEX, sql) === "write"
      : classifyStatement(sql, "postgres") === "write";

  const run = (sql: string, emit: (event: QueryEvent) => void) => {
    if (readOnly && writes(sql)) mockError("INVALID_INPUT", READ_ONLY_MESSAGE);
    const span = { start: 0, end: sql.length, line: 1 };
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
      durationMs: 2,
      error: null,
      notices: [],
    };
    emit({ event: "started", data: { index: 0, sql, span } });
    const fail = (message: string, sqlstate: string) =>
      emit({ event: "failed", data: { index: 0, error: serverError(message, sqlstate, 0) } });
    const rows = (columns: ResultColumn[], values: Value[][]) => {
      emit({ event: "columns", data: { index: 0, set: 0, columns } });
      emit({ event: "rows", data: { index: 0, set: 0, rows: values } });
      const result = {
        ...base,
        kind: "rows" as const,
        columns,
        resultSets: 1,
        rowCount: values.length,
      };
      emit({ event: "finished", data: { index: 0, result } });
    };
    const text = sql.trim();
    // Redshift has no savepoints and no RETURNING (the reason for `row_cap_savepoint`).
    if (engine === "redshift" && /^savepoint\b/i.test(text))
      return fail("SAVEPOINT is not supported", "0A000");
    if (engine === "redshift" && /\breturning\b/i.test(text))
      return fail('syntax error at or near "RETURNING"', "42601");
    if (/^select\s+version\s*\(\s*\)\s*;?$/i.test(text))
      return rows(
        [{ name: "version", dataType: "text", kind: "text", nullable: true, table: null }],
        [[{ t: "text", v: VERSION_TEXT[engine] }]],
      );
    const select =
      /^select\s+\*\s+from\s+(?:"?(\w+)"?\.)?"?(\w+)"?(?:\s+limit\s+(\d+))?\s*;?$/i.exec(text);
    if (select) {
      const t = data.tables.find(
        (x) =>
          x.name === select[2]!.toLowerCase() &&
          x.schema === (select[1]?.toLowerCase() ?? "public"),
      );
      if (!t) return fail(`relation "${select[2]!}" does not exist`, "42P01");
      const limit = select[3] ? Number(select[3]) : t.rows.length;
      return rows(
        t.cols.map((c) => ({
          name: c.name,
          dataType: c.type,
          kind: c.kind,
          nullable: c.nullable ?? true,
          table: t.name,
        })),
        t.rows.slice(0, limit),
      );
    }
    const del = /^delete\s+from\s+(?:"?(\w+)"?\.)?"?(\w+)"?\s*;?$/i.exec(text);
    if (del) {
      const t = data.tables.find((x) => x.name === del[2]!.toLowerCase());
      if (!t) return fail(`relation "${del[2]!}" does not exist`, "42P01");
      const affected = t.rows.length;
      t.rows = [];
      emit({
        event: "finished",
        data: { index: 0, result: { ...base, kind: "affected", affected } },
      });
      return;
    }
    emit({
      event: "finished",
      data: {
        index: 0,
        result: {
          ...base,
          kind: writes(sql) ? "affected" : "empty",
          affected: writes(sql) ? 0 : null,
        },
      },
    });
  };

  const users: UserSummary[] = [
    { name: "gpadmin", host: null, canLogin: true, superuser: true },
    { name: "etl", host: null, canLogin: true, superuser: false },
    { name: "analysts", host: null, canLogin: false, superuser: false },
  ];

  const session: MockSession = {
    caps,
    listDatabases: () =>
      data.databases.map((name) => ({
        name,
        owner: engine === "redshift" ? "rdsdb" : engine === "cockroachdb" ? "root" : "gpadmin",
        encoding: "UTF8",
        isCurrent: name === database,
      })),
    listSchemas: () => data.schemas.map((name) => ({ name, isDefault: name === "public" })),
    listTables: (schema) =>
      data.tables
        .filter((t) => t.schema === schema)
        .map((t) => ({
          name: t.name,
          schema: t.schema,
          kind: t.kind,
          rowEstimate: t.rows.length,
          sizeBytes: t.sizeBytes,
          comment: t.comment,
        })),
    tableMeta: (ref) => meta(table(ref)),
    tableDdl: (ref) => table(ref).ddl,
    browse: (req) => {
      const t = table(req.table);
      const m = meta(t);
      let all = req.filter ? t.rows.filter((r) => matchesGroup(req.filter!, r, m.columns)) : t.rows;
      all = sortRows(all, req.sort, m.columns);
      const page = all.slice(req.offset, req.offset + req.limit);
      const cols = req.columns ? m.columns.filter((c) => req.columns!.includes(c.name)) : m.columns;
      const idx = cols.map((c) => m.columns.indexOf(c));
      const order = req.sort.length
        ? ` ORDER BY ${req.sort.map((s) => `${quote(s.column)} ${s.dir.toUpperCase()}`).join(", ")}`
        : "";
      // Redshift reads SUPER through JSON_SERIALIZE (`sqlgen::dialect::redshift`).
      const list = cols
        .map((c) =>
          engine === "redshift" && c.dataType === "super"
            ? `JSON_SERIALIZE(${quote(c.name)}) AS ${quote(c.name)}`
            : quote(c.name),
        )
        .join(", ");
      return {
        columns: cols,
        rows: page.map((r) => idx.map((i) => r[i] ?? NULL)),
        pkColumns: m.primaryKey,
        identity: m.primaryKey.length ? "pk" : "none",
        rowIds: null,
        offset: req.offset,
        limit: req.limit,
        sql: `SELECT ${list} FROM ${qualified(t)}${order} LIMIT ${req.limit} OFFSET ${req.offset}`,
        durationMs: 4,
      };
    },
    count: (ref, filter) => {
      const t = table(ref);
      const m = meta(t);
      const rows = filter ? t.rows.filter((r) => matchesGroup(filter, r, m.columns)) : t.rows;
      return { count: rows.length, exact: true };
    },
    fetchCell: (ref, key, column) => {
      const t = table(ref);
      const at = rowAt(t, key);
      const k = t.cols.findIndex((c) => c.name === column);
      if (at < 0 || k < 0) mockError("NOT_FOUND", "not found: the row is gone");
      return t.rows[at]![k] ?? NULL;
    },
    preview: (req) => plan(req).sql,
    apply,
    run,
    listProcesses: () =>
      data.processes.map((p) => ({
        id: p.id,
        user: p.user,
        database,
        application: p.application,
        client: "10.0.0.12",
        state: p.state,
        command: null,
        startedAt: "2026-11-01 09:00:00+00",
        durationMs: 1200,
        waitEvent: null,
        query: p.query,
        current: p.application === "tabler",
        own: p.application === "tabler",
      })),
    killProcesses: (ids) =>
      ids.map((id) => {
        // CockroachDB's session ids are hexadecimal; the others' process ids integers.
        const valid =
          engine === "cockroachdb" ? /^[0-9a-fA-F]+$/.test(id.trim()) : /^-?\d+$/.test(id.trim());
        if (!valid)
          return {
            id,
            ok: false,
            message:
              engine === "cockroachdb" ? `invalid session id ${id}` : `invalid process id ${id}`,
          };
        const at = data.processes.findIndex((p) => p.id === id.trim());
        if (at < 0) return { id, ok: false, message: "No such process: it may have ended already" };
        data.processes.splice(at, 1);
        return { id, ok: true, message: null };
      }),
    listRoutines: (schema) =>
      data.routines.filter((r) => r.schema === schema).map(({ definition: _d, ...r }) => r),
    routineDefinition: (ref) => {
      const r = data.routines.find(
        (x) =>
          x.schema === ref.schema &&
          x.name === ref.name &&
          x.kind === ref.kind &&
          x.identityArguments === ref.identityArguments,
      );
      if (!r)
        return mockError("NOT_FOUND", `not found: routine ${ref.name}(${ref.identityArguments})`);
      if (r.definition.startsWith("!"))
        return mockError("UNSUPPORTED", `unsupported: ${r.definition.slice(1)}`);
      return r.definition;
    },
  };

  if (has("users")) {
    session.listUsers = (includeNologin) => users.filter((u) => includeNologin || u.canLogin);
    session.loadUser = (ref) => {
      const u = users.find((x) => x.name === ref.name);
      if (!u) return mockError("NOT_FOUND", `not found: role ${ref.name}`);
      return {
        state: {
          name: u.name,
          host: null,
          globalPrivileges: [],
          limits: null,
          attributes: {
            superuser: u.superuser,
            createDb: u.superuser,
            createRole: u.superuser,
            login: u.canLogin,
            replication: u.superuser,
            inherit: true,
            connectionLimit: -1,
            validUntil: null,
          },
          databasePrivileges: [{ object: database, privileges: ["CONNECT"] }],
          schemaPrivileges:
            u.name === "etl" ? [{ object: "sales", privileges: ["USAGE", "CREATE"] }] : [],
          tablePrivileges:
            u.name === "etl" ? [{ object: "sales", privileges: ["INSERT", "SELECT"] }] : [],
          partialTablePrivileges: [],
        },
        schemas: data.schemas,
      };
    };
  }
  if (has("sessionDetails"))
    session.sessionDetails = () => {
      const tls = profile.ssl !== "disable";
      return {
        tls,
        tlsVersion: tls ? "TLSv1.3" : null,
        tlsCipher: tls ? "TLS_AES_256_GCM_SHA384" : null,
        encoding: "UTF8",
      };
    };
  return session;
}
