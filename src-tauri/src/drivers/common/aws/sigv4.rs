//! AWS Signature Version 4 of reqwest requests (DynamoDB `dynamodb`, OpenSearch Service `es`,
//! OpenSearch Serverless `aoss`) on aws-lc-rs. What is signed: the method; the path as sent with
//! every byte outside the unreserved set encoded once more (each segment encoded twice, as AWS
//! does for every service but S3: an id's `%2F` is signed as `%252F`); the query, decoded,
//! encoded again and sorted; every header the request has when it is signed but the ones AWS's
//! SDKs leave out (Host from the Host header or the URL, `X-Amz-Date`, `X-Amz-Security-Token`
//! with temporary credentials, `X-Amz-Content-Sha256` for OpenSearch), names lower-cased and
//! values trimmed; and the SHA-256 of the body, which must not be a stream.

use std::sync::Arc;

use aws_lc_rs::{digest, hmac};
use chrono::{DateTime, Utc};
use reqwest::header::{AUTHORIZATION, HOST, HeaderValue};

use super::credentials::{CredentialProvider, Credentials};
use crate::drivers::common::http::authority;
use crate::error::AppError;

/// Headers that are not signed, as AWS's SDKs leave them out: the transport may add or change
/// them.
const UNSIGNED: &[&str] = &[
    "authorization",
    "connection",
    "expect",
    "transfer-encoding",
    "user-agent",
    "x-amzn-trace-id",
];

/// The services whose requests carry `X-Amz-Content-Sha256`: OpenSearch Serverless refuses
/// requests without it, and the OpenSearch clients send it to both.
const CONTENT_SHA256_SERVICES: &[&str] = &["es", "aoss"];

/// Signs requests for one service in one region.
pub struct Signer {
    service: String,
    region: String,
    creds: Arc<CredentialProvider>,
    /// Sends and signs `X-Amz-Content-Sha256`, the hash of the body.
    content_sha256: bool,
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
    /// `service` as AWS names it in the scope (`dynamodb`, `es`, `aoss`); OpenSearch's send
    /// `X-Amz-Content-Sha256`.
    pub fn new(service: &str, region: &str, creds: Arc<CredentialProvider>) -> Signer {
        Signer {
            service: service.to_string(),
            region: region.to_string(),
            creds,
            content_sha256: CONTENT_SHA256_SERVICES.contains(&service),
        }
    }

