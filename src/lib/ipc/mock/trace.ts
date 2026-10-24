import { coreDialect } from "@/features/drivers/catalog";
import type { BrowsePage } from "../bindings/BrowsePage";
import type { ConnectionInfo } from "../bindings/ConnectionInfo";
import type { Dialect } from "../bindings/Dialect";
import type { AdminResult } from "../bindings/AdminResult";
import type { ExplainResult } from "../bindings/ExplainResult";
import type { RowCount } from "../bindings/RowCount";
import type { SqlDumpPreview } from "../bindings/SqlDumpPreview";
import type { SqlImportReport } from "../bindings/SqlImportReport";
import type { TableRef } from "../bindings/TableRef";
import type { TraceCategory } from "../bindings/TraceCategory";
import type { TracedSql } from "../bindings/TracedSql";
import { observeMock, registerMock } from "./transport";

/**
 * The statements the real backend's sessions record (`SqlTrace`, drained with `drain_sql_trace`),
 * as the browser mock would run them: realistic catalog SELECTs for the introspection commands,
 * the COUNT of a row count, the SELECT of a table page. Recorded after each command that ran; the
 * facade drains them after the call and logs them in the console. Commands that run no SQL (the
 * mock's cached table structure, SQLite's database list) record nothing.
 */

const dialects = new Map<string, Dialect>();
const queues = new Map<string, TracedSql[]>();
/** Tables whose structure the mock "server" has read once (the backend caches it). */
const cachedTables = new Set<string>();

/** The statements recorded for a connection since the last drain (`drain_sql_trace`). */
export function drainMockTrace(connectionId: string): TracedSql[] {
  const trace = queues.get(connectionId) ?? [];
  queues.delete(connectionId);
  return trace;
}

registerMock("drain_sql_trace", ({ connectionId }) => drainMockTrace(connectionId as string));

/** Test helper: forget the recorded statements and the structures the "server" has cached. */
export function resetMockTrace(): void {
  queues.clear();
  cachedTables.clear();
  dumps.clear();
}

type Statement = readonly [category: TraceCategory, sql: string, rows?: number];

const lit = (v: string) => `'${v.replaceAll("'", "''")}'`;
const ident = (d: Dialect, name: string) => (d === "mysql" ? `\`${name}\`` : `"${name}"`);
const qualified = (d: Dialect, t: TableRef) =>
  t.schema ? `${ident(d, t.schema)}.${ident(d, t.name)}` : ident(d, t.name);

const PG_USER_SCHEMAS =
  "n.nspname NOT IN ('pg_catalog','information_schema','pg_toast') AND n.nspname NOT LIKE 'pg\\_temp\\_%' AND n.nspname NOT LIKE 'pg\\_toast\\_temp\\_%'";

function tableRef(args: Record<string, unknown>, key = "table"): TableRef {
  return args[key] as TableRef;
}

function schemaOf(d: Dialect, t: TableRef): string {
  return t.schema ?? (d === "sqlite" ? "main" : "public");
}

