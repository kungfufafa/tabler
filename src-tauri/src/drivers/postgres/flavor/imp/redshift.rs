//! Amazon Redshift. Its leader node is PostgreSQL 8.0.2 with Redshift's own catalog: the
//! catalog SQL here uses none of what 8.0 lacks (`string_agg`, `unnest`, `LATERAL`, `WITH
//! ORDINALITY`, `typcategory`, collations, identity columns), builds lists in Rust, and reads
//! the `svv_` / `stv_` / `sys_` views in queries of their own: a query that joins them with
//! `pg_catalog` or calls a leader-node function next to them fails at run time.
//!
//! - Session init: no IntervalStyle, client_encoding or default_transaction_read_only (not
//!   Redshift settings), and extra_float_digits 2 (its maximum). A read-only session is
//!   enforced by Tabler's classifier only (`ReadOnlyMode::ClassifierOnly`).
//! - Tables: `pg_class` plus `svv_table_info` (row counts, sizes in 1 MB blocks), the
//!   materialized views of `svv_mv_info` and the Spectrum tables of `svv_external_tables`.
//!   Columns carry DISTKEY, SORTKEY and ENCODE in `extra`, and the table's DISTSTYLE is its
//!   `TableOptions.row_format`. A late-binding view (`WITH NO SCHEMA BINDING`) has no columns in
//!   `pg_attribute`: they come from `pg_get_late_binding_view_cols()`. Keys come from
//!   `pg_get_constraintdef` (they are informational: Redshift enforces none). There are no
//!   indexes, triggers or CHECK constraints.
//! - DDL is the server's own `SHOW TABLE` / `SHOW VIEW` / `SHOW EXTERNAL TABLE`.
//! - Sessions: `stv_sessions` and `stv_recents` on a provisioned cluster, `sys_session_history`
//!   and `sys_query_history` on Serverless (which has no `stv_` views); kills by
//!   `pg_cancel_backend` / `pg_terminate_backend`.
//! - The session speaks Redshift SQL (`sqlgen::dialect::redshift`): the grid, its edits (by
//!   primary key only, checked to affect one row) and the editor's classifier go through it.

use std::collections::BTreeMap;

use async_trait::async_trait;

use super::catalog::{kill_outcome, optional, paren_lists, signalled, typmod_info};
use crate::drivers::postgres::decode::kind_from_name;
use crate::drivers::postgres::flavor::PgFlavorHooks;
use crate::drivers::postgres::introspect::Traced;
use crate::error::AppError;
use crate::sqlgen::admin::kill_sql;
use crate::sqlgen::dialect::redshift::{SPEC, ShowDdl, show_ddl};
use crate::sqlgen::{Dialect, SqlDialect};
use crate::types::admin::{KillOutcome, KillScope, ProcessRow};
use crate::types::browse::FilterOp;
use crate::types::caps::{DataCaps, Feature, SessionCaps};
use crate::types::driver::{KindId, PgFlavor, ReadOnlyMode};
use crate::types::mutate::ServerExpr;
use crate::types::profile::{ConnectionProfile, Driver};
use crate::types::schema::{
    ColumnMeta, CompletionMetadata, DatabaseInfo, IndexMeta, SchemaInfo, TableKind, TableMeta,
    TableOptions, TableRef, TableSummary, ValueKind,
};

pub(super) struct Redshift;

pub(super) static REDSHIFT: Redshift = Redshift;

/// Redshift's highest `extra_float_digits` (PostgreSQL's 3 is refused).
const EXTRA_FLOAT_DIGITS: &str = "SET extra_float_digits = 2;";

/// The init statements of PostgreSQL that Redshift has no setting for.
const NOT_SETTINGS: &[&str] = &[
    "SET IntervalStyle",
    "SET client_encoding",
    "SET default_transaction_read_only",
];

/// The schemas Redshift keeps for itself.
const USER_SCHEMAS: &str = "n.nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast', \
     'pg_internal', 'pg_automv', 'pg_auto_copy', 'pg_mv', 'pg_s3', 'catalog_history') \
     AND n.nspname !~ '^pg_temp_'";

const LIST_DATABASES: &str = "SELECT d.datname, pg_get_userbyid(d.datdba), \
     d.datname = current_database() FROM pg_database d \
     WHERE d.datallowconn AND NOT d.datistemplate AND d.datname <> 'padb_harvest' ORDER BY 1";

const CURRENT_SCHEMA: &str = "SELECT current_schema()";

const RELATIONS: &str = "SELECT c.relname, CASE WHEN c.relkind = 'v' THEN 'v' ELSE 'r' END, \
     obj_description(c.oid, 'pg_class') \
     FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
     WHERE n.nspname = $1 AND c.relkind IN ('r', 'v') ORDER BY c.relname";

/// Row counts and sizes (in 1 MB blocks) of a schema's tables that hold data.
const TABLE_INFO: &str =
    "SELECT \"table\", size::bigint, tbl_rows::bigint FROM svv_table_info WHERE \"schema\" = $1";

const MATERIALIZED_VIEWS: &str = "SELECT trim(name) FROM svv_mv_info WHERE trim(schema_name) = $1";

/// Whether a view of `pg_class` is a materialized view (one row then).
const MATERIALIZED_VIEW: &str =
    "SELECT trim(name) FROM svv_mv_info WHERE trim(schema_name) = $1 AND trim(name) = $2";

const EXTERNAL_TABLES: &str =
    "SELECT tablename FROM svv_external_tables WHERE schemaname = $1 ORDER BY tablename";

const RELATION: &str = "SELECT CASE WHEN c.relkind = 'v' THEN 'v' ELSE 'r' END, \
     obj_description(c.oid, 'pg_class'), c.reldiststyle::int \
     FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
     WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind IN ('r', 'v')";

const COLUMNS: &str = "SELECT a.attnum::int, a.attname, format_type(a.atttypid, a.atttypmod), \
     t.typname, NOT a.attnotnull, pg_get_expr(d.adbin, d.adrelid), \
     col_description(c.oid, a.attnum), a.atttypmod::int, a.attisdistkey, \
     a.attsortkeyord::int, format_encoding(a.attencodingtype::int) \
     FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid \
     JOIN pg_namespace n ON n.oid = c.relnamespace JOIN pg_type t ON t.oid = a.atttypid \
     LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
     WHERE n.nspname = $1 AND c.relname = $2 AND a.attnum > 0 AND NOT a.attisdropped \
     ORDER BY a.attnum";

const CONSTRAINTS: &str = "SELECT con.conname, \
     CASE con.contype WHEN 'p' THEN 'p' WHEN 'f' THEN 'f' ELSE 'u' END, \
     pg_get_constraintdef(con.oid), fn.nspname, fc.relname \
     FROM pg_constraint con JOIN pg_class c ON c.oid = con.conrelid \
     JOIN pg_namespace n ON n.oid = c.relnamespace \
     LEFT JOIN pg_class fc ON fc.oid = con.confrelid \
     LEFT JOIN pg_namespace fn ON fn.oid = fc.relnamespace \
     WHERE n.nspname = $1 AND c.relname = $2 AND con.contype IN ('p', 'f', 'u') \
     ORDER BY con.conname";

const LATE_BINDING_COLUMNS: &str = "SELECT col_name, col_type, col_num \
     FROM pg_get_late_binding_view_cols() \
     cols(view_schema name, view_name name, col_name name, col_type varchar, col_num int) \
     WHERE view_schema = $1 AND view_name = $2 ORDER BY col_num";

