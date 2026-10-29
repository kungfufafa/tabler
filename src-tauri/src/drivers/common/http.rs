//! The HTTP client of the HTTP drivers (LibSQL, D1, ClickHouse, BigQuery, Elasticsearch,
//! DynamoDB): reqwest on the `tls_client` config, an SSH tunnel that keeps the server's name for
//! the Host header, SNI and the certificate check, credentials on the endpoint's own requests
//! only, one token refresh after a 401, backoff on 429 and 503, and a cancel slot that stops the
//! request and the reading of its reply.
//!
//! ```ignore
//! let client = HttpClient::for_target(&target, "https://db.example.com:8443", options, cred)?;
//! let mut reply = client.send(client.post("/query")?.json(&body), Some(&slot)).await?;
//! while let Some(chunk) = reply.chunk().await? { … }
//! ```

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use futures::future::BoxFuture;
use futures::stream::{BoxStream, StreamExt as _};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HOST, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, RequestBuilder, Url};
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;

use super::token::TokenSource;
use crate::drivers::{
    ArmedCancel, CancelAction, CancelHandle, CancelSlot, ConnectTarget, arm_cancel,
};
use crate::error::AppError;
use crate::tls_files::ClientCertificate;
use crate::types::profile::{ConnectionProfile, DEFAULT_CONNECT_TIMEOUT_SECS, SslMode};

/// Where a driver's server is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpEndpoint {
    /// The server's own URL (what the profile names, never the tunnel's), with a path prefix
    /// when the server sits under one.
    pub base: Url,
    /// The host of `base`: the Host header, SNI and the certificate check use it.
    pub real_host: String,
    /// The local end of the SSH tunnel to `base`'s host and port.
    pub tunnel: Option<SocketAddr>,
}

impl HttpEndpoint {
    pub fn new(base: &str, tunnel: Option<SocketAddr>) -> Result<HttpEndpoint, AppError> {
        let base = Url::parse(base.trim())
            .map_err(|e| AppError::InvalidInput(format!("invalid server URL {base}: {e}")))?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(AppError::InvalidInput(format!(
                "the server URL must start with http:// or https:// (not {}://)",
                base.scheme()
            )));
        }
        let real_host = base
            .host_str()
            .filter(|h| !h.is_empty())
            .ok_or_else(|| AppError::InvalidInput(format!("the server URL {base} has no host")))?
            .to_string();
        Ok(HttpEndpoint {
            base,
            real_host,
            tunnel,
        })
    }

    /// `base` for a connect of `t`: through its SSH tunnel when it has one (the tunnel forwards
    /// to the profile's host and port, which `base` must name).
    pub fn for_target(t: &ConnectTarget<'_>, base: &str) -> Result<HttpEndpoint, AppError> {
        let tunnel = match &t.host_override {
            None => None,
            Some((host, port)) => {
                let ip = match host.parse::<IpAddr>() {
                    Ok(ip) => ip,
                    Err(_) if host.eq_ignore_ascii_case("localhost") => {
                        IpAddr::V4(Ipv4Addr::LOCALHOST)
                    }
                    Err(_) => {
                        return Err(AppError::internal(format!(
                            "the tunnel listens on {host}, which is not an address"
                        )));
                    }
                };
                Some(SocketAddr::new(ip, *port))
            }
        };
        HttpEndpoint::new(base, tunnel)
    }

    fn is_https(&self) -> bool {
        self.base.scheme() == "https"
    }

    /// Whether `url` goes to this server (same scheme, host and port): only those requests get
    /// the credential and go through the tunnel.
    fn owns(&self, url: &Url) -> bool {
        url.scheme() == self.base.scheme()
            && url.host_str() == self.base.host_str()
            && url.port_or_known_default() == self.base.port_or_known_default()
    }
}

/// TLS for an `https` endpoint (an `http` one ignores it).
#[derive(Debug)]
pub struct HttpTls {
    pub mode: SslMode,
    /// A PEM file of CA certificates trusted next to the system's.
    pub ca_path: Option<String>,
    /// The client certificate for servers that ask for one (ClickHouse X.509 users).
    pub identity: Option<ClientCertificate>,
    /// SHA-256 of a certificate the server must present (an Elasticsearch CA fingerprint).
    pub pin_sha256: Option<[u8; 32]>,
}

impl HttpTls {
    /// The chain and the name checked against the system's roots: what cloud APIs need.
    pub fn verified() -> HttpTls {
        HttpTls {
            mode: SslMode::VerifyFull,
            ca_path: None,
            identity: None,
            pin_sha256: None,
        }
    }

    /// The profile's SSL mode, CA file and client certificate (Key… / Cert…).
    pub fn from_profile(p: &ConnectionProfile) -> Result<HttpTls, AppError> {
        Ok(HttpTls {
            mode: p.ssl,
            ca_path: p.ssl_ca_path.clone(),
            identity: crate::tls_files::load_client_certificate(
                p.options.ssl_key_path.as_deref(),
                p.options.ssl_cert_path.as_deref(),
            )?,
            pin_sha256: None,
        })
    }
}

impl Default for HttpTls {
    fn default() -> HttpTls {
        HttpTls::verified()
    }
}

/// What `send` does with a 429 or 503.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// How many times such a reply is retried; 0 hands it to the caller, with its
    /// `HttpReply::retry_after`.
    pub max_retries: u32,
    /// The first wait when the server names none; it doubles each time, with jitter.
    pub base_delay: Duration,
    /// The longest wait: a server asking for more gets its reply handed to the caller.
    pub max_delay: Duration,
}

impl RetryPolicy {
    pub const NONE: RetryPolicy = RetryPolicy {
        max_retries: 0,
        base_delay: Duration::ZERO,
        max_delay: Duration::ZERO,
    };

    /// The wait before retry `n` (0-based) without a `Retry-After`: `base_delay · 2ⁿ`, between half
    /// and all of it (jitter), at most `max_delay`.
    fn backoff(&self, n: u32) -> Duration {
        let full = self
            .base_delay
            .saturating_mul(1u32.checked_shl(n.min(16)).unwrap_or(u32::MAX))
            .min(self.max_delay);
        let mut r = [0u8; 2];
        let _ = aws_lc_rs::rand::fill(&mut r);
        let fraction = 0.5 + f64::from(u16::from_le_bytes(r)) / f64::from(u16::MAX) / 2.0;
        full.mul_f64(fraction)
    }
}

impl Default for RetryPolicy {
    fn default() -> RetryPolicy {
        RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(20),
        }
    }
}

