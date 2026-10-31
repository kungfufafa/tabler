//! The entry points `drivers::registry` calls (docs/drivers/FOUNDATION.md): opening a session on
//! the cluster a profile names, its checks, and the connection form's calls.
//!
//! Where the cluster is: a Cloud ID, the `urls` params (tried in order at connect), or the host
//! and port (HTTPS unless the SSL mode is Disable), under the path prefix. Over SSH the tunnel
//! reaches the profile's host and port, so the cluster must be named by them.

use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use crate::drivers::common::http::{HttpClient, HttpOptions};
use crate::drivers::native::{CallKind, unknown_call};
use crate::drivers::registry::{self, UtilInput};
use crate::drivers::{ConnectTarget, Session, SqlTrace};
use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, Driver, SslMode};

use super::auth::{self, AwsService};
use super::client::EsClient;
use super::params::{ElasticAuth, ElasticParams};
use super::processes::OPAQUE_PREFIX;
use super::session::EsSession;
use super::transport::{BaseUrl, HttpTransport};

const DRIVER: Driver = Driver::Elasticsearch;

/// The params of a profile.
fn params_of(profile: &ConnectionProfile) -> Result<ElasticParams, AppError> {
    ElasticParams::from_driver_params(profile.options.driver_params.as_ref())
}

/// Whether plain host names (and the connection URL schemes) mean HTTPS.
fn https(profile: &ConnectionProfile) -> bool {
    profile.ssl != SslMode::Disable
}

/// The cluster's addresses, in the order they are tried.
pub fn bases(
    profile: &ConnectionProfile,
    params: &ElasticParams,
) -> Result<Vec<BaseUrl>, AppError> {
    let mut out = if let Some(id) = params.cloud_id() {
        vec![auth::cloud_id_url(id)?]
    } else if !params.urls().is_empty() {
        params
            .urls()
            .iter()
            .map(|u| BaseUrl::parse(u, https(profile)))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        let host = profile
            .host
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .ok_or_else(|| {
                AppError::InvalidInput(
                    "Elasticsearch: enter the host, the URLs or a Cloud ID".into(),
                )
            })?;
        let mut base = BaseUrl::parse(host, https(profile))?;
        // A port of its own wins over the host's default.
        if let Some(port) = profile.port.filter(|_| !host.contains("://")) {
            base.port = port;
        }
        vec![base]
    };
    if let Some(prefix) = params.path_prefix() {
        for base in out.iter_mut().filter(|b| b.prefix.is_empty()) {
            base.prefix = prefix.to_string();
        }
    }
    // An `http://` URL never wins over a profile that asks for TLS: the credentials would go in
    // clear text and neither the certificate nor the CA pin would be checked.
    let wants_tls = matches!(
        profile.ssl,
        SslMode::Require | SslMode::VerifyCa | SslMode::VerifyFull
    ) || params.pin()?.is_some();
    if wants_tls && let Some(plain) = out.iter().find(|b| !b.https) {
        return Err(AppError::InvalidInput(format!(
            "Elasticsearch: {} is plain HTTP, but the SSL mode or the CA fingerprint asks for \
             TLS: use https://, or set the SSL mode to Disable or Prefer",
            plain.root()
        )));
    }
    Ok(out)
}

/// Over SSH the tunnel forwards to the profile's host and port: the cluster must be named by
/// them, not by URLs or a Cloud ID elsewhere.
fn check_tunnel(profile: &ConnectionProfile, bases: &[BaseUrl]) -> Result<(), AppError> {
    let host = profile
        .host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .unwrap_or(crate::tunnel::LOCAL_HOST);
    let port = profile.port.or_else(|| profile.default_port()).unwrap_or(0);
    let reached = |b: &BaseUrl| b.host.eq_ignore_ascii_case(host) && b.port == port;
    match bases {
        [b] if reached(b) => Ok(()),
        _ => Err(AppError::InvalidInput(format!(
            "Elasticsearch over SSH reaches {host}:{port} only: name the cluster by the host and \
             port (not URLs or a Cloud ID)"
        ))),
    }
}

/// OpenSearch Serverless (`aoss`) is refused, by the service SigV4 would sign for or by its
/// endpoint host: a collection has no `GET /`, no scroll, no task API, no data streams and none
/// of the index actions, and OpenSearch has no `_shard_doc` to page a point in time exactly, so
/// this session cannot serve one.
fn refuse_serverless(params: &ElasticParams, bases: &[BaseUrl]) -> Result<(), AppError> {
    let signs_aoss =
        params.auth == ElasticAuth::AwsSigv4 && params.aws_service() == Some(AwsService::Aoss);
    let aoss_host = bases
        .iter()
        .any(|b| AwsService::for_host(&b.host) == AwsService::Aoss);
    if signs_aoss || aoss_host {
        Err(AppError::Unsupported(
            "OpenSearch Serverless (aoss) collections are not supported yet; Amazon OpenSearch \
             Service domains (es) are"
                .into(),
        ))
    } else {
        Ok(())
    }
}

/// The HTTP settings of a profile: its TLS (SSL mode, CA file, client certificate, CA pin) and
/// time outs.
fn options(profile: &ConnectionProfile, params: &ElasticParams) -> Result<HttpOptions, AppError> {
    let mut o = HttpOptions::for_profile(profile)?;
    o.tls.pin_sha256 = params.pin()?;
    o.request_timeout = params
        .request_timeout_secs
        .map(|s| Duration::from_secs(u64::from(s)));
    Ok(o)
}

