//! The small HTTP seam both clients are written against. Sessions send through `HttpTransport`,
//! the shared `drivers::common::http::HttpClient` (credentials, SSH tunnels, TLS, retries); the
//! tests use each driver's in-process fake, which answers the same calls. Credentials are the
//! transport's business: requests here carry none.

use std::pin::Pin;

use async_trait::async_trait;
use futures::{Stream, StreamExt as _};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::drivers::ConnectTarget;
pub use crate::drivers::common::http::body_excerpt;
use crate::drivers::common::http::{Credential, HttpClient, HttpOptions, HttpReply};
use crate::error::AppError;

/// The most of a reply read whole (a Hrana 2 result, any D1 result: both protocols send a result
/// in one body, before `max_rows` can apply) or of one line of a cursor: beyond it the call
/// fails, rather than a broken or hostile server exhausting the app's memory. The reply is
/// decoded whole too, so the memory it takes at its peak is a multiple of this: about 2 to 3
/// times for Hrana's typed values, and up to about 16 times for D1, whose cells become one JSON
/// value each (32 bytes for a `0,` of two).
pub const MAX_REPLY: usize = 256 << 20;

/// The most of an error reply's body read: its message is at the start.
const MAX_ERROR_BODY: usize = 64 << 10;

/// A reply too large to read.
pub fn too_large() -> AppError {
    AppError::db(format!(
        "the server sent more than {} MB at once: ask for fewer rows (add a LIMIT)",
        MAX_REPLY >> 20
    ))
}

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
    /// The start of the body, for an error status (the rest is dropped with the request).
    pub async fn collect(self) -> HttpResponse {
        let mut body = Vec::new();
        let mut pieces = self.body;
        while body.len() < MAX_ERROR_BODY {
            let Some(Ok(piece)) = pieces.next().await else {
                break;
            };
            let room = MAX_ERROR_BODY - body.len();
            body.extend_from_slice(&piece[..piece.len().min(room)]);
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

/// The sessions' transport: the shared HTTP client, which adds the credential to the requests
/// for its own server only (a `base_url` on another origin is never followed anyway), goes
/// through the SSH tunnel, checks TLS as the profile says and waits out a 429. A request is
/// stopped by dropping it: the sessions arm their cancel slot themselves.
pub struct HttpTransport {
    client: HttpClient,
}

impl HttpTransport {
    pub fn new(client: HttpClient) -> HttpTransport {
        HttpTransport { client }
    }

    /// The transport to `base` as `target` reaches it.
    pub fn for_target(
        target: &ConnectTarget<'_>,
        base: &str,
        options: HttpOptions,
        credential: Credential,
    ) -> Result<HttpTransport, AppError> {
        Ok(HttpTransport::new(HttpClient::for_target(
            target, base, options, credential,
        )?))
    }

    async fn start(&self, req: HttpRequest) -> Result<HttpReply<'static>, AppError> {
        let url = reqwest::Url::parse(&req.url)
            .map_err(|e| AppError::InvalidInput(format!("invalid URL {}: {e}", req.url)))?;
        let method = match req.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
        };
        let mut builder = self.client.request_url(method, url);
        if let Some(body) = req.body {
            builder = builder
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body);
        }
        self.client.send(builder, None).await
    }
}

fn head(reply: &HttpReply<'_>) -> (u16, Option<u64>, Option<String>) {
    (
        reply.status(),
        reply.retry_after().map(|wait| wait.as_secs()),
        reply.content_type().map(str::to_string),
    )
}

#[async_trait]
impl Transport for HttpTransport {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, AppError> {
        let mut reply = self.start(req).await?;
        let (status, retry_after, content_type) = head(&reply);
        let mut body = Vec::new();
        while let Some(piece) = reply.chunk().await? {
            if body.len() + piece.len() > MAX_REPLY {
                return Err(too_large());
            }
            body.extend_from_slice(&piece);
        }
        Ok(HttpResponse {
            status,
            retry_after,
            content_type,
            body,
        })
    }

