// Mock handlers for the sidebar objects (M8): list_routines, list_sequences, list_system_schemas,
// list_database_items, get_routine_definition, get_sequence_definition, get_item_overview, preview_object_op and
// apply_object_op. Table changes (drop / truncate / clone) go through the DDL mock (`apply_ddl`),
// so the table list and metadata follow; routines, sequences and comments live here.
import type { ColumnDef } from "../bindings/ColumnDef";
import type { ConnectionInfo } from "../bindings/ConnectionInfo";
import type { DdlOp } from "../bindings/DdlOp";
import type { DdlPlan } from "../bindings/DdlPlan";
import type { DdlResult } from "../bindings/DdlResult";
import type { Driver } from "../bindings/Driver";
import type { ObjectOp } from "../bindings/ObjectOp";
import type { ObjectTarget } from "../bindings/ObjectTarget";
import type { OverviewItem } from "../bindings/OverviewItem";
import type { RoutineRef } from "../bindings/RoutineRef";
import type { RoutineSummary } from "../bindings/RoutineSummary";
import type { SchemaInfo } from "../bindings/SchemaInfo";
import type { SequenceSummary } from "../bindings/SequenceSummary";
import type { TableKind } from "../bindings/TableKind";
import type { TableMeta } from "../bindings/TableMeta";
import type { TableRef } from "../bindings/TableRef";
import type { TableSummary } from "../bindings/TableSummary";
// The DDL mock registers apply_ddl / list_tables / get_table, which this file calls.
import "./ddl";
import { mockError, mockInvoke, registerMock } from "./transport";

type Routine = Omit<RoutineSummary, "schema"> & { body: string };

const PG_ROUTINES: Record<string, Routine[]> = {
  public: [
    {
      name: "add",
      kind: "function",
      arguments: "a integer, b integer",
      identityArguments: "a integer, b integer",
      returnType: "integer",
      language: "sql",
      comment: null,
      body: "SELECT a + b",
    },
    {
      name: "add",
      kind: "function",
      arguments: "a numeric, b numeric",
      identityArguments: "a numeric, b numeric",
      returnType: "numeric",
      language: "sql",
      comment: null,
      body: "SELECT a + b",
    },
    {
      name: "refresh_post_counts",
      kind: "procedure",
      arguments: "",
      identityArguments: "",
      returnType: null,
      language: "plpgsql",
      comment: "Rebuilds post_counts_mv",
      body: "BEGIN\n  REFRESH MATERIALIZED VIEW public.post_counts_mv;\nEND;",
    },
    {
      name: "slugify",
      kind: "function",
      arguments: "value text",
      identityArguments: "value text",
      returnType: "text",
      language: "sql",
      comment: "URL-safe form of a title",
      body: "SELECT lower(regexp_replace(value, '[^a-zA-Z0-9]+', '-', 'g'))",
    },
    {
      name: "total_revenue",
      kind: "aggregate",
      arguments: "numeric",
      identityArguments: "numeric",
      returnType: "numeric",
      language: "internal",
      comment: null,
      body: "",
    },
    {
      name: "touch_updated_at",
      kind: "function",
      arguments: "",
      identityArguments: "",
      returnType: "trigger",
      language: "plpgsql",
      comment: null,
      body: "BEGIN\n  NEW.updated_at := now();\n  RETURN NEW;\nEND;",
    },
  ],
  inventory: [
    {
      name: "restock",
      kind: "procedure",
      arguments: "item integer, qty integer",
      identityArguments: "item integer, qty integer",
      returnType: null,
      language: "plpgsql",
      comment: null,
      body: "BEGIN\n  UPDATE inventory.items SET stock = stock + qty WHERE id = item;\nEND;",
    },
  ],
};

const MYSQL_ROUTINES: Routine[] = [
  {
    name: "calc_total",
    kind: "function",
    arguments: "order_id int",
    identityArguments: "",
    returnType: "decimal(10,2)",
    language: "SQL",
    comment: null,
    body: "BEGIN\n  DECLARE total DECIMAL(10,2);\n  SELECT SUM(price) INTO total FROM order_items WHERE id = order_id;\n  RETURN IFNULL(total, 0);\nEND",
  },
  {
    name: "archive_old_posts",
    kind: "procedure",
    arguments: "IN days int",
    identityArguments: "",
    returnType: null,
    language: "SQL",
    comment: "Nightly cleanup",
    body: "BEGIN\n  DELETE FROM posts WHERE created_at < NOW() - INTERVAL days DAY;\n  SELECT ROW_COUNT() AS archived;\nEND",
  },
];

