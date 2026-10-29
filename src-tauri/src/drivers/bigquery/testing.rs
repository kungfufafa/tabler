//! Fakes for the tests: a scripted BigQuery transport (canned replies per method and path,
//! every request recorded), canned REST payloads, credential files with a test RSA key made
//! once per run, and a plain reqwest transport for the wire tests against
//! `testkit::fake_http`.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use aws_lc_rs::encoding::AsDer as _;
use aws_lc_rs::rsa::{KeyPair, KeySize};
use aws_lc_rs::signature::KeyPair as _;
use base64::Engine as _;
use serde_json::json;

use super::transport::{HttpRequest, HttpResponse, Method, Transport};
use crate::error::AppError;

/// A canned reply, sent after `delay`; `Err` is a transport failure with that message.
#[derive(Debug, Clone)]
pub struct FakeReply {
    pub reply: Result<HttpResponse, String>,
    pub delay: Duration,
}

impl FakeReply {
    pub fn after(reply: HttpResponse, delay: Duration) -> FakeReply {
        FakeReply {
            reply: Ok(reply),
            delay,
        }
    }

    pub fn broken(message: &str) -> FakeReply {
        FakeReply {
            reply: Err(message.to_string()),
            delay: Duration::ZERO,
        }
    }
}

impl From<HttpResponse> for FakeReply {
    fn from(reply: HttpResponse) -> Self {
        FakeReply {
            reply: Ok(reply),
            delay: Duration::ZERO,
        }
    }
}

struct Route {
    method: &'static str,
    /// Exact, or a prefix when it ends in `*`.
    path: String,
    replies: Vec<FakeReply>,
    served: usize,
}

#[derive(Default)]
struct State {
    routes: Vec<Route>,
    requests: Vec<HttpRequest>,
}

/// An in-process BigQuery: the replies of each method and path are used in order and the last
/// one repeats; an unknown path gets a 404 in Google's error envelope.
#[derive(Clone, Default)]
pub struct FakeBigquery {
    state: Arc<Mutex<State>>,
}

impl FakeBigquery {
    pub fn new() -> FakeBigquery {
        FakeBigquery::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn on(&self, method: &'static str, path: &str, reply: HttpResponse) -> &FakeBigquery {
        self.on_seq(method, path, vec![reply])
    }

    pub fn on_seq(
        &self,
        method: &'static str,
        path: &str,
        replies: Vec<HttpResponse>,
    ) -> &FakeBigquery {
        self.on_replies(
            method,
            path,
            replies.into_iter().map(FakeReply::from).collect(),
        )
    }

    pub fn on_replies(
        &self,
        method: &'static str,
        path: &str,
        replies: Vec<FakeReply>,
    ) -> &FakeBigquery {
        self.lock().routes.push(Route {
            method,
            path: path.to_string(),
            replies,
            served: 0,
        });
        self
    }

    pub fn transport(&self) -> Arc<dyn Transport> {
        Arc::new(self.clone())
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.lock().requests.clone()
    }

    /// The requests to paths that end with `suffix`.
    pub fn requests_to(&self, suffix: &str) -> Vec<HttpRequest> {
        self.requests()
            .into_iter()
            .filter(|r| r.path().ends_with(suffix))
            .collect()
    }
}

#[async_trait]
impl Transport for FakeBigquery {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, AppError> {
        let method = match req.method {
            Method::Get => "GET",
            Method::Post => "POST",
        };
        let path = req.path().to_string();
        let next = {
            let mut s = self.lock();
            s.requests.push(req);
            s.routes
                .iter_mut()
                .find(|r| {
                    r.method == method
                        && match r.path.strip_suffix('*') {
                            Some(prefix) => path.starts_with(prefix),
                            None => path == r.path,
                        }
                })
                .and_then(|r| {
                    let i = r.served.min(r.replies.len().checked_sub(1)?);
                    r.served += 1;
                    r.replies.get(i).cloned()
                })
        };
        let Some(next) = next else {
            return Ok(error_reply(
                404,
                "notFound",
                &format!("Not found: {method} {path}"),
            ));
        };
        if !next.delay.is_zero() {
            tokio::time::sleep(next.delay).await;
        }
        // A transport failure: the connection broke.
        next.reply.map_err(AppError::Connect)
    }
}

/// Google's error envelope.
pub fn error_reply(status: u16, reason: &str, message: &str) -> HttpResponse {
    HttpResponse::json(
        status,
        &json!({"error": {"code": status, "message": message, "status": "X",
            "errors": [{"reason": reason, "message": message, "domain": "global"}]}}),
    )
}

/// A schema of one field per `(name, type)`; `"STRING*"` is REPEATED.
pub fn schema(fields: &[(&str, &str)]) -> serde_json::Value {
    let fields: Vec<serde_json::Value> = fields
        .iter()
        .map(|(n, t)| match t.strip_suffix('*') {
            Some(t) => json!({"name": n, "type": t, "mode": "REPEATED"}),
            None => json!({"name": n, "type": t, "mode": "NULLABLE"}),
        })
        .collect();
    json!({ "fields": fields })
}

/// `{"f": [{"v": …}]}` rows from plain cells.
pub fn rows(cells: &[Vec<serde_json::Value>]) -> serde_json::Value {
    cells
        .iter()
        .map(|r| json!({"f": r.iter().map(|v| json!({"v": v})).collect::<Vec<_>>()}))
        .collect()
}

/// A `jobs.insert` reply for a job that runs.
pub fn job_inserted(project: &str, job_id: &str, location: &str) -> HttpResponse {
    HttpResponse::json(
        200,
        &json!({"kind": "bigquery#job", "id": format!("{project}:{location}.{job_id}"),
            "jobReference": {"projectId": project, "jobId": job_id, "location": location},
            "status": {"state": "RUNNING"}}),
    )
}

/// A `jobs.getQueryResults` reply of a job that still runs.
pub fn still_running(project: &str, job_id: &str) -> HttpResponse {
    HttpResponse::json(
        200,
        &json!({"kind": "bigquery#getQueryResultsResponse", "jobComplete": false,
            "jobReference": {"projectId": project, "jobId": job_id}}),
    )
}

/// A finished query's page.
pub fn results(
    schema: serde_json::Value,
    rows: serde_json::Value,
    total: u64,
    page_token: Option<&str>,
) -> HttpResponse {
    let mut r = json!({"kind": "bigquery#getQueryResultsResponse", "jobComplete": true,
        "schema": schema, "rows": rows, "totalRows": total.to_string(),
        "totalBytesProcessed": "1024", "cacheHit": false});
    if let Some(t) = page_token {
        r["pageToken"] = t.into();
    }
    HttpResponse::json(200, &r)
}

/// A test RSA key: its PKCS#8 PEM and its public key.
pub struct TestKey {
    pub pem: String,
    pub public_key: Vec<u8>,
}

/// One 2048-bit key per test run (made, never checked in).
pub fn test_key() -> &'static TestKey {
    static KEY: OnceLock<TestKey> = OnceLock::new();
    KEY.get_or_init(|| {
        let pair = KeyPair::generate(KeySize::Rsa2048).expect("an RSA test key");
        let der = pair.as_der().expect("its PKCS#8 form");
        let b64 = base64::engine::general_purpose::STANDARD.encode(der.as_ref());
        let lines: Vec<String> = b64
            .as_bytes()
            .chunks(64)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect();
        TestKey {
            pem: format!(
                "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
                lines.join("\n")
            ),
            public_key: pair.public_key().as_ref().to_vec(),
        }
    })
}

