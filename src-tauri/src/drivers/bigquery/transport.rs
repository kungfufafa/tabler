//! The HTTP seam the client is written against. In Phase B the real implementation is the shared
//! `drivers::common::http::HttpClient` (F8a: TLS through `tls_client`, the system proxy,
//! retries); the tests use the scripted fake of `testing`, or a plain reqwest transport against
//! `testkit::fake_http`.
//!
//! Unlike the sqlite-remote seam, requests carry their own `authorization` header: the token
//! endpoint takes none and the REST API takes the bearer the client got from it, so the client
//! (`client::Client`) owns the credentials and refreshes them once on a 401.

use async_trait::async_trait;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::de::DeserializeOwned;

use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

pub const JSON: &str = "application/json";
pub const FORM: &str = "application/x-www-form-urlencoded";

#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: Method,
    /// Absolute, with the query string.
    pub url: String,
    /// Names lower case.
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> HttpRequest {
        HttpRequest {
            method: Method::Get,
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn post_json(url: impl Into<String>, body: &serde_json::Value) -> HttpRequest {
        HttpRequest {
            method: Method::Post,
            url: url.into(),
            headers: vec![("content-type".into(), JSON.into())],
            body: Some(body.to_string().into_bytes()),
        }
    }

    /// A form post (the OAuth token endpoint).
    pub fn post_form(url: impl Into<String>, fields: &[(&str, &str)]) -> HttpRequest {
        let body = fields
            .iter()
            .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        HttpRequest {
            method: Method::Post,
            url: url.into(),
            headers: vec![("content-type".into(), FORM.into())],
            body: Some(body.into_bytes()),
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> HttpRequest {
        self.headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
        self.headers
            .push((name.to_ascii_lowercase(), value.to_string()));
        self
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The path of `url`, without scheme, host or query (still percent-encoded).
    pub fn path(&self) -> &str {
        let rest = self
            .url
            .split_once("://")
            .map_or(self.url.as_str(), |(_, r)| r);
        let path = rest.find('/').map_or("/", |i| &rest[i..]);
        path.split(['?', '#']).next().unwrap_or("/")
    }

    /// The decoded query parameters, in order.
    pub fn query_pairs(&self) -> Vec<(String, String)> {
        let Some((_, q)) = self.url.split_once('?') else {
            return Vec::new();
        };
        decode_pairs(q.split('#').next().unwrap_or(q))
    }

    /// The last value of query parameter `name`.
    pub fn param(&self, name: &str) -> Option<String> {
        self.query_pairs()
            .into_iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v)
    }

    /// The body as JSON (fakes and tests); `null` without one.
    pub fn json(&self) -> serde_json::Value {
        self.body
            .as_deref()
            .and_then(|b| serde_json::from_slice(b).ok())
            .unwrap_or(serde_json::Value::Null)
    }

    /// The fields of a form body (fakes and tests).
    pub fn form(&self) -> Vec<(String, String)> {
        decode_pairs(&String::from_utf8_lossy(
            self.body.as_deref().unwrap_or_default(),
        ))
    }
}

/// Tokens, assertions, client secrets and refresh tokens never show in logs or panics.
impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(k, v)| {
                let secret = k.eq_ignore_ascii_case("authorization");
                (k.as_str(), if secret { "***" } else { v.as_str() })
            })
            .collect();
        let body = match (&self.body, self.header("content-type")) {
            (Some(_), Some(FORM)) => "***".to_string(),
            (Some(b), _) => String::from_utf8_lossy(b).into_owned(),
            (None, _) => String::new(),
        };
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &headers)
            .field("body", &body)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HttpResponse {
    pub status: u16,
    /// `Retry-After` in seconds.
    pub retry_after: Option<u64>,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn json(status: u16, value: &serde_json::Value) -> HttpResponse {
        HttpResponse {
            status,
            retry_after: None,
            content_type: Some(JSON.into()),
            body: value.to_string().into_bytes(),
        }
    }

    pub fn text(status: u16, text: &str) -> HttpResponse {
        HttpResponse {
            status,
            retry_after: None,
            content_type: Some("text/plain; charset=utf-8".into()),
            body: text.as_bytes().to_vec(),
        }
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The body parsed as `T`; a body that is not one is a protocol error, never a panic.
    pub fn parse<T: DeserializeOwned>(&self, what: &str) -> Result<T, AppError> {
        serde_json::from_slice(&self.body).map_err(|e| {
            AppError::db(format!(
                "unexpected {what} response from BigQuery: {e} (body: {})",
                body_excerpt(&self.body, self.content_type.as_deref())
            ))
        })
    }
}

#[async_trait]
pub trait Transport: Send + Sync {
    /// Sends `req` and reads the whole reply. Transport failures are `Connect`, `Timeout`,
    /// `NotConnected` or `Cancelled`; any HTTP status is a reply.
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, AppError>;
}

/// What a URL component escapes: everything but the unreserved characters.
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// One path segment or query value, percent-encoded.
pub fn encode(s: &str) -> String {
    utf8_percent_encode(s, COMPONENT).to_string()
}

/// Whether credentials (a signed assertion, a refresh token, a bearer) may be sent to `url`:
/// HTTPS, or plain HTTP to this machine (tests and emulators). The host is parsed and compared
/// whole, so `http://localhost.example.com`, `http://127.0.0.1.nip.io` and
/// `http://127.0.0.1:80@evil.example` are other machines.
pub fn credentials_may_go_to(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url.trim()) else {
        return false;
    };
    let host = u.host_str().unwrap_or_default();
    match u.scheme() {
        "https" => !host.is_empty(),
        // The parser has lower-cased the name and written `127.1` as `127.0.0.1`.
        "http" => {
            host == "localhost"
                || host
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        }
        _ => false,
    }
}

/// `a=1&b=x%20y` (also `+` for a space) decoded; invalid escapes stay as written.
fn decode_pairs(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (decode(k), decode(v))
        })
        .collect()
}

