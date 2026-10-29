//! The HTTP seam the client is written against. In Phase B the real implementation is the shared
//! `drivers::common::http::HttpClient` (F8a: TLS through `tls_client`, client certificates and
//! CA pinning, SSH tunnels through `resolve`, the system proxy); the tests use the fakes of
//! `testing`, or a plain reqwest transport against `testkit::fake_http`.
//!
//! Elasticsearch needs request headers (credentials, `content-type`, `x-opaque-id`) and signs
//! with SigV4 over the exact URL and body, so requests carry both.

use async_trait::async_trait;

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

impl BaseUrl {
    /// `http(s)://host[:port][/prefix]`, `host[:port]`, IPv6 in brackets. A URL without a port
    /// takes 9200 (or 443 for `https://` with a host that is not local, as Elastic Cloud and
    /// Amazon OpenSearch serve).
    pub fn parse(text: &str) -> Result<BaseUrl, AppError> {
        let t = text.trim();
        let bad = |why: &str| AppError::InvalidInput(format!("URL \u{201c}{t}\u{201d}: {why}"));
        let (https, rest, scheme_given) = if let Some(r) = t.strip_prefix("https://") {
            (true, r, true)
        } else if let Some(r) = t.strip_prefix("http://") {
            (false, r, true)
        } else if t.contains("://") {
            return Err(bad("use http:// or https://"));
        } else {
            (false, t, false)
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        if authority.contains('@') {
            return Err(bad("put the user and password in their own fields"));
        }
        if path.contains(['?', '#']) {
            return Err(bad("a query or fragment is not allowed"));
        }
        let (host, port) = if let Some(r) = authority.strip_prefix('[') {
            let (h, after) = r
                .split_once(']')
                .ok_or_else(|| bad("unclosed IPv6 bracket"))?;
            (h.to_string(), after.strip_prefix(':'))
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), Some(p)),
                None => (authority.to_string(), None),
            }
        };
        if host.is_empty() {
            return Err(bad("no host"));
        }
        let port = match port {
            Some(p) => p.parse().map_err(|_| bad("the port is not a number"))?,
            None if https && scheme_given && !is_local(&host) => 443,
            None => 9200,
        };
        let prefix = path.trim_end_matches('/').to_string();
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

    /// The absolute URL of a request.
    pub fn url(&self, req: &EsRequest) -> String {
        format!(
            "{}://{}{}{}",
            if self.https { "https" } else { "http" },
            self.host_header(),
            self.prefix,
            req.target()
        )
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
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Sets a header, replacing one of the same name.
    pub fn set_header(&mut self, name: &str, value: impl Into<String>) {
        let name = name.to_ascii_lowercase();
        self.headers.retain(|(k, _)| *k != name);
        self.headers.push((name, value.into()));
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

    pub fn is_json(&self) -> bool {
        self.content_type
            .as_deref()
            .is_some_and(|c| c.contains("json"))
    }
}

#[async_trait]
pub trait Transport: Send + Sync {
    /// Sends `req` and reads the whole reply. Transport failures are `Connect`, `Timeout`,
    /// `NotConnected` or `Cancelled`; any HTTP status is a reply. Dropping the future drops the
    /// request, which Elasticsearch takes as a cancel of a running search.
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, AppError>;
}

/// At most this many characters of an unexpected body go into an error message.
const EXCERPT_CHARS: usize = 300;

/// A short plain-text form of a reply body for an error message: HTML loses its tags, and
/// whitespace runs collapse.
pub fn body_excerpt(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let plain = if text.trim_start().starts_with('<') {
        let mut out = String::with_capacity(text.len());
        let mut in_tag = false;
        for c in text.chars() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_urls() {
        let b = BaseUrl::parse("https://es.example.com").unwrap();
        assert_eq!((b.https, b.port, b.prefix.as_str()), (true, 443, ""));
        assert_eq!(b.host_header(), "es.example.com");
        let b = BaseUrl::parse("localhost").unwrap();
        assert_eq!((b.https, b.port), (false, 9200));
        assert_eq!(b.host_header(), "localhost:9200");
        let b = BaseUrl::parse("https://localhost").unwrap();
        assert_eq!(b.port, 9200);
        let b = BaseUrl::parse("http://[::1]:9201/proxy/es/").unwrap();
        assert_eq!(
            (b.host.as_str(), b.port, b.prefix.as_str()),
            ("::1", 9201, "/proxy/es")
        );
        let req = EsRequest::new(Method::Get, &["logs", "_search"]).param("q", "a b");
        assert_eq!(
            b.url(&req),
            "http://[::1]:9201/proxy/es/logs/_search?q=a%20b"
        );
        for bad in [
            "",
            "ftp://h",
            "http://user:pw@h",
            "http://h:x",
            "http://[::1",
            "http://h/p?x=1",
        ] {
            assert!(BaseUrl::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn request_parts_and_excerpts() {
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
        assert_eq!(body_excerpt(b"<html><h1>502</h1> Bad</html>"), "502 Bad");
        assert_eq!(
            body_excerpt("x".repeat(400).as_bytes()).chars().count(),
            EXCERPT_CHARS + 1
        );
    }
}
