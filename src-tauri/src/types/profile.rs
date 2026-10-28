use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::sqlgen::SqlDialect;
use crate::types::caps::SessionCaps;
use crate::types::driver::{DriverDescriptor, KindDescriptor, KindId, PgFlavor};

/// The engine a connection is for. The ids are what profiles store and IPC carries: the first
/// three serialize exactly as before the other drivers existed. Engine facts (ports, labels,
/// fields) live in the descriptor table of `types::driver`, not in matches on this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Driver {
    #[serde(rename = "postgres")]
    Postgres,
    #[serde(rename = "mysql")]
    Mysql,
    #[serde(rename = "sqlite")]
    Sqlite,
    #[serde(rename = "sqlserver")]
    Sqlserver,
    #[serde(rename = "libsql")]
    Libsql,
    #[serde(rename = "d1")]
    D1,
    #[serde(rename = "duckdb")]
    Duckdb,
    #[serde(rename = "clickhouse")]
    Clickhouse,
    #[serde(rename = "snowflake")]
    Snowflake,
    #[serde(rename = "bigquery")]
    Bigquery,
    #[serde(rename = "vertica")]
    Vertica,
    #[serde(rename = "cassandra")]
    Cassandra,
    #[serde(rename = "redis")]
    Redis,
    #[serde(rename = "mongodb")]
    Mongodb,
    #[serde(rename = "elasticsearch")]
    Elasticsearch,
    #[serde(rename = "dynamodb")]
    Dynamodb,
}

impl Driver {
    pub const ALL: [Driver; 16] = [
        Driver::Postgres,
        Driver::Mysql,
        Driver::Sqlite,
        Driver::Sqlserver,
        Driver::Libsql,
        Driver::D1,
        Driver::Duckdb,
        Driver::Clickhouse,
        Driver::Snowflake,
        Driver::Bigquery,
        Driver::Vertica,
        Driver::Cassandra,
        Driver::Redis,
        Driver::Mongodb,
        Driver::Elasticsearch,
        Driver::Dynamodb,
    ];

    /// PostgreSQL, MySQL / MariaDB or SQLite: the drivers of every Tabler build. A connections
    /// file holding any other one is written with a newer layout version (`profiles_io`).
    pub const fn is_core(self) -> bool {
        matches!(self, Driver::Postgres | Driver::Mysql | Driver::Sqlite)
    }

    pub fn descriptor(self) -> &'static DriverDescriptor {
        crate::types::driver::descriptor(self)
    }

    /// The engine's name ("PostgreSQL", "SQL Server"…).
    pub fn label(self) -> &'static str {
        self.descriptor().label
    }

    // No `default_port` here: a driver's port depends on the profile's flavor and TLS
    // (CockroachDB is not on 5432), so callers use `ConnectionProfile::default_port`.
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum SslMode {
    Disable,
    #[default]
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum SshAuth {
    #[default]
    Password,
    Key,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: SshAuth,
    pub key_path: Option<String>,
    pub local_port: Option<u16>,
    pub keepalive_secs: Option<u32>,
    /// Where the SSH password of `auth == password` comes from; None (profiles saved before the
    /// setting) = Keychain.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub password_mode: Option<SshPasswordMode>,
}

impl SshConfig {
    pub fn password_mode(&self) -> SshPasswordMode {
        self.password_mode.unwrap_or_default()
    }
}

/// How the SSH password (password auth) is stored, like the database password's menu:
/// - `keychain`: stored in the macOS Keychain (the default).
/// - `ask`: asked on every connect through the SSH prompt dialog; never stored.
/// - `none`: no password is sent (keyboard-interactive / 2FA servers answer through prompts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SshPasswordMode {
    #[default]
    Keychain,
    Ask,
    None,
}

/// TablePlus-style environment tag of a connection; the workspace's status bar shows it. Five
/// built-in tags plus custom ones (b494) the user names; the colours of custom tags live in the
/// renderer's `connectionTags` setting. Crosses IPC (and connection files) as its name: the
/// built-ins as `local` / `testing` / `development` / `staging` / `production` (older profiles'
/// values), a custom tag as the name the user gave it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, TS)]
#[ts(export, type = "string")]
pub enum ConnectionTag {
    Local,
    Testing,
    Development,
    Staging,
    Production,
    Custom(String),
}

/// Longest custom tag name.
pub const TAG_NAME_MAX: usize = 40;

/// The connect timeout (seconds) of a profile that sets none: 60 s, as in TablePlus. The form and
/// Settings ▸ Connections accept 1 to 300.
pub const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 60;

impl ConnectionTag {
    pub const BUILT_IN: [ConnectionTag; 5] = [
        ConnectionTag::Local,
        ConnectionTag::Testing,
        ConnectionTag::Development,
        ConnectionTag::Staging,
        ConnectionTag::Production,
    ];

