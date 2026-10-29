//! The HTTP seam the client is written against. In Phase B the real implementation is the shared
//! `drivers::common::http::HttpClient` (F8a: TLS through `tls_client`, client certificates,
//! tunnels through `resolve`, the system proxy); the tests use the in-process fake of
//! `testing`, or a plain reqwest transport against `testkit::fake_http`.
//!
//! ClickHouse needs what the sqlite-remote seam has no room for: request headers (the user and
//! key, the output format) and response headers (`X-ClickHouse-Summary`, `-Exception-Code`,
//! `-Exception-Tag`, `-Format`, `-Timezone`, `-Query-Id`).

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// Header names, lower case.
pub mod header {
    pub const USER: &str = "x-clickhouse-user";
    pub const KEY: &str = "x-clickhouse-key";
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
                    if k == header::KEY { "***" } else { v.as_str() },
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

impl StreamingResponse {
    /// The whole body (an error reply, a small answer). A transport error mid-body ends it.
    pub async fn collect(self) -> Result<(ResponseHead, Vec<u8>), AppError> {
        use futures::StreamExt as _;
        let mut body = Vec::new();
        let mut pieces = self.body;
        while let Some(piece) = pieces.next().await {
            body.extend_from_slice(&piece?);
        }
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
