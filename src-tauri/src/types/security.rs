//! App lock (passcode, Touch ID) and connection security details.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Settings ▸ Security ▸ Passcode: whether one is set, and whether this Mac can ask for Touch ID
/// (or the account password, LocalAuthentication's fallback).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PasscodeStatus {
    pub enabled: bool,
    pub biometrics: bool,
}

/// What the status bar's details popover shows about an open session beyond `ConnectionInfo`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionDetails {
    /// The session to the server is encrypted (TLS/SSL), whatever the tunnel does.
    pub tls: bool,
    /// e.g. `TLSv1.3`, when the server reports it.
    pub tls_version: Option<String>,
    /// e.g. `TLS_AES_256_GCM_SHA384`, when the server reports it.
    pub tls_cipher: Option<String>,
    /// The client / connection character set (`UTF8`, `utf8mb4`, `UTF-8`).
    pub encoding: Option<String>,
}
