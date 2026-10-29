//! AWS Signature Version 4 (`AWS4-HMAC-SHA256`) on aws-lc-rs, checked against AWS's own test
//! suite (`aws-c-auth`, `tests/aws-signing-test-suite/v4`). DynamoDB signs every header it sends
//! (`content-type`, `host`, `x-amz-date`, `x-amz-security-token`, `x-amz-target`) and the body.
//! Nothing here is DynamoDB's own, so the shared `drivers::common::aws` of step F8a can take the
//! module as it is.

use aws_lc_rs::{digest, hmac};
use chrono::{DateTime, Utc};

pub const ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// What is signed of a request.
pub struct Request<'a> {
    pub method: &'a str,
    /// The path as sent, before encoding (`/`, `/example space/`).
    pub path: &'a str,
    /// The query string without `?` (`Param2=value2&Param1=value1`), empty for none.
    pub query: &'a str,
    /// The headers to sign, `host` included, names in any case; a repeated name is joined in
    /// order.
    pub headers: &'a [(String, String)],
    pub payload: &'a [u8],
}

/// Region and service of the credential scope.
#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    pub region: &'a str,
    pub service: &'a str,
}

/// The signature of a request and the `Authorization` header that carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signed {
    pub canonical_request: String,
    pub string_to_sign: String,
    pub signature: String,
    pub authorization: String,
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(digest::digest(&digest::SHA256, data))
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    hmac::sign(&key, data).as_ref().to_vec()
}

/// `X-Amz-Date`: `20150830T123600Z`.
pub fn amz_date(t: DateTime<Utc>) -> String {
    t.format("%Y%m%dT%H%M%SZ").to_string()
}

fn short_date(t: DateTime<Utc>) -> String {
    t.format("%Y%m%d").to_string()
}

/// RFC 3986 percent-encoding: everything but `A-Z a-z 0-9 - . _ ~` (and `/` when `keep_slash`).
pub fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(b));
            }
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

/// The path encoded once, slashes kept (the services other than S3 sign it as sent).
pub fn canonical_uri(path: &str) -> String {
    if path.is_empty() {
        return "/".into();
    }
    uri_encode(&decode(path), true)
}

