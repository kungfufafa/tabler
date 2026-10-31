//! How requests authenticate, as the shared HTTP client's [`Credential`]: none, Basic (the
//! `elastic` user of a new 8.x cluster), an API key (`Authorization: ApiKey <base64(id:key)>`,
//! given encoded or as id + key), a bearer token (a service-account token, an OAuth or JWT realm),
//! or AWS SigV4 for Amazon OpenSearch Service (`es`) through `common::aws` (Serverless, `aoss`,
//! is refused at connect). Secrets come from the profile: stored, or printed by its Command
//! Line mode (run again once the cluster refuses them). Also the Elastic Cloud ID, which names
//! the cluster's URL.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine as _;

use crate::drivers::common::aws::credentials::Fetch;
use crate::drivers::common::aws::{CredentialProvider, Credentials, Signer};
use crate::drivers::common::http::Credential;
use crate::drivers::common::token::{FetchToken, IssuedToken, TokenSource};
use crate::error::AppError;
use crate::secrets::CommandPassword;
use crate::types::driver::ExtraSecret;
use crate::types::profile::{ConnectionProfile, Driver, PasswordMode};

use super::params::{ElasticAuth, ElasticAwsCredentials, ElasticParams};
use super::transport::BaseUrl;

/// The AWS service a SigV4 signature names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwsService {
    /// Amazon OpenSearch Service (managed clusters).
    Es,
    /// OpenSearch Serverless collections.
    Aoss,
}

impl AwsService {
    pub fn name(self) -> &'static str {
        match self {
            AwsService::Es => "es",
            AwsService::Aoss => "aoss",
        }
    }

    /// The service of an endpoint host (`*.aoss.amazonaws.com` is Serverless).
    pub fn for_host(host: &str) -> AwsService {
        if host.contains(".aoss.") {
            AwsService::Aoss
        } else {
            AwsService::Es
        }
    }
}

/// The region of an Amazon OpenSearch endpoint host
/// (`search-x-abc.eu-west-1.es.amazonaws.com`, `id.us-east-1.aoss.amazonaws.com`).
pub fn region_for_host(host: &str) -> Option<String> {
    let parts: Vec<&str> = host.split('.').collect();
    let i = parts
        .iter()
        .position(|p| *p == "es" || *p == "aoss")
        .filter(|&i| i >= 1 && parts.get(i + 1) == Some(&"amazonaws"))?;
    parts.get(i - 1).map(|r| r.to_string())
}

/// The region SigV4 signs for at `host`: the params', else the host's.
pub fn region(params: &ElasticParams, host: &str) -> Result<String, AppError> {
    match params.aws_region() {
        Some(r) => Ok(r.to_string()),
        None => region_for_host(host).ok_or_else(|| {
            AppError::InvalidInput(
                "Elasticsearch: AWS SigV4 needs the region (the endpoint's host name does not \
                 tell it)"
                    .into(),
            )
        }),
    }
}

/// How long a Command Line secret is reused (the descriptor's `command_ttl_secs`).
fn command_ttl() -> Duration {
    Duration::from_secs(u64::from(
        Driver::Elasticsearch.descriptor().command_ttl_secs,
    ))
}

/// The command of a Command Line secret, seeded with what it printed for this connect.
fn command(profile: &ConnectionProfile, secret: Option<&str>) -> Option<Arc<CommandPassword>> {
    CommandPassword::for_profile_ttl(profile, secret, command_ttl())
}

/// Whether the profile has a secret to send now or a command that prints one.
fn has_secret(profile: &ConnectionProfile, secret: Option<&str>) -> bool {
    profile.options.password_mode() == PasswordMode::Command
        || secret.is_some_and(|s| !s.trim().is_empty())
}

/// `base64(id:key)`, the encoded form of an API key.
pub fn encode_api_key(id: &str, key: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", id.trim(), key.trim()))
}

