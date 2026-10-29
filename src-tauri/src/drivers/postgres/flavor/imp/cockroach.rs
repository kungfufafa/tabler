//! CockroachDB. Its `pg_catalog` runs the PostgreSQL driver's queries, with gaps the hooks fill:
//! `crdb_internal` and `pg_extension` are system schemas; a table created without a primary key
//! has a hidden `rowid` key (`information_schema.columns.is_hidden`), which stays the edit key
//! but is not editable; `format_type` drops type modifiers, which `crdb_sql_type` keeps;
//! `reltuples` is NULL, so row counts come from `crdb_internal.table_row_statistics`;
//! `pg_stat_activity` is empty and `pg_cancel_backend` missing, so sessions come from
//! `crdb_internal.cluster_sessions` and kills are `CANCEL QUERIES` / `CANCEL SESSION` by session
//! id; DDL is the server's own (`crdb_internal.create_statements`); EXPLAIN takes only its own
//! options; and a transaction the server could not serialize (40001) runs again, as CockroachDB
//! asks of its clients.
//!
//! DDL in a transaction is not atomic on CockroachDB (`autocommit_before_ddl`), so structure
//! editing stays off; so do backups (pg_dump), users (no `aclexplode`), row edits without a key
//! (no `ctid`), other client encodings than UTF-8, the diagram (`relispartition` is NULL) and
//! the Create Database options (no collations or templates).

use async_trait::async_trait;

use super::catalog::{kill_outcome, optional, schema_of};
use crate::drivers::postgres::flavor::PgFlavorHooks;
use crate::drivers::postgres::introspect::{self, Traced};
use crate::error::AppError;
use crate::sqlgen::dialect::spec::ExplainForm;
use crate::types::admin::{KillOutcome, KillScope, ProcessRow};
use crate::types::caps::{Feature, SessionCaps};
use crate::types::driver::{KindId, PgFlavor};
use crate::types::profile::ConnectionProfile;
use crate::types::query::ExplainMode;
use crate::types::schema::{
    ColumnMeta, CompletionMetadata, SchemaInfo, TableMeta, TableRef, TableSummary,
};

pub(super) struct Cockroach;

pub(super) static COCKROACH: Cockroach = Cockroach;

/// The schemas CockroachDB keeps for itself beyond PostgreSQL's.
const SYSTEM_SCHEMAS: &[&str] = &["crdb_internal", "pg_extension"];

/// Defaults that make a column fill itself, like a sequence on PostgreSQL.
const AUTO_DEFAULTS: &[&str] = &[
    "unique_rowid()",
    "unordered_unique_rowid()",
    "gen_random_uuid()",
];

/// The tables of a schema with CockroachDB's row estimates (`reltuples` is NULL there); no sizes,
/// which CockroachDB reads from a slow, often stale system table.
const LIST_TABLES: &str = "SELECT c.relname::text, c.relkind::text, \
     obj_description(c.oid, 'pg_class')::text, s.estimated_row_count::int8 \
     FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
     LEFT JOIN crdb_internal.table_row_statistics s ON s.table_id = c.oid::int8 \
     WHERE n.nspname = $1 AND c.relkind IN ('r','p','v','m','f') ORDER BY c.relname";

/// `LIST_TABLES` without the estimates, for a server without that view.
const LIST_TABLES_PLAIN: &str = "SELECT c.relname::text, c.relkind::text, \
     obj_description(c.oid, 'pg_class')::text, NULL::int8 \
     FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
     WHERE n.nspname = $1 AND c.relkind IN ('r','p','v','m','f') ORDER BY c.relname";

/// What `pg_attribute` does not say of a table's columns: hidden ones and spelled types.
const COLUMN_FACTS: &str = "SELECT column_name::text, crdb_sql_type::text, is_hidden::text \
     FROM information_schema.columns WHERE table_schema = $1 AND table_name = $2";

