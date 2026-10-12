//! SQL of the admin surfaces: CREATE / ALTER / DROP DATABASE and SCHEMA, the process list and
//! killing sessions, users / roles and their privileges, plus the diff that turns an edited
//! account into CREATE / ALTER / RENAME / GRANT / REVOKE / DROP statements.
//!
//! Identifiers go through `Dialect::quote_ident`, MySQL account names and literals through the
//! literal quoting below (which follows the session's `NO_BACKSLASH_ESCAPES` mode, see
//! `UserSqlContext`); privilege names, character sets and collations are checked against fixed
//! lists or a strict pattern, so nothing user-supplied reaches the SQL unquoted. Every statement
//! carries a `display` form with passwords masked: that is what previews, the console log and
//! results show. PostgreSQL passwords are sent as SCRAM-SHA-256 verifiers computed here, like
//! psql's `\password`, so the cleartext never reaches the server's logs.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use base64::Engine as _;

use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::quote::MysqlLiteralMode;
use crate::types::admin::{
    AdminDdl, KillScope, ObjectPrivileges, ResourceLimits, RoleAttributes, UserChange, UserState,
};

/// Stands in for a password in `AdminStatement::display`.
pub const PASSWORD_MASK: &str = "'********'";

/// PBKDF2 rounds of a SCRAM-SHA-256 verifier (libpq's and the server's default).
pub const SCRAM_ITERATIONS: u32 = 4096;
/// Salt length of a SCRAM-SHA-256 verifier, in bytes (libpq's default).
pub const SCRAM_SALT_LEN: usize = 16;

/// Server encodings of PostgreSQL (the ones `CREATE DATABASE … ENCODING` accepts).
pub const PG_ENCODINGS: &[&str] = &[
    "UTF8",
    "SQL_ASCII",
    "EUC_CN",
    "EUC_JP",
    "EUC_JIS_2004",
    "EUC_KR",
    "EUC_TW",
    "ISO_8859_5",
    "ISO_8859_6",
    "ISO_8859_7",
    "ISO_8859_8",
    "KOI8R",
    "KOI8U",
    "LATIN1",
    "LATIN2",
    "LATIN3",
    "LATIN4",
    "LATIN5",
    "LATIN6",
    "LATIN7",
    "LATIN8",
    "LATIN9",
    "LATIN10",
    "MULE_INTERNAL",
    "WIN866",
    "WIN874",
    "WIN1250",
    "WIN1251",
    "WIN1252",
    "WIN1253",
    "WIN1254",
    "WIN1255",
    "WIN1256",
    "WIN1257",
    "WIN1258",
];

/// MySQL `ON *.*` privileges the Global Privileges tab offers, with their `mysql.user` column.
pub const MYSQL_GLOBAL_PRIVILEGES: &[(&str, &str)] = &[
    ("SELECT", "Select_priv"),
    ("INSERT", "Insert_priv"),
    ("UPDATE", "Update_priv"),
    ("DELETE", "Delete_priv"),
    ("CREATE", "Create_priv"),
    ("DROP", "Drop_priv"),
    ("RELOAD", "Reload_priv"),
    ("PROCESS", "Process_priv"),
    ("FILE", "File_priv"),
    ("REFERENCES", "References_priv"),
    ("INDEX", "Index_priv"),
    ("ALTER", "Alter_priv"),
    ("SHOW DATABASES", "Show_db_priv"),
    ("SUPER", "Super_priv"),
    ("CREATE TEMPORARY TABLES", "Create_tmp_table_priv"),
    ("LOCK TABLES", "Lock_tables_priv"),
    ("EXECUTE", "Execute_priv"),
    ("REPLICATION SLAVE", "Repl_slave_priv"),
    ("REPLICATION CLIENT", "Repl_client_priv"),
    ("CREATE VIEW", "Create_view_priv"),
    ("SHOW VIEW", "Show_view_priv"),
    ("CREATE ROUTINE", "Create_routine_priv"),
    ("ALTER ROUTINE", "Alter_routine_priv"),
    ("CREATE USER", "Create_user_priv"),
    ("EVENT", "Event_priv"),
    ("TRIGGER", "Trigger_priv"),
];

/// MySQL `ON db.*` privileges, with their `mysql.db` column.
pub const MYSQL_DB_PRIVILEGES: &[(&str, &str)] = &[
    ("SELECT", "Select_priv"),
    ("INSERT", "Insert_priv"),
    ("UPDATE", "Update_priv"),
    ("DELETE", "Delete_priv"),
    ("CREATE", "Create_priv"),
    ("DROP", "Drop_priv"),
    ("REFERENCES", "References_priv"),
    ("INDEX", "Index_priv"),
    ("ALTER", "Alter_priv"),
    ("CREATE TEMPORARY TABLES", "Create_tmp_table_priv"),
    ("LOCK TABLES", "Lock_tables_priv"),
    ("EXECUTE", "Execute_priv"),
    ("CREATE VIEW", "Create_view_priv"),
    ("SHOW VIEW", "Show_view_priv"),
    ("CREATE ROUTINE", "Create_routine_priv"),
    ("ALTER ROUTINE", "Alter_routine_priv"),
    ("EVENT", "Event_priv"),
    ("TRIGGER", "Trigger_priv"),
];

/// PostgreSQL privileges on a database, a schema, and on all tables of a schema.
pub const PG_DATABASE_PRIVILEGES: &[&str] = &["CONNECT", "CREATE", "TEMPORARY"];
pub const PG_SCHEMA_PRIVILEGES: &[&str] = &["USAGE", "CREATE"];
pub const PG_TABLE_PRIVILEGES: &[&str] = &["SELECT", "INSERT", "UPDATE", "DELETE"];

/// One statement of an admin plan: `sql` runs, `display` is shown (passwords masked).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminStatement {
    pub sql: String,
    pub display: String,
    /// The password literal `sql` contains, as written there (to mask it in server messages).
    pub secret: Option<String>,
}

impl AdminStatement {
    pub fn plain(sql: String) -> AdminStatement {
        AdminStatement {
            display: sql.clone(),
            sql,
            secret: None,
        }
    }

    /// `head` + the password literal + `tail`; the display masks the password.
    fn with_secret(head: &str, secret_literal: &str, tail: &str) -> AdminStatement {
        AdminStatement {
            sql: format!("{head}{secret_literal}{tail}"),
            display: format!("{head}{PASSWORD_MASK}{tail}"),
            secret: Some(secret_literal.to_string()),
        }
    }
}

/// How the PostgreSQL password of a role change is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PgPassword {
    /// The cleartext as a literal: the server hashes it per its `password_encryption` (servers
    /// before 10, or set to `md5`).
    #[default]
    Plain,
    /// A SCRAM-SHA-256 verifier computed here with this salt (`password_encryption` is
    /// `scram-sha-256`): the server stores it as is.
    Scram { salt: [u8; SCRAM_SALT_LEN] },
}

/// What the account statements need to know about the session they run on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserSqlContext {
    /// MySQL: how the session reads backslashes in the account names and password literals.
    pub mysql_mode: MysqlLiteralMode,
    /// MySQL: the `mysql.db` patterns of the account's existing database-level grants, keyed by
    /// the database name the editor shows, when the pattern is not that name escaped (a
    /// wildcard grant such as `app%`, or `app_prod` granted without escaping). GRANT / REVOKE on
    /// those databases reuse the stored pattern so they hit the same grant; any other database
    /// is addressed by its escaped name (`app\_prod`).
    pub mysql_db_patterns: BTreeMap<String, String>,
    /// PostgreSQL: how a new password is sent.
    pub pg_password: PgPassword,
}

/// The forms a password can take in a server message about `statements`: the cleartext and the
/// text of each literal it was written as (without the quotes), longest first.
pub fn secret_forms(password: Option<&str>, statements: &[AdminStatement]) -> Vec<String> {
    let mut forms: Vec<String> = password.map(str::to_string).into_iter().collect();
    for literal in statements.iter().filter_map(|s| s.secret.as_deref()) {
        let body = literal.strip_prefix('E').unwrap_or(literal);
        let body = body
            .strip_prefix('\'')
            .and_then(|b| b.strip_suffix('\''))
            .unwrap_or(body);
        forms.push(body.to_string());
    }
    forms.retain(|f| !f.is_empty());
    forms.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    forms.dedup();
    forms
}