const TABLE_STATS: &str = "SELECT tbl_rows::bigint, diststyle FROM svv_table_info \
     WHERE \"schema\" = $1 AND \"table\" = $2";

const EXTERNAL_TABLE: &str =
    "SELECT location FROM svv_external_tables WHERE schemaname = $1 AND tablename = $2";

const EXTERNAL_COLUMNS: &str = "SELECT columnname, external_type, columnnum::int, is_nullable \
     FROM svv_external_columns WHERE schemaname = $1 AND tablename = $2 ORDER BY columnnum";

const VIEW_DEFINITION: &str = "SELECT pg_get_viewdef(c.oid, true) \
     FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
     WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind = 'v'";

const BACKEND_PID: &str = "SELECT pg_backend_pid()";

/// A provisioned cluster's sessions and running statements.
const SESSIONS: &str = "SELECT process, trim(user_name), trim(db_name), \
     to_char(starttime, 'YYYY-MM-DD HH24:MI:SS') FROM stv_sessions ORDER BY process";

const RUNNING: &str = "SELECT pid, trim(query), to_char(starttime, 'YYYY-MM-DD HH24:MI:SS'), \
     (duration / 1000)::bigint FROM stv_recents WHERE status = 'Running'";

/// Serverless's (it has no `stv_` views).
const SERVERLESS_SESSIONS: &str = "SELECT session_id, trim(user_id), trim(database_name), \
     to_char(start_time, 'YYYY-MM-DD HH24:MI:SS') FROM sys_session_history \
     WHERE trim(status) = 'active' ORDER BY session_id";

const SERVERLESS_RUNNING: &str = "SELECT session_id, trim(query_text), \
     to_char(start_time, 'YYYY-MM-DD HH24:MI:SS'), (elapsed_time / 1000)::bigint \
     FROM sys_query_history WHERE trim(status) = 'running'";

#[async_trait]
impl PgFlavorHooks for Redshift {
    fn flavor(&self) -> Option<PgFlavor> {
        Some(PgFlavor::Redshift)
    }

    fn sql_dialect(&self) -> SqlDialect {
        SqlDialect::Redshift
    }

    fn default_database(&self) -> &'static str {
        KindId::Redshift
            .descriptor()
            .default_database
            .unwrap_or("dev")
    }

    fn init_sql(&self, _: &ConnectionProfile, _: bool, base: Vec<String>) -> Vec<String> {
        base.into_iter()
            .filter(|s| !NOT_SETTINGS.iter().any(|n| s.starts_with(n)))
            .map(|s| {
                if s.starts_with("SET extra_float_digits") {
                    EXTRA_FLOAT_DIGITS.to_string()
                } else {
                    s
                }
            })
            .collect()
    }

    /// One failing statement then names itself.
    fn init_one_by_one(&self) -> bool {
        true
    }

    async fn list_databases(&self, c: &Traced<'_>) -> Option<Result<Vec<DatabaseInfo>, AppError>> {
        Some(list_databases(c).await)
    }

    async fn list_schemas(&self, c: &Traced<'_>) -> Option<Result<Vec<SchemaInfo>, AppError>> {
        Some(list_schemas(c).await)
    }

    async fn list_tables(
        &self,
        c: &Traced<'_>,
        schema: &str,
    ) -> Option<Result<Vec<TableSummary>, AppError>> {
        Some(list_tables(c, schema).await)
    }

    async fn table_meta(
        &self,
        c: &Traced<'_>,
        table: &TableRef,
    ) -> Option<Result<TableMeta, AppError>> {
        Some(table_meta(c, table).await)
    }

    async fn view_definition(
        &self,
        c: &Traced<'_>,
        table: &TableRef,
    ) -> Option<Result<Option<String>, AppError>> {
        Some(view_definition(c, table).await)
    }

    async fn completion_metadata(
        &self,
        c: &Traced<'_>,
    ) -> Option<Result<CompletionMetadata, AppError>> {
        Some(completion_metadata(c).await)
    }

    async fn list_processes(&self, c: &Traced<'_>) -> Option<Result<Vec<ProcessRow>, AppError>> {
        Some(list_processes(c).await)
    }

    async fn kill(
        &self,
        c: &Traced<'_>,
        ids: &[String],
        scope: KillScope,
    ) -> Option<Vec<KillOutcome>> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let sql = id
                .trim()
                .parse::<i64>()
                .map_err(|_| AppError::InvalidInput(format!("invalid process id {id}")))
                .and_then(|pid| kill_sql(Dialect::Postgres, pid, scope));
            let result = match sql {
                Ok(sql) => c
                    .query(&sql, &[])
                    .await
                    .map(|rows| signalled(rows.first()))
                    .map_err(AppError::from),
                Err(e) => Err(e),
            };
            out.push(kill_outcome(id, result));
        }
        Some(out)
    }

    async fn table_ddl(
        &self,
        c: &Traced<'_>,
        table: &TableRef,
    ) -> Option<Result<String, AppError>> {
        Some(table_ddl(c, table).await)
    }

    /// What the session is proven to do through Redshift SQL and this file: browse, filter, sort
    /// and count (estimates from `svv_table_info`), edit rows by primary key (the affected count
    /// checked before commit), EXPLAIN plans, the server's DDL, view definitions, the session
    /// list and its kills, other databases, and the time zone and startup commands of the init.
    /// Not yet: RETURNING (Redshift has none), edits without a key, structure editing, export,
    /// import, backups, users, routines, the diagram, cancelling on the server (CancelRequest is
    /// unverified on a cluster), MCP (until the classifier's red-team review) and keep-alive
    /// pings.
    fn adjust_caps(&self, caps: &mut SessionCaps, _: i32) {
        let core = SessionCaps::core(Driver::Postgres, Dialect::Postgres, caps.flavor);
        caps.namespaces = core.namespaces;
        caps.data = DataCaps {
            filter_ops: core
                .data
                .filter_ops
                .into_iter()
                .filter(|op| !matches!(op, FilterOp::JsonContains | FilterOp::JsonNotContains))
                .collect(),
            default_keyword: SPEC.knobs.set_default,
            server_exprs: vec![
                ServerExpr::Now,
                ServerExpr::CurrentTimestamp,
                ServerExpr::CurrentDate,
            ],
            max_params: SPEC.max_params,
            ..core.data
        };
        caps.safety.read_only = ReadOnlyMode::ClassifierOnly;
        caps.session.keepalive_secs = None;
        *caps = caps.clone().with(&[
            Feature::Explain,
            Feature::Format,
            Feature::EditRows,
            Feature::InsertRows,
            Feature::DeleteRows,
            Feature::ServerExpressions,
            Feature::ExactCount,
            Feature::TableDdl,
            Feature::ViewDefinition,
            Feature::SwitchDatabase,
            Feature::ProcessList,
            Feature::KillProcess,
            Feature::Timezone,
            Feature::StartupCommands,
        ]);
    }
}

