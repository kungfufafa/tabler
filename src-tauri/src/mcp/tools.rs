//! The tools MCP clients call: list the saved connections, browse a server's databases, tables
//! and table structure, run a query and explain one. Every call re-reads the preferences and the
//! saved connections, so changes made in the app apply at once. Queries go through the safety
//! gate (`gate`) and are logged in the query history with source `mcp`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value as Json, json};
use uuid::Uuid;

use crate::appdb::AppDb;
use crate::commands::query::{explain_inner, run_sql_inner};
use crate::drivers::{CancelSlot, DbSession, request_cancel};
use crate::error::{AppError, ErrorPayload};
use crate::export::structure::pg_table_context;
use crate::sqlgen::Dialect;
use crate::sqlgen::ddl::create_table_sql_with;
use crate::sqlgen::split::split_statements;
use crate::state::AppState;
use crate::types::driver::Location;
use crate::types::history::NewHistoryEntry;
use crate::types::profile::{ConnectionProfile, Driver};
use crate::types::query::{ExplainMode, QueryEvent, ResultColumn, RunOptions, StatementKind};
use crate::types::schema::{TableKind, TableMeta, TableRef};
use crate::types::value::Value;

use super::gate::{self, level_label, safe_mode};
use super::output::{Budget, DEFAULT_ROWS, MAX_BYTES, MAX_ROWS, columns_json, rows_json};
use super::pool::{Lease, Pool};
use super::prefs::McpPrefs;

/// What a call returns while the server is turned off in Settings.
pub const DISABLED_MESSAGE: &str = "Tabler's MCP server is turned off. Turn it on in Tabler \u{25b8} \
     Settings \u{25b8} MCP Server.";
/// History `source` of the statements MCP clients run.
pub const HISTORY_SOURCE: &str = "mcp";
/// Longer statements are stored truncated in the history (as the editor does).
const HISTORY_SQL_MAX_CHARS: usize = 100_000;
/// Statement text echoed back with its result.
const ECHO_SQL_MAX_CHARS: usize = 300;
/// The longest `sql` argument `run_query` and `explain_query` take (1 MiB).
pub const MAX_SQL_BYTES: usize = 1024 * 1024;

/// The server's state shared by every call.
pub struct Ctx {
    pub state: Arc<AppState>,
    pub pool: Pool,
}

impl Ctx {
    pub fn new(state: Arc<AppState>) -> Ctx {
        Ctx {
            pool: Pool::new(state.clone()),
            state,
        }
    }

    pub fn appdb(&self) -> &AppDb {
        &self.state.appdb
    }
}

/// Cancels a running call (`notifications/cancelled`): the statement on the server too.
#[derive(Default)]
pub struct CallCancel {
    pub slot: CancelSlot,
    session: Mutex<Option<Arc<dyn DbSession>>>,
    cancelled: AtomicBool,
}

impl CallCancel {
    fn attach(&self, session: Arc<dyn DbSession>) {
        *self.session.lock().unwrap_or_else(|p| p.into_inner()) = Some(session);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub async fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let session = self
            .session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if let (Some(handle), Some(session)) = (request_cancel(&self.slot), session)
            && let Err(e) = session.cancel(handle).await
        {
            log::warn!("mcp: cancelling a statement failed: {e}");
        }
    }
}

/// A call's result: JSON text for the model, and whether it reports an error.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}

impl ToolOutput {
    fn ok(value: Json) -> ToolOutput {
        ToolOutput {
            text: value.to_string(),
            is_error: false,
        }
    }

    fn failed(message: impl Into<String>) -> ToolOutput {
        ToolOutput {
            text: message.into(),
            is_error: true,
        }
    }
}

/// The client called a tool that does not exist (a protocol error, not a tool error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownTool(pub String);

type Fail = String;

fn tool(
    name: &str,
    title: &str,
    description: &str,
    properties: Json,
    required: &[&str],
    read_only: bool,
) -> Json {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
        "annotations": {
            "title": title,
            "readOnlyHint": read_only,
            "destructiveHint": !read_only,
            "idempotentHint": read_only,
            "openWorldHint": false,
        },
    })
}

fn connection_prop() -> Json {
    json!({
        "type": "string",
        "description": "The saved connection: its name or id, as list_connections returns them.",
    })
}

fn database_prop() -> Json {
    json!({
        "type": "string",
        "description": "Another database on the same server (PostgreSQL / MySQL). Default: the \
                        connection's own database.",
    })
}