/// A PostgreSQL SCRAM-SHA-256 password verifier,
/// `SCRAM-SHA-256$<iterations>:<salt>$<StoredKey>:<ServerKey>` (RFC 5802 / 7677), as libpq's
/// `PQencryptPasswordConn` builds it: the password goes through SASLprep when it can (else its
/// bytes are used as they are), then PBKDF2-HMAC-SHA-256 with `SCRAM_ITERATIONS` rounds.
pub fn scram_sha_256_verifier(password: &str, salt: &[u8; SCRAM_SALT_LEN]) -> String {
    use aws_lc_rs::{digest, hmac, pbkdf2};
    let prepared = stringprep::saslprep(password).unwrap_or(Cow::Borrowed(password));
    let rounds = NonZeroU32::new(SCRAM_ITERATIONS).unwrap_or(NonZeroU32::MIN);
    let mut salted = [0u8; 32];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        rounds,
        salt,
        prepared.as_bytes(),
        &mut salted,
    );
    let key = hmac::Key::new(hmac::HMAC_SHA256, &salted);
    let client_key = hmac::sign(&key, b"Client Key");
    let stored_key = digest::digest(&digest::SHA256, client_key.as_ref());
    let server_key = hmac::sign(&key, b"Server Key");
    let b64 = &base64::engine::general_purpose::STANDARD;
    format!(
        "SCRAM-SHA-256${SCRAM_ITERATIONS}:{}${}:{}",
        b64.encode(salt),
        b64.encode(stored_key.as_ref()),
        b64.encode(server_key.as_ref())
    )
}

fn invalid(message: impl Into<String>) -> AppError {
    AppError::InvalidInput(message.into())
}

fn unsupported(message: impl Into<String>) -> AppError {
    AppError::Unsupported(message.into())
}

/// A database / schema / role / account name: not blank, no NUL character.
fn check_name<'a>(name: &'a str, what: &str) -> Result<&'a str, AppError> {
    if name.trim().is_empty() {
        return Err(invalid(format!("{what} name is required")));
    }
    if name.contains('\0') {
        return Err(invalid(format!("{what} name contains a NUL character")));
    }
    Ok(name)
}

/// MySQL character sets and collations are bare words (`utf8mb4_0900_ai_ci`).
fn check_mysql_word<'a>(word: &'a str, what: &str) -> Result<&'a str, AppError> {
    if !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Ok(word)
    } else {
        Err(invalid(format!("invalid {what} {word:?}")))
    }
}

/// `Some(trimmed)` unless blank.
fn opt(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// A PostgreSQL string literal that means the same whatever `standard_conforming_strings` is:
/// text with a backslash uses the escape-string form `E'…'`.
pub fn pg_literal(s: &str) -> Result<String, AppError> {
    if s.contains('\0') {
        return Err(invalid("text contains a NUL character"));
    }
    let doubled = s.replace('\'', "''");
    Ok(if s.contains('\\') {
        format!("E'{}'", doubled.replace('\\', "\\\\"))
    } else {
        format!("'{doubled}'")
    })
}

/// A MySQL string literal for a session in `mode` (see `Dialect::quote_literal_with`).
pub fn mysql_literal(s: &str, mode: MysqlLiteralMode) -> String {
    Dialect::Mysql.quote_literal_with(s, mode)
}

/// The PASSWORD literal of a PostgreSQL role change: the cleartext, or its SCRAM verifier.
fn pg_password_literal(password: &str, how: PgPassword) -> Result<String, AppError> {
    if password.contains('\0') {
        return Err(invalid("the password contains a NUL character"));
    }
    match how {
        PgPassword::Plain => pg_literal(password),
        PgPassword::Scram { salt } => pg_literal(&scram_sha_256_verifier(password, &salt)),
    }
}

/// MySQL account name: `'user'@'host'` (host defaults to `%`).
pub fn mysql_account(name: &str, host: Option<&str>, mode: MysqlLiteralMode) -> String {
    let host = host.map(str::trim).filter(|h| !h.is_empty()).unwrap_or("%");
    format!(
        "{}@{}",
        mysql_literal(name, mode),
        mysql_literal(host, mode)
    )
}

/// The `mysql.db` pattern that matches database `name` only: `_` and `%` are wildcards there,
/// so they (and the escape character `\` itself) are escaped. Goes inside the backticks of a
/// database-level `GRANT … ON db.*`, where backslashes are plain characters in every mode.
pub fn mysql_db_pattern(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for c in name.chars() {
        if matches!(c, '\\' | '_' | '%') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The database name a `mysql.db` pattern (or a SHOW GRANTS database) stands for, its escapes
/// removed: `app\_prod` → `app_prod`. Wildcards stay as they are (`app%`).
pub fn mysql_db_unescape(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next().unwrap_or('\\')),
            c => out.push(c),
        }
    }
    out
}

/// `UserSqlContext::mysql_db_patterns` of the stored `mysql.db` patterns of an account: only the
/// ones that are not their name escaped, and only when no escaped one covers the same name.
pub fn mysql_db_patterns<'a>(
    stored: impl IntoIterator<Item = &'a str>,
) -> BTreeMap<String, String> {
    let stored: Vec<&str> = stored.into_iter().collect();
    let mut out = BTreeMap::new();
    for raw in &stored {
        let name = mysql_db_unescape(raw);
        let canonical = mysql_db_pattern(&name);
        if canonical != *raw && !stored.contains(&canonical.as_str()) {
            out.entry(name).or_insert_with(|| raw.to_string());
        }
    }
    out
}

fn require_pg(dialect: Dialect, what: &str) -> Result<(), AppError> {
    match dialect {
        Dialect::Postgres => Ok(()),
        Dialect::Mysql => Err(unsupported(format!("MySQL does not support {what}"))),
        Dialect::Sqlite => Err(unsupported(format!("SQLite does not support {what}"))),
    }
}

// --- databases and schemas ------------------------------------------------------------------

/// The statement of one database / schema change.
pub fn admin_ddl_sql(dialect: Dialect, op: &AdminDdl) -> Result<String, AppError> {
    if dialect == Dialect::Sqlite {
        return Err(unsupported(
            "SQLite databases are files: create or remove them in the connection form",
        ));
    }
    let q = |s: &str| dialect.quote_ident(s);
    match op {
        AdminDdl::CreateDatabase {
            name,
            encoding,
            collation,
            template,
        } => {
            let mut sql = format!("CREATE DATABASE {}", q(check_name(name, "Database")?));
            match dialect {
                Dialect::Postgres => {
                    if let Some(t) = opt(template) {
                        sql.push_str(&format!(" TEMPLATE {}", q(t)));
                    }
                    if let Some(e) = opt(encoding) {
                        if !PG_ENCODINGS.iter().any(|x| x.eq_ignore_ascii_case(e)) {
                            return Err(invalid(format!("unknown encoding {e:?}")));
                        }
                        sql.push_str(&format!(" ENCODING {}", pg_literal(e)?));
                    }
                    if let Some(c) = opt(collation) {
                        let lit = pg_literal(c)?;
                        sql.push_str(&format!(" LC_COLLATE {lit} LC_CTYPE {lit}"));
                    }
                }
                _ => {
                    if opt(template).is_some() {
                        return Err(unsupported("MySQL has no template databases"));
                    }
                    if let Some(e) = opt(encoding) {
                        sql.push_str(&format!(
                            " CHARACTER SET {}",
                            check_mysql_word(e, "character set")?
                        ));
                    }
                    if let Some(c) = opt(collation) {
                        sql.push_str(&format!(" COLLATE {}", check_mysql_word(c, "collation")?));
                    }
                }
            }
            Ok(sql)
        }
        AdminDdl::RenameDatabase { name, new_name } => {
            require_pg(dialect, "renaming a database")?;
            Ok(format!(
                "ALTER DATABASE {} RENAME TO {}",
                q(check_name(name, "Database")?),
                q(check_name(new_name, "New database")?)
            ))
        }
        AdminDdl::DropDatabase { name } => Ok(format!(
            "DROP DATABASE {}",
            q(check_name(name, "Database")?)
        )),
        AdminDdl::CreateSchema { name, owner } => {
            require_pg(dialect, "schemas apart from databases")?;
            let mut sql = format!("CREATE SCHEMA {}", q(check_name(name, "Schema")?));
            if let Some(o) = opt(owner) {
                sql.push_str(&format!(" AUTHORIZATION {}", q(o)));
            }
            Ok(sql)
        }
        AdminDdl::DropSchema { name, cascade } => {
            require_pg(dialect, "schemas apart from databases")?;
            let mut sql = format!("DROP SCHEMA {}", q(check_name(name, "Schema")?));
            if *cascade {
                sql.push_str(" CASCADE");
            }
            Ok(sql)
        }
    }
}

