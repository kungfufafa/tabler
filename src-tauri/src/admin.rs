//! Server administration round trips behind the Connection menu (Process List…, User
//! Management…) and the ⌘K database list: read the server's catalogs and run the statements
//! `sqlgen::admin` builds. Everything runs on the session's worker pool, never on the SQL
//! editor's session, so nothing here reaches the query history; passwords only ever appear in
//! the executed SQL, never in what is returned or logged.

use std::collections::BTreeMap;
use std::time::Instant;

use crate::drivers::{DbSession, TxStatement};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::admin::{
    self as admin_sql, AdminStatement, MYSQL_DB_PRIVILEGES, MYSQL_GLOBAL_PRIVILEGES,
    PG_DATABASE_PRIVILEGES, PG_ENCODINGS, PG_SCHEMA_PRIVILEGES, PG_TABLE_PRIVILEGES, PgPassword,
    SCRAM_SALT_LEN, UserSqlContext,
};
use crate::sqlgen::dialect::DialectRef;
use crate::sqlgen::quote::MysqlLiteralMode;
use crate::types::admin::{
    AdminDdl, AdminResult, CollationOption, DatabaseOptions, EncodingOption, KillOutcome,
    KillScope, ObjectPrivileges, ProcessRow, ResourceLimits, RoleAttributes, UserChange,
    UserDetail, UserRef, UserState, UserSummary,
};
use crate::types::caps::Feature;
use crate::types::value::Value;

// --- cells ----------------------------------------------------------------------------------

/// A cell as text, whatever the driver decoded it to (MySQL may hand out bytes).
fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::Bytes(b) => {
            use base64::Engine as _;
            let raw = base64::engine::general_purpose::STANDARD
                .decode(&b.base64)
                .ok()?;
            Some(String::from_utf8_lossy(&raw).into_owned())
        }
        other => other.as_text(),
    }
}

fn cell(row: &[Value], i: usize) -> Option<String> {
    row.get(i).and_then(text_of)
}

/// PG `true`, MySQL `Y` / `1`.
fn flag(row: &[Value], i: usize) -> bool {
    matches!(
        cell(row, i).as_deref().map(str::trim),
        Some("t" | "true" | "1" | "Y" | "y")
    )
}

fn int(row: &[Value], i: usize) -> Option<i64> {
    cell(row, i)?.trim().parse().ok()
}

fn uint(row: &[Value], i: usize) -> u32 {
    int(row, i).and_then(|n| u32::try_from(n).ok()).unwrap_or(0)
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty())
}

// --- databases and schemas ------------------------------------------------------------------

/// Encodings, collations and templates for the Create Database dialog.
pub async fn database_options(session: &dyn DbSession) -> Result<DatabaseOptions, AppError> {
    match session.core_dialect(Feature::CreateDatabase)? {
        Dialect::Postgres => {
            let collations = session.query(admin_sql::PG_COLLATIONS, &[]).await?;
            let templates = session.query(admin_sql::PG_TEMPLATES, &[]).await?;
            let template1 = session.query(admin_sql::PG_TEMPLATE1, &[]).await?;
            let defaults = template1.rows.first();
            Ok(DatabaseOptions {
                encodings: PG_ENCODINGS
                    .iter()
                    .map(|e| EncodingOption {
                        name: e.to_string(),
                        description: None,
                        default_collation: None,
                    })
                    .collect(),
                collations: collations
                    .rows
                    .iter()
                    .filter_map(|r| {
                        Some(CollationOption {
                            name: cell(r, 0)?,
                            encoding: cell(r, 1),
                        })
                    })
                    .collect(),
                templates: templates.rows.iter().filter_map(|r| cell(r, 0)).collect(),
                default_encoding: Some(
                    defaults
                        .and_then(|r| cell(r, 0))
                        .unwrap_or_else(|| "UTF8".into()),
                ),
                default_collation: defaults.and_then(|r| non_empty(cell(r, 1))),
            })
        }
        Dialect::Mysql => {
            let charsets = session.query(admin_sql::MYSQL_CHARSETS, &[]).await?;
            let collations = session.query(admin_sql::MYSQL_COLLATIONS, &[]).await?;
            let mut encodings: Vec<EncodingOption> = charsets
                .rows
                .iter()
                .filter_map(|r| {
                    Some(EncodingOption {
                        name: cell(r, 0)?,
                        description: non_empty(cell(r, 1)),
                        default_collation: non_empty(cell(r, 2)),
                    })
                })
                .collect();
            encodings.sort_by(|a, b| a.name.cmp(&b.name));
            let mut collations: Vec<CollationOption> = collations
                .rows
                .iter()
                .filter_map(|r| {
                    Some(CollationOption {
                        name: cell(r, 0)?,
                        encoding: non_empty(cell(r, 1)),
                    })
                })
                .collect();
            collations.sort_by(|a, b| a.name.cmp(&b.name));
            let default_encoding = encodings
                .iter()
                .find(|e| e.name == "utf8mb4")
                .or(encodings.first())
                .map(|e| e.name.clone());
            Ok(DatabaseOptions {
                encodings,
                collations,
                templates: Vec::new(),
                default_encoding,
                default_collation: None,
            })
        }
        Dialect::Sqlite => Err(AppError::Unsupported(
            "SQLite databases are files: create them in the connection form".into(),
        )),
    }
}