/// The least time an address gets to answer at connect, however many follow it.
const MIN_NODE_TIME: Duration = Duration::from_secs(2);

/// How long the next of `left` addresses may take to answer: the time left until `deadline`
/// shared among them, at least [`MIN_NODE_TIME`] but never past the deadline.
fn node_share(deadline: tokio::time::Instant, left: usize) -> Duration {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    let n = u32::try_from(left.max(1)).unwrap_or(u32::MAX);
    (remaining / n).max(MIN_NODE_TIME).min(remaining)
}

/// MCP (v1) serves SQL engines only: in the MCP process (`unattended`) a native store does not
/// open.
pub fn refuse_unattended(unattended: bool) -> Result<(), AppError> {
    if unattended {
        Err(AppError::Unsupported(format!(
            "{} is not available to MCP",
            DRIVER.label()
        )))
    } else {
        Ok(())
    }
}

/// Opens a session on the profile's cluster: the first of its addresses that answers.
pub async fn open(target: ConnectTarget<'_>) -> Result<Session, AppError> {
    refuse_unattended(registry::unattended())?;
    let profile = target.profile;
    let params = params_of(profile)?;
    let bases = bases(profile, &params)?;
    refuse_serverless(&params, &bases)?;
    if target.host_override.is_some() {
        check_tunnel(profile, &bases)?;
    }
    let read_only = crate::conn_options::effective_read_only(profile);
    let session_id = Uuid::new_v4().simple().to_string();
    let opaque = format!(
        "{OPAQUE_PREFIX}{}",
        session_id.get(..12).unwrap_or(&session_id)
    );
    let trace = SqlTrace::default();
    // The connect time out is shared among the addresses: one that never answers (its packets
    // dropped, or a proxy that accepts and hangs) still leaves time to try the next.
    let deadline = tokio::time::Instant::now() + options(profile, &params)?.connect_timeout;
    let count = bases.len();
    let mut last = None;
    for (i, base) in bases.into_iter().enumerate() {
        let credential = auth::credential(profile, &params, target.password, &base).await?;
        let http = HttpClient::for_target(
            &target,
            &base.root(),
            options(profile, &params)?,
            credential,
        )?;
        let client = EsClient::new(Arc::new(HttpTransport::new(http)), base.clone(), &opaque)
            .with_trace(trace.clone());
        let share = node_share(deadline, count - i);
        let answer = tokio::time::timeout(share, client.info())
            .await
            .unwrap_or(Err(AppError::Timeout));
        match answer {
            Ok(info) => {
                let user = (params.auth == ElasticAuth::Basic)
                    .then(|| profile.user.clone())
                    .flatten()
                    .filter(|u| !u.trim().is_empty());
                return Ok(Session::Native(Arc::new(EsSession::new(
                    client, info, params, read_only, user, trace,
                ))));
            }
            // Another node may answer.
            Err(e @ (AppError::Connect(_) | AppError::Timeout | AppError::NotConnected)) => {
                log::info!("Elasticsearch: {} did not answer: {e}", base.root());
                last = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| AppError::Connect("no Elasticsearch address to connect to".into())))
}

/// The driver's own checks of a profile being saved or tested (its `driver_params`), after the
/// registry's common ones.
pub fn validate(profile: &ConnectionProfile) -> Result<(), AppError> {
    let params = params_of(profile)?;
    let bases = bases(profile, &params)?;
    refuse_serverless(&params, &bases)?;
    if profile.ssh.is_some() {
        check_tunnel(profile, &bases)?;
    }
    if params.auth == ElasticAuth::AwsSigv4 {
        for base in &bases {
            auth::region(&params, &base.host)?;
        }
    }
    Ok(())
}

/// A call of the connection form that needs no session.
///
/// - `cloudIdUrl {cloudId}` → `{url}`: the cluster a Cloud ID names.
/// - `awsEndpoint {host}` → `{region, service}`: what SigV4 signs for at an Amazon host.
pub async fn util(
    _input: UtilInput<'_>,
    method: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    let arg = |name: &str| {
        args.get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                AppError::InvalidInput(format!("{}: {name} is required", DRIVER.label()))
            })
    };
    match method {
        "cloudIdUrl" => {
            Ok(serde_json::json!({ "url": auth::cloud_id_url(arg("cloudId")?)?.root() }))
        }
        "awsEndpoint" => {
            let base = BaseUrl::parse(arg("host")?, true)?;
            Ok(serde_json::json!({
                "region": auth::region_for_host(&base.host),
                "service": AwsService::for_host(&base.host).name(),
            }))
        }
        other => Err(unknown_call(DRIVER, other)),
    }
}

/// Whether a `util` call only reads; anything unknown changes something.
pub fn util_kind(method: &str) -> CallKind {
    match method {
        "cloudIdUrl" | "awsEndpoint" => CallKind::Read,
        _ => CallKind::Write,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn addresses_share_the_connect_time_out() {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        assert_eq!(node_share(deadline, 2), Duration::from_secs(30));
        assert_eq!(node_share(deadline, 1), Duration::from_secs(60));
        // Many addresses: each still gets the least time, and none goes past the deadline.
        assert_eq!(node_share(deadline, 100), MIN_NODE_TIME);
        let soon = tokio::time::Instant::now() + Duration::from_secs(1);
        assert_eq!(node_share(soon, 3), Duration::from_secs(1));
        tokio::time::advance(Duration::from_secs(5)).await;
        assert_eq!(node_share(soon, 1), Duration::ZERO);
    }
}
