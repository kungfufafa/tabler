//! `ElasticParams`: the Elasticsearch settings of a profile beyond the common fields, read from
//! `options.driver_params`. The common fields keep their meaning: the host and port (or the
//! `urls` / `cloudId` params) name the cluster, the user and password are the Basic credentials
//! (the access key id and secret access key for SigV4), the SSL mode says HTTP or HTTPS and how
//! the certificate is checked, the CA file and the client certificate are the profile's own.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::AppError;
use crate::types::profile::DriverParams;

use super::auth::AwsService;

/// How requests prove who sends them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ElasticAuth {
    /// No credentials (a cluster without security).
    None,
    /// The user and password (`elastic` on a new 8.x cluster); nothing is sent while both are
    /// empty.
    #[default]
    Basic,
    /// The password is an API key: the encoded form, or the key of `apiKeyId`.
    ApiKey,
    /// The password is a bearer token (a service-account token, an OAuth or JWT realm).
    Bearer,
    /// AWS Signature Version 4 (Amazon OpenSearch Service, `es`).
    AwsSigv4,
}

/// Where SigV4 credentials come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ElasticAwsCredentials {
    /// The user is the access key id, the password the secret access key, with the optional
    /// session token secret. In Command mode the command prints `credential_process` JSON or
    /// the secret access key.
    #[default]
    AccessKey,
    /// A named profile of `~/.aws/credentials` and `~/.aws/config` (static keys or
    /// `credential_process`).
    Profile,
    /// `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and `AWS_SESSION_TOKEN` of Tabler's
    /// environment.
    Environment,
}

/// The AWS service a signature names (`awsService`; unset: read from the endpoint host). OpenSearch
/// Serverless (`aoss`) is read so that the connect can refuse it by name: not supported yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ElasticAwsService {
    Es,
    Aoss,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ElasticParams {
    /// Node URLs (`https://es1:9200`), tried in order at connect; empty: the host and port.
    pub urls: Vec<String>,
    /// An Elastic Cloud ID in place of the host or URLs.
    #[ts(optional = nullable)]
    pub cloud_id: Option<String>,
    /// The path the cluster is served under behind a reverse proxy (`/es`).
    #[ts(optional = nullable)]
    pub path_prefix: Option<String>,
    pub auth: ElasticAuth,
    /// The id of an API key given as id + key (the password is then the key).
    #[ts(optional = nullable)]
    pub api_key_id: Option<String>,
    pub aws_credentials: ElasticAwsCredentials,
    /// The profile of `Profile` (empty: `default`).
    #[ts(optional = nullable)]
    pub aws_profile: Option<String>,
    /// The region SigV4 signs for; empty: read from the endpoint host.
    #[ts(optional = nullable)]
    pub aws_region: Option<String>,
    #[ts(optional = nullable)]
    pub aws_service: Option<ElasticAwsService>,
    /// SHA-256 of the cluster's CA certificate (`ca_trusted_fingerprint`), checked whatever the
    /// SSL mode.
    #[ts(optional = nullable)]
    pub ca_fingerprint: Option<String>,
    /// Lists hidden and dot-prefixed indices (`expand_wildcards=all`).
    pub show_system_indices: bool,
    /// Ends a request that takes longer, in seconds; unset: only the query time out does.
    #[ts(optional = nullable)]
    pub request_timeout_secs: Option<u32>,
}

/// Longest request time out a profile may set (a day).
const MAX_REQUEST_TIMEOUT_SECS: u32 = 86_400;

fn non_empty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

impl ElasticParams {
    /// The params of a profile (none: the defaults), checked.
    pub fn from_driver_params(params: Option<&DriverParams>) -> Result<ElasticParams, AppError> {
        let parsed: ElasticParams = match params {
            Some(DriverParams(map)) => {
                serde_json::from_value(serde_json::Value::Object(map.clone()))
                    .map_err(|e| AppError::InvalidInput(format!("Elasticsearch settings: {e}")))?
            }
            None => ElasticParams::default(),
        };
        parsed.check()?;
        Ok(parsed)
    }

    pub fn check(&self) -> Result<(), AppError> {
        if self.cloud_id().is_some() && !self.urls().is_empty() {
            return Err(AppError::InvalidInput(
                "Elasticsearch: give either a Cloud ID or URLs, not both".into(),
            ));
        }
        if let Some(prefix) = self.path_prefix() {
            check_prefix(prefix)?;
        }
        if let Some(fp) = non_empty(&self.ca_fingerprint) {
            crate::tls_client::parse_pin(fp)?;
        }
        if let Some(region) = self.aws_region() {
            check_region(region)?;
        }
        if self
            .request_timeout_secs
            .is_some_and(|s| s == 0 || s > MAX_REQUEST_TIMEOUT_SECS)
        {
            return Err(AppError::InvalidInput(format!(
                "Elasticsearch: the request time out is 1 to {MAX_REQUEST_TIMEOUT_SECS} seconds"
            )));
        }
        Ok(())
    }

    /// The node URLs given, trimmed, without empty ones.
    pub fn urls(&self) -> Vec<&str> {
        self.urls
            .iter()
            .map(|u| u.trim())
            .filter(|u| !u.is_empty())
            .collect()
    }

