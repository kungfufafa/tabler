//! The AI Assistant of the right sidebar: chat requests to the Claude API, an OpenAI-compatible
//! server or a local Ollama, streamed back to the renderer. What is sent about the database is
//! its STRUCTURE only (`context`): the request builders (`request`) take the agent's
//! instructions, the typed conversation and that rendered context — never rows or cell values.

pub mod context;
pub mod context_cache;
pub mod keys;
pub mod request;
pub mod stream;

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

use futures::StreamExt;
use serde_json::Value as Json;
use tokio_util::sync::CancellationToken;

use crate::error::AppError;
use crate::state::AppState;
use crate::types::assistant::{
    AssistantChatRequest, AssistantChatResult, AssistantContextPreview, AssistantContextRequest,
    AssistantEvent, AssistantFailure, AssistantFailureKind, AssistantProvider,
    AssistantProviderConfig, AssistantRole, AssistantTestResult,
};
use keys::KeyStore;
use request::HttpRequest;
use stream::{Decoder, StreamItem};

/// Requests in flight, by renderer request id.
static RUNNING: LazyLock<Mutex<HashMap<String, CancellationToken>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn running() -> std::sync::MutexGuard<'static, HashMap<String, CancellationToken>> {
    RUNNING.lock().unwrap_or_else(|p| p.into_inner())
}

/// A registered request; unregisters itself when dropped.
pub struct Running {
    id: String,
    pub token: CancellationToken,
}

impl Drop for Running {
    fn drop(&mut self) {
        running().remove(&self.id);
    }
}

pub fn register(request_id: &str) -> Result<Running, AppError> {
    if request_id.is_empty() || request_id.len() > 128 {
        return Err(AppError::InvalidInput("invalid request id".into()));
    }
    let mut map = running();
    if map.contains_key(request_id) {
        return Err(AppError::InvalidInput(format!(
            "request {request_id} is already running"
        )));
    }
    let token = CancellationToken::new();
    map.insert(request_id.to_string(), token.clone());
    Ok(Running {
        id: request_id.to_string(),
        token,
    })
}

/// Stops a running request; false when it is not running (finished already).
pub fn cancel(request_id: &str) -> bool {
    match running().get(request_id) {
        Some(token) => {
            token.cancel();
            true
        }
        None => false,
    }
}

fn client() -> Result<&'static reqwest::Client, AssistantFailure> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                // Between two chunks of a reply (providers send pings while the model thinks).
                .read_timeout(Duration::from_secs(300))
                .user_agent(concat!("Tabler/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| {
            AssistantFailure::new(AssistantFailureKind::Other, format!("HTTP client: {e}"))
        })
}

/// The structure context of `req` (see `context::gather`), reused for a minute per connection,
/// schema and table (`context_cache`; a schema refresh forgets it). A connection that is not open
/// gives an empty context with a warning.
pub async fn context_for(
    state: &AppState,
    req: &AssistantContextRequest,
) -> AssistantContextPreview {
    let key: context_cache::ContextKey = (req.connection_id, req.schema.clone(), req.table.clone());
    if let Some(cached) = context_cache::cache().get(&key, std::time::Instant::now()) {
        return cached;
    }
    let preview = gather_context(state, req).await;
    // Still open: a context of a connection closed meanwhile is not kept.
    if state.connection(req.connection_id).await.is_ok() {
        context_cache::cache().put(key, std::time::Instant::now(), &preview);
    }
    preview
}

async fn gather_context(
    state: &AppState,
    req: &AssistantContextRequest,
) -> AssistantContextPreview {
    let conn = match state.connection(req.connection_id).await {
        Ok(c) => c,
        Err(e) => {
            return AssistantContextPreview {
                text: String::new(),
                summary: "no database context".into(),
                tables: 0,
                truncated: false,
                warning: Some(format!("Could not read the structure: {e}")),
            };
        }
    };
    let info = conn.info().await;
    let facts = context::ServerFacts {
        server_version: info.server_version.clone(),
        current_database: info.current_database.clone(),
    };
    let session = conn.session().await;
    context::gather(
        session.as_ref(),
        &facts,
        req.schema.as_deref(),
        req.table.as_ref(),
    )
    .await
    .render()
}