/// `tools/list`.
pub fn definitions() -> Vec<Json> {
    vec![
        tool(
            "list_connections",
            "List connections",
            "Lists the database connections saved in Tabler: id, name, driver (postgres, mysql or \
             sqlite), tag, database, safe mode, whether it is read-only and whether MCP clients may \
             change data on it. Passwords are never included.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "list_databases",
            "List databases",
            "Lists the databases on the server of a saved connection.",
            json!({ "connection": connection_prop() }),
            &["connection"],
            true,
        ),
        tool(
            "list_tables",
            "List tables",
            "Lists the tables and views of a schema (PostgreSQL schema, MySQL database, SQLite \
             main) with their kind, estimated row count and comment.",
            json!({
                "connection": connection_prop(),
                "schema": {
                    "type": "string",
                    "description": "Default: the connection's default schema (PostgreSQL: usually \
                                    public; MySQL: the current database; SQLite: main).",
                },
                "database": database_prop(),
            }),
            &["connection"],
            true,
        ),
        tool(
            "describe_table",
            "Describe table",
            "Describes a table or view: columns (type, nullability, default, key), primary key, \
             indexes, foreign keys, check constraints, triggers and its CREATE statement.",
            json!({
                "connection": connection_prop(),
                "table": {
                    "type": "string",
                    "description": "The table name; `schema.table` also works.",
                },
                "schema": {
                    "type": "string",
                    "description": "Default: the connection's default schema.",
                },
                "database": database_prop(),
            }),
            &["connection", "table"],
            true,
        ),
        tool(
            "run_query",
            "Run query",
            "Runs SQL on a saved connection and returns the result rows as JSON objects (at most \
             `limit` rows, default 100, up to 500, and 1 MB in all). Several statements separated \
             by `;` run in order and stop at the first error. Read-only by default: statements \
             that change data or the schema are refused unless the user allowed MCP clients to \
             change data in Tabler and the connection is in Silent Mode and not read-only \
             (list_connections shows canChangeData). MySQL's USE is not supported: pass `database` \
             instead. Every query is logged in Tabler's history.",
            json!({
                "connection": connection_prop(),
                "sql": { "type": "string", "description": "The SQL to run (at most 1 MiB)." },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_ROWS,
                    "description": "Most rows returned (default 100, up to 500).",
                },
                "database": database_prop(),
            }),
            &["connection", "sql"],
            false,
        ),
        tool(
            "explain_query",
            "Explain query",
            "Shows the query plan of one statement (EXPLAIN; the statement itself is not run).",
            json!({
                "connection": connection_prop(),
                "sql": { "type": "string", "description": "The statement to explain (at most 1 MiB)." },
                "database": database_prop(),
            }),
            &["connection", "sql"],
            true,
        ),
    ]
}

// ----- arguments -------------------------------------------------------------------------------

fn opt_str<'a>(args: &'a Json, key: &str) -> Result<Option<&'a str>, Fail> {
    match args.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Json::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

fn req_str<'a>(args: &'a Json, key: &str) -> Result<&'a str, Fail> {
    opt_str(args, key)?.ok_or_else(|| format!("`{key}` is required"))
}

/// The `sql` argument, refused when it is longer than [`MAX_SQL_BYTES`] (before anything is parsed,
/// run or written to the history).
fn sql_arg(args: &Json) -> Result<&str, Fail> {
    let sql = req_str(args, "sql")?;
    if sql.len() > MAX_SQL_BYTES {
        return Err(format!(
            "`sql` is too long ({} KiB): at most {} KiB per call. Send it in smaller calls.",
            sql.len().div_ceil(1024),
            MAX_SQL_BYTES / 1024
        ));
    }
    Ok(sql)
}

fn opt_u32(args: &Json, key: &str) -> Result<Option<u32>, Fail> {
    match args.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .or_else(|| {
                v.as_f64()
                    .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                    .map(|f| f as u64)
            })
            .map(|n| Some(n.min(u64::from(u32::MAX)) as u32))
            .ok_or_else(|| format!("`{key}` must be a positive whole number")),
    }
}

// ----- connections -----------------------------------------------------------------------------