const PG_SEQUENCES: Record<string, Array<Omit<SequenceSummary, "schema">>> = {
  public: [
    { name: "invoice_number_seq", dataType: "bigint", ownedBy: null, comment: null },
    { name: "posts_id_seq", dataType: "bigint", ownedBy: "posts.id", comment: null },
    { name: "users_id_seq", dataType: "bigint", ownedBy: "users.id", comment: null },
  ],
  inventory: [{ name: "items_id_seq", dataType: "integer", ownedBy: "items.id", comment: null }],
};

/** `${connectionId}|${key}` of dropped routines / sequences. */
const dropped = new Set<string>();
/** `${driver}:${schema}.${name}` → comment set through the overview. */
const comments = new Map<string, string | null>();

/** Test helper: forget dropped objects and changed comments. */
export function resetMockObjects(): void {
  dropped.clear();
  comments.clear();
}

async function connectionOf(connectionId: unknown): Promise<ConnectionInfo | null> {
  const all = await mockInvoke<ConnectionInfo[]>("list_connections", {});
  return all.find((c) => c.connectionId === connectionId) ?? null;
}

async function requireConnection(connectionId: unknown): Promise<ConnectionInfo> {
  return (await connectionOf(connectionId)) ?? mockError("NOT_CONNECTED", "not connected");
}

const routineId = (schema: string | null, name: string, args: string) =>
  `f:${schema ?? ""}.${name}(${args})`;

function routinesFor(c: ConnectionInfo, schema: string): RoutineSummary[] {
  const list =
    c.driver === "postgres"
      ? (PG_ROUTINES[schema] ?? [])
      : c.driver === "mysql"
        ? MYSQL_ROUTINES
        : [];
  return list
    .map(({ body: _body, ...r }) => ({ ...r, schema }))
    .filter(
      (r) => !dropped.has(`${c.connectionId}|${routineId(schema, r.name, r.identityArguments)}`),
    );
}

function sequencesFor(c: ConnectionInfo, schema: string): SequenceSummary[] {
  if (c.driver !== "postgres") return [];
  return (PG_SEQUENCES[schema] ?? [])
    .map((s) => ({ ...s, schema }))
    .filter((s) => !dropped.has(`${c.connectionId}|s:${schema}.${s.name}`));
}

// --- SQL text (mirrors src-tauri/src/sqlgen/objects.rs) ------------------------------------------

const q = (d: Driver, ident: string) =>
  d === "mysql" ? `\`${ident.replace(/`/g, "``")}\`` : `"${ident.replace(/"/g, '""')}"`;
const qt = (d: Driver, t: TableRef) =>
  t.schema ? `${q(d, t.schema)}.${q(d, t.name)}` : q(d, t.name);
const lit = (d: Driver, s: string) =>
  d === "mysql"
    ? `'${s.replace(/\\/g, "\\\\").replace(/'/g, "''")}'`
    : `'${s.replace(/'/g, "''")}'`;

function objectWord(d: Driver, kind: TableKind): string {
  switch (kind) {
    case "view":
      return "VIEW";
    case "materializedView":
      return d === "postgres" ? "MATERIALIZED VIEW" : "VIEW";
    case "foreignTable":
      return d === "postgres" ? "FOREIGN TABLE" : "TABLE";
    case "systemView":
      return mockError("UNSUPPORTED", "system views belong to the server and cannot be changed");
    default:
      return "TABLE";
  }
}

function withFkChecksOff(d: Driver, ignore: boolean, statements: string[]): string[] {
  return ignore && d === "mysql"
    ? ["SET FOREIGN_KEY_CHECKS = 0", ...statements, "SET FOREIGN_KEY_CHECKS = 1"]
    : statements;
}