/// The credential of a session on `base`: what the params name, from the profile's secret.
/// Called inside the connect, so the session token secret can be read.
pub async fn credential(
    profile: &ConnectionProfile,
    params: &ElasticParams,
    secret: Option<&str>,
    base: &BaseUrl,
) -> Result<Credential, AppError> {
    let missing = |what: &str| {
        AppError::InvalidInput(format!(
            "Elasticsearch: enter the {what} in the password field"
        ))
    };
    Ok(match params.auth {
        ElasticAuth::None => Credential::None,
        ElasticAuth::Basic => Credential::Basic(
            profile
                .user
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_string(),
            TokenSource::for_profile(profile, secret),
        ),
        ElasticAuth::Bearer => {
            if !has_secret(profile, secret) {
                return Err(missing("bearer token"));
            }
            Credential::bearer(TokenSource::for_profile(profile, secret))
        }
        ElasticAuth::ApiKey => {
            if !has_secret(profile, secret) {
                return Err(missing("API key"));
            }
            Credential::Scheme("ApiKey", api_key(profile, params.api_key_id(), secret))
        }
        ElasticAuth::AwsSigv4 => {
            let service = params
                .aws_service()
                .unwrap_or_else(|| AwsService::for_host(&base.host));
            let region = region(params, &base.host)?;
            let provider = aws_provider(profile, params, secret).await?;
            Credential::Sigv4(Signer::new(service.name(), &region, Arc::new(provider)))
        }
    })
}

/// An API key as sent: the secret as it is (the encoded form), or encoded with `id`. A Command
/// Line key is encoded each time the command prints it.
fn api_key(profile: &ConnectionProfile, id: Option<&str>, secret: Option<&str>) -> TokenSource {
    let Some(id) = id.map(str::to_string) else {
        return TokenSource::for_profile(profile, secret);
    };
    match command(profile, secret) {
        None => TokenSource::from_secret(
            secret
                .filter(|s| !s.trim().is_empty())
                .map(|key| encode_api_key(&id, key))
                .as_deref(),
        ),
        Some(command) => {
            // The connect's own output first; after that the command runs again (the cluster
            // refused the key, or its time is up).
            let seeded = Arc::new(AtomicBool::new(true));
            let ttl = command_ttl();
            let fetch: FetchToken = Arc::new(move || {
                let (command, id, seeded) = (command.clone(), id.clone(), seeded.clone());
                Box::pin(async move {
                    if !seeded.swap(false, Ordering::SeqCst) {
                        command.invalidate().await;
                    }
                    let key = command.password().await?;
                    Ok(IssuedToken {
                        token: encode_api_key(&id, &key),
                        expires_in: Some(ttl),
                    })
                })
            });
            TokenSource::issued(fetch)
        }
    }
}

/// The AWS credentials a signer uses.
async fn aws_provider(
    profile: &ConnectionProfile,
    params: &ElasticParams,
    secret: Option<&str>,
) -> Result<CredentialProvider, AppError> {
    match params.aws_credentials {
        ElasticAwsCredentials::Profile => {
            CredentialProvider::from_profile(params.aws_profile()).await
        }
        ElasticAwsCredentials::Environment => {
            Ok(CredentialProvider::fixed(Credentials::from_env()?))
        }
        ElasticAwsCredentials::AccessKey => {
            let key_id = profile.user.as_deref().unwrap_or_default().to_string();
            let token = crate::secrets::extra_secret(ExtraSecret::SessionToken);
            match command(profile, secret) {
                None => Ok(CredentialProvider::fixed(Credentials::static_keys(
                    &key_id,
                    secret.unwrap_or_default(),
                    token.as_deref(),
                )?)),
                Some(command) => {
                    let first =
                        command_credentials(&key_id, secret.unwrap_or_default(), token.as_deref())?;
                    let fetch: Fetch = Arc::new(move || {
                        let (command, key_id, token) =
                            (command.clone(), key_id.clone(), token.clone());
                        Box::pin(async move {
                            // Fetched only once the first credentials expire or are refused.
                            command.invalidate().await;
                            let out = command.password().await?;
                            command_credentials(&key_id, &out, token.as_deref())
                        })
                    });
                    Ok(CredentialProvider::fetching(fetch, Some(first)))
                }
            }
        }
    }
}

/// What a Command Line secret printed: `credential_process` JSON, or the secret access key of
/// the profile's access key id.
fn command_credentials(
    key_id: &str,
    out: &str,
    session_token: Option<&str>,
) -> Result<Credentials, AppError> {
    if out.trim_start().starts_with('{') {
        Credentials::from_process_json(out)
    } else {
        Credentials::static_keys(key_id, out, session_token)
    }
}

