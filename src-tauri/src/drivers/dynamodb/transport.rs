//! The HTTP seam the client is written against: one signed POST, one whole reply (at most
//! [`max_reply_bytes`] of JSON; more is refused). [`HttpTransport`] sends it with the shared
//! `drivers::common::http::HttpClient` (TLS, the system proxy, timeouts) exactly as it was
//! signed, without retries of its own (the client has its throttling and clock-skew loop); the
//! tests use the in-process fake of `testing`, or `HttpTransport` against `testkit::fake_http`.

use async_trait::async_trait;
use reqwest::Method;
use reqwest::header::HeaderValue;

use crate::drivers::common::http::{HttpClient, Repeat};
use crate::error::AppError;

/// Header names, lower case.
pub mod header {
    pub const AUTHORIZATION: &str = "authorization";
    pub const CONTENT_TYPE: &str = "content-type";
    pub const DATE: &str = "date";
    pub const AMZ_DATE: &str = "x-amz-date";
    pub const SECURITY_TOKEN: &str = "x-amz-security-token";
    pub const TARGET: &str = "x-amz-target";
    pub const ERROR_TYPE: &str = "x-amzn-errortype";
}

#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// Absolute; always a POST.
    pub url: String,
    /// Names lower case; `host` is not among them (the transport sends the URL's).
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        find(&self.headers, name)
    }

    /// The action of `x-amz-target` (`Scan` of `DynamoDB_20120810.Scan`).
    pub fn action(&self) -> Option<&str> {
        self.header(header::TARGET)?.split_once('.').map(|(_, a)| a)
    }

    #[cfg(test)]
    pub fn json(&self) -> Result<serde_json::Value, AppError> {
        Ok(serde_json::from_slice(&self.body)?)
    }
}

/// The signature and token never show in logs or panics.
impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(k, v)| {
                let hidden = k == header::AUTHORIZATION || k == header::SECURITY_TOKEN;
                (k.as_str(), if hidden { "***" } else { v.as_str() })
            })
            .collect();
        f.debug_struct("HttpRequest")
            .field("url", &self.url)
            .field("headers", &headers)
            .field("body", &String::from_utf8_lossy(&self.body))
            .finish()
    }
}