/// The SQL dialect `run_query` classifies a profile's statements with, before it connects: the
/// core drivers only (the MCP gate has no classifier for the others yet).
fn dialect_of(driver: Driver) -> Result<Dialect, Fail> {
    match driver {
        Driver::Postgres => Ok(Dialect::Postgres),
        Driver::Mysql => Ok(Dialect::Mysql),
        Driver::Sqlite => Ok(Dialect::Sqlite),
        other => Err(format!(
            "MCP clients cannot run SQL on {} connections yet.",
            other.label()
        )),
    }
}

fn hidden_message(p: &ConnectionProfile) -> String {
    format!(
        "\u{201c}{}\u{201d} is tagged Production: MCP clients can use production connections \
         only when \u{201c}Expose production connections\u{201d} is on in Tabler \u{25b8} Settings \
         \u{25b8} MCP Server.",
        p.name
    )
}

/// The saved connection a call names (its id, or its name ignoring case).
pub fn resolve(
    profiles: &[ConnectionProfile],
    prefs: &McpPrefs,
    wanted: &str,
) -> Result<ConnectionProfile, Fail> {
    let wanted = wanted.trim();
    let visible = |p: &ConnectionProfile| -> Result<ConnectionProfile, Fail> {
        if gate::exposed(p, prefs) {
            Ok(p.clone())
        } else {
            Err(hidden_message(p))
        }
    };
    if let Ok(id) = Uuid::parse_str(wanted)
        && let Some(p) = profiles.iter().find(|p| p.id == id)
    {
        return visible(p);
    }
    let same: Vec<&ConnectionProfile> = profiles
        .iter()
        .filter(|p| p.name.trim().eq_ignore_ascii_case(wanted))
        .collect();
    let exact: Vec<&ConnectionProfile> = same
        .iter()
        .copied()
        .filter(|p| p.name.trim() == wanted)
        .collect();
    let pick = if exact.len() == 1 { exact } else { same };
    match pick.as_slice() {
        [] => Err(format!(
            "No saved connection is named \u{201c}{wanted}\u{201d}. Call list_connections to see them."
        )),
        [one] => visible(one),
        many => {
            let shown: Vec<&&ConnectionProfile> =
                many.iter().filter(|p| gate::exposed(p, prefs)).collect();
            match shown.as_slice() {
                [one] => Ok((**one).clone()),
                [] => Err(hidden_message(many[0])),
                _ => Err(format!(
                    "Several connections are named \u{201c}{wanted}\u{201d}; pass the id of one: {}",
                    shown
                        .iter()
                        .map(|p| p.id.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }
    }
}

fn database_label(p: &ConnectionProfile) -> Option<String> {
    match p.driver.descriptor().location {
        Location::File => p.sqlite_path.clone(),
        _ => p.database.clone().filter(|d| !d.trim().is_empty()),
    }
}

fn list_connections(profiles: &[ConnectionProfile], prefs: &McpPrefs) -> ToolOutput {
    let mut hidden = 0;
    let connections: Vec<Json> = profiles
        .iter()
        .filter(|p| {
            let shown = gate::exposed(p, prefs);
            hidden += usize::from(!shown);
            shown
        })
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "driver": p.driver,
                "tag": p.options.tag.as_ref().map(|t| t.as_str()),
                "group": p.group,
                "database": database_label(p),
                "safeMode": level_label(safe_mode(p)),
                "readOnly": p.options.read_only,
                "canChangeData": gate::session_writable(p, prefs),
            })
        })
        .collect();
    let mut out = json!({ "connections": connections });
    if hidden > 0 {
        out["hiddenProductionConnections"] = json!(hidden);
    }
    if !prefs.allow_writes {
        out["note"] = json!(
            "MCP clients can only read: changing data is off in Tabler \u{25b8} Settings \u{25b8} \
             MCP Server."
        );
    }
    ToolOutput::ok(out)
}

/// Whether an error means the connection itself is gone (the next call reconnects).
fn lost_connection(code: &str) -> bool {
    matches!(
        code,
        "CONNECT_FAILED" | "TIMEOUT" | "SSH_TUNNEL" | "NOT_CONNECTED" | "IO"
    )
}

async fn lease(
    ctx: &Ctx,
    prefs: &McpPrefs,
    profile: &ConnectionProfile,
    database: Option<&str>,
) -> Result<Lease, Fail> {
    let writable = gate::session_writable(profile, prefs);
    ctx.pool
        .get(profile, database, writable)
        .await
        .map_err(|e| format!("Could not connect to \u{201c}{}\u{201d}: {e}", profile.name))
}