fn host_of(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string())
}

fn network_failure(url: &str, e: &reqwest::Error) -> AssistantFailure {
    let host = host_of(url);
    let message = if e.is_timeout() {
        format!("{host} did not answer in time")
    } else if e.is_connect() {
        format!("Could not connect to {host}. Check the address and your network.")
    } else {
        format!("Network error talking to {host}: {e}")
    };
    AssistantFailure::new(AssistantFailureKind::Network, message)
}

/// The message of an error body (Claude API, OpenAI-compatible, Ollama shapes).
fn error_message(body: &str) -> Option<String> {
    let json: Json = serde_json::from_str(body).ok()?;
    let err = json.get("error")?;
    err.get("message")
        .and_then(Json::as_str)
        .or_else(|| err.as_str())
        .map(str::to_string)
}

/// Maps a non-success HTTP answer to a failure.
pub fn http_failure(
    provider: AssistantProvider,
    status: u16,
    retry_after: Option<&str>,
    body: &str,
) -> AssistantFailure {
    let detail = error_message(body).unwrap_or_else(|| {
        let trimmed = body.trim();
        let (short, _) = trimmed.split_at(
            trimmed
                .char_indices()
                .nth(300)
                .map_or(trimmed.len(), |(i, _)| i),
        );
        short.to_string()
    });
    let with = |s: &str| {
        if detail.is_empty() {
            s.to_string()
        } else {
            format!("{s}: {detail}")
        }
    };
    let (kind, message) = match status {
        401 | 403 => (
            AssistantFailureKind::Auth,
            with(match provider {
                AssistantProvider::Ollama => "The server refused the request",
                _ => "The API key was rejected",
            }),
        ),
        429 => {
            let wait = retry_after
                .and_then(|s| s.trim().parse::<u64>().ok())
                .map(|s| format!(" Try again in {s} s."))
                .unwrap_or_default();
            (
                AssistantFailureKind::RateLimit,
                format!("{}.{wait}", with("Rate limited")),
            )
        }
        400 | 404 | 413 | 422 => (AssistantFailureKind::BadRequest, with("Request rejected")),
        500..=599 => (
            AssistantFailureKind::Server,
            with(if status == 529 {
                "The service is overloaded"
            } else {
                "The server had an error"
            }),
        ),
        _ => (AssistantFailureKind::Other, with("Unexpected answer")),
    };
    AssistantFailure {
        kind,
        message,
        status: Some(status),
    }
}