    pub fn as_str(&self) -> &str {
        match self {
            ConnectionTag::Local => "local",
            ConnectionTag::Testing => "testing",
            ConnectionTag::Development => "development",
            ConnectionTag::Staging => "staging",
            ConnectionTag::Production => "production",
            ConnectionTag::Custom(name) => name,
        }
    }

    /// The tag a stored name stands for: a built-in name (any case) is that built-in, anything
    /// else a custom tag of that (trimmed) name.
    pub fn from_name(name: &str) -> ConnectionTag {
        let name = name.trim();
        ConnectionTag::BUILT_IN
            .into_iter()
            .find(|t| t.as_str().eq_ignore_ascii_case(name))
            .unwrap_or_else(|| ConnectionTag::Custom(name.to_string()))
    }
}

impl Serialize for ConnectionTag {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ConnectionTag {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(ConnectionTag::from_name(&name))
    }
}

/// MySQL "Ver." (driver version): which server family the form is for. It picks the SSL mode
/// names (MariaDB / SingleStore / old MySQL: PREFERRED, ENFORCE, VERIFY-SERVER-CERT). One driver
/// serves all; `Old` also accepts the pre-4.1 `mysql_old_password` login (`secure_auth` off) and
/// negotiates TLS 1.0 / 1.1 where the platform's TLS library allows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum MysqlVersion {
    #[default]
    Default,
    Mariadb,
    /// "Old MySQL (5.x)".
    Old,
    /// "SingleStore DB" (MySQL wire protocol).
    Singlestore,
}

/// PostgreSQL "SSL negotiation": the `SSLRequest` handshake (`postgres`, every server) or TLS
/// straight away (`direct`, PostgreSQL 17+, libpq's `sslnegotiation=direct`; needs SSL mode
/// require or stricter).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SslNegotiation {
    #[default]
    Postgres,
    Direct,
}

/// TablePlus safe-mode level of a connection: what the app asks before it sends statements to the
/// server (editor runs, commits of staged changes, imports, restores, kills).
/// - `silent`: never asks.
/// - `alert` / `alertNonSelect`: a confirmation alert (the second skips SELECT / EXPLAIN / SHOW).
/// - `safe` / `safeNonSelect`: asks for the connection's password (the second skips reads).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SafeModeLevel {
    #[default]
    Silent,
    Alert,
    AlertNonSelect,
    Safe,
    SafeNonSelect,
}

