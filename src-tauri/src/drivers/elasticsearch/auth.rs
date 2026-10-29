//! How requests authenticate: none, Basic (the `elastic` user of a new 8.x cluster), an API key
//! (`Authorization: ApiKey <base64(id:key)>`, given encoded or as id + key), a bearer token (a
//! service-account token, an OAuth or JWT realm), or AWS SigV4 for Amazon OpenSearch Service
//! (`es`) and Serverless (`aoss`). Also the Elastic Cloud ID, which names the cluster's URL.

use base64::Engine as _;
use chrono::{DateTime, Utc};

use crate::error::AppError;

use super::sigv4::{self, AwsCredentials, SigningScope};
use super::transport::{BaseUrl, HttpRequest};

#[derive(Clone, PartialEq, Eq)]
pub enum Auth {
    None,
    Basic {
        user: String,
        password: String,
    },
    /// The encoded form (`base64(id:key)`).
    ApiKey(String),
    Bearer(String),
    SigV4 {
        credentials: AwsCredentials,
        region: String,
        service: AwsService,
    },
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Auth::None => f.write_str("None"),
            Auth::Basic { user, .. } => write!(f, "Basic({user}, …)"),
            Auth::ApiKey(_) => f.write_str("ApiKey(…)"),
            Auth::Bearer(_) => f.write_str("Bearer(…)"),
            Auth::SigV4 {
                credentials,
                region,
                service,
            } => write!(f, "SigV4({credentials:?}, {region}, {service:?})"),
        }
    }
}

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
    Some(parts[i - 1].to_string())
}

impl Auth {
    /// An API key from its encoded form, or from id and key.
    pub fn api_key(id: Option<&str>, key: &str) -> Result<Auth, AppError> {
        let key = key.trim();
        if key.is_empty() {
            return Err(AppError::InvalidInput("the API key is empty".into()));
        }
        Ok(Auth::ApiKey(
            match id.map(str::trim).filter(|i| !i.is_empty()) {
                Some(id) => base64::engine::general_purpose::STANDARD.encode(format!("{id}:{key}")),
                None => key.to_string(),
            },
        ))
    }

    /// Adds the credentials to a request that is ready to send (SigV4 signs its URL, headers and
    /// body, so it comes last).
    pub fn authorize(&self, req: &mut HttpRequest, now: DateTime<Utc>) {
        match self {
            Auth::None => {}
            Auth::Basic { user, password } => {
                let token =
                    base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
                req.set_header("authorization", format!("Basic {token}"));
            }
            Auth::ApiKey(encoded) => req.set_header("authorization", format!("ApiKey {encoded}")),
            Auth::Bearer(token) => req.set_header("authorization", format!("Bearer {token}")),
            Auth::SigV4 {
                credentials,
                region,
                service,
            } => sigv4::sign(
                req,
                credentials,
                &SigningScope {
                    region: region.clone(),
                    service: service.name().to_string(),
                    content_sha256: true,
                },
                now,
            ),
        }
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
    if host.is_empty() || es.is_empty() {
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
    use crate::drivers::elasticsearch::console::Method;

    fn req() -> HttpRequest {
        HttpRequest {
            method: Method::Get,
            url: "https://h:9200/".into(),
            headers: vec![],
            body: None,
        }
    }

    #[test]
    fn header_schemes() {
        let now = Utc::now();
        let mut r = req();
        Auth::Basic {
            user: "elastic".into(),
            password: "changeme".into(),
        }
        .authorize(&mut r, now);
        assert_eq!(
            r.header("authorization"),
            Some("Basic ZWxhc3RpYzpjaGFuZ2VtZQ==")
        );
        let mut r = req();
        Auth::api_key(Some("VuaCfGcBCdbkQm-e5aOx"), "ui2lp2axTNmsyakw9tvNnw")
            .unwrap()
            .authorize(&mut r, now);
        assert_eq!(
            r.header("authorization"),
            Some("ApiKey VnVhQ2ZHY0JDZGJrUW0tZTVhT3g6dWkybHAyYXhUTm1zeWFrdzl0dk5udw==")
        );
        let mut r = req();
        Auth::api_key(None, " ZW5jb2RlZA== ")
            .unwrap()
            .authorize(&mut r, now);
        assert_eq!(r.header("authorization"), Some("ApiKey ZW5jb2RlZA=="));
        assert!(Auth::api_key(Some("id"), "  ").is_err());
        let mut r = req();
        Auth::Bearer("t0k".into()).authorize(&mut r, now);
        assert_eq!(r.header("authorization"), Some("Bearer t0k"));
        let mut r = req();
        Auth::None.authorize(&mut r, now);
        assert!(r.headers.is_empty());
        let dbg = format!(
            "{:?}",
            Auth::Basic {
                user: "u".into(),
                password: "secret".into()
            }
        );
        assert!(!dbg.contains("secret"));
    }

    #[test]
    fn sigv4_names_the_service() {
        let mut r = req();
        r.url = "https://abc.us-east-1.aoss.amazonaws.com/_search".into();
        Auth::SigV4 {
            credentials: AwsCredentials {
                access_key_id: "AKID".into(),
                secret_access_key: "s".into(),
                session_token: None,
            },
            region: "us-east-1".into(),
            service: AwsService::for_host("abc.us-east-1.aoss.amazonaws.com"),
        }
        .authorize(&mut r, Utc::now());
        let a = r.header("authorization").unwrap();
        assert!(a.contains("/us-east-1/aoss/aws4_request"), "{a}");
        assert!(r.header("x-amz-content-sha256").is_some());
        assert_eq!(
            region_for_host("search-logs-abc123.eu-west-1.es.amazonaws.com").as_deref(),
            Some("eu-west-1")
        );
        assert_eq!(
            region_for_host("x.us-east-2.aoss.amazonaws.com").as_deref(),
            Some("us-east-2")
        );
        assert_eq!(region_for_host("localhost"), None);
        assert_eq!(
            AwsService::for_host("search-x.es.amazonaws.com"),
            AwsService::Es
        );
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
        for bad in ["", "name:!!!", "name:aG9zdA==", "x:JGVz"] {
            assert!(cloud_id_url(bad).is_err(), "{bad}");
        }
    }
}
