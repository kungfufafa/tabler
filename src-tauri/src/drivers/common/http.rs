//! The HTTP client of the HTTP drivers (LibSQL, D1, ClickHouse, BigQuery, Elasticsearch,
//! DynamoDB): reqwest on the `tls_client` config, an SSH tunnel that keeps the server's name for
//! the Host header, SNI and the certificate check, credentials on the endpoint's own requests
//! only, one token refresh after a 401, backoff on 429 and on the server errors of requests that
//! may run twice ([`Repeat`]), the system proxy (never for a tunnel or this machine), and a
//! cancel slot that stops the request and the reading of its reply. Requests carry their own
//! headers and body as built; a credential (a SigV4 signature) is added last, per attempt.
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

/// Which failed replies `send` sends a request again after, within its [`RetryPolicy`]. A
/// request whose body is a stream is never sent twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    /// A 429 (the server turned the request away), and a 503 of a GET, HEAD or OPTIONS. Any
    /// other request may have done its work before the server failed: PUT and DELETE are
    /// idempotent in HTTP's terms, but an edit guarded by a version, run twice, reports a
    /// conflict for the change it made.
    #[default]
    Safe,
    /// Also a 500, 502 and 504 of a GET, HEAD or OPTIONS (BigQuery's `backendError`).
    ServerErrors,
    /// A 429, 500, 502, 503 or 504 whatever the method: the caller knows the request may run
    /// twice (a read sent as a POST, a write guarded by a condition).
    Always,
}

impl Repeat {
    /// Whether a reply of `status` to a `method` request is sent again.
    fn repeats(self, method: &Method, status: u16) -> bool {
        let safe = matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS);
        match status {
            429 => true,
            503 => safe || self == Repeat::Always,
            500 | 502 | 504 => match self {
                Repeat::Safe => false,
                Repeat::ServerErrors => safe,
                Repeat::Always => true,
            },
            _ => false,
        }
    }
}

/// What `send` does with a 429 or a server error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// How many times such a reply is retried; 0 hands it to the caller, with its
    /// `HttpReply::retry_after`.
    pub max_retries: u32,
    /// The first wait when the server names none; it doubles each time, with jitter.
    pub base_delay: Duration,
    /// The longest wait: a server asking for more gets its reply handed to the caller.
    pub max_delay: Duration,
    /// Which replies are retried (`HttpClient::send_repeating` chooses per request).
    pub repeat: Repeat,
}