/// A service-account key file for `demo-proj` with the test key.
pub fn sa_key_json(token_uri: &str) -> String {
    json!({
        "type": "service_account",
        "project_id": "demo-proj",
        "private_key_id": "k1",
        "private_key": test_key().pem,
        "client_email": "tabler@demo-proj.iam.gserviceaccount.com",
        "client_id": "123",
        "auth_uri": "https://accounts.google.com/o/oauth2/auth",
        "token_uri": token_uri,
        "universe_domain": "googleapis.com"
    })
    .to_string()
}

/// The `authorized_user` file `gcloud auth application-default login` writes.
pub fn user_json(token_uri: &str) -> String {
    json!({
        "type": "authorized_user",
        "client_id": "764086051850-x.apps.googleusercontent.com",
        "client_secret": "client-secret",
        "refresh_token": "refresh-secret",
        "quota_project_id": "quota-proj",
        "token_uri": token_uri,
        "universe_domain": "googleapis.com"
    })
    .to_string()
}

/// reqwest without proxies (the requests must reach 127.0.0.1).
pub struct ReqwestTransport {
    http: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> ReqwestTransport {
        crate::testkit::install_crypto();
        ReqwestTransport {
            http: reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("a reqwest client"),
        }
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        ReqwestTransport::new()
    }
}

#[async_trait]
impl Transport for ReqwestTransport {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, AppError> {
        let mut b = match req.method {
            Method::Get => self.http.get(&req.url),
            Method::Post => self.http.post(&req.url),
        };
        for (k, v) in &req.headers {
            b = b.header(k, v);
        }
        if let Some(body) = req.body {
            b = b.body(body);
        }
        let resp = b.send().await.map_err(|e| {
            if e.is_timeout() {
                AppError::Timeout
            } else if e.is_connect() {
                AppError::Connect(e.to_string())
            } else {
                AppError::NotConnected
            }
        })?;
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body = resp
            .bytes()
            .await
            .map_err(|_| AppError::NotConnected)?
            .to_vec();
        Ok(HttpResponse {
            status,
            retry_after: None,
            content_type,
            body,
        })
    }
}
