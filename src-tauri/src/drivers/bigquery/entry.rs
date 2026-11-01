//! The entry points `drivers::registry` calls (docs/drivers/FOUNDATION.md): a BigQuery project
//! (the profile's database) reached through the REST API, signed in with a service-account key
//! file, Application Default Credentials or an access token (the profile's secret, or what its
//! Command Line mode prints).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::auth::{
    CredentialFile, SCOPE_BIGQUERY, adc_path, issued_tokens, read_credential_file, token_endpoint,
};
use super::client::Client;
use super::jobs::{DatasetReference, QueryOptions};
use super::params::{BigqueryAuth, BigqueryParams, DEFAULT_MAXIMUM_BYTES_BILLED};
use super::session::{BigquerySession, SessionConfig};
use super::transport::{HttpTransport, retry_policy};
use crate::conn_options::effective_read_only;
use crate::drivers::common::http::{Credential, HttpOptions, HttpTls};
use crate::drivers::common::token::TokenSource;
use crate::drivers::native::{CallKind, unknown_call};
use crate::drivers::registry::{UtilInput, unattended};
use crate::drivers::{ConnectTarget, Session};
use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, Driver};

const DRIVER: Driver = Driver::Bigquery;

/// The connection form's calls (no session yet): the project's datasets (the default dataset's
/// picker) and the projects the account can see.
pub const LIST_DATASETS: &str = "listDatasets";
pub const LIST_PROJECTS: &str = "listProjects";

/// The longest one request may take: a poll waits 10 s on the server and a page is about 10 MB,
/// so a request still going after this is a connection that hangs.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// The project the profile names.
fn project_of(profile: &ConnectionProfile) -> Result<String, AppError> {
    profile
        .database
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidInput("BigQuery: enter the project".into()))
}

/// The requests' options: Google's certificates always checked (the profile's SSL settings are
/// for servers of its own), the profile's connect timeout, a limit per request, and GETs retried
/// after a server error.
pub fn http_options(profile: &ConnectionProfile) -> Result<HttpOptions, AppError> {
    Ok(HttpOptions {
        tls: HttpTls::verified(),
        request_timeout: Some(REQUEST_TIMEOUT),
        retry: retry_policy(),
        ..HttpOptions::for_profile(profile)?
    })
}

/// What a sign-in gives: the tokens, the project user credentials bill their API calls to, and
/// who signs in (a service account's address).
struct Login {
    tokens: TokenSource,
    quota_project: Option<String>,
    user: Option<String>,
}

/// The profile's sign-in. A key file and Application Default Credentials are read now and never
/// stored; a service account asks for the read-only scope on a read-only profile.
async fn login(
    profile: &ConnectionProfile,
    params: &BigqueryParams,
    secret: Option<&str>,
) -> Result<Login, AppError> {
    let file = match params.auth {
        BigqueryAuth::AccessToken => {
            let tokens = TokenSource::for_profile(profile, secret);
            if matches!(tokens, TokenSource::None) {
                return Err(AppError::Auth(
                    "BigQuery: enter the access token, or a command that prints one (gcloud auth \
                     print-access-token)"
                        .into(),
                ));
            }
            return Ok(Login {
                tokens,
                quota_project: None,
                user: None,
            });
        }
        BigqueryAuth::ServiceAccount => {
            let path = params
                .key_file_path
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .ok_or_else(|| {
                    AppError::InvalidInput(
                        "BigQuery: choose the service account's key file (JSON).".into(),
                    )
                })?;
            read_credential_file(Path::new(path)).await?
        }
        BigqueryAuth::ApplicationDefault => {
            let path = adc_path().ok_or_else(|| {
                AppError::Auth(
                    "BigQuery: no Application Default Credentials: run `gcloud auth \
                     application-default login`, or set GOOGLE_APPLICATION_CREDENTIALS"
                        .into(),
                )
            })?;
            read_credential_file(&path).await?
        }
    };
    // The full scope on every profile: `jobs.insert`, which runs every statement, refuses the
    // `bigquery.readonly` scope (read-only is the classifier's job, ClassifierOnly).
    let scope = SCOPE_BIGQUERY;
    let token_transport = Arc::new(HttpTransport::token_endpoint(
        &token_endpoint(&file)?,
        http_options(profile)?,
    )?);
    let (quota_project, user) = match &file {
        CredentialFile::AuthorizedUser(u) => (u.quota_project_id.clone(), None),
        CredentialFile::ServiceAccount(k) => (None, Some(k.client_email.clone())),
    };
    Ok(Login {
        tokens: issued_tokens(token_transport, &file, scope)?,
        quota_project,
        user,
    })
}