impl RetryPolicy {
    pub const NONE: RetryPolicy = RetryPolicy {
        max_retries: 0,
        base_delay: Duration::ZERO,
        max_delay: Duration::ZERO,
        repeat: Repeat::Safe,
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
            repeat: Repeat::Safe,
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
    /// A proxy (`http://host:port`) in place of the system's. Neither is used for a tunnel or
    /// this machine, which a proxy cannot reach.
    pub proxy: Option<String>,
}

impl Default for HttpOptions {
    fn default() -> HttpOptions {
        HttpOptions {
            tls: HttpTls::verified(),
            connect_timeout: Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS),
            request_timeout: None,
            retry: RetryPolicy::default(),
            proxy: None,
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
        if !uses_proxy(&endpoint) {
            builder = builder.no_proxy();
        } else if let Some(proxy) = o.proxy.as_deref() {
            // The URL may hold the proxy's password: the message leaves it out.
            let proxy = reqwest::Proxy::all(proxy.trim()).map_err(|e| {
                AppError::InvalidInput(format!(
                    "the proxy is not a valid URL: {}",
                    error_text(&e.without_url())
                ))
            })?;
            builder = builder.proxy(proxy);
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

    /// The URL of `target`, an absolute URL or a path and query under the endpoint's base,
    /// exactly as written: a request signed over its text (SigV4) must arrive as signed, so a
    /// text the URL parser would change is refused rather than sent as something else (a `.` or
    /// `..` segment, also as `%2E`; a space or another character it encodes; a `\` or a `#`).
    pub fn exact_url(&self, target: &str) -> Result<Url, AppError> {
        let refused = |why: String| {
            AppError::InvalidInput(format!("the request URL {target} cannot be sent: {why}"))
        };
        let (url, written) = match target.split_once("://") {
            Some(("http" | "https", rest)) => {
                let url = Url::parse(target).map_err(|e| refused(e.to_string()))?;
                let written = rest.find(['/', '?', '#']).map_or("", |i| &rest[i..]);
                (url, written.to_string())
            }
            _ => {
                let written = format!(
                    "{}/{}",
                    self.endpoint.base.path().trim_end_matches('/'),
                    target.trim_start_matches('/')
                );
                let url = self
                    .endpoint
                    .base
                    .join(&written)
                    .map_err(|e| refused(e.to_string()))?;
                // A path must not name another server (`/\\host/…` would).
                if !self.endpoint.owns(&url) {
                    return Err(refused("it names another server".into()));
                }
                (url, written)
            }
        };
        let written = if written.is_empty() || written.starts_with('?') {
            format!("/{written}")
        } else {
            written
        };
        let sent = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_string(),
        };
        if sent != written || url.fragment().is_some() {
            return Err(refused(format!("it would go as {sent}")));
        }
        Ok(url)
    }

    /// A request for [`HttpClient::exact_url`] of `target`.
    pub fn request_exact(&self, method: Method, target: &str) -> Result<RequestBuilder, AppError> {
        Ok(self.request_url(method, self.exact_url(target)?))
    }

    pub fn get(&self, path: &str) -> Result<RequestBuilder, AppError> {
        self.request(Method::GET, path)
    }

    pub fn head(&self, path: &str) -> Result<RequestBuilder, AppError> {
        self.request(Method::HEAD, path)
    }

    pub fn post(&self, path: &str) -> Result<RequestBuilder, AppError> {
        self.request(Method::POST, path)
    }

    pub fn put(&self, path: &str) -> Result<RequestBuilder, AppError> {
        self.request(Method::PUT, path)
    }

    pub fn delete(&self, path: &str) -> Result<RequestBuilder, AppError> {
        self.request(Method::DELETE, path)
    }

    pub fn patch(&self, path: &str) -> Result<RequestBuilder, AppError> {
        self.request(Method::PATCH, path)
    }

    /// Sends `req` and returns the reply once its head has arrived; any status is a reply. With
    /// `cancel`, the slot stops the request, the waits between retries and the reading of the
    /// reply until the reply is dropped. A 401 from a credential that can bring a fresh secret
    /// is retried once with it; a 429 or a server error as the retry policy and its [`Repeat`]
    /// say. A request whose body is a stream is sent once.
    pub async fn send<'a>(
        &self,
        req: RequestBuilder,
        cancel: Option<&'a CancelSlot>,
    ) -> Result<HttpReply<'a>, AppError> {
        self.send_repeating(req, cancel, self.retry.repeat).await
    }

    /// [`HttpClient::send`] retrying the replies `repeat` names instead of the policy's.
    pub async fn send_repeating<'a>(
        &self,
        req: RequestBuilder,
        cancel: Option<&'a CancelSlot>,
        repeat: Repeat,
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
        let method = template
            .as_ref()
            .map_or(Method::GET, |r| r.method().clone());
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
            if repeat.repeats(&method, status) && replayable && retries < self.retry.max_retries {
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
        Some(ip) => ip.to_canonical().is_loopback(),
        None => url.host_str().is_some_and(|h| {
            let h = h.to_ascii_lowercase();
            h == "localhost" || h.ends_with(".localhost")
        }),
    }
}

/// Whether requests to `endpoint` may go through a proxy (the system's or the options'): never
/// through a tunnel or to this machine.
fn uses_proxy(endpoint: &HttpEndpoint) -> bool {
    endpoint.tunnel.is_none() && !is_loopback(&endpoint.base)
}

/// What `application/x-www-form-urlencoded` leaves as it is: letters, digits and `*-._`.
const FORM_KEEP: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'*')
    .remove(b'-')
    .remove(b'.')
    .remove(b'_');