const ROW_ESTIMATE: &str = "SELECT s.estimated_row_count::int8 \
     FROM crdb_internal.table_row_statistics s \
     JOIN pg_class c ON s.table_id = c.oid::int8 \
     JOIN pg_namespace n ON n.oid = c.relnamespace \
     WHERE n.nspname = $1 AND c.relname = $2";

/// The listing session's own id, as `cluster_sessions` spells it (hex).
const OWN_SESSION: &str = "SHOW session_id";

/// The cluster's sessions, every column as text.
const SESSIONS: &str = "SELECT session_id::text, user_name::text, application_name::text, \
     client_address::text, active_queries::text, last_active_query::text, \
     COALESCE(active_query_start, session_start)::text, \
     (EXTRACT(EPOCH FROM now() - COALESCE(active_query_start, session_start)) * 1000)::int8::text \
     FROM crdb_internal.cluster_sessions ORDER BY session_start";

const CANCEL_SESSION: &str = "CANCEL SESSION $1";

/// The running statements of a session (it may have none).
const CANCEL_QUERIES: &str = "CANCEL QUERIES IF EXISTS \
     SELECT query_id FROM crdb_internal.cluster_queries WHERE session_id = $1";

const CREATE_STATEMENT: &str = "SELECT create_statement::text FROM crdb_internal.create_statements \
     WHERE database_name = current_database() AND schema_name = $1 AND descriptor_name = $2";

#[async_trait]
impl PgFlavorHooks for Cockroach {
    fn flavor(&self) -> Option<PgFlavor> {
        Some(PgFlavor::Cockroachdb)
    }

    fn default_database(&self) -> &'static str {
        KindId::Cockroachdb
            .descriptor()
            .default_database
            .unwrap_or("defaultdb")
    }

    /// Without the DateStyle and IntervalStyle settings, which older versions refuse unless
    /// enabled and which change nothing Tabler reads (it sends dates as ISO text). A client
    /// encoding other than UTF-8 is refused by the server, so the connect fails.
    fn init_sql(&self, _: &ConnectionProfile, _: bool, base: Vec<String>) -> Vec<String> {
        base.into_iter()
            .filter(|s| !s.starts_with("SET DateStyle") && !s.starts_with("SET IntervalStyle"))
            .collect()
    }

    /// One failing statement then names itself.
    fn init_one_by_one(&self) -> bool {
        true
    }

    async fn list_schemas(&self, c: &Traced<'_>) -> Option<Result<Vec<SchemaInfo>, AppError>> {
        Some(introspect::list_schemas(c).await.map(|schemas| {
            schemas
                .into_iter()
                .filter(|s| !SYSTEM_SCHEMAS.contains(&s.name.as_str()))
                .collect()
        }))
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

    async fn completion_metadata(
        &self,
        c: &Traced<'_>,
    ) -> Option<Result<CompletionMetadata, AppError>> {
        Some(introspect::completion_metadata(c).await.map(|mut meta| {
            meta.schemas
                .retain(|s, _| !SYSTEM_SCHEMAS.contains(&s.as_str()));
            meta
        }))
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
            let result = match session_id(id) {
                Some(id) => {
                    let sql = match scope {
                        KillScope::Query => CANCEL_QUERIES,
                        KillScope::Connection => CANCEL_SESSION,
                    };
                    c.query(sql, &[&id])
                        .await
                        .map(|_| true)
                        .map_err(AppError::from)
                }
                None => Err(AppError::InvalidInput(format!("invalid session id {id}"))),
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

    /// CockroachDB's EXPLAIN takes VERBOSE, TYPES, OPT, VEC, DISTSQL… but not COSTS or FORMAT.
    fn explain_form(&self, mode: ExplainMode) -> Option<ExplainForm> {
        Some(match mode {
            ExplainMode::Plan => ExplainForm::Prefix("EXPLAIN (VERBOSE)"),
            ExplainMode::Analyze => ExplainForm::Prefix("EXPLAIN ANALYZE"),
        })
    }

    fn adjust_caps(&self, caps: &mut SessionCaps, _: i32) {
        *caps = caps
            .clone()
            // Sessions are listed and cancelled by this file's own SQL.
            .with(&[Feature::KillProcess])
            .without(&[
                Feature::EditWithoutKey,
                Feature::Encoding,
                Feature::Diagram,
                Feature::CreateDatabase,
            ]);
    }

    fn retry_serialization(&self) -> bool {
        true
    }
}

/// A CockroachDB session id: hexadecimal digits.
fn session_id(id: &str) -> Option<&str> {
    let id = id.trim();
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit())).then_some(id)
}