    /// Whether requests carry and sign `X-Amz-Content-Sha256`.
    pub fn with_content_sha256(mut self, on: bool) -> Signer {
        self.content_sha256 = on;
        self
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

    /// Signs `req` as of `at`: sets `X-Amz-Date`, `X-Amz-Security-Token` (with a session token),
    /// `X-Amz-Content-Sha256` (when the signer sends it) and `Authorization`.
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
        let payload_hash = match req.body() {
            None => sha256_hex(b""),
            Some(body) => sha256_hex(body.as_bytes().ok_or_else(|| {
                AppError::InvalidInput("a SigV4 request needs its whole body, not a stream".into())
            })?),
        };
        let headers = req.headers_mut();
        headers.remove(AUTHORIZATION);
        headers.insert("x-amz-date", header(&amz_date(at))?);
        if self.content_sha256 {
            headers.insert("x-amz-content-sha256", header(&payload_hash)?);
        }
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
        let signature = sign_hashed(
            req.method().as_str(),
            req.url().path(),
            req.url().query().unwrap_or(""),
            &signed,
            &payload_hash,
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
    sign_hashed(
        method,
        path,
        query,
        headers,
        &sha256_hex(payload),
        creds,
        at,
        region,
        service,
    )
}

/// [`sign_parts`] with the body's SHA-256 in hex.
#[allow(clippy::too_many_arguments)]
fn sign_hashed(
    method: &str,
    path: &str,
    query: &str,
    headers: &[(String, String)],
    payload_hash: &str,
    creds: &Credentials,
    at: DateTime<Utc>,
    region: &str,
    service: &str,
) -> Signature {
    let (canonical_headers, signed_headers) = canonical_headers(headers);
    let canonical_request = format!(
        "{method}\n{}\n{}\n{canonical_headers}\n{signed_headers}\n{payload_hash}",
        canonical_uri(path),
        canonical_query(query),
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

    /// `sign_parts` of the suite's request: `path` and `query` as on the wire, its headers plus
    /// `extra` (and the session token).
    fn suite(
        method: &str,
        path: &str,
        query: &str,
        extra: &[(&str, &str)],
        body: &str,
        token: Option<&str>,
    ) -> String {
        let mut headers: Vec<(String, String)> = vec![
            ("Host".into(), v::HOST.into()),
            ("X-Amz-Date".into(), amz_date(at())),
        ];
        if let Some(token) = token {
            headers.push(("X-Amz-Security-Token".into(), token.into()));
        }
        headers.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        let creds = Credentials::static_keys(v::KEY_ID, v::SECRET, None).unwrap();
        sign_parts(
            method,
            path,
            query,
            &headers,
            body.as_bytes(),
            &creds,
            at(),
            v::REGION,
            v::SERVICE,
        )
        .signature
    }

    /// The cases of AWS's suite (`aws-c-auth`, `tests/aws-signing-test-suite/v4`) the
    /// Elasticsearch and DynamoDB lanes check their own signers with.
    #[test]
    fn the_suite_cases_of_the_lanes_sign_alike() {
        let unreserved = "-._~0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        let form_hash = sha256_hex(b"Param1=value1");
        let cases: [(&str, String, &str); 10] = [
            (
                "post-vanilla",
                suite("POST", "/", "", &[], "", None),
                "5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b",
            ),
            (
                "get-utf8",
                suite("GET", "/\u{1234}", "", &[], "", None),
                "8318018e0b0f223aa2bbf98705b62bb787dc9c0e678f255a891fd03141be5d85",
            ),
            (
                "get-space",
                suite("GET", "/example space/", "", &[], "", None),
                "652487583200325589f1fba4c7e578f72c47cb61beeca81406b39ddec1366741",
            ),
            (
                "get-vanilla-utf8-query",
                suite("GET", "/", "\u{1234}=bar", &[], "", None),
                "2cdec8eed098649ff3a119c94853b13c643bcf08f8b0a1d91e12c9027818dd04",
            ),
            (
                "get-vanilla-query-unreserved",
                suite(
                    "GET",
                    "/",
                    &format!("{unreserved}={unreserved}"),
                    &[],
                    "",
                    None,
                ),
                "9c3e54bfcdf0b19771a7f523ee5669cdf59bc7cc0884027167c21bb143a40197",
            ),
            (
                "get-header-value-trim",
                suite(
                    "GET",
                    "/",
                    "",
                    &[("My-Header1", " value1"), ("My-Header2", " \"a   b   c\"")],
                    "",
                    None,
                ),
                "acc3ed3afb60bb290fc8d2dd0098b9911fcaa05412b367055dee359757a9c736",
            ),
            (
                "get-header-key-duplicate",
                suite(
                    "GET",
                    "/",
                    "",
                    &[
                        ("My-Header1", "value2"),
                        ("My-Header1", "value2"),
                        ("My-Header1", "value1"),
                    ],
                    "",
                    None,
                ),
                "c9d5ea9f3f72853aea855b47ea873832890dbdd183b4468f858259531a5138ea",
            ),
            (
                "get-header-value-multiline",
                suite(
                    "GET",
                    "/",
                    "",
                    &[("My-Header1", "value1\n  value2\n     value3")],
                    "",
                    None,
                ),
                "cfd34249e4b1c8d6b91ef74165d41a32e5fab3306300901bb65a51a73575eefd",
            ),
            (
                "post-x-www-form-urlencoded (signed body)",
                suite(
                    "POST",
                    "/",
                    "",
                    &[
                        ("Content-Type", "application/x-www-form-urlencoded"),
                        ("Content-Length", "13"),
                        ("X-Amz-Content-Sha256", &form_hash),
                    ],
                    "Param1=value1",
                    None,
                ),
                "d3875051da38690788ef43de4db0d8f280229d82040bfac253562e56c3f20e0b",
            ),
            (
                "get-vanilla-with-session-token",
                suite(
                    "GET",
                    "/",
                    "",
                    &[],
                    "",
                    Some("6e86291e8372ff2a2260956d9b8aae1d763fbf315fa00fa31553b73ebf194267"),
                ),
                "07ec1639c89043aa0e3e2de82b96708f198cceab042d4a97044c66dd9f74e7f8",
            ),
        ];
        for (name, got, expected) in cases {
            assert_eq!(got, expected, "{name}");
        }
        // The signed body through a request: the signer adds the hash itself.
        let mut req = request(
            "POST",
            "/",
            &[
                ("Content-Type", "application/x-www-form-urlencoded"),
                ("Content-Length", "13"),
            ],
            "Param1=value1",
        );
        let creds = Credentials::static_keys(v::KEY_ID, v::SECRET, None).unwrap();
        let signed = signer()
            .with_content_sha256(true)
            .sign_with(&mut req, &creds, at())
            .unwrap();
        assert_eq!(
            signed.signature,
            "d3875051da38690788ef43de4db0d8f280229d82040bfac253562e56c3f20e0b"
        );
        assert_eq!(req.headers()["x-amz-content-sha256"], form_hash.as_str());
    }

    /// Requests of the three services, against a signer written apart from this one (Node's
    /// crypto, following AWS's documentation): `es` with an id holding `/` and a wildcard index,
    /// `aoss` with a session token, `dynamodb` with its target header.
    #[test]
    fn es_aoss_and_dynamodb_requests_sign_like_an_independent_signer() {
        let creds = |token: Option<&str>| Credentials::static_keys(v::KEY_ID, v::SECRET, token);
        let sign = |service: &str,
                    region: &str,
                    url: &str,
                    method: &str,
                    headers: &[(&str, &str)],
                    body: &str,
                    token: Option<&str>| {
            let url = reqwest::Url::parse(url).unwrap();
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
            let fixed = Arc::new(CredentialProvider::fixed(creds(token).unwrap()));
            let signed = Signer::new(service, region, fixed)
                .sign_with(&mut req, &creds(token).unwrap(), at())
                .unwrap();
            (signed, req)
        };
        let (es, req) = sign(
            "es",
            "eu-west-1",
            "https://search-logs.eu-west-1.es.amazonaws.com/logs-*/_doc/a%2Fb?refresh=wait_for&routing=x%20y",
            "POST",
            &[
                ("content-type", "application/json"),
                ("x-opaque-id", "tabler-7"),
            ],
            r#"{"a":1}"#,
            None,
        );
        assert!(
            es.canonical_request
                .starts_with("POST\n/logs-%2A/_doc/a%252Fb\nrefresh=wait_for&routing=x%20y\n"),
            "{}",
            es.canonical_request
        );
        assert_eq!(
            es.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/eu-west-1/es/aws4_request, \
             SignedHeaders=content-type;host;x-amz-content-sha256;x-amz-date;x-opaque-id, \
             Signature=a79ad3378d8107a12745cea4cad981df311f3e705db4b69f06d3c782c965e6c5"
        );
        assert_eq!(
            req.headers()["x-amz-content-sha256"],
            "015abd7f5cc57a2dd94b7590f04ad8084273905ee33ec5cebeae62276a97f862"
        );
        let (aoss, req) = sign(
            "aoss",
            "us-east-1",
            "https://abc123.us-east-1.aoss.amazonaws.com/my-index/_search?q=title:%E2%9C%93",
            "GET",
            &[],
            "",
            Some("session-token"),
        );
        assert_eq!(
            aoss.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/aoss/aws4_request, \
             SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-security-token, \
             Signature=7745c1999065c484f77539680f88451c3582375e694224d5295a3ee071ed4b5c"
        );
        assert_eq!(req.headers()["x-amz-security-token"], "session-token");
        let (dynamo, req) = sign(
            "dynamodb",
            "eu-west-1",
            "https://dynamodb.eu-west-1.amazonaws.com/",
            "POST",
            &[
                ("content-type", "application/x-amz-json-1.0"),
                ("x-amz-target", "DynamoDB_20120810.Query"),
            ],
            r#"{"TableName":"t"}"#,
            None,
        );
        assert_eq!(
            dynamo.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/eu-west-1/dynamodb/aws4_request, \
             SignedHeaders=content-type;host;x-amz-date;x-amz-target, \
             Signature=72c09e038961bf84a50369c11da09c3184102ab649eb39ac23687fda23529368"
        );
        // DynamoDB's SDKs send no body hash; an OpenSearch signer can be told not to either.
        assert!(req.headers().get("x-amz-content-sha256").is_none());
        let fixed = Arc::new(CredentialProvider::fixed(creds(None).unwrap()));
        let mut req = request("GET", "/", &[], "");
        let signed = Signer::new("es", "eu-west-1", fixed)
            .with_content_sha256(false)
            .sign_with(&mut req, &creds(None).unwrap(), at())
            .unwrap();
        assert!(
            signed
                .authorization
                .contains("SignedHeaders=host;x-amz-date,")
        );
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
