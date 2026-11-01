//! The HTTP seam the client is written against. Sessions send through `HttpTransport`, the
//! shared `drivers::common::http::HttpClient` (TLS through `tls_client`, the client certificate
//! of X.509 users, the SSH tunnel, the system proxy, a Command Line password); the tests use the in-process fake of `testing`, or `HttpTransport` against
//! `testkit::fake_http`.
//!
//! ClickHouse needs what the sqlite-remote seam has no room for: request headers (the user and
//! key, the output format) and response headers (`X-ClickHouse-Summary`, `-Exception-Code`,
//! `-Exception-Tag`, `-Format`, `-Timezone`, `-Query-Id`).

use std::pin::Pin;

use async_trait::async_trait;
use futures::{Stream, StreamExt as _};

use crate::drivers::common::http::{HttpClient, Repeat};
use crate::drivers::common::token::TokenSource;
use crate::error::AppError;

use super::format::{code, parse_exception};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// Header names, lower case.
pub mod header {
    pub const USER: &str = "x-clickhouse-user";
    pub const KEY: &str = "x-clickhouse-key";
    /// HTTP Basic, for a password a header value cannot carry.
    pub const AUTHORIZATION: &str = "authorization";
    pub const CERT_AUTH: &str = "x-clickhouse-ssl-certificate-auth";
    pub const FORMAT: &str = "x-clickhouse-format";
    pub const SUMMARY: &str = "x-clickhouse-summary";
    pub const EXCEPTION_CODE: &str = "x-clickhouse-exception-code";
    pub const EXCEPTION_TAG: &str = "x-clickhouse-exception-tag";
    pub const TIMEZONE: &str = "x-clickhouse-timezone";
    pub const QUERY_ID: &str = "x-clickhouse-query-id";
    pub const SERVER_NAME: &str = "x-clickhouse-server-display-name";
}

#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: Method,
    /// Absolute, with the query string.
    pub url: String,
    /// Names lower case.
    pub headers: Vec<(String, String)>,
    /// The SQL text (POST).
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        find(&self.headers, name)
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

    /// The decoded query parameters, in order.
    pub fn query_pairs(&self) -> Vec<(String, String)> {
        let Some((_, q)) = self.url.split_once('?') else {
            return Vec::new();
        };
        q.split('&')
            .filter(|p| !p.is_empty())
            .map(|p| {
                let (k, v) = p.split_once('=').unwrap_or((p, ""));
                (decode_component(k), decode_component(v))
            })
            .collect()
    }

    /// The last value of query parameter `name`.
    pub fn param(&self, name: &str) -> Option<String> {
        self.query_pairs()
            .into_iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v)
    }

    pub fn sql(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// The key never shows in logs or panics.
impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str(),
                    if k == header::KEY || k == header::AUTHORIZATION {
                        "***"
                    } else {
                        v.as_str()
                    },
                )
            })
            .collect();
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
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