/// The client of the profile's project, signed in.
async fn client_for(
    target: &ConnectTarget<'_>,
    params: &BigqueryParams,
) -> Result<(Client, Option<String>), AppError> {
    let profile = target.profile;
    let project = project_of(profile)?;
    let login = login(profile, params, target.password).await?;
    let transport = HttpTransport::for_target(
        target,
        &params.endpoint(),
        http_options(profile)?,
        Credential::bearer(login.tokens),
    )?;
    let client = Client::new(Arc::new(transport), &params.endpoint(), &project)
        .with_quota_project(login.quota_project.as_deref());
    Ok((client, login.user))
}

/// What every job of a session carries. The MCP process (`unattended`) never runs a job without
/// a cost guard.
pub fn query_options(params: &BigqueryParams, project: &str, unattended: bool) -> QueryOptions {
    let guard = match params.maximum_bytes_billed {
        None if unattended => Some(DEFAULT_MAXIMUM_BYTES_BILLED),
        b => b,
    };
    QueryOptions {
        location: params.location().map(str::to_string),
        default_dataset: params.default_dataset().map(|d| DatasetReference {
            project_id: project.to_string(),
            dataset_id: d.to_string(),
        }),
        maximum_bytes_billed: guard,
        use_query_cache: params.use_query_cache,
        job_timeout_ms: params.job_timeout_ms,
        labels: params.labels.clone(),
        ..QueryOptions::default()
    }
}

/// Opens a session on the profile's project; a free call proves the sign-in first.
pub async fn open(target: ConnectTarget<'_>) -> Result<Session, AppError> {
    let profile = target.profile;
    validate(profile)?;
    let params = BigqueryParams::of(profile)?;
    let (client, user) = client_for(&target, &params).await?;
    client.ping().await?;
    let config = SessionConfig {
        options: query_options(&params, client.project(), unattended()),
        extra_projects: params.extra_projects.clone(),
        read_only: effective_read_only(profile),
        user,
    };
    Ok(Session::Sql(Arc::new(BigquerySession::new(
        Arc::new(client),
        config,
    ))))
}

/// The params parse and pass their checks (a key file for a service account, the cost guard,
/// labels, an endpoint the token may go to).
pub fn validate(profile: &ConnectionProfile) -> Result<(), AppError> {
    BigqueryParams::of(profile)?.validate()
}

/// The form's calls, with the secret being edited: `listDatasets` (`[{name, location}]` of the
/// profile's project) and `listProjects` (`[{id, name}]`).
pub async fn util(
    input: UtilInput<'_>,
    method: &str,
    _args: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    if util_kind(method) != CallKind::Read {
        return Err(unknown_call(DRIVER, method));
    }
    let params = BigqueryParams::of(input.profile)?;
    params.validate()?;
    let target = ConnectTarget {
        profile: input.profile,
        password: input.secret,
        host_override: None,
    };
    let (client, _) = client_for(&target, &params).await?;
    let list = if method == LIST_DATASETS {
        let datasets = client.list_datasets(client.project()).await?;
        let mut out: Vec<serde_json::Value> = datasets
            .iter()
            .filter(|d| !d.dataset_reference.dataset_id.is_empty())
            .map(|d| {
                serde_json::json!({
                    "name": d.dataset_reference.dataset_id,
                    "location": d.location,
                })
            })
            .collect();
        out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        out
    } else {
        client
            .list_projects()
            .await?
            .iter()
            .filter_map(|p| {
                Some(serde_json::json!({
                    "id": p.project_id()?,
                    "name": p.friendly_name,
                }))
            })
            .collect()
    };
    Ok(serde_json::Value::Array(list))
}

/// Whether a `util` call only reads; anything unknown changes something.
pub fn util_kind(method: &str) -> CallKind {
    match method {
        LIST_DATASETS | LIST_PROJECTS => CallKind::Read,
        _ => CallKind::Write,
    }
}