/// libc locales usable as a database collation, with their encoding (`NULL` = any encoding).
pub const PG_COLLATIONS: &str = "SELECT DISTINCT collcollate::text, \
     CASE WHEN collencoding = -1 THEN NULL ELSE pg_encoding_to_char(collencoding)::text END \
     FROM pg_collation WHERE collprovider = 'c' AND collcollate IS NOT NULL ORDER BY 1, 2";

/// Databases a new one can be copied from: the templates first.
pub const PG_TEMPLATES: &str =
    "SELECT datname::text FROM pg_database ORDER BY datistemplate DESC, datname";

/// Encoding and collation of `template1`, what a new database gets by default.
pub const PG_TEMPLATE1: &str = "SELECT pg_encoding_to_char(encoding)::text, datcollate::text \
     FROM pg_database WHERE datname = 'template1'";

/// Columns: Charset, Description, Default collation, Maxlen.
pub const MYSQL_CHARSETS: &str = "SHOW CHARACTER SET";
/// Columns: Collation, Charset, Id, Default, …
pub const MYSQL_COLLATIONS: &str = "SHOW COLLATION";

// --- process list ---------------------------------------------------------------------------

/// Client sessions of `pg_stat_activity`, every column as text. Columns: pid, user, database,
/// application, client, state, started, duration (ms), wait event, query, current, tabler.
pub const PG_PROCESS_LIST: &str = "SELECT pid::text, usename::text, datname::text, \
     application_name::text, \
     CASE WHEN client_addr IS NOT NULL THEN host(client_addr) || ':' || client_port::text \
          WHEN client_port = -1 THEN 'local socket' END, \
     state::text, \
     COALESCE(CASE WHEN state = 'active' THEN query_start END, state_change, backend_start)::text, \
     (EXTRACT(EPOCH FROM clock_timestamp() - COALESCE(CASE WHEN state = 'active' THEN query_start END, \
          state_change, backend_start)) * 1000)::bigint::text, \
     CASE WHEN wait_event IS NOT NULL THEN wait_event_type || ': ' || wait_event END, \
     query::text, \
     (pid = pg_backend_pid())::text, \
     (application_name = 'tabler')::text \
     FROM pg_stat_activity \
     WHERE backend_type = 'client backend' \
     ORDER BY pid";

/// MySQL threads. Columns: Id, User, Host, db, Command, Time, State, Info, current.
pub const MYSQL_PROCESS_LIST: &str = "SELECT ID, USER, HOST, DB, COMMAND, TIME, STATE, INFO, \
     ID = CONNECTION_ID() FROM information_schema.PROCESSLIST ORDER BY ID";

pub fn process_list_sql(dialect: Dialect) -> Result<&'static str, AppError> {
    match dialect {
        Dialect::Postgres => Ok(PG_PROCESS_LIST),
        Dialect::Mysql => Ok(MYSQL_PROCESS_LIST),
        Dialect::Sqlite => Err(unsupported(
            "SQLite has no server processes: the process list is not available",
        )),
    }
}

/// Kill Query / Kill Connection of one session.
pub fn kill_sql(dialect: Dialect, id: i64, scope: KillScope) -> Result<String, AppError> {
    if id <= 0 {
        return Err(invalid(format!("invalid process id {id}")));
    }
    match (dialect, scope) {
        (Dialect::Postgres, KillScope::Query) => Ok(format!("SELECT pg_cancel_backend({id})")),
        (Dialect::Postgres, KillScope::Connection) => {
            Ok(format!("SELECT pg_terminate_backend({id})"))
        }
        (Dialect::Mysql, KillScope::Query) => Ok(format!("KILL QUERY {id}")),
        (Dialect::Mysql, KillScope::Connection) => Ok(format!("KILL {id}")),
        (Dialect::Sqlite, _) => Err(unsupported("SQLite has no server processes")),
    }
}

// --- users: reading -------------------------------------------------------------------------

/// PG roles without the predefined `pg_*` ones. Columns: name, can login, superuser.
pub const PG_LIST_ROLES: &str = "SELECT rolname::text, rolcanlogin::text, rolsuper::text \
     FROM pg_roles WHERE rolname !~ '^pg_' ORDER BY rolname";

/// MySQL accounts. Columns: User, Host, Super_priv.
pub const MYSQL_LIST_USERS: &str =
    "SELECT User, Host, Super_priv FROM mysql.user ORDER BY User, Host";

/// Attributes of the role `$1`.
pub const PG_ROLE_ATTRIBUTES: &str = "SELECT rolsuper::text, rolcreatedb::text, \
     rolcreaterole::text, rolcanlogin::text, rolreplication::text, rolinherit::text, \
     rolconnlimit::text, rolvaliduntil::text FROM pg_roles WHERE rolname::text = $1::text";

/// Privileges granted directly to role `$1` on each database: (database, privilege).
pub const PG_DATABASE_GRANTS: &str = "SELECT d.datname::text, a.privilege_type::text \
     FROM pg_database d, aclexplode(COALESCE(d.datacl, acldefault('d', d.datdba))) a \
     WHERE a.grantee = (SELECT oid FROM pg_roles WHERE rolname::text = $1::text) \
     AND NOT d.datistemplate ORDER BY 1, 2";

/// Privileges granted directly to role `$1` on each schema of the current database.
pub const PG_SCHEMA_GRANTS: &str = "SELECT n.nspname::text, a.privilege_type::text \
     FROM pg_namespace n, aclexplode(COALESCE(n.nspacl, acldefault('n', n.nspowner))) a \
     WHERE a.grantee = (SELECT oid FROM pg_roles WHERE rolname::text = $1::text) \
     AND n.nspname !~ '^pg_' AND n.nspname <> 'information_schema' ORDER BY 1, 2";

/// Per schema and table privilege: how many relations there are and on how many of them role
/// `$1` holds the privilege directly. Columns: schema, privilege, total, granted.
pub const PG_TABLE_GRANTS: &str = "SELECT n.nspname::text, p.priv, count(*)::text, \
     (count(*) FILTER (WHERE EXISTS (SELECT 1 \
         FROM aclexplode(COALESCE(c.relacl, acldefault('r', c.relowner))) a \
         WHERE a.grantee = r.oid AND a.privilege_type = p.priv)))::text \
     FROM pg_namespace n \
     JOIN pg_class c ON c.relnamespace = n.oid AND c.relkind IN ('r', 'p', 'v', 'm', 'f') \
     CROSS JOIN (VALUES ('SELECT'), ('INSERT'), ('UPDATE'), ('DELETE')) AS p(priv) \
     CROSS JOIN (SELECT oid FROM pg_roles WHERE rolname::text = $1::text) r \
     WHERE n.nspname !~ '^pg_' AND n.nspname <> 'information_schema' \
     GROUP BY n.nspname, p.priv ORDER BY 1, 2";

/// Schemas of the current database (for the Database Privileges tab).
pub const PG_SCHEMAS: &str = "SELECT nspname::text FROM pg_namespace \
     WHERE nspname !~ '^pg_' AND nspname <> 'information_schema' ORDER BY 1";

/// `mysql.user` row of `?@?`: the global privilege columns, then the four resource limits.
pub fn mysql_user_detail_sql() -> String {
    let cols: Vec<&str> = MYSQL_GLOBAL_PRIVILEGES.iter().map(|(_, c)| *c).collect();
    format!(
        "SELECT {}, max_questions, max_updates, max_connections, max_user_connections \
         FROM mysql.user WHERE User = ? AND Host = ?",
        cols.join(", ")
    )
}