/// Runs `statements` one by one, stopping at the first failure (MySQL commits each statement
/// implicitly; PG `CREATE DATABASE` cannot run inside a transaction).
async fn run_sequential(
    session: &dyn DbSession,
    statements: &[AdminStatement],
    secrets: &[String],
) -> AdminResult {
    let start = Instant::now();
    let mut executed = Vec::new();
    for (i, st) in statements.iter().enumerate() {
        if let Err(e) = session.execute(&st.sql, &[]).await {
            return AdminResult {
                executed,
                failed_index: Some(i),
                error: Some(scrub(e.with_statement_index(i), secrets).to_payload()),
                duration_ms: start.elapsed().as_millis() as u64,
            };
        }
        executed.push(st.display.clone());
    }
    AdminResult {
        executed,
        failed_index: None,
        error: None,
        duration_ms: start.elapsed().as_millis() as u64,
    }
}

/// Runs `statements` in one transaction (PG roles and grants): all or nothing.
async fn run_transaction(
    session: &dyn DbSession,
    statements: &[AdminStatement],
    secrets: &[String],
) -> AdminResult {
    let start = Instant::now();
    let tx = statements
        .iter()
        .map(|s| TxStatement::new(s.sql.clone(), Vec::new()))
        .collect();
    match session.transaction(tx).await {
        Ok(_) => AdminResult {
            executed: statements.iter().map(|s| s.display.clone()).collect(),
            failed_index: None,
            error: None,
            duration_ms: start.elapsed().as_millis() as u64,
        },
        Err(e) => {
            let failed_index = match &e {
                AppError::Db {
                    statement_index, ..
                } => *statement_index,
                _ => None,
            };
            AdminResult {
                executed: Vec::new(),
                failed_index,
                error: Some(scrub(e, secrets).to_payload()),
                duration_ms: start.elapsed().as_millis() as u64,
            }
        }
    }
}

/// Masks the password should a server message quote it back, in any of the forms it was sent
/// in (`sqlgen::admin::secret_forms`: the cleartext, the escaped literal text, a SCRAM verifier).
/// Forms shorter than 3 characters are left alone (masking them would mangle the message).
fn scrub(e: AppError, secrets: &[String]) -> AppError {
    let forms: Vec<&str> = secrets
        .iter()
        .map(String::as_str)
        .filter(|s| s.chars().count() >= 3)
        .collect();
    if forms.is_empty() {
        return e;
    }
    let mask = |s: String| {
        forms
            .iter()
            .fold(s, |acc, secret| acc.replace(secret, "********"))
    };
    match e {
        AppError::Db {
            message,
            detail,
            hint,
            sqlstate,
            vendor_code,
            position,
            statement_index,
        } => AppError::Db {
            message: mask(message),
            detail: detail.map(mask),
            hint: hint.map(mask),
            sqlstate,
            vendor_code,
            position,
            statement_index,
        },
        other => other,
    }
}

/// The feature a database / schema change needs.
pub fn ddl_feature(op: &AdminDdl) -> Feature {
    match op {
        AdminDdl::CreateDatabase { .. } => Feature::CreateDatabase,
        AdminDdl::RenameDatabase { .. } => Feature::RenameDatabase,
        AdminDdl::DropDatabase { .. } => Feature::DropDatabase,
        AdminDdl::CreateSchema { .. } => Feature::CreateSchema,
        AdminDdl::DropSchema { .. } => Feature::DropSchema,
    }
}

