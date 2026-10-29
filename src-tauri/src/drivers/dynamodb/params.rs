//! `DynamoParams`: the DynamoDB settings of a profile beyond the common fields (the access key id
//! is the user, the region the database, the secret access key the password), read from
//! `options.driver_params`, and the endpoint a region or an override resolves to.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::AppError;
use crate::types::profile::DriverParams;

/// Where the credentials come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DynamoAuth {
    /// The access key id (the profile's user) and secret access key (its password), with the
    /// optional session token secret.
    #[default]
    AccessKey,
    /// A named profile of `~/.aws/credentials` and `~/.aws/config` (static keys or
    /// `credential_process`).
    Profile,
    /// `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and `AWS_SESSION_TOKEN` of Tabler's
    /// environment.
    Environment,
    /// The password's Command mode prints `credential_process` JSON (or the secret access key).
    Command,
    /// DynamoDB Local or another emulator: dummy keys, and the endpoint is required.
    Local,
}

/// Grid pages read at most this many items per request by default.
pub const DEFAULT_SAMPLE_SIZE: u32 = 100;
/// Requests one grid page may take (a filter can leave pages of evaluated items empty).
pub const DEFAULT_PAGE_REQUESTS: u32 = 5;
/// DynamoDB Local's own default endpoint.
pub const LOCAL_ENDPOINT: &str = "http://localhost:8000/";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct DynamoParams {
    pub auth: DynamoAuth,
    /// The profile of `Profile` (empty: `default`).
    #[ts(optional = nullable)]
    pub profile_name: Option<String>,
    /// Another endpoint than `https://dynamodb.<region>.amazonaws.com`: DynamoDB Local
    /// (`http://localhost:8000`), LocalStack, a VPC or FIPS endpoint, ScyllaDB Alternator.
    #[ts(optional = nullable)]
    pub endpoint: Option<String>,
    /// Strongly consistent Scan, Query and GetItem (twice the read capacity).
    pub consistent_reads: bool,
    /// Items read to infer a table's columns.
    pub sample_size: u32,
    /// Requests a grid page may take before it stops with a cursor.
    pub page_requests: u32,
    /// Ask for the capacity each request consumed (`ReturnConsumedCapacity: TOTAL`).
    pub consumed_capacity: bool,
}

impl Default for DynamoParams {
    fn default() -> DynamoParams {
        DynamoParams {
            auth: DynamoAuth::AccessKey,
            profile_name: None,
            endpoint: None,
            consistent_reads: false,
            sample_size: DEFAULT_SAMPLE_SIZE,
            page_requests: DEFAULT_PAGE_REQUESTS,
            consumed_capacity: false,
        }
    }
}

impl DynamoParams {
    /// The params of a profile (none: the defaults), checked.
    pub fn from_driver_params(params: Option<&DriverParams>) -> Result<DynamoParams, AppError> {
        let parsed: DynamoParams = match params {
            Some(DriverParams(map)) => {
                serde_json::from_value(serde_json::Value::Object(map.clone()))
                    .map_err(|e| AppError::InvalidInput(format!("DynamoDB settings: {e}")))?
            }
            None => DynamoParams::default(),
        };
        parsed.check()?;
        Ok(parsed)
    }

    pub fn check(&self) -> Result<(), AppError> {
        if !(1..=1000).contains(&self.sample_size) {
            return Err(AppError::InvalidInput(
                "DynamoDB: the sample size is 1 to 1000 items".into(),
            ));
        }
        if !(1..=50).contains(&self.page_requests) {
            return Err(AppError::InvalidInput(
                "DynamoDB: a grid page takes 1 to 50 requests".into(),
            ));
        }
        if let Some(e) = self.endpoint.as_deref().filter(|e| !e.trim().is_empty()) {
            normalize_endpoint(e)?;
        }
        Ok(())
    }

    /// The profile name of `Profile` (`default` when empty).
    pub fn profile(&self) -> &str {
        self.profile_name
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .unwrap_or("default")
    }

    /// The endpoint requests go to: the override, DynamoDB Local's default for `Local`, else
    /// the region's.
    pub fn endpoint_url(&self, region: &str) -> Result<String, AppError> {
        match self.endpoint.as_deref().filter(|e| !e.trim().is_empty()) {
            Some(e) => normalize_endpoint(e),
            None if self.auth == DynamoAuth::Local => Ok(LOCAL_ENDPOINT.into()),
            None => region_endpoint(region),
        }
    }
}

