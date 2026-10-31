//! The HTTP seam the client is written against, and the cluster's address. The real transport is
//! the shared `HttpClient` ([`HttpTransport`]): TLS through `tls_client` with the CA file, the CA
//! fingerprint pin and the client certificate, SSH tunnels through `resolve`, the credential
//! (Basic, API key, bearer, SigV4 signed after the headers are final), retries of rejected
//! requests. The tests use `testing::FakeEs` behind the same seam, or serve it over HTTP.
//!
//! Requests carry their headers (`content-type`, `x-opaque-id`) and body as built, so a SigV4
//! signature covers them as sent. Dropping a request's future closes its connection, which is how
//! Elasticsearch learns that a search was cancelled.

use async_trait::async_trait;

use crate::drivers::common::http::HttpClient;
use crate::error::AppError;

use super::console::{EsRequest, Method};

/// The cluster's address: `https://host:9200` plus a path prefix (a cluster behind a reverse
/// proxy at `https://proxy/es`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseUrl {
    pub https: bool,
    pub host: String,
    pub port: u16,
    /// Without a trailing `/`; empty or starting with `/`.
    pub prefix: String,
}

/// The connection URL schemes of the driver (TablePlus's `elasticsearch://`, and OpenSearch's):
/// HTTP or HTTPS as the profile's SSL mode says.
pub const URL_SCHEMES: &[&str] = &["elasticsearch", "opensearch"];

impl BaseUrl {
    /// `http(s)://host[:port][/prefix]`, `elasticsearch://…` or `opensearch://…`, or
    /// `host[:port]`, IPv6 in brackets. `https` decides HTTP or HTTPS where the scheme does not
    /// (the profile's SSL mode). A URL without a port takes 9200 (or 443 for `https://` with a
    /// host that is not local, as Elastic Cloud and Amazon OpenSearch serve).
    pub fn parse(text: &str, https: bool) -> Result<BaseUrl, AppError> {
        let t = text.trim();
        let bad = |why: &str| AppError::InvalidInput(format!("URL \u{201c}{t}\u{201d}: {why}"));
        let (https, rest, explicit_https) = match t.split_once("://") {
            Some((scheme, rest)) => match scheme.to_ascii_lowercase().as_str() {
                "https" => (true, rest, true),
                "http" => (false, rest, false),
                s if URL_SCHEMES.contains(&s) => (https, rest, false),
                _ => {
                    return Err(bad(
                        "use http://, https://, elasticsearch:// or opensearch://",
                    ));
                }
            },
            None => (https, t, false),
        };
        // Also where no path follows the host (`https://h?x`, `h#f`).
        if rest.contains(['?', '#']) {
            return Err(bad("a query or fragment is not allowed"));
        }
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        if authority.contains('@') {
            return Err(bad("put the user and password in their own fields"));
        }
        let (host, port) = if let Some(r) = authority.strip_prefix('[') {
            let (h, after) = r
                .split_once(']')
                .ok_or_else(|| bad("unclosed IPv6 bracket"))?;
            let port = match after {
                "" => None,
                a => Some(
                    a.strip_prefix(':')
                        .ok_or_else(|| bad("the port is not a number"))?,
                ),
            };
            (h.to_string(), port)
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), Some(p)),
                None => (authority.to_string(), None),
            }
        };
        if host.is_empty() {
            return Err(bad("no host"));
        }
        if host.chars().any(|c| {
            c.is_whitespace() || c.is_control() || matches!(c, '\\' | '%' | '?' | '#' | '@')
        }) {
            return Err(bad("the host holds characters a host name cannot"));
        }
        let port = match port {
            Some(p) => p.parse().map_err(|_| bad("the port is not a number"))?,
            None if explicit_https && !is_local(&host) => 443,
            None => 9200,
        };
        // The rule of the `pathPrefix` param: every request goes under the prefix, the connect's
        // `GET /` too, so `https://es/_refresh` would refresh every index on a read-only profile.
        let prefix = path.trim_end_matches('/').to_string();
        if !prefix.is_empty() {
            super::params::check_prefix(&prefix)?;
        }
        Ok(BaseUrl {
            https,
            host,
            port,
            prefix,
        })
    }

    /// The host as it goes into a URL or `Host` header (IPv6 in brackets).
    pub fn host_for_url(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        }
    }

    /// The `Host` header: the port only when it is not the scheme's default.
    pub fn host_header(&self) -> String {
        let default = if self.https { 443 } else { 80 };
        if self.port == default {
            self.host_for_url()
        } else {
            format!("{}:{}", self.host_for_url(), self.port)
        }
    }

    /// `scheme://host[:port]/prefix`, what the HTTP client's endpoint is.
    pub fn root(&self) -> String {
        format!(
            "{}://{}{}",
            if self.https { "https" } else { "http" },
            self.host_header(),
            self.prefix
        )
    }

    /// The absolute URL of a request.
    pub fn url(&self, req: &EsRequest) -> String {
        format!("{}{}", self.root(), req.target())
    }
}