#[derive(Debug)]
pub struct HttpOptions {
    pub tls: HttpTls,
    pub connect_timeout: Duration,
    /// The whole request, reply body included; `None` for no limit (long streamed results, where
    /// the cancel slot ends a request).
    pub request_timeout: Option<Duration>,
    pub retry: RetryPolicy,
}

impl Default for HttpOptions {
    fn default() -> HttpOptions {
        HttpOptions {
            tls: HttpTls::verified(),
            connect_timeout: Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS),
            request_timeout: None,
            retry: RetryPolicy::default(),
        }
    }
}

impl HttpOptions {
    /// The profile's TLS settings and connect timeout.
    pub fn for_profile(p: &ConnectionProfile) -> Result<HttpOptions, AppError> {
        Ok(HttpOptions {
            tls: HttpTls::from_profile(p)?,
            connect_timeout: p
                .options
                .connect_timeout_secs
                .filter(|s| *s > 0)
                .map_or(Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS), |s| {
                    Duration::from_secs(u64::from(s))
                }),
            ..HttpOptions::default()
        })
    }
}

/// How requests to the endpoint prove who sends them. A token that is empty sends nothing.
pub enum Credential {
    None,
    /// `Authorization: Bearer <token>`.
    Bearer(TokenSource),
    /// `Authorization: Basic base64(<user>:<password>)`.
    Basic(String, TokenSource),
    /// `<header>: <token>` (`X-ClickHouse-Key`).
    Header(&'static str, TokenSource),
    /// `Authorization: <scheme> <token>` (Elasticsearch's `ApiKey`).
    Scheme(&'static str, TokenSource),
    /// AWS Signature Version 4.
    #[cfg(feature = "common-aws")]
    Sigv4(super::aws::Signer),
}

/// Secrets never show in logs or panics.
impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Credential::None => f.write_str("Credential::None"),
            Credential::Bearer(t) => write!(f, "Credential::Bearer({t:?})"),
            Credential::Basic(user, t) => write!(f, "Credential::Basic({user:?}, {t:?})"),
            Credential::Header(name, t) => write!(f, "Credential::Header({name:?}, {t:?})"),
            Credential::Scheme(scheme, t) => write!(f, "Credential::Scheme({scheme:?}, {t:?})"),
            #[cfg(feature = "common-aws")]
            Credential::Sigv4(s) => write!(f, "Credential::Sigv4({s:?})"),
        }
    }
}

impl Credential {
    /// A bearer token, or nothing when there is none.
    pub fn bearer(token: TokenSource) -> Credential {
        match token {
            TokenSource::None => Credential::None,
            token => Credential::Bearer(token),
        }
    }

    /// Whether a refused request can be retried with a fresh secret.
    fn refreshes(&self) -> bool {
        match self {
            Credential::None => false,
            Credential::Bearer(t)
            | Credential::Basic(_, t)
            | Credential::Header(_, t)
            | Credential::Scheme(_, t) => t.refreshes(),
            #[cfg(feature = "common-aws")]
            Credential::Sigv4(s) => s.refreshes(),
        }
    }

    async fn invalidate(&self) {
        match self {
            Credential::None => {}
            Credential::Bearer(t)
            | Credential::Basic(_, t)
            | Credential::Header(_, t)
            | Credential::Scheme(_, t) => t.invalidate().await,
            #[cfg(feature = "common-aws")]
            Credential::Sigv4(s) => s.invalidate().await,
        }
    }

    async fn apply(&self, req: &mut reqwest::Request) -> Result<(), AppError> {
        // Tokens pasted with a line break or spaces around them would not be a header.
        async fn token(t: &TokenSource) -> Result<Option<String>, AppError> {
            Ok(t.get()
                .await?
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty()))
        }
        let (name, value) = match self {
            Credential::None => return Ok(()),
            Credential::Bearer(t) => (
                AUTHORIZATION,
                token(t).await?.map(|t| format!("Bearer {t}")),
            ),
            Credential::Scheme(scheme, t) => (
                AUTHORIZATION,
                token(t).await?.map(|t| format!("{scheme} {t}")),
            ),
            Credential::Basic(user, t) => {
                let password = t.get().await?.unwrap_or_default();
                let value = (!user.is_empty() || !password.is_empty()).then(|| {
                    let pair = format!("{user}:{password}");
                    format!(
                        "Basic {}",
                        base64::engine::general_purpose::STANDARD.encode(pair)
                    )
                });
                (AUTHORIZATION, value)
            }
            Credential::Header(name, t) => {
                let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                    AppError::internal(format!("{name} is not an HTTP header name"))
                })?;
                (name, token(t).await?)
            }
            #[cfg(feature = "common-aws")]
            Credential::Sigv4(signer) => return signer.sign(req).await,
        };
        if let Some(value) = value {
            let mut value = HeaderValue::from_str(&value).map_err(|_| {
                AppError::InvalidInput(
                    "the token holds characters an HTTP header cannot carry".into(),
                )
            })?;
            value.set_sensitive(true);
            req.headers_mut().insert(name, value);
        }
        Ok(())
    }
}

/// How requests to the endpoint reach a tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Direct,
    /// A DNS name: the client resolves it to the tunnel (`ClientBuilder::resolve`). The URL loses
    /// its port, which would win over the tunnel's; the Host header keeps it.
    Resolve,
    /// An IP address, which no resolver is asked about: the URL names the tunnel, and the
    /// certificate is checked for the server's address (`tls_client::rustls_config_through`).
    Rewrite(SocketAddr),
}

/// The client of one session's server.
pub struct HttpClient {
    client: reqwest::Client,
    endpoint: HttpEndpoint,
    route: Route,
    credential: Credential,
    retry: RetryPolicy,
}

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient")
            .field("endpoint", &self.endpoint)
            .field("route", &self.route)
            .field("credential", &self.credential)
            .finish_non_exhaustive()
    }
}

impl HttpClient {
    /// The client for `base` as `t` reaches it (see [`HttpEndpoint::for_target`]).
    pub fn for_target(
        t: &ConnectTarget<'_>,
        base: &str,
        o: HttpOptions,
        c: Credential,
    ) -> Result<HttpClient, AppError> {
        HttpClient::new(HttpEndpoint::for_target(t, base)?, o, c)
    }