/// How a connection gets its database password (TablePlus's storage menu next to Password).
/// - `keychain`: stored in the macOS Keychain (the default).
/// - `ask`: asked on every connect; kept in memory for that app session's reconnects only.
/// - `none`: the server needs no password; never asked, nothing stored.
/// - `command`: "Command Line (IAM authentication)": the output of `options.password_command`,
///   run through the login shell on every connect (e.g. `aws rds generate-db-auth-token …`);
///   the output is never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PasswordMode {
    #[default]
    Keychain,
    Ask,
    None,
    Command,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ProfileOptions {
    /// Environment tag (Local, Testing, …); None for profiles saved before tags existed.
    pub tag: Option<ConnectionTag>,
    /// Safe-mode level; None for profiles saved before safe mode existed (= silent).
    pub safe_mode: Option<SafeModeLevel>,
    pub timezone: Option<String>,
    /// Advanced ▸ Connect timeout in seconds; None (or 0): [`DEFAULT_CONNECT_TIMEOUT_SECS`].
    pub connect_timeout_secs: Option<u32>,
    pub read_only: bool,
    /// Bootstrap commands: SQL run right after login on every session connection.
    pub startup_sql: Option<String>,
    pub default_row_limit: Option<u32>,
    pub sqlite_foreign_keys: bool,
    /// Where the DB password comes from; None (profiles saved before the setting) = Keychain.
    #[ts(optional = nullable)]
    pub password_mode: Option<PasswordMode>,
    /// TLS client authentication: private key (PEM) and its certificate (chain, PEM).
    #[ts(optional = nullable)]
    pub ssl_key_path: Option<String>,
    #[ts(optional = nullable)]
    pub ssl_cert_path: Option<String>,
    /// The shell command of the `command` password mode. Stored with the profile (what it prints
    /// is never stored); treated like a secret by connection exports, which only write it when
    /// database passwords are included, and dropped on import unless the user keeps it.
    #[ts(optional = nullable)]
    pub password_command: Option<String>,
    /// MySQL "Use socket": connect through a Unix socket instead of host and port.
    #[ts(optional = nullable)]
    pub use_socket: Option<bool>,
    /// The socket file of "Use socket"; empty: the first of the usual paths that exists.
    #[ts(optional = nullable)]
    pub socket_path: Option<String>,
    /// Bootstrap CLI script (b546): a shell script run through the login shell before every
    /// connect and test (start a proxy, copy a SQLite file…); a failure aborts the connect.
    #[ts(optional = nullable)]
    pub bootstrap_script: Option<String>,
    /// Connection encoding: MySQL `SET NAMES <charset>` (default utf8mb4), PostgreSQL
    /// `client_encoding` (default UTF8). Not used by SQLite.
    #[ts(optional = nullable)]
    pub encoding: Option<String>,
    /// SQLCipher "SQLCipher compatibility" (`PRAGMA cipher_compatibility`, 1–4); None: 4, the
    /// current format. Only used with an encryption key (the SQLite profile's password).
    #[ts(optional = nullable)]
    pub sqlite_cipher_compatibility: Option<u8>,
    /// SQLite "Resource file" (`.sqliterc`): SQL statements read from this file at every
    /// connect and run after open (after the encryption key, before the bootstrap commands).
    #[ts(optional = nullable)]
    pub sqlite_resource_file: Option<String>,
    /// SQLite over SSH: upload the downloaded copy back on Commit / Disconnect; unset or false:
    /// the copy opens read-only.
    #[ts(optional = nullable)]
    pub sqlite_ssh_upload: Option<bool>,
    /// MySQL "Ver." dropdown; None = Default.
    #[ts(optional = nullable)]
    pub mysql_version: Option<MysqlVersion>,
    /// MySQL Other options ▸ Enable CLEARTEXT_PLUGIN: send the password in clear text when the
    /// server asks (`mysql_clear_password`, PAM / LDAP); only over TLS or a socket.
    #[ts(optional = nullable)]
    pub mysql_cleartext_plugin: Option<bool>,
    /// MySQL Other options ▸ Enable LOAD DATA LOCAL INFILE: the server may read local files, each
    /// one only after the user allowed it.
    #[ts(optional = nullable)]
    pub mysql_local_infile: Option<bool>,
    /// PostgreSQL SSL negotiation; None = postgres.
    #[ts(optional = nullable)]
    pub pg_ssl_negotiation: Option<SslNegotiation>,
    /// PostgreSQL Other options ▸ Disable channel binding (SCRAM-SHA-256-PLUS is not used).
    #[ts(optional = nullable)]
    pub pg_disable_channel_binding: Option<bool>,
    /// PostgreSQL extra connection parameters, libpq style (`key=value` separated by spaces,
    /// newlines or `&`; values may be single-quoted): `application_name` and `options` go into
    /// the startup message, other names are server settings applied after login
    /// (`statement_timeout`, `search_path`, `jit`…).
    #[ts(optional = nullable)]
    pub pg_params: Option<String>,
    /// "Lazy Load Schema Items" of the sidebar for this connection (`lazyload=` of a link's URL);
    /// None = the Settings ▸ General default.
    #[ts(optional = nullable)]
    pub lazy_load: Option<bool>,
    /// The PostgreSQL-compatible engine a `postgres` profile is for (CockroachDB, Greenplum,
    /// Redshift); None = PostgreSQL. Left out of the JSON when unset, so PostgreSQL profiles are
    /// stored as before.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub pg_flavor: Option<PgFlavor>,
    /// Settings of a driver beyond the common fields, which the driver itself parses (an
    /// account id, a key file path…). Never holds a secret (`drivers::registry::validate`).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub driver_params: Option<DriverParams>,
    /// Option keys this build does not know (a newer Tabler wrote them), kept as they are so a
    /// save here does not drop them. Not in the TS type: the dialog leaves them out of what it
    /// saves and `AppDb::upsert_profile` puts them back.
    #[serde(flatten)]
    #[ts(skip)]
    pub unknown: serde_json::Map<String, serde_json::Value>,
}

impl ProfileOptions {
    pub fn password_mode(&self) -> PasswordMode {
        self.password_mode.unwrap_or_default()
    }

    /// The trimmed password command, if one is set.
    pub fn password_command(&self) -> Option<&str> {
        self.password_command
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
    }

    /// The trimmed bootstrap CLI script, if one is set.
    pub fn bootstrap_script(&self) -> Option<&str> {
        self.bootstrap_script
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
    }

    /// The trimmed connection encoding, if one is set.
    pub fn encoding(&self) -> Option<&str> {
        self.encoding
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
    }
}

/// `ProfileOptions::driver_params`: a JSON object each driver reads its own settings from (a
/// newtype, which serde_json reads and writes as the object itself).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, type = "Record<string, unknown>")]
pub struct DriverParams(pub serde_json::Map<String, serde_json::Value>);

/// A saved connection. Never carries secrets: passwords live in the Keychain (see `secrets.rs`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConnectionProfile {
    pub id: Uuid,
    pub name: String,
    pub color: Option<String>,
    pub group: Option<String>,
    pub driver: Driver,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub database: Option<String>,
    #[serde(default)]
    pub ssl: SslMode,
    pub ssl_ca_path: Option<String>,
    pub sqlite_path: Option<String>,
    pub ssh: Option<SshConfig>,
    #[serde(default)]
    pub options: ProfileOptions,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    pub last_connected_at: Option<String>,
}

