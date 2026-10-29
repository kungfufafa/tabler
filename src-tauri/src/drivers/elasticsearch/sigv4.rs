//! AWS Signature Version 4 for Amazon OpenSearch Service (`es`) and OpenSearch Serverless
//! (`aoss`), over the exact URL, headers and body a request is sent with. Phase B can swap it
//! for the shared signer of `drivers::common::aws` (F8a) once that exists; the test vectors of
//! the AWS signing test suite pin the behaviour either way.
//!
//! - The canonical path encodes each segment of the path as sent once more (every service but
//!   S3 double-encodes), so `%2F` in an id is signed as `%252F`.
//! - The canonical query decodes and re-encodes each key and value strictly, sorted.
//! - Every header the request carries is signed, plus `host` (from the URL, which keeps the
//!   real host name through an SSH tunnel), `x-amz-date` and, for OpenSearch,
//!   `x-amz-content-sha256` (Serverless requires it) and `x-amz-security-token`.

use aws_lc_rs::{digest, hmac};
use chrono::{DateTime, Utc};

use super::transport::HttpRequest;

#[derive(Clone, PartialEq, Eq)]
pub struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

impl std::fmt::Debug for AwsCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsCredentials")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"…")
            .field("session_token", &self.session_token.as_ref().map(|_| "…"))
            .finish()
    }
}

/// What a signature covers besides the credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningScope {
    pub region: String,
    /// `es` or `aoss`.
    pub service: String,
    /// Send and sign `x-amz-content-sha256`.
    pub content_sha256: bool,
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(digest::digest(&digest::SHA256, data).as_ref())
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), data)
        .as_ref()
        .to_vec()
}

/// RFC 3986 encoding: only `A-Z a-z 0-9 - _ . ~` stay.
fn uri_encode(s: &[u8]) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Percent-decoding for the canonical query (lenient: a malformed escape stays as typed).
fn decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

pub fn canonical_path(path: &str) -> String {
    let encoded: Vec<String> = path
        .split('/')
        .map(|seg| uri_encode(seg.as_bytes()))
        .collect();
    let joined = encoded.join("/");
    if joined.is_empty() {
        "/".to_string()
    } else {
        joined
    }
}

pub fn canonical_query(query: Option<&str>) -> String {
    let Some(q) = query.filter(|q| !q.is_empty()) else {
        return String::new();
    };
    let mut pairs: Vec<(String, String)> = q
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (uri_encode(&decode(k)), uri_encode(&decode(v)))
        })
        .collect();
    pairs.sort();
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// The `host` the request goes to, as the `Host` header carries it.
fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .to_string()
}

/// Signs `req` in place: adds `x-amz-date` (and the content hash and session token) and the
/// `authorization` header.
pub fn sign(
    req: &mut HttpRequest,
    creds: &AwsCredentials,
    scope: &SigningScope,
    now: DateTime<Utc>,
) {
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();
    let payload_hash = sha256_hex(req.body.as_deref().unwrap_or_default());
    req.headers.retain(|(k, _)| k != "authorization");
    req.set_header("x-amz-date", amz_date.clone());
    if scope.content_sha256 {
        req.set_header("x-amz-content-sha256", payload_hash.clone());
    }
    if let Some(token) = &creds.session_token {
        req.set_header("x-amz-security-token", token.clone());
    }

    let mut headers: Vec<(String, String)> = req
        .headers
        .iter()
        .map(|(k, v)| {
            (
                k.to_ascii_lowercase(),
                v.split_whitespace().collect::<Vec<_>>().join(" "),
            )
        })
        .collect();
    if !headers.iter().any(|(k, _)| k == "host") {
        headers.push(("host".to_string(), host_of(&req.url)));
    }
    headers.sort();
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_headers = headers
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        req.method.as_str(),
        canonical_path(req.path()),
        canonical_query(req.query()),
        canonical_headers,
        signed_headers,
        payload_hash
    );
    let credential_scope = format!("{date}/{}/{}/aws4_request", scope.region, scope.service);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let k_date = hmac(
        format!("AWS4{}", creds.secret_access_key).as_bytes(),
        date.as_bytes(),
    );
    let k_region = hmac(&k_date, scope.region.as_bytes());
    let k_service = hmac(&k_region, scope.service.as_bytes());
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = hex::encode(hmac(&k_signing, string_to_sign.as_bytes()));
    req.set_header(
        "authorization",
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
            creds.access_key_id
        ),
    );
}

#[cfg(test)]
mod tests {
    //! Vectors from the AWS signing test suite (awslabs/aws-c-auth,
    //! tests/aws-signing-test-suite/v4): credentials `AKIDEXAMPLE`, region `us-east-1`,
    //! service `service`, 2015-08-30T12:36:00Z.

    use chrono::TimeZone as _;

    use super::*;
    use crate::drivers::elasticsearch::console::Method;

    fn creds(token: Option<&str>) -> AwsCredentials {
        AwsCredentials {
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            session_token: token.map(str::to_string),
        }
    }

