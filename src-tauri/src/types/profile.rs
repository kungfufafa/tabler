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

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ProfileOptions {
    /// Environment tag (Local, Testing, …); None for profiles saved before tags existed.
    pub tag: Option<ConnectionTag>,
    pub timezone: Option<String>,
    pub connect_timeout_secs: Option<u32>,
    pub read_only: bool,
    pub startup_sql: Option<String>,
    pub default_row_limit: Option<u32>,
    pub sqlite_foreign_keys: bool,
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