/** What the real backend would have run for `cmd`, or null for commands that run no SQL. */
function statementsFor(
  d: Dialect,
  cmd: string,
  args: Record<string, unknown>,
  result: unknown,
): Statement[] | null {
  const count = (r: unknown) => (Array.isArray(r) ? r.length : undefined);
  const schema = String(args.schema ?? "");
  switch (cmd) {
    case "list_databases":
      if (d === "postgres")
        return [
          [
            "structure",
            "SELECT d.datname::text, pg_get_userbyid(d.datdba)::text AS owner, pg_encoding_to_char(d.encoding)::text AS encoding, d.datname = current_database() AS is_current FROM pg_database d WHERE NOT d.datistemplate AND d.datallowconn ORDER BY 1",
            count(result),
          ],
        ];
      if (d === "mysql")
        return [
          ["structure", "SELECT DATABASE()", 1],
          [
            "structure",
            "SELECT SCHEMA_NAME, DEFAULT_CHARACTER_SET_NAME FROM information_schema.SCHEMATA ORDER BY SCHEMA_NAME",
            count(result),
          ],
        ];
      return null;
    case "list_schemas":
      if (d === "postgres")
        return [
          [
            "structure",
            `SELECT n.nspname::text, COALESCE(n.nspname = (current_schemas(false))[1], false) AS is_default FROM pg_namespace n WHERE ${PG_USER_SCHEMAS} ORDER BY 1`,
            count(result),
          ],
        ];
      if (d === "mysql") return [["structure", "SELECT DATABASE()", 1]];
      return [["structure", "PRAGMA database_list", count(result)]];
    case "list_tables":
      if (d === "postgres")
        return [
          [
            "structure",
            `SELECT c.relname::text, c.relkind::text, obj_description(c.oid, 'pg_class')::text, CASE WHEN c.reltuples < 0 THEN NULL ELSE c.reltuples::bigint END, CASE WHEN c.relkind IN ('r','p','m') THEN pg_total_relation_size(c.oid)::bigint END FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = ${lit(schema)} AND c.relkind IN ('r','p','v','m','f') ORDER BY c.relname`,
            count(result),
          ],
        ];
      if (d === "mysql")
        return [
          [
            "structure",
            `SELECT TABLE_NAME, TABLE_TYPE, TABLE_ROWS, DATA_LENGTH, INDEX_LENGTH, TABLE_COMMENT FROM information_schema.TABLES WHERE TABLE_SCHEMA = ${lit(schema)} ORDER BY TABLE_NAME`,
            count(result),
          ],
        ];
      return [
        [
          "structure",
          `SELECT name, type FROM ${ident(d, schema || "main")}.sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name`,
          count(result),
        ],
      ];
    case "get_table":
    case "count_table_rows":
    case "fetch_cell":
    case "browse_table":
      return null;
    case "get_view_definition": {
      const t = tableRef(args);
      if (d === "postgres")
        return [
          [
            "structure",
            `SELECT pg_get_viewdef(c.oid, true)::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = ${lit(schemaOf(d, t))} AND c.relname = ${lit(t.name)} AND c.relkind IN ('v','m')`,
            1,
          ],
        ];
      if (d === "mysql")
        return [
          [
            "structure",
            `SELECT VIEW_DEFINITION FROM information_schema.VIEWS WHERE TABLE_SCHEMA = ${lit(schemaOf(d, t))} AND TABLE_NAME = ${lit(t.name)}`,
            1,
          ],
        ];
      return [
        [
          "structure",
          `SELECT sql FROM ${ident(d, schemaOf(d, t))}.sqlite_master WHERE type = 'view' AND name = ${lit(t.name)}`,
          1,
        ],
      ];
    }
    case "get_completion_metadata":
      if (d === "postgres")
        return [
          [
            "structure",
            `SELECT n.nspname::text, c.relname::text, a.attname::text FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid JOIN pg_namespace n ON n.oid = c.relnamespace WHERE a.attnum > 0 AND NOT a.attisdropped AND c.relkind IN ('r','p','v','m','f') AND ${PG_USER_SCHEMAS} ORDER BY 1, 2, a.attnum`,
          ],
        ];
      if (d === "mysql")
        return [
          [
            "structure",
            "SELECT TABLE_SCHEMA, TABLE_NAME, COLUMN_NAME FROM information_schema.COLUMNS WHERE TABLE_SCHEMA NOT IN ('information_schema','performance_schema','mysql','sys') ORDER BY TABLE_SCHEMA, TABLE_NAME, ORDINAL_POSITION",
          ],
        ];
      return [
        ["structure", "PRAGMA database_list", 1],
        [
          "structure",
          `SELECT name, type FROM "main".sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name`,
        ],
        ["structure", `PRAGMA "main".table_info("users");\nPRAGMA "main".table_info("orders")`],
      ];
    case "get_structure_choices":
      if (d === "postgres")
        return [
          [
            "structure",
            "SELECT t.typname::text, t.typtype::text, n.nspname::text FROM pg_type t JOIN pg_namespace n ON n.oid = t.typnamespace WHERE t.typtype IN ('e','d','c') AND n.nspname NOT IN ('pg_catalog','information_schema')",
          ],
        ];
      if (d === "mysql")
        return [
          ["structure", "SHOW ENGINES"],
          [
            "structure",
            "SELECT COLLATION_NAME, CHARACTER_SET_NAME FROM information_schema.COLLATIONS",
          ],
        ];
      return null;
    case "list_routines":
      if (d === "postgres")
        return [
          [
            "structure",
            `SELECT p.proname::text, p.prokind::text, pg_get_function_identity_arguments(p.oid)::text FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace WHERE n.nspname = ${lit(schema)} ORDER BY p.proname`,
            count(result),
          ],
        ];
      if (d === "mysql")
        return [
          [
            "structure",
            `SELECT ROUTINE_NAME, ROUTINE_TYPE FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA = ${lit(schema)} ORDER BY ROUTINE_NAME`,
            count(result),
          ],
        ];
      return null;
    case "list_sequences":
      if (d === "postgres")
        return [
          [
            "structure",
            `SELECT c.relname::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = ${lit(schema)} AND c.relkind = 'S' ORDER BY c.relname`,
            count(result),
          ],
        ];
      return null;
    case "list_database_items":
      if (d === "postgres")
        return [
          [
            "structure",
            `SELECT n.nspname::text, c.relname::text, c.relkind::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE c.relkind IN ('r','p','v','m','f') AND ${PG_USER_SCHEMAS} ORDER BY 1, 2`,
          ],
          [
            "structure",
            `SELECT n.nspname::text, p.proname::text, p.prokind::text FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace WHERE ${PG_USER_SCHEMAS} ORDER BY 1, 2`,
          ],
        ];
      if (d === "mysql")
        return [
          [
            "structure",
            "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE FROM information_schema.TABLES ORDER BY TABLE_SCHEMA, TABLE_NAME",
          ],
          [
            "structure",
            "SELECT ROUTINE_SCHEMA, ROUTINE_NAME, ROUTINE_TYPE FROM information_schema.ROUTINES ORDER BY ROUTINE_SCHEMA, ROUTINE_NAME",
          ],
        ];
      return [
        [
          "structure",
          `SELECT name, type FROM "main".sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name`,
        ],
      ];
    case "get_routine_definition":
      if (d === "postgres")
        return [
          [
            "structure",
            "SELECT pg_get_functiondef(p.oid)::text FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace WHERE n.nspname = 'public' ORDER BY p.proname LIMIT 1",
            1,
          ],
        ];
      if (d === "mysql") return [["structure", "SHOW CREATE FUNCTION `calc_total`", 1]];
      return null;
    case "get_sequence_definition":
      return d === "postgres"
        ? [
            [
              "structure",
              `SELECT start_value, minimum_value, maximum_value, increment_by, cycle FROM pg_sequences WHERE schemaname = ${lit(schemaOf(d, tableRef(args, "sequence")))} AND sequencename = ${lit(tableRef(args, "sequence").name)}`,
              1,
            ],
          ]
        : null;
    case "get_item_overview":
      if (d === "postgres")
        return [
          [
            "structure",
            `SELECT c.relname::text, c.relkind::text, pg_total_relation_size(c.oid)::bigint, c.reltuples::bigint FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = ${lit(schema)} AND c.relkind IN ('r','p','v','m','f','S') ORDER BY c.relname`,
            count(result),
          ],
        ];
      if (d === "mysql")
        return [
          [
            "structure",
            `SELECT TABLE_NAME, TABLE_TYPE, TABLE_ROWS, DATA_LENGTH + INDEX_LENGTH FROM information_schema.TABLES WHERE TABLE_SCHEMA = ${lit(schema)} ORDER BY TABLE_NAME`,
            count(result),
          ],
        ];
      return [
        [
          "structure",
          `SELECT name, type FROM ${ident(d, schema || "main")}.sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name`,
          count(result),
        ],
      ];
    case "get_table_ddl": {
      const t = tableRef(args);
      return d === "mysql"
        ? [["structure", `SHOW CREATE TABLE ${qualified(d, t)}`, 1]]
        : d === "sqlite"
          ? [
              [
                "structure",
                `SELECT sql FROM ${ident(d, schemaOf(d, t))}.sqlite_master WHERE name = ${lit(t.name)}`,
                1,
              ],
            ]
          : null;
    }
    case "schema_relations":
      if (d === "postgres")
        return [
          [
            "structure",
            `SELECT c.relname::text, a.attname::text, format_type(a.atttypid, a.atttypmod)::text FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = ${lit(schema)} AND a.attnum > 0 AND NOT a.attisdropped AND c.relkind IN ('r','p') ORDER BY c.relname, a.attnum`,
          ],
          [
            "structure",
            `SELECT con.conname::text, c.relname::text, rc.relname::text FROM pg_constraint con JOIN pg_class c ON c.oid = con.conrelid JOIN pg_class rc ON rc.oid = con.confrelid JOIN pg_namespace n ON n.oid = c.relnamespace WHERE con.contype = 'f' AND n.nspname = ${lit(schema)}`,
          ],
        ];
      if (d === "mysql")
        return [
          [
            "structure",
            `SELECT TABLE_NAME, COLUMN_NAME, COLUMN_TYPE FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = ${lit(schema)} ORDER BY TABLE_NAME, ORDINAL_POSITION`,
          ],
          [
            "structure",
            `SELECT CONSTRAINT_NAME, TABLE_NAME, REFERENCED_TABLE_NAME FROM information_schema.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA = ${lit(schema)} AND REFERENCED_TABLE_NAME IS NOT NULL`,
          ],
        ];
      return [
        ["structure", `SELECT name FROM "main".sqlite_master WHERE type = 'table' ORDER BY name`],
      ];
    case "export_database_tables":
      return d === "postgres"
        ? [
            [
              "structure",
              `SELECT n.nspname::text, c.relname::text, c.relkind::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE c.relkind IN ('r','p','v','m') AND ${PG_USER_SCHEMAS} ORDER BY 1, 2`,
              count(result),
            ],
          ]
        : d === "mysql"
          ? [
              [
                "structure",
                "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_NAME",
                count(result),
              ],
            ]
          : [
              [
                "structure",
                `SELECT name, type FROM "main".sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name`,
                count(result),
              ],
            ];
    default:
      return null;
  }
}

