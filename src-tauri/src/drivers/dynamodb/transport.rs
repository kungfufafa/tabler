//! The HTTP seam the client is written against: one signed POST, one whole reply (a DynamoDB
//! reply is JSON of at most a few MB). In Phase B the real implementation is the shared
//! `drivers::common::http::HttpClient` of step F8a (TLS, the system proxy, timeouts); the tests
//! use the in-process fake of `testing`, or a plain reqwest transport against
//! `testkit::fake_http`.

use async_trait::async_trait;

use crate::error::AppError;

/// Header names, lower case.
pub mod header {
    pub const AUTHORIZATION: &str = "authorization";
    pub const CONTENT_TYPE: &str = "content-type";
    pub const DATE: &str = "date";
    pub const AMZ_DATE: &str = "x-amz-date";
    pub const SECURITY_TOKEN: &str = "x-amz-security-token";
    pub const TARGET: &str = "x-amz-target";
    pub const REQUEST_ID: &str = "x-amzn-requestid";
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
}
