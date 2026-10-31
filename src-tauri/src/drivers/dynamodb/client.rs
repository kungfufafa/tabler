//! Signed calls of DynamoDB's JSON 1.0 protocol: `POST /` with the action in `X-Amz-Target`, the
//! request as JSON, SigV4 over every header sent and the body.
//!
//! What is repeated, and when:
//! - throttling (`ProvisionedThroughputExceededException`, `ThrottlingException`,
//!   `RequestLimitExceeded`): the request did nothing, so it is sent again after an exponential
//!   pause with full jitter, up to `RetryPolicy::max_attempts`;
//! - server errors and lost connections: only for reads and for `TransactWriteItems` with a
//!   `ClientRequestToken` (DynamoDB applies such a transaction once), since a write may have
//!   been applied before the reply was lost;
//! - a signature the server finds too old or too new: once, with the clock corrected by the
//!   reply's `Date` (a user's clock that drifted);
//! - an expired session token: once, with credentials fetched again (when they can be, the
//!   Command mode's command run again).
//!
//! Cancel drops the request in flight (every call is a single request) or the pause. The
//! session's calls are kept in its trace for the console log; the console's own commands are not
//! (the editor logs them itself).

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio_util::sync::CancellationToken;

use super::errors::ServiceError;
use super::transport::{
    HttpRequest, HttpResponse, MAX_REPLY_MEMORY, Transport, header, parsed_size,
};
use crate::drivers::common::aws::credentials::{CredentialProvider, Credentials};
use crate::drivers::common::aws::sigv4::{amz_date, sign_parts};
use crate::drivers::{SqlTrace, TRACE_SQL_MAX_CHARS};
use crate::error::AppError;
use crate::secrets::CommandPassword;
use crate::sqlgen::native::dynamo::{is_read_action, request_line};
use crate::types::browse::{TraceCategory, TracedSql};

pub const TARGET_PREFIX: &str = "DynamoDB_20120810";
pub const CONTENT_TYPE: &str = "application/x-amz-json-1.0";
pub const SERVICE: &str = "dynamodb";
/// The most a reply's `Date` may move our clock.
const MAX_CLOCK_SKEW: chrono::TimeDelta = chrono::TimeDelta::days(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Tries in all, the first included.
    pub max_attempts: u32,
    /// The pause before the first retry may be up to this long; it doubles each time.
    pub base: Duration,
    /// No pause is longer.
    pub cap: Duration,
}

impl Default for RetryPolicy {
    fn default() -> RetryPolicy {
        RetryPolicy {
            max_attempts: 5,
            base: Duration::from_millis(50),
            cap: Duration::from_secs(5),
        }
    }
}

impl RetryPolicy {
    /// The pause before retry `retry` (0 for the first): a random length up to
    /// `min(cap, base * 2^retry)`.
    pub fn delay(&self, retry: u32) -> Duration {
        let ceiling = self
            .base
            .saturating_mul(1u32.checked_shl(retry.min(20)).unwrap_or(u32::MAX))
            .min(self.cap);
        let mut bytes = [0u8; 4];
        let fraction = match aws_lc_rs::rand::fill(&mut bytes) {
            Ok(()) => f64::from(u32::from_le_bytes(bytes)) / f64::from(u32::MAX),
            Err(_) => 1.0,
        };
        ceiling.mul_f64(fraction)
    }
}

/// What a call can end with besides a reply: DynamoDB's own error (kept whole, so a cancelled
/// transaction's reasons reach the caller) or Tabler's.
#[derive(Debug)]
pub enum CallError {
    Service(ServiceError),
    App(AppError),
}

impl CallError {
    pub fn into_app_error(self) -> AppError {
        match self {
            CallError::Service(e) => e.into_app_error(),
            CallError::App(e) => e,
        }
    }

    /// What the console log shows of it.
    fn text(&self) -> String {
        match self {
            CallError::Service(e) if e.message.is_empty() => e.code.clone(),
            CallError::Service(e) => format!("{}: {}", e.code, e.message),
            CallError::App(e) => e.to_string(),
        }
    }
}