async fn list_tables(c: &Traced<'_>, schema: &str) -> Result<Vec<TableSummary>, AppError> {
    let rows = match c.query(LIST_TABLES, &[&schema]).await {
        Ok(rows) => rows,
        Err(e) if super::catalog::missing(&e) => c.query(LIST_TABLES_PLAIN, &[&schema]).await?,
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let relkind: String = row.try_get(1)?;
        out.push(TableSummary {
            name: row.try_get(0)?,
            schema: Some(schema.to_string()),
            kind: super::catalog::kind_of_relkind(&relkind),
            row_estimate: row.try_get(3)?,
            size_bytes: None,
            comment: row.try_get(2)?,
        });
    }
    Ok(out)
}

/// The PostgreSQL `table_meta`, with what CockroachDB's catalog leaves out.
async fn table_meta(c: &Traced<'_>, table: &TableRef) -> Result<TableMeta, AppError> {
    let schema = schema_of(c, table).await?;
    let table = TableRef::new(Some(&schema), &table.name);
    let mut meta = introspect::table_meta(c, &table).await?;
    let params: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[&schema, &table.name];
    for row in c.query(COLUMN_FACTS, params).await? {
        let name: String = row.try_get(0)?;
        let sql_type: Option<String> = row.try_get(1)?;
        let hidden: Option<String> = row.try_get(2)?;
        if let Some(col) = meta.columns.iter_mut().find(|c| c.name == name) {
            patch_column(col, sql_type.as_deref(), hidden.as_deref() == Some("YES"));
        }
    }
    if meta.row_estimate.is_none() {
        let rows = optional(c.query(ROW_ESTIMATE, params).await, "row estimate")?;
        meta.row_estimate = match rows.first() {
            Some(row) => row.try_get(0)?,
            None => None,
        };
    }
    Ok(meta)
}

/// Fills in what CockroachDB's `pg_catalog` gets wrong about a column: a hidden column (the
/// implicit `rowid` key) keeps its place as the key but cannot be edited; the type modifier
/// `format_type` drops comes from `crdb_sql_type`; `unique_rowid()` and `gen_random_uuid()`
/// defaults fill the column like a sequence.
fn patch_column(col: &mut ColumnMeta, sql_type: Option<&str>, hidden: bool) {
    if hidden {
        col.is_generated = true;
        col.extra = Some("hidden".into());
    }
    if col
        .default_expr
        .as_deref()
        .is_some_and(|d| AUTO_DEFAULTS.iter().any(|a| d.contains(a)))
    {
        col.is_auto_increment = true;
    }
    let Some(modifier) = sql_type.and_then(modifier_of) else {
        return;
    };
    if !col.data_type.contains('(') {
        col.data_type = with_modifier(&col.data_type, modifier);
    }
    let numbers: Vec<i64> = modifier
        .split(',')
        .filter_map(|n| n.trim().parse().ok())
        .collect();
    let ty = col.data_type.to_ascii_lowercase();
    if ty.contains("char") || ty.starts_with("bit") {
        col.char_max_len = col.char_max_len.or(numbers.first().copied());
    } else if ty.starts_with("numeric") || ty.starts_with("decimal") {
        let precision = numbers.first().and_then(|n| i32::try_from(*n).ok());
        let scale = numbers.get(1).and_then(|n| i32::try_from(*n).ok());
        col.numeric_precision = col.numeric_precision.or(precision);
        col.numeric_scale = col.numeric_scale.or(scale).or(precision.map(|_| 0));
    }
}