    async fn send_streaming(&self, req: HttpRequest) -> Result<StreamingResponse, AppError> {
        let reply = self.start(req).await?;
        let (status, retry_after, content_type) = head(&reply);
        let body = reply
            .into_stream()
            .map(|piece| piece.map(|bytes| bytes.to_vec()));
        Ok(StreamingResponse {
            status,
            retry_after,
            content_type,
            body: Box::pin(body),
        })
    }
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
    use crate::drivers::common::http::RetryPolicy;
    use crate::testkit::{FakeServer, Piece, Reply, Route};

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
    fn unexpected_bodies_are_errors_with_an_excerpt() {
        let r = HttpResponse::text(200, "not json");
        let e = r.parse::<serde_json::Value>("pipeline").unwrap_err();
        assert!(e.to_string().contains("not json"), "{e}");
        let html = HttpResponse {
            content_type: Some("text/html".into()),
            ..HttpResponse::text(200, "<html><h1>502 Bad Gateway</h1></html>")
        };
        let e = html.parse::<serde_json::Value>("pipeline").unwrap_err();
        assert!(e.to_string().contains("(body: 502 Bad Gateway)"), "{e}");
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

    fn transport(server: &FakeServer, token: &str, retry: RetryPolicy) -> HttpTransport {
        crate::testkit::install_crypto();
        let client = HttpClient::new(
            crate::drivers::common::http::HttpEndpoint::new(&server.url(""), None).unwrap(),
            HttpOptions {
                retry,
                ..HttpOptions::default()
            },
            Credential::bearer(crate::drivers::common::token::TokenSource::from_secret(
                Some(token),
            )),
        )
        .unwrap();
        HttpTransport::new(client)
    }

    #[tokio::test]
    async fn the_shared_client_carries_bodies_statuses_and_streams() {
        let server = FakeServer::start(vec![
            Route::new("POST", "/echo").handler(|req| Reply::json(200, &req.json())),
            Route::new("GET", "/slow-down")
                .reply(Reply::text(429, "later").header("retry-after", "120")),
            Route::new("GET", "/stream").reply(Reply::ok().chunked(vec![
                Piece::Data(b"ab".to_vec()),
                Piece::Data(b"cd".to_vec()),
            ])),
        ])
        .await;
        let t = transport(&server, "tok", RetryPolicy::default());
        let reply = t
            .send(
                HttpRequest::post_json(server.url("/echo"), &serde_json::json!({"a": 1})).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.content_type.as_deref(), Some("application/json"));
        assert_eq!(reply.body, br#"{"a":1}"#);
        let sent = &server.requests()[0];
        assert_eq!(sent.header("authorization"), Some("Bearer tok"));
        assert_eq!(sent.header("content-type"), Some("application/json"));

        // A wait longer than the policy's is handed over with the reply.
        let reply = t
            .send(HttpRequest::get(server.url("/slow-down")))
            .await
            .unwrap();
        assert_eq!((reply.status, reply.retry_after), (429, Some(120)));

        let streamed = t
            .send_streaming(HttpRequest::get(server.url("/stream")))
            .await
            .unwrap();
        let pieces: Vec<Vec<u8>> = streamed.body.map(|p| p.unwrap()).collect::<Vec<_>>().await;
        assert_eq!(pieces.concat(), b"abcd");

        // Without a token no authorization header goes out (an open `sqld` refuses an empty
        // one), and a request to another origin never gets the token.
        let open = transport(&server, "", RetryPolicy::NONE);
        open.send(HttpRequest::get(server.url("/stream")))
            .await
            .unwrap();
        assert_eq!(
            server.requests().pop().unwrap().header("authorization"),
            None
        );
        let other = format!("http://localhost:{}/stream", server.addr().port());
        t.send(HttpRequest::get(other)).await.unwrap();
        assert_eq!(
            server.requests().pop().unwrap().header("authorization"),
            None
        );
    }
}