/// Text a CHAR column of a system view pads with spaces.
fn trimmed(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

async fn list_databases(c: &Traced<'_>) -> Result<Vec<DatabaseInfo>, AppError> {
    let mut out = Vec::new();
    for row in c.query(LIST_DATABASES, &[]).await? {
        out.push(DatabaseInfo {
            name: row.try_get(0)?,
            owner: row.try_get(1)?,
            encoding: Some("UTF8".into()),
            is_current: row.try_get::<_, Option<bool>>(2)?.unwrap_or(false),
        });
    }
    Ok(out)
}

async fn list_schemas(c: &Traced<'_>) -> Result<Vec<SchemaInfo>, AppError> {
    let sql = format!(
        "SELECT n.nspname, n.nspname = current_schema() FROM pg_namespace n \
         WHERE {USER_SCHEMAS} ORDER BY 1"
    );
    let mut out = Vec::new();
    for row in c.query(&sql, &[]).await? {
        out.push(SchemaInfo {
            name: row.try_get(0)?,
            is_default: row.try_get::<_, Option<bool>>(1)?.unwrap_or(false),
        });
    }
    Ok(out)
}

async fn schema_of(c: &Traced<'_>, table: &TableRef) -> Result<String, AppError> {
    if let Some(s) = table.schema.as_deref().filter(|s| !s.is_empty()) {
        return Ok(s.to_string());
    }
    let row = c.query_one(CURRENT_SCHEMA, &[]).await?;
    Ok(row
        .try_get::<_, Option<String>>(0)?
        .unwrap_or_else(|| "public".into()))
}

async fn list_tables(c: &Traced<'_>, schema: &str) -> Result<Vec<TableSummary>, AppError> {
    let mut out = Vec::new();
    for row in c.query(RELATIONS, &[&schema]).await? {
        let kind: String = row.try_get(1)?;
        out.push(TableSummary {
            name: row.try_get(0)?,
            schema: Some(schema.to_string()),
            kind: if kind == "v" {
                TableKind::View
            } else {
                TableKind::Table
            },
            row_estimate: None,
            size_bytes: None,
            comment: row.try_get(2)?,
        });
    }
    // Each view of its own: one Redshift lacks, or this user may not read, leaves the list as is.
    for row in optional(c.query(TABLE_INFO, &[&schema]).await, "svv_table_info")? {
        let Some(name) = trimmed(row.try_get(0)?) else {
            continue;
        };
        let blocks: Option<i64> = row.try_get(1)?;
        let rows: Option<i64> = row.try_get(2)?;
        if let Some(t) = out.iter_mut().find(|t| t.name == name) {
            t.size_bytes = blocks.map(|b| b.saturating_mul(1 << 20));
            t.row_estimate = rows;
        }
    }
    for row in optional(c.query(MATERIALIZED_VIEWS, &[&schema]).await, "svv_mv_info")? {
        let name: Option<String> = row.try_get(0)?;
        if let Some(t) = out.iter_mut().find(|t| Some(&t.name) == name.as_ref()) {
            t.kind = TableKind::MaterializedView;
        }
    }
    for row in optional(
        c.query(EXTERNAL_TABLES, &[&schema]).await,
        "svv_external_tables",
    )? {
        let Some(name) = trimmed(row.try_get(0)?) else {
            continue;
        };
        if !out.iter().any(|t| t.name == name) {
            out.push(TableSummary {
                name,
                schema: Some(schema.to_string()),
                kind: TableKind::ForeignTable,
                row_estimate: None,
                size_bytes: None,
                comment: None,
            });
        }
    }
    Ok(out)
}

/// The kind of a type Redshift spells (`pg_type.typname`, or a late-binding view's or a Spectrum
/// column's type).
fn kind_of_type(ty: &str) -> ValueKind {
    let lower = ty.trim().to_ascii_lowercase();
    let base = lower.split('(').next().unwrap_or_default().trim();
    match base {
        "super" => ValueKind::Json,
        "varbyte" | "binary varying" | "varbinary" | "binary" => ValueKind::Bytes,
        "hllsketch" => ValueKind::Other,
        "intervaly2m" | "intervald2s" => ValueKind::Interval,
        b if b.starts_with("interval") => ValueKind::Interval,
        "integer" | "int" | "smallint" => ValueKind::Int,
        "bigint" => ValueKind::BigInt,
        "decimal" => ValueKind::Decimal,
        "real" | "double precision" | "float" | "double" => ValueKind::Float,
        "boolean" => ValueKind::Bool,
        "character varying" | "character" | "string" | "nvarchar" | "nchar" => ValueKind::Text,
        "timestamp without time zone" | "timestamp" => ValueKind::Timestamp,
        "timestamp with time zone" | "timestamptz" => ValueKind::TimestampTz,
        b if b.starts_with("time") => ValueKind::Time,
        // Spectrum's nested types.
        b if b.starts_with("struct<") || b.starts_with("array<") || b.starts_with("map<") => {
            ValueKind::Json
        }
        other => kind_from_name(other),
    }
}

/// DISTSTYLE from `pg_class.reldiststyle`.
fn diststyle(code: Option<i32>) -> Option<&'static str> {
    Some(match code? {
        0 => "DISTSTYLE EVEN",
        1 => "DISTSTYLE KEY",
        8 => "DISTSTYLE ALL",
        9 => "DISTSTYLE AUTO (ALL)",
        10 => "DISTSTYLE AUTO (EVEN)",
        11 => "DISTSTYLE AUTO (KEY)",
        _ => return None,
    })
}