impl From<AppError> for CallError {
    fn from(e: AppError) -> Self {
        CallError::App(e)
    }
}

/// The time requests are signed with (the system clock; tests pin it).
pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

pub struct Client {
    transport: Arc<dyn Transport>,
    url: String,
    /// `host[:port]` as the transport sends it (the default port left out).
    host: String,
    path: String,
    region: String,
    creds: Arc<CredentialProvider>,
    /// The Command mode's command, forgotten with the credentials it printed when DynamoDB
    /// refuses them as expired.
    command: Option<Arc<CommandPassword>>,
    trace: Option<SqlTrace>,
    retry: RetryPolicy,
    /// The server's clock minus ours, learned from a refused signature.
    skew_ms: AtomicI64,
    clock: Clock,
}

/// Runs `fut` unless `cancel` fires first.
async fn race<F: std::future::Future>(
    fut: F,
    cancel: Option<&CancellationToken>,
) -> Result<F::Output, AppError> {
    match cancel {
        None => Ok(fut.await),
        Some(token) => tokio::select! {
            out = fut => Ok(out),
            () = token.cancelled() => Err(AppError::Cancelled),
        },
    }
}

/// Whether a request may be sent again after a server error or a lost connection.
pub fn repeatable(action: &str, body: &serde_json::Value) -> bool {
    is_read_action(action)
        || (action == "TransactWriteItems"
            && body
                .get("ClientRequestToken")
                .and_then(|t| t.as_str())
                .is_some_and(|t| !t.is_empty()))
}

impl Client {
    /// A client of `endpoint` (from `DynamoParams::endpoint_url`) signing for `region`.
    pub fn new(
        transport: Arc<dyn Transport>,
        endpoint: &str,
        region: &str,
        creds: Arc<CredentialProvider>,
    ) -> Result<Client, AppError> {
        let url = reqwest::Url::parse(endpoint)
            .map_err(|_| AppError::InvalidInput(format!("DynamoDB endpoint {endpoint}")))?;
        let host = match (url.host_str(), url.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_string(),
            (None, _) => {
                return Err(AppError::InvalidInput(format!(
                    "DynamoDB endpoint {endpoint} has no host"
                )));
            }
        };
        Ok(Client {
            transport,
            url: url.to_string(),
            host,
            path: url.path().to_string(),
            region: region.to_string(),
            creds,
            command: None,
            trace: None,
            retry: RetryPolicy::default(),
            skew_ms: AtomicI64::new(0),
            clock: Arc::new(Utc::now),
        })
    }

    /// Tests pause less (a session keeps the default).
    #[cfg(test)]
    pub fn with_retry(mut self, retry: RetryPolicy) -> Client {
        self.retry = retry;
        self
    }

    /// Tests pin the clock (a session signs with the system's).
    #[cfg(test)]
    pub fn with_clock(mut self, clock: Clock) -> Client {
        self.clock = clock;
        self
    }

    pub fn with_command(mut self, command: Option<Arc<CommandPassword>>) -> Client {
        self.command = command;
        self
    }

    /// Records the calls in `trace` (all but [`Client::call_quiet`]'s).
    pub fn with_trace(mut self, trace: SqlTrace) -> Client {
        self.trace = Some(trace);
        self
    }