impl ConnectionProfile {
    /// A MySQL profile that connects through a Unix socket ("Use socket").
    pub fn uses_socket(&self) -> bool {
        self.driver.descriptor().socket && self.options.use_socket == Some(true)
    }

    /// The database of the new-connection picker this profile is for: its driver, or the flavor
    /// that driver serves (MariaDB, CockroachDB…).
    pub fn kind(&self) -> KindId {
        KindId::of(
            self.driver,
            self.options.pg_flavor,
            self.options.mysql_version,
        )
    }

    fn kind_descriptor(&self) -> &'static KindDescriptor {
        self.kind().descriptor()
    }

    /// The port the profile connects to when it sets none: its flavor's (CockroachDB 26257,
    /// Redshift 5439), or the driver's TLS port when TLS is on (ClickHouse 8443).
    pub fn default_port(&self) -> Option<u16> {
        match self.driver.descriptor().tls_port {
            Some(port) if self.ssl != SslMode::Disable => Some(port),
            _ => self.kind_descriptor().default_port,
        }
    }

    /// The database a profile that names none ends up on, where the engine has a fixed one
    /// (`defaultdb` on CockroachDB, `master` on SQL Server…).
    pub fn default_database(&self) -> Option<&'static str> {
        self.kind_descriptor().default_database
    }

    /// The SQL the profile's sessions speak, known before connecting: its kind's (a Redshift
    /// profile is the PostgreSQL driver speaking Redshift SQL). None for a native store.
    pub fn sql_dialect(&self) -> Option<SqlDialect> {
        self.kind_descriptor().dialect
    }
}

/// Payload of `save_profile` / `test_connection`: the profile plus any secrets being set.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProfileInput {
    pub profile: ConnectionProfile,
    pub password: Option<String>,
    pub ssh_password: Option<String>,
    pub ssh_passphrase: Option<String>,
    /// When true the stored DB password is removed.
    #[serde(default)]
    pub clear_password: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SecretStatus {
    pub has_password: bool,
    pub has_ssh_password: bool,
    pub has_ssh_passphrase: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TestResult {
    pub ok: bool,
    pub server_version: Option<String>,
    pub latency_ms: u64,
    pub message: String,
    pub tunnel_used: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TunnelInfo {
    pub local_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConnectionInfo {
    pub connection_id: Uuid,
    pub profile_id: Uuid,
    pub name: String,
    pub color: Option<String>,
    pub driver: Driver,
    pub server_version: String,
    pub current_database: Option<String>,
    pub current_user: Option<String>,
    pub tunnel: Option<TunnelInfo>,
    #[serde(default)]
    pub read_only: bool,
    /// What the session can do, as it reported at connect (absent from older payloads).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub capabilities: Option<SessionCaps>,
}

/// Payload of `export_connections` ("Export Connections ▸ …"): which profiles go to `path`, and
/// which of their secrets. Secrets require `file_password`; with a file password the whole file
/// is encrypted.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportConnectionsRequest {
    pub profile_ids: Vec<Uuid>,
    /// Group names written to the file (empty groups included).
    pub groups: Vec<String>,
    pub include_passwords: bool,
    pub include_ssh_secrets: bool,
    pub file_password: Option<String>,
    pub path: String,
}

/// One connection of a `.tablerconnections` file, as previewed before importing.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportPreviewItem {
    /// The connection as stored in the file (its id is the file's; importing assigns a new one).
    pub profile: ConnectionProfile,
    /// A saved connection already has this name.
    pub duplicate_name: bool,
    pub has_password: bool,
    pub has_ssh_secrets: bool,
}

/// A connection of a `.tablerconnections` file that this build cannot read: a newer Tabler wrote
/// it (a driver or an option value this one does not know). The preview lists it; it is never
/// imported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SkippedConnection {
    /// Its name in the file ("" when it has none).
    pub name: String,
    /// Why it cannot be read.
    pub reason: String,
}

/// What `inspect_connections_file` found. An encrypted file read without its password comes
/// back with `needs_password` and no connections.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConnectionsFilePreview {
    pub encrypted: bool,
    pub needs_password: bool,
    pub groups: Vec<String>,
    pub connections: Vec<ImportPreviewItem>,
    /// The file's connections this build cannot read (not in `connections`).
    pub skipped: Vec<SkippedConnection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum ConnStatus {
    Connected,
    Lost,
    Reconnecting,
    Disconnected,
}

/// Payload of the global `connection:status` event.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConnectionStatusEvent {
    pub connection_id: Uuid,
    pub status: ConnStatus,
    pub message: Option<String>,
}