/// `10,2` of `DECIMAL(10,2)`; None for a type without a modifier.
fn modifier_of(sql_type: &str) -> Option<&str> {
    let open = sql_type.find('(')?;
    let close = open + sql_type[open..].find(')')?;
    let inner = sql_type[open + 1..close].trim();
    (!inner.is_empty()
        && inner
            .chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c == ' '))
    .then_some(inner)
}

/// `format_type`'s name with the modifier where PostgreSQL spells it: `character varying(10)`,
/// `timestamp(3) with time zone`, `numeric(10,2)[]`.
fn with_modifier(data_type: &str, modifier: &str) -> String {
    let (base, array) = match data_type.strip_suffix("[]") {
        Some(base) => (base, "[]"),
        None => (data_type, ""),
    };
    let lower = base.to_ascii_lowercase();
    if (lower.starts_with("timestamp ") || lower.starts_with("time "))
        && let Some((first, rest)) = base.split_once(' ')
    {
        return format!("{first}({modifier}) {rest}{array}");
    }
    format!("{base}({modifier}){array}")
}

async fn list_processes(c: &Traced<'_>) -> Result<Vec<ProcessRow>, AppError> {
    // By id, not by the text of the running query: another listing, or any query naming
    // `cluster_sessions`, would match that too. A server without the setting marks none.
    let own = match optional(c.query(OWN_SESSION, &[]).await, "session_id")?.first() {
        Some(row) => row.try_get::<_, Option<String>>(0)?,
        None => None,
    };
    let own = own.map(|id| id.trim().to_ascii_lowercase());
    let mut out = Vec::new();
    for row in c.query(SESSIONS, &[]).await? {
        let text = |i: usize| -> Result<Option<String>, AppError> {
            Ok(row
                .try_get::<_, Option<String>>(i)?
                .filter(|s| !s.trim().is_empty()))
        };
        let Some(id) = text(0)? else {
            continue;
        };
        let application = text(2)?;
        let active = text(4)?;
        let current = own.as_deref() == Some(id.trim().to_ascii_lowercase().as_str());
        out.push(ProcessRow {
            id,
            user: text(1)?,
            database: None,
            own: current || application.as_deref() == Some("tabler"),
            application,
            client: text(3)?,
            state: Some(if active.is_some() { "active" } else { "idle" }.into()),
            command: None,
            started_at: text(6)?,
            duration_ms: text(7)?.and_then(|d| d.trim().parse().ok()),
            wait_event: None,
            query: match active {
                Some(q) => Some(q),
                None => text(5)?,
            },
            current,
        });
    }
    Ok(out)
}