/// `mysql.db` rows of `?@?`: Db, then the database privilege columns.
pub fn mysql_db_grants_sql() -> String {
    let cols: Vec<&str> = MYSQL_DB_PRIVILEGES.iter().map(|(_, c)| *c).collect();
    format!(
        "SELECT Db, {} FROM mysql.db WHERE User = ? AND Host = ? ORDER BY Db",
        cols.join(", ")
    )
}

// --- users: the diff ------------------------------------------------------------------------

/// Privileges of `list` on `object` (empty when absent).
fn privileges_of<'a>(list: &'a [ObjectPrivileges], object: &str) -> BTreeSet<&'a str> {
    list.iter()
        .filter(|p| p.object == object)
        .flat_map(|p| p.privileges.iter().map(String::as_str))
        .collect()
}

fn objects<'a>(lists: &[&'a [ObjectPrivileges]]) -> BTreeSet<&'a str> {
    lists
        .iter()
        .flat_map(|l| l.iter().map(|p| p.object.as_str()))
        .collect()
}

/// Every privilege of `state` must be one the dialect's lists know (also keeps the statements
/// free of anything user-supplied that is not quoted).
fn check_privileges(dialect: Dialect, state: &UserState) -> Result<(), AppError> {
    let check = |list: &[String], allowed: &[&str], scope: &str| -> Result<(), AppError> {
        match list.iter().find(|p| !allowed.contains(&p.as_str())) {
            Some(p) => Err(invalid(format!("unknown {scope} privilege {p:?}"))),
            None => Ok(()),
        }
    };
    let names = |pairs: &[(&'static str, &'static str)]| -> Vec<&'static str> {
        pairs.iter().map(|(p, _)| *p).collect()
    };
    match dialect {
        Dialect::Mysql => {
            check(
                &state.global_privileges,
                &names(MYSQL_GLOBAL_PRIVILEGES),
                "global",
            )?;
            let db = names(MYSQL_DB_PRIVILEGES);
            for p in &state.database_privileges {
                check(&p.privileges, &db, "database")?;
            }
        }
        Dialect::Postgres => {
            for p in &state.database_privileges {
                check(&p.privileges, PG_DATABASE_PRIVILEGES, "database")?;
            }
            for p in &state.schema_privileges {
                check(&p.privileges, PG_SCHEMA_PRIVILEGES, "schema")?;
            }
            for p in state
                .table_privileges
                .iter()
                .chain(&state.partial_table_privileges)
            {
                check(&p.privileges, PG_TABLE_PRIVILEGES, "table")?;
            }
        }
        Dialect::Sqlite => {}
    }
    Ok(())
}