    pub fn new(
        endpoint: HttpEndpoint,
        o: HttpOptions,
        c: Credential,
    ) -> Result<HttpClient, AppError> {
        let route = match endpoint.tunnel {
            None => Route::Direct,
            Some(tunnel) => match host_ip(&endpoint.base) {
                None => Route::Resolve,
                Some(_) => Route::Rewrite(tunnel),
            },
        };
        let mut builder = reqwest::Client::builder()
            .connect_timeout(o.connect_timeout)
            // A driver's server does not redirect; following one would carry its credential.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("Tabler/", env!("CARGO_PKG_VERSION")));
        if let Some(limit) = o.request_timeout {
            builder = builder.timeout(limit);
        }
        // A proxy cannot reach this machine's loopback, nor the tunnel's server by its name.
        if endpoint.tunnel.is_some() || is_loopback(&endpoint.base) {
            builder = builder.no_proxy();
        }
        if let (Route::Resolve, Some(tunnel)) = (route, endpoint.tunnel) {
            builder = builder.resolve(&endpoint.real_host, tunnel);
        }
        if endpoint.is_https() {
            let redirect = match route {
                Route::Rewrite(tunnel) => Some((
                    tunnel.ip(),
                    crate::tls_client::server_name(&endpoint.real_host)?,
                )),
                _ => None,
            };
            let tls = crate::tls_client::rustls_config_through(
                o.tls.mode,
                o.tls.ca_path.as_deref(),
                o.tls.identity,
                &[b"http/1.1"],
                o.tls.pin_sha256,
                redirect,
            )?;
            builder = builder.tls_backend_preconfigured(tls);
        }
        let client = builder
            .build()
            .map_err(|e| AppError::Connect(format!("HTTP client: {}", error_text(&e))))?;
        Ok(HttpClient {
            client,
            endpoint,
            route,
            credential: c,
            retry: o.retry,
        })
    }

    pub fn endpoint(&self) -> &HttpEndpoint {
        &self.endpoint
    }

    /// `path` (with a query, if any) under the endpoint's base path.
    pub fn url(&self, path: &str) -> Result<Url, AppError> {
        let mut url = self.endpoint.base.clone();
        let (path, query) = match path.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (path, None),
        };
        let joined = format!(
            "{}/{}",
            url.path().trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        url.set_path(&joined);
        url.set_query(query);
        Ok(url)
    }

    /// A request for `path` under the endpoint; `send` it.
    pub fn request(&self, method: Method, path: &str) -> Result<RequestBuilder, AppError> {
        Ok(self.request_url(method, self.url(path)?))
    }

    /// A request for any URL; only the endpoint's own get its credential.
    pub fn request_url(&self, method: Method, url: Url) -> RequestBuilder {
        self.client.request(method, url)
    }

    pub fn get(&self, path: &str) -> Result<RequestBuilder, AppError> {
        self.request(Method::GET, path)
    }

    pub fn post(&self, path: &str) -> Result<RequestBuilder, AppError> {
        self.request(Method::POST, path)
    }

    /// Sends `req` and returns the reply once its head has arrived; any status is a reply. With
    /// `cancel`, the slot stops the request, the waits between retries and the reading of the
    /// reply until the reply is dropped. A 401 from a credential that can bring a fresh secret
    /// is retried once with it; a 429 or 503 as the retry policy says. A request whose body is a
    /// stream is sent once.
    pub async fn send<'a>(
        &self,
        req: RequestBuilder,
        cancel: Option<&'a CancelSlot>,
    ) -> Result<HttpReply<'a>, AppError> {
        let token = CancellationToken::new();
        let armed = match cancel {
            Some(slot) => Some(arm_cancel(
                slot,
                CancelHandle::Custom(Arc::new(TokenCancel(token.clone()))),
            )?),
            None => None,
        };
        let mut template = Some(req.build().map_err(map_reqwest_error)?);
        let mut refreshed = false;
        let mut retries = 0;
        loop {
            let attempt = match template.as_ref().and_then(reqwest::Request::try_clone) {
                Some(copy) => copy,
                None => template
                    .take()
                    .ok_or_else(|| AppError::internal("a request sent twice"))?,
            };
            let replayable = template.is_some();
            let resp = self.execute(attempt, &token).await?;
            let status = resp.status().as_u16();
            if status == 401 && replayable && !refreshed && self.credential.refreshes() {
                refreshed = true;
                self.credential.invalidate().await;
                continue;
            }
            if matches!(status, 429 | 503) && replayable && retries < self.retry.max_retries {
                let wait =
                    retry_after(resp.headers()).unwrap_or_else(|| self.retry.backoff(retries));
                if wait <= self.retry.max_delay {
                    retries += 1;
                    drop(resp);
                    tokio::select! {
                        biased;
                        _ = token.cancelled() => return Err(AppError::Cancelled),
                        _ = tokio::time::sleep(wait) => {}
                    }
                    continue;
                }
            }
            return Ok(HttpReply {
                resp,
                cancel: token,
                _armed: armed,
            });
        }
    }

    /// `send`, then the reply's JSON body; a status other than 2xx is an error.
    pub async fn json<T: DeserializeOwned>(
        &self,
        req: RequestBuilder,
        cancel: Option<&CancelSlot>,
    ) -> Result<T, AppError> {
        self.send(req, cancel).await?.json().await
    }

    /// One attempt: the credential (for the endpoint's own requests), the tunnel, the request.
    async fn execute(
        &self,
        mut req: reqwest::Request,
        cancel: &CancellationToken,
    ) -> Result<reqwest::Response, AppError> {
        let attempt = async {
            if self.endpoint.owns(req.url()) {
                self.credential.apply(&mut req).await?;
                self.route(&mut req);
            }
            self.client.execute(req).await.map_err(map_reqwest_error)
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(AppError::Cancelled),
            r = attempt => r,
        }
    }

    /// Sends a request for the endpoint through its tunnel, keeping the server's name and port in
    /// the Host header (and so in a SigV4 signature, made before).
    fn route(&self, req: &mut reqwest::Request) {
        if self.route == Route::Direct {
            return;
        }
        let authority = authority(req.url());
        if !req.headers().contains_key(HOST)
            && let Ok(value) = HeaderValue::from_str(&authority)
        {
            req.headers_mut().insert(HOST, value);
        }
        let url = req.url_mut();
        match self.route {
            Route::Direct => {}
            Route::Resolve => {
                let _ = url.set_port(None);
            }
            Route::Rewrite(tunnel) => {
                let _ = url.set_ip_host(tunnel.ip());
                let _ = url.set_port(Some(tunnel.port()));
            }
        }
    }
}

