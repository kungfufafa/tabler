//! The entry points `drivers::registry` calls (docs/drivers/FOUNDATION.md): a ClickHouse server's
//! HTTP interface at the profile's host and port (HTTPS unless SSL is off), as the profile's user
//! with its password, or with its client certificate (X.509 users).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::request::{self, ConnSettings, Credentials, Endpoint};
use super::session::{ClickhouseSession, SessionOptions};
use super::transport::{HttpTransport, header};
use crate::conn_options::effective_read_only;
use crate::drivers::common::http::{Credential, HttpClient, HttpOptions};
use crate::drivers::common::token::TokenSource;
use crate::drivers::native::{CallKind, unknown_call};
use crate::drivers::registry::{self, UtilInput};
use crate::drivers::{ConnectTarget, Session};
use crate::error::AppError;
use crate::types::caps::SafeConfirm;
use crate::types::profile::{ConnectionProfile, Driver, DriverParams, PasswordMode, SslMode};

const DRIVER: Driver = Driver::Clickhouse;

/// The longest idle time of the editor's HTTP session Tabler asks for: the server refuses more
/// than its `max_session_timeout` (3600 s by default).
pub const MAX_SESSION_TIMEOUT_SECS: u32 = 3600;

/// A setting sent with every request (`SET name = value` for the connection).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClickhouseSetting {
    pub name: String,
    pub value: String,
}

/// The ClickHouse settings of a profile beyond the common fields (`options.driver_params`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ClickhouseParams {
    /// Sign in with the profile's client certificate (a user `IDENTIFIED WITH ssl_certificate`)
    /// instead of a password: needs HTTPS and the certificate and key files.
    pub certificate_auth: bool,
    /// Roles to act as on every request (`role=`, ClickHouse 24.4+).
    pub roles: Vec<String>,
    /// Settings sent with every request. The ones Tabler sets itself, and those that change what
    /// a statement means or where a read goes, are refused (`request::validate_settings`).
    pub settings: Vec<ClickhouseSetting>,
    /// Seconds the editor's session (its `SET`s and temporary tables) lasts while idle; the
    /// server's default (60 s) when unset.
    pub session_timeout_secs: Option<u32>,
}

impl ClickhouseParams {
    pub fn from_driver_params(params: Option<&DriverParams>) -> Result<ClickhouseParams, AppError> {
        let Some(DriverParams(map)) = params else {
            return Ok(ClickhouseParams::default());
        };
        serde_json::from_value(serde_json::Value::Object(map.clone()))
            .map_err(|e| AppError::InvalidInput(format!("ClickHouse settings: {e}")))
    }

    fn roles(&self) -> Vec<String> {
        self.roles
            .iter()
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
            .collect()
    }

    fn settings(&self) -> Vec<(String, String)> {
        self.settings
            .iter()
            .map(|s| (s.name.trim().to_string(), s.value.clone()))
            .collect()
    }
}

/// How HTTPS checks the server's certificate for a profile's SSL mode. `prefer` (a new
/// profile's) has no plain HTTP to fall back to, so it checks fully: the password goes only to
/// the server the name says. Plain "HTTPS" (`require`) is the choice of a server whose
/// certificate cannot be checked; `verify-ca` and `verify-full` are kept as chosen.
pub fn certificate_check(ssl: SslMode) -> SslMode {
    match ssl {
        SslMode::Disable | SslMode::Prefer => SslMode::VerifyFull,
        chosen => chosen,
    }
}

/// The server as the requests name it: the profile's own host and port (an SSH tunnel forwards
/// to them), HTTPS unless SSL is off.
pub fn endpoint_of(profile: &ConnectionProfile) -> Endpoint {
    Endpoint {
        tls: profile.ssl != SslMode::Disable,
        host: profile
            .host
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .to_string(),
        port: profile
            .port
            .or_else(|| profile.default_port())
            .unwrap_or(8123),
    }
}

/// Who the requests say they are, and what the transport adds: a certificate user sends no
/// password; a Command Line password goes through the transport (run again when the server
/// refuses it, see `refresh_of`), a stored one in the request's own header.
pub fn credentials(
    params: &ClickhouseParams,
    user: &str,
    token: TokenSource,
) -> (Credentials, Credential) {
    let user = user.to_string();
    if params.certificate_auth {
        return (Credentials::Certificate { user }, Credential::None);
    }
    match token {
        token @ (TokenSource::Command(_) | TokenSource::Issued(_)) => (
            Credentials::Key { user },
            Credential::Header(header::KEY, token),
        ),
        TokenSource::Static(password) => {
            (Credentials::Password { user, password }, Credential::None)
        }
        TokenSource::None => (
            Credentials::Password {
                user,
                password: String::new(),
            },
            Credential::None,
        ),
    }
}