/// The statement a database / schema change runs.
pub fn preview_ddl(dialect: Dialect, op: &AdminDdl) -> Result<Vec<String>, AppError> {
    Ok(vec![admin_sql::admin_ddl_sql(dialect, op)?])
}

/// Runs one database / schema change.
pub async fn apply_ddl(session: &dyn DbSession, op: &AdminDdl) -> Result<AdminResult, AppError> {
    let sql = admin_sql::admin_ddl_sql(session.core_dialect(ddl_feature(op))?, op)?;
    Ok(run_sequential(session, &[AdminStatement::plain(sql)], &[]).await)
}

// --- process list ---------------------------------------------------------------------------

/// `pg_stat_activity` rows (`sqlgen::admin::PG_PROCESS_LIST`) as process rows.
pub fn pg_process_rows(rows: &[Vec<Value>]) -> Vec<ProcessRow> {
    rows.iter()
        .filter_map(|r| {
            Some(ProcessRow {
                id: int(r, 0)?.to_string(),
                user: cell(r, 1),
                database: cell(r, 2),
                application: non_empty(cell(r, 3)),
                client: cell(r, 4),
                state: cell(r, 5),
                command: None,
                started_at: cell(r, 6),
                duration_ms: int(r, 7),
                wait_event: cell(r, 8),
                query: non_empty(cell(r, 9)),
                current: flag(r, 10),
                own: flag(r, 11) || flag(r, 10),
            })
        })
        .collect()
}

/// `127.0.0.1:52144` → `127.0.0.1`; `localhost` stays.
fn client_host(host: &str) -> &str {
    match host.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => host,
    }
}

/// `information_schema.PROCESSLIST` rows (`sqlgen::admin::MYSQL_PROCESS_LIST`) as process rows.
/// MySQL cannot tell which threads are Tabler's: the listing one is, and so are probably the
/// others of the same account from the same client host.
pub fn mysql_process_rows(rows: &[Vec<Value>]) -> Vec<ProcessRow> {
    let mut out: Vec<ProcessRow> = rows
        .iter()
        .filter_map(|r| {
            Some(ProcessRow {
                id: int(r, 0)?.to_string(),
                user: cell(r, 1),
                database: cell(r, 3),
                application: None,
                client: cell(r, 2),
                state: non_empty(cell(r, 6)),
                command: cell(r, 4),
                started_at: None,
                duration_ms: int(r, 5).map(|s| s.saturating_mul(1000)),
                wait_event: None,
                query: non_empty(cell(r, 7)),
                current: flag(r, 8),
                own: flag(r, 8),
            })
        })
        .collect();
    let me = out.iter().find(|p| p.current).map(|p| {
        (
            p.user.clone(),
            p.client.as_deref().map(client_host).map(String::from),
        )
    });
    if let Some((user, host)) = me {
        for p in &mut out {
            if p.user == user && p.client.as_deref().map(client_host) == host.as_deref() {
                p.own = true;
            }
        }
    }
    out
}

/// The server's sessions: the driver's own list where it has one (`DbSession::list_processes`),
/// otherwise the core one of the session's dialect.
pub async fn list_processes(session: &dyn DbSession) -> Result<Vec<ProcessRow>, AppError> {
    session.require(Feature::ProcessList)?;
    if let Some(rows) = session.list_processes().await {
        return rows;
    }
    let dialect = session.core_dialect(Feature::ProcessList)?;
    let rs = session
        .query(admin_sql::process_list_sql(dialect)?, &[])
        .await?;
    Ok(match dialect {
        Dialect::Mysql => mysql_process_rows(&rs.rows),
        _ => pg_process_rows(&rs.rows),
    })
}

/// A core process id (PG `pid`, MySQL thread `Id`) as `list_processes` gave it.
fn process_id(id: &str) -> Result<i64, AppError> {
    id.parse()
        .map_err(|_| AppError::InvalidInput(format!("invalid process id {id}")))
}

