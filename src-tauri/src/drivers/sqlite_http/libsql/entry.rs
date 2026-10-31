//! The entry points `drivers::registry` calls (docs/drivers/FOUNDATION.md): a LibSQL server
//! (Turso, `sqld`) is reached at the profile's host, port and SSL (HTTPS, or HTTP when SSL is
//! off), or at the URL its params name, with the profile's password as the auth token. HTTPS
//! checks the server's certificate unless the profile says plain "HTTPS" (`require`).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::client::{HranaClient, Prelude};
use super::session::LibsqlSession;
use super::url::{base_from_parts, parse_url};
use crate::conn_options::effective_read_only;
use crate::drivers::common::http::{Credential, HttpOptions};
use crate::drivers::common::token::TokenSource;
use crate::drivers::native::{CallKind, unknown_call};
use crate::drivers::registry::UtilInput;
use crate::drivers::sqlite_http::transport::HttpTransport;
use crate::drivers::{ConnectTarget, Session};
use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, Driver, DriverParams, SslMode};

const DRIVER: Driver = Driver::Libsql;

/// The LibSQL settings of a profile beyond the common fields (`options.driver_params`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct LibsqlParams {
    /// The server's URL in place of host and port, for a server under a path
    /// (`https://example.com/db`): its scheme says HTTP or HTTPS (`libsql://` is HTTPS). The
    /// certificate is checked as the SSL mode says: fully unless it is plain "HTTPS"
    /// (`require`), with the profile's CA file. Never with a token or a password in it.
    pub url: Option<String>,
    /// Sends the user and the password as HTTP Basic credentials (a `sqld` behind basic auth)
    /// instead of the password as a bearer token.
    pub basic_auth: bool,
}

impl LibsqlParams {
    pub fn from_driver_params(params: Option<&DriverParams>) -> Result<LibsqlParams, AppError> {
        let Some(DriverParams(map)) = params else {
            return Ok(LibsqlParams::default());
        };
        serde_json::from_value(serde_json::Value::Object(map.clone()))
            .map_err(|e| AppError::InvalidInput(format!("LibSQL settings: {e}")))
    }

    fn url(&self) -> Option<&str> {
        self.url.as_deref().map(str::trim).filter(|u| !u.is_empty())
    }
}

/// Where a profile's server is: the HTTP base the Hrana endpoints hang off, and whether it is
/// HTTPS. Host and port are the profile's own (an SSH tunnel forwards to them), never the
/// tunnel's.
pub fn base_of(profile: &ConnectionProfile) -> Result<(String, bool), AppError> {
    let params = LibsqlParams::from_driver_params(profile.options.driver_params.as_ref())?;
    if let Some(url) = params.url() {
        let parsed = parse_url(url)?;
        return Ok((parsed.base, parsed.tls));
    }
    let host = profile.host.as_deref().unwrap_or_default();
    let tls = profile.ssl != SslMode::Disable;
    Ok((
        base_from_parts(host, profile.port.or_else(|| profile.default_port()), tls)?,
        tls,
    ))
}

/// How HTTPS checks the server's certificate for a profile's SSL mode. There is no plain HTTP
/// to fall back to, so `prefer` (a new profile's mode, which the form's HTTPS menu does not
/// offer) checks it fully, and so does `disable` with a URL whose scheme is HTTPS: the auth token
/// goes only to the server the name says. Plain "HTTPS" (`require`) is the choice of a server
/// whose certificate cannot be checked; `verify-ca` and `verify-full` are kept as chosen.
pub fn certificate_check(ssl: SslMode) -> SslMode {
    match ssl {
        SslMode::Disable | SslMode::Prefer => SslMode::VerifyFull,
        chosen => chosen,
    }
}

/// Opens a session on the profile's server.
pub async fn open(target: ConnectTarget<'_>) -> Result<Session, AppError> {
    let profile = target.profile;
    validate(profile)?;
    let params = LibsqlParams::from_driver_params(profile.options.driver_params.as_ref())?;
    let (base, tls) = base_of(profile)?;
    let token = TokenSource::for_profile(profile, target.password);
    let credential = if params.basic_auth {
        Credential::Basic(profile.user.clone().unwrap_or_default(), token)
    } else {
        Credential::bearer(token)
    };
    let mut options = HttpOptions::for_profile(profile)?;
    options.tls.mode = certificate_check(profile.ssl);
    let transport = HttpTransport::for_target(&target, &base, options, credential)?;
    let read_only = effective_read_only(profile);
    // `query_only` until the server is found to refuse it (`LibsqlSession::connect`).
    let prelude = Prelude::new(
        Some(profile.options.sqlite_foreign_keys),
        profile.options.startup_sql.as_deref(),
        read_only,
    );
    let database = reqwest::Url::parse(&base)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| base.clone());
    let client = HranaClient::new(Arc::new(transport), base, prelude);
    let session = LibsqlSession::connect(client, database, tls, read_only).await?;
    Ok(Session::Sql(Arc::new(session)))
}

/// The profile's host or URL, and no secret in its params: a pasted URL's token or password
/// belongs in the Auth Token field (`driver_params` are stored as plain text).
pub fn validate(profile: &ConnectionProfile) -> Result<(), AppError> {
    let params = LibsqlParams::from_driver_params(profile.options.driver_params.as_ref())?;
    match params.url() {
        Some(url) => {
            let parsed = parse_url(url)?;
            if parsed.has_secret {
                return Err(AppError::InvalidInput(
                    "LibSQL: the URL holds a token or a password: put it in the Auth Token \
                     field (and the user in User)"
                        .into(),
                ));
            }
            // The tunnel forwards to the profile's host and port, which a URL does not set.
            if profile.ssh.is_some() {
                return Err(AppError::InvalidInput(
                    "LibSQL over SSH takes the host and port, not a URL".into(),
                ));
            }
        }
        None => {
            let host = profile.host.as_deref().unwrap_or_default();
            if host.trim().is_empty() {
                return Err(AppError::InvalidInput("Host is required".into()));
            }
            base_from_parts(host, profile.port, profile.ssl != SslMode::Disable)?;
        }
    }
    if params.basic_auth && profile.user.as_deref().is_none_or(|u| u.trim().is_empty()) {
        return Err(AppError::InvalidInput(
            "LibSQL: Basic authentication needs a user".into(),
        ));
    }
    Ok(())
}

/// The connection form has no calls of LibSQL's own.
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