/// The same command or login as `token`, for `HttpTransport::refreshing` to run again after a
/// refused password (a stored password or a certificate has nothing to refresh).
pub fn refresh_of(params: &ClickhouseParams, token: &TokenSource) -> Option<TokenSource> {
    match token {
        _ if params.certificate_auth => None,
        TokenSource::Command(command) => Some(TokenSource::Command(command.clone())),
        TokenSource::Issued(tokens) => Some(TokenSource::Issued(tokens.clone())),
        TokenSource::Static(_) | TokenSource::None => None,
    }
}

/// What a session of `profile` sends with every request. `unattended` (the MCP process) is
/// read-only like a read-only profile: `readonly=2` on every request.
pub fn conn_settings(
    profile: &ConnectionProfile,
    params: &ClickhouseParams,
    unattended: bool,
) -> ConnSettings {
    ConnSettings {
        database: profile
            .database
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_string),
        read_only: effective_read_only(profile) || unattended,
        roles: params.roles(),
        settings: params.settings(),
        output_settings: true,
    }
}

/// Opens a session on the profile's server.
pub async fn open(target: ConnectTarget<'_>) -> Result<Session, AppError> {
    let profile = target.profile;
    validate(profile)?;
    let params = ClickhouseParams::from_driver_params(profile.options.driver_params.as_ref())?;
    let endpoint = endpoint_of(profile);
    let user = profile
        .user
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .unwrap_or("default");
    let token = TokenSource::for_profile(profile, target.password);
    let refresh = refresh_of(&params, &token);
    let (creds, credential) = credentials(&params, user, token);
    let mut options = HttpOptions::for_profile(profile)?;
    options.tls.mode = certificate_check(profile.ssl);
    let client = HttpClient::for_target(&target, &endpoint.base_url(), options, credential)?;
    let confirm = if profile.options.password_mode() == PasswordMode::Command {
        SafeConfirm::Passcode
    } else {
        SafeConfirm::Password
    };
    let session = ClickhouseSession::start(
        Arc::new(HttpTransport::new(client).refreshing(refresh)),
        endpoint.clone(),
        creds,
        conn_settings(profile, &params, registry::unattended()),
        SessionOptions {
            tls: endpoint.tls,
            confirm,
            session_timeout_secs: params.session_timeout_secs,
        },
    )
    .await?;
    Ok(Session::Sql(Arc::new(session)))
}

/// The profile's host, its ClickHouse settings (`ClickhouseParams`), and what a certificate
/// sign-in needs.
pub fn validate(profile: &ConnectionProfile) -> Result<(), AppError> {
    let params = ClickhouseParams::from_driver_params(profile.options.driver_params.as_ref())?;
    if profile.host.as_deref().is_none_or(|h| h.trim().is_empty()) {
        return Err(AppError::InvalidInput("Host is required".into()));
    }
    request::validate_settings(&params.settings())?;
    if let Some(role) = params
        .roles()
        .into_iter()
        .find(|r| r.chars().any(|c| c.is_control()))
    {
        return Err(AppError::InvalidInput(format!(
            "ClickHouse: \u{201c}{role}\u{201d} is not a role name"
        )));
    }
    if let Some(secs) = params.session_timeout_secs
        && !(1..=MAX_SESSION_TIMEOUT_SECS).contains(&secs)
    {
        return Err(AppError::InvalidInput(format!(
            "ClickHouse: the session timeout is 1 to {MAX_SESSION_TIMEOUT_SECS} seconds"
        )));
    }
    let blank = |s: Option<&str>| s.is_none_or(|s| s.trim().is_empty());
    let (cert, key) = (
        blank(profile.options.ssl_cert_path.as_deref()),
        blank(profile.options.ssl_key_path.as_deref()),
    );
    if cert != key {
        return Err(AppError::InvalidInput(
            "ClickHouse: a client certificate needs both its certificate and its key file".into(),
        ));
    }
    if params.certificate_auth {
        if profile.ssl == SslMode::Disable {
            return Err(AppError::InvalidInput(
                "ClickHouse: signing in with a certificate needs HTTPS (turn SSL on)".into(),
            ));
        }
        if cert {
            return Err(AppError::InvalidInput(
                "ClickHouse: signing in with a certificate needs the client certificate and key \
                 files"
                    .into(),
            ));
        }
    }
    Ok(())
}

/// The connection form has no calls of ClickHouse's own.
pub async fn util(
    _input: UtilInput<'_>,
    method: &str,
    _args: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    Err(unknown_call(DRIVER, method))
}

/// Whether a `util` call only reads; anything unknown changes something.
pub fn util_kind(_method: &str) -> CallKind {
    CallKind::Write
}