/// Kills each process in turn; one failure does not stop the others. A session without Kill
/// Process is refused before any. The driver kills its own where it can
/// (`DbSession::kill_processes`).
pub async fn kill(
    session: &dyn DbSession,
    ids: &[String],
    scope: KillScope,
) -> Result<Vec<KillOutcome>, AppError> {
    session.require(Feature::KillProcess)?;
    if let Some(outcomes) = session.kill_processes(ids, scope).await {
        return Ok(outcomes);
    }
    let dialect = session.core_dialect(Feature::KillProcess)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let sql = process_id(id).and_then(|n| admin_sql::kill_sql(dialect, n, scope));
        let result = match sql {
            Err(e) => Err(e),
            Ok(sql) if dialect == Dialect::Postgres => session.query(&sql, &[]).await.map(|rs| {
                // `pg_cancel_backend` / `pg_terminate_backend` say false for an unknown pid.
                rs.rows.first().is_some_and(|r| flag(r, 0))
            }),
            Ok(sql) => session.execute(&sql, &[]).await.map(|_| true),
        };
        out.push(match result {
            Ok(true) => KillOutcome {
                id: id.clone(),
                ok: true,
                message: None,
            },
            Ok(false) => KillOutcome {
                id: id.clone(),
                ok: false,
                message: Some("No such process: it may have ended already".into()),
            },
            Err(e) => KillOutcome {
                id: id.clone(),
                ok: false,
                message: Some(e.to_string()),
            },
        });
    }
    Ok(out)
}

// --- users ----------------------------------------------------------------------------------

pub async fn list_users(
    session: &dyn DbSession,
    include_nologin: bool,
) -> Result<Vec<UserSummary>, AppError> {
    match session.core_dialect(Feature::Users)? {
        Dialect::Postgres => {
            let rs = session.query(admin_sql::PG_LIST_ROLES, &[]).await?;
            Ok(rs
                .rows
                .iter()
                .filter_map(|r| {
                    Some(UserSummary {
                        name: cell(r, 0)?,
                        host: None,
                        can_login: flag(r, 1),
                        superuser: flag(r, 2),
                    })
                })
                .filter(|u| include_nologin || u.can_login)
                .collect())
        }
        Dialect::Mysql => {
            let rs = session.query(admin_sql::MYSQL_LIST_USERS, &[]).await?;
            Ok(rs
                .rows
                .iter()
                .filter_map(|r| {
                    Some(UserSummary {
                        name: cell(r, 0).unwrap_or_default(),
                        host: Some(cell(r, 1)?),
                        can_login: true,
                        superuser: flag(r, 2),
                    })
                })
                .collect())
        }
        Dialect::Sqlite => Err(AppError::Unsupported("SQLite has no users".into())),
    }
}

/// Groups (object, privilege) rows, keeping only privileges of `known`.
fn group_privileges(rows: &[Vec<Value>], known: &[&str]) -> Vec<ObjectPrivileges> {
    let mut by_object: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in rows {
        let (Some(object), Some(privilege)) = (cell(r, 0), cell(r, 1)) else {
            continue;
        };
        if !known.contains(&privilege.as_str()) {
            continue;
        }
        let list = by_object.entry(object).or_default();
        if !list.contains(&privilege) {
            list.push(privilege);
        }
    }
    by_object
        .into_iter()
        .map(|(object, mut privileges)| {
            privileges.sort_by_key(|p| known.iter().position(|k| k == p));
            ObjectPrivileges { object, privileges }
        })
        .collect()
}

/// (all tables, some tables) per schema from `PG_TABLE_GRANTS` rows.
pub fn pg_table_privileges(rows: &[Vec<Value>]) -> (Vec<ObjectPrivileges>, Vec<ObjectPrivileges>) {
    let mut all = Vec::new();
    let mut some = Vec::new();
    for r in rows {
        let (Some(schema), Some(privilege)) = (cell(r, 0), cell(r, 1)) else {
            continue;
        };
        let total = int(r, 2).unwrap_or(0);
        let granted = int(r, 3).unwrap_or(0);
        if granted <= 0 || total <= 0 {
            continue;
        }
        let row = [Value::Text(schema), Value::Text(privilege)].to_vec();
        if granted >= total {
            all.push(row);
        } else {
            some.push(row);
        }
    }
    (
        group_privileges(&all, PG_TABLE_PRIVILEGES),
        group_privileges(&some, PG_TABLE_PRIVILEGES),
    )
}