fn find<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HttpResponse {
    pub status: u16,
    /// Names lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    #[cfg(test)]
    pub fn json(status: u16, value: &serde_json::Value) -> HttpResponse {
        HttpResponse {
            status,
            headers: vec![(
                header::CONTENT_TYPE.into(),
                "application/x-amz-json-1.0".into(),
            )],
            body: value.to_string().into_bytes(),
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        find(&self.headers, name)
    }

    #[cfg(test)]
    pub fn with_header(mut self, name: &str, value: &str) -> HttpResponse {
        self.headers
            .push((name.to_ascii_lowercase(), value.to_string()));
        self
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

#[async_trait]
pub trait Transport: Send + Sync {
    /// Sends `req` and reads the whole reply. Transport failures are `Connect`, `Timeout` or
    /// `NotConnected`; any HTTP status is a reply. Dropping the future abandons the request.
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, AppError>;
}

/// The most of a reply body read for `action`, so a server that streams on is refused before it
/// fills memory. DynamoDB's own are at most 16 MB of items for `BatchGetItem`, 4 MB for
/// `TransactGetItems` and 1 MB for anything else (a page of `Scan`, `Query` or
/// `ExecuteStatement`); as JSON (type tags, base64, escapes) they take more, so each has room.
pub fn max_reply_bytes(action: Option<&str>) -> usize {
    const MB: usize = 1024 * 1024;
    match action {
        Some("BatchGetItem") => 32 * MB,
        Some("TransactGetItems") => 16 * MB,
        _ => 8 * MB,
    }
}

/// The most memory a reply may take once parsed. A JSON object is a tree node however small
/// (`{"":0}` is 6 bytes of text and over 600 of memory), so a reply of many small objects is
/// refused before it is parsed, whatever its length.
pub const MAX_REPLY_MEMORY: usize = 128 * 1024 * 1024;

/// At most the memory `body` takes as a `serde_json::Value`, from one pass over its text: its
/// text, a tree node for every object, and a value with an allocation for every other one.
pub fn parsed_size(body: &[u8]) -> usize {
    // A leaf of `Map<String, Value>` (11 keys and values) with the allocator's due.
    const OBJECT: usize = 760;
    // A value's own place (twice, in a list's spare room) and a key's or string's allocation.
    const VALUE: usize = 96;
    let mut size = body.len();
    let (mut in_string, mut escaped) = (false, false);
    for &c in body {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'{' => size += OBJECT,
            b'[' | b',' | b':' => size += VALUE,
            _ => {}
        }
    }
    size
}

/// The transport of a session: the shared HTTP client of the profile's endpoint. Its own retry
/// policy must be `RetryPolicy::NONE` (the client repeats what may be repeated), and it carries
/// no credential (the client signs every request itself, with its clock corrected).
pub struct HttpTransport {
    http: HttpClient,
}

impl HttpTransport {
    pub fn new(http: HttpClient) -> HttpTransport {
        HttpTransport { http }
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, AppError> {
        let req_action = req.action().map(str::to_string);
        // Exactly the URL that was signed: one the parser would change is refused.
        let mut builder = self.http.request_exact(Method::POST, &req.url)?;
        for (name, value) in &req.headers {
            let mut value = HeaderValue::from_str(value).map_err(|_| {
                AppError::InvalidInput(format!("the {name} header holds characters HTTP cannot"))
            })?;
            // Kept out of reqwest's debug output.
            value.set_sensitive(name == header::AUTHORIZATION || name == header::SECURITY_TOKEN);
            builder = builder.header(name.as_str(), value);
        }
        let mut reply = self
            .http
            .send_repeating(builder.body(req.body), None, Repeat::Safe)
            .await?;
        let headers = reply
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        let status = reply.status();
        let cap = max_reply_bytes(req_action.as_deref());
        let mut body = Vec::new();
        while let Some(chunk) = reply.chunk().await? {
            if body.len() + chunk.len() > cap {
                return Err(AppError::db(format!(
                    "DynamoDB's reply is larger than {} MB",
                    cap >> 20
                )));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_headers_and_hidden_secrets() {
        let req = HttpRequest {
            url: "https://dynamodb.us-east-1.amazonaws.com/".into(),
            headers: vec![
                (header::TARGET.into(), "DynamoDB_20120810.Scan".into()),
                (
                    header::AUTHORIZATION.into(),
                    "AWS4-HMAC-SHA256 secret".into(),
                ),
                (header::SECURITY_TOKEN.into(), "T0KVALUE".into()),
            ],
            body: b"{}".to_vec(),
        };
        assert_eq!(req.action(), Some("Scan"));
        assert_eq!(req.header("X-Amz-Target"), Some("DynamoDB_20120810.Scan"));
        assert_eq!(req.json().unwrap(), serde_json::json!({}));
        let shown = format!("{req:?}");
        assert!(
            !shown.contains("secret") && !shown.contains("T0KVALUE"),
            "{shown}"
        );
        let resp = HttpResponse::json(400, &serde_json::json!({})).with_header("Date", "x");
        assert_eq!(resp.header("date"), Some("x"));
        assert!(!resp.is_success());
    }

    /// Braces and commas inside strings (escaped quotes too) are text, not values.
    #[test]
    fn parsed_sizes_count_values_outside_strings() {
        assert_eq!(parsed_size(b""), 0);
        assert_eq!(parsed_size(b"{}"), 2 + 760);
        assert_eq!(parsed_size(br#"{"a":[1,2]}"#), 11 + 760 + 3 * 96);
        let quoted = br#"{"a":"{[,:\"{{"}"#;
        assert_eq!(parsed_size(quoted), quoted.len() + 760 + 96);
        // One-entry objects: at least the tree node each takes.
        let deep = format!("{}0{}", r#"{"":"#.repeat(100), "}".repeat(100));
        assert!(parsed_size(deep.as_bytes()) >= 100 * 632);
    }
}