/// An Elastic Cloud ID (`name:base64(host[:port]$es-uuid[:port]$kibana-uuid)`) as the cluster's
/// URL, `https://<es-uuid>.<host>:<port>` (443 unless the ID names a port).
pub fn cloud_id_url(cloud_id: &str) -> Result<BaseUrl, AppError> {
    let bad = || AppError::InvalidInput("the Cloud ID is not valid".into());
    let encoded = match cloud_id.trim().split_once(':') {
        Some((_, rest)) => rest,
        None => cloud_id.trim(),
    };
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(encoded))
        .map_err(|_| bad())?;
    let decoded = String::from_utf8(decoded).map_err(|_| bad())?;
    let mut parts = decoded.split('$');
    let (host, es) = (parts.next().ok_or_else(bad)?, parts.next().ok_or_else(bad)?);
    let (host, host_port) = match host.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (host, None),
    };
    let (es, es_port) = match es.rsplit_once(':') {
        Some((e, p)) => (e, Some(p)),
        None => (es, None),
    };
    let label_ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    };
    if !label_ok(host) || !label_ok(es) {
        return Err(bad());
    }
    let port = match es_port.or(host_port) {
        Some(p) => p.parse().map_err(|_| bad())?,
        None => 443,
    };
    Ok(BaseUrl {
        https: true,
        host: format!("{es}.{host}"),
        port,
        prefix: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_and_services_from_hosts() {
        assert_eq!(
            region_for_host("search-logs-abc123.eu-west-1.es.amazonaws.com").as_deref(),
            Some("eu-west-1")
        );
        assert_eq!(
            region_for_host("x.us-east-2.aoss.amazonaws.com").as_deref(),
            Some("us-east-2")
        );
        assert_eq!(region_for_host("localhost"), None);
        assert_eq!(region_for_host("es.amazonaws.com"), None);
        assert_eq!(
            AwsService::for_host("search-x.es.amazonaws.com"),
            AwsService::Es
        );
        assert_eq!(
            AwsService::for_host("abc.us-east-1.aoss.amazonaws.com"),
            AwsService::Aoss
        );
        assert_eq!(
            encode_api_key("VuaCfGcBCdbkQm-e5aOx", "ui2lp2axTNmsyakw9tvNnw"),
            "VnVhQ2ZHY0JDZGJrUW0tZTVhT3g6dWkybHAyYXhUTm1zeWFrdzl0dk5udw=="
        );
    }

    #[test]
    fn command_output_is_process_json_or_the_secret() {
        let c = command_credentials("AKID", "  s3cret \n", Some("tok")).unwrap();
        assert_eq!(
            (c.access_key_id.as_str(), c.secret_access_key.as_str()),
            ("AKID", "s3cret")
        );
        assert_eq!(c.session_token.as_deref(), Some("tok"));
        let c = command_credentials(
            "ignored",
            r#"{"Version":1,"AccessKeyId":"AK2","SecretAccessKey":"x","SessionToken":"t2"}"#,
            None,
        )
        .unwrap();
        assert_eq!(c.access_key_id, "AK2");
        assert_eq!(c.session_token.as_deref(), Some("t2"));
        assert!(command_credentials("", "secret", None).is_err());
    }

    #[test]
    fn cloud_ids() {
        // base64("us-central1.gcp.cloud.es.io$abc123$def456")
        let id = format!(
            "my-deployment:{}",
            base64::engine::general_purpose::STANDARD
                .encode("us-central1.gcp.cloud.es.io$abc123$def456")
        );
        let b = cloud_id_url(&id).unwrap();
        assert_eq!(b.host, "abc123.us-central1.gcp.cloud.es.io");
        assert_eq!((b.https, b.port), (true, 443));
        let with_port =
            base64::engine::general_purpose::STANDARD.encode("eu-west-1.aws.found.io:9243$es1$kb1");
        assert_eq!(cloud_id_url(&format!("x:{with_port}")).unwrap().port, 9243);
        let es_port = base64::engine::general_purpose::STANDARD.encode("host.io:9243$es1:9244$kb1");
        assert_eq!(cloud_id_url(&es_port).unwrap().port, 9244);
        // A decoded host must be a host name: nothing that would change the URL.
        let slash = base64::engine::general_purpose::STANDARD.encode("evil.io/x$es1$kb1");
        let at = base64::engine::general_purpose::STANDARD.encode("evil.io$a@b$kb1");
        for bad in ["", "name:!!!", "name:aG9zdA==", "x:JGVz", &slash, &at] {
            assert!(cloud_id_url(bad).is_err(), "{bad}");
        }
    }
}