/// `PG_ROLE_ATTRIBUTES` row as attributes (`infinity` = never expires).
pub fn pg_role_attributes(row: &[Value]) -> RoleAttributes {
    RoleAttributes {
        superuser: flag(row, 0),
        create_db: flag(row, 1),
        create_role: flag(row, 2),
        login: flag(row, 3),
        replication: flag(row, 4),
        inherit: flag(row, 5),
        connection_limit: int(row, 6)
            .and_then(|n| i32::try_from(n).ok())
            .unwrap_or(-1),
        valid_until: non_empty(cell(row, 7)).filter(|v| v != "infinity"),
    }
}

/// `mysql.user` row (`mysql_user_detail_sql`) as global privileges and limits.
pub fn mysql_user_row(row: &[Value]) -> (Vec<String>, ResourceLimits) {
    let privileges = MYSQL_GLOBAL_PRIVILEGES
        .iter()
        .enumerate()
        .filter(|(i, _)| flag(row, *i))
        .map(|(_, (p, _))| p.to_string())
        .collect();
    let n = MYSQL_GLOBAL_PRIVILEGES.len();
    let limits = ResourceLimits {
        max_queries_per_hour: uint(row, n),
        max_updates_per_hour: uint(row, n + 1),
        max_connections_per_hour: uint(row, n + 2),
        max_user_connections: uint(row, n + 3),
    };
    (privileges, limits)
}

/// `mysql.db` rows (`mysql_db_grants_sql`) as per-database privileges. `Db` is a pattern where
/// `_` / `%` are wildcards: its escapes are removed (`app\_prod` → `app_prod`), so the editor
/// shows the database's real name; a wildcard grant shows as its pattern (`app%`).
pub fn mysql_db_rows(rows: &[Vec<Value>]) -> Vec<ObjectPrivileges> {
    rows.iter()
        .filter_map(|r| {
            let object = admin_sql::mysql_db_unescape(&cell(r, 0)?);
            let privileges: Vec<String> = MYSQL_DB_PRIVILEGES
                .iter()
                .enumerate()
                .filter(|(i, _)| flag(r, i + 1))
                .map(|(_, (p, _))| p.to_string())
                .collect();
            (!privileges.is_empty()).then_some(ObjectPrivileges { object, privileges })
        })
        .collect()
}

/// Loads an account for the User Management window.
pub async fn load_user(session: &dyn DbSession, user: &UserRef) -> Result<UserDetail, AppError> {
    let name = Value::Text(user.name.clone());
    match session.core_dialect(Feature::Users)? {
        Dialect::Postgres => {
            let params = [name];
            let attrs = session
                .query(admin_sql::PG_ROLE_ATTRIBUTES, &params)
                .await?;
            let row = attrs
                .rows
                .first()
                .ok_or_else(|| AppError::NotFound(format!("role {}", user.name)))?;
            let databases = session
                .query(admin_sql::PG_DATABASE_GRANTS, &params)
                .await?;
            let schemas = session.query(admin_sql::PG_SCHEMA_GRANTS, &params).await?;
            let tables = session.query(admin_sql::PG_TABLE_GRANTS, &params).await?;
            let names = session.query(admin_sql::PG_SCHEMAS, &[]).await?;
            let (table_privileges, partial_table_privileges) = pg_table_privileges(&tables.rows);
            Ok(UserDetail {
                state: UserState {
                    name: user.name.clone(),
                    host: None,
                    global_privileges: Vec::new(),
                    limits: None,
                    attributes: Some(pg_role_attributes(row)),
                    database_privileges: group_privileges(&databases.rows, PG_DATABASE_PRIVILEGES),
                    schema_privileges: group_privileges(&schemas.rows, PG_SCHEMA_PRIVILEGES),
                    table_privileges,
                    partial_table_privileges,
                },
                schemas: names.rows.iter().filter_map(|r| cell(r, 0)).collect(),
            })
        }
        Dialect::Mysql => {
            let host = user.host.clone().unwrap_or_else(|| "%".into());
            let params = [name, Value::Text(host.clone())];
            let rs = session
                .query(&admin_sql::mysql_user_detail_sql(), &params)
                .await?;
            let row = rs
                .rows
                .first()
                .ok_or_else(|| AppError::NotFound(format!("user '{}'@'{host}'", user.name)))?;
            let (global_privileges, limits) = mysql_user_row(row);
            let dbs = session
                .query(&admin_sql::mysql_db_grants_sql(), &params)
                .await?;
            Ok(UserDetail {
                state: UserState {
                    name: user.name.clone(),
                    host: Some(host),
                    global_privileges,
                    limits: Some(limits),
                    attributes: None,
                    database_privileges: mysql_db_rows(&dbs.rows),
                    schema_privileges: Vec::new(),
                    table_privileges: Vec::new(),
                    partial_table_privileges: Vec::new(),
                },
                schemas: Vec::new(),
            })
        }
        Dialect::Sqlite => Err(AppError::Unsupported("SQLite has no users".into())),
    }
}