async fn read_capped(resp: reqwest::Response, cap: usize) -> String {
    let mut out = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(Ok(chunk)) = stream.next().await {
        out.extend_from_slice(&chunk);
        if out.len() >= cap {
            out.truncate(cap);
            break;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn apply_headers(builder: reqwest::RequestBuilder, req: &HttpRequest) -> reqwest::RequestBuilder {
    req.headers.iter().fold(builder, |b, (name, value)| {
        b.header(name.as_str(), value.as_str())
    })
}

/// Sends `req` and streams the reply through `emit` until it ends, fails or `cancel` fires.
pub async fn stream_reply(
    provider: AssistantProvider,
    req: HttpRequest,
    emit: &(dyn Fn(AssistantEvent) + Send + Sync),
    cancel: &CancellationToken,
) -> AssistantChatResult {
    let mut result = AssistantChatResult {
        stop_reason: None,
        cancelled: false,
        error: None,
    };
    let client = match client() {
        Ok(c) => c,
        Err(f) => {
            result.error = Some(f);
            return result;
        }
    };
    let builder = apply_headers(client.post(&req.url), &req).body(req.body.to_string());
    let resp = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            result.cancelled = true;
            return result;
        }
        r = builder.send() => r,
    };
    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            result.error = Some(network_failure(&req.url, &e));
            return result;
        }
    };
    let status = resp.status();
    if !status.is_success() {
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body = read_capped(resp, 64 * 1024).await;
        result.error = Some(http_failure(
            provider,
            status.as_u16(),
            retry_after.as_deref(),
            &body,
        ));
        return result;
    }
    let mut decoder = Decoder::new(provider);
    let mut body = resp.bytes_stream();
    loop {
        let items = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                result.cancelled = true;
                return result;
            }
            next = body.next() => match next {
                Some(Ok(chunk)) => decoder.feed(&chunk),
                Some(Err(e)) => {
                    result.error = Some(network_failure(&req.url, &e));
                    return result;
                }
                None => decoder.finish(),
            },
        };
        for item in items {
            match item {
                StreamItem::Thinking => emit(AssistantEvent::Thinking),
                StreamItem::Text(text) => emit(AssistantEvent::Delta { text }),
                StreamItem::Notice(message) => emit(AssistantEvent::Notice { message }),
                StreamItem::Stop(reason) => result.stop_reason = reason,
                StreamItem::Failed(f) => result.error = Some(f),
            }
        }
        if decoder.is_done() {
            return result;
        }
    }
}

/// Checks a chat request and builds its HTTP request (the key comes from `keys`).
pub async fn prepare(
    keys: &KeyStore,
    req: &AssistantChatRequest,
    context: Option<&AssistantContextPreview>,
) -> Result<HttpRequest, AssistantFailure> {
    if req
        .messages
        .last()
        .is_none_or(|m| m.role != AssistantRole::User || m.content.trim().is_empty())
    {
        return Err(AssistantFailure::new(
            AssistantFailureKind::Config,
            "Nothing to send",
        ));
    }
    let provider = req.provider.provider;
    let key = match provider {
        AssistantProvider::Ollama => None,
        _ => keys
            .get(provider)
            .await
            .map_err(|e| AssistantFailure::new(AssistantFailureKind::Other, e.to_string()))?,
    };
    if provider == AssistantProvider::Anthropic && key.is_none() {
        return Err(AssistantFailure::new(
            AssistantFailureKind::MissingKey,
            "Add your Anthropic API key in Settings ▸ AI Assistant",
        ));
    }
    let system = request::system_text(&req.system_prompt, context.map(|c| c.text.as_str()));
    request::build(&req.provider, key.as_deref(), &system, &req.messages)
        .map_err(|m| AssistantFailure::new(AssistantFailureKind::Config, m))
}