    pub fn cloud_id(&self) -> Option<&str> {
        non_empty(&self.cloud_id)
    }

    /// The path prefix without a trailing `/` (`None` when empty or `/`).
    pub fn path_prefix(&self) -> Option<&str> {
        non_empty(&self.path_prefix)
            .map(|p| p.trim_end_matches('/'))
            .filter(|p| !p.is_empty())
    }

    pub fn api_key_id(&self) -> Option<&str> {
        non_empty(&self.api_key_id)
    }

    /// The profile name of `Profile` (`default` when empty).
    pub fn aws_profile(&self) -> &str {
        non_empty(&self.aws_profile).unwrap_or("default")
    }

    pub fn aws_region(&self) -> Option<&str> {
        non_empty(&self.aws_region)
    }

    pub fn aws_service(&self) -> Option<AwsService> {
        self.aws_service.map(|s| match s {
            ElasticAwsService::Es => AwsService::Es,
            ElasticAwsService::Aoss => AwsService::Aoss,
        })
    }

    /// The certificate pin of `caFingerprint`.
    pub fn pin(&self) -> Result<Option<[u8; 32]>, AppError> {
        non_empty(&self.ca_fingerprint)
            .map(crate::tls_client::parse_pin)
            .transpose()
    }
}

/// A path prefix (`pathPrefix`, or the path of a node URL or the host): `/`-separated segments
/// without `.`, `..`, a query, a fragment or a backslash (it goes into every URL as written, so a
/// SigV4 signature covers it as sent), and none starting with `_`: an endpoint name there would
/// turn every request, the connect's `GET /` too, into a call of that endpoint (`/_refresh/`).
pub(super) fn check_prefix(prefix: &str) -> Result<(), AppError> {
    let bad = |why: &str| {
        AppError::InvalidInput(format!(
            "Elasticsearch: the path prefix \u{201c}{prefix}\u{201d} {why}"
        ))
    };
    if !prefix.starts_with('/') {
        return Err(bad("must start with /"));
    }
    if prefix
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '?' | '#' | '\\' | '%'))
    {
        return Err(bad("may hold no spaces, ?, #, % or \\"));
    }
    if prefix
        .split('/')
        .any(|s| s == "." || s == ".." || s.starts_with('_'))
    {
        return Err(bad("may hold no . or .. segment, nor one starting with _"));
    }
    Ok(())
}

/// A region name as AWS spells them (`us-east-1`, `cn-north-1`, `us-gov-west-1`).
pub fn check_region(region: &str) -> Result<&str, AppError> {
    let r = region.trim();
    let ok = !r.is_empty()
        && r.len() <= 64
        && r.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !r.starts_with('-')
        && !r.ends_with('-');
    if ok {
        Ok(r)
    } else {
        Err(AppError::InvalidInput(format!(
            "Elasticsearch: \u{201c}{region}\u{201d} is not an AWS region name (like us-east-1)"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(json: serde_json::Value) -> Result<ElasticParams, AppError> {
        let serde_json::Value::Object(map) = json else {
            panic!("an object")
        };
        ElasticParams::from_driver_params(Some(&DriverParams(map)))
    }

    #[test]
    fn defaults_and_camel_case() {
        let p = ElasticParams::from_driver_params(None).unwrap();
        assert_eq!(p.auth, ElasticAuth::Basic);
        assert!(p.urls().is_empty() && p.cloud_id().is_none() && !p.show_system_indices);
        let p = params(serde_json::json!({
            "urls": [" https://a:9200 ", ""],
            "pathPrefix": "/es/",
            "auth": "awsSigv4",
            "awsCredentials": "profile",
            "awsRegion": "eu-west-1",
            "awsService": "aoss",
            "caFingerprint": "AA:".repeat(31) + "AA",
            "showSystemIndices": true,
            "requestTimeoutSecs": 30
        }))
        .unwrap();
        assert_eq!(p.urls(), ["https://a:9200"]);
        assert_eq!(p.path_prefix(), Some("/es"));
        assert_eq!(p.auth, ElasticAuth::AwsSigv4);
        assert_eq!(p.aws_credentials, ElasticAwsCredentials::Profile);
        assert_eq!(p.aws_profile(), "default");
        assert_eq!(p.aws_service(), Some(AwsService::Aoss));
        assert_eq!(p.pin().unwrap(), Some([0xAA; 32]));
        assert_eq!(p.request_timeout_secs, Some(30));
    }

    #[test]
    fn refusals() {
        for bad in [
            serde_json::json!({"auth": "kerberos"}),
            serde_json::json!({"urls": ["http://a"], "cloudId": "x:eA=="}),
            serde_json::json!({"pathPrefix": "es"}),
            serde_json::json!({"pathPrefix": "/a/../_bulk"}),
            serde_json::json!({"pathPrefix": "/_cluster"}),
            serde_json::json!({"pathPrefix": "/a?b"}),
            serde_json::json!({"caFingerprint": "abc"}),
            serde_json::json!({"awsRegion": "Not A Region"}),
            serde_json::json!({"requestTimeoutSecs": 0}),
        ] {
            assert!(params(bad.clone()).is_err(), "{bad}");
        }
    }
}