/// Parameters sorted by encoded name, then value, each encoded once.
pub fn canonical_query(query: &str) -> String {
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

/// A header value as signed: trimmed, every run of whitespace (folded lines too) one space.
fn header_value(v: &str) -> String {
    v.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The canonical header block (one `name:value\n` line per name, sorted) and the signed header
/// list (`host;x-amz-date`).
pub fn canonical_headers(headers: &[(String, String)]) -> (String, String) {
    let mut by_name: Vec<(String, Vec<String>)> = Vec::new();
    for (name, value) in headers {
        let name = name.trim().to_ascii_lowercase();
        let value = header_value(value);
        match by_name.iter_mut().find(|(n, _)| *n == name) {
            Some((_, values)) => values.push(value),
            None => by_name.push((name, vec![value])),
        }
    }
    by_name.sort_by(|a, b| a.0.cmp(&b.0));
    let mut block = String::new();
    for (name, values) in &by_name {
        block.push_str(name);
        block.push(':');
        block.push_str(&values.join(","));
        block.push('\n');
    }
    let signed = by_name
        .iter()
        .map(|(n, _)| n.as_str())
        .collect::<Vec<_>>()
        .join(";");
    (block, signed)
}

/// The canonical request and its signed header list.
pub fn canonical_request(req: &Request<'_>) -> (String, String) {
    let (headers, signed) = canonical_headers(req.headers);
    let text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        req.method.to_ascii_uppercase(),
        canonical_uri(req.path),
        canonical_query(req.query),
        headers,
        signed,
        sha256_hex(req.payload)
    );
    (text, signed)
}

pub fn credential_scope(t: DateTime<Utc>, scope: Scope<'_>) -> String {
    format!(
        "{}/{}/{}/aws4_request",
        short_date(t),
        scope.region,
        scope.service
    )
}

pub fn string_to_sign(t: DateTime<Utc>, scope: Scope<'_>, canonical_request: &str) -> String {
    format!(
        "{ALGORITHM}\n{}\n{}\n{}",
        amz_date(t),
        credential_scope(t, scope),
        sha256_hex(canonical_request.as_bytes())
    )
}

/// `kSigning`: the secret through the date, region, service and `aws4_request`.
pub fn signing_key(secret: &str, t: DateTime<Utc>, scope: Scope<'_>) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), short_date(t).as_bytes());
    let k_region = hmac_sha256(&k_date, scope.region.as_bytes());
    let k_service = hmac_sha256(&k_region, scope.service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

/// Signs `req` at `t`. The headers must already hold everything sent and signed (`host`,
/// `x-amz-date`, a session token's `x-amz-security-token`).
pub fn sign(
    req: &Request<'_>,
    access_key_id: &str,
    secret: &str,
    t: DateTime<Utc>,
    scope: Scope<'_>,
) -> Signed {
    let (canonical_request, signed_headers) = canonical_request(req);
    let string_to_sign = string_to_sign(t, scope, &canonical_request);
    let signature = hex::encode(hmac_sha256(
        &signing_key(secret, t, scope),
        string_to_sign.as_bytes(),
    ));
    let authorization = format!(
        "{ALGORITHM} Credential={access_key_id}/{}, SignedHeaders={signed_headers}, \
         Signature={signature}",
        credential_scope(t, scope)
    );
    Signed {
        canonical_request,
        string_to_sign,
        signature,
        authorization,
    }
}

/// The parts of an `Authorization` header (`Credential`, `SignedHeaders`, `Signature`), for the
/// fakes that check what a client signed.
pub fn parse_authorization(header: &str) -> Option<(String, Vec<String>, String)> {
    let rest = header.strip_prefix(ALGORITHM)?.trim();
    let mut credential = None;
    let mut signed = None;
    let mut signature = None;
    for part in rest.split(',') {
        let (k, v) = part.trim().split_once('=')?;
        match k {
            "Credential" => credential = Some(v.to_string()),
            "SignedHeaders" => signed = Some(v.split(';').map(str::to_string).collect()),
            "Signature" => signature = Some(v.to_string()),
            _ => {}
        }
    }
    Some((credential?, signed?, signature?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    const KEY_ID: &str = "AKIDEXAMPLE";
    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
    const SCOPE: Scope<'static> = Scope {
        region: "us-east-1",
        service: "service",
    };
    const TOKEN: &str = "AQoDYXdzEPT//////////wEXAMPLEtc764bNrC9SAPBSM22wDOk4x4HIZ8j4FZTwdQWLWsKWHGBuFqwAeMicRXmxfpSPfIeoIYRqTflfKD8YUuwthAx7mSEI/qkPpKPi/kMcGdQrmGdeehM4IC1NtBmUpp2wUE8phUZampKsburEDy0KPkyQDYwT7WZ0wq5VSXDvp75YU9HFvlRd8Tx6q6fE8YQcHNVXAkiY9q6d+xo0rKwT38xVqr7ZD0u0iPPkUL64lIZbqBAz+scqKmlzm8FDrypNC9Yjc8fPOLn9FX9KSYvKTr4rvx3iSIlTJabIQwj2ICCR/oLxBA==";

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0).unwrap()
    }

    fn headers(extra: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut h = vec![
            ("Host".to_string(), "example.amazonaws.com".to_string()),
            ("X-Amz-Date".to_string(), amz_date(at())),
        ];
        h.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        h
    }

    /// One case of the suite: the canonical request, the string to sign and the signature.
    #[allow(clippy::too_many_arguments)]
    fn check(
        method: &str,
        path: &str,
        query: &str,
        hdrs: &[(String, String)],
        payload: &[u8],
        canonical: &str,
        sts_hash: &str,
        signature: &str,
    ) {
        let req = Request {
            method,
            path,
            query,
            headers: hdrs,
            payload,
        };
        let signed = sign(&req, KEY_ID, SECRET, at(), SCOPE);
        assert_eq!(signed.canonical_request, canonical);
        assert_eq!(
            signed.string_to_sign,
            format!(
                "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\n{sts_hash}"
            )
        );
        assert_eq!(signed.signature, signature);
        let (credential, names, sig) = parse_authorization(&signed.authorization).unwrap();
        assert_eq!(
            credential,
            "AKIDEXAMPLE/20150830/us-east-1/service/aws4_request"
        );
        assert_eq!(names.join(";"), canonical.lines().rev().nth(1).unwrap());
        assert_eq!(sig, signature);
    }

    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn get_vanilla() {
        check(
            "GET",
            "/",
            "",
            &headers(&[]),
            b"",
            &format!(
                "GET\n/\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\nhost;x-amz-date\n{EMPTY}"
            ),
            "bb579772317eb040ac9ed261061d46c1f17a8133879d6129b6e1c25292927e63",
            "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31",
        );
    }

    #[test]
    fn post_vanilla() {
        check(
            "POST",
            "/",
            "",
            &headers(&[]),
            b"",
            &format!(
                "POST\n/\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\nhost;x-amz-date\n{EMPTY}"
            ),
            "553f88c9e4d10fc9e109e2aeb65f030801b70c2f6468faca261d401ae622fc87",
            "5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b",
        );
    }

    #[test]
    fn query_parameters_are_sorted() {
        check(
            "GET",
            "/",
            "Param2=value2&Param1=value1",
            &headers(&[]),
            b"",
            &format!(
                "GET\n/\nParam1=value1&Param2=value2\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\nhost;x-amz-date\n{EMPTY}"
            ),
            "816cd5b414d056048ba4f7c5386d6e0533120fb1fcfa93762cf0fc39e2cf19e0",
            "b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500",
        );
    }

    #[test]
    fn unreserved_and_utf8_query_characters() {
        let unreserved = "-._~0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        let query = format!("{unreserved}={unreserved}");
        let req = Request {
            method: "GET",
            path: "/",
            query: &query,
            headers: &headers(&[]),
            payload: b"",
        };
        assert_eq!(
            sign(&req, KEY_ID, SECRET, at(), SCOPE).signature,
            "9c3e54bfcdf0b19771a7f523ee5669cdf59bc7cc0884027167c21bb143a40197"
        );
        check(
            "GET",
            "/",
            "\u{1234}=bar",
            &headers(&[]),
            b"",
            &format!(
                "GET\n/\n%E1%88%B4=bar\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\nhost;x-amz-date\n{EMPTY}"
            ),
            "eb30c5bed55734080471a834cc727ae56beb50e5f39d1bff6d0d38cb192a7073",
            "2cdec8eed098649ff3a119c94853b13c643bcf08f8b0a1d91e12c9027818dd04",
        );
    }

    #[test]
    fn paths_are_encoded_once() {
        check(
            "GET",
            "/example space/",
            "",
            &headers(&[]),
            b"",
            &format!(
                "GET\n/example%20space/\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\nhost;x-amz-date\n{EMPTY}"
            ),
            "63ee75631ed7234ae61b5f736dfc7754cdccfedbff4b5128a915706ee9390d86",
            "652487583200325589f1fba4c7e578f72c47cb61beeca81406b39ddec1366741",
        );
        let req = Request {
            method: "GET",
            path: "/\u{1234}",
            query: "",
            headers: &headers(&[]),
            payload: b"",
        };
        let signed = sign(&req, KEY_ID, SECRET, at(), SCOPE);
        assert!(signed.canonical_request.starts_with("GET\n/%E1%88%B4\n"));
        assert_eq!(
            signed.signature,
            "8318018e0b0f223aa2bbf98705b62bb787dc9c0e678f255a891fd03141be5d85"
        );
    }

    #[test]
    fn header_values_are_trimmed_folded_and_joined() {
        check(
            "GET",
            "/",
            "",
            &headers(&[("My-Header1", " value1"), ("My-Header2", " \"a   b   c\"")]),
            b"",
            &format!(
                "GET\n/\n\nhost:example.amazonaws.com\nmy-header1:value1\nmy-header2:\"a b c\"\nx-amz-date:20150830T123600Z\n\nhost;my-header1;my-header2;x-amz-date\n{EMPTY}"
            ),
            "a726db9b0df21c14f559d0a978e563112acb1b9e05476f0a6a1c7d68f28605c7",
            "acc3ed3afb60bb290fc8d2dd0098b9911fcaa05412b367055dee359757a9c736",
        );
        let duplicate = headers(&[
            ("My-Header1", "value2"),
            ("My-Header1", "value2"),
            ("My-Header1", "value1"),
        ]);
        let req = Request {
            method: "GET",
            path: "/",
            query: "",
            headers: &duplicate,
            payload: b"",
        };
        let signed = sign(&req, KEY_ID, SECRET, at(), SCOPE);
        assert!(
            signed
                .canonical_request
                .contains("my-header1:value2,value2,value1\n")
        );
        assert_eq!(
            signed.signature,
            "c9d5ea9f3f72853aea855b47ea873832890dbdd183b4468f858259531a5138ea"
        );
        let multiline = headers(&[("My-Header1", "value1\n  value2\n     value3")]);
        let req = Request {
            method: "GET",
            path: "/",
            query: "",
            headers: &multiline,
            payload: b"",
        };
        assert_eq!(
            sign(&req, KEY_ID, SECRET, at(), SCOPE).signature,
            "cfd34249e4b1c8d6b91ef74165d41a32e5fab3306300901bb65a51a73575eefd"
        );
    }

    #[test]
    fn signed_body_and_session_token() {
        let payload_hash = sha256_hex(b"Param1=value1");
        check(
            "POST",
            "/",
            "",
            &headers(&[
                ("Content-Type", "application/x-www-form-urlencoded"),
                ("Content-Length", "13"),
                ("X-Amz-Content-Sha256", &payload_hash),
            ]),
            b"Param1=value1",
            &format!(
                "POST\n/\n\ncontent-length:13\ncontent-type:application/x-www-form-urlencoded\nhost:example.amazonaws.com\nx-amz-content-sha256:{payload_hash}\nx-amz-date:20150830T123600Z\n\ncontent-length;content-type;host;x-amz-content-sha256;x-amz-date\n{payload_hash}"
            ),
            "b1edd1d03544c25390e32085d55b57acc9a3961bb59415ff86c45c3d89d16cfb",
            "d3875051da38690788ef43de4db0d8f280229d82040bfac253562e56c3f20e0b",
        );
        // post-sts-header-before: the token is a signed header.
        check(
            "POST",
            "/",
            "",
            &headers(&[("X-Amz-Security-Token", TOKEN)]),
            b"",
            &format!(
                "POST\n/\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\nx-amz-security-token:{TOKEN}\n\nhost;x-amz-date;x-amz-security-token\n{EMPTY}"
            ),
            "c237e1b440d4c63c32ca95b5b99481081cb7b13c7e40434868e71567c1a882f6",
            "85d96828115b5dc0cfc3bd16ad9e210dd772bbebba041836c64533a82be05ead",
        );
    }

    #[test]
    fn signing_key_derivation() {
        // The derivation example of the SigV4 documentation (IAM, 2012-02-15).
        let t = Utc.with_ymd_and_hms(2012, 2, 15, 0, 0, 0).unwrap();
        let key = signing_key(
            SECRET,
            t,
            Scope {
                region: "us-east-1",
                service: "iam",
            },
        );
        assert_eq!(
            hex::encode(key),
            "f4780e2d9f65fa895f9c67b32ce1baf0b0d8a43505a000a1a9e090d414db404d"
        );
    }

    #[test]
    fn malformed_authorization_headers_are_none() {
        assert!(parse_authorization("Bearer x").is_none());
        assert!(parse_authorization("AWS4-HMAC-SHA256 Credential=a").is_none());
        assert!(parse_authorization("AWS4-HMAC-SHA256 nonsense").is_none());
    }
}