/// `pairs` as an `application/x-www-form-urlencoded` body, as browsers write one (a space is
/// `+`): what an OAuth token endpoint takes. reqwest's own `form` needs a feature the app does
/// not build.
pub fn form_body(pairs: &[(&str, &str)]) -> String {
    let encode = |s: &str| {
        percent_encoding::utf8_percent_encode(s, FORM_KEEP)
            .to_string()
            .replace("%20", "+")
    };
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// `req` with the form `pairs` as its body (see [`form_body`]) and that content type.
pub fn with_form(req: RequestBuilder, pairs: &[(&str, &str)]) -> RequestBuilder {
    req.header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(form_body(pairs))
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

    /// The status, headers and whole body, whatever the status: a 4xx body is the caller's to
    /// read (DynamoDB names its error in `x-amzn-ErrorType` and the JSON of a 400).
    pub async fn into_response(self) -> Result<HttpResponse, AppError> {
        let status = self.status();
        let headers = self.headers().clone();
        let body = self.bytes().await?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
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

/// A whole reply ([`HttpReply::into_response`]).
#[derive(Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

/// The body stays out of logs and panics: a token endpoint's reply holds a token.
impl std::fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("content_type", &self.content_type())
            .field("body_len", &self.body.len())
            .finish_non_exhaustive()
    }
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)?.to_str().ok()
    }

    pub fn content_type(&self) -> Option<&str> {
        self.header(CONTENT_TYPE.as_str())
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
    use crate::testkit::tls::{
        CA_PEM, CLIENT_CERT_PEM, TlsServer, ca_file, cert_der, client_identity,
    };
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

    /// What the lanes' transports send: their own headers, a raw body and an encoded path, by
    /// every method, arriving as built.
    #[tokio::test]
    async fn headers_raw_bodies_and_encoded_paths_arrive_unchanged_by_every_method() {
        let server = FakeServer::start(vec![
            FakeRoute::new("*", "/*").reply(
                Reply::text(200, "ok")
                    .header("x-elastic-product", "Elasticsearch")
                    .header("x-amzn-requestid", "req-1"),
            ),
        ])
        .await;
        let client = plain(
            &server.url("/es"),
            Credential::Scheme("ApiKey", TokenSource::from_secret(Some("k"))),
            RetryPolicy::NONE,
        );
        let target = "/logs-*/_doc/a%2Fb%20c+d'!$(),;=:@~?refresh=wait_for&q=x%20y%2B1&s=*";
        let body: &[u8] = b"{\"a\":1}\n\xff\x00 raw";
        for method in [
            Method::GET,
            Method::HEAD,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::PATCH,
        ] {
            let req = client
                .request_exact(method.clone(), target)
                .unwrap()
                .header("x-opaque-id", "tabler-7")
                .header("x-goog-user-project", "billing-project")
                .header(CONTENT_TYPE, "application/x-ndjson")
                .body(body.to_vec());
            let reply = client.send(req, None).await.unwrap();
            assert_eq!(reply.status(), 200, "{method}");
            assert_eq!(reply.header("x-amzn-requestid"), Some("req-1"));
            let got = server.requests().pop().unwrap();
            assert_eq!(got.method, method.as_str());
            assert_eq!(
                got.path, "/es/logs-*/_doc/a%2Fb%20c+d'!$(),;=:@~",
                "{method}"
            );
            assert_eq!(
                got.query.as_deref(),
                Some("refresh=wait_for&q=x%20y%2B1&s=*")
            );
            assert_eq!(got.body, body, "{method}");
            assert_eq!(got.header("x-opaque-id"), Some("tabler-7"));
            assert_eq!(got.header("x-goog-user-project"), Some("billing-project"));
            assert_eq!(got.header("content-type"), Some("application/x-ndjson"));
            assert_eq!(got.header("authorization"), Some("ApiKey k"));
        }
        // The helpers name their methods.
        for (req, method) in [
            (client.head("/h").unwrap(), "HEAD"),
            (client.put("/h").unwrap(), "PUT"),
            (client.delete("/h").unwrap(), "DELETE"),
            (client.patch("/h").unwrap(), "PATCH"),
        ] {
            client.send(req, None).await.unwrap();
            assert_eq!(server.requests().pop().unwrap().method, method);
        }
        // An absolute URL of the endpoint, as the lanes' seams carry it.
        let absolute = server.url("/es/_cat/indices?format=json&h=index");
        let reply = client
            .send(client.request_exact(Method::GET, &absolute).unwrap(), None)
            .await
            .unwrap();
        assert_eq!(reply.status(), 200);
        let got = server.requests().pop().unwrap();
        assert_eq!(got.path, "/es/_cat/indices");
        assert_eq!(got.query.as_deref(), Some("format=json&h=index"));
        assert_eq!(got.header("authorization"), Some("ApiKey k"));
    }

    #[test]
    fn exact_urls_are_sent_as_written_or_refused() {
        let client = plain("https://h.test/base/", Credential::None, RetryPolicy::NONE);
        let exact = |t: &str| client.exact_url(t).map(|u| u.to_string());
        assert_eq!(
            exact("/_doc/a%2fB?x=%2B").unwrap(),
            "https://h.test/base/_doc/a%2fB?x=%2B"
        );
        assert_eq!(exact("").unwrap(), "https://h.test/base/");
        assert_eq!(exact("?q").unwrap(), "https://h.test/base/?q");
        assert_eq!(
            exact("https://other.test:8443/x?y").unwrap(),
            "https://other.test:8443/x?y"
        );
        assert_eq!(exact("https://other.test").unwrap(), "https://other.test/");
        for bad in [
            "/a/../b",
            "/a/%2E%2E/b",
            "/a/%2e/b",
            "/a/./b",
            "/a b",
            "/a\\b",
            "/a#b",
            "/q?it's",
            "/q?a b",
            "/caf\u{e9}",
            "https://other.test/a/../b",
            "\\\\evil.test/x",
            "/\\evil.test/x",
        ] {
            let err = client.exact_url(bad).unwrap_err();
            assert_eq!(err.code(), "INVALID_INPUT", "{bad}");
            assert!(err.to_string().contains("cannot be sent"), "{err}");
        }
    }

    /// DynamoDB names its errors in a 400's headers and body: the caller gets them all, and a
    /// 400 is no credential problem.
    #[tokio::test]
    async fn replies_hand_back_status_headers_and_4xx_bodies() {
        let error = r#"{"__type":"com.amazonaws.dynamodb.v20120810#ValidationException","message":"bad key"}"#;
        let server = FakeServer::start(vec![
            FakeRoute::new("POST", "/").reply(
                Reply::new(400)
                    .header("content-type", "application/x-amz-json-1.0")
                    .header("date", "Thu, 29 Oct 2026 06:00:00 GMT")
                    .header("x-amzn-requestid", "RID-1")
                    .header("x-amzn-errortype", "ValidationException:http://internal/")
                    .body(error),
            ),
        ])
        .await;
        let client = plain(
            &server.url(""),
            Credential::Bearer(counted(Duration::from_secs(600)).0),
            RetryPolicy::default(),
        );
        let req = client
            .post("/")
            .unwrap()
            .header("x-amz-target", "DynamoDB_20120810.GetItem")
            .body("{}");
        let reply = client.send(req, None).await.unwrap();
        let response = reply.into_response().await.unwrap();
        assert_eq!(response.status, 400);
        assert!(!response.is_success());
        assert_eq!(response.body, error.as_bytes());
        assert_eq!(response.content_type(), Some("application/x-amz-json-1.0"));
        assert_eq!(
            response.header("date"),
            Some("Thu, 29 Oct 2026 06:00:00 GMT")
        );
        assert_eq!(response.header("x-amzn-RequestId"), Some("RID-1"));
        assert_eq!(
            response.header("x-amzn-ErrorType"),
            Some("ValidationException:http://internal/")
        );
        assert!(!format!("{response:?}").contains("bad key"));
        // Sent once: no token refresh, no retry.
        assert_eq!(server.requests().len(), 1);
        let req = client.post("/").unwrap().body("{}");
        let err = client
            .send(req, None)
            .await
            .unwrap()
            .error_for_status()
            .await
            .unwrap_err();
        assert_eq!(err.code(), "DB_ERROR");
        assert!(err.to_string().contains("ValidationException"), "{err}");
        assert_eq!(status_error(400, b"", None, None).code(), "DB_ERROR");
    }

    #[tokio::test]
    async fn server_errors_are_retried_only_where_the_request_may_run_twice() {
        let flaky = |status: u16| vec![Reply::text(status, "busy"), Reply::text(200, "done")];
        let mut routes = Vec::new();
        for (path, status) in [
            ("/get-503", 503),
            ("/post-503", 503),
            ("/put-503", 503),
            ("/get-500", 500),
            ("/get-500-opted", 500),
            ("/head-502", 502),
            ("/post-500-opted", 500),
            ("/post-500-always", 500),
            ("/post-429", 429),
            ("/stream-503", 503),
        ] {
            let mut route = FakeRoute::new("*", path);
            for reply in flaky(status) {
                route = route.reply(reply);
            }
            routes.push(route);
        }
        let server = FakeServer::start(routes).await;
        let retry = RetryPolicy {
            max_retries: 2,
            base_delay: Duration::from_millis(5),
            max_delay: Duration::from_secs(1),
            repeat: Repeat::Safe,
        };
        let client = plain(&server.url(""), Credential::None, retry);
        async fn status(c: &HttpClient, method: Method, path: &str, repeat: Repeat) -> u16 {
            let req = c.request(method, path).unwrap().body("{\"n\":1}");
            c.send_repeating(req, None, repeat).await.unwrap().status()
        }
        let c = &client;
        assert_eq!(status(c, Method::GET, "/get-503", Repeat::Safe).await, 200);
        assert_eq!(
            status(c, Method::POST, "/post-503", Repeat::Safe).await,
            503
        );
        assert_eq!(status(c, Method::PUT, "/put-503", Repeat::Safe).await, 503);
        assert_eq!(status(c, Method::GET, "/get-500", Repeat::Safe).await, 500);
        assert_eq!(
            status(c, Method::GET, "/get-500-opted", Repeat::ServerErrors).await,
            200
        );
        assert_eq!(
            status(c, Method::HEAD, "/head-502", Repeat::ServerErrors).await,
            200
        );
        assert_eq!(
            status(c, Method::POST, "/post-500-opted", Repeat::ServerErrors).await,
            500
        );
        assert_eq!(
            status(c, Method::POST, "/post-500-always", Repeat::Always).await,
            200
        );
        // The server turned a 429 away unprocessed: any method goes again.
        assert_eq!(
            status(c, Method::POST, "/post-429", Repeat::Safe).await,
            200
        );
        // A streamed body cannot be sent twice, whatever the caller allows.
        let pieces: Vec<Result<&'static [u8], std::io::Error>> = vec![Ok(b"a")];
        let req = client
            .post("/stream-503")
            .unwrap()
            .body(reqwest::Body::wrap_stream(futures::stream::iter(pieces)));
        let reply = client
            .send_repeating(req, None, Repeat::Always)
            .await
            .unwrap();
        assert_eq!(reply.status(), 503);
        let sent = |path: &str| server.requests().iter().filter(|r| r.path == path).count();
        for (path, times) in [
            ("/get-503", 2),
            ("/post-503", 1),
            ("/put-503", 1),
            ("/get-500", 1),
            ("/get-500-opted", 2),
            ("/head-502", 2),
            ("/post-500-opted", 1),
            ("/post-500-always", 2),
            ("/post-429", 2),
            ("/stream-503", 1),
        ] {
            assert_eq!(sent(path), times, "{path}");
        }
        // The client's policy chooses for `send`.
        let server = FakeServer::start(vec![
            FakeRoute::new("GET", "/q")
                .reply(Reply::text(500, "backendError"))
                .reply(Reply::text(200, "done")),
        ])
        .await;
        let client = plain(
            &server.url(""),
            Credential::None,
            RetryPolicy {
                repeat: Repeat::ServerErrors,
                ..retry
            },
        );
        let reply = client.send(client.get("/q").unwrap(), None).await.unwrap();
        assert_eq!(reply.status(), 200);
        assert_eq!(server.requests().len(), 2);
    }

    /// The OAuth token endpoint takes a form, on another host than the API: it gets the form and
    /// none of the API's credentials.
    #[tokio::test]
    async fn form_posts_reach_a_token_endpoint_without_the_api_credential() {
        let api = FakeServer::start(vec![FakeRoute::new("*", "/*").reply(Reply::ok())]).await;
        let oauth = FakeServer::start(vec![FakeRoute::new("POST", "/token").reply(Reply::json(
            200,
            &serde_json::json!({"access_token": "ya29.x", "expires_in": 3599}),
        ))])
        .await;
        let client = plain(
            &api.url(""),
            Credential::Bearer(TokenSource::from_secret(Some("api-token"))),
            RetryPolicy::NONE,
        );
        let url = Url::parse(&oauth.url("/token")).unwrap();
        let req = with_form(
            client.request_url(Method::POST, url),
            &[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", "a.b+c/d=e f*~"),
            ],
        );
        let reply: serde_json::Value = client.json(req, None).await.unwrap();
        assert_eq!(reply["access_token"], "ya29.x");
        let got = oauth.requests().pop().unwrap();
        assert_eq!(
            got.header("content-type"),
            Some("application/x-www-form-urlencoded")
        );
        assert_eq!(
            got.text(),
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer\
             &assertion=a.b%2Bc%2Fd%3De+f*%7E"
        );
        assert_eq!(got.header("authorization"), None);
        assert_eq!(form_body(&[]), "");
        assert_eq!(form_body(&[("é", "-._*")]), "%C3%A9=-._*");
    }

    #[tokio::test]
    async fn a_401_fetches_an_issued_token_again_once() {
        let server = FakeServer::start(vec![
            FakeRoute::new("GET", "/q")
                .auth("Bearer tok-2")
                .reply(Reply::json(200, &serde_json::json!({"ok": true}))),
        ])
        .await;
        let (token, fetches) =
            crate::drivers::common::token::tests::issued(Some(Duration::from_secs(3600)));
        let client = plain(
            &server.url(""),
            Credential::Bearer(token),
            RetryPolicy::NONE,
        );
        let reply: serde_json::Value = client.json(client.get("/q").unwrap(), None).await.unwrap();
        assert_eq!(reply, serde_json::json!({"ok": true}));
        assert_eq!(fetches.load(Ordering::SeqCst), 2);
        // Reused while it lasts.
        client.send(client.get("/q").unwrap(), None).await.unwrap();
        assert_eq!(fetches.load(Ordering::SeqCst), 2);
        assert_eq!(server.requests().len(), 3);
    }

    #[tokio::test]
    async fn a_proxy_carries_requests_but_never_to_a_tunnel_or_this_machine() {
        let proxy = FakeServer::start(vec![
            FakeRoute::new("*", "http://*").reply(Reply::text(200, "via proxy")),
        ])
        .await;
        let target = FakeServer::start(vec![FakeRoute::new("*", "/*").reply(Reply::ok())]).await;
        install_crypto();
        let through = |base: &str, tunnel: Option<SocketAddr>| {
            let options = HttpOptions {
                proxy: Some(proxy.url("")),
                ..HttpOptions::default()
            };
            HttpClient::new(
                HttpEndpoint::new(base, tunnel).unwrap(),
                options,
                Credential::None,
            )
            .unwrap()
        };
        // A server elsewhere: the request goes to the proxy, in absolute form.
        let client = through("http://db.example.test:9200", None);
        let reply = client
            .send(client.get("/x?y=1").unwrap(), None)
            .await
            .unwrap();
        assert_eq!(reply.text().await.unwrap(), "via proxy");
        let got = proxy.requests().pop().unwrap();
        assert_eq!(got.path, "http://db.example.test:9200/x");
        assert_eq!(got.query.as_deref(), Some("y=1"));
        // This machine and a tunnel are reached directly.
        let client = through(&target.url(""), None);
        client
            .send(client.get("/local").unwrap(), None)
            .await
            .unwrap();
        let client = through("http://db.example.test:9200", Some(target.addr()));
        client
            .send(client.get("/tunnel").unwrap(), None)
            .await
            .unwrap();
        let paths: Vec<String> = target.requests().into_iter().map(|r| r.path).collect();
        assert_eq!(paths, ["/local", "/tunnel"]);
        assert_eq!(proxy.requests().len(), 1);
        // Which endpoints a proxy, the system's too, may carry.
        let uses = |base: &str, tunnel: Option<SocketAddr>| {
            uses_proxy(&HttpEndpoint::new(base, tunnel).unwrap())
        };
        assert!(uses("https://search.example.com", None));
        assert!(uses("http://10.0.0.5:8123", None));
        for local in [
            "http://localhost:9200",
            "http://LOCALHOST",
            "http://es.localhost",
            "http://127.0.0.1",
            "http://127.0.0.2:1",
            "http://[::1]:9200",
            "http://[::ffff:127.0.0.1]",
        ] {
            assert!(!uses(local, None), "{local}");
        }
        assert!(!uses("https://search.example.com", Some(target.addr())));
        let bad = HttpOptions {
            proxy: Some("http://ann:s3cret@[bad".into()),
            ..HttpOptions::default()
        };
        let endpoint = HttpEndpoint::new("https://search.example.com", None).unwrap();
        let err = HttpClient::new(endpoint, bad, Credential::None).unwrap_err();
        assert_eq!(err.code(), "INVALID_INPUT");
        assert!(!err.to_string().contains("s3cret"), "{err}");
    }

    #[tokio::test]
    async fn a_client_certificate_is_shown_and_verification_can_be_skipped() {
        let server = TlsServer::requiring_client_cert();
        let base = format!("https://localhost:{}", server.addr().port());
        // Skip-verify (`require`): the test CA is trusted by nobody, and the server is reached.
        let skip = |identity| HttpTls {
            mode: SslMode::Require,
            ca_path: None,
            identity,
            pin_sha256: None,
        };
        let client = tls_client(&base, None, skip(Some(client_identity()))).unwrap();
        assert_eq!(get(&client).await.unwrap(), 200);
        let seen = server.requests().pop().unwrap();
        assert_eq!(
            seen.client_cert.as_deref(),
            Some(cert_der(CLIENT_CERT_PEM).as_ref())
        );
        // Without the certificate the server refuses the client.
        let client = tls_client(&base, None, skip(None)).unwrap();
        assert!(get(&client).await.is_err());
        // Verifying, the certificate is not enough: the server's CA is not trusted.
        let verified = HttpTls {
            identity: Some(client_identity()),
            ..HttpTls::verified()
        };
        let client = tls_client(&base, None, verified).unwrap();
        assert_eq!(get(&client).await.unwrap_err().code(), "CONNECT_FAILED");
        // With the CA file, both sides check out.
        let ca = ca_file();
        let both = HttpTls {
            mode: SslMode::VerifyFull,
            ca_path: Some(ca.path().to_string_lossy().into_owned()),
            identity: Some(client_identity()),
            pin_sha256: None,
        };
        let client = tls_client(&base, None, both).unwrap();
        assert_eq!(get(&client).await.unwrap(), 200);
        assert_eq!(server.requests().len(), 2);
    }

    /// Elasticsearch on Amazon OpenSearch: a path with an encoded id, the lane's own headers and
    /// a raw body, signed after they are final with every header, the body hash among them.
    #[cfg(feature = "common-aws")]
    #[tokio::test]
    async fn sigv4_signs_an_encoded_path_and_every_header_the_request_carries() {
        use super::super::aws::{CredentialProvider, Credentials, Signer, sigv4};
        let server = FakeServer::start(vec![FakeRoute::new("*", "/*").reply(Reply::ok())]).await;
        install_crypto();
        let creds = Credentials::static_keys("AKID", "secret", Some("session")).unwrap();
        let signer = Signer::new(
            "aoss",
            "us-east-1",
            Arc::new(CredentialProvider::fixed(creds.clone())),
        );
        let endpoint =
            HttpEndpoint::new("http://abc.us-east-1.aoss.test:9200", Some(server.addr())).unwrap();
        let client =
            HttpClient::new(endpoint, HttpOptions::default(), Credential::Sigv4(signer)).unwrap();
        let req = client
            .request_exact(
                Method::PUT,
                "/logs-*/_doc/a%2Fb%20c?refresh=wait_for&q=x%20y",
            )
            .unwrap()
            .header("x-opaque-id", "tabler-7")
            .header(CONTENT_TYPE, "application/json")
            .body(b"{\"a\":\"\xc3\xa9\"}".to_vec());
        assert_eq!(client.send(req, None).await.unwrap().status(), 200);
        let got = server.requests().pop().unwrap();
        assert_eq!(got.path, "/logs-*/_doc/a%2Fb%20c");
        let auth = got.header("authorization").unwrap();
        let names = auth
            .split("SignedHeaders=")
            .nth(1)
            .and_then(|r| r.split(',').next())
            .unwrap();
        assert_eq!(
            names,
            "content-type;host;x-amz-content-sha256;x-amz-date;x-amz-security-token;x-opaque-id"
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
            "PUT",
            &got.path,
            got.query.as_deref().unwrap_or(""),
            &headers,
            &got.body,
            &creds,
            at,
            "us-east-1",
            "aoss",
        );
        assert_eq!(auth, expected.authorization);
        assert!(
            expected
                .canonical_request
                .starts_with("PUT\n/logs-%2A/_doc/a%252Fb%2520c\n")
        );
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
            repeat: Repeat::Safe,
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