    fn scope(content_sha256: bool) -> SigningScope {
        SigningScope {
            region: "us-east-1".into(),
            service: "service".into(),
            content_sha256,
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0).unwrap()
    }

    fn signed(
        method: Method,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
        token: Option<&str>,
        content_sha256: bool,
    ) -> String {
        let mut req = HttpRequest {
            method,
            url: url.into(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.map(|b| b.as_bytes().to_vec()),
        };
        sign(&mut req, &creds(token), &scope(content_sha256), now());
        req.header("authorization").unwrap().to_string()
    }

    fn expect(signed_headers: &str, signature: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders={signed_headers}, Signature={signature}"
        )
    }

    #[test]
    fn get_vanilla() {
        assert_eq!(
            signed(
                Method::Get,
                "https://example.amazonaws.com/",
                &[],
                None,
                None,
                false
            ),
            expect(
                "host;x-amz-date",
                "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
            )
        );
    }

    #[test]
    fn post_vanilla() {
        assert_eq!(
            signed(
                Method::Post,
                "https://example.amazonaws.com/",
                &[],
                None,
                None,
                false
            ),
            expect(
                "host;x-amz-date",
                "5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b"
            )
        );
    }

    #[test]
    fn query_keys_are_sorted() {
        assert_eq!(
            signed(
                Method::Get,
                "https://example.amazonaws.com/?Param2=value2&Param1=value1",
                &[],
                None,
                None,
                false
            ),
            expect(
                "host;x-amz-date",
                "b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500"
            )
        );
    }

    #[test]
    fn utf8_in_query_and_path() {
        assert_eq!(
            signed(
                Method::Get,
                "https://example.amazonaws.com/?ሴ=bar",
                &[],
                None,
                None,
                false
            ),
            expect(
                "host;x-amz-date",
                "2cdec8eed098649ff3a119c94853b13c643bcf08f8b0a1d91e12c9027818dd04"
            )
        );
        assert_eq!(
            signed(
                Method::Get,
                "https://example.amazonaws.com/ሴ",
                &[],
                None,
                None,
                false
            ),
            expect(
                "host;x-amz-date",
                "8318018e0b0f223aa2bbf98705b62bb787dc9c0e678f255a891fd03141be5d85"
            )
        );
    }

    #[test]
    fn session_token() {
        assert_eq!(
            signed(
                Method::Get,
                "https://example.amazonaws.com/",
                &[],
                None,
                Some("6e86291e8372ff2a2260956d9b8aae1d763fbf315fa00fa31553b73ebf194267"),
                false
            ),
            expect(
                "host;x-amz-date;x-amz-security-token",
                "07ec1639c89043aa0e3e2de82b96708f198cceab042d4a97044c66dd9f74e7f8"
            )
        );
    }

    #[test]
    fn signed_body() {
        assert_eq!(
            signed(
                Method::Post,
                "https://example.amazonaws.com/",
                &[
                    ("content-type", "application/x-www-form-urlencoded"),
                    ("content-length", "13"),
                ],
                Some("Param1=value1"),
                None,
                true
            ),
            expect(
                "content-length;content-type;host;x-amz-content-sha256;x-amz-date",
                "d3875051da38690788ef43de4db0d8f280229d82040bfac253562e56c3f20e0b"
            )
        );
    }

    #[test]
    fn opensearch_paths_are_encoded_twice() {
        assert_eq!(
            canonical_path("/logs-*/_doc/a%2Fb"),
            "/logs-%2A/_doc/a%252Fb"
        );
        assert_eq!(canonical_path("/"), "/");
        assert_eq!(canonical_path(""), "/");
        assert_eq!(
            canonical_query(Some("b=2&a=x%20y&flag&s=*")),
            "a=x%20y&b=2&flag=&s=%2A"
        );
        assert_eq!(host_of("https://h:9200/a?b"), "h:9200");
        // A re-sign replaces the old signature and keeps one of each header.
        let mut req = HttpRequest {
            method: Method::Post,
            url: "https://search-x.us-east-1.es.amazonaws.com/_bulk".into(),
            headers: vec![("content-type".into(), "application/x-ndjson".into())],
            body: Some(b"{}\n".to_vec()),
        };
        let sc = SigningScope {
            region: "us-east-1".into(),
            service: "es".into(),
            content_sha256: true,
        };
        sign(&mut req, &creds(None), &sc, now());
        sign(&mut req, &creds(None), &sc, now());
        assert_eq!(
            req.headers
                .iter()
                .filter(|(k, _)| k == "authorization")
                .count(),
            1
        );
        assert!(
            req.header("authorization")
                .unwrap()
                .contains("SignedHeaders=content-type;host;x-amz-content-sha256;x-amz-date,")
        );
        assert_eq!(
            req.header("x-amz-content-sha256"),
            Some(sha256_hex(b"{}\n").as_str())
        );
        assert!(!format!("{:?}", creds(Some("t"))).contains("EXAMPLEKEY"));
    }
}