/// `%XX` and `+` decoded; invalid escapes stay as written.
pub fn decode_component(s: &str) -> String {
    percent_encoding::percent_decode_str(&s.replace('+', " "))
        .decode_utf8_lossy()
        .into_owned()
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResponseHead {
    pub status: u16,
    /// Names lower case.
    pub headers: Vec<(String, String)>,
}

impl ResponseHead {
    pub fn header(&self, name: &str) -> Option<&str> {
        find(&self.headers, name)
    }
}

/// A body that arrives in pieces.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>, AppError>> + Send>>;

pub struct StreamingResponse {
    pub head: ResponseHead,
    pub body: ByteStream,
}

/// How much of an error reply is read: its message comes first (and only 300 characters of a
/// reply that is not ClickHouse's show), and a server or a proxy may send an endless one.
pub const ERROR_BODY_LIMIT: usize = 64 * 1024;

impl StreamingResponse {
    /// A reply whose body is `body`, in one piece.
    pub fn whole(head: ResponseHead, body: Vec<u8>) -> StreamingResponse {
        StreamingResponse {
            head,
            body: Box::pin(futures::stream::iter([Ok(body)])),
        }
    }

    /// The body (an error reply, a small answer) cut at `limit` bytes; the rest is never read
    /// (dropping the stream closes the connection). A transport error mid-body ends it.
    pub async fn collect(self, limit: usize) -> Result<(ResponseHead, Vec<u8>), AppError> {
        let mut body = Vec::new();
        let mut pieces = self.body;
        while body.len() < limit {
            let Some(piece) = pieces.next().await else {
                break;
            };
            body.extend_from_slice(&piece?);
        }
        body.truncate(limit);
        Ok((self.head, body))
    }
}

#[async_trait]
pub trait Transport: Send + Sync {
    /// Sends `req` and hands the body over as it arrives. Transport failures are `Connect`,
    /// `Timeout`, `NotConnected` or `Cancelled`; any HTTP status is a reply. Dropping the body
    /// closes the connection, which stops a read-only query on the server
    /// (`cancel_http_readonly_queries_on_client_close`).
    async fn send(&self, req: HttpRequest) -> Result<StreamingResponse, AppError>;
}

/// The sessions' transport: the shared HTTP client of the profile's server. A statement's POST
/// is sent once on a server error (`Repeat::Safe`: it may have run), again only after a 429, or
/// after a refused password when a Command Line password can bring a new one (`refreshing`).
/// The session stops a request by dropping it (and `KILL QUERY`), so none is sent with a cancel
/// slot here.
pub struct HttpTransport {
    client: HttpClient,
    /// The source of the `X-ClickHouse-Key` the client sends, when it can refresh.
    refresh: Option<TokenSource>,
}

impl HttpTransport {
    pub fn new(client: HttpClient) -> HttpTransport {
        HttpTransport {
            client,
            refresh: None,
        }
    }

    /// Runs `token` again (once per request) when the server refuses the password it gave.
    /// ClickHouse refuses one with HTTP 403 (`AUTHENTICATION_FAILED`, `WRONG_PASSWORD`; only
    /// `REQUIRED_PASSWORD` is a 401), which the shared client's refresh on a 401 never sees.
    pub fn refreshing(mut self, token: Option<TokenSource>) -> HttpTransport {
        self.refresh = token.filter(TokenSource::refreshes);
        self
    }

    async fn send_once(&self, req: HttpRequest) -> Result<StreamingResponse, AppError> {
        let url = reqwest::Url::parse(&req.url)
            .map_err(|e| AppError::InvalidInput(format!("invalid URL {}: {e}", req.url)))?;
        let method = match req.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
        };
        let mut builder = self.client.request_url(method, url);
        for (k, v) in &req.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        if !req.body.is_empty() {
            builder = builder.body(req.body);
        }
        let reply = self
            .client
            .send_repeating(builder, None, Repeat::Safe)
            .await?;
        let head = ResponseHead {
            status: reply.status(),
            // `HeaderName`s are lower case already; a value that is no text is kept lossily.
            headers: reply
                .headers()
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().to_string(),
                        String::from_utf8_lossy(v.as_bytes()).into_owned(),
                    )
                })
                .collect(),
        };
        let body = reply
            .into_stream()
            .map(|piece| piece.map(|bytes| bytes.to_vec()));
        Ok(StreamingResponse {
            head,
            body: Box::pin(body),
        })
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn send(&self, req: HttpRequest) -> Result<StreamingResponse, AppError> {
        let Some(token) = &self.refresh else {
            return self.send_once(req).await;
        };
        let resp = self.send_once(req.clone()).await?;
        if resp.head.status != 403 {
            return Ok(resp);
        }
        let (head, body) = resp.collect(ERROR_BODY_LIMIT).await?;
        if !refused_password(&head, &body) {
            return Ok(StreamingResponse::whole(head, body));
        }
        // Our own sign-in was refused before the query started, so sending it again runs it once.
        token.invalidate().await;
        self.send_once(req).await
    }
}

/// Whether a 403 is ClickHouse refusing our password before the query started (not the user
/// name or the address). A 516 or 193 can also come from a running query reaching another
/// server (`remote()`, a Distributed insert, a dictionary source), after it may have written
/// rows: the server sets `X-ClickHouse-Query-Id` once the query starts, after the sign-in, and
/// a remote's exception says "Received from"; either one means the statement must not go again.
fn refused_password(head: &ResponseHead, body: &[u8]) -> bool {
    if head.header(header::QUERY_ID).is_some() {
        return false;
    }
    let text = String::from_utf8_lossy(body);
    if text.contains("Received from") {
        return false;
    }
    let code = parse_exception(&text).code.or_else(|| {
        head.header(header::EXCEPTION_CODE)
            .and_then(|c| c.trim().parse().ok())
    });
    matches!(
        code,
        Some(code::AUTHENTICATION_FAILED | code::WRONG_PASSWORD)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_decode_and_keys_stay_hidden() {
        let req = HttpRequest {
            method: Method::Post,
            url: "http://h:8123/?database=a%20b&param_p1=x%2By+z&bad=%zz&end=%4".into(),
            headers: vec![
                (header::USER.into(), "u".into()),
                (header::KEY.into(), "secret".into()),
            ],
            body: b"SELECT 1".to_vec(),
        };
        assert_eq!(req.path(), "/");
        assert_eq!(req.param("database").as_deref(), Some("a b"));
        assert_eq!(req.param("param_p1").as_deref(), Some("x+y z"));
        assert_eq!(req.param("bad").as_deref(), Some("%zz"));
        assert_eq!(req.param("end").as_deref(), Some("%4"));
        assert_eq!(req.header("X-ClickHouse-User"), Some("u"));
        let shown = format!("{req:?}");
        assert!(
            !shown.contains("secret") && shown.contains("***"),
            "{shown}"
        );
        assert_eq!(req.sql(), "SELECT 1");
    }
}
