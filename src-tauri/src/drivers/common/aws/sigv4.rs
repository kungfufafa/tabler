//! AWS Signature Version 4 of reqwest requests (DynamoDB, OpenSearch Service) on aws-lc-rs. What
//! is signed: the method; the path as sent with every byte outside the unreserved set encoded
//! once more (as AWS does for every service but S3); the query, decoded, encoded again and
//! sorted; the headers the request has when it is signed (Host from the Host header or the URL,
//! `X-Amz-Date`, `X-Amz-Security-Token` with temporary credentials), names lower-cased and values
//! trimmed; and the SHA-256 of the body, which must not be a stream.

use std::sync::Arc;

use aws_lc_rs::{digest, hmac};
use chrono::{DateTime, Utc};
use reqwest::header::{AUTHORIZATION, HOST, HeaderValue};

use super::credentials::{CredentialProvider, Credentials};
use crate::drivers::common::http::authority;
use crate::error::AppError;

/// Headers that are not signed: the transport may add or change them.
const UNSIGNED: &[&str] = &[
    "authorization",
    "connection",
    "expect",
    "transfer-encoding",
    "user-agent",
    "x-amzn-trace-id",
];

/// Signs requests for one service in one region.
pub struct Signer {
    service: String,
    region: String,
    creds: Arc<CredentialProvider>,
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signer")
            .field("service", &self.service)
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

/// The parts of one signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub canonical_request: String,
    pub string_to_sign: String,
    /// Hex.
    pub signature: String,
    /// The `Authorization` header.
    pub authorization: String,
}

impl Signer {
    /// `service` as AWS names it in the scope (`dynamodb`, `es`, `aoss`).
    pub fn new(service: &str, region: &str, creds: Arc<CredentialProvider>) -> Signer {
        Signer {
            service: service.to_string(),
            region: region.to_string(),
            creds,
        }
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    /// Signs `req` now with the provider's credentials.
    pub async fn sign(&self, req: &mut reqwest::Request) -> Result<(), AppError> {
        let now = Utc::now();
        let creds = self.creds.get(now).await?;
        self.sign_with(req, &creds, now).map(|_| ())
    }

    /// Signs `req` as of `at`: sets `X-Amz-Date`, `X-Amz-Security-Token` (with a session token)
    /// and `Authorization`.
    pub fn sign_with(
        &self,
        req: &mut reqwest::Request,
        creds: &Credentials,
        at: DateTime<Utc>,
    ) -> Result<Signature, AppError> {
        let header = |v: &str| {
            HeaderValue::from_str(v).map_err(|_| {
                AppError::InvalidInput("AWS credentials hold characters a header cannot".into())
            })
        };
        let headers = req.headers_mut();
        headers.remove(AUTHORIZATION);
        headers.insert("x-amz-date", header(&amz_date(at))?);
        match &creds.session_token {
            Some(token) => {
                let mut value = header(token)?;
                value.set_sensitive(true);
                headers.insert("x-amz-security-token", value);
            }
            None => {
                headers.remove("x-amz-security-token");
            }
        }
        let payload: &[u8] = match req.body() {
            None => b"",
            Some(body) => body.as_bytes().ok_or_else(|| {
                AppError::InvalidInput("a SigV4 request needs its whole body, not a stream".into())
            })?,
        };
        let mut signed: Vec<(String, String)> = Vec::new();
        if !req.headers().contains_key(HOST) {
            signed.push(("host".into(), authority(req.url())));
        }
        for (name, value) in req.headers() {
            let name = name.as_str();
            if UNSIGNED.contains(&name) {
                continue;
            }
            let value = value.to_str().map_err(|_| {
                AppError::InvalidInput(format!(
                    "the {name} header is not text and cannot be signed"
                ))
            })?;
            signed.push((name.to_string(), value.to_string()));
        }
        let signature = sign_parts(
            req.method().as_str(),
            req.url().path(),
            req.url().query().unwrap_or(""),
            &signed,
            payload,
            creds,
            at,
            &self.region,
            &self.service,
        );
        let mut value = header(&signature.authorization)?;
        value.set_sensitive(true);
        req.headers_mut().insert(AUTHORIZATION, value);
        Ok(signature)
    }

    /// Whether credentials the server refused can be fetched again.
    pub fn refreshes(&self) -> bool {
        self.creds.refreshable()
    }

    pub async fn invalidate(&self) {
        self.creds.invalidate().await;
    }
}

/// The signature of a request given by its parts: `path` and `query` as sent, `headers` with
/// `host`, in any order and case.
#[allow(clippy::too_many_arguments)]
pub fn sign_parts(
    method: &str,
    path: &str,
    query: &str,
    headers: &[(String, String)],
    payload: &[u8],
    creds: &Credentials,
    at: DateTime<Utc>,
    region: &str,
    service: &str,
) -> Signature {
    let (canonical_headers, signed_headers) = canonical_headers(headers);
    let canonical_request = format!(
        "{method}\n{}\n{}\n{canonical_headers}\n{signed_headers}\n{}",
        canonical_uri(path),
        canonical_query(query),
        sha256_hex(payload)
    );
    let date = at.format("%Y%m%d").to_string();
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{scope}\n{}",
        amz_date(at),
        sha256_hex(canonical_request.as_bytes())
    );
    let key = signing_key(&creds.secret_access_key, &date, region, service);
    let signature = hex::encode(hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA256, &key),
        string_to_sign.as_bytes(),
    ));
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, \
         Signature={signature}",
        creds.access_key_id
    );
    Signature {
        canonical_request,
        string_to_sign,
        signature,
        authorization,
    }
}