fn is_local(host: &str) -> bool {
    host == "localhost" || host == "127.0.0.1" || host == "::1"
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: Method,
    /// Absolute, with the query string.
    pub url: String,
    /// Names lower case.
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

impl HttpRequest {
    /// Sets a header, replacing one of the same name.
    pub fn set_header(&mut self, name: &str, value: impl Into<String>) {
        let name = name.to_ascii_lowercase();
        self.headers.retain(|(k, _)| *k != name);
        self.headers.push((name, value.into()));
    }
}

/// What the fakes and the tests read of a request.
#[cfg(test)]
impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The path of `url` (encoded), without scheme, host or query.
    pub fn path(&self) -> &str {
        let rest = self
            .url
            .split_once("://")
            .map_or(self.url.as_str(), |(_, r)| r);
        let path = rest.find('/').map_or("/", |i| &rest[i..]);
        path.split(['?', '#']).next().unwrap_or("/")
    }

    /// The query string of `url` (encoded), if any.
    pub fn query(&self) -> Option<&str> {
        let q = self.url.split_once('?')?.1;
        Some(q.split('#').next().unwrap_or(q))
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(self.body.as_deref().unwrap_or_default()).into_owned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HttpResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// The fakes' replies.
#[cfg(test)]
impl HttpResponse {
    pub fn json(status: u16, body: &str) -> HttpResponse {
        HttpResponse {
            status,
            content_type: Some("application/json".into()),
            body: body.as_bytes().to_vec(),
        }
    }

    pub fn text(status: u16, body: &str) -> HttpResponse {
        HttpResponse {
            status,
            content_type: Some("text/plain; charset=UTF-8".into()),
            body: body.as_bytes().to_vec(),
        }
    }
}

#[async_trait]
pub trait Transport: Send + Sync {
    /// Sends `req` and reads its reply. Transport failures are `Connect`, `Timeout`,
    /// `NotConnected` or `Auth` (a credential that could not be made); any HTTP status is a
    /// reply. Dropping the future drops the request, which Elasticsearch takes as a cancel of a
    /// running search.
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, AppError>;
}

/// The largest reply body read: a broken or hostile server (or anyone in the middle of an
/// unchecked HTTPS connection) could send one without end.
const MAX_BODY: usize = 256 * 1024 * 1024;
/// The part of an error reply (4xx, 5xx) that is read; its status says what went wrong.
const MAX_ERROR_BODY: usize = 1024 * 1024;

/// The shared HTTP client as the transport. Every request goes to the URL as written
/// (`request_exact`: a path the URL parser would change is refused rather than sent elsewhere),
/// with its credential added last. Reply bodies are read up to a limit.
pub struct HttpTransport {
    client: HttpClient,
    max_body: usize,
    max_error_body: usize,
}

impl HttpTransport {
    pub fn new(client: HttpClient) -> HttpTransport {
        HttpTransport {
            client,
            max_body: MAX_BODY,
            max_error_body: MAX_ERROR_BODY,
        }
    }

    /// Smaller limits, so a test need not send hundreds of MiB.
    #[cfg(test)]
    pub fn with_limits(mut self, body: usize, error_body: usize) -> HttpTransport {
        self.max_body = body;
        self.max_error_body = error_body;
        self
    }
}

fn http_method(m: Method) -> reqwest::Method {
    match m {
        Method::Get => reqwest::Method::GET,
        Method::Head => reqwest::Method::HEAD,
        Method::Post => reqwest::Method::POST,
        Method::Put => reqwest::Method::PUT,
        Method::Delete => reqwest::Method::DELETE,
        Method::Patch => reqwest::Method::PATCH,
        Method::Options => reqwest::Method::OPTIONS,
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, AppError> {
        let mut builder = self
            .client
            .request_exact(http_method(req.method), &req.url)?;
        for (name, value) in &req.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        if let Some(body) = req.body {
            builder = builder.body(body);
        }
        let mut reply = self.client.send(builder, None).await?;
        let status = reply.status();
        let content_type = reply.content_type().map(str::to_string);
        let mut body = Vec::new();
        while let Some(chunk) = reply.chunk().await? {
            if status >= 400 {
                // The start of an error reply is enough to name the error.
                let room = self.max_error_body.saturating_sub(body.len());
                body.extend_from_slice(chunk.get(..room).unwrap_or(&chunk[..]));
                if body.len() >= self.max_error_body {
                    break;
                }
            } else if body.len() + chunk.len() > self.max_body {
                return Err(AppError::db(format!(
                    "the reply is larger than {} MiB: Tabler stopped reading it (ask for less, \
                     with a smaller size or fewer fields)",
                    self.max_body / (1024 * 1024)
                )));
            } else {
                body.extend_from_slice(&chunk);
            }
        }
        Ok(HttpResponse {
            status,
            content_type,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_urls() {
        let b = BaseUrl::parse("https://es.example.com", false).unwrap();
        assert_eq!((b.https, b.port, b.prefix.as_str()), (true, 443, ""));
        assert_eq!(b.host_header(), "es.example.com");
        assert_eq!(b.root(), "https://es.example.com");
        let b = BaseUrl::parse("localhost", false).unwrap();
        assert_eq!((b.https, b.port), (false, 9200));
        assert_eq!(b.host_header(), "localhost:9200");
        assert!(BaseUrl::parse("localhost", true).unwrap().https);
        let b = BaseUrl::parse("https://localhost", false).unwrap();
        assert_eq!(b.port, 9200);
        let b = BaseUrl::parse("http://[::1]:9201/proxy/es/", true).unwrap();
        assert_eq!(
            (b.https, b.host.as_str(), b.port, b.prefix.as_str()),
            (false, "::1", 9201, "/proxy/es")
        );
        let req = EsRequest::new(Method::Get, &["logs", "_search"]).param("q", "a b");
        assert_eq!(
            b.url(&req),
            "http://[::1]:9201/proxy/es/logs/_search?q=a%20b"
        );
        // The connection URL schemes take TLS from the profile.
        for (url, https) in [
            ("elasticsearch://es1:9201", false),
            ("OpenSearch://es1", true),
        ] {
            let b = BaseUrl::parse(url, https).unwrap();
            assert_eq!((b.https, b.host.as_str()), (https, "es1"), "{url}");
            assert!(b.port == 9201 || b.port == 9200);
        }
        for bad in [
            "",
            "ftp://h",
            "http://user:pw@h",
            "http://h:x",
            "http://[::1",
            "http://h/p?x=1",
            "http://h/a/../_bulk",
            "http://h/%2e%2e",
            "http://h\\x",
            // A query or fragment where no path follows, a user at the host.
            "es.example.com?x",
            "https://h#f",
            "http://h:9200?x",
            "http://[::1]?x",
            "http://[::1]x",
            "h@",
            // An endpoint name under the prefix: every request would call it.
            "http://h/_refresh",
            "127.0.0.1/_flush",
            "https://h/es/_security/enroll/kibana",
            "http://h/a b",
        ] {
            assert!(BaseUrl::parse(bad, false).is_err(), "{bad}");
        }
    }

    #[test]
    fn request_parts() {
        let mut r = HttpRequest {
            method: Method::Post,
            url: "https://h:9200/a/_search?x=1".into(),
            headers: vec![],
            body: Some(b"{}".to_vec()),
        };
        assert_eq!(r.path(), "/a/_search");
        assert_eq!(r.query(), Some("x=1"));
        r.set_header("Content-Type", "a");
        r.set_header("content-type", "b");
        assert_eq!(
            r.headers,
            vec![("content-type".to_string(), "b".to_string())]
        );
    }
}