/// `list` in the canonical order of `order`.
fn ordered<'a>(set: &BTreeSet<&'a str>, order: &[&'a str]) -> Vec<&'a str> {
    order.iter().copied().filter(|p| set.contains(p)).collect()
}

/// GRANT / REVOKE of one object: `on` is the SQL after `ON` (`*.*`, `DATABASE "d"`…).
fn grant_revoke(
    out: &mut Vec<AdminStatement>,
    before: &BTreeSet<&str>,
    after: &BTreeSet<&str>,
    order: &[&str],
    on: &str,
    grantee: &str,
) {
    let added: BTreeSet<&str> = after.difference(before).copied().collect();
    let removed: BTreeSet<&str> = before.difference(after).copied().collect();
    if !added.is_empty() {
        out.push(AdminStatement::plain(format!(
            "GRANT {} ON {on} TO {grantee}",
            ordered(&added, order).join(", ")
        )));
    }
    if !removed.is_empty() {
        out.push(AdminStatement::plain(format!(
            "REVOKE {} ON {on} FROM {grantee}",
            ordered(&removed, order).join(", ")
        )));
    }
}

fn empty_state() -> UserState {
    UserState::default()
}

/// The statements that turn `change.original` into `change.edited` on a session described by
/// `ctx`.
pub fn user_statements(
    dialect: Dialect,
    change: &UserChange,
    ctx: &UserSqlContext,
) -> Result<Vec<AdminStatement>, AppError> {
    match dialect {
        Dialect::Mysql => mysql_user_statements(change, ctx),
        Dialect::Postgres => pg_role_statements(change, ctx.pg_password),
        Dialect::Sqlite => Err(unsupported("SQLite has no users")),
    }
}

fn mysql_user_statements(
    change: &UserChange,
    ctx: &UserSqlContext,
) -> Result<Vec<AdminStatement>, AppError> {
    let mode = ctx.mysql_mode;
    let mut out = Vec::new();
    let password = change.password.as_deref();
    let (original, edited) = match (&change.original, &change.edited) {
        (None, None) => return Ok(out),
        (Some(o), None) => {
            check_name(&o.name, "User")?;
            out.push(AdminStatement::plain(format!(
                "DROP USER {}",
                mysql_account(&o.name, o.host.as_deref(), mode)
            )));
            return Ok(out);
        }
        (o, Some(e)) => (o.as_ref(), e),
    };
    check_name(&edited.name, "User")?;
    check_privileges(Dialect::Mysql, edited)?;
    let account = mysql_account(&edited.name, edited.host.as_deref(), mode);
    let limits = edited.limits.unwrap_or_default();
    let secret = password
        .map(|p| {
            if p.contains('\0') {
                Err(invalid("the password contains a NUL character"))
            } else {
                Ok(mysql_literal(p, mode))
            }
        })
        .transpose()?;
    let base = match original {
        None => {
            let head = format!("CREATE USER {account}");
            let with = limit_clause(&ResourceLimits::default(), &limits, true);
            let tail = if with.is_empty() {
                String::new()
            } else {
                format!(" WITH {with}")
            };
            out.push(match &secret {
                Some(lit) => {
                    AdminStatement::with_secret(&format!("{head} IDENTIFIED BY "), lit, &tail)
                }
                None => AdminStatement::plain(format!("{head}{tail}")),
            });
            empty_state()
        }
        Some(o) => {
            check_name(&o.name, "User")?;
            let old = mysql_account(&o.name, o.host.as_deref(), mode);
            if old != account {
                out.push(AdminStatement::plain(format!(
                    "RENAME USER {old} TO {account}"
                )));
            }
            if let Some(lit) = &secret {
                out.push(AdminStatement::with_secret(
                    &format!("ALTER USER {account} IDENTIFIED BY "),
                    lit,
                    "",
                ));
            }
            let with = limit_clause(&o.limits.unwrap_or_default(), &limits, false);
            if !with.is_empty() {
                out.push(AdminStatement::plain(format!(
                    "ALTER USER {account} WITH {with}"
                )));
            }
            o.clone()
        }
    };
    let order: Vec<&str> = MYSQL_GLOBAL_PRIVILEGES.iter().map(|(p, _)| *p).collect();
    grant_revoke(
        &mut out,
        &base.global_privileges.iter().map(String::as_str).collect(),
        &edited
            .global_privileges
            .iter()
            .map(String::as_str)
            .collect(),
        &order,
        "*.*",
        &account,
    );
    let db_order: Vec<&str> = MYSQL_DB_PRIVILEGES.iter().map(|(p, _)| *p).collect();
    for db in objects(&[&base.database_privileges, &edited.database_privileges]) {
        check_name(db, "Database")?;
        // `_` and `%` are wildcards in a database-level grant: escape them, unless the account
        // already holds a grant stored under another pattern, which is the one to change.
        let pattern = ctx
            .mysql_db_patterns
            .get(db)
            .cloned()
            .unwrap_or_else(|| mysql_db_pattern(db));
        grant_revoke(
            &mut out,
            &privileges_of(&base.database_privileges, db),
            &privileges_of(&edited.database_privileges, db),
            &db_order,
            &format!("{}.*", Dialect::Mysql.quote_ident(&pattern)),
            &account,
        );
    }
    Ok(out)
}

/// `MAX_QUERIES_PER_HOUR n …` for the limits that differ from `before` (all non-zero ones when
/// `create`).
fn limit_clause(before: &ResourceLimits, after: &ResourceLimits, create: bool) -> String {
    let fields = [
        (
            "MAX_QUERIES_PER_HOUR",
            before.max_queries_per_hour,
            after.max_queries_per_hour,
        ),
        (
            "MAX_UPDATES_PER_HOUR",
            before.max_updates_per_hour,
            after.max_updates_per_hour,
        ),
        (
            "MAX_CONNECTIONS_PER_HOUR",
            before.max_connections_per_hour,
            after.max_connections_per_hour,
        ),
        (
            "MAX_USER_CONNECTIONS",
            before.max_user_connections,
            after.max_user_connections,
        ),
    ];
    fields
        .iter()
        .filter(|(_, b, a)| if create { *a != 0 } else { a != b })
        .map(|(name, _, a)| format!("{name} {a}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `SUPERUSER NOCREATEDB …` for the attributes that differ from `before`.
fn role_options(before: &RoleAttributes, after: &RoleAttributes) -> Result<Vec<String>, AppError> {
    let flag = |on: bool, word: &str| {
        if on {
            word.to_string()
        } else {
            format!("NO{word}")
        }
    };
    let mut opts = Vec::new();
    for (b, a, word) in [
        (before.superuser, after.superuser, "SUPERUSER"),
        (before.create_db, after.create_db, "CREATEDB"),
        (before.create_role, after.create_role, "CREATEROLE"),
        (before.login, after.login, "LOGIN"),
        (before.replication, after.replication, "REPLICATION"),
        (before.inherit, after.inherit, "INHERIT"),
    ] {
        if a != b {
            opts.push(flag(a, word));
        }
    }
    if after.connection_limit != before.connection_limit {
        if after.connection_limit < -1 {
            return Err(invalid("connection limit must be -1 (no limit) or more"));
        }
        opts.push(format!("CONNECTION LIMIT {}", after.connection_limit));
    }
    if opt(&after.valid_until) != opt(&before.valid_until) {
        let lit = match opt(&after.valid_until) {
            Some(v) => pg_literal(v)?,
            None => "'infinity'".to_string(),
        };
        opts.push(format!("VALID UNTIL {lit}"));
    }
    Ok(opts)
}

fn pg_role_statements(
    change: &UserChange,
    how: PgPassword,
) -> Result<Vec<AdminStatement>, AppError> {
    let q = |s: &str| Dialect::Postgres.quote_ident(s);
    let mut out = Vec::new();
    let (original, edited) = match (&change.original, &change.edited) {
        (None, None) => return Ok(out),
        (Some(o), None) => {
            // A role that still holds privileges cannot be dropped: revoke the ones the editor
            // knows about first (same transaction), then drop it.
            let role = q(check_name(&o.name, "Role")?);
            pg_privilege_statements(&mut out, o, &empty_state(), &role)?;
            out.push(AdminStatement::plain(format!("DROP ROLE {role}")));
            return Ok(out);
        }
        (o, Some(e)) => (o.as_ref(), e),
    };
    check_privileges(Dialect::Postgres, edited)?;
    let role = q(check_name(&edited.name, "Role")?);
    let attrs = edited.attributes.clone().unwrap_or_default();
    let secret = change
        .password
        .as_deref()
        .map(|p| pg_password_literal(p, how))
        .transpose()?;
    let base = match original {
        None => {
            let opts = role_options(&RoleAttributes::default(), &attrs)?;
            let head = if opts.is_empty() {
                format!("CREATE ROLE {role}")
            } else {
                format!("CREATE ROLE {role} WITH {}", opts.join(" "))
            };
            out.push(match &secret {
                Some(lit) => AdminStatement::with_secret(
                    &format!(
                        "{head}{}PASSWORD ",
                        if opts.is_empty() { " WITH " } else { " " }
                    ),
                    lit,
                    "",
                ),
                None => AdminStatement::plain(head),
            });
            empty_state()
        }
        Some(o) => {
            let old = q(check_name(&o.name, "Role")?);
            if o.name != edited.name {
                out.push(AdminStatement::plain(format!(
                    "ALTER ROLE {old} RENAME TO {role}"
                )));
            }
            let opts = role_options(&o.attributes.clone().unwrap_or_default(), &attrs)?;
            if !opts.is_empty() {
                out.push(AdminStatement::plain(format!(
                    "ALTER ROLE {role} WITH {}",
                    opts.join(" ")
                )));
            }
            if let Some(lit) = &secret {
                out.push(AdminStatement::with_secret(
                    &format!("ALTER ROLE {role} WITH PASSWORD "),
                    lit,
                    "",
                ));
            }
            o.clone()
        }
    };
    pg_privilege_statements(&mut out, &base, edited, &role)?;
    Ok(out)
}

/// GRANT / REVOKE of `role` (quoted) on databases, schemas and all tables of schemas, from
/// `base` to `edited`.
fn pg_privilege_statements(
    out: &mut Vec<AdminStatement>,
    base: &UserState,
    edited: &UserState,
    role: &str,
) -> Result<(), AppError> {
    let q = |s: &str| Dialect::Postgres.quote_ident(s);
    for db in objects(&[&base.database_privileges, &edited.database_privileges]) {
        check_name(db, "Database")?;
        grant_revoke(
            out,
            &privileges_of(&base.database_privileges, db),
            &privileges_of(&edited.database_privileges, db),
            PG_DATABASE_PRIVILEGES,
            &format!("DATABASE {}", q(db)),
            role,
        );
    }
    for schema in objects(&[&base.schema_privileges, &edited.schema_privileges]) {
        check_name(schema, "Schema")?;
        grant_revoke(
            out,
            &privileges_of(&base.schema_privileges, schema),
            &privileges_of(&edited.schema_privileges, schema),
            PG_SCHEMA_PRIVILEGES,
            &format!("SCHEMA {}", q(schema)),
            role,
        );
    }
    let table_lists = [
        base.table_privileges.as_slice(),
        base.partial_table_privileges.as_slice(),
        edited.table_privileges.as_slice(),
        edited.partial_table_privileges.as_slice(),
    ];
    for schema in objects(&table_lists) {
        check_name(schema, "Schema")?;
        let all_before = privileges_of(&base.table_privileges, schema);
        let some_before = privileges_of(&base.partial_table_privileges, schema);
        let all_after = privileges_of(&edited.table_privileges, schema);
        let some_after = privileges_of(&edited.partial_table_privileges, schema);
        // all → GRANT unless it was all already; none → REVOKE unless it was none already;
        // "some" stays as it is (only reachable when it was "some" before).
        let grant: BTreeSet<&str> = all_after.difference(&all_before).copied().collect();
        let revoke: BTreeSet<&str> = all_before
            .union(&some_before)
            .copied()
            .filter(|p| !all_after.contains(p) && !some_after.contains(p))
            .collect();
        let on = format!("ALL TABLES IN SCHEMA {}", q(schema));
        if !grant.is_empty() {
            out.push(AdminStatement::plain(format!(
                "GRANT {} ON {on} TO {role}",
                ordered(&grant, PG_TABLE_PRIVILEGES).join(", ")
            )));
        }
        if !revoke.is_empty() {
            out.push(AdminStatement::plain(format!(
                "REVOKE {} ON {on} FROM {role}",
                ordered(&revoke, PG_TABLE_PRIVILEGES).join(", ")
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PG: Dialect = Dialect::Postgres;
    const MY: Dialect = Dialect::Mysql;

    fn privs(object: &str, list: &[&str]) -> ObjectPrivileges {
        ObjectPrivileges {
            object: object.into(),
            privileges: list.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn sql(statements: &[AdminStatement]) -> Vec<&str> {
        statements.iter().map(|s| s.sql.as_str()).collect()
    }

    fn ctx() -> UserSqlContext {
        UserSqlContext::default()
    }

    const ESC: MysqlLiteralMode = MysqlLiteralMode::BackslashEscapes;
    const NO_ESC: MysqlLiteralMode = MysqlLiteralMode::NoBackslashEscapes;

    fn display(statements: &[AdminStatement]) -> Vec<&str> {
        statements.iter().map(|s| s.display.as_str()).collect()
    }

    fn create_db(name: &str, enc: Option<&str>, coll: Option<&str>, tpl: Option<&str>) -> AdminDdl {
        AdminDdl::CreateDatabase {
            name: name.into(),
            encoding: enc.map(Into::into),
            collation: coll.map(Into::into),
            template: tpl.map(Into::into),
        }
    }

    #[test]
    fn creates_databases_per_dialect() {
        assert_eq!(
            admin_ddl_sql(PG, &create_db("shop", None, None, None)).unwrap(),
            r#"CREATE DATABASE "shop""#
        );
        assert_eq!(
            admin_ddl_sql(
                PG,
                &create_db(
                    "my \"db\"",
                    Some("UTF8"),
                    Some("en_US.UTF-8"),
                    Some("template0")
                )
            )
            .unwrap(),
            r#"CREATE DATABASE "my ""db""" TEMPLATE "template0" ENCODING 'UTF8' LC_COLLATE 'en_US.UTF-8' LC_CTYPE 'en_US.UTF-8'"#
        );
        // Blank options mean the server default.
        assert_eq!(
            admin_ddl_sql(PG, &create_db("a", Some(" "), Some(""), Some(""))).unwrap(),
            r#"CREATE DATABASE "a""#
        );
        assert_eq!(
            admin_ddl_sql(
                MY,
                &create_db("sh`op", Some("utf8mb4"), Some("utf8mb4_0900_ai_ci"), None)
            )
            .unwrap(),
            "CREATE DATABASE `sh``op` CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci"
        );
    }

    #[test]
    fn rejects_bad_database_options() {
        assert!(admin_ddl_sql(PG, &create_db("", None, None, None)).is_err());
        assert!(admin_ddl_sql(PG, &create_db("a", Some("KLINGON"), None, None)).is_err());
        assert!(admin_ddl_sql(MY, &create_db("a", Some("utf8; DROP"), None, None)).is_err());
        assert!(admin_ddl_sql(MY, &create_db("a", None, Some("x y"), None)).is_err());
        assert!(admin_ddl_sql(MY, &create_db("a", None, None, Some("template0"))).is_err());
        assert!(admin_ddl_sql(Dialect::Sqlite, &create_db("a", None, None, None)).is_err());
        // A quote in a locale stays inside the literal.
        assert_eq!(
            admin_ddl_sql(PG, &create_db("a", None, Some("x'; DROP"), None)).unwrap(),
            r#"CREATE DATABASE "a" LC_COLLATE 'x''; DROP' LC_CTYPE 'x''; DROP'"#
        );
    }

    #[test]
    fn renames_drops_and_schemas() {
        let rename = AdminDdl::RenameDatabase {
            name: "old".into(),
            new_name: "new".into(),
        };
        assert_eq!(
            admin_ddl_sql(PG, &rename).unwrap(),
            r#"ALTER DATABASE "old" RENAME TO "new""#
        );
        let err = admin_ddl_sql(MY, &rename).unwrap_err();
        assert!(err.to_string().contains("MySQL"), "{err}");
        let drop = AdminDdl::DropDatabase { name: "x".into() };
        assert_eq!(admin_ddl_sql(PG, &drop).unwrap(), r#"DROP DATABASE "x""#);
        assert_eq!(admin_ddl_sql(MY, &drop).unwrap(), "DROP DATABASE `x`");
        assert_eq!(
            admin_ddl_sql(
                PG,
                &AdminDdl::CreateSchema {
                    name: "sales".into(),
                    owner: Some("bob".into())
                }
            )
            .unwrap(),
            r#"CREATE SCHEMA "sales" AUTHORIZATION "bob""#
        );
        assert_eq!(
            admin_ddl_sql(
                PG,
                &AdminDdl::CreateSchema {
                    name: "sales".into(),
                    owner: None
                }
            )
            .unwrap(),
            r#"CREATE SCHEMA "sales""#
        );
        assert_eq!(
            admin_ddl_sql(
                PG,
                &AdminDdl::DropSchema {
                    name: "sales".into(),
                    cascade: true
                }
            )
            .unwrap(),
            r#"DROP SCHEMA "sales" CASCADE"#
        );
        assert!(
            admin_ddl_sql(
                MY,
                &AdminDdl::DropSchema {
                    name: "s".into(),
                    cascade: false
                }
            )
            .is_err()
        );
    }

    #[test]
    fn kills_per_dialect() {
        assert_eq!(
            kill_sql(PG, 42, KillScope::Query).unwrap(),
            "SELECT pg_cancel_backend(42)"
        );
        assert_eq!(
            kill_sql(PG, 42, KillScope::Connection).unwrap(),
            "SELECT pg_terminate_backend(42)"
        );
        assert_eq!(kill_sql(MY, 7, KillScope::Query).unwrap(), "KILL QUERY 7");
        assert_eq!(kill_sql(MY, 7, KillScope::Connection).unwrap(), "KILL 7");
        assert!(kill_sql(MY, 0, KillScope::Query).is_err());
        assert!(kill_sql(Dialect::Sqlite, 1, KillScope::Query).is_err());
        assert!(process_list_sql(Dialect::Sqlite).is_err());
        assert!(process_list_sql(PG).unwrap().contains("pg_stat_activity"));
        assert!(process_list_sql(MY).unwrap().contains("PROCESSLIST"));
    }

    #[test]
    fn escapes_password_literals() {
        assert_eq!(pg_literal("it's").unwrap(), "'it''s'");
        // Backslashes: the escape-string form means the same with either
        // standard_conforming_strings setting.
        assert_eq!(pg_literal(r"a\b'c").unwrap(), r"E'a\\b''c'");
        assert!(pg_literal("a\0b").is_err());
        assert_eq!(mysql_literal(r"it's a\b", ESC), r"'it''s a\\b'");
        // NO_BACKSLASH_ESCAPES: a backslash is a plain character, so it is not doubled (it
        // would be stored twice) and a quote is never escaped with one.
        assert_eq!(mysql_literal(r"it's a\b", NO_ESC), r"'it''s a\b'");
        assert_eq!(
            mysql_account("bo'b", Some("10.0.%"), ESC),
            r"'bo''b'@'10.0.%'"
        );
        assert_eq!(mysql_account(r"x\", Some("%"), NO_ESC), r"'x\'@'%'");
        assert_eq!(mysql_account(r"x\", Some("%"), ESC), r"'x\\'@'%'");
        assert_eq!(mysql_account("bob", None, ESC), "'bob'@'%'");
        assert_eq!(mysql_account("bob", Some(" "), ESC), "'bob'@'%'");
    }

    #[test]
    fn mysql_passwords_follow_the_session_mode() {
        let change = UserChange {
            original: Some(my_user("app", "%")),
            edited: Some(my_user("app", "%")),
            password: Some(r"a\b'c".into()),
        };
        let no_esc = UserSqlContext {
            mysql_mode: NO_ESC,
            ..UserSqlContext::default()
        };
        let out = user_statements(MY, &change, &no_esc).unwrap();
        assert_eq!(sql(&out), [r"ALTER USER 'app'@'%' IDENTIFIED BY 'a\b''c'"]);
        assert_eq!(out[0].secret.as_deref(), Some(r"'a\b''c'"));
        let out = user_statements(MY, &change, &ctx()).unwrap();
        assert_eq!(sql(&out), [r"ALTER USER 'app'@'%' IDENTIFIED BY 'a\\b''c'"]);
        let nul = UserChange {
            password: Some("a\0b".into()),
            ..change
        };
        assert!(user_statements(MY, &nul, &ctx()).is_err());
    }

    #[test]
    fn escapes_wildcards_of_database_level_grants() {
        assert_eq!(mysql_db_pattern("app_prod"), r"app\_prod");
        assert_eq!(mysql_db_pattern(r"50%\off"), r"50\%\\off");
        assert_eq!(mysql_db_unescape(r"app\_prod"), "app_prod");
        assert_eq!(mysql_db_unescape(r"50\%\\off"), r"50%\off");
        assert_eq!(mysql_db_unescape("app%"), "app%");
        assert_eq!(mysql_db_unescape("trailing\\"), "trailing\\");
        // Only stored patterns that are not a name escaped are remembered, and an escaped grant
        // of the same name wins.
        assert_eq!(
            mysql_db_patterns([r"app\_prod", "app%", "shop", "old_db", r"x\_y", "x_y"]),
            BTreeMap::from([
                ("app%".to_string(), "app%".to_string()),
                ("old_db".to_string(), "old_db".to_string()),
            ])
        );

        let mut original = my_user("app", "%");
        original.database_privileges = vec![privs("old_db", &["SELECT"])];
        let mut edited = original.clone();
        edited.database_privileges = vec![
            privs("old_db", &["SELECT", "INSERT"]),
            privs("app_prod", &["SELECT"]),
            privs("t`x_1", &["SELECT"]),
        ];
        let context = UserSqlContext {
            mysql_db_patterns: mysql_db_patterns(["old_db"]),
            ..UserSqlContext::default()
        };
        let change = UserChange {
            original: Some(original),
            edited: Some(edited),
            password: None,
        };
        assert_eq!(
            sql(&user_statements(MY, &change, &context).unwrap()),
            [
                r"GRANT SELECT ON `app\_prod`.* TO 'app'@'%'",
                // The stored (unescaped) grant is the one extended.
                "GRANT INSERT ON `old_db`.* TO 'app'@'%'",
                r"GRANT SELECT ON `t``x\_1`.* TO 'app'@'%'",
            ]
        );
    }

    #[test]
    fn builds_scram_verifiers_like_the_server() {
        // Verifiers the local PostgreSQL 17 stored for these passwords
        // (`SET password_encryption = 'scram-sha-256'; CREATE ROLE … PASSWORD …`).
        let b64 = &base64::engine::general_purpose::STANDARD;
        let salt_of = |v: &str| -> [u8; SCRAM_SALT_LEN] {
            let salt = v.split(['$', ':']).nth(2).unwrap();
            b64.decode(salt).unwrap().try_into().unwrap()
        };
        let stored = "SCRAM-SHA-256$4096:iz7XmBEj0dRurtsUkLYaiw==$n1SFsNK3BUMkSPieMffNRiTJajREYnpFIVkeaB8jQIs=:vDKavGm3OqERIVqZYOg/4I+kLCWb8ywD99vALh0uFJA=";
        assert_eq!(
            scram_sha_256_verifier(r"pa'ss\word", &salt_of(stored)),
            stored
        );
        // SASLprep: full-width letters normalize to "pass".
        let stored = "SCRAM-SHA-256$4096:9voLBk+jXHKgExMFQOLE1Q==$eFibbgcAf7nvF3BORqXSTjaEWj66pvLlVRn+qCt4VeE=:9AYrNpv9inp2S8LuExCcjdCnP+5RRfD12aa4AnQy+jI=";
        assert_eq!(scram_sha_256_verifier("ｐａｓｓ", &salt_of(stored)), stored);
        assert_eq!(scram_sha_256_verifier("pass", &salt_of(stored)), stored);
    }

    #[test]
    fn sends_pg_passwords_as_scram_verifiers() {
        let salt = [7u8; SCRAM_SALT_LEN];
        let scram = UserSqlContext {
            pg_password: PgPassword::Scram { salt },
            ..UserSqlContext::default()
        };
        let change = UserChange {
            original: None,
            edited: Some(pg_role("r", RoleAttributes::default())),
            password: Some(r"it's\secret".into()),
        };
        let out = user_statements(PG, &change, &scram).unwrap();
        let verifier = scram_sha_256_verifier(r"it's\secret", &salt);
        assert_eq!(
            sql(&out),
            [format!(r#"CREATE ROLE "r" WITH PASSWORD '{verifier}'"#)]
        );
        assert!(!out[0].sql.contains("secret"));
        assert_eq!(
            display(&out),
            [r#"CREATE ROLE "r" WITH PASSWORD '********'"#]
        );
        let forms = secret_forms(change.password.as_deref(), &out);
        assert_eq!(forms, [verifier, r"it's\secret".to_string()]);
        let nul = UserChange {
            password: Some("a\0".into()),
            ..change
        };
        assert!(user_statements(PG, &nul, &scram).is_err());
    }

    #[test]
    fn lists_every_spelling_of_a_secret() {
        let statements = [
            AdminStatement::plain("GRANT …".into()),
            AdminStatement::with_secret("ALTER USER x IDENTIFIED BY ", r"'p''w\\d'", ""),
            AdminStatement::with_secret("ALTER ROLE x PASSWORD ", r"E'p''w\\d'", ""),
        ];
        assert_eq!(
            secret_forms(Some(r"p'w\d"), &statements),
            [r"p''w\\d", r"p'w\d"]
        );
        assert!(secret_forms(None, &[]).is_empty());
    }

    fn my_user(name: &str, host: &str) -> UserState {
        UserState {
            name: name.into(),
            host: Some(host.into()),
            limits: Some(ResourceLimits::default()),
            ..UserState::default()
        }
    }

    #[test]
    fn creates_a_mysql_user_with_grants_and_masks_the_password() {
        let mut edited = my_user("app", "%");
        edited.global_privileges = vec!["PROCESS".into(), "SELECT".into()];
        edited.database_privileges = vec![privs("shop", &["INSERT", "SELECT"])];
        edited.limits = Some(ResourceLimits {
            max_queries_per_hour: 100,
            max_user_connections: 5,
            ..ResourceLimits::default()
        });
        let change = UserChange {
            original: None,
            edited: Some(edited),
            password: Some(r"p'w\d".into()),
        };
        let out = user_statements(MY, &change, &ctx()).unwrap();
        assert_eq!(
            sql(&out),
            [
                r"CREATE USER 'app'@'%' IDENTIFIED BY 'p''w\\d' WITH MAX_QUERIES_PER_HOUR 100 MAX_USER_CONNECTIONS 5",
                "GRANT SELECT, PROCESS ON *.* TO 'app'@'%'",
                "GRANT SELECT, INSERT ON `shop`.* TO 'app'@'%'",
            ]
        );
        assert_eq!(
            display(&out)[0],
            "CREATE USER 'app'@'%' IDENTIFIED BY '********' WITH MAX_QUERIES_PER_HOUR 100 MAX_USER_CONNECTIONS 5"
        );
        assert!(display(&out).iter().all(|s| !s.contains("p''w")));
    }

    #[test]
    fn diffs_a_mysql_user() {
        let mut original = my_user("app", "%");
        original.global_privileges = vec!["SELECT".into(), "SUPER".into()];
        original.database_privileges = vec![
            privs("shop", &["SELECT", "INSERT"]),
            privs("old", &["SELECT"]),
        ];
        original.limits = Some(ResourceLimits {
            max_queries_per_hour: 100,
            ..ResourceLimits::default()
        });
        let mut edited = original.clone();
        edited.name = "app2".into();
        edited.host = Some("localhost".into());
        edited.global_privileges = vec!["SELECT".into(), "RELOAD".into()];
        edited.database_privileges = vec![
            privs("shop", &["SELECT", "UPDATE"]),
            privs("new", &["SHOW VIEW"]),
        ];
        edited.limits = Some(ResourceLimits {
            max_queries_per_hour: 0,
            max_updates_per_hour: 10,
            ..ResourceLimits::default()
        });
        let change = UserChange {
            original: Some(original.clone()),
            edited: Some(edited),
            password: Some("s3cret".into()),
        };
        let out = user_statements(MY, &change, &ctx()).unwrap();
        assert_eq!(
            sql(&out),
            [
                "RENAME USER 'app'@'%' TO 'app2'@'localhost'",
                "ALTER USER 'app2'@'localhost' IDENTIFIED BY 's3cret'",
                "ALTER USER 'app2'@'localhost' WITH MAX_QUERIES_PER_HOUR 0 MAX_UPDATES_PER_HOUR 10",
                "GRANT RELOAD ON *.* TO 'app2'@'localhost'",
                "REVOKE SUPER ON *.* FROM 'app2'@'localhost'",
                "GRANT SHOW VIEW ON `new`.* TO 'app2'@'localhost'",
                "REVOKE SELECT ON `old`.* FROM 'app2'@'localhost'",
                "GRANT UPDATE ON `shop`.* TO 'app2'@'localhost'",
                "REVOKE INSERT ON `shop`.* FROM 'app2'@'localhost'",
            ]
        );
        assert_eq!(
            display(&out)[1],
            "ALTER USER 'app2'@'localhost' IDENTIFIED BY '********'"
        );
        // Nothing changed: nothing to run.
        let same = UserChange {
            original: Some(original.clone()),
            edited: Some(original.clone()),
            password: None,
        };
        assert!(user_statements(MY, &same, &ctx()).unwrap().is_empty());
        // Drop.
        let drop = UserChange {
            original: Some(original),
            edited: None,
            password: None,
        };
        assert_eq!(
            sql(&user_statements(MY, &drop, &ctx()).unwrap()),
            ["DROP USER 'app'@'%'"]
        );
    }

    #[test]
    fn rejects_unknown_privileges() {
        let mut edited = my_user("app", "%");
        edited.global_privileges = vec!["SELECT; DROP".into()];
        let change = UserChange {
            original: None,
            edited: Some(edited),
            password: None,
        };
        assert!(user_statements(MY, &change, &ctx()).is_err());
        let mut role = UserState {
            name: "r".into(),
            ..UserState::default()
        };
        role.schema_privileges = vec![privs("public", &["SELECT"])];
        let change = UserChange {
            original: None,
            edited: Some(role),
            password: None,
        };
        assert!(user_statements(PG, &change, &ctx()).is_err());
        assert!(user_statements(Dialect::Sqlite, &change, &ctx()).is_err());
    }

    fn pg_role(name: &str, attributes: RoleAttributes) -> UserState {
        UserState {
            name: name.into(),
            attributes: Some(attributes),
            ..UserState::default()
        }
    }

    #[test]
    fn creates_a_pg_role() {
        let attrs = RoleAttributes {
            login: true,
            create_db: true,
            connection_limit: 10,
            valid_until: Some("2030-01-01".into()),
            ..RoleAttributes::default()
        };
        let mut edited = pg_role("app user", attrs);
        edited.database_privileges = vec![privs("shop", &["TEMPORARY", "CONNECT"])];
        edited.schema_privileges = vec![privs("public", &["USAGE"])];
        edited.table_privileges = vec![privs("public", &["SELECT", "INSERT"])];
        let change = UserChange {
            original: None,
            edited: Some(edited),
            password: Some("pa'ss".into()),
        };
        let out = user_statements(PG, &change, &ctx()).unwrap();
        assert_eq!(
            sql(&out),
            [
                r#"CREATE ROLE "app user" WITH CREATEDB LOGIN CONNECTION LIMIT 10 VALID UNTIL '2030-01-01' PASSWORD 'pa''ss'"#,
                r#"GRANT CONNECT, TEMPORARY ON DATABASE "shop" TO "app user""#,
                r#"GRANT USAGE ON SCHEMA "public" TO "app user""#,
                r#"GRANT SELECT, INSERT ON ALL TABLES IN SCHEMA "public" TO "app user""#,
            ]
        );
        assert_eq!(
            display(&out)[0],
            r#"CREATE ROLE "app user" WITH CREATEDB LOGIN CONNECTION LIMIT 10 VALID UNTIL '2030-01-01' PASSWORD '********'"#
        );
        // Defaults only, with and without a password.
        let bare = UserChange {
            original: None,
            edited: Some(pg_role("r", RoleAttributes::default())),
            password: None,
        };
        assert_eq!(
            sql(&user_statements(PG, &bare, &ctx()).unwrap()),
            [r#"CREATE ROLE "r""#]
        );
        let with_pw = UserChange {
            password: Some(r"a\b".into()),
            ..bare
        };
        let out = user_statements(PG, &with_pw, &ctx()).unwrap();
        assert_eq!(sql(&out), [r#"CREATE ROLE "r" WITH PASSWORD E'a\\b'"#]);
        assert_eq!(
            display(&out),
            [r#"CREATE ROLE "r" WITH PASSWORD '********'"#]
        );
    }

    #[test]
    fn diffs_a_pg_role() {
        let mut original = pg_role(
            "app",
            RoleAttributes {
                login: true,
                valid_until: Some("2030-01-01 00:00:00+00".into()),
                ..RoleAttributes::default()
            },
        );
        original.database_privileges = vec![privs("shop", &["CONNECT", "CREATE"])];
        original.schema_privileges = vec![privs("public", &["USAGE", "CREATE"])];
        original.table_privileges = vec![privs("public", &["SELECT"])];
        original.partial_table_privileges = vec![
            privs("public", &["INSERT", "UPDATE"]),
            privs("sales", &["SELECT"]),
        ];
        let mut edited = original.clone();
        edited.name = "app2".into();
        edited.attributes = Some(RoleAttributes {
            login: false,
            superuser: true,
            inherit: false,
            connection_limit: 3,
            valid_until: None,
            ..RoleAttributes::default()
        });
        edited.database_privileges = vec![privs("shop", &["CONNECT"]), privs("hr", &["CONNECT"])];
        edited.schema_privileges = vec![privs("public", &["USAGE"])];
        // public: SELECT → none, INSERT stays partial, UPDATE partial → all, DELETE none → all.
        edited.table_privileges = vec![privs("public", &["UPDATE", "DELETE"])];
        // sales: SELECT partial → none.
        edited.partial_table_privileges = vec![privs("public", &["INSERT"])];
        let change = UserChange {
            original: Some(original),
            edited: Some(edited),
            password: Some("x".into()),
        };
        let out = user_statements(PG, &change, &ctx()).unwrap();
        assert_eq!(
            sql(&out),
            [
                r#"ALTER ROLE "app" RENAME TO "app2""#,
                r#"ALTER ROLE "app2" WITH SUPERUSER NOLOGIN NOINHERIT CONNECTION LIMIT 3 VALID UNTIL 'infinity'"#,
                r#"ALTER ROLE "app2" WITH PASSWORD 'x'"#,
                r#"GRANT CONNECT ON DATABASE "hr" TO "app2""#,
                r#"REVOKE CREATE ON DATABASE "shop" FROM "app2""#,
                r#"REVOKE CREATE ON SCHEMA "public" FROM "app2""#,
                r#"GRANT UPDATE, DELETE ON ALL TABLES IN SCHEMA "public" TO "app2""#,
                r#"REVOKE SELECT ON ALL TABLES IN SCHEMA "public" FROM "app2""#,
                r#"REVOKE SELECT ON ALL TABLES IN SCHEMA "sales" FROM "app2""#,
            ]
        );
        assert_eq!(
            display(&out)[2],
            r#"ALTER ROLE "app2" WITH PASSWORD '********'"#
        );
        let drop = UserChange {
            original: Some(pg_role("old", RoleAttributes::default())),
            edited: None,
            password: None,
        };
        assert_eq!(
            sql(&user_statements(PG, &drop, &ctx()).unwrap()),
            [r#"DROP ROLE "old""#]
        );
        // Privileges it still holds are revoked first, or DROP ROLE would refuse.
        let mut holder = pg_role("old", RoleAttributes::default());
        holder.database_privileges = vec![privs("shop", &["CONNECT"])];
        holder.schema_privileges = vec![privs("public", &["USAGE"])];
        holder.partial_table_privileges = vec![privs("public", &["SELECT"])];
        let drop = UserChange {
            original: Some(holder),
            edited: None,
            password: None,
        };
        assert_eq!(
            sql(&user_statements(PG, &drop, &ctx()).unwrap()),
            [
                r#"REVOKE CONNECT ON DATABASE "shop" FROM "old""#,
                r#"REVOKE USAGE ON SCHEMA "public" FROM "old""#,
                r#"REVOKE SELECT ON ALL TABLES IN SCHEMA "public" FROM "old""#,
                r#"DROP ROLE "old""#,
            ]
        );
    }

    #[test]
    fn checks_role_attributes() {
        let change = UserChange {
            original: None,
            edited: Some(pg_role(
                "r",
                RoleAttributes {
                    connection_limit: -5,
                    ..RoleAttributes::default()
                },
            )),
            password: None,
        };
        assert!(user_statements(PG, &change, &ctx()).is_err());
        let quoted = UserChange {
            original: None,
            edited: Some(pg_role(
                "r",
                RoleAttributes {
                    valid_until: Some("2030'; DROP".into()),
                    ..RoleAttributes::default()
                },
            )),
            password: None,
        };
        assert_eq!(
            sql(&user_statements(PG, &quoted, &ctx()).unwrap()),
            [r#"CREATE ROLE "r" WITH VALID UNTIL '2030''; DROP'"#]
        );
    }

    #[test]
    fn detail_queries_list_every_privilege_column() {
        let user = mysql_user_detail_sql();
        assert!(user.contains("Show_db_priv") && user.contains("max_user_connections"));
        assert_eq!(user.matches("_priv").count(), MYSQL_GLOBAL_PRIVILEGES.len());
        let db = mysql_db_grants_sql();
        assert_eq!(db.matches("_priv").count(), MYSQL_DB_PRIVILEGES.len());
    }
}