async function planOf(c: ConnectionInfo, op: ObjectOp): Promise<DdlPlan> {
  const d = c.driver;
  const plan = (statements: string[], warnings: string[] = []): DdlPlan => ({
    statements,
    warnings,
    transactional: d !== "mysql",
  });
  switch (op.op) {
    case "dropTables": {
      // Like the backend: SQLite one table per statement; PostgreSQL / MySQL one statement per
      // kind (views first), so foreign keys between the selected tables do not get in the way.
      if (d === "sqlite")
        return plan(op.tables.map((t) => `DROP ${objectWord(d, t.kind)} ${qt(d, t.table)}`));
      const byWord = new Map<string, string[]>();
      for (const t of op.tables) {
        const word = objectWord(d, t.kind);
        byWord.set(word, [...(byWord.get(word) ?? []), qt(d, t.table)]);
      }
      const order = ["VIEW", "MATERIALIZED VIEW", "FOREIGN TABLE", "TABLE"];
      const statements = order
        .filter((w) => byWord.has(w))
        .map(
          (w, i) =>
            `DROP ${w}${d === "postgres" && op.cascade && i > 0 ? " IF EXISTS" : ""} ${(byWord.get(w) ?? []).join(", ")}${d === "postgres" && op.cascade ? " CASCADE" : ""}`,
        );
      return plan(withFkChecksOff(d, op.ignoreForeignKeys, statements));
    }
    case "truncateTables":
      return plan(
        withFkChecksOff(
          d,
          op.ignoreForeignKeys,
          d === "postgres"
            ? [
                `TRUNCATE TABLE ${op.tables.map((t) => qt(d, t)).join(", ")}${op.restartIdentity ? " RESTART IDENTITY" : ""}${op.cascade ? " CASCADE" : ""}`,
              ]
            : op.tables.map((t) =>
                d === "sqlite" ? `DELETE FROM ${qt(d, t)}` : `TRUNCATE TABLE ${qt(d, t)}`,
              ),
        ),
        d === "mysql" && !op.restartIdentity ? ["MySQL TRUNCATE always resets AUTO_INCREMENT"] : [],
      );
    case "cloneTable": {
      const name = op.newName.trim();
      if (!name) mockError("INVALID_INPUT", "the new table needs a name");
      if (name === op.source.name)
        mockError("INVALID_INPUT", "the clone needs a name other than the original's");
      const meta = await mockInvoke<TableMeta>("get_table", {
        connectionId: c.connectionId,
        table: op.source,
      });
      const target = { schema: op.source.schema, name };
      const cols = meta.columns
        .filter((x) => !x.isGenerated)
        .map((x) => q(d, x.name))
        .join(", ");
      const create =
        d === "postgres"
          ? `CREATE TABLE ${qt(d, target)} (LIKE ${qt(d, op.source)} INCLUDING ALL)`
          : d === "mysql"
            ? `CREATE TABLE ${qt(d, target)} LIKE ${qt(d, op.source)}`
            : `CREATE TABLE ${qt(d, target)} (${meta.columns.map((x) => `${q(d, x.name)} ${x.dataType}`).join(", ")})`;
      const statements = [create];
      if (op.withData && cols)
        statements.push(
          `INSERT INTO ${qt(d, target)} (${cols}) SELECT ${cols} FROM ${qt(d, op.source)}`,
        );
      return plan(statements, d === "sqlite" ? [] : ["Foreign keys are not copied to the clone"]);
    }
    case "dropRoutines":
      if (d === "sqlite") mockError("UNSUPPORTED", "SQLite has no stored routines");
      return plan(
        op.routines.map((r) => {
          const word =
            r.kind === "procedure"
              ? "PROCEDURE"
              : r.kind === "aggregate" && d === "postgres"
                ? "AGGREGATE"
                : "FUNCTION";
          const name = r.schema ? `${q(d, r.schema)}.${q(d, r.name)}` : q(d, r.name);
          return d === "postgres"
            ? `DROP ${word} ${name}(${r.identityArguments})${op.cascade ? " CASCADE" : ""}`
            : `DROP ${word} ${name}`;
        }),
      );
    case "dropSequences":
      if (d !== "postgres") mockError("UNSUPPORTED", "sequences are listed for PostgreSQL only");
      return plan(
        op.sequences.map((s) => `DROP SEQUENCE ${qt(d, s)}${op.cascade ? " CASCADE" : ""}`),
      );
    case "setComment": {
      const comment = op.comment || null;
      const t = op.target;
      if (d === "sqlite") mockError("UNSUPPORTED", "SQLite tables cannot have a comment");
      if (d === "mysql") {
        if (t.kind !== "table" && t.kind !== "partitioned")
          mockError("UNSUPPORTED", "MySQL views cannot have a comment");
        return plan([`ALTER TABLE ${qt(d, t.table)} COMMENT = ${lit(d, comment ?? "")}`]);
      }
      return plan([
        `COMMENT ON ${objectWord(d, t.kind)} ${qt(d, t.table)} IS ${comment === null ? "NULL" : lit(d, comment)}`,
      ]);
    }
  }
}