/// `20150830T123600Z`.
pub fn amz_date(at: DateTime<Utc>) -> String {
    at.format("%Y%m%dT%H%M%SZ").to_string()
}

/// The key of one day, region and service (`date` as `20150830`).
pub fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let step = |key: &[u8], data: &str| {
        hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), data.as_bytes())
            .as_ref()
            .to_vec()
    };
    let key = step(format!("AWS4{secret}").as_bytes(), date);
    let key = step(&key, region);
    let key = step(&key, service);
    step(&key, "aws4_request")
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(digest::digest(&digest::SHA256, data))
}

/// Percent-encodes every byte but `A-Z a-z 0-9 - _ . ~` (and `/` when `keep_slash`), in upper
/// case.
fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'_' | b'.' | b'~')
            || (keep_slash && b == b'/')
        {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The path as sent, encoded once more (`%20` becomes `%2520`); `/` when empty.
pub fn canonical_uri(path: &str) -> String {
    if path.is_empty() {
        "/".into()
    } else {
        uri_encode(path, true)
    }
}

/// The query's parameters decoded, encoded again and sorted by name, then value.
pub fn canonical_query(query: &str) -> String {
    let decode = |s: &str| {
        percent_encoding::percent_decode_str(s)
            .decode_utf8_lossy()
            .into_owned()
    };
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (uri_encode(&decode(k), false), uri_encode(&decode(v), false))
        })
        .collect();
    pairs.sort();
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// `name:value\n` per header, sorted, values trimmed with inner whitespace runs as one space and
/// repeated names joined with `,`; and the `;`-joined names.
fn canonical_headers(headers: &[(String, String)]) -> (String, String) {
    let mut merged: Vec<(String, String)> = Vec::new();
    for (name, value) in headers {
        let name = name.trim().to_ascii_lowercase();
        let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
        match merged.iter_mut().find(|(n, _)| *n == name) {
            Some((_, v)) => {
                v.push(',');
                v.push_str(&value);
            }
            None => merged.push((name, value)),
        }
    }
    merged.sort_by(|a, b| a.0.cmp(&b.0));
    let canonical = merged
        .iter()
        .map(|(n, v)| format!("{n}:{v}\n"))
        .collect::<String>();
    let names = merged
        .iter()
        .map(|(n, _)| n.as_str())
        .collect::<Vec<_>>()
        .join(";");
    (canonical, names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::sigv4_vectors::{self as v, VECTORS};
    use chrono::TimeZone as _;

    fn at() -> DateTime<Utc> {
        Utc.timestamp_opt(v::TIME, 0).unwrap()
    }

    fn signer() -> Signer {
        let creds = Credentials::static_keys(v::KEY_ID, v::SECRET, None).unwrap();
        Signer::new(
            v::SERVICE,
            v::REGION,
            Arc::new(CredentialProvider::fixed(creds)),
        )
    }

    fn request(
        method: &str,
        target: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> reqwest::Request {
        let url = reqwest::Url::parse(&format!("https://{}{target}", v::HOST)).unwrap();
        let mut req = reqwest::Request::new(method.parse().unwrap(), url);
        for (name, value) in headers {
            req.headers_mut().append(
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        if !body.is_empty() {
            *req.body_mut() = Some(body.to_string().into());
        }
        req
    }

    #[test]
    fn the_aws_suite_vectors_sign_as_expected() {
        assert_eq!(amz_date(at()), "20150830T123600Z");
        for case in &VECTORS {
            let creds = Credentials::static_keys(v::KEY_ID, v::SECRET, case.session_token).unwrap();
            let mut req = request(case.method, case.target, case.headers, case.body);
            let signed = signer().sign_with(&mut req, &creds, at()).unwrap();
            assert_eq!(
                signed.canonical_request, case.canonical_request,
                "{}",
                case.name
            );
            assert_eq!(signed.string_to_sign, case.string_to_sign, "{}", case.name);
            assert_eq!(signed.authorization, case.authorization, "{}", case.name);
            assert_eq!(
                req.headers()[AUTHORIZATION].to_str().unwrap(),
                case.authorization,
                "{}",
                case.name
            );
            assert_eq!(req.headers()["x-amz-date"], "20150830T123600Z");
            assert_eq!(
                req.headers()
                    .get("x-amz-security-token")
                    .map(|t| t.to_str().unwrap()),
                case.session_token,
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn the_signing_key_derivation_of_the_documentation() {
        // The example of the SigV4 documentation (IAM, 2012-02-15).
        let key = signing_key(v::SECRET, "20120215", "us-east-1", "iam");
        assert_eq!(
            hex::encode(key),
            "f4780e2d9f65fa895f9c67b32ce1baf0b0d8a43505a000a1a9e090d414db404d"
        );
    }

    #[test]
    fn paths_queries_and_hosts_are_canonical() {
        assert_eq!(canonical_uri(""), "/");
        assert_eq!(canonical_uri("/idx/_doc/a%2Fb"), "/idx/_doc/a%252Fb");
        assert_eq!(canonical_uri("/a-b_c.d~e"), "/a-b_c.d~e");
        assert_eq!(canonical_query(""), "");
        assert_eq!(
            canonical_query("b=2&a=x%20y&a=1&flag"),
            "a=1&a=x%20y&b=2&flag="
        );
        assert_eq!(canonical_query("q=*&s=a,b"), "q=%2A&s=a%2Cb");
        // A port that is not the scheme's default is part of the signed host.
        let url = reqwest::Url::parse("http://localhost:8000/").unwrap();
        let mut req = reqwest::Request::new(reqwest::Method::POST, url);
        let creds = Credentials::static_keys("A", "s", None).unwrap();
        let signed = signer().sign_with(&mut req, &creds, at()).unwrap();
        assert!(signed.canonical_request.contains("\nhost:localhost:8000\n"));
        // A Host header the client set wins; transport headers are not signed.
        let mut req = request(
            "GET",
            "/",
            &[
                ("Host", "db.example.test:9200"),
                ("User-Agent", "x"),
                ("X-Multi", "a"),
                ("x-multi", " b  c "),
            ],
            "",
        );
        let signed = signer().sign_with(&mut req, &creds, at()).unwrap();
        assert!(
            signed
                .canonical_request
                .contains("\nhost:db.example.test:9200\n")
        );
        assert!(signed.canonical_request.contains("\nx-multi:a,b c\n"));
        assert!(!signed.canonical_request.contains("user-agent"));
        assert!(
            signed
                .authorization
                .contains("SignedHeaders=host;x-amz-date;x-multi,")
        );
    }

    #[test]
    fn a_streamed_body_cannot_be_signed() {
        let url = reqwest::Url::parse("https://example.amazonaws.com/").unwrap();
        let mut req = reqwest::Request::new(reqwest::Method::POST, url);
        let pieces: Vec<Result<&'static [u8], std::io::Error>> = vec![Ok(b"a")];
        *req.body_mut() = Some(reqwest::Body::wrap_stream(futures::stream::iter(pieces)));
        let creds = Credentials::static_keys("A", "s", None).unwrap();
        let err = signer().sign_with(&mut req, &creds, at()).unwrap_err();
        assert_eq!(err.code(), "INVALID_INPUT");
    }

    #[tokio::test]
    async fn requests_are_signed_with_the_provider_s_credentials() {
        let mut req = request("POST", "/", &[], "{}");
        signer().sign(&mut req).await.unwrap();
        let auth = req.headers()[AUTHORIZATION].to_str().unwrap();
        assert!(
            auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"),
            "{auth}"
        );
        assert!(auth.contains("/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, "));
        assert!(!signer().refreshes());
        // Signed again (a retry), the old signature is replaced, not added to.
        signer().sign(&mut req).await.unwrap();
        assert_eq!(req.headers().get_all(AUTHORIZATION).iter().count(), 1);
    }
}