/// How the session's MySQL string literals treat backslashes (its `sql_mode`), read once per
/// command, before the statements are generated. The worker pool's connections all run the same
/// startup SQL, so the one answering stands for the one that executes. Other dialects: the
/// default (unused).
pub async fn mysql_literal_mode(session: &dyn DbSession) -> Result<MysqlLiteralMode, AppError> {
    if !matches!(
        session.dialect().resolve(),
        DialectRef::Core(Dialect::Mysql)
    ) {
        return Ok(MysqlLiteralMode::default());
    }
    let rs = session.query(MysqlLiteralMode::QUERY, &[]).await?;
    Ok(rs
        .rows
        .first()
        .and_then(|r| cell(r, 0))
        .map(|m| MysqlLiteralMode::from_sql_mode(&m))
        .unwrap_or_default())
}

/// A fresh random SCRAM salt.
fn scram_salt() -> Result<[u8; SCRAM_SALT_LEN], AppError> {
    let mut salt = [0u8; SCRAM_SALT_LEN];
    aws_lc_rs::rand::fill(&mut salt)
        .map_err(|_| AppError::Internal("no secure random numbers".into()))?;
    Ok(salt)
}

/// What generating `change` needs to know about the session: MySQL's literal mode and the
/// stored patterns of the account's database-level grants; whether PostgreSQL stores passwords
/// as SCRAM-SHA-256 (`password_encryption`, PG 10+), in which case the verifier is computed here
/// like psql's `\password` does, so the cleartext never reaches the server.
pub async fn user_context(
    session: &dyn DbSession,
    change: &UserChange,
) -> Result<UserSqlContext, AppError> {
    let mut ctx = UserSqlContext::default();
    match session.core_dialect(Feature::Users)? {
        Dialect::Mysql => {
            ctx.mysql_mode = mysql_literal_mode(session).await?;
            if let Some(o) = &change.original {
                let params = [
                    Value::Text(o.name.clone()),
                    Value::Text(o.host.clone().unwrap_or_else(|| "%".into())),
                ];
                // Without read access to `mysql.db` every database is addressed escaped.
                if let Ok(rs) = session
                    .query(&admin_sql::mysql_db_grants_sql(), &params)
                    .await
                {
                    let stored: Vec<String> = rs.rows.iter().filter_map(|r| cell(r, 0)).collect();
                    ctx.mysql_db_patterns =
                        admin_sql::mysql_db_patterns(stored.iter().map(String::as_str));
                }
            }
        }
        Dialect::Postgres if change.password.is_some() => {
            let setting = session
                .query("SELECT current_setting('password_encryption')::text", &[])
                .await
                .ok()
                .and_then(|rs| rs.rows.first().and_then(|r| cell(r, 0)));
            if setting.is_some_and(|s| s.trim().eq_ignore_ascii_case("scram-sha-256")) {
                ctx.pg_password = PgPassword::Scram {
                    salt: scram_salt()?,
                };
            }
        }
        _ => {}
    }
    Ok(ctx)
}

/// The statements of a user change for a session in its default state (MySQL backslash
/// escapes, every database escaped, PG cleartext password), passwords masked.
pub fn preview_user(dialect: Dialect, change: &UserChange) -> Result<Vec<String>, AppError> {
    Ok(
        admin_sql::user_statements(dialect, change, &UserSqlContext::default())?
            .into_iter()
            .map(|s| s.display)
            .collect(),
    )
}

/// The statements a user change runs on `session` (see `user_context`), passwords masked.
pub async fn preview_user_on(
    session: &dyn DbSession,
    change: &UserChange,
) -> Result<Vec<String>, AppError> {
    let dialect = session.core_dialect(Feature::Users)?;
    let mut ctx = user_context(session, change).await?;
    // The password is masked in the preview: no need to derive a verifier for it.
    ctx.pg_password = PgPassword::Plain;
    Ok(admin_sql::user_statements(dialect, change, &ctx)?
        .into_iter()
        .map(|s| s.display)
        .collect())
}