/// Ends a call's use of the connection; a connection-level failure drops it, so the next call
/// reconnects.
async fn finish<T>(ctx: &Ctx, lease: Lease, result: Result<T, AppError>) -> Result<T, Fail> {
    match result {
        Ok(v) => Ok(v),
        Err(e) => {
            if lost_connection(e.code()) {
                ctx.pool.discard(lease).await;
            }
            Err(e.to_string())
        }
    }
}

async fn default_schema(session: &dyn DbSession) -> Result<(String, Vec<String>), AppError> {
    let schemas = session.list_schemas().await?;
    let names: Vec<String> = schemas.iter().map(|s| s.name.clone()).collect();
    let default = schemas
        .iter()
        .find(|s| s.is_default)
        .or(schemas.first())
        .map(|s| s.name.clone())
        .ok_or_else(|| AppError::NotFound("the server reports no schema".into()))?;
    Ok((default, names))
}

fn kind_json(kind: TableKind) -> Json {
    serde_json::to_value(kind).unwrap_or(Json::Null)
}

// ----- history ---------------------------------------------------------------------------------

fn history_sql(sql: &str) -> String {
    match sql.char_indices().nth(HISTORY_SQL_MAX_CHARS) {
        Some((byte, _)) => format!("{}\n-- … (truncated)", &sql[..byte]),
        None => sql.to_string(),
    }
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn history_row(
    profile: &ConnectionProfile,
    database: Option<String>,
    sql: &str,
) -> NewHistoryEntry {
    NewHistoryEntry {
        profile_id: Some(profile.id),
        connection_name: profile.name.clone(),
        database,
        sql: history_sql(sql),
        started_at: now_rfc3339(),
        duration_ms: None,
        row_count: None,
        affected: None,
        success: true,
        error: None,
        source: HISTORY_SOURCE.into(),
    }
}

fn record(appdb: &AppDb, rows: &[NewHistoryEntry]) {
    if let Err(e) = appdb.insert_history(rows) {
        log::warn!("mcp: could not record the query history: {e}");
    }
}

// ----- tools -----------------------------------------------------------------------------------

async fn list_databases(ctx: &Ctx, prefs: &McpPrefs, args: &Json) -> Result<ToolOutput, Fail> {
    let profile = resolve(
        &ctx.appdb().list_profiles().map_err(|e| e.to_string())?,
        prefs,
        req_str(args, "connection")?,
    )?;
    let lease = lease(ctx, prefs, &profile, None).await?;
    let result = lease.session.list_databases().await;
    let dbs = finish(ctx, lease, result).await?;
    let databases: Vec<Json> = dbs
        .iter()
        .map(|d| json!({ "name": d.name, "current": d.is_current }))
        .collect();
    Ok(ToolOutput::ok(
        json!({ "connection": profile.name, "databases": databases }),
    ))
}

async fn list_tables(ctx: &Ctx, prefs: &McpPrefs, args: &Json) -> Result<ToolOutput, Fail> {
    let profile = resolve(
        &ctx.appdb().list_profiles().map_err(|e| e.to_string())?,
        prefs,
        req_str(args, "connection")?,
    )?;
    let schema = opt_str(args, "schema")?.map(str::to_string);
    let lease = lease(ctx, prefs, &profile, opt_str(args, "database")?).await?;
    let s = lease.session.clone();
    let result = async {
        let (schema, schemas) = match schema {
            Some(name) => (name, None),
            None => {
                let (name, all) = default_schema(s.as_ref()).await?;
                (name, Some(all))
            }
        };
        let tables = s.list_tables(&schema).await?;
        Ok((schema, schemas, tables))
    }
    .await;
    let (schema, schemas, tables) = finish(ctx, lease, result).await?;
    let tables: Vec<Json> = tables
        .iter()
        .map(|t| {
            let mut o = json!({ "name": t.name, "kind": kind_json(t.kind) });
            if let Some(n) = t.row_estimate.filter(|n| *n >= 0) {
                o["rowEstimate"] = json!(n);
            }
            if let Some(c) = t.comment.as_deref().filter(|c| !c.is_empty()) {
                o["comment"] = json!(c);
            }
            o
        })
        .collect();
    let mut out = json!({ "connection": profile.name, "schema": schema, "tables": tables });
    if let Some(all) = schemas.filter(|a| a.len() > 1) {
        out["schemas"] = json!(all);
    }
    Ok(ToolOutput::ok(out))
}

/// The CREATE statement of a table or view (as Structure ▸ DDL shows it).
async fn table_ddl(session: &dyn DbSession, meta: &TableMeta) -> Result<String, AppError> {
    if let Some(ddl) = meta.ddl.as_deref().filter(|d| !d.trim().is_empty()) {
        return Ok(ddl.to_string());
    }
    let dialect = session.dialect();
    match meta.kind {
        TableKind::View | TableKind::MaterializedView | TableKind::SystemView => {
            let definition = session
                .view_definition(&meta.table)
                .await?
                .unwrap_or_default();
            let what = if meta.kind == TableKind::MaterializedView {
                "MATERIALIZED VIEW"
            } else {
                "VIEW"
            };
            let body = definition.trim();
            let body = body.strip_suffix(';').unwrap_or(body).trim_end();
            Ok(format!(
                "CREATE {what} {} AS\n{body};",
                dialect.qualify(meta.table.schema.as_deref(), &meta.table.name)
            ))
        }
        _ => {
            let context = if dialect == Dialect::Postgres {
                pg_table_context(session, &meta.table, false)
                    .await
                    .unwrap_or_default()
            } else {
                Default::default()
            };
            Ok(create_table_sql_with(dialect, meta, &context))
        }
    }
}

fn describe_json(meta: &TableMeta, ddl: Option<String>) -> Json {
    let columns: Vec<Json> = meta
        .columns
        .iter()
        .map(|c| {
            let mut o = json!({
                "name": c.name,
                "type": c.data_type,
                "nullable": c.nullable,
            });
            if let Some(d) = &c.default_expr {
                o["default"] = json!(d);
            }
            if c.is_pk {
                o["primaryKey"] = json!(true);
            }
            if c.is_auto_increment {
                o["autoIncrement"] = json!(true);
            }
            if c.is_generated {
                o["generated"] = json!(true);
            }
            if let Some(v) = c.enum_values.as_ref().filter(|v| !v.is_empty()) {
                o["enumValues"] = json!(v);
            }
            if let Some(cm) = c.comment.as_deref().filter(|c| !c.is_empty()) {
                o["comment"] = json!(cm);
            }
            o
        })
        .collect();
    let indexes: Vec<Json> = meta
        .indexes
        .iter()
        .map(|i| {
            json!({
                "name": i.name,
                "columns": i.columns,
                "unique": i.unique,
                "primary": i.primary,
                "method": i.method,
                "where": i.predicate,
            })
        })
        .collect();
    let foreign_keys: Vec<Json> = meta
        .foreign_keys
        .iter()
        .map(|f| {
            json!({
                "name": f.name,
                "columns": f.columns,
                "references": {
                    "schema": f.ref_table.schema,
                    "table": f.ref_table.name,
                    "columns": f.ref_columns,
                },
                "onUpdate": f.on_update,
                "onDelete": f.on_delete,
            })
        })
        .collect();
    let checks: Vec<Json> = meta
        .checks
        .iter()
        .map(|c| json!({ "name": c.name, "expression": c.expression }))
        .collect();
    let triggers: Vec<Json> = meta
        .triggers
        .iter()
        .map(|t| json!({ "name": t.name, "timing": t.timing, "events": t.events, "enabled": t.enabled }))
        .collect();
    let mut out = json!({
        "schema": meta.table.schema,
        "table": meta.table.name,
        "kind": kind_json(meta.kind),
        "columns": columns,
        "primaryKey": meta.primary_key,
        "indexes": indexes,
        "foreignKeys": foreign_keys,
    });
    if !checks.is_empty() {
        out["checks"] = json!(checks);
    }
    if !triggers.is_empty() {
        out["triggers"] = json!(triggers);
    }
    if let Some(c) = meta.comment.as_deref().filter(|c| !c.is_empty()) {
        out["comment"] = json!(c);
    }
    if let Some(n) = meta.row_estimate.filter(|n| *n >= 0) {
        out["rowEstimate"] = json!(n);
    }
    if let Some(ddl) = ddl {
        out["ddl"] = json!(ddl);
    }
    out
}

async fn describe_table(ctx: &Ctx, prefs: &McpPrefs, args: &Json) -> Result<ToolOutput, Fail> {
    let profile = resolve(
        &ctx.appdb().list_profiles().map_err(|e| e.to_string())?,
        prefs,
        req_str(args, "connection")?,
    )?;
    let table = req_str(args, "table")?.trim().to_string();
    let schema = opt_str(args, "schema")?.map(str::to_string);
    let lease = lease(ctx, prefs, &profile, opt_str(args, "database")?).await?;
    let s = lease.session.clone();
    let result = async {
        let (schema, name) = match (schema, table.split_once('.')) {
            (Some(schema), _) => (schema, table.clone()),
            (None, Some((schema, name))) if !schema.is_empty() && !name.is_empty() => {
                (schema.to_string(), name.to_string())
            }
            (None, _) => (default_schema(s.as_ref()).await?.0, table.clone()),
        };
        let tref = TableRef {
            schema: Some(schema.clone()),
            name: name.clone(),
        };
        let meta = s.table_meta(&tref).await?;
        if meta.columns.is_empty() {
            return Err(AppError::NotFound(format!(
                "no table or view \u{201c}{name}\u{201d} in \u{201c}{schema}\u{201d}; call list_tables to see them"
            )));
        }
        let ddl = match table_ddl(s.as_ref(), &meta).await {
            Ok(ddl) => Some(ddl),
            Err(e) => {
                log::warn!("mcp: no DDL for {name}: {e}");
                None
            }
        };
        Ok((meta, ddl))
    }
    .await;
    let (meta, ddl) = finish(ctx, lease, result).await?;
    let mut out = describe_json(&meta, ddl);
    out["connection"] = json!(profile.name);
    Ok(ToolOutput::ok(out))
}

/// One result set: its columns and rows.
type ResultRows = (Vec<ResultColumn>, Vec<Vec<Value>>);

/// Gathers the result sets and notices a run streams.
#[derive(Default)]
struct Collected {
    /// Per statement: its result sets.
    sets: HashMap<usize, Vec<ResultRows>>,
    notices: HashMap<usize, Vec<String>>,
}

impl Collected {
    fn push(&mut self, event: QueryEvent) {
        match event {
            QueryEvent::Columns { index, columns, .. } => {
                self.sets
                    .entry(index)
                    .or_default()
                    .push((columns, Vec::new()));
            }
            QueryEvent::Rows { index, set, rows } => {
                if let Some(s) = self
                    .sets
                    .get_mut(&index)
                    .and_then(|sets| sets.get_mut(set as usize))
                {
                    s.1.extend(rows);
                }
            }
            QueryEvent::Notice {
                index: Some(index),
                severity,
                message,
                ..
            } => self
                .notices
                .entry(index)
                .or_default()
                .push(format!("{severity}: {message}")),
            _ => {}
        }
    }
}

fn echo_sql(sql: &str) -> String {
    match sql.char_indices().nth(ECHO_SQL_MAX_CHARS) {
        Some((byte, _)) => format!("{}…", &sql[..byte]),
        None => sql.to_string(),
    }
}

fn error_json(e: &ErrorPayload) -> Json {
    let mut o = json!({ "message": e.message });
    if let Some(d) = &e.detail {
        o["detail"] = json!(d);
    }
    if let Some(h) = &e.hint {
        o["hint"] = json!(h);
    }
    if let Some(s) = &e.sqlstate {
        o["sqlstate"] = json!(s);
    }
    o
}

/// The database the MySQL session is on now (`DATABASE()`), asked the way the user's statements
/// run, on the same connection: `Some(None)` is no database, None that it could not be read.
async fn session_database(session: &dyn DbSession) -> Option<Option<String>> {
    let options = RunOptions {
        max_rows: 1,
        stop_on_error: true,
        explain: None,
        timeout_secs: 10,
    };
    let mut probe = Collected::default();
    let results = run_sql_inner(
        session,
        "SELECT DATABASE()",
        &options,
        &CancelSlot::default(),
        &mut |event| probe.push(event),
    )
    .await;
    let index = results.first().filter(|r| r.error.is_none())?.index;
    let (_, rows) = probe.sets.remove(&index)?.into_iter().next()?;
    match rows.first()?.first()? {
        Value::Null => Some(None),
        Value::Text(name) => Some(Some(name.clone())),
        _ => None,
    }
}

async fn run_query(
    ctx: &Ctx,
    prefs: &McpPrefs,
    args: &Json,
    cancel: &CallCancel,
) -> Result<ToolOutput, Fail> {
    let profile = resolve(
        &ctx.appdb().list_profiles().map_err(|e| e.to_string())?,
        prefs,
        req_str(args, "connection")?,
    )?;
    let sql = sql_arg(args)?;
    let limit = opt_u32(args, "limit")?
        .unwrap_or(DEFAULT_ROWS)
        .clamp(1, MAX_ROWS);
    let database = opt_str(args, "database")?;
    let dialect = dialect_of(profile.driver)?;
    let total = split_statements(sql, dialect).len();
    if total == 0 {
        return Err("There is no SQL statement to run.".into());
    }
    if let Err(refused) = gate::check_query(&profile, prefs, sql, dialect) {
        let mut row = history_row(
            &profile,
            database.map(str::to_string).or(profile.database.clone()),
            sql,
        );
        row.success = false;
        row.error = Some(refused.clone());
        record(ctx.appdb(), &[row]);
        return Err(refused);
    }
    let lease = lease(ctx, prefs, &profile, database).await?;
    let session = lease.session.clone();
    cancel.attach(session.clone());
    let current_db = lease
        .server
        .current_database
        .clone()
        .or(lease.profile.database.clone());
    let options = RunOptions {
        max_rows: limit,
        stop_on_error: true,
        explain: None,
        timeout_secs: prefs.query_timeout_secs,
    };
    let mut collected = Collected::default();
    let results = run_sql_inner(
        session.as_ref(),
        sql,
        &options,
        &cancel.slot,
        &mut |event| collected.push(event),
    )
    .await;

    // The gate refuses a MySQL `USE` it can read, but the server lexes the text in ways no
    // prediction covers (version-conditional comments, `NO_BACKSLASH_ESCAPES`…). So ask the
    // session where it is now, as the statements ran it: if it is not where it opened, the
    // connection is closed rather than pooled for the next call.
    let ended_on = if dialect == Dialect::Mysql {
        session_database(session.as_ref()).await
    } else {
        Some(lease.server.current_database.clone())
    };
    let moved_to = match &ended_on {
        Some(now) if *now != lease.server.current_database => Some(now.clone()),
        _ => None,
    };

    let rows: Vec<NewHistoryEntry> = results
        .iter()
        .map(|r| NewHistoryEntry {
            started_at: r.started_at.clone(),
            duration_ms: Some(r.duration_ms),
            row_count: (r.kind == StatementKind::Rows).then_some(r.row_count),
            affected: r.affected,
            success: r.error.is_none(),
            error: r.error.as_ref().map(|e| e.message.clone()),
            ..history_row(&profile, current_db.clone(), &r.sql)
        })
        .collect();
    record(ctx.appdb(), &rows);

    if ended_on.is_none()
        || moved_to.is_some()
        || results
            .iter()
            .any(|r| r.error.as_ref().is_some_and(|e| lost_connection(&e.code)))
    {
        ctx.pool.discard(lease).await;
    } else {
        drop(lease);
    }

    let mut budget = Budget::new(limit);
    let mut cut_any = false;
    let mut failed = false;
    let statements: Vec<Json> = results
        .iter()
        .map(|r| {
            let mut o = json!({ "statement": echo_sql(&r.sql) });
            let sets = collected.sets.remove(&r.index).unwrap_or_default();
            let mut sets = sets.into_iter();
            if let Some((columns, rows)) = sets.next() {
                let (json_rows, cut) = rows_json(&columns, &rows, &mut budget);
                let truncated = cut || r.truncated;
                cut_any |= truncated;
                o["columns"] = columns_json(&columns);
                o["rows"] = json!(json_rows);
                if truncated {
                    o["truncated"] = json!(true);
                }
                let extra: Vec<Json> = sets
                    .map(|(columns, rows)| {
                        let (json_rows, cut) = rows_json(&columns, &rows, &mut budget);
                        cut_any |= cut;
                        json!({ "columns": columns_json(&columns), "rows": json_rows, "truncated": cut })
                    })
                    .collect();
                if !extra.is_empty() {
                    o["moreResultSets"] = json!(extra);
                }
            }
            if let Some(n) = r.affected {
                o["affectedRows"] = json!(n);
            }
            if let Some(id) = &r.last_insert_id {
                o["lastInsertId"] = json!(id);
            }
            o["durationMs"] = json!(r.duration_ms);
            let mut notices = collected.notices.remove(&r.index).unwrap_or_default();
            notices.extend(r.notices.iter().cloned());
            if !notices.is_empty() {
                o["notices"] = json!(notices);
            }
            if let Some(e) = &r.error {
                failed = true;
                o["error"] = error_json(e);
            }
            o
        })
        .collect();
    let mut out = json!({
        "connection": profile.name,
        "database": current_db,
        "results": statements,
    });
    if let Some(now) = &moved_to {
        out["database"] = json!(now);
        out["databaseChanged"] = json!(format!(
            "This SQL switched the session from {} to {}. The connection was closed, so the next \
             call starts on the database it asks for again.",
            current_db.as_deref().unwrap_or("no database"),
            now.as_deref().unwrap_or("no database"),
        ));
    }
    if cut_any {
        out["note"] = json!(format!(
            "Not every row is shown (at most {limit} rows and {} MB per call). Narrow the query \
             (WHERE, LIMIT) to see the others.",
            MAX_BYTES / (1024 * 1024)
        ));
    }
    if results.len() < total {
        out["notRun"] = json!(format!(
            "{} later statement(s) were not run after the error.",
            total - results.len()
        ));
    }
    if cancel.is_cancelled() {
        failed = true;
    }
    Ok(ToolOutput {
        text: out.to_string(),
        is_error: failed,
    })
}

async fn explain_query(
    ctx: &Ctx,
    prefs: &McpPrefs,
    args: &Json,
    cancel: &CallCancel,
) -> Result<ToolOutput, Fail> {
    let profile = resolve(
        &ctx.appdb().list_profiles().map_err(|e| e.to_string())?,
        prefs,
        req_str(args, "connection")?,
    )?;
    let sql = sql_arg(args)?.to_string();
    let lease = lease(ctx, prefs, &profile, opt_str(args, "database")?).await?;
    cancel.attach(lease.session.clone());
    let current_db = lease
        .server
        .current_database
        .clone()
        .or(lease.profile.database.clone());
    let started_at = now_rfc3339();
    let result = explain_inner(
        lease.session.as_ref(),
        &sql,
        ExplainMode::Plan,
        &cancel.slot,
    )
    .await;
    let outcome = finish(ctx, lease, result).await;
    let mut row = history_row(&profile, current_db, &sql);
    row.started_at = started_at;
    match &outcome {
        Ok(r) => {
            row.duration_ms = Some(r.duration_ms);
            row.row_count = Some(r.rows.len() as u64);
        }
        Err(e) => {
            row.success = false;
            row.error = Some(e.clone());
        }
    }
    record(ctx.appdb(), &[row]);
    let r = outcome?;
    let plan = match &r.text {
        Some(text) => json!(text),
        None => {
            let mut budget = Budget::new(MAX_ROWS);
            json!(rows_json(&r.columns, &r.rows, &mut budget).0)
        }
    };
    Ok(ToolOutput::ok(json!({
        "connection": profile.name,
        "plan": plan,
        "durationMs": r.duration_ms,
    })))
}

/// `tools/call`.
pub async fn call(
    ctx: &Ctx,
    name: &str,
    args: &Json,
    cancel: &CallCancel,
) -> Result<ToolOutput, UnknownTool> {
    if !definitions().iter().any(|t| t["name"] == name) {
        return Err(UnknownTool(name.to_string()));
    }
    let prefs = match McpPrefs::load(ctx.appdb()) {
        Ok(p) => p,
        Err(e) => {
            return Ok(ToolOutput::failed(format!(
                "Could not read Tabler's settings: {e}"
            )));
        }
    };
    if !prefs.enabled {
        return Ok(ToolOutput::failed(DISABLED_MESSAGE));
    }
    if !args.is_object() && !args.is_null() {
        return Ok(ToolOutput::failed("The arguments must be an object."));
    }
    let outcome = match name {
        "list_connections" => ctx
            .appdb()
            .list_profiles()
            .map(|profiles| list_connections(&profiles, &prefs))
            .map_err(|e| e.to_string()),
        "list_databases" => list_databases(ctx, &prefs, args).await,
        "list_tables" => list_tables(ctx, &prefs, args).await,
        "describe_table" => describe_table(ctx, &prefs, args).await,
        "run_query" => run_query(ctx, &prefs, args, cancel).await,
        "explain_query" => explain_query(ctx, &prefs, args, cancel).await,
        _ => unreachable!("checked against the definitions"),
    };
    Ok(outcome.unwrap_or_else(ToolOutput::failed))
}