// --- handlers --------------------------------------------------------------------------------------

registerMock("list_routines", async ({ connectionId, schema }) => {
  const c = await connectionOf(connectionId);
  return c ? routinesFor(c, schema as string) : [];
});

registerMock("list_sequences", async ({ connectionId, schema }) => {
  const c = await connectionOf(connectionId);
  return c ? sequencesFor(c, schema as string) : [];
});

registerMock("list_system_schemas", async ({ connectionId }) => {
  const c = await connectionOf(connectionId);
  if (c?.driver !== "postgres") return [];
  return ["information_schema", "pg_catalog", "pg_toast"].map((name): SchemaInfo => ({
    name,
    isDefault: false,
  }));
});

registerMock("list_database_items", async ({ connectionId, includeSystem }) => {
  const c = await requireConnection(connectionId);
  const schemas = await mockInvoke<SchemaInfo[]>("list_schemas", { connectionId });
  const names = schemas.map((s) => s.name);
  // The system schemas' relations, like the backend (their routines never are).
  if (includeSystem === true && c.driver === "postgres") names.push("information_schema");
  const tables: TableSummary[] = [];
  const routines: RoutineSummary[] = [];
  for (const schema of names) {
    tables.push(...(await mockInvoke<TableSummary[]>("list_tables", { connectionId, schema })));
    if (schema !== "information_schema") routines.push(...routinesFor(c, schema));
  }
  return { tables, routines };
});

registerMock("get_routine_definition", async ({ connectionId, routine }) => {
  const c = await requireConnection(connectionId);
  const r = routine as RoutineRef;
  const list = c.driver === "postgres" ? (PG_ROUTINES[r.schema ?? "public"] ?? []) : MYSQL_ROUTINES;
  const found = list.find(
    (x) =>
      x.name === r.name && (c.driver !== "postgres" || x.identityArguments === r.identityArguments),
  );
  if (!found) return mockError("NOT_FOUND", `routine ${r.name}(${r.identityArguments})`);
  if (c.driver === "mysql") {
    const what = found.kind === "procedure" ? "PROCEDURE" : "FUNCTION";
    const returns = found.returnType ? `\n    RETURNS ${found.returnType}\n    DETERMINISTIC` : "";
    return `DROP ${what} IF EXISTS ${q("mysql", found.name)};\n\nCREATE DEFINER=\`root\`@\`localhost\` ${what} ${q("mysql", found.name)}(${found.arguments})${returns}\n${found.body};\n`;
  }
  const name = `${r.schema ?? "public"}.${found.name}`;
  if (found.kind === "aggregate")
    return `CREATE OR REPLACE AGGREGATE "${r.schema ?? "public"}"."${found.name}"(${found.arguments}) (\n  SFUNC = numeric_add,\n  STYPE = numeric,\n  INITCOND = '0'\n);\n`;
  const what = found.kind === "procedure" ? "PROCEDURE" : "FUNCTION";
  const returns = found.returnType ? `\n RETURNS ${found.returnType}` : "";
  return `CREATE OR REPLACE ${what} ${name}(${found.arguments})${returns}\n LANGUAGE ${found.language}\nAS $function$\n${found.body}\n$function$;\n`;
});

registerMock("get_sequence_definition", async ({ connectionId, sequence }) => {
  const c = await requireConnection(connectionId);
  const s = sequence as TableRef;
  const found = sequencesFor(c, s.schema ?? "public").find((x) => x.name === s.name);
  if (!found) return mockError("NOT_FOUND", `sequence ${s.name}`);
  const name = `"${s.schema ?? "public"}"."${s.name}"`;
  const max = found.dataType === "integer" ? "2147483647" : "9223372036854775807";
  let sql = `CREATE SEQUENCE ${name}\n  AS ${found.dataType}\n  INCREMENT BY 1\n  MINVALUE 1\n  MAXVALUE ${max}\n  START WITH 1\n  CACHE 1\n  NO CYCLE;\n`;
  if (found.ownedBy) {
    const [table, column] = found.ownedBy.split(".");
    sql += `\nALTER SEQUENCE ${name} OWNED BY "${s.schema ?? "public"}"."${table}"."${column}";\n`;
  }
  return sql;
});