/** The statements that read a table's structure from the server (the first read, or a refresh). */
function tableMetaStatements(d: Dialect, t: TableRef): Statement[] {
  const schema = schemaOf(d, t);
  if (d === "postgres")
    return [
      [
        "structure",
        `SELECT c.oid::bigint, c.relkind::text, c.reltuples::bigint, obj_description(c.oid, 'pg_class')::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = ${lit(schema)} AND c.relname = ${lit(t.name)}`,
        1,
      ],
      [
        "structure",
        `SELECT a.attname::text, format_type(a.atttypid, a.atttypmod)::text, a.attnotnull, pg_get_expr(d.adbin, d.adrelid)::text FROM pg_attribute a LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum WHERE a.attrelid = (SELECT c.oid FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = ${lit(schema)} AND c.relname = ${lit(t.name)}) AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum`,
      ],
      [
        "structure",
        `SELECT i.relname::text, pg_get_indexdef(i.oid)::text FROM pg_index x JOIN pg_class i ON i.oid = x.indexrelid JOIN pg_class c ON c.oid = x.indrelid JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = ${lit(schema)} AND c.relname = ${lit(t.name)}`,
      ],
    ];
  if (d === "mysql")
    return [
      [
        "structure",
        `SELECT ORDINAL_POSITION, COLUMN_NAME, COLUMN_TYPE, DATA_TYPE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_KEY, EXTRA FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = ${lit(schema)} AND TABLE_NAME = ${lit(t.name)} ORDER BY ORDINAL_POSITION`,
      ],
      [
        "structure",
        `SELECT INDEX_NAME, NON_UNIQUE, COLUMN_NAME FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = ${lit(schema)} AND TABLE_NAME = ${lit(t.name)} ORDER BY INDEX_NAME, SEQ_IN_INDEX`,
      ],
      ["structure", `SHOW CREATE TABLE ${qualified(d, t)}`, 1],
    ];
  return [
    [
      "structure",
      `SELECT type, sql FROM ${ident(d, schema)}.sqlite_master WHERE type IN ('table','view') AND name = ${lit(t.name)}`,
      1,
    ],
    ["structure", `PRAGMA ${ident(d, schema)}.table_xinfo(${ident(d, t.name)})`],
    ["structure", `PRAGMA ${ident(d, schema)}.index_list(${ident(d, t.name)})`],
    ["structure", `PRAGMA ${ident(d, schema)}.foreign_key_list(${ident(d, t.name)})`],
  ];
}

