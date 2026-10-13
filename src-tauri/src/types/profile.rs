use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Driver {
    Postgres,
    Mysql,
    Sqlite,
}

impl Driver {
    pub fn default_port(self) -> Option<u16> {
        match self {
            Driver::Postgres => Some(5432),
            Driver::Mysql => Some(3306),
            Driver::Sqlite => None,
        }
    }
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
}

/// TablePlus-style environment tag of a connection; the workspace's status bar shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum ConnectionTag {
    Local,
    Testing,
    Development,
    Staging,
    Production,
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
}

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
        self.driver == Driver::Mysql && self.options.use_socket == Some(true)
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