fn decode(s: &str) -> String {
    let s = s.replace('+', " ");
    percent_encoding::percent_decode_str(&s)
        .decode_utf8_lossy()
        .into_owned()
}

/// At most this many characters of an unexpected body go into an error message.
const EXCERPT_CHARS: usize = 300;

/// A short plain-text form of a reply body for an error message: HTML loses its tags, and
/// whitespace runs collapse.
pub fn body_excerpt(body: &[u8], content_type: Option<&str>) -> String {
    let text = String::from_utf8_lossy(body);
    let html =
        content_type.is_some_and(|c| c.contains("html")) || text.trim_start().starts_with('<');
    let plain = if html {
        strip_tags(&text)
    } else {
        text.into_owned()
    };
    let collapsed = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > EXCERPT_CHARS {
        let cut: String = collapsed.chars().take(EXCERPT_CHARS).collect();
        format!("{cut}…")
    } else {
        collapsed
    }
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_paths_queries_and_forms() {
        let r = HttpRequest::get(
            "https://h.example/bigquery/v2/projects/p/queries/j?location=asia-southeast2&pageToken=a%2Bb+c#x",
        );
        assert_eq!(r.path(), "/bigquery/v2/projects/p/queries/j");
        assert_eq!(r.param("location").as_deref(), Some("asia-southeast2"));
        assert_eq!(r.param("pageToken").as_deref(), Some("a+b c"));
        assert_eq!(HttpRequest::get("http://h").path(), "/");
        let f = HttpRequest::post_form("https://t/token", &[("a", "x y&z"), ("b", "é=")]);
        assert_eq!(f.body.as_deref(), Some(&b"a=x%20y%26z&b=%C3%A9%3D"[..]));
        assert_eq!(
            f.form(),
            [("a".into(), "x y&z".into()), ("b".into(), "é=".into())]
        );
        let j = HttpRequest::post_json("https://h/x", &serde_json::json!({"k": 1}));
        assert_eq!(j.json(), serde_json::json!({"k": 1}));
        assert_eq!(j.header("Content-Type"), Some(JSON));
        assert_eq!(encode("my-proj:x/y"), "my-proj%3Ax%2Fy");
    }

    #[test]
    fn credentials_go_over_https_or_to_this_machine() {
        for url in [
            "https://oauth2.googleapis.com/token",
            "https://bq.example/bigquery/v2",
            "http://127.0.0.1:9/token",
            "http://127.1/x",
            "http://localhost",
            "HTTP://LocalHost:9050/bigquery/v2",
            "http://[::1]:9050/bigquery/v2",
            " http://localhost:9050 ",
        ] {
            assert!(credentials_may_go_to(url), "{url}");
        }
        for url in [
            "http://evil.example/token",
            "http://localhost.example.com/token",
            "http://127.0.0.1.nip.io/token",
            "http://localhost@evil.example/token",
            "http://127.0.0.1:80@evil.example/token",
            "http://[::1].evil.example/token",
            "http://10.0.0.5:9050/bigquery/v2",
            "file:///etc/passwd",
            "ftp://localhost/x",
            "https://",
            "localhost:9050",
            "",
        ] {
            assert!(!credentials_may_go_to(url), "{url}");
        }
    }

    #[test]
    fn secrets_stay_out_of_debug_output() {
        let r = HttpRequest::post_form("https://t/token", &[("assertion", "eyJsecret")])
            .with_header("Authorization", "Bearer ya29.secret");
        let shown = format!("{r:?}");
        assert!(!shown.contains("secret"), "{shown}");
        assert!(shown.contains("***"));
    }

    #[test]
    fn excerpts_are_short_plain_text() {
        let html = b"<html><body><h1>502 Bad Gateway</h1>\n<p>gfe</p></body></html>";
        assert_eq!(body_excerpt(html, Some("text/html")), "502 Bad Gateway gfe");
        let long = "a".repeat(1000);
        assert_eq!(
            body_excerpt(long.as_bytes(), None).chars().count(),
            EXCERPT_CHARS + 1
        );
        let e = HttpResponse::text(200, "not json")
            .parse::<serde_json::Value>("jobs.query")
            .unwrap_err();
        assert!(e.to_string().contains("not json"), "{e}");
    }
}