    pub fn endpoint(&self) -> &str {
        &self.url
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn credentials(&self) -> &CredentialProvider {
        &self.creds
    }

    /// Our clock corrected by what the server's says (chrono's `+` panics past year 262143).
    pub fn now(&self) -> DateTime<Utc> {
        let local = (self.clock)();
        chrono::TimeDelta::try_milliseconds(self.skew_ms.load(Ordering::Relaxed))
            .and_then(|skew| local.checked_add_signed(skew))
            .unwrap_or(local)
    }

    /// The signed request of `action` with `body`, at `at`.
    pub fn signed_request(
        &self,
        action: &str,
        body: Vec<u8>,
        creds: &Credentials,
        at: DateTime<Utc>,
    ) -> HttpRequest {
        let mut headers = vec![
            (header::CONTENT_TYPE.to_string(), CONTENT_TYPE.to_string()),
            (header::AMZ_DATE.to_string(), amz_date(at)),
            (
                header::TARGET.to_string(),
                format!("{TARGET_PREFIX}.{action}"),
            ),
        ];
        if let Some(token) = &creds.session_token {
            headers.push((header::SECURITY_TOKEN.to_string(), token.clone()));
        }
        let mut signed_headers = headers.clone();
        signed_headers.push(("host".to_string(), self.host.clone()));
        let signed = sign_parts(
            "POST",
            &self.path,
            "",
            &signed_headers,
            &body,
            creds,
            at,
            &self.region,
            SERVICE,
        );
        headers.push((header::AUTHORIZATION.to_string(), signed.authorization));
        HttpRequest {
            url: self.url.clone(),
            headers,
            body,
        }
    }

    /// Learns the server's clock from a reply's `Date`. A `Date` more than a day off is not a
    /// drifted clock but a broken or hostile server (AWS itself allows 5 minutes), so it is ignored.
    fn correct_clock(&self, resp: &HttpResponse) -> bool {
        let Some(server) = resp
            .header(header::DATE)
            .and_then(|d| DateTime::parse_from_rfc2822(d.trim()).ok())
        else {
            return false;
        };
        let skew = server
            .with_timezone(&Utc)
            .signed_duration_since((self.clock)());
        if skew.abs() > MAX_CLOCK_SKEW {
            return false;
        }
        self.skew_ms
            .store(skew.num_milliseconds(), Ordering::Relaxed);
        true
    }

    /// Calls `action`; the reply's JSON (`{}` for an empty body).
    pub async fn call(
        &self,
        action: &str,
        body: &serde_json::Value,
        cancel: Option<&CancellationToken>,
    ) -> Result<serde_json::Value, AppError> {
        self.call_detailed(action, body, cancel)
            .await
            .map_err(CallError::into_app_error)
    }

    /// [`Client::call`] for the console's own commands, which the session does not trace.
    pub async fn call_quiet(
        &self,
        action: &str,
        body: &serde_json::Value,
        cancel: Option<&CancellationToken>,
    ) -> Result<serde_json::Value, AppError> {
        self.send(action, body, cancel)
            .await
            .map_err(CallError::into_app_error)
    }

    /// [`Client::call`], with DynamoDB's own error kept whole.
    pub async fn call_detailed(
        &self,
        action: &str,
        body: &serde_json::Value,
        cancel: Option<&CancellationToken>,
    ) -> Result<serde_json::Value, CallError> {
        let started = Instant::now();
        let result = self.send(action, body, cancel).await;
        if let Some(trace) = &self.trace {
            trace.record(traced(action, body, started, &result));
        }
        result
    }

    /// One call with its retries.
    async fn send(
        &self,
        action: &str,
        body: &serde_json::Value,
        cancel: Option<&CancellationToken>,
    ) -> Result<serde_json::Value, CallError> {
        let payload = serde_json::to_vec(body).map_err(AppError::from)?;
        let repeat = repeatable(action, body);
        let mut retries = 0u32;
        let mut clock_corrected = false;
        let mut refreshed = false;
        loop {
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                return Err(AppError::Cancelled.into());
            }
            let now = self.now();
            let creds = race(self.creds.get(now), cancel).await??;
            let req = self.signed_request(action, payload.clone(), &creds, now);
            let sent = race(self.transport.send(req), cancel).await?;
            let resp = match sent {
                Ok(resp) => resp,
                Err(AppError::NotConnected) if repeat && retries + 1 < self.retry.max_attempts => {
                    self.pause(retries, cancel).await?;
                    retries += 1;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            if resp.is_success() {
                if resp.body.iter().all(u8::is_ascii_whitespace) {
                    return Ok(serde_json::json!({}));
                }
                if parsed_size(&resp.body) > MAX_REPLY_MEMORY {
                    return Err(AppError::db(format!(
                        "DynamoDB's {action} reply holds too many values to read at once (more \
                         than {} MB once read): ask for fewer items with a Limit or fewer keys",
                        MAX_REPLY_MEMORY >> 20
                    ))
                    .into());
                }
                return serde_json::from_slice(&resp.body).map_err(|e| {
                    CallError::App(AppError::db(format!(
                        "unexpected {action} reply from DynamoDB: {e}"
                    )))
                });
            }
            let err = ServiceError::from_response(&resp);
            if err.is_clock_skew() && !clock_corrected && self.correct_clock(&resp) {
                clock_corrected = true;
                continue;
            }
            if err.is_expired_token() && !refreshed && self.creds.refreshable() {
                refreshed = true;
                self.creds.invalidate().await;
                if let Some(command) = &self.command {
                    command.invalidate().await;
                }
                continue;
            }
            let retry = err.is_throttling() || (repeat && err.is_server_error());
            if retry && retries + 1 < self.retry.max_attempts {
                self.pause(retries, cancel).await?;
                retries += 1;
                continue;
            }
            return Err(CallError::Service(err));
        }
    }

    async fn pause(&self, retry: u32, cancel: Option<&CancellationToken>) -> Result<(), AppError> {
        race(tokio::time::sleep(self.retry.delay(retry)), cancel).await
    }
}

/// A call as the console log shows it: the request's text (capped), the items it returned or
/// counted, its error.
fn traced(
    action: &str,
    body: &serde_json::Value,
    started: Instant,
    result: &Result<serde_json::Value, CallError>,
) -> TracedSql {
    let mut sql = request_line(action, body);
    if let Some((cut, _)) = sql.char_indices().nth(TRACE_SQL_MAX_CHARS) {
        sql.truncate(cut);
        sql.push('…');
    }
    let category = if action.starts_with("List") || action.starts_with("Describe") {
        TraceCategory::Structure
    } else {
        TraceCategory::Data
    };
    let rows = result.as_ref().ok().and_then(|reply| {
        match (
            reply.get("Items").and_then(serde_json::Value::as_array),
            reply.get("Count"),
        ) {
            (Some(items), _) => Some(items.len() as u64),
            (None, Some(count)) => count.as_u64(),
            (None, None) => None,
        }
    });
    TracedSql {
        sql,
        category,
        duration_ms: started.elapsed().as_millis() as u64,
        rows,
        affected: None,
        error: result.as_ref().err().map(CallError::text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use chrono::TimeZone as _;

    use crate::drivers::common::aws::credentials::Fetch;
    use crate::drivers::dynamodb::auth;
    use crate::drivers::dynamodb::testing::parse_authorization;

    /// Replies in order (the last repeats) and records every request.
    struct Script {
        replies: Mutex<Vec<Result<HttpResponse, AppError>>>,
        seen: Mutex<Vec<HttpRequest>>,
    }

    impl Script {
        fn new(replies: Vec<Result<HttpResponse, AppError>>) -> Arc<Script> {
            Arc::new(Script {
                replies: Mutex::new(replies),
                seen: Mutex::new(Vec::new()),
            })
        }

        fn seen(&self) -> Vec<HttpRequest> {
            self.seen.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Transport for Script {
        async fn send(&self, req: HttpRequest) -> Result<HttpResponse, AppError> {
            self.seen.lock().unwrap().push(req);
            let mut replies = self.replies.lock().unwrap();
            if replies.len() > 1 {
                replies.remove(0)
            } else {
                replies[0].clone()
            }
        }
    }

    fn error(status: u16, code: &str, message: &str) -> Result<HttpResponse, AppError> {
        Ok(HttpResponse::json(
            status,
            &serde_json::json!({"__type": format!("com.amazonaws.dynamodb.v20120810#{code}"), "message": message}),
        ))
    }

    fn ok(body: serde_json::Value) -> Result<HttpResponse, AppError> {
        Ok(HttpResponse::json(200, &body))
    }

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 29, 10, 0, 0).unwrap()
    }

    fn client(script: &Arc<Script>, creds: Arc<CredentialProvider>) -> Client {
        Client::new(
            script.clone(),
            "https://dynamodb.us-east-1.amazonaws.com/",
            "us-east-1",
            creds,
        )
        .unwrap()
        .with_retry(RetryPolicy {
            max_attempts: 3,
            base: Duration::ZERO,
            cap: Duration::ZERO,
        })
        .with_clock(Arc::new(at))
    }

    fn keys() -> Arc<CredentialProvider> {
        Arc::new(CredentialProvider::fixed(
            Credentials::static_keys("AKID", "secret", Some("tok")).unwrap(),
        ))
    }

    #[tokio::test]
    async fn requests_are_signed_dynamodb_calls() {
        let script = Script::new(vec![ok(serde_json::json!({"TableNames": []}))]);
        let c = client(&script, keys());
        let reply = c
            .call("ListTables", &serde_json::json!({"Limit": 100}), None)
            .await
            .unwrap();
        assert_eq!(reply, serde_json::json!({"TableNames": []}));
        let req = &script.seen()[0];
        assert_eq!(req.url, "https://dynamodb.us-east-1.amazonaws.com/");
        assert_eq!(req.action(), Some("ListTables"));
        assert_eq!(req.header("content-type"), Some(CONTENT_TYPE));
        assert_eq!(req.header("x-amz-date"), Some("20261029T100000Z"));
        assert_eq!(req.header("x-amz-security-token"), Some("tok"));
        let (credential, signed, signature) =
            parse_authorization(req.header("authorization").unwrap()).unwrap();
        assert_eq!(credential, "AKID/20261029/us-east-1/dynamodb/aws4_request");
        assert_eq!(
            signed,
            [
                "content-type",
                "host",
                "x-amz-date",
                "x-amz-security-token",
                "x-amz-target"
            ]
        );
        // The signature covers the host the transport sends and the body.
        let mut headers: Vec<(String, String)> = req
            .headers
            .iter()
            .filter(|(k, _)| k != "authorization")
            .cloned()
            .collect();
        headers.push(("host".into(), "dynamodb.us-east-1.amazonaws.com".into()));
        let again = sign_parts(
            "POST",
            "/",
            "",
            &headers,
            br#"{"Limit":100}"#,
            &Credentials::static_keys("AKID", "secret", None).unwrap(),
            at(),
            "us-east-1",
            "dynamodb",
        );
        assert_eq!(again.signature, signature);
        assert!(again.canonical_request.contains(
            "content-type:application/x-amz-json-1.0\nhost:dynamodb.us-east-1.amazonaws.com\nx-amz-date:20261029T100000Z\nx-amz-security-token:tok\nx-amz-target:DynamoDB_20120810.ListTables\n"
        ));
        let local = Client::new(
            script.clone(),
            "http://127.0.0.1:8000/",
            "local",
            Arc::new(CredentialProvider::fixed(auth::local(None))),
        )
        .unwrap();
        assert_eq!(local.host, "127.0.0.1:8000");
        assert!(Client::new(script.clone(), "nonsense", "x", keys()).is_err());
    }

    #[tokio::test]
    async fn throttling_is_retried_then_reported() {
        let script = Script::new(vec![
            error(400, "ProvisionedThroughputExceededException", "slow down"),
            error(400, "ThrottlingException", "slow down"),
            ok(serde_json::json!({"Count": 0})),
        ]);
        let c = client(&script, keys());
        // Writes are retried too: a throttled request did nothing.
        c.call("PutItem", &serde_json::json!({}), None)
            .await
            .unwrap();
        assert_eq!(script.seen().len(), 3);

        let always = Script::new(vec![error(400, "RequestLimitExceeded", "too many")]);
        let c = client(&always, keys());
        match c.call("Scan", &serde_json::json!({}), None).await {
            Err(AppError::Db { detail, hint, .. }) => {
                assert_eq!(detail.as_deref(), Some("RequestLimitExceeded"));
                assert!(hint.is_some());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(always.seen().len(), 3);
    }

    #[tokio::test]
    async fn server_errors_repeat_only_what_can_be_repeated() {
        let script = Script::new(vec![
            error(500, "InternalServerError", "oops"),
            ok(serde_json::json!({})),
        ]);
        client(&script, keys())
            .call("Query", &serde_json::json!({}), None)
            .await
            .unwrap();
        assert_eq!(script.seen().len(), 2);

        let write = Script::new(vec![
            error(500, "InternalServerError", "oops"),
            ok(serde_json::json!({})),
        ]);
        assert!(
            client(&write, keys())
                .call("UpdateItem", &serde_json::json!({}), None)
                .await
                .is_err()
        );
        assert_eq!(write.seen().len(), 1);

        let tx = Script::new(vec![
            error(500, "InternalServerError", "oops"),
            ok(serde_json::json!({})),
        ]);
        client(&tx, keys())
            .call(
                "TransactWriteItems",
                &serde_json::json!({"ClientRequestToken": "t"}),
                None,
            )
            .await
            .unwrap();
        assert_eq!(tx.seen().len(), 2);

        let lost = Script::new(vec![Err(AppError::NotConnected), ok(serde_json::json!({}))]);
        client(&lost, keys())
            .call("GetItem", &serde_json::json!({}), None)
            .await
            .unwrap();
        let lost_write = Script::new(vec![Err(AppError::NotConnected), ok(serde_json::json!({}))]);
        assert!(matches!(
            client(&lost_write, keys())
                .call("DeleteItem", &serde_json::json!({}), None)
                .await,
            Err(AppError::NotConnected)
        ));
    }

    #[tokio::test]
    async fn a_drifted_clock_is_corrected_once() {
        let skewed = HttpResponse::json(
            400,
            &serde_json::json!({"__type": "com.amazon.coral.service#InvalidSignatureException", "message": "Signature expired: 20261029T100000Z is now earlier than 20261029T101500Z (20261029T102000Z - 5 min.)"}),
        )
        .with_header("Date", "Thu, 29 Oct 2026 10:20:00 GMT");
        let script = Script::new(vec![Ok(skewed), ok(serde_json::json!({}))]);
        let c = client(&script, keys());
        c.call("DescribeTable", &serde_json::json!({}), None)
            .await
            .unwrap();
        let seen = script.seen();
        assert_eq!(seen[1].header("x-amz-date"), Some("20261029T102000Z"));
        assert_eq!(
            c.now(),
            Utc.with_ymd_and_hms(2026, 10, 29, 10, 20, 0).unwrap()
        );

        // Refused again: the credentials are wrong after all.
        let bad = HttpResponse::json(
            400,
            &serde_json::json!({"__type": "com.amazon.coral.service#InvalidSignatureException", "message": "Signature not yet current"}),
        )
        .with_header("Date", "Thu, 29 Oct 2026 09:00:00 GMT");
        let script = Script::new(vec![Ok(bad)]);
        assert!(matches!(
            client(&script, keys())
                .call("Scan", &serde_json::json!({}), None)
                .await,
            Err(AppError::Auth(_))
        ));
        assert_eq!(script.seen().len(), 2);
    }

    /// A `Date` far from ours (here near chrono's last year, where adding to it panics) is not
    /// taken as the server's clock: the refusal is reported and later calls sign with ours.
    #[tokio::test]
    async fn an_implausible_server_date_is_ignored() {
        for date in [
            "31 Dec 262142 23:59:59 GMT",
            "Fri, 30 Oct 2026 10:00:01 GMT",
            "Thu, 1 Jan 1970 00:00:00 GMT",
        ] {
            let skewed = HttpResponse::json(
                400,
                &serde_json::json!({"__type": "com.amazon.coral.service#InvalidSignatureException", "message": "Signature expired: x"}),
            )
            .with_header("Date", date);
            let script = Script::new(vec![Ok(skewed), ok(serde_json::json!({}))]);
            let c = client(&script, keys());
            assert!(matches!(
                c.call("Scan", &serde_json::json!({}), None).await,
                Err(AppError::Auth(_))
            ));
            assert_eq!(script.seen().len(), 1, "{date}");
            assert_eq!(c.now(), at());
            c.call("Scan", &serde_json::json!({}), None).await.unwrap();
            assert_eq!(
                script.seen()[1].header("x-amz-date"),
                Some("20261029T100000Z")
            );
        }
        // Fetched credentials check their expiry against the same corrected clock.
        let fetch: Fetch = Arc::new(|| {
            Box::pin(async {
                auth::from_command_output(
                    r#"{"Version":1,"AccessKeyId":"A","SecretAccessKey":"s","SessionToken":"t","Expiration":"2026-10-29T11:00:00Z"}"#,
                    None,
                )
            })
        });
        let skewed = HttpResponse::json(
            400,
            &serde_json::json!({"__type": "com.amazon.coral.service#InvalidSignatureException", "message": "Signature expired: x"}),
        )
        .with_header("Date", "31 Dec 262142 23:59:59 GMT");
        let script = Script::new(vec![Ok(skewed), ok(serde_json::json!({}))]);
        let c = client(&script, Arc::new(CredentialProvider::fetching(fetch, None)));
        assert!(c.call("Scan", &serde_json::json!({}), None).await.is_err());
        c.call("Scan", &serde_json::json!({}), None).await.unwrap();
        // The clamp itself: an extreme skew stored by any path still leaves a usable clock.
        for extreme in [i64::MAX, i64::MIN] {
            c.skew_ms.store(extreme, Ordering::Relaxed);
            assert_eq!(c.now(), at());
        }
    }

    #[tokio::test]
    async fn expired_tokens_are_fetched_again_once() {
        let fetch: Fetch =
            Arc::new(|| Box::pin(async { Credentials::static_keys("ASIA", "s", Some("fresh")) }));
        let stale = Credentials::static_keys("ASIA", "s", Some("stale")).unwrap();
        let script = Script::new(vec![
            error(
                400,
                "ExpiredTokenException",
                "The security token included in the request is expired",
            ),
            ok(serde_json::json!({})),
        ]);
        let c = client(
            &script,
            Arc::new(CredentialProvider::fetching(fetch, Some(stale))),
        );
        c.call("Scan", &serde_json::json!({}), None).await.unwrap();
        let tokens: Vec<_> = script
            .seen()
            .iter()
            .map(|r| r.header("x-amz-security-token").unwrap().to_string())
            .collect();
        assert_eq!(tokens, ["stale", "fresh"]);

        let fixed = Script::new(vec![error(400, "ExpiredTokenException", "expired")]);
        assert!(matches!(
            client(&fixed, keys())
                .call("Scan", &serde_json::json!({}), None)
                .await,
            Err(AppError::Auth(_))
        ));
        assert_eq!(fixed.seen().len(), 1);
    }

    #[tokio::test]
    async fn cancel_stops_a_pause_and_a_request() {
        let script = Script::new(vec![error(400, "ThrottlingException", "slow")]);
        let c = client(&script, keys()).with_retry(RetryPolicy {
            max_attempts: 5,
            base: Duration::from_secs(60),
            cap: Duration::from_secs(60),
        });
        let token = CancellationToken::new();
        let canceller = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            canceller.cancel();
        });
        let started = std::time::Instant::now();
        let out = c.call("Scan", &serde_json::json!({}), Some(&token)).await;
        assert!(matches!(out, Err(AppError::Cancelled)), "{out:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
        // Cancelled before it starts: nothing is sent.
        let quiet = Script::new(vec![ok(serde_json::json!({}))]);
        assert!(matches!(
            client(&quiet, keys())
                .call("Scan", &serde_json::json!({}), Some(&token))
                .await,
            Err(AppError::Cancelled)
        ));
        assert!(quiet.seen().is_empty());
    }

    /// The Command mode's output is reused for its time to live; refused as expired, the command
    /// runs again (once).
    #[tokio::test]
    async fn an_expired_command_output_runs_the_command_again() {
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = runs.clone();
        let cmd = Arc::new(CommandPassword::with_fetch(
            Duration::from_secs(600),
            move || {
                let n = seen.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    Ok(format!(
                        "{{\"Version\":1,\"AccessKeyId\":\"A\",\"SecretAccessKey\":\"s\",\
                         \"SessionToken\":\"t{n}\"}}"
                    ))
                })
            },
        ));
        let script = Script::new(vec![
            error(400, "ExpiredTokenException", "expired"),
            ok(serde_json::json!({})),
        ]);
        let c = client(&script, auth::command_provider(cmd.clone(), None)).with_command(Some(cmd));
        c.call("Scan", &serde_json::json!({}), None).await.unwrap();
        let tokens: Vec<_> = script
            .seen()
            .iter()
            .map(|r| r.header("x-amz-security-token").unwrap().to_string())
            .collect();
        assert_eq!(tokens, ["t0", "t1"]);
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    /// The session's calls go into its trace with their item counts and errors; the console's
    /// commands (`call_quiet`) do not.
    #[tokio::test]
    async fn calls_are_traced_but_console_commands_are_not() {
        let script = Script::new(vec![
            ok(serde_json::json!({"Items": [{"a": {"S": "x"}}], "Count": 1})),
            error(400, "ResourceNotFoundException", "no such table"),
            ok(serde_json::json!({"Count": 7})),
        ]);
        let trace = SqlTrace::default();
        let c = client(&script, keys()).with_trace(trace.clone());
        c.call("Scan", &serde_json::json!({"TableName": "t"}), None)
            .await
            .unwrap();
        assert!(
            c.call(
                "DescribeTable",
                &serde_json::json!({"TableName": "x"}),
                None
            )
            .await
            .is_err()
        );
        c.call_quiet("Scan", &serde_json::json!({"Select": "COUNT"}), None)
            .await
            .unwrap();
        let entries = trace.drain();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].sql, r#"Scan {"TableName":"t"}"#);
        assert_eq!(
            (entries[0].category, entries[0].rows),
            (TraceCategory::Data, Some(1))
        );
        assert_eq!(entries[1].category, TraceCategory::Structure);
        assert_eq!(
            entries[1].error.as_deref(),
            Some("ResourceNotFoundException: no such table")
        );
        let long = "x".repeat(TRACE_SQL_MAX_CHARS + 10);
        let t = traced(
            "PutItem",
            &serde_json::json!({ "TableName": long }),
            Instant::now(),
            &Ok(serde_json::json!({})),
        );
        assert_eq!(t.sql.chars().count(), TRACE_SQL_MAX_CHARS + 1);
    }

    #[tokio::test]
    async fn replies_that_are_not_json_are_errors() {
        let script = Script::new(vec![Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: b"<html>".to_vec(),
        })]);
        assert!(matches!(
            client(&script, keys())
                .call("Scan", &serde_json::json!({}), None)
                .await,
            Err(AppError::Db { .. })
        ));
        let empty = Script::new(vec![Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: Vec::new(),
        })]);
        assert_eq!(
            client(&empty, keys())
                .call("DeleteItem", &serde_json::json!({}), None)
                .await
                .unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn delays_grow_and_stay_capped() {
        let p = RetryPolicy {
            max_attempts: 10,
            base: Duration::from_millis(100),
            cap: Duration::from_millis(400),
        };
        for retry in 0..40 {
            assert!(p.delay(retry) <= Duration::from_millis(400));
        }
        assert!(p.delay(0) <= Duration::from_millis(100));
    }
}