/// Settings ▸ AI Assistant ▸ Test: checks the key / server and the model, without a chat.
pub async fn test_provider(
    keys: &KeyStore,
    config: &AssistantProviderConfig,
) -> AssistantTestResult {
    let fail = |message: String| AssistantTestResult {
        ok: false,
        message,
        models: Vec::new(),
    };
    let key = match config.provider {
        AssistantProvider::Ollama => None,
        p => match keys.get(p).await {
            Ok(k) => k,
            Err(e) => return fail(e.to_string()),
        },
    };
    if config.provider == AssistantProvider::Anthropic && key.is_none() {
        return fail("No API key stored".into());
    }
    let req = match request::test_request(config, key.as_deref()) {
        Ok(r) => r,
        Err(m) => return fail(m),
    };
    let client = match client() {
        Ok(c) => c,
        Err(f) => return fail(f.message),
    };
    let send = apply_headers(client.get(&req.url), &req)
        .timeout(Duration::from_secs(20))
        .send();
    let resp = match send.await {
        Ok(r) => r,
        Err(e) => return fail(network_failure(&req.url, &e).message),
    };
    let status = resp.status();
    let body = read_capped(resp, 1024 * 1024).await;
    if !status.is_success() {
        let f = http_failure(config.provider, status.as_u16(), None, &body);
        return fail(
            if status.as_u16() == 404 && config.provider == AssistantProvider::Anthropic {
                format!("Model {} was not found", config.model.trim())
            } else {
                f.message
            },
        );
    }
    let json: Json = serde_json::from_str(&body).unwrap_or(Json::Null);
    match config.provider {
        AssistantProvider::Anthropic => {
            let name = json
                .get("display_name")
                .and_then(Json::as_str)
                .unwrap_or(config.model.trim());
            AssistantTestResult {
                ok: true,
                message: format!("Connected — {name} is available"),
                models: Vec::new(),
            }
        }
        provider => {
            let models = request::model_list(provider, &json);
            let model = config.model.trim();
            let message = if model.is_empty() {
                format!("Connected — {} models; choose one", models.len())
            } else if models.is_empty() || models.iter().any(|m| m == model) {
                format!("Connected — {model} is available")
            } else {
                format!("Connected, but {model} is not in the server's model list")
            };
            AssistantTestResult {
                ok: !model.is_empty(),
                message,
                models,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::{self, ConnectTarget};
    use crate::sqlgen::Dialect;
    use crate::types::assistant::AssistantMessage;
    use crate::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
    use crate::types::schema::TableRef;
    use crate::types::value::Value;
    use uuid::Uuid;

    fn sqlite_profile(path: &str) -> ConnectionProfile {
        ConnectionProfile {
            id: Uuid::new_v4(),
            name: "assistant".into(),
            color: None,
            group: None,
            driver: Driver::Sqlite,
            host: None,
            port: None,
            user: None,
            database: None,
            ssl: SslMode::Disable,
            ssl_ca_path: None,
            sqlite_path: Some(path.to_string()),
            ssh: None,
            options: ProfileOptions::default(),
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        }
    }

    const SENTINELS: [&str; 4] = [
        "alice@sentinel.example",
        "ROW-SENTINEL-8472",
        "93817.55",
        "secret-cell-value",
    ];

    /// The structure of a table whose rows hold sentinel values reaches every provider's
    /// request; none of the values do, although a result holding them was just read.
    #[tokio::test]
    async fn requests_carry_structure_but_never_row_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shop.sqlite");
        let path = path.to_string_lossy().into_owned();
        crate::drivers::sqlite::create_database_file(&path)
            .await
            .unwrap();
        let session = drivers::connect(ConnectTarget {
            profile: &sqlite_profile(&path),
            password: None,
            host_override: None,
        })
        .await
        .unwrap();
        session
            .execute(
                "CREATE TABLE customers (id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE, \
                 note TEXT DEFAULT 'n/a', balance NUMERIC CHECK (balance >= 0))",
                &[],
            )
            .await
            .unwrap();
        session
            .execute("CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER REFERENCES customers(id), total NUMERIC)", &[])
            .await
            .unwrap();
        session
            .execute(
                "CREATE VIEW big_spenders AS SELECT email FROM customers WHERE balance > 1000",
                &[],
            )
            .await
            .unwrap();
        session
            .execute(
                "INSERT INTO customers (email, note, balance) VALUES (?1, ?2, ?3)",
                &[
                    Value::Text(SENTINELS[0].into()),
                    Value::Text(SENTINELS[1].into()),
                    Value::Text(SENTINELS[2].into()),
                ],
            )
            .await
            .unwrap();
        session
            .execute(
                "INSERT INTO orders (customer_id, total) VALUES (1, ?1)",
                &[Value::Text(SENTINELS[3].into())],
            )
            .await
            .unwrap();
        // A result holding the values exists in the same session.
        let result = session.query("SELECT * FROM customers", &[]).await.unwrap();
        assert!(format!("{:?}", result.rows).contains(SENTINELS[1]));

        let facts = context::ServerFacts {
            server_version: "3.50.0".into(),
            current_database: Some("main".into()),
        };
        let table = TableRef::new(Some("main"), "customers");
        let ctx = context::gather(session.as_ref(), &facts, None, Some(&table)).await;
        assert_eq!(ctx.dialect, Dialect::Sqlite);
        let preview = ctx.render();
        assert_eq!(preview.warning, None);
        assert!(
            preview.text.contains("CREATE TABLE customers"),
            "{}",
            preview.text
        );
        assert!(preview.text.contains("- orders: id, customer_id, total"));
        assert!(preview.text.contains("- big_spenders [view]: email"));

        let messages = vec![AssistantMessage {
            role: AssistantRole::User,
            content: "Which customers spend most?".into(),
        }];
        for (provider, model) in [
            (AssistantProvider::Anthropic, "claude-opus-5-5"),
            (AssistantProvider::Openai, "deepseek-chat"),
            (AssistantProvider::Ollama, "llama3.2"),
        ] {
            let config = AssistantProviderConfig {
                provider,
                model: model.into(),
                base_url: None,
            };
            let system = request::system_text("SQL helper", Some(&preview.text));
            let req = request::build(&config, Some("test-key"), &system, &messages).unwrap();
            let body = req.body.to_string();
            assert!(body.contains("CREATE TABLE customers"), "{provider:?}");
            assert!(body.contains("balance"), "{provider:?}");
            for value in SENTINELS {
                assert!(!body.contains(value), "{provider:?} leaked {value}");
            }
        }
        session.close().await;
    }

    #[tokio::test]
    async fn missing_anthropic_key_is_reported_before_any_request() {
        let keys = KeyStore::new(
            crate::secrets::Backend::File,
            crate::appdb::AppDb::open_in_memory().unwrap(),
        );
        let req = AssistantChatRequest {
            request_id: "r1".into(),
            provider: AssistantProviderConfig {
                provider: AssistantProvider::Anthropic,
                model: "claude-opus-5-5".into(),
                base_url: None,
            },
            system_prompt: String::new(),
            messages: vec![AssistantMessage {
                role: AssistantRole::User,
                content: "hi".into(),
            }],
            context: None,
        };
        let err = prepare(&keys, &req, None).await.unwrap_err();
        assert_eq!(err.kind, AssistantFailureKind::MissingKey);

        keys.set(AssistantProvider::Anthropic, Some("sk-ant-x"))
            .await
            .unwrap();
        let built = prepare(&keys, &req, None).await.unwrap();
        assert_eq!(built.header("x-api-key"), Some("sk-ant-x"));

        // Ollama needs no key; an OpenAI-compatible server may not either.
        let mut local = req.clone();
        local.provider = AssistantProviderConfig {
            provider: AssistantProvider::Ollama,
            model: "llama3.2".into(),
            base_url: Some("http://127.0.0.1:11434".into()),
        };
        assert!(prepare(&keys, &local, None).await.is_ok());
        local.messages.clear();
        assert_eq!(
            prepare(&keys, &local, None).await.unwrap_err().kind,
            AssistantFailureKind::Config
        );
    }

    #[test]
    fn http_failures_are_classified() {
        let f = http_failure(
            AssistantProvider::Anthropic,
            429,
            Some("12"),
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"Number of requests exceeded"}}"#,
        );
        assert_eq!(f.kind, AssistantFailureKind::RateLimit);
        assert_eq!(
            f.message,
            "Rate limited: Number of requests exceeded. Try again in 12 s."
        );
        assert_eq!(f.status, Some(429));
        let f = http_failure(
            AssistantProvider::Openai,
            401,
            None,
            r#"{"error":{"message":"Incorrect API key provided"}}"#,
        );
        assert_eq!(f.kind, AssistantFailureKind::Auth);
        assert_eq!(
            f.message,
            "The API key was rejected: Incorrect API key provided"
        );
        let f = http_failure(
            AssistantProvider::Ollama,
            404,
            None,
            r#"{"error":"model not found"}"#,
        );
        assert_eq!(f.kind, AssistantFailureKind::BadRequest);
        let f = http_failure(AssistantProvider::Anthropic, 529, None, "");
        assert_eq!(f.kind, AssistantFailureKind::Server);
        assert_eq!(f.message, "The service is overloaded");
    }

    #[test]
    fn requests_register_and_cancel_once() {
        let a = register("req-a").unwrap();
        assert!(register("req-a").is_err());
        assert!(cancel("req-a"));
        assert!(a.token.is_cancelled());
        drop(a);
        assert!(!cancel("req-a"));
        assert!(register("").is_err());
    }

    /// A local server streaming an OpenAI-style reply: the bytes go through the real client.
    #[tokio::test]
    async fn streams_from_a_local_server_and_stops_on_cancel() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for slow in [false, true] {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 16 * 1024];
                let _ = sock.read(&mut buf).await;
                let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n";
                sock.write_all(head.as_bytes()).await.unwrap();
                for part in ["Hello", " there"] {
                    let line = format!(
                        "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{part}\"}}}}]}}\n\n"
                    );
                    sock.write_all(line.as_bytes()).await.unwrap();
                    sock.flush().await.unwrap();
                }
                if slow {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
                sock.write_all(b"data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
                    .await
                    .unwrap();
            }
        });
        let config = AssistantProviderConfig {
            provider: AssistantProvider::Openai,
            model: "local".into(),
            base_url: Some(format!("http://{addr}/v1")),
        };
        let messages = vec![AssistantMessage {
            role: AssistantRole::User,
            content: "hi".into(),
        }];
        let texts = std::sync::Arc::new(Mutex::new(String::new()));
        let sink = texts.clone();
        let emit = move |e: AssistantEvent| {
            if let AssistantEvent::Delta { text } = e {
                sink.lock().unwrap().push_str(&text);
            }
        };
        let req = request::build(&config, None, "s", &messages).unwrap();
        let done = stream_reply(
            AssistantProvider::Openai,
            req,
            &emit,
            &CancellationToken::new(),
        )
        .await;
        assert_eq!(done.error, None);
        assert_eq!(done.stop_reason.as_deref(), Some("stop"));
        assert_eq!(texts.lock().unwrap().as_str(), "Hello there");

        // The second answer stalls after its text: cancelling returns at once.
        texts.lock().unwrap().clear();
        let token = CancellationToken::new();
        let stopper = token.clone();
        let wait_for_text = texts.clone();
        tokio::spawn(async move {
            while wait_for_text.lock().unwrap().len() < "Hello there".len() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            stopper.cancel();
        });
        let req = request::build(&config, None, "s", &messages).unwrap();
        let stopped = tokio::time::timeout(
            Duration::from_secs(10),
            stream_reply(AssistantProvider::Openai, req, &emit, &token),
        )
        .await
        .expect("cancel returns promptly");
        assert!(stopped.cancelled);
        assert_eq!(texts.lock().unwrap().as_str(), "Hello there");
    }

    #[tokio::test]
    async fn unreachable_servers_are_network_failures() {
        // A port nothing listens on.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let config = AssistantProviderConfig {
            provider: AssistantProvider::Ollama,
            model: "m".into(),
            base_url: Some(format!("http://127.0.0.1:{port}")),
        };
        let messages = vec![AssistantMessage {
            role: AssistantRole::User,
            content: "hi".into(),
        }];
        let req = request::build(&config, None, "s", &messages).unwrap();
        let result = stream_reply(
            AssistantProvider::Ollama,
            req,
            &|_| {},
            &CancellationToken::new(),
        )
        .await;
        assert_eq!(
            result.error.map(|e| e.kind),
            Some(AssistantFailureKind::Network)
        );
    }
}