/// DISTKEY, SORTKEY n (INTERLEAVED for a negative order) and ENCODE of a column.
fn column_extra(distkey: bool, sortkey: i32, encoding: Option<&str>) -> Option<String> {
    let mut parts = Vec::new();
    if distkey {
        parts.push("DISTKEY".to_string());
    }
    match sortkey {
        0 => {}
        n if n < 0 => parts.push(format!("SORTKEY INTERLEAVED {}", -n)),
        n => parts.push(format!("SORTKEY {n}")),
    }
    if let Some(e) = encoding
        .map(str::trim)
        .filter(|e| !e.is_empty() && *e != "none")
    {
        parts.push(format!("ENCODE {e}"));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn blank_column(name: String, ordinal: u32, data_type: String, kind: ValueKind) -> ColumnMeta {
    ColumnMeta {
        name,
        ordinal,
        data_type,
        kind,
        nullable: true,
        default_expr: None,
        is_pk: false,
        pk_ordinal: None,
        is_auto_increment: false,
        is_generated: false,
        is_unsigned: false,
        comment: None,
        enum_values: None,
        char_max_len: None,
        numeric_precision: None,
        numeric_scale: None,
        collation: None,
        array_elem_type: None,
        extra: None,
        base_type: None,
    }
}

async fn table_meta(c: &Traced<'_>, table: &TableRef) -> Result<TableMeta, AppError> {
    let schema = schema_of(c, table).await?;
    let name = table.name.as_str();
    let params: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[&schema, &name];
    let Some(rel) = c.query_opt(RELATION, params).await? else {
        return external_table_meta(c, &schema, name).await;
    };
    let mut kind = match rel.try_get::<_, String>(0)?.as_str() {
        "v" => TableKind::View,
        _ => TableKind::Table,
    };
    // `pg_class` shows a materialized view as a view; `list_tables` names it by `svv_mv_info`,
    // and so does this.
    if kind == TableKind::View
        && !optional(c.query(MATERIALIZED_VIEW, params).await, "svv_mv_info")?.is_empty()
    {
        kind = TableKind::MaterializedView;
    }
    let comment: Option<String> = rel.try_get(1)?;
    let reldiststyle: Option<i32> = rel.try_get(2)?;

    let mut columns = Vec::new();
    for row in c.query(COLUMNS, params).await? {
        let attnum: i32 = row.try_get(0)?;
        let typname: String = row.try_get(3)?;
        let typmod: i32 = row.try_get(7)?;
        let encoding: Option<String> = row.try_get(10)?;
        let mut col = blank_column(
            row.try_get(1)?,
            attnum.max(0) as u32,
            row.try_get(2)?,
            kind_of_type(&typname),
        );
        col.nullable = row.try_get(4)?;
        col.default_expr = row.try_get(5)?;
        col.comment = row.try_get(6)?;
        // An IDENTITY column's default is `"identity"(…)`.
        col.is_auto_increment = col
            .default_expr
            .as_deref()
            .is_some_and(|d| d.contains("\"identity\"("));
        (col.char_max_len, col.numeric_precision, col.numeric_scale) =
            typmod_info(&typname, typmod);
        col.extra = column_extra(
            row.try_get::<_, Option<bool>>(8)?.unwrap_or(false),
            row.try_get::<_, Option<i32>>(9)?.unwrap_or(0),
            encoding.as_deref(),
        );
        columns.push(col);
    }
    if kind == TableKind::View && columns.is_empty() {
        // A late-binding view.
        for row in c.query(LATE_BINDING_COLUMNS, params).await? {
            let ty: String = row.try_get(1)?;
            let num: Option<i32> = row.try_get(2)?;
            columns.push(blank_column(
                row.try_get(0)?,
                num.unwrap_or(0).max(0) as u32,
                ty.clone(),
                kind_of_type(&ty),
            ));
        }
    }

    let mut primary_key = Vec::new();
    let mut foreign_keys = Vec::new();
    let mut indexes = Vec::new();
    if kind == TableKind::Table {
        for row in c.query(CONSTRAINTS, params).await? {
            let con_name: String = row.try_get(0)?;
            let contype: String = row.try_get(1)?;
            let definition: Option<String> = row.try_get(2)?;
            let lists = paren_lists(definition.as_deref().unwrap_or(""));
            let cols = lists.first().cloned().unwrap_or_default();
            match contype.as_str() {
                "p" => primary_key = cols,
                "f" => {
                    let ref_schema: Option<String> = row.try_get(3)?;
                    let ref_table: Option<String> = row.try_get(4)?;
                    foreign_keys.push(crate::types::schema::ForeignKeyMeta {
                        name: con_name,
                        columns: cols,
                        ref_table: TableRef::new(
                            ref_schema.as_deref(),
                            ref_table.as_deref().unwrap_or_default(),
                        ),
                        ref_columns: lists.get(1).cloned().unwrap_or_default(),
                        // Informational keys: Redshift neither checks nor acts on them.
                        on_update: "NO ACTION".into(),
                        on_delete: "NO ACTION".into(),
                        definition,
                    });
                }
                _ => indexes.push(IndexMeta {
                    name: con_name,
                    columns: cols,
                    unique: true,
                    primary: false,
                    method: None,
                    predicate: None,
                    definition,
                    comment: None,
                    include: Vec::new(),
                }),
            }
        }
        for (i, key) in primary_key.iter().enumerate() {
            if let Some(col) = columns.iter_mut().find(|c| &c.name == key) {
                col.is_pk = true;
                col.pk_ordinal = Some(i as u32 + 1);
            }
        }
    }

    let mut row_estimate = None;
    let mut distribution = diststyle(reldiststyle).map(str::to_string);
    if kind == TableKind::Table {
        let stats = optional(c.query(TABLE_STATS, params).await, "svv_table_info")?;
        if let Some(row) = stats.first() {
            row_estimate = row.try_get(0)?;
            distribution = trimmed(row.try_get(1)?)
                .map(|d| format!("DISTSTYLE {d}"))
                .or(distribution);
        }
    }
    Ok(TableMeta {
        table: TableRef::new(Some(&schema), name),
        kind,
        columns,
        primary_key,
        indexes,
        foreign_keys,
        triggers: Vec::new(),
        comment,
        row_estimate,
        ddl: None,
        checks: Vec::new(),
        options: distribution.map(|d| TableOptions {
            row_format: Some(d),
            ..TableOptions::default()
        }),
    })
}

/// A Spectrum table: read-only, its columns from `svv_external_columns`.
async fn external_table_meta(
    c: &Traced<'_>,
    schema: &str,
    name: &str,
) -> Result<TableMeta, AppError> {
    let params: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[&schema, &name];
    let found = optional(c.query(EXTERNAL_TABLE, params).await, "svv_external_tables")?;
    let Some(location) = found.first() else {
        return Err(AppError::NotFound(format!("table {schema}.{name}")));
    };
    let location: Option<String> = location.try_get(0)?;
    let mut columns = Vec::new();
    for row in c.query(EXTERNAL_COLUMNS, params).await? {
        let ty: String = row.try_get(1)?;
        let num: Option<i32> = row.try_get(2)?;
        let nullable: Option<String> = row.try_get(3)?;
        let mut col = blank_column(
            row.try_get(0)?,
            num.unwrap_or(0).max(0) as u32,
            ty.clone(),
            kind_of_type(&ty),
        );
        col.nullable = nullable.as_deref().map(str::trim) != Some("false");
        columns.push(col);
    }
    Ok(TableMeta {
        table: TableRef::new(Some(schema), name),
        kind: TableKind::ForeignTable,
        columns,
        primary_key: Vec::new(),
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
        triggers: Vec::new(),
        comment: trimmed(location),
        row_estimate: None,
        ddl: None,
        checks: Vec::new(),
        options: None,
    })
}

async fn view_definition(c: &Traced<'_>, table: &TableRef) -> Result<Option<String>, AppError> {
    let schema = schema_of(c, table).await?;
    match c
        .query_opt(VIEW_DEFINITION, &[&schema, &table.name])
        .await?
    {
        Some(row) => Ok(row.try_get(0)?),
        None => Ok(None),
    }
}

async fn completion_metadata(c: &Traced<'_>) -> Result<CompletionMetadata, AppError> {
    let mut meta = CompletionMetadata::default();
    for schema in list_schemas(c).await? {
        if schema.is_default {
            meta.default_schema = Some(schema.name.clone());
        }
        meta.schemas.insert(schema.name, BTreeMap::new());
    }
    let sql = format!(
        "SELECT n.nspname, c.relname, a.attname FROM pg_class c \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         LEFT JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
         WHERE c.relkind IN ('r', 'v') AND {USER_SCHEMAS} ORDER BY 1, 2, a.attnum"
    );
    for row in c.query(&sql, &[]).await? {
        let schema: String = row.try_get(0)?;
        let table: String = row.try_get(1)?;
        let column: Option<String> = row.try_get(2)?;
        let columns = meta
            .schemas
            .entry(schema)
            .or_default()
            .entry(table)
            .or_default();
        columns.extend(column);
    }
    Ok(meta)
}

/// A session and, while it runs one, its statement.
struct Session {
    pid: i32,
    user: Option<String>,
    database: Option<String>,
    started: Option<String>,
}

struct Running {
    pid: i32,
    query: Option<String>,
    started: Option<String>,
    duration_ms: Option<i64>,
}

async fn list_processes(c: &Traced<'_>) -> Result<Vec<ProcessRow>, AppError> {
    let me: Option<i32> = c.query_one(BACKEND_PID, &[]).await?.try_get(0)?;
    let (sessions, running) = match c.query(SESSIONS, &[]).await {
        Ok(rows) => (rows, c.query(RUNNING, &[]).await?),
        // Serverless has no `stv_` views.
        Err(e) if super::catalog::missing(&e) => (
            c.query(SERVERLESS_SESSIONS, &[]).await?,
            c.query(SERVERLESS_RUNNING, &[]).await?,
        ),
        Err(e) => return Err(e.into()),
    };
    let mut list = Vec::new();
    for row in &sessions {
        list.push(Session {
            pid: row.try_get(0)?,
            user: trimmed(row.try_get(1)?),
            database: trimmed(row.try_get(2)?),
            started: row.try_get(3)?,
        });
    }
    let mut runs = Vec::new();
    for row in &running {
        runs.push(Running {
            pid: row.try_get(0)?,
            query: trimmed(row.try_get(1)?),
            started: row.try_get(2)?,
            duration_ms: row.try_get(3)?,
        });
    }
    Ok(process_rows(list, runs, me))
}

/// The sessions, each `active` with the statement it runs or `idle`; `me` is the listing one.
fn process_rows(sessions: Vec<Session>, running: Vec<Running>, me: Option<i32>) -> Vec<ProcessRow> {
    sessions
        .into_iter()
        .map(|s| {
            let run = running.iter().find(|r| r.pid == s.pid);
            let current = me == Some(s.pid);
            ProcessRow {
                id: s.pid.to_string(),
                user: s.user,
                database: s.database,
                application: None,
                client: None,
                state: Some(if run.is_some() { "active" } else { "idle" }.into()),
                command: None,
                started_at: run.and_then(|r| r.started.clone()).or(s.started),
                duration_ms: run.and_then(|r| r.duration_ms),
                wait_event: None,
                query: run.and_then(|r| r.query.clone()),
                current,
                own: current,
            }
        })
        .collect()
}

async fn table_ddl(c: &Traced<'_>, table: &TableRef) -> Result<String, AppError> {
    let schema = schema_of(c, table).await?;
    let table = TableRef::new(Some(&schema), &table.name);
    let params: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[&schema, &table.name];
    let what = match c.query_opt(RELATION, params).await? {
        Some(rel) if rel.try_get::<_, String>(0)? == "v" => ShowDdl::View,
        Some(_) => ShowDdl::Table,
        None => ShowDdl::ExternalTable,
    };
    let mut lines = Vec::new();
    for row in c.query(&show_ddl(what, &table), &[]).await? {
        if let Some(line) = row.try_get::<_, Option<String>>(0)? {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return Err(AppError::NotFound(format!("table {schema}.{}", table.name)));
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{self, REDSHIFT as VERSION};
    use super::*;
    use crate::drivers::postgres::testing::{Cell, Col, FakePg};
    use crate::drivers::{DbSession, TxStatement};
    use crate::sqlgen::portable::mutate::build_change;
    use crate::sqlgen::sql::TableTarget;
    use crate::types::browse::RowKey;
    use crate::types::driver::Flavor;
    use crate::types::mutate::StagedChange;
    use crate::types::query::ExplainMode;
    use crate::types::value::Value;

    /// A fake Redshift: it refuses PostgreSQL's settings and what its SQL lacks.
    fn fake() -> FakePg {
        let mut pg = FakePg::new().server_version("8.0.2").version(VERSION);
        for (part, message) in [
            (
                "SET IntervalStyle",
                "unrecognized configuration parameter \"intervalstyle\"",
            ),
            (
                "extra_float_digits = 3",
                "3 is outside the valid range (-15 .. 2)",
            ),
            (
                "SET client_encoding",
                "unrecognized configuration parameter",
            ),
            (
                "default_transaction_read_only",
                "unrecognized configuration parameter",
            ),
            ("RETURNING", "syntax error at or near \"RETURNING\""),
            ("ON CONFLICT", "syntax error at or near \"ON\""),
            ("DISTINCT FROM", "syntax error at or near \"DISTINCT\""),
            ("SAVEPOINT", "SAVEPOINT is not supported"),
            ("string_agg", "function string_agg does not exist"),
            ("unnest", "function unnest does not exist"),
            ("LATERAL", "syntax error at or near \"LATERAL\""),
            ("ORDINALITY", "syntax error at or near \"ORDINALITY\""),
            ("typcategory", "column \"typcategory\" does not exist"),
        ] {
            pg = pg.error(part, "42601", message);
        }
        pg
    }

    fn text_col(name: &str) -> Col {
        Col::text(name)
    }

    #[test]
    fn the_init_is_what_redshift_takes() {
        let base = vec![
            "SET DateStyle = 'ISO, YMD';".to_string(),
            "SET IntervalStyle = 'postgres';".to_string(),
            "SET extra_float_digits = 3;".to_string(),
            "SET client_encoding = 'UTF8';".to_string(),
            "SET TIME ZONE 'UTC';".to_string(),
            "SET default_transaction_read_only = on;".to_string(),
            "SELECT set_config('query_group', 'etl', false);".to_string(),
            "SET search_path = app, public;".to_string(),
        ];
        let profile = testkit::profile(5439);
        assert_eq!(
            REDSHIFT.init_sql(&profile, true, base),
            [
                "SET DateStyle = 'ISO, YMD';",
                "SET extra_float_digits = 2;",
                "SET TIME ZONE 'UTC';",
                "SELECT set_config('query_group', 'etl', false);",
                "SET search_path = app, public;",
            ]
        );
    }

    #[test]
    fn its_catalog_sql_uses_nothing_the_leader_node_lacks() {
        let schemas = format!("WHERE {USER_SCHEMAS}");
        for sql in [
            LIST_DATABASES,
            CURRENT_SCHEMA,
            RELATIONS,
            TABLE_INFO,
            MATERIALIZED_VIEWS,
            MATERIALIZED_VIEW,
            EXTERNAL_TABLES,
            RELATION,
            COLUMNS,
            CONSTRAINTS,
            LATE_BINDING_COLUMNS,
            TABLE_STATS,
            EXTERNAL_TABLE,
            EXTERNAL_COLUMNS,
            VIEW_DEFINITION,
            BACKEND_PID,
            SESSIONS,
            RUNNING,
            SERVERLESS_SESSIONS,
            SERVERLESS_RUNNING,
            &schemas,
        ] {
            for lacking in [
                "string_agg",
                "unnest",
                "LATERAL",
                "ORDINALITY",
                "typcategory",
                "attidentity",
                "relispartition",
                "::text",
                "RETURNING",
            ] {
                assert!(!sql.contains(lacking), "{lacking} in {sql}");
            }
        }
        // The system views are never joined with the leader's catalog.
        for sql in [
            TABLE_INFO,
            TABLE_STATS,
            SESSIONS,
            RUNNING,
            MATERIALIZED_VIEWS,
            MATERIALIZED_VIEW,
        ] {
            assert!(!sql.contains("pg_"), "{sql}");
        }
    }

    #[test]
    fn reads_types_dist_and_sort_keys() {
        assert_eq!(kind_of_type("super"), ValueKind::Json);
        assert_eq!(kind_of_type("character varying(256)"), ValueKind::Text);
        assert_eq!(kind_of_type("numeric(10,2)"), ValueKind::Decimal);
        assert_eq!(kind_of_type("int8"), ValueKind::BigInt);
        assert_eq!(kind_of_type("geometry"), ValueKind::Geometry);
        assert_eq!(kind_of_type("varbyte(64000)"), ValueKind::Bytes);
        assert_eq!(kind_of_type("hllsketch"), ValueKind::Other);
        assert_eq!(
            kind_of_type("timestamp with time zone"),
            ValueKind::TimestampTz
        );
        assert_eq!(kind_of_type("struct<a:int>"), ValueKind::Json);
        assert_eq!(
            column_extra(true, 1, Some("az64 ")).as_deref(),
            Some("DISTKEY, SORTKEY 1, ENCODE az64")
        );
        assert_eq!(
            column_extra(false, -2, Some("none")).as_deref(),
            Some("SORTKEY INTERLEAVED 2")
        );
        assert_eq!(column_extra(false, 0, None), None);
        assert_eq!(diststyle(Some(1)), Some("DISTSTYLE KEY"));
        assert_eq!(diststyle(Some(99)), None);
    }

    #[test]
    fn sessions_with_their_running_statements() {
        let rows = process_rows(
            vec![
                Session {
                    pid: 10,
                    user: Some("awsuser".into()),
                    database: Some("dev".into()),
                    started: Some("2026-10-29 09:00:00".into()),
                },
                Session {
                    pid: 11,
                    user: Some("etl".into()),
                    database: Some("dev".into()),
                    started: Some("2026-10-29 08:00:00".into()),
                },
            ],
            vec![Running {
                pid: 11,
                query: Some("COPY sales FROM 's3://…'".into()),
                started: Some("2026-10-29 10:00:00".into()),
                duration_ms: Some(1500),
            }],
            Some(10),
        );
        assert!(rows[0].current && rows[0].own);
        assert_eq!(rows[0].state.as_deref(), Some("idle"));
        assert_eq!(rows[1].state.as_deref(), Some("active"));
        assert_eq!(rows[1].started_at.as_deref(), Some("2026-10-29 10:00:00"));
        assert_eq!(rows[1].duration_ms, Some(1500));
        assert!(!rows[1].current);
    }

    /// A session on the fake Redshift: the init it takes, Redshift SQL and the caps it proves.
    #[tokio::test]
    async fn a_session_on_redshift() {
        let pg = fake().start().await;
        let mut profile = testkit::profile(pg.port());
        profile.options.read_only = true;
        profile.options.pg_flavor = Some(PgFlavor::Redshift);
        profile.options.timezone = Some("UTC".into());
        let session = testkit::session(&profile).await;
        assert_eq!(session.flavor(), Some(Flavor::Redshift));
        assert_eq!(session.dialect(), SqlDialect::Redshift);
        let database = ("database".to_string(), "dev".to_string());
        assert!(pg.startups()[0].contains(&database), "{:?}", pg.startups());
        assert_eq!(
            pg.queries(),
            [
                "SELECT version()",
                "SET DateStyle = 'ISO, YMD';",
                "SET extra_float_digits = 2;",
                "SET TIME ZONE 'UTC';",
            ]
        );
        let caps = session.capabilities();
        assert_eq!(caps.dialect, Some(SqlDialect::Redshift));
        assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);
        assert_eq!(caps.session.keepalive_secs, None);
        assert!(caps.namespaces.databases);
        for f in [
            Feature::Editor,
            Feature::StructureView,
            Feature::EditRows,
            Feature::InsertRows,
            Feature::DeleteRows,
            Feature::TableDdl,
            Feature::ProcessList,
            Feature::KillProcess,
            Feature::Explain,
        ] {
            assert!(caps.has(f), "{f:?}");
        }
        for f in [
            Feature::Returning,
            Feature::EditWithoutKey,
            Feature::ExplainAnalyze,
            Feature::StructureEdit,
            Feature::CancelOnServer,
            Feature::Users,
            Feature::Backup,
            Feature::ExportTables,
            Feature::Mcp,
            Feature::Diagram,
        ] {
            assert!(!caps.has(f), "{f:?}");
        }
        assert!(!caps.data.filter_ops.contains(&FilterOp::JsonContains));
        assert!(!caps.data.default_keyword);
        assert_eq!(session.explain_form(ExplainMode::Plan), None);
        session.close().await;
    }

    /// The fake Redshift answers the catalog as a cluster does (rows recorded from the views'
    /// documented columns), and refuses anything of PostgreSQL's later versions.
    #[tokio::test]
    async fn the_catalog_of_a_cluster() {
        let pg = fake()
            .rows(
                "FROM pg_database d",
                vec![text_col("datname"), text_col("owner"), Col::bool("current")],
                vec![vec![
                    Cell::text("dev"),
                    Cell::text("rdsdb"),
                    Cell::Bool(true),
                ]],
            )
            .rows(
                "n.nspname = current_schema() FROM pg_namespace",
                vec![text_col("nspname"), Col::bool("default")],
                vec![
                    vec![Cell::text("public"), Cell::Bool(true)],
                    vec![Cell::text("spectrum"), Cell::Bool(false)],
                ],
            )
            .rows(
                "obj_description(c.oid, 'pg_class') FROM pg_class c",
                vec![text_col("relname"), text_col("kind"), text_col("comment")],
                vec![
                    vec![
                        Cell::text("sales"),
                        Cell::text("r"),
                        Cell::text("ticket sales"),
                    ],
                    vec![Cell::text("lbv"), Cell::text("v"), Cell::Null],
                    vec![Cell::text("mv_daily"), Cell::text("v"), Cell::Null],
                ],
            )
            .rows(
                "size::bigint, tbl_rows::bigint FROM svv_table_info",
                vec![text_col("table"), Col::int8("size"), Col::int8("tbl_rows")],
                vec![vec![Cell::text("sales"), Cell::Int(12), Cell::Int(172_456)]],
            )
            .rows(
                "FROM svv_mv_info",
                vec![text_col("name")],
                vec![vec![Cell::text("mv_daily")]],
            )
            .error(
                "FROM svv_external_tables WHERE schemaname = $1 ORDER BY",
                "42501",
                "permission denied for relation svv_external_tables",
            )
            .start()
            .await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 80_002);
        let databases = REDSHIFT.list_databases(&c).await.unwrap().unwrap();
        assert_eq!(databases[0].name, "dev");
        assert!(databases[0].is_current);
        let schemas = REDSHIFT.list_schemas(&c).await.unwrap().unwrap();
        assert_eq!(schemas.len(), 2);
        assert!(schemas[0].is_default);
        let tables = REDSHIFT.list_tables(&c, "public").await.unwrap().unwrap();
        assert_eq!(tables.len(), 3);
        assert_eq!(tables[0].row_estimate, Some(172_456));
        assert_eq!(tables[0].size_bytes, Some(12 << 20));
        assert_eq!(tables[0].comment.as_deref(), Some("ticket sales"));
        assert_eq!(tables[1].kind, TableKind::View);
        assert_eq!(tables[2].kind, TableKind::MaterializedView);
        // The system views run on their own, never joined with pg_catalog.
        for q in testkit::sent(&pg, "svv_") {
            assert!(
                !q.contains("pg_class") && !q.contains("pg_namespace"),
                "{q}"
            );
        }
    }

    #[tokio::test]
    async fn a_table_a_late_binding_view_and_a_spectrum_table() {
        let column = |num: i64, name: &str, ty: &str, typname: &str, extras: (bool, i64, &str)| {
            vec![
                Cell::Int(num),
                Cell::text(name),
                Cell::text(ty),
                Cell::text(typname),
                Cell::Bool(num != 1),
                Cell::Null,
                Cell::Null,
                Cell::Int(-1),
                Cell::Bool(extras.0),
                Cell::Int(extras.1),
                Cell::text(extras.2),
            ]
        };
        let pg = fake()
            .rows(
                "c.reldiststyle::int FROM pg_class c",
                vec![
                    text_col("kind"),
                    text_col("comment"),
                    Col::int4("reldiststyle"),
                ],
                vec![vec![Cell::text("r"), Cell::Null, Cell::Int(1)]],
            )
            .rows(
                "format_encoding(a.attencodingtype::int)",
                vec![
                    Col::int4("attnum"),
                    text_col("attname"),
                    text_col("type"),
                    text_col("typname"),
                    Col::bool("nullable"),
                    text_col("default"),
                    text_col("comment"),
                    Col::int4("typmod"),
                    Col::bool("distkey"),
                    Col::int4("sortkey"),
                    text_col("encoding"),
                ],
                vec![
                    column(1, "salesid", "integer", "int4", (true, 1, "az64")),
                    column(2, "doc", "super", "super", (false, 0, "zstd")),
                    column(3, "shape", "geometry", "geometry", (false, 0, "none")),
                ],
            )
            .rows(
                "pg_get_constraintdef(con.oid)",
                vec![
                    text_col("conname"),
                    text_col("contype"),
                    text_col("def"),
                    text_col("nspname"),
                    text_col("relname"),
                ],
                vec![
                    vec![
                        Cell::text("sales_pkey"),
                        Cell::text("p"),
                        Cell::text("PRIMARY KEY (salesid)"),
                        Cell::Null,
                        Cell::Null,
                    ],
                    vec![
                        Cell::text("sales_event_fk"),
                        Cell::text("f"),
                        Cell::text("FOREIGN KEY (eventid) REFERENCES event(eventid)"),
                        Cell::text("public"),
                        Cell::text("event"),
                    ],
                ],
            )
            .rows(
                "diststyle FROM svv_table_info",
                vec![Col::int8("tbl_rows"), text_col("diststyle")],
                vec![vec![Cell::Int(172_456), Cell::text("KEY(salesid)")]],
            )
            .start()
            .await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 80_002);
        let sales = TableRef::new(Some("public"), "sales");
        let meta = REDSHIFT.table_meta(&c, &sales).await.unwrap().unwrap();
        assert_eq!(meta.kind, TableKind::Table);
        assert_eq!(meta.primary_key, ["salesid"]);
        assert!(meta.columns[0].is_pk && !meta.columns[0].nullable);
        assert_eq!(
            meta.columns[0].extra.as_deref(),
            Some("DISTKEY, SORTKEY 1, ENCODE az64")
        );
        assert_eq!(meta.columns[1].kind, ValueKind::Json);
        assert_eq!(meta.columns[2].kind, ValueKind::Geometry);
        assert_eq!(meta.foreign_keys[0].ref_columns, ["eventid"]);
        assert_eq!(
            meta.foreign_keys[0].ref_table,
            TableRef::new(Some("public"), "event")
        );
        assert_eq!(meta.row_estimate, Some(172_456));
        assert_eq!(
            meta.options.unwrap().row_format.as_deref(),
            Some("DISTSTYLE KEY(salesid)")
        );
        // The SUPER and GEOMETRY columns are read shortened, so they are not editable.
        assert!(crate::sqlgen::portable::select::is_lossy(
            &SPEC,
            &meta.columns[1]
        ));
        assert!(crate::sqlgen::portable::select::is_lossy(
            &SPEC,
            &meta.columns[2]
        ));
        assert!(!crate::sqlgen::portable::select::is_lossy(
            &SPEC,
            &meta.columns[0]
        ));

        // A late-binding view: its columns from pg_get_late_binding_view_cols().
        let pg = fake()
            .rows(
                "c.reldiststyle::int FROM pg_class c",
                vec![
                    text_col("kind"),
                    text_col("comment"),
                    Col::int4("reldiststyle"),
                ],
                vec![vec![Cell::text("v"), Cell::Null, Cell::Int(0)]],
            )
            .rows(
                "FROM pg_get_late_binding_view_cols()",
                vec![
                    text_col("col_name"),
                    text_col("col_type"),
                    Col::int4("col_num"),
                ],
                vec![
                    vec![Cell::text("a"), Cell::text("integer"), Cell::Int(1)],
                    vec![
                        Cell::text("b"),
                        Cell::text("character varying(256)"),
                        Cell::Int(2),
                    ],
                ],
            )
            .start()
            .await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 80_002);
        let view = REDSHIFT
            .table_meta(&c, &TableRef::new(Some("public"), "lbv"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(view.kind, TableKind::View);
        let kinds: Vec<ValueKind> = view.columns.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, [ValueKind::Int, ValueKind::Text]);
        assert!(testkit::sent(&pg, "pg_constraint").is_empty());
        assert_eq!(testkit::sent(&pg, "svv_mv_info"), [MATERIALIZED_VIEW]);

        // A materialized view: a view to pg_class, named by svv_mv_info, as in the table list.
        let pg = fake()
            .rows(
                "c.reldiststyle::int FROM pg_class c",
                vec![
                    text_col("kind"),
                    text_col("comment"),
                    Col::int4("reldiststyle"),
                ],
                vec![vec![Cell::text("v"), Cell::Null, Cell::Int(0)]],
            )
            .rows(
                "format_encoding(a.attencodingtype::int)",
                vec![
                    Col::int4("attnum"),
                    text_col("attname"),
                    text_col("type"),
                    text_col("typname"),
                    Col::bool("nullable"),
                    text_col("default"),
                    text_col("comment"),
                    Col::int4("typmod"),
                    Col::bool("distkey"),
                    Col::int4("sortkey"),
                    text_col("encoding"),
                ],
                vec![column(1, "day", "date", "date", (false, 0, "none"))],
            )
            .rows(
                "FROM svv_mv_info",
                vec![text_col("name")],
                vec![vec![Cell::text("mv_daily")]],
            )
            .start()
            .await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 80_002);
        let mv = REDSHIFT
            .table_meta(&c, &TableRef::new(Some("public"), "mv_daily"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(mv.kind, TableKind::MaterializedView);
        assert_eq!(mv.columns.len(), 1);
        assert!(testkit::sent(&pg, "pg_get_late_binding_view_cols").is_empty());
        assert!(testkit::sent(&pg, "pg_constraint").is_empty());

        // A Spectrum table: not in pg_class.
        let pg = fake()
            .rows(
                "SELECT location FROM svv_external_tables",
                vec![text_col("location")],
                vec![vec![Cell::text("s3://bucket/sales/")]],
            )
            .rows(
                "FROM svv_external_columns",
                vec![
                    text_col("columnname"),
                    text_col("external_type"),
                    Col::int4("columnnum"),
                    text_col("is_nullable"),
                ],
                vec![vec![
                    Cell::text("id"),
                    Cell::text("bigint"),
                    Cell::Int(1),
                    Cell::text("false"),
                ]],
            )
            .start()
            .await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 80_002);
        let external = REDSHIFT
            .table_meta(&c, &TableRef::new(Some("spectrum"), "sales"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(external.kind, TableKind::ForeignTable);
        assert!(external.primary_key.is_empty());
        assert!(!external.columns[0].nullable);
        assert_eq!(external.comment.as_deref(), Some("s3://bucket/sales/"));

        // In neither catalog.
        let pg = fake().start().await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 80_002);
        let gone = REDSHIFT
            .table_meta(&c, &TableRef::new(Some("public"), "gone"))
            .await
            .unwrap()
            .unwrap_err();
        assert!(matches!(gone, AppError::NotFound(_)), "{gone}");
    }

    #[tokio::test]
    async fn processes_on_a_cluster_and_on_serverless() {
        let pg = fake()
            .rows(
                "SELECT pg_backend_pid()",
                vec![Col::int4("pid")],
                vec![vec![Cell::Int(10)]],
            )
            .rows(
                "FROM stv_sessions",
                vec![
                    Col::int4("process"),
                    text_col("u"),
                    text_col("d"),
                    text_col("s"),
                ],
                vec![
                    vec![
                        Cell::Int(10),
                        Cell::text("awsuser"),
                        Cell::text("dev"),
                        Cell::text("2026-10-29 09:00:00"),
                    ],
                    vec![
                        Cell::Int(11),
                        Cell::text("etl"),
                        Cell::text("dev"),
                        Cell::text("2026-10-29 08:00:00"),
                    ],
                ],
            )
            .rows(
                "FROM stv_recents",
                vec![
                    Col::int4("pid"),
                    text_col("q"),
                    text_col("s"),
                    Col::int8("ms"),
                ],
                vec![vec![
                    Cell::Int(11),
                    Cell::text("COPY sales FROM 's3://b/x'"),
                    Cell::text("2026-10-29 10:00:00"),
                    Cell::Int(1500),
                ]],
            )
            .rows(
                "pg_cancel_backend(11)",
                vec![Col::int4("pg_cancel_backend")],
                vec![vec![Cell::Int(1)]],
            )
            .rows(
                "pg_terminate_backend(12)",
                vec![Col::bool("pg_terminate_backend")],
                vec![vec![Cell::Bool(false)]],
            )
            .start()
            .await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 80_002);
        let rows = REDSHIFT.list_processes(&c).await.unwrap().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].current);
        assert_eq!(rows[1].query.as_deref(), Some("COPY sales FROM 's3://b/x'"));
        let killed = REDSHIFT
            .kill(&c, &["11".into(), "x".into()], KillScope::Query)
            .await
            .unwrap();
        assert!(killed[0].ok && !killed[1].ok);
        let ended = REDSHIFT
            .kill(&c, &["12".into()], KillScope::Connection)
            .await
            .unwrap();
        assert!(!ended[0].ok);
        assert_eq!(
            testkit::sent(&pg, "pg_terminate_backend"),
            ["SELECT pg_terminate_backend(12)"]
        );

        // Serverless: no stv_ views.
        let pg = fake()
            .rows(
                "SELECT pg_backend_pid()",
                vec![Col::int4("pid")],
                vec![vec![Cell::Int(1_073_807_506)]],
            )
            .error(
                "FROM stv_sessions",
                "42P01",
                "relation \"stv_sessions\" does not exist",
            )
            .rows(
                "FROM sys_session_history",
                vec![
                    Col::int4("session_id"),
                    text_col("u"),
                    text_col("d"),
                    text_col("s"),
                ],
                vec![vec![
                    Cell::Int(1_073_807_506),
                    Cell::text("100"),
                    Cell::text("dev"),
                    Cell::text("2026-10-29 09:00:00"),
                ]],
            )
            .start()
            .await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 80_002);
        let rows = REDSHIFT.list_processes(&c).await.unwrap().unwrap();
        assert_eq!(rows[0].id, "1073807506");
        assert!(rows[0].current);
        assert_eq!(testkit::sent(&pg, "sys_query_history").len(), 1);
    }

    #[tokio::test]
    async fn the_ddl_is_shown_by_the_server() {
        let pg = fake()
            .rows(
                "c.reldiststyle::int FROM pg_class c",
                vec![
                    text_col("kind"),
                    text_col("comment"),
                    Col::int4("reldiststyle"),
                ],
                vec![vec![Cell::text("r"), Cell::Null, Cell::Int(1)]],
            )
            .rows(
                "SHOW TABLE",
                vec![text_col("Show Table DDL statement")],
                vec![vec![Cell::text(
                    "CREATE TABLE public.sales (\n    salesid integer NOT NULL ENCODE az64\n) \
                     DISTSTYLE KEY;",
                )]],
            )
            .start()
            .await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 80_002);
        let ddl = REDSHIFT
            .table_ddl(&c, &TableRef::new(Some("public"), "sales"))
            .await
            .unwrap()
            .unwrap();
        assert!(ddl.contains("DISTSTYLE KEY"), "{ddl}");
        assert_eq!(
            testkit::sent(&pg, "SHOW TABLE"),
            ["SHOW TABLE \"public\".\"sales\""]
        );
    }

    /// The grid's edits, built with Redshift SQL, run on the fake Redshift, which refuses
    /// RETURNING, ON CONFLICT, IS DISTINCT FROM and SAVEPOINT.
    #[tokio::test]
    async fn edits_in_redshift_sql_run_there() {
        let pg = fake()
            .command("UPDATE \"public\".\"sales\"", "UPDATE 1")
            .command("INSERT INTO \"public\".\"sales\"", "INSERT 0 1")
            .command("DELETE FROM \"public\".\"sales\"", "DELETE 2")
            .start()
            .await;
        let session = testkit::session(&testkit::profile(pg.port())).await;
        let mut id = blank_column("salesid".into(), 1, "integer".into(), ValueKind::Int);
        id.is_pk = true;
        id.pk_ordinal = Some(1);
        let note = blank_column(
            "note".into(),
            2,
            "character varying(256)".into(),
            ValueKind::Text,
        );
        let price = blank_column("price".into(), 3, "numeric(8,2)".into(), ValueKind::Decimal);
        let columns = [id, note, price];
        let table = TableRef::new(Some("public"), "sales");
        let target = TableTarget {
            table: &table,
            columns: &columns,
        };
        let key = RowKey::Pk([("salesid".to_string(), Value::Int(7))].into());
        let returning = session.capabilities().has(Feature::Returning);
        let changes = [
            StagedChange::Update {
                row_id: "r".into(),
                key: key.clone(),
                changes: [
                    ("note".to_string(), Value::Text("x".into())),
                    ("price".to_string(), Value::Decimal("9.99".into())),
                ]
                .into(),
                exprs: None,
            },
            StagedChange::Insert {
                row_id: "n".into(),
                values: [("salesid".to_string(), Value::Int(8))].into(),
                exprs: None,
            },
        ];
        let mut statements = Vec::new();
        for change in &changes {
            let (built, expect) = build_change(&SPEC, &target, change, returning).unwrap();
            statements.push(TxStatement {
                sql: built.sql,
                params: built.params,
                expect_affected: expect,
            });
        }
        let outcomes = session.transaction(statements).await.unwrap();
        assert_eq!(outcomes.len(), 2);
        let update = testkit::sent(&pg, "UPDATE \"public\"");
        assert_eq!(
            update,
            [
                "UPDATE \"public\".\"sales\" SET \"note\" = $1::varchar(65535), \"price\" = \
              $2::numeric(8,2) WHERE \"salesid\" = $3::integer"
            ]
        );
        // A delete that hits two rows (keys are not enforced) is rolled back.
        let delete = StagedChange::Delete {
            row_id: "r".into(),
            key,
        };
        let (built, expect) = build_change(&SPEC, &target, &delete, false).unwrap();
        let refused = session
            .transaction(vec![TxStatement {
                sql: built.sql,
                params: built.params,
                expect_affected: expect,
            }])
            .await;
        assert!(refused.is_err());
        assert_eq!(testkit::sent(&pg, "ROLLBACK").len(), 1);
        session.close().await;
    }
}