/** The category the backend gives a statement it was handed (`classify_statement`). */
function categoryOf(sql: string): TraceCategory {
  return /^\s*(create|alter|drop|truncate|rename|comment|grant|revoke|vacuum|reindex|pragma|show|describe|desc)\b/i.test(
    sql,
  )
    ? "structure"
    : "data";
}

/** The statements the Explain command runs: the wrapped statement, in a transaction for Analyze. */
function explainStatements(d: Dialect, sql: string, mode: string, rows: number): Statement[] {
  const statement = sql.trim().replace(/[\s;]+$/, "");
  const analyze = mode === "analyze";
  const prefix =
    d === "postgres"
      ? analyze
        ? "EXPLAIN (ANALYZE, BUFFERS, VERBOSE, FORMAT TEXT)"
        : "EXPLAIN (VERBOSE, COSTS, FORMAT TEXT)"
      : d === "mysql"
        ? analyze
          ? "EXPLAIN ANALYZE"
          : "EXPLAIN FORMAT=TREE"
        : "EXPLAIN QUERY PLAN";
  const wrapped: Statement = ["data", `${prefix} ${statement}`, rows];
  if (!analyze || d === "sqlite") return [wrapped];
  return [["data", d === "mysql" ? "START TRANSACTION" : "BEGIN"], wrapped, ["data", "ROLLBACK"]];
}