registerMock("get_item_overview", async ({ connectionId, schema }) => {
  const c = await requireConnection(connectionId);
  const tables = await mockInvoke<TableSummary[]>("list_tables", { connectionId, schema });
  return tables.map((t): OverviewItem => {
    const sized = t.kind === "table" || t.kind === "partitioned" || t.kind === "materializedView";
    const data = sized ? (t.sizeBytes ?? 8192) : null;
    const index = sized ? Math.round((t.sizeBytes ?? 8192) * 0.35) : null;
    const key = `${c.driver}:${t.schema ?? ""}.${t.name}`;
    return {
      name: t.name,
      schema: t.schema,
      kind: t.kind,
      estimatedRows: sized ? t.rowEstimate : null,
      totalSize: data !== null && index !== null ? data + index : null,
      dataSize: data,
      indexSize: index,
      engine: c.driver === "mysql" && t.kind === "table" ? "InnoDB" : null,
      charset: c.driver === "mysql" && t.kind === "table" ? "utf8mb4" : null,
      collation: c.driver === "mysql" && t.kind === "table" ? "utf8mb4_0900_ai_ci" : null,
      owner: c.driver === "postgres" ? "tabler" : null,
      comment: comments.has(key) ? (comments.get(key) ?? null) : t.comment,
    };
  });
});

registerMock("preview_object_op", async ({ connectionId, op }) => {
  const c = await requireConnection(connectionId);
  return planOf(c, op as ObjectOp);
});

/** Runs one DDL op through the DDL mock; its error (if any) fails the whole operation. */
async function ddl(c: ConnectionInfo, table: TableRef, ops: DdlOp[]): Promise<DdlResult> {
  return mockInvoke<DdlResult>("apply_ddl", { connectionId: c.connectionId, table, ops });
}

registerMock("apply_object_op", async ({ connectionId, op }) => {
  const c = await requireConnection(connectionId);
  if (c.readOnly) mockError("UNSUPPORTED", "this connection is read-only");
  const o = op as ObjectOp;
  const p = await planOf(c, o);
  const started = performance.now();
  const done = (
    error: DdlResult["error"] = null,
    failedIndex: number | null = null,
  ): DdlResult => ({
    executed: error ? p.statements.slice(0, failedIndex ?? 0) : p.statements,
    failedIndex,
    error,
    durationMs: Math.max(3, Math.round(performance.now() - started)) + 7,
  });
  switch (o.op) {
    case "dropTables":
      for (const t of o.tables as ObjectTarget[]) {
        const r = await ddl(c, t.table, [
          { op: "dropTable", table: t.table, ifExists: false, cascade: o.cascade },
        ]);
        if (r.error) return done(r.error, 0);
      }
      return done();
    case "truncateTables":
      for (const t of o.tables) {
        const r = await ddl(c, t, [
          { op: "truncateTable", table: t, restartIdentity: o.restartIdentity, cascade: o.cascade },
        ]);
        if (r.error) return done(r.error, 0);
      }
      return done();
    case "cloneTable": {
      const meta = await mockInvoke<TableMeta>("get_table", { connectionId, table: o.source });
      const target = { schema: o.source.schema, name: o.newName.trim() };
      const columns = meta.columns.map((x): ColumnDef => ({
        name: x.name,
        dataType: x.dataType,
        nullable: x.nullable,
        defaultExpr: x.defaultExpr,
        autoIncrement: x.isAutoIncrement,
        comment: x.comment,
        collation: x.collation,
        unsigned: x.isUnsigned,
        onUpdate: null,
      }));
      const r = await ddl(c, target, [
        {
          op: "createTable",
          table: target,
          columns,
          primaryKey: meta.primaryKey,
          ifNotExists: false,
        },
      ]);
      return r.error ? done(r.error, 0) : done();
    }
    case "dropRoutines":
      for (const r of o.routines)
        dropped.add(`${c.connectionId}|${routineId(r.schema, r.name, r.identityArguments)}`);
      return done();
    case "dropSequences":
      for (const s of o.sequences) dropped.add(`${c.connectionId}|s:${s.schema ?? ""}.${s.name}`);
      return done();
    case "setComment": {
      const t = o.target.table;
      comments.set(`${c.driver}:${t.schema ?? ""}.${t.name}`, o.comment || null);
      return done();
    }
  }
});