/// `host[:port]` of `url`, the port only when it is not the scheme's default.
pub fn authority(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    }
}

/// The address `url` names, when its host is one (`[::1]` without the brackets).
fn host_ip(url: &Url) -> Option<IpAddr> {
    let host = url.host_str()?;
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .ok()
}

fn is_loopback(url: &Url) -> bool {
    match host_ip(url) {
        Some(ip) => ip.is_loopback(),
        None => url
            .host_str()
            .is_some_and(|h| h.eq_ignore_ascii_case("localhost")),
    }
}

/// `Retry-After` in seconds or as an HTTP date (a date in the past is no wait).
pub fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(secs) = value.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let at = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let wait = at.with_timezone(&chrono::Utc) - chrono::Utc::now();
    Some(wait.to_std().unwrap_or(Duration::ZERO))
}

/// What `HttpClient::send` arms the cancel slot with: dropping the request and its reply closes
/// the connection, which is how an HTTP server learns of a cancel.
struct TokenCancel(CancellationToken);

impl CancelAction for TokenCancel {
    fn fire(&self) -> BoxFuture<'static, Result<(), AppError>> {
        self.0.cancel();
        Box::pin(async { Ok(()) })
    }

    fn label(&self) -> &'static str {
        "HTTP request cancel"
    }
}

/// A reply whose head has arrived. Until it is dropped, the cancel slot `send` was given stops
/// the reading of its body (`Cancelled`).
pub struct HttpReply<'a> {
    resp: reqwest::Response,
    cancel: CancellationToken,
    _armed: Option<ArmedCancel<'a>>,
}

impl std::fmt::Debug for HttpReply<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpReply")
            .field("status", &self.resp.status().as_u16())
            .finish_non_exhaustive()
    }
}

/// At most this much of an error reply is read for its message.
const ERROR_BODY_LIMIT: usize = 64 * 1024;
/// At most this many characters of a reply body go into an error message.
const EXCERPT_CHARS: usize = 300;

impl<'a> HttpReply<'a> {
    pub fn status(&self) -> u16 {
        self.resp.status().as_u16()
    }

    pub fn is_success(&self) -> bool {
        self.resp.status().is_success()
    }

    pub fn headers(&self) -> &HeaderMap {
        self.resp.headers()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.resp.headers().get(name)?.to_str().ok()
    }

    pub fn content_type(&self) -> Option<&str> {
        self.header(CONTENT_TYPE.as_str())
    }

    /// The wait the server asked for (a 429 or 503 the retry policy handed over).
    pub fn retry_after(&self) -> Option<Duration> {
        retry_after(self.resp.headers())
    }

    /// The next piece of the body; `None` at its end.
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, AppError> {
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Err(AppError::Cancelled),
            c = self.resp.chunk() => c.map_err(map_reqwest_error),
        }
    }

    pub async fn bytes(mut self) -> Result<Vec<u8>, AppError> {
        let mut body = Vec::new();
        while let Some(chunk) = self.chunk().await? {
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    pub async fn text(self) -> Result<String, AppError> {
        let body = self.bytes().await?;
        String::from_utf8(body).map_err(|_| AppError::db("the server's reply is not UTF-8 text"))
    }

    /// The body as `T`; a status other than 2xx is an error (see [`status_error`]), and a body
    /// that is not a `T` a protocol error naming what came.
    pub async fn json<T: DeserializeOwned>(self) -> Result<T, AppError> {
        let reply = self.error_for_status().await?;
        let content_type = reply.content_type().map(str::to_string);
        let body = reply.bytes().await?;
        serde_json::from_slice(&body).map_err(|e| {
            AppError::db(format!(
                "unexpected response from the server: {e} (body: {})",
                body_excerpt(&body, content_type.as_deref())
            ))
        })
    }

    /// The reply itself when its status is 2xx, else the error [`status_error`] makes of it.
    pub async fn error_for_status(mut self) -> Result<HttpReply<'a>, AppError> {
        if self.is_success() {
            return Ok(self);
        }
        let status = self.status();
        let retry = self.retry_after();
        let content_type = self.content_type().map(str::to_string);
        let mut body = Vec::new();
        while body.len() < ERROR_BODY_LIMIT {
            match self.chunk().await {
                Ok(Some(chunk)) => body.extend_from_slice(&chunk),
                Ok(None) => break,
                Err(AppError::Cancelled) => return Err(AppError::Cancelled),
                // The status says enough.
                Err(_) => break,
            }
        }
        Err(status_error(status, &body, content_type.as_deref(), retry))
    }

    /// The body piece by piece.
    pub fn into_stream(self) -> BoxStream<'a, Result<Bytes, AppError>> {
        futures::stream::unfold(Some(self), |state| async move {
            let mut reply = state?;
            match reply.chunk().await {
                Ok(Some(chunk)) => Some((Ok(chunk), Some(reply))),
                Ok(None) => None,
                Err(e) => Some((Err(e), None)),
            }
        })
        .boxed()
    }

    /// The body line by line (NDJSON), without the line ends; a last line without one too.
    pub fn into_lines(self) -> BoxStream<'a, Result<String, AppError>> {
        struct State<'a> {
            reply: Option<HttpReply<'a>>,
            pending: Vec<u8>,
        }
        fn line(mut bytes: Vec<u8>) -> Result<String, AppError> {
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            String::from_utf8(bytes)
                .map_err(|_| AppError::db("the server sent a line that is not UTF-8"))
        }
        let state = State {
            reply: Some(self),
            pending: Vec::new(),
        };
        futures::stream::unfold(state, |mut state| async move {
            loop {
                if let Some(end) = state.pending.iter().position(|b| *b == b'\n') {
                    let rest = state.pending.split_off(end + 1);
                    let mut found = std::mem::replace(&mut state.pending, rest);
                    found.pop();
                    return Some((line(found), state));
                }
                let reply = state.reply.as_mut()?;
                match reply.chunk().await {
                    Ok(Some(chunk)) => state.pending.extend_from_slice(&chunk),
                    Ok(None) => {
                        state.reply = None;
                        if state.pending.is_empty() {
                            return None;
                        }
                        let last = std::mem::take(&mut state.pending);
                        return Some((line(last), state));
                    }
                    Err(e) => {
                        state.reply = None;
                        state.pending.clear();
                        return Some((Err(e), state));
                    }
                }
            }
        })
        .boxed()
    }
}

