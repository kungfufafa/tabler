//! The small HTTP seam both clients are written against. The real implementation is the shared
//! `drivers::common::http::HttpClient` (F8a: credentials, tunnels, TLS, retries); the tests use
//! the in-process fakes of each driver, or a plain reqwest client against `testkit::fake_http`.
//! Credentials are the transport's business: requests here carry none.

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: Method,
    /// Absolute.
    pub url: String,
    /// A JSON body.
    pub body: Option<Vec<u8>>,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> HttpRequest {
        HttpRequest {
            method: Method::Get,
            url: url.into(),
            body: None,
        }
    }

    pub fn post_json(
        url: impl Into<String>,
        body: &impl Serialize,
    ) -> Result<HttpRequest, AppError> {
        Ok(HttpRequest {
            method: Method::Post,
            url: url.into(),
            body: Some(serde_json::to_vec(body)?),
        })
    }

    /// The body as JSON (fakes and tests).
    pub fn json(&self) -> Result<serde_json::Value, AppError> {
        Ok(serde_json::from_slice(
            self.body.as_deref().unwrap_or(b"null"),
        )?)
    }

    /// The path of `url`, without scheme, host or query.
    pub fn path(&self) -> &str {
        let rest = self
            .url
            .split_once("://")
            .map_or(self.url.as_str(), |(_, r)| r);
        let path = rest.find('/').map_or("/", |i| &rest[i..]);
        path.split(['?', '#']).next().unwrap_or("/")
    }

    /// The query string of `url`, if any.
    pub fn query(&self) -> Option<&str> {
        let q = self.url.split_once('?')?.1;
        Some(q.split('#').next().unwrap_or(q))
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
            content_type: Some("application/json".into()),
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
                "unexpected {what} response from the server: {e} (body: {})",
                body_excerpt(&self.body, self.content_type.as_deref())
            ))
        })
    }
}

/// A response whose body arrives in pieces (LibSQL's cursor).
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>, AppError>> + Send>>;

pub struct StreamingResponse {
    pub status: u16,
    pub retry_after: Option<u64>,
    pub content_type: Option<String>,
    pub body: ByteStream,
}

impl StreamingResponse {
    /// The whole body, for an error status.
    pub async fn collect(self) -> HttpResponse {
        use futures::StreamExt as _;
        let mut body = Vec::new();
        let mut pieces = self.body;
        while let Some(Ok(piece)) = pieces.next().await {
            body.extend_from_slice(&piece);
        }
        HttpResponse {
            status: self.status,
            retry_after: self.retry_after,
            content_type: self.content_type,
            body,
        }
    }
}

#[async_trait]
pub trait Transport: Send + Sync {
    /// Sends `req` and reads the whole reply. Transport failures are `Connect`, `Timeout`,
    /// `NotConnected` or `Cancelled`; any HTTP status is a reply.
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, AppError>;

    /// Sends `req` and hands the body over as it arrives. By default the body is read whole.
    async fn send_streaming(&self, req: HttpRequest) -> Result<StreamingResponse, AppError> {
        let reply = self.send(req).await?;
        Ok(StreamingResponse {
            status: reply.status,
            retry_after: reply.retry_after,
            content_type: reply.content_type,
            body: Box::pin(futures::stream::iter([Ok(reply.body)])),
        })
    }
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

/// The error of an HTTP status that is not a success, with the server's own `message` (already
/// taken out of its JSON envelope where the engine has one). `engine` names the driver.
pub fn status_error(
    engine: &str,
    status: u16,
    retry_after: Option<u64>,
    message: &str,
) -> AppError {
    let said = if message.is_empty() {
        String::new()
    } else {
        format!(": {message}")
    };
    match status {
        401 | 403 => AppError::Auth(format!(
            "{engine} refused the token (HTTP {status}){said}; check the token and its permissions"
        )),
        404 => AppError::NotFound(format!("{engine} database (HTTP 404){said}")),
        408 | 504 => AppError::Timeout,
        429 => AppError::Db {
            message: format!("{engine} is rate limiting requests (HTTP 429){said}"),
            detail: None,
            hint: Some(match retry_after {
                Some(secs) => format!("Retry in {secs} s."),
                None => "Wait a little before retrying.".to_string(),
            }),
            sqlstate: None,
            vendor_code: None,
            position: None,
            statement_index: None,
        },
        502 | 503 => AppError::Connect(format!("{engine} is unavailable (HTTP {status}){said}")),
        _ => AppError::db(format!("{engine} answered HTTP {status}{said}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_paths_and_queries() {
        let r = HttpRequest::get("https://h.example:8080/v3/pipeline?x=1#f");
        assert_eq!(r.path(), "/v3/pipeline");
        assert_eq!(r.query(), Some("x=1"));
        assert_eq!(HttpRequest::get("http://h").path(), "/");
        assert_eq!(HttpRequest::get("http://h").query(), None);
        let p = HttpRequest::post_json("http://h/a", &serde_json::json!({"k": 1})).unwrap();
        assert_eq!(p.json().unwrap(), serde_json::json!({"k": 1}));
    }

    #[test]
    fn excerpts_are_short_plain_text() {
        let html = b"<html><body><h1>502 Bad Gateway</h1>\n<p>nginx</p></body></html>";
        assert_eq!(
            body_excerpt(html, Some("text/html")),
            "502 Bad Gateway nginx"
        );
        assert_eq!(body_excerpt(b"<p>x</p>", None), "x");
        let long = "a".repeat(1000);
        assert_eq!(
            body_excerpt(long.as_bytes(), None).chars().count(),
            EXCERPT_CHARS + 1
        );
        let r = HttpResponse::text(200, "not json");
        let e = r.parse::<serde_json::Value>("pipeline").unwrap_err();
        assert!(e.to_string().contains("not json"), "{e}");
    }

    #[test]
    fn statuses_map_to_error_kinds() {
        assert!(matches!(
            status_error("LibSQL", 401, None, ""),
            AppError::Auth(_)
        ));
        assert!(matches!(
            status_error("D1", 403, None, "x"),
            AppError::Auth(_)
        ));
        assert!(matches!(
            status_error("D1", 404, None, ""),
            AppError::NotFound(_)
        ));
        assert!(matches!(
            status_error("D1", 504, None, ""),
            AppError::Timeout
        ));
        assert!(matches!(
            status_error("D1", 503, None, ""),
            AppError::Connect(_)
        ));
        match status_error("D1", 429, Some(7), "slow down") {
            AppError::Db { message, hint, .. } => {
                assert!(message.contains("slow down"));
                assert_eq!(hint.as_deref(), Some("Retry in 7 s."));
            }
            other => panic!("{other:?}"),
        }
        assert!(
            status_error("LibSQL", 400, None, "bad")
                .to_string()
                .contains("HTTP 400: bad")
        );
    }

    #[tokio::test]
    async fn the_default_streaming_send_reads_the_whole_body() {
        struct Echo;
        #[async_trait]
        impl Transport for Echo {
            async fn send(&self, req: HttpRequest) -> Result<HttpResponse, AppError> {
                Ok(HttpResponse::text(200, req.path()))
            }
        }
        let r = Echo
            .send_streaming(HttpRequest::get("http://h/p"))
            .await
            .unwrap();
        assert_eq!(r.collect().await.body, b"/p");
    }
}