async fn table_ddl(c: &Traced<'_>, table: &TableRef) -> Result<String, AppError> {
    let schema = schema_of(c, table).await?;
    let row = c
        .query_opt(CREATE_STATEMENT, &[&schema, &table.name])
        .await?
        .ok_or_else(|| AppError::NotFound(format!("table {schema}.{}", table.name)))?;
    let ddl: Option<String> = row.try_get(0)?;
    let ddl = ddl.unwrap_or_default();
    let ddl = ddl.trim_end();
    Ok(if ddl.ends_with(';') {
        ddl.to_string()
    } else {
        format!("{ddl};")
    })
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{self, COCKROACH as VERSION, CoreColumn};
    use super::*;
    use crate::drivers::DbSession;
    use crate::drivers::postgres::testing::{Cell, Col, FakePg};
    use crate::sqlgen::SqlDialect;
    use crate::types::driver::Flavor;
    use crate::types::driver::ReadOnlyMode;
    use crate::types::schema::ValueKind;

    fn fake() -> FakePg {
        FakePg::new().server_version("13.0.0").version(VERSION)
    }

    #[tokio::test]
    async fn a_session_on_cockroachdb() {
        let pg = fake()
            .error("SET DateStyle", "22023", "invalid value for parameter")
            .error("SET IntervalStyle", "22023", "invalid value for parameter")
            .start()
            .await;
        let mut profile = testkit::profile(pg.port());
        profile.options.read_only = true;
        // The profile's engine picks the database to connect to.
        profile.options.pg_flavor = Some(PgFlavor::Cockroachdb);
        profile.options.timezone = Some("UTC".into());
        let session = testkit::session(&profile).await;
        assert_eq!(session.flavor(), Some(Flavor::Cockroachdb));
        assert_eq!(session.dialect(), SqlDialect::Postgres);
        let database = ("database".to_string(), "defaultdb".to_string());
        assert!(pg.startups()[0].contains(&database), "{:?}", pg.startups());
        // The init, one statement at a time and without the date and interval styles.
        assert_eq!(
            pg.queries()[..5],
            [
                "SELECT version()",
                "SET extra_float_digits = 3;",
                "SET client_encoding = 'UTF8';",
                "SET TIME ZONE 'UTC';",
                "SET default_transaction_read_only = on;",
            ]
        );
        let caps = session.capabilities();
        assert_eq!(caps.safety.read_only, ReadOnlyMode::Server);
        assert!(caps.has(Feature::KillProcess) && caps.has(Feature::ProcessList));
        assert!(caps.has(Feature::EditRows) && caps.has(Feature::TableDdl));
        for f in [
            Feature::StructureEdit,
            Feature::Backup,
            Feature::Restore,
            Feature::Users,
            Feature::EditWithoutKey,
            Feature::Encoding,
            Feature::Diagram,
            Feature::CreateDatabase,
        ] {
            assert!(!caps.has(f), "{f:?}");
        }
        assert_eq!(
            session.explain_form(ExplainMode::Plan),
            Some(ExplainForm::Prefix("EXPLAIN (VERBOSE)"))
        );
        assert!(COCKROACH.retry_serialization());
        session.close().await;
    }

    #[tokio::test]
    async fn hides_the_system_schemas() {
        let pg = fake()
            .rows(
                "FROM pg_namespace n WHERE",
                vec![Col::text("nspname"), Col::bool("is_default")],
                ["crdb_internal", "pg_extension", "public", "app"]
                    .iter()
                    .map(|s| vec![Cell::text(*s), Cell::Bool(*s == "public")])
                    .collect(),
            )
            .start()
            .await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 130_000);
        let schemas = COCKROACH.list_schemas(&c).await.unwrap().unwrap();
        let names: Vec<&str> = schemas.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["public", "app"]);
    }

    #[tokio::test]
    async fn lists_tables_with_row_estimates_or_without() {
        let row = |name: &str, kind: &str, rows: Option<i64>| {
            vec![
                Cell::text(name),
                Cell::text(kind),
                Cell::Null,
                rows.map_or(Cell::Null, Cell::Int),
            ]
        };
        let columns = vec![
            Col::text("relname"),
            Col::text("relkind"),
            Col::text("comment"),
            Col::int8("estimated_row_count"),
        ];
        let pg = fake()
            .rows(
                "table_row_statistics s ON",
                columns.clone(),
                vec![row("orders", "r", Some(1200)), row("v", "v", None)],
            )
            .start()
            .await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 130_000);
        let tables = COCKROACH.list_tables(&c, "public").await.unwrap().unwrap();
        assert_eq!(tables[0].row_estimate, Some(1200));
        assert_eq!(tables[0].size_bytes, None);
        assert_eq!(tables[1].kind, crate::types::schema::TableKind::View);

        // An older server without the statistics view: the plain list.
        let pg = fake()
            .error(
                "table_row_statistics",
                "42P01",
                "relation \"crdb_internal.table_row_statistics\" does not exist",
            )
            .rows("NULL::int8", columns, vec![row("orders", "r", None)])
            .start()
            .await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 130_000);
        let tables = COCKROACH.list_tables(&c, "public").await.unwrap().unwrap();
        assert_eq!(tables.len(), 1);
        assert!(
            !pg.queries()
                .iter()
                .any(|q| q.contains("pg_total_relation_size"))
        );
    }

    #[tokio::test]
    async fn the_hidden_rowid_stays_the_key_and_types_keep_their_modifiers() {
        let pg = testkit::core_table(
            fake(),
            "r",
            &[
                CoreColumn {
                    attnum: 1,
                    name: "name",
                    data_type: "character varying",
                    typname: "varchar",
                    nullable: true,
                    default: None,
                    typmod: -1,
                },
                CoreColumn {
                    attnum: 2,
                    name: "price",
                    data_type: "numeric",
                    typname: "numeric",
                    nullable: true,
                    default: None,
                    typmod: -1,
                },
                CoreColumn {
                    attnum: 3,
                    name: "rowid",
                    data_type: "bigint",
                    typname: "int8",
                    nullable: false,
                    default: Some("unique_rowid()"),
                    typmod: -1,
                },
            ],
            &["rowid"],
        )
        .rows(
            "FROM information_schema.columns",
            vec![
                Col::text("column_name"),
                Col::text("crdb_sql_type"),
                Col::text("is_hidden"),
            ],
            vec![
                vec![
                    Cell::text("name"),
                    Cell::text("VARCHAR(10)"),
                    Cell::text("NO"),
                ],
                vec![
                    Cell::text("price"),
                    Cell::text("DECIMAL(10,2)"),
                    Cell::text("NO"),
                ],
                vec![Cell::text("rowid"), Cell::text("INT8"), Cell::text("YES")],
            ],
        )
        .rows(
            "FROM crdb_internal.table_row_statistics s",
            vec![Col::int8("estimated_row_count")],
            vec![vec![Cell::Int(5)]],
        )
        .start()
        .await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 130_000);
        let meta = COCKROACH
            .table_meta(&c, &TableRef::new(Some("public"), "t"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(meta.primary_key, ["rowid"]);
        let col = |n: &str| meta.columns.iter().find(|c| c.name == n).unwrap();
        assert_eq!(col("name").data_type, "character varying(10)");
        assert_eq!(col("name").char_max_len, Some(10));
        assert_eq!(col("price").data_type, "numeric(10,2)");
        assert_eq!(
            (col("price").numeric_precision, col("price").numeric_scale),
            (Some(10), Some(2))
        );
        let rowid = col("rowid");
        assert!(rowid.is_pk && rowid.is_generated && rowid.is_auto_increment);
        assert_eq!(rowid.extra.as_deref(), Some("hidden"));
        assert_eq!(rowid.kind, ValueKind::BigInt);
        assert_eq!(meta.row_estimate, Some(5));
    }

    #[test]
    fn spells_modifiers_where_postgres_does() {
        assert_eq!(modifier_of("VARCHAR(10)"), Some("10"));
        assert_eq!(modifier_of("DECIMAL(10,2)[]"), Some("10,2"));
        assert_eq!(modifier_of("INT8"), None);
        assert_eq!(modifier_of("STRING(x)"), None);
        assert_eq!(
            with_modifier("timestamp with time zone", "3"),
            "timestamp(3) with time zone"
        );
        assert_eq!(
            with_modifier("character varying[]", "5"),
            "character varying(5)[]"
        );
        assert_eq!(with_modifier("bit", "8"), "bit(8)");
        assert_eq!(session_id(" 17f5c1a2b3 "), Some("17f5c1a2b3"));
        assert_eq!(session_id("1; DROP"), None);
        assert_eq!(session_id(""), None);
    }

    #[tokio::test]
    async fn lists_and_cancels_sessions_by_id() {
        let text_row = |cells: [Option<&str>; 8]| {
            cells
                .iter()
                .map(|c| c.map_or(Cell::Null, Cell::text))
                .collect::<Vec<_>>()
        };
        let sessions = vec![
            text_row([
                Some("17f5c1a2b3c4d5e60000000000000001"),
                Some("root"),
                Some("tabler"),
                Some("127.0.0.1:52144"),
                Some("SELECT session_id::text … crdb_internal.cluster_sessions"),
                Some("SHOW tables"),
                Some("2026-10-29 10:00:00+00"),
                Some("12"),
            ]),
            text_row([
                Some("17f5c1a2b3c4d5e60000000000000002"),
                Some("app"),
                Some(""),
                Some("10.0.0.5:4000"),
                Some(""),
                Some("SELECT 1"),
                Some("2026-10-29 09:00:00+00"),
                Some("3600000"),
            ]),
            // Another client listing sessions at the same moment.
            text_row([
                Some("17f5c1a2b3c4d5e60000000000000003"),
                Some("admin"),
                Some("cockroach sql"),
                Some("10.0.0.6:4100"),
                Some("SELECT * FROM crdb_internal.cluster_sessions"),
                Some(""),
                Some("2026-10-29 10:00:01+00"),
                Some("5"),
            ]),
        ];
        let columns = || -> Vec<Col> { (0..8).map(|i| Col::text(&format!("c{i}"))).collect() };
        let pg = fake()
            .rows(
                "SHOW session_id",
                vec![Col::text("session_id")],
                vec![vec![Cell::text("17f5c1a2b3c4d5e60000000000000001")]],
            )
            .rows(
                "FROM crdb_internal.cluster_sessions ORDER BY",
                columns(),
                sessions.clone(),
            )
            .error(
                "CANCEL SESSION",
                "42501",
                "user app does not have privilege",
            )
            .start()
            .await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 130_000);
        let rows = COCKROACH.list_processes(&c).await.unwrap().unwrap();
        assert!(rows[0].current && rows[0].own);
        assert_eq!(rows[0].state.as_deref(), Some("active"));
        assert_eq!(rows[1].state.as_deref(), Some("idle"));
        assert_eq!(rows[1].query.as_deref(), Some("SELECT 1"));
        assert_eq!(rows[1].application, None);
        assert_eq!(rows[1].duration_ms, Some(3_600_000));
        assert!(!rows[1].own);
        assert!(!rows[2].current && !rows[2].own);

        // A server without the setting lists every session and marks none.
        let older = fake()
            .error(
                "SHOW session_id",
                "42704",
                "unrecognized configuration parameter \"session_id\"",
            )
            .rows(
                "FROM crdb_internal.cluster_sessions ORDER BY",
                columns(),
                sessions,
            )
            .start()
            .await;
        let (older_client, older_trace) = testkit::client(&older).await;
        let listed = COCKROACH
            .list_processes(&Traced::new(&older_client, &older_trace, 130_000))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(listed.len(), 3);
        assert!(listed.iter().all(|p| !p.current));
        assert!(listed[0].own);

        let ids = [rows[1].id.clone(), "x'; DROP".to_string()];
        let cancelled = COCKROACH.kill(&c, &ids, KillScope::Query).await.unwrap();
        assert!(cancelled[0].ok);
        assert!(!cancelled[1].ok);
        let ended = COCKROACH
            .kill(&c, &ids[..1], KillScope::Connection)
            .await
            .unwrap();
        assert!(!ended[0].ok);
        assert!(ended[0].message.as_deref().unwrap().contains("privilege"));
        assert_eq!(testkit::sent(&pg, "CANCEL QUERIES IF EXISTS").len(), 1);
        assert_eq!(testkit::sent(&pg, "CANCEL SESSION $1").len(), 1);
        assert!(!pg.queries().iter().any(|q| q.contains("DROP")));
    }

    #[tokio::test]
    async fn the_ddl_is_the_servers() {
        let pg = fake()
            .rows(
                "FROM crdb_internal.create_statements",
                vec![Col::text("create_statement")],
                vec![vec![Cell::text(
                    "CREATE TABLE public.t (\n\tid INT8 NOT NULL,\n\tCONSTRAINT t_pkey PRIMARY \
                     KEY (id ASC)\n)",
                )]],
            )
            .start()
            .await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 130_000);
        let ddl = COCKROACH
            .table_ddl(&c, &TableRef::new(Some("public"), "t"))
            .await
            .unwrap()
            .unwrap();
        assert!(ddl.starts_with("CREATE TABLE public.t") && ddl.ends_with(");"));
        assert_eq!(testkit::sent(&pg, "create_statements"), [CREATE_STATEMENT]);
    }
}