/** The statements of the dumps the mock previewed, by path (an import runs them). */
const dumps = new Map<string, string[]>();

function record(connectionId: string, statements: readonly Statement[]): void {
  const queue = queues.get(connectionId) ?? [];
  for (const [category, sql, rows] of statements) {
    queue.push({
      sql,
      category,
      durationMs: 3,
      rows: rows ?? null,
      affected: null,
      error: null,
    });
  }
  // The backend keeps the newest 500.
  queues.set(connectionId, queue.slice(-500));
}

observeMock((cmd, args, result) => {
  if (cmd === "connect") {
    const info = result as ConnectionInfo;
    // Another driver's connection records nothing here.
    const dialect = coreDialect(info.driver);
    if (dialect) dialects.set(info.connectionId, dialect);
    return;
  }
  if (cmd === "disconnect") {
    const id = args.connectionId as string;
    dialects.delete(id);
    queues.delete(id);
    for (const key of cachedTables) if (key.startsWith(`${id}|`)) cachedTables.delete(key);
    return;
  }
  const id = args.connectionId;
  if (typeof id !== "string") return;
  const d = dialects.get(id);
  if (!d) return;

  if (cmd === "preview_sql_dump") {
    const preview = result as SqlDumpPreview;
    dumps.set((args.req as { path: string }).path, preview.statements);
    return;
  }
  if (cmd === "invalidate_schema_cache") {
    for (const key of cachedTables) if (key.startsWith(`${id}|`)) cachedTables.delete(key);
    return;
  }
  // A table's structure is read once and then cached by the backend (`refresh` reads it again).
  const structureOf = (t: TableRef): Statement[] => {
    const key = `${id}|${schemaOf(d, t)}.${t.name}`;
    if (cachedTables.has(key)) return [];
    cachedTables.add(key);
    return tableMetaStatements(d, t);
  };
  if (cmd === "get_table") {
    const t = tableRef(args);
    if (args.refresh === true) cachedTables.delete(`${id}|${schemaOf(d, t)}.${t.name}`);
    record(id, structureOf(t));
    return;
  }
  if (cmd === "browse_table") {
    const req = args.req as { table: TableRef };
    const page = result as BrowsePage;
    record(id, [...structureOf(req.table), ["data", page.sql, page.rows.length]]);
    return;
  }
  if (cmd === "count_table_rows") {
    const t = tableRef(args);
    const counted = result as RowCount;
    // An estimate reads the planner's figure from the table's structure, not a COUNT.
    record(
      id,
      counted.exact
        ? [...structureOf(t), ["data", `SELECT COUNT(*) FROM ${qualified(d, t)}`, 1]]
        : structureOf(t),
    );
    return;
  }
  if (cmd === "fetch_cell") {
    const t = tableRef(args);
    record(id, [
      ...structureOf(t),
      [
        "data",
        `SELECT ${ident(d, String(args.column))} FROM ${qualified(d, t)} WHERE ${ident(d, "id")} = 1`,
        1,
      ],
    ]);
    return;
  }
  if (cmd === "explain") {
    record(
      id,
      explainStatements(
        d,
        String(args.sql ?? ""),
        String(args.mode),
        (result as ExplainResult).rows.length,
      ),
    );
    return;
  }
  if (cmd === "import_sql_dump") {
    // The backend records every statement of the dump it ran; the mock knows the ones it showed.
    const report = result as SqlImportReport;
    const path = (args.req as { path: string }).path;
    const ran = (dumps.get(path) ?? []).slice(0, report.executed + report.failed);
    if (ran.length === 0) return;
    record(
      id,
      ran.map((sql): Statement => [categoryOf(sql), sql]),
    );
    const failures = new Map(report.errors.map((e) => [e.sql, e.message]));
    const queue = queues.get(id);
    for (const entry of queue?.slice(-ran.length) ?? []) {
      const message = failures.get(entry.sql);
      if (message !== undefined) entry.error = message;
    }
    return;
  }
  if (cmd === "admin_apply_user") {
    // The statements ran masked, as the result shows them (`AdminStatement::display`).
    record(
      id,
      (result as AdminResult).executed.map((sql): Statement => ["structure", sql]),
    );
    return;
  }
  const statements = statementsFor(d, cmd, args, result);
  if (statements) record(id, statements);
});