/// The error of a reply with a status other than 2xx: 401 and 403 are `Auth`, 404 `NotFound`,
/// 408 and 504 `Timeout`, 502 and 503 `Connect`, anything else `Db` with the status; the
/// message carries the start of the body as plain text (and a 429's wait).
pub fn status_error(
    status: u16,
    body: &[u8],
    content_type: Option<&str>,
    retry_after: Option<Duration>,
) -> AppError {
    let excerpt = body_excerpt(body, content_type);
    let said = |what: &str| {
        if excerpt.is_empty() {
            format!("{what} (HTTP {status})")
        } else {
            format!("{what} (HTTP {status}): {excerpt}")
        }
    };
    match status {
        401 => AppError::Auth(said("the server refused the credentials")),
        403 => AppError::Auth(said("access denied")),
        404 => AppError::NotFound(said("the server has no such resource")),
        408 | 504 => AppError::Timeout,
        429 => AppError::db(match retry_after {
            Some(wait) => format!(
                "{}; it asks to wait {} s",
                said("the server is rate limiting requests"),
                wait.as_secs()
            ),
            None => said("the server is rate limiting requests"),
        }),
        502 | 503 => AppError::Connect(said("the server is unavailable")),
        _ => AppError::db(said("the server answered with an error")),
    }
}

