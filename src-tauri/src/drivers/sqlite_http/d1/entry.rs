//! The entry points `drivers::registry` calls (docs/drivers/FOUNDATION.md): a D1 database of a
//! Cloudflare account (its UUID or its name in the profile's database), reached through the
//! REST API with the profile's password as the API token.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::api::{D1Target, DEFAULT_API_BASE, DatabaseInfo, validate_account_id};
use super::client::{D1Client, list_databases, resolve_database};
use super::decode::ParamEncoding;
use super::session::D1Session;
use crate::conn_options::effective_read_only;
use crate::drivers::common::http::{Credential, HttpOptions, HttpTls};
use crate::drivers::common::token::TokenSource;
use crate::drivers::native::{CallKind, unknown_call};
use crate::drivers::registry::UtilInput;
use crate::drivers::sqlite_http::transport::HttpTransport;
use crate::drivers::{ConnectTarget, Session};
use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, Driver, DriverParams};

const DRIVER: Driver = Driver::D1;

/// The connection form's call that lists the account's databases (the database picker).
pub const LIST_DATABASES: &str = "listDatabases";

/// Databases per page of the picker's list.
const PAGE: u32 = 100;

/// The D1 settings of a profile beyond the common fields (`options.driver_params`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct D1Params {
    /// The Cloudflare account: 32 hexadecimal characters (Workers & Pages ▸ Overview).
    pub account_id: String,
    /// The API's base URL in place of `https://api.cloudflare.com/client/v4` (a gateway, a test
    /// server). HTTPS, or HTTP to this machine only: the API token goes there.
    pub api_base: Option<String>,
}

impl D1Params {
    pub fn from_driver_params(params: Option<&DriverParams>) -> Result<D1Params, AppError> {
        let Some(DriverParams(map)) = params else {
            return Ok(D1Params::default());
        };
        serde_json::from_value(serde_json::Value::Object(map.clone()))
            .map_err(|e| AppError::InvalidInput(format!("Cloudflare D1 settings: {e}")))
    }

    fn of(profile: &ConnectionProfile) -> Result<D1Params, AppError> {
        D1Params::from_driver_params(profile.options.driver_params.as_ref())
    }

    /// The API base, checked: the token must not travel in clear text to another machine.
    pub fn api_base(&self) -> Result<String, AppError> {
        let base = self
            .api_base
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .unwrap_or(DEFAULT_API_BASE)
            .trim_end_matches('/');
        let url = reqwest::Url::parse(base)
            .map_err(|e| AppError::InvalidInput(format!("Cloudflare D1: the API base: {e}")))?;
        let host = url
            .host_str()
            .unwrap_or_default()
            .trim_start_matches('[')
            .trim_end_matches(']');
        let loopback = host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
        match url.scheme() {
            "https" => Ok(base.to_string()),
            "http" if loopback => Ok(base.to_string()),
            _ => Err(AppError::InvalidInput(
                "Cloudflare D1: the API base must be an https:// URL (http:// only to this \
                 machine)"
                    .into(),
            )),
        }
    }
}

/// The API's transport for `target`'s profile: the certificate always checked (the profile's SSL
/// settings are for servers of its own), the profile's connect timeout, the token as a bearer.
fn transport(target: &ConnectTarget<'_>, api_base: &str) -> Result<HttpTransport, AppError> {
    let options = HttpOptions {
        tls: HttpTls::verified(),
        ..HttpOptions::for_profile(target.profile)?
    };
    let token = TokenSource::for_profile(target.profile, target.password);
    HttpTransport::for_target(target, api_base, options, Credential::bearer(token))
}

/// Opens a session on the profile's database (a name is looked up once; the session keeps the
/// UUID).
pub async fn open(target: ConnectTarget<'_>) -> Result<Session, AppError> {
    let profile = target.profile;
    validate(profile)?;
    let params = D1Params::of(profile)?;
    validate_account_id(&params.account_id)?;
    let api_base = params.api_base()?;
    let database = profile.database.as_deref().unwrap_or_default().trim();
    let transport = Arc::new(transport(&target, &api_base)?);
    let uuid = resolve_database(
        transport.as_ref(),
        &api_base,
        params.account_id.trim(),
        database,
    )
    .await?;
    let client = D1Client::new(
        transport,
        D1Target::new(Some(&api_base), &params.account_id, &uuid)?,
        ParamEncoding::Native,
    );
    let session = D1Session::connect(client, effective_read_only(profile)).await?;
    Ok(Session::Sql(Arc::new(session)))
}

/// The params parse, and the API base is one the token may go to. The account ID's form is
/// checked when connecting.
pub fn validate(profile: &ConnectionProfile) -> Result<(), AppError> {
    D1Params::of(profile)?.api_base().map(|_| ())
}

/// The form's calls: `listDatabases` lists the account's databases for the picker
/// (`[{uuid, name, createdAt, fileSize}]`), with the token being edited.
pub async fn util(
    input: UtilInput<'_>,
    method: &str,
    _args: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    if method != LIST_DATABASES {
        return Err(unknown_call(DRIVER, method));
    }
    let params = D1Params::of(input.profile)?;
    validate_account_id(&params.account_id)?;
    let api_base = params.api_base()?;
    let target = ConnectTarget {
        profile: input.profile,
        password: input.secret,
        host_override: None,
    };
    let found = list_databases(
        &transport(&target, &api_base)?,
        &api_base,
        params.account_id.trim(),
        None,
        PAGE,
    )
    .await?;
    Ok(serde_json::Value::Array(
        found.iter().map(picker_entry).collect(),
    ))
}

fn picker_entry(d: &DatabaseInfo) -> serde_json::Value {
    serde_json::json!({
        "uuid": d.uuid,
        "name": d.name,
        "createdAt": d.created_at,
        "fileSize": d.file_size,
    })
}

/// Whether a `util` call only reads; anything unknown changes something.
pub fn util_kind(method: &str) -> CallKind {
    if method == LIST_DATABASES {
        CallKind::Read
    } else {
        CallKind::Write
    }
}