/// A region name as AWS spells them (`us-east-1`, `cn-north-1`, `us-gov-west-1`); emulators take
/// any such name too.
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
            "DynamoDB: \u{201c}{region}\u{201d} is not a region name (like us-east-1)"
        )))
    }
}

/// `https://dynamodb.<region>.amazonaws.com/` (`.com.cn` in the China regions).
pub fn region_endpoint(region: &str) -> Result<String, AppError> {
    let r = check_region(region)?;
    let suffix = if r.starts_with("cn-") {
        "amazonaws.com.cn"
    } else {
        "amazonaws.com"
    };
    Ok(format!("https://dynamodb.{r}.{suffix}/"))
}

/// An endpoint override as `scheme://host[:port]/`: http or https, a host, no credentials,
/// query or fragment; a path is kept.
pub fn normalize_endpoint(endpoint: &str) -> Result<String, AppError> {
    let text = endpoint.trim();
    let text = if text.contains("://") {
        text.to_string()
    } else {
        format!("https://{text}")
    };
    let bad = |why: &str| {
        AppError::InvalidInput(format!(
            "DynamoDB: the endpoint \u{201c}{endpoint}\u{201d} {why}"
        ))
    };
    let url = reqwest::Url::parse(&text).map_err(|_| bad("is not a URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(bad("must start with http:// or https://"));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(bad("has no host"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(bad("must not hold credentials"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(bad("must not have a query or fragment"));
    }
    let mut out = url.to_string();
    if !out.ends_with('/') {
        out.push('/');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(json: serde_json::Value) -> Result<DynamoParams, AppError> {
        let serde_json::Value::Object(map) = json else {
            panic!("an object")
        };
        DynamoParams::from_driver_params(Some(&DriverParams(map)))
    }

    #[test]
    fn defaults_and_reading() {
        let d = DynamoParams::from_driver_params(None).unwrap();
        assert_eq!(d, DynamoParams::default());
        assert_eq!(d.profile(), "default");
        let p = params(serde_json::json!({
            "auth": "profile", "profileName": " dev ", "consistentReads": true, "sampleSize": 20
        }))
        .unwrap();
        assert_eq!(p.auth, DynamoAuth::Profile);
        assert_eq!(p.profile(), "dev");
        assert_eq!(p.sample_size, 20);
        assert_eq!(p.page_requests, DEFAULT_PAGE_REQUESTS);
        assert!(params(serde_json::json!({"auth": "magic"})).is_err());
        assert!(params(serde_json::json!({"sampleSize": 0})).is_err());
        assert!(params(serde_json::json!({"pageRequests": 51})).is_err());
        assert!(params(serde_json::json!({"endpoint": "ftp://x"})).is_err());
    }

    #[test]
    fn endpoints() {
        let d = DynamoParams::default();
        assert_eq!(
            d.endpoint_url("us-east-1").unwrap(),
            "https://dynamodb.us-east-1.amazonaws.com/"
        );
        assert_eq!(
            d.endpoint_url("cn-north-1").unwrap(),
            "https://dynamodb.cn-north-1.amazonaws.com.cn/"
        );
        assert!(d.endpoint_url("US East").is_err());
        assert!(d.endpoint_url("evil.com/x").is_err());
        let local = DynamoParams {
            auth: DynamoAuth::Local,
            ..DynamoParams::default()
        };
        assert_eq!(local.endpoint_url("x").unwrap(), LOCAL_ENDPOINT);
        let custom = DynamoParams {
            endpoint: Some(" localhost:4566 ".into()),
            ..DynamoParams::default()
        };
        assert_eq!(
            custom.endpoint_url("us-east-1").unwrap(),
            "https://localhost:4566/"
        );
        assert_eq!(
            normalize_endpoint("http://127.0.0.1:8000").unwrap(),
            "http://127.0.0.1:8000/"
        );
        assert_eq!(
            normalize_endpoint("https://vpce.example/ddb").unwrap(),
            "https://vpce.example/ddb/"
        );
        for bad in [
            "http://u:p@h/",
            "http://h/?a=1",
            "http://h/#f",
            "file:///x",
            "http://",
        ] {
            assert!(normalize_endpoint(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn region_names() {
        for ok in ["us-east-1", "us-gov-west-1", "local", "ap-southeast-3"] {
            assert!(check_region(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-x", "x-", "US-EAST-1", "a.b", "a/b"] {
            assert!(check_region(bad).is_err(), "{bad}");
        }
    }
}