/// Applies a user change: one transaction on PostgreSQL, statement by statement on MySQL (where
/// every account statement commits on its own).
pub async fn apply_user(
    session: &dyn DbSession,
    change: &UserChange,
) -> Result<AdminResult, AppError> {
    let dialect = session.core_dialect(Feature::Users)?;
    let ctx = user_context(session, change).await?;
    let statements = admin_sql::user_statements(dialect, change, &ctx)?;
    let secrets = admin_sql::secret_forms(change.password.as_deref(), &statements);
    if statements.is_empty() {
        return Ok(AdminResult {
            executed: Vec::new(),
            failed_index: None,
            error: None,
            duration_ms: 0,
        });
    }
    Ok(match dialect {
        Dialect::Postgres => run_transaction(session, &statements, &secrets).await,
        _ => run_sequential(session, &statements, &secrets).await,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn maps_pg_process_rows() {
        let rows = vec![
            vec![
                t("101"),
                t("alice"),
                t("shop"),
                t("psql"),
                t("10.0.0.5:51234"),
                t("active"),
                t("2026-10-12 10:00:00+00"),
                t("1500"),
                t("Lock: relation"),
                t("select pg_sleep(10)"),
                t("false"),
                t("false"),
            ],
            vec![
                t("102"),
                t("me"),
                t("shop"),
                t("tabler"),
                Value::Null,
                t("idle"),
                Value::Null,
                Value::Null,
                Value::Null,
                t(""),
                t("false"),
                t("true"),
            ],
            vec![t("not a pid")],
        ];
        let out = pg_process_rows(&rows);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "101");
        assert_eq!(out[0].client.as_deref(), Some("10.0.0.5:51234"));
        assert_eq!(out[0].duration_ms, Some(1500));
        assert_eq!(out[0].wait_event.as_deref(), Some("Lock: relation"));
        assert!(!out[0].own && !out[0].current);
        assert_eq!(out[1].application.as_deref(), Some("tabler"));
        assert!(out[1].own);
        assert_eq!(out[1].query, None);
    }

    #[test]
    fn maps_mysql_process_rows_and_guesses_tablers_threads() {
        let row = |id: i64, user: &str, host: &str, cmd: &str, time: i64, info: Value, me: i64| {
            vec![
                Value::Int(id),
                t(user),
                t(host),
                t("shop"),
                t(cmd),
                Value::Int(time),
                t(""),
                info,
                Value::Int(me),
            ]
        };
        let rows = vec![
            row(7, "root", "127.0.0.1:50001", "Query", 0, t("SELECT ..."), 1),
            row(8, "root", "127.0.0.1:50002", "Sleep", 12, Value::Null, 0),
            row(9, "app", "127.0.0.1:50003", "Sleep", 3, Value::Null, 0),
            row(10, "root", "10.1.1.1:40000", "Query", 2, t("UPDATE t"), 0),
            row(
                11,
                "event_scheduler",
                "localhost",
                "Daemon",
                99,
                Value::Null,
                0,
            ),
        ];
        let out = mysql_process_rows(&rows);
        assert_eq!(
            out.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["7", "8", "9", "10", "11"]
        );
        assert!(out[0].current && out[0].own);
        assert!(out[1].own, "same account from the same host");
        assert!(!out[2].own && !out[3].own && !out[4].own);
        assert_eq!(out[1].duration_ms, Some(12_000));
        assert_eq!(out[1].command.as_deref(), Some("Sleep"));
        assert_eq!(out[1].state, None);
        assert_eq!(out[3].query.as_deref(), Some("UPDATE t"));
        assert_eq!(client_host("localhost"), "localhost");
        assert_eq!(client_host("::1:5000"), "::1");
    }

    #[test]
    fn reads_role_attributes_and_table_grants() {
        let attrs = pg_role_attributes(&[
            t("false"),
            t("true"),
            t("false"),
            t("true"),
            t("false"),
            t("true"),
            t("5"),
            t("infinity"),
        ]);
        assert!(attrs.create_db && attrs.login && attrs.inherit && !attrs.superuser);
        assert_eq!(attrs.connection_limit, 5);
        assert_eq!(attrs.valid_until, None);
        let rows = vec![
            vec![t("public"), t("SELECT"), t("3"), t("3")],
            vec![t("public"), t("INSERT"), t("3"), t("1")],
            vec![t("public"), t("DELETE"), t("3"), t("0")],
            vec![t("sales"), t("SELECT"), t("2"), t("2")],
        ];
        let (all, some) = pg_table_privileges(&rows);
        assert_eq!(
            all,
            [
                ObjectPrivileges {
                    object: "public".into(),
                    privileges: vec!["SELECT".into()]
                },
                ObjectPrivileges {
                    object: "sales".into(),
                    privileges: vec!["SELECT".into()]
                }
            ]
        );
        assert_eq!(
            some,
            [ObjectPrivileges {
                object: "public".into(),
                privileges: vec!["INSERT".into()]
            }]
        );
        // Unknown privileges (PG 17 MAINTAIN…) are left out, the rest keep the canonical order.
        let grouped = group_privileges(
            &[
                vec![t("shop"), t("TEMPORARY")],
                vec![t("shop"), t("CONNECT")],
                vec![t("shop"), t("MAINTAIN")],
            ],
            PG_DATABASE_PRIVILEGES,
        );
        assert_eq!(grouped[0].privileges, ["CONNECT", "TEMPORARY"]);
    }

    #[test]
    fn reads_mysql_user_rows() {
        let n = MYSQL_GLOBAL_PRIVILEGES.len();
        let mut row: Vec<Value> = (0..n).map(|_| t("N")).collect();
        row[0] = t("Y"); // SELECT
        row[12] = t("Y"); // SHOW DATABASES
        row.extend([Value::Int(10), Value::Int(0), Value::Int(3), Value::Int(2)]);
        let (privileges, limits) = mysql_user_row(&row);
        assert_eq!(privileges, ["SELECT", "SHOW DATABASES"]);
        assert_eq!(limits.max_queries_per_hour, 10);
        assert_eq!(limits.max_connections_per_hour, 3);
        assert_eq!(limits.max_user_connections, 2);

        let mut db = vec![t("shop")];
        db.extend((0..MYSQL_DB_PRIVILEGES.len()).map(|i| t(if i < 2 { "Y" } else { "N" })));
        let none: Vec<Value> = std::iter::once(t("empty"))
            .chain((0..MYSQL_DB_PRIVILEGES.len()).map(|_| t("N")))
            .collect();
        let out = mysql_db_rows(&[db, none]);
        assert_eq!(
            out,
            [ObjectPrivileges {
                object: "shop".into(),
                privileges: vec!["SELECT".into(), "INSERT".into()]
            }]
        );
    }

    #[test]
    fn scrubs_passwords_from_server_messages() {
        let secrets = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let e = AppError::db("password \"hunter22\" is too weak");
        let scrubbed = scrub(e, &secrets(&["hunter22"]));
        assert_eq!(scrubbed.to_string(), "password \"********\" is too weak");
        // Too short to mask without mangling the message.
        assert_eq!(
            scrub(AppError::db("ab"), &secrets(&["ab"])).to_string(),
            "ab"
        );
        // A syntax error quotes the statement: the escaped literal text is masked too.
        let statements = [AdminStatement {
            sql: r"ALTER USER 'u'@'%' IDENTIFIED BY 'p''w\\d' X".into(),
            display: "ALTER USER 'u'@'%' IDENTIFIED BY '********' X".into(),
            secret: Some(r"'p''w\\d'".into()),
        }];
        let forms = admin_sql::secret_forms(Some(r"p'w\d"), &statements);
        let e = AppError::db(r"You have an error in your SQL syntax near 'p''w\\d' X' at line 1");
        assert_eq!(
            scrub(e, &forms).to_string(),
            "You have an error in your SQL syntax near '********' X' at line 1"
        );
    }

    #[test]
    fn unescapes_database_grant_patterns() {
        let mut row = vec![t(r"app\_prod")];
        row.extend((0..MYSQL_DB_PRIVILEGES.len()).map(|i| t(if i == 0 { "Y" } else { "N" })));
        let mut wild = vec![t("app%")];
        wild.extend((0..MYSQL_DB_PRIVILEGES.len()).map(|i| t(if i == 0 { "Y" } else { "N" })));
        let out = mysql_db_rows(&[row, wild]);
        assert_eq!(
            out.iter().map(|p| p.object.as_str()).collect::<Vec<_>>(),
            ["app_prod", "app%"]
        );
    }
}