/// A short plain-text form of a reply body for a message: HTML loses its tags and whitespace
/// runs collapse.
pub fn body_excerpt(body: &[u8], content_type: Option<&str>) -> String {
    let text = String::from_utf8_lossy(body);
    let html =
        content_type.is_some_and(|c| c.contains("html")) || text.trim_start().starts_with('<');
    let plain = if html {
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
    match collapsed.char_indices().nth(EXCERPT_CHARS) {
        Some((cut, _)) => format!("{}…", &collapsed[..cut]),
        None => collapsed,
    }
}

/// A reqwest error as the app's: a timeout is `Timeout`, a connect or TLS failure `Connect`, a
/// status `Auth` (401 / 403) or `Db`, a request that cannot be built `InvalidInput`, and a
/// connection lost while sending or reading `NotConnected` (logged with its cause).
pub fn map_reqwest_error(e: reqwest::Error) -> AppError {
    let e = e.without_url();
    if e.is_timeout() {
        return AppError::Timeout;
    }
    if let Some(status) = e.status() {
        return match status.as_u16() {
            401 | 403 => AppError::Auth(error_text(&e)),
            _ => AppError::db(error_text(&e)),
        };
    }
    if e.is_connect() {
        return AppError::Connect(error_text(&e));
    }
    if e.is_builder() {
        return AppError::InvalidInput(error_text(&e));
    }
    log::warn!("HTTP connection lost: {}", error_text(&e));
    AppError::NotConnected
}

/// The error and its causes, which hold the useful part (`invalid peer certificate: …`).
fn error_text(e: &(dyn std::error::Error + 'static)) -> String {
    let mut text = e.to_string();
    let mut source = e.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::common::token::tests::{counted, profile};
    use crate::drivers::{cancel_portable, request_cancel};
    use crate::testkit::tls::{CA_PEM, TlsServer, ca_file, cert_der};
    use crate::testkit::{FakeServer, Outcome, Piece, Reply, Route as FakeRoute, install_crypto};
    use crate::types::profile::Driver;
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    fn plain(base: &str, credential: Credential, retry: RetryPolicy) -> HttpClient {
        install_crypto();
        let options = HttpOptions {
            retry,
            ..HttpOptions::default()
        };
        HttpClient::new(HttpEndpoint::new(base, None).unwrap(), options, credential).unwrap()
    }

    /// Cancels what runs on `slot` the way the app does: `request_cancel`, then the session's
    /// `cancel` with the handle.
    async fn cancel(slot: &CancelSlot) {
        let handle = request_cancel(slot).expect("the request armed the slot");
        cancel_portable(handle).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_cancel_mid_body_stops_the_read_within_a_second() {
        let server = FakeServer::start(vec![FakeRoute::new("GET", "/stream").reply(
            Reply::ok().chunked(vec![
                Piece::Data(b"first".to_vec()),
                Piece::Wait(Duration::from_secs(30)),
                Piece::Data(b"never".to_vec()),
            ]),
        )])
        .await;
        let client = plain(&server.url(""), Credential::None, RetryPolicy::NONE);
        let slot = CancelSlot::default();
        let mut reply = client
            .send(client.get("/stream").unwrap(), Some(&slot))
            .await
            .unwrap();
        assert_eq!(reply.chunk().await.unwrap().as_deref(), Some(&b"first"[..]));
        assert!(
            slot.is_armed(),
            "the slot stays armed while the body is read"
        );
        let started = Instant::now();
        let read = async { reply.chunk().await };
        let (read, ()) = tokio::join!(read, async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel(&slot).await;
        });
        assert_eq!(read.unwrap_err().code(), "CANCELLED");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        drop(reply);
        assert!(!slot.is_armed());
        // The connection was closed: the server saw the client go.
        server
            .wait_until(Duration::from_secs(5), |r| {
                r.first().is_some_and(|r| r.outcome == Outcome::Aborted)
            })
            .await;
        // A cancelled slot starts nothing more.
        let again = client
            .send(client.get("/stream").unwrap(), Some(&slot))
            .await;
        assert_eq!(again.unwrap_err().code(), "CANCELLED");
    }

    #[tokio::test]
    async fn a_cancel_stops_a_request_waiting_for_its_reply_and_streams() {
        let server = FakeServer::start(vec![
            FakeRoute::new("GET", "/slow").reply(Reply::ok().delay(Duration::from_secs(30))),
            FakeRoute::new("GET", "/lines").reply(Reply::ok().chunked(vec![
                Piece::Data(b"{\"a\":1}\n{\"b\"".to_vec()),
                Piece::Data(b":2}\r\n\n".to_vec()),
                Piece::Data(b"{\"c\":3}".to_vec()),
            ])),
        ])
        .await;
        let client = plain(&server.url(""), Credential::None, RetryPolicy::NONE);
        let slot = CancelSlot::default();
        let started = Instant::now();
        let (sent, ()) = tokio::join!(
            client.send(client.get("/slow").unwrap(), Some(&slot)),
            async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                cancel(&slot).await;
            }
        );
        assert_eq!(sent.unwrap_err().code(), "CANCELLED");
        assert!(started.elapsed() < Duration::from_secs(1));

        let reply = client
            .send(client.get("/lines").unwrap(), None)
            .await
            .unwrap();
        let lines: Vec<String> = reply
            .into_lines()
            .map(|l| l.unwrap())
            .collect::<Vec<_>>()
            .await;
        assert_eq!(lines, [r#"{"a":1}"#, r#"{"b":2}"#, "", r#"{"c":3}"#]);
    }

    #[tokio::test]
    async fn a_401_runs_the_token_command_again_once() {
        let server = FakeServer::start(vec![
            FakeRoute::new("GET", "/q")
                .auth("Bearer tok-2")
                .reply(Reply::json(200, &serde_json::json!({"ok": true}))),
        ])
        .await;
        let (token, runs) = counted(Duration::from_secs(600));
        let client = plain(
            &server.url(""),
            Credential::Bearer(token),
            RetryPolicy::NONE,
        );
        let reply: serde_json::Value = client.json(client.get("/q").unwrap(), None).await.unwrap();
        assert_eq!(reply, serde_json::json!({"ok": true}));
        assert_eq!(runs.load(Ordering::SeqCst), 2);
        let auth: Vec<_> = server
            .requests()
            .iter()
            .map(|r| r.header("authorization").unwrap_or_default().to_string())
            .collect();
        assert_eq!(auth, ["Bearer tok-1", "Bearer tok-2"]);

        // A second refusal is the caller's: no third run.
        let server =
            FakeServer::start(vec![FakeRoute::new("GET", "/q").auth("Bearer never")]).await;
        let (token, runs) = counted(Duration::from_secs(600));
        let client = plain(
            &server.url(""),
            Credential::Bearer(token),
            RetryPolicy::NONE,
        );
        let err = client
            .json::<serde_json::Value>(client.get("/q").unwrap(), None)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "AUTH_FAILED");
        assert_eq!(runs.load(Ordering::SeqCst), 2);
        assert_eq!(server.requests().len(), 2);

        // A stored token is not retried.
        let client = plain(
            &server.url(""),
            Credential::Bearer(TokenSource::from_secret(Some("old"))),
            RetryPolicy::NONE,
        );
        let reply = client.send(client.get("/q").unwrap(), None).await.unwrap();
        assert_eq!(reply.status(), 401);
        assert_eq!(server.requests().len(), 3);
    }

    #[tokio::test]
    async fn credentials_go_to_the_endpoint_only_and_empty_tokens_send_nothing() {
        let server = FakeServer::start(vec![FakeRoute::new("*", "/*").reply(Reply::ok())]).await;
        let other = FakeServer::start(vec![FakeRoute::new("*", "/*").reply(Reply::ok())]).await;
        let sent = |client: HttpClient, url: Option<String>| async move {
            let req = match url {
                Some(url) => client.request_url(Method::GET, Url::parse(&url).unwrap()),
                None => client.get("/x").unwrap(),
            };
            client.send(req, None).await.unwrap();
        };
        let base = server.url("");
        sent(
            plain(
                &base,
                Credential::Basic("ann".into(), TokenSource::from_secret(Some("pw"))),
                RetryPolicy::NONE,
            ),
            None,
        )
        .await;
        sent(
            plain(
                &base,
                Credential::Header("X-ClickHouse-Key", TokenSource::from_secret(Some(" k \n"))),
                RetryPolicy::NONE,
            ),
            None,
        )
        .await;
        sent(
            plain(
                &base,
                Credential::Scheme("ApiKey", TokenSource::from_secret(Some("abc"))),
                RetryPolicy::NONE,
            ),
            None,
        )
        .await;
        sent(
            plain(
                &base,
                Credential::bearer(TokenSource::from_secret(Some(""))),
                RetryPolicy::NONE,
            ),
            None,
        )
        .await;
        sent(
            plain(
                &base,
                Credential::Basic("ann".into(), TokenSource::None),
                RetryPolicy::NONE,
            ),
            None,
        )
        .await;
        // Another host (a token endpoint, a link the server returned) gets no credential.
        sent(
            plain(
                &base,
                Credential::Bearer(TokenSource::from_secret(Some("t"))),
                RetryPolicy::NONE,
            ),
            Some(other.url("/y")),
        )
        .await;
        let r = server.requests();
        assert_eq!(r[0].header("authorization"), Some("Basic YW5uOnB3"));
        assert_eq!(r[1].header("x-clickhouse-key"), Some("k"));
        assert_eq!(r[1].header("authorization"), None);
        assert_eq!(r[2].header("authorization"), Some("ApiKey abc"));
        assert_eq!(r[3].header("authorization"), None);
        assert_eq!(r[4].header("authorization"), Some("Basic YW5uOg=="));
        assert_eq!(other.requests()[0].header("authorization"), None);
        assert!(r[0].header("user-agent").unwrap().starts_with("Tabler/"));
    }

    #[tokio::test]
    async fn a_429_waits_as_long_as_retry_after_says() {
        let server = FakeServer::start(vec![
            FakeRoute::new("GET", "/busy")
                .reply(Reply::text(429, "slow down").header("retry-after", "1"))
                .reply(Reply::text(200, "done")),
        ])
        .await;
        let client = plain(&server.url(""), Credential::None, RetryPolicy::default());
        let started = Instant::now();
        let reply = client
            .send(client.get("/busy").unwrap(), None)
            .await
            .unwrap();
        assert_eq!(reply.status(), 200);
        assert_eq!(reply.text().await.unwrap(), "done");
        assert!(
            started.elapsed() >= Duration::from_millis(950),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(server.requests().len(), 2);
    }

    #[tokio::test]
    async fn retry_after_goes_to_the_caller_when_it_does_not_retry() {
        let server = FakeServer::start(vec![
            FakeRoute::new("GET", "/busy")
                .reply(Reply::text(429, "slow down").header("retry-after", "7")),
            FakeRoute::new("GET", "/later")
                .reply(Reply::text(503, "x").header("retry-after", "120")),
        ])
        .await;
        let client = plain(&server.url(""), Credential::None, RetryPolicy::NONE);
        let reply = client
            .send(client.get("/busy").unwrap(), None)
            .await
            .unwrap();
        assert_eq!(reply.status(), 429);
        assert_eq!(reply.retry_after(), Some(Duration::from_secs(7)));
        let err = reply.error_for_status().await.unwrap_err();
        assert_eq!(err.code(), "DB_ERROR");
        assert!(err.to_string().contains("wait 7 s"), "{err}");
        // A wait beyond the policy's longest is handed over at once.
        let client = plain(&server.url(""), Credential::None, RetryPolicy::default());
        let started = Instant::now();
        let reply = client
            .send(client.get("/later").unwrap(), None)
            .await
            .unwrap();
        assert_eq!(reply.status(), 503);
        assert_eq!(reply.retry_after(), Some(Duration::from_secs(120)));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(server.requests().len(), 2);
    }

    #[tokio::test]
    async fn a_cancel_ends_the_wait_between_retries() {
        let server = FakeServer::start(vec![
            FakeRoute::new("GET", "/busy").reply(Reply::new(503).header("retry-after", "10")),
        ])
        .await;
        let client = plain(&server.url(""), Credential::None, RetryPolicy::default());
        let slot = CancelSlot::default();
        let started = Instant::now();
        let (sent, ()) = tokio::join!(
            client.send(client.get("/busy").unwrap(), Some(&slot)),
            async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                cancel(&slot).await;
            }
        );
        assert_eq!(sent.unwrap_err().code(), "CANCELLED");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn the_tunnel_keeps_the_host_header() {
        let server = FakeServer::start(vec![FakeRoute::new("GET", "/*").reply(Reply::ok())]).await;
        install_crypto();
        // The fake server stands in for the tunnel's local end.
        for (base, host) in [
            ("http://db.example.test:9200/prefix", "db.example.test:9200"),
            ("http://db.example.test/prefix", "db.example.test"),
            ("http://10.1.2.3:8123/prefix", "10.1.2.3:8123"),
        ] {
            let endpoint = HttpEndpoint::new(base, Some(server.addr())).unwrap();
            let client =
                HttpClient::new(endpoint, HttpOptions::default(), Credential::None).unwrap();
            let reply = client
                .send(client.get("/_cat?v=1").unwrap(), None)
                .await
                .unwrap();
            assert_eq!(reply.status(), 200, "{base}");
            let last = server.requests().pop().unwrap();
            assert_eq!(last.header("host"), Some(host), "{base}");
            assert_eq!(last.path, "/prefix/_cat");
            assert_eq!(last.query.as_deref(), Some("v=1"));
        }
    }

    fn tls_client(
        base: &str,
        tunnel: Option<SocketAddr>,
        tls: HttpTls,
    ) -> Result<HttpClient, AppError> {
        install_crypto();
        let options = HttpOptions {
            tls,
            connect_timeout: Duration::from_secs(5),
            ..HttpOptions::default()
        };
        HttpClient::new(
            HttpEndpoint::new(base, tunnel).unwrap(),
            options,
            Credential::None,
        )
    }

    async fn get(client: &HttpClient) -> Result<u16, AppError> {
        Ok(client.send(client.get("/").unwrap(), None).await?.status())
    }

    #[tokio::test]
    async fn the_tunnel_keeps_sni_and_the_certificate_name() {
        let server = TlsServer::start();
        let ca = ca_file();
        let tls = || HttpTls {
            mode: SslMode::VerifyFull,
            ca_path: Some(ca.path().to_string_lossy().into_owned()),
            identity: None,
            pin_sha256: None,
        };
        // A DNS name: resolved to the tunnel, the name kept for SNI, the certificate and Host.
        let client =
            tls_client("https://db.example.test:9443", Some(server.addr()), tls()).unwrap();
        assert_eq!(get(&client).await.unwrap(), 200);
        let seen = server.requests().pop().unwrap();
        assert_eq!(seen.sni.as_deref(), Some("db.example.test"));
        assert_eq!(seen.header("host"), Some("db.example.test:9443"));
        // An IP address: no SNI, the certificate checked for the server's address.
        let client = tls_client("https://127.0.0.1:9443", Some(server.addr()), tls()).unwrap();
        assert_eq!(get(&client).await.unwrap(), 200);
        let seen = server.requests().pop().unwrap();
        assert_eq!(seen.sni, None);
        assert_eq!(seen.header("host"), Some("127.0.0.1:9443"));
        // A name the certificate does not hold is refused.
        let client = tls_client("https://elsewhere.test", Some(server.addr()), tls()).unwrap();
        assert_eq!(get(&client).await.unwrap_err().code(), "CONNECT_FAILED");
        let client = tls_client("https://10.9.9.9", Some(server.addr()), tls()).unwrap();
        assert_eq!(get(&client).await.unwrap_err().code(), "CONNECT_FAILED");
        assert_eq!(server.requests().len(), 2);
    }

    #[tokio::test]
    async fn an_untrusted_ca_is_refused_and_a_ca_file_accepted() {
        let server = TlsServer::start();
        let base = format!("https://localhost:{}", server.addr().port());
        let client = tls_client(&base, None, HttpTls::verified()).unwrap();
        let err = get(&client).await.unwrap_err();
        assert_eq!(err.code(), "CONNECT_FAILED");
        assert!(
            err.to_string().to_lowercase().contains("certificate"),
            "{err}"
        );
        let ca = ca_file();
        let tls = HttpTls {
            ca_path: Some(ca.path().to_string_lossy().into_owned()),
            ..HttpTls::verified()
        };
        let client = tls_client(&base, None, tls).unwrap();
        assert_eq!(get(&client).await.unwrap(), 200);
        assert_eq!(server.requests()[0].sni.as_deref(), Some("localhost"));
    }

    #[tokio::test]
    async fn a_wrong_ca_pin_fails_to_connect() {
        let server = TlsServer::start();
        let base = format!("https://localhost:{}", server.addr().port());
        let pin = crate::tls_client::sha256(&cert_der(CA_PEM));
        let pinned = |pin| HttpTls {
            mode: SslMode::Require,
            ca_path: None,
            identity: None,
            pin_sha256: Some(pin),
        };
        let client = tls_client(&base, None, pinned(pin)).unwrap();
        assert_eq!(get(&client).await.unwrap(), 200);
        let mut wrong = pin;
        wrong[31] ^= 0xff;
        let client = tls_client(&base, None, pinned(wrong)).unwrap();
        let err = get(&client).await.unwrap_err();
        assert_eq!(err.code(), "CONNECT_FAILED");
        assert!(err.to_string().contains("pinned"), "{err}");
        assert_eq!(server.requests().len(), 1);
    }

    /// What a server checking SigV4 does: the signature of the request as it arrived, with the
    /// headers it names, must be the one sent.
    #[cfg(feature = "common-aws")]
    #[tokio::test]
    async fn sigv4_signs_what_the_server_receives_through_the_tunnel() {
        use super::super::aws::{CredentialProvider, Credentials, Signer, sigv4};
        let server = FakeServer::start(vec![FakeRoute::new("POST", "/").reply(Reply::ok())]).await;
        install_crypto();
        let creds = Credentials::static_keys("AKID", "secret", Some("session")).unwrap();
        let signer = Signer::new(
            "dynamodb",
            "eu-west-1",
            Arc::new(CredentialProvider::fixed(creds.clone())),
        );
        let endpoint = HttpEndpoint::new("http://dynamodb.test:8000", Some(server.addr())).unwrap();
        let client =
            HttpClient::new(endpoint, HttpOptions::default(), Credential::Sigv4(signer)).unwrap();
        let req = client
            .post("/")
            .unwrap()
            .header("x-amz-target", "DynamoDB_20120810.ListTables")
            .header(CONTENT_TYPE, "application/x-amz-json-1.0")
            .body(r#"{"Limit":10}"#);
        assert_eq!(client.send(req, None).await.unwrap().status(), 200);
        let got = server.requests().pop().unwrap();
        assert_eq!(got.header("host"), Some("dynamodb.test:8000"));
        assert_eq!(got.header("x-amz-security-token"), Some("session"));
        let auth = got.header("authorization").unwrap();
        let names = auth
            .split("SignedHeaders=")
            .nth(1)
            .and_then(|r| r.split(',').next())
            .unwrap();
        assert_eq!(
            names,
            "content-type;host;x-amz-date;x-amz-security-token;x-amz-target"
        );
        let headers: Vec<(String, String)> = names
            .split(';')
            .map(|n| (n.to_string(), got.header(n).unwrap().to_string()))
            .collect();
        let at = chrono::NaiveDateTime::parse_from_str(
            got.header("x-amz-date").unwrap(),
            "%Y%m%dT%H%M%SZ",
        )
        .unwrap()
        .and_utc();
        let expected = sigv4::sign_parts(
            "POST",
            &got.path,
            got.query.as_deref().unwrap_or(""),
            &headers,
            &got.body,
            &creds,
            at,
            "eu-west-1",
            "dynamodb",
        );
        assert_eq!(auth, expected.authorization);
    }

    #[test]
    fn endpoints_take_http_urls_and_the_tunnel_of_a_target() {
        let p = profile(Driver::Elasticsearch, None);
        let t = ConnectTarget {
            profile: &p,
            password: None,
            host_override: Some(("127.0.0.1".into(), 50_001)),
        };
        let e = HttpEndpoint::for_target(&t, "https://db.example.test:9200").unwrap();
        assert_eq!(e.real_host, "db.example.test");
        assert_eq!(e.tunnel, Some("127.0.0.1:50001".parse().unwrap()));
        let direct = ConnectTarget {
            host_override: None,
            ..t
        };
        assert_eq!(
            HttpEndpoint::for_target(&direct, "http://h:1")
                .unwrap()
                .tunnel,
            None
        );
        for bad in ["ftp://h", "not a url", "http://"] {
            assert_eq!(
                HttpEndpoint::new(bad, None).unwrap_err().code(),
                "INVALID_INPUT",
                "{bad}"
            );
        }
        let client = plain("https://h.test/base/", Credential::None, RetryPolicy::NONE);
        assert_eq!(
            client.url("/a b?x=1").unwrap().as_str(),
            "https://h.test/base/a%20b?x=1"
        );
        assert_eq!(client.url("c").unwrap().as_str(), "https://h.test/base/c");
        let client = plain("https://h.test", Credential::None, RetryPolicy::NONE);
        assert_eq!(client.url("/v1/q").unwrap().as_str(), "https://h.test/v1/q");
        assert_eq!(
            authority(&Url::parse("https://h.test:443/").unwrap()),
            "h.test"
        );
        assert_eq!(
            authority(&Url::parse("http://[::1]:8080/").unwrap()),
            "[::1]:8080"
        );
    }

    #[test]
    fn statuses_become_the_app_errors() {
        let code = |s| status_error(s, b"", None, None).code();
        assert_eq!(code(401), "AUTH_FAILED");
        assert_eq!(code(403), "AUTH_FAILED");
        assert_eq!(code(404), "NOT_FOUND");
        assert_eq!(code(408), "TIMEOUT");
        assert_eq!(code(504), "TIMEOUT");
        assert_eq!(code(502), "CONNECT_FAILED");
        assert_eq!(code(503), "CONNECT_FAILED");
        assert_eq!(code(500), "DB_ERROR");
        let err = status_error(
            500,
            b"<html><b>Oops</b>\n  broke</html>",
            Some("text/html"),
            None,
        );
        assert!(err.to_string().ends_with("(HTTP 500): Oops broke"), "{err}");
        let long = "x".repeat(400);
        assert_eq!(
            body_excerpt(long.as_bytes(), None).chars().count(),
            EXCERPT_CHARS + 1
        );
    }

    #[test]
    fn retry_after_reads_seconds_and_dates() {
        let mut h = HeaderMap::new();
        assert_eq!(retry_after(&h), None);
        h.insert("retry-after", HeaderValue::from_static("3"));
        assert_eq!(retry_after(&h), Some(Duration::from_secs(3)));
        h.insert(
            "retry-after",
            HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"),
        );
        assert_eq!(retry_after(&h), Some(Duration::ZERO));
        let later = (chrono::Utc::now() + chrono::Duration::seconds(90)).to_rfc2822();
        h.insert("retry-after", HeaderValue::from_str(&later).unwrap());
        let wait = retry_after(&h).unwrap();
        assert!(
            wait > Duration::from_secs(80) && wait <= Duration::from_secs(90),
            "{wait:?}"
        );
        h.insert("retry-after", HeaderValue::from_static("soon"));
        assert_eq!(retry_after(&h), None);
    }

    #[test]
    fn backoff_doubles_with_jitter_up_to_the_limit() {
        let p = RetryPolicy {
            max_retries: 10,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(1),
        };
        for _ in 0..20 {
            let first = p.backoff(0);
            assert!(first >= Duration::from_millis(50) && first <= Duration::from_millis(100));
            let third = p.backoff(2);
            assert!(third >= Duration::from_millis(200) && third <= Duration::from_millis(400));
            assert!(p.backoff(30) <= Duration::from_secs(1));
        }
    }

    #[test]
    fn secrets_stay_out_of_debug_output() {
        let c = Credential::Basic("ann".into(), TokenSource::from_secret(Some("pw")));
        let shown = format!("{c:?}");
        assert!(!shown.contains("pw"), "{shown}");
    }
}
