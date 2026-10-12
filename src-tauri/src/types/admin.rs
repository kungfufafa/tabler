//! Server administration (Connection menu, ⌘K database list): databases and schemas, the process
//! list ("Running Queries") and user management. SQL generation lives in `sqlgen::admin`, the
//! server round trips in `crate::admin`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::ErrorPayload;

/// One database / schema change of the admin dialogs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export, rename_all_fields = "camelCase")]
pub enum AdminDdl {
    /// `encoding`: PG encoding (`UTF8`) / MySQL character set (`utf8mb4`). `collation`: PG locale
    /// (`LC_COLLATE` and `LC_CTYPE`) / MySQL collation. `template`: PG only. `None` = server default.
    CreateDatabase {
        name: String,
        encoding: Option<String>,
        collation: Option<String>,
        template: Option<String>,
    },
    /// PostgreSQL only (MySQL cannot rename a database).
    RenameDatabase {
        name: String,
        new_name: String,
    },
    DropDatabase {
        name: String,
    },
    /// PostgreSQL only.
    CreateSchema {
        name: String,
        owner: Option<String>,
    },
    /// PostgreSQL only.
    DropSchema {
        name: String,
        cascade: bool,
    },
}

/// An encoding (PG) / character set (MySQL) the Create Database dialog offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EncodingOption {
    pub name: String,
    pub description: Option<String>,
    /// MySQL: the character set's default collation.
    pub default_collation: Option<String>,
}

/// A collation (MySQL) / libc locale (PG) the Create Database dialog offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CollationOption {
    pub name: String,
    /// Encoding / character set it belongs to; `None` = usable with any encoding (PG `C`).
    pub encoding: Option<String>,
}

/// What the Create Database dialog can choose from.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DatabaseOptions {
    pub encodings: Vec<EncodingOption>,
    pub collations: Vec<CollationOption>,
    /// PG template databases (`template0`, `template1`, …); empty for MySQL.
    pub templates: Vec<String>,
    /// Preselected encoding: `template1`'s (PG, normally `UTF8`), `utf8mb4` (MySQL).
    pub default_encoding: Option<String>,
    /// PG: `template1`'s collation; a database with another encoding or collation has to be
    /// copied from `template0`. MySQL: `None` (see `EncodingOption::default_collation`).
    pub default_collation: Option<String>,
}

/// Outcome of an admin change. A failed PostgreSQL user change runs in one transaction, so
/// nothing of it persists (`executed` is empty); other plans stop at the first failure and list
/// the statements that ran. Passwords are masked in `executed`.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AdminResult {
    pub executed: Vec<String>,
    pub failed_index: Option<usize>,
    pub error: Option<ErrorPayload>,
    pub duration_ms: u64,
}

// --- process list ---------------------------------------------------------------------------

/// One server session of the process list. PostgreSQL fills `application`, `client`, `state`,
/// `wait_event`; MySQL fills `client` (its `Host`), `command`, `state`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProcessRow {
    /// PG `pid` / MySQL `Id`.
    pub id: i64,
    pub user: Option<String>,
    pub database: Option<String>,
    pub application: Option<String>,
    pub client: Option<String>,
    /// PG `state` (active, idle, idle in transaction…) / MySQL `State`.
    pub state: Option<String>,
    /// MySQL `Command` (Query, Sleep…).
    pub command: Option<String>,
    /// PG: when the running query (or, for an idle session, the current state) started.
    pub started_at: Option<String>,
    /// Time in the current state: PG since `started_at`, MySQL `Time` (seconds) × 1000.
    pub duration_ms: Option<i64>,
    /// PG `wait_event_type: wait_event`.
    pub wait_event: Option<String>,
    /// PG `query` / MySQL `Info`.
    pub query: Option<String>,
    /// The session that listed the processes.
    pub current: bool,
    /// One of Tabler's own sessions: PG `application_name = 'tabler'`; MySQL a best guess (the
    /// listing session, or the same account from the same client host).
    pub own: bool,
}

/// Kill Query (cancel the running statement) or Kill Connection (end the session).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum KillScope {
    Query,
    Connection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KillOutcome {
    pub id: i64,
    pub ok: bool,
    /// Why it failed (permission denied, no such process…).
    pub message: Option<String>,
}

// --- users ----------------------------------------------------------------------------------

/// A MySQL account (`user@host`) or a PostgreSQL role (`host` is `None`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UserRef {
    pub name: String,
    pub host: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UserSummary {
    pub name: String,
    pub host: Option<String>,
    /// PG `rolcanlogin`; always true for MySQL accounts.
    pub can_login: bool,
    pub superuser: bool,
}

/// PostgreSQL role attributes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RoleAttributes {
    pub superuser: bool,
    pub create_db: bool,
    pub create_role: bool,
    pub login: bool,
    pub replication: bool,
    pub inherit: bool,
    /// -1 = no limit.
    pub connection_limit: i32,
    /// Timestamp text (`2027-01-01 00:00:00+00`); `None` = never expires.
    pub valid_until: Option<String>,
}

impl Default for RoleAttributes {
    /// What `CREATE ROLE` gives a role without options.
    fn default() -> Self {
        RoleAttributes {
            superuser: false,
            create_db: false,
            create_role: false,
            login: false,
            replication: false,
            inherit: true,
            connection_limit: -1,
            valid_until: None,
        }
    }
}

/// MySQL account resource limits; 0 = unlimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResourceLimits {
    pub max_queries_per_hour: u32,
    pub max_updates_per_hour: u32,
    pub max_connections_per_hour: u32,
    pub max_user_connections: u32,
}

/// Privileges granted on one object (a database, a schema).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ObjectPrivileges {
    pub object: String,
    pub privileges: Vec<String>,
}

/// Everything the User Management window edits for one account. Only the fields of the
/// connection's driver are used; privilege names are upper case (`SELECT`, `SHOW DATABASES`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UserState {
    pub name: String,
    /// MySQL host (`%`, `localhost`); `None` for PG.
    pub host: Option<String>,
    /// MySQL `ON *.*` privileges.
    pub global_privileges: Vec<String>,
    /// MySQL resource limits.
    pub limits: Option<ResourceLimits>,
    /// PG role attributes.
    pub attributes: Option<RoleAttributes>,
    /// MySQL `ON db.*` / PG `ON DATABASE` (CONNECT, CREATE, TEMPORARY), directly granted.
    pub database_privileges: Vec<ObjectPrivileges>,
    /// PG `ON SCHEMA` (USAGE, CREATE) in the connection's current database.
    pub schema_privileges: Vec<ObjectPrivileges>,
    /// PG privileges held on every table of a schema (`object` = schema), current database.
    pub table_privileges: Vec<ObjectPrivileges>,
    /// PG privileges held on some, not all, tables of a schema. Left alone unless the edit moves
    /// them to all (`table_privileges`) or none (dropped from here).
    pub partial_table_privileges: Vec<ObjectPrivileges>,
}

/// An account as loaded for editing, plus what its editor needs to list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UserDetail {
    pub state: UserState,
    /// PG: the schemas of the connection's current database.
    pub schemas: Vec<String>,
}

/// One edit of the User Management window: `original` → `edited`. `original: None` creates the
/// account, `edited: None` drops it. `password: Some` sets (or changes) the password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UserChange {
    pub original: Option<UserState>,
    pub edited: Option<UserState>,
    pub password: Option<String>,
}
