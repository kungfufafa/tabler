//! JSON-RPC 2.0 over stdio, as the Model Context Protocol's stdio transport defines it: one
//! message per line (UTF-8, no embedded newlines) on stdin / stdout; logs go to stderr.
//!
//! Implemented: `initialize` (with protocol version negotiation), `ping`, `tools/list`,
//! `tools/call`, and the `notifications/initialized` / `notifications/cancelled` notifications.
//! Requests run concurrently, so a long query does not hold up a `ping` or its own cancellation;
//! a cancelled request gets no response, as the protocol asks.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value as Json, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::tools::{self, CallCancel, Ctx, UnknownTool};

/// The protocol revision this server implements (and answers with when the client asks for one
/// it does not know).
pub const LATEST_PROTOCOL: &str = "2025-06-18";
/// Revisions a client may pick; the tools part is the same in all of them.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
/// Implementation-defined server error (the JSON-RPC range -32000..-32099): too many requests.
pub const SERVER_BUSY: i64 = -32000;

/// The longest request line read (an `sql` argument is at most 1 MiB, so this leaves room for the
/// JSON around it and for a client that escapes every character).
pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
/// Requests handled at once; the next one is answered with [`SERVER_BUSY`] instead of queueing.
pub const MAX_IN_FLIGHT: usize = 16;
/// Bytes of a too-long line read (and dropped) at a time.
const DISCARD_CHUNK: usize = 64 * 1024;

pub const TOO_LARGE_MESSAGE: &str = "Request too large";
pub const BUSY_MESSAGE: &str = "Too many requests in flight";

/// What `serve` bounds: a line, and the requests being handled.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_line_bytes: usize,
    pub max_in_flight: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_line_bytes: MAX_LINE_BYTES,
            max_in_flight: MAX_IN_FLIGHT,
        }
    }
}

/// How often idle connections are looked for.
const REAP_EVERY: Duration = Duration::from_secs(30);
/// How long calls still running get once the client closed stdin.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

const INSTRUCTIONS: &str = "Tabler gives access to the database connections saved in the Tabler \
     app (PostgreSQL, MySQL / MariaDB, SQLite). Start with list_connections, then list_tables / \
     describe_table to learn the schema before writing SQL with run_query. Queries are read-only \
     unless the user allowed changes for a connection; results are capped, so filter and limit \
     in SQL.";

fn response(id: &Json, result: Json) -> Json {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: &Json, code: i64, message: impl Into<String>) -> Json {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

/// The protocol version to answer `initialize` with.
pub fn negotiate(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|r| PROTOCOL_VERSIONS.iter().find(|v| **v == r).copied())
        .unwrap_or(LATEST_PROTOCOL)
}

pub struct Server {
    ctx: Arc<Ctx>,
    /// Calls running now, by request id (as JSON text).
    inflight: Mutex<HashMap<String, Arc<CallCancel>>>,
    initialized: AtomicBool,
}

impl Server {
    pub fn new(ctx: Arc<Ctx>) -> Server {
        Server {
            ctx,
            inflight: Mutex::new(HashMap::new()),
            initialized: AtomicBool::new(false),
        }
    }

    pub fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    /// Whether the client sent `notifications/initialized`.
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::SeqCst)
    }

    /// One line from the client; the line to answer with, if any.
    pub async fn handle_line(&self, line: &str) -> Option<String> {
        let reply = match serde_json::from_str::<Json>(line) {
            Ok(msg) => self.handle(msg).await?,
            Err(e) => error(&Json::Null, PARSE_ERROR, format!("Parse error: {e}")),
        };
        Some(reply.to_string())
    }

    /// One message (or a batch, from clients of the 2025-03-26 revision).
    pub async fn handle(&self, msg: Json) -> Option<Json> {
        match msg {
            Json::Array(batch) if batch.is_empty() => Some(error(
                &Json::Null,
                INVALID_REQUEST,
                "Invalid Request: empty batch",
            )),
            Json::Array(batch) => {
                let mut out = Vec::new();
                for m in batch {
                    if let Some(r) = Box::pin(self.handle_one(m)).await {
                        out.push(r);
                    }
                }
                (!out.is_empty()).then_some(Json::Array(out))
            }
            other => self.handle_one(other).await,
        }
    }

    async fn handle_one(&self, msg: Json) -> Option<Json> {
        let Json::Object(obj) = &msg else {
            return Some(error(&Json::Null, INVALID_REQUEST, "Invalid Request"));
        };
        let id = obj.get("id").cloned();
        let Some(method) = obj.get("method").and_then(Json::as_str) else {
            // A response to a request of ours (there are none), or garbage.
            return match id {
                Some(id) if !obj.contains_key("result") && !obj.contains_key("error") => {
                    Some(error(&id, INVALID_REQUEST, "Invalid Request: no method"))
                }
                _ => None,
            };
        };
        if obj.get("jsonrpc").and_then(Json::as_str) != Some("2.0") {
            return id.map(|id| {
                error(
                    &id,
                    INVALID_REQUEST,
                    "Invalid Request: jsonrpc must be \"2.0\"",
                )
            });
        }
        let params = obj.get("params").cloned().unwrap_or(Json::Null);
        let Some(id) = id else {
            self.notification(method, &params).await;
            return None;
        };
        if !matches!(id, Json::String(_) | Json::Number(_)) {
            return Some(error(
                &Json::Null,
                INVALID_REQUEST,
                "Invalid Request: bad id",
            ));
        }
        Some(match method {
            "initialize" => {
                let requested = params.get("protocolVersion").and_then(Json::as_str);
                response(
                    &id,
                    json!({
                        "protocolVersion": negotiate(requested),
                        "capabilities": { "tools": { "listChanged": false } },
                        "serverInfo": {
                            "name": super::SERVER_NAME,
                            "title": "Tabler",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                        "instructions": INSTRUCTIONS,
                    }),
                )
            }
            "ping" => response(&id, json!({})),
            "tools/list" => response(&id, json!({ "tools": tools::definitions() })),
            "tools/call" => return self.call(id, &params).await,
            other => error(&id, METHOD_NOT_FOUND, format!("Method not found: {other}")),
        })
    }

    async fn notification(&self, method: &str, params: &Json) {
        match method {
            "notifications/initialized" => self.initialized.store(true, Ordering::SeqCst),
            "notifications/cancelled" => {
                let Some(id) = params.get("requestId") else {
                    return;
                };
                let call = self
                    .inflight
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .get(&id.to_string())
                    .cloned();
                if let Some(call) = call {
                    call.cancel().await;
                }
            }
            _ => {}
        }
    }

    async fn call(&self, id: Json, params: &Json) -> Option<Json> {
        let Some(name) = params.get("name").and_then(Json::as_str) else {
            return Some(error(
                &id,
                INVALID_PARAMS,
                "Invalid params: `name` is required",
            ));
        };
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        let key = id.to_string();
        let cancel = Arc::new(CallCancel::default());
        self.inflight
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key.clone(), cancel.clone());
        let outcome = tools::call(&self.ctx, name, &args, &cancel).await;
        self.inflight
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key);
        if cancel.is_cancelled() {
            return None;
        }
        Some(match outcome {
            Ok(out) => response(
                &id,
                json!({
                    "content": [{ "type": "text", "text": out.text }],
                    "isError": out.is_error,
                }),
            ),
            Err(UnknownTool(name)) => error(&id, INVALID_PARAMS, format!("Unknown tool: {name}")),
        })
    }

    /// Cancels every running call (the client went away).
    pub async fn cancel_all(&self) {
        let calls: Vec<Arc<CallCancel>> = self
            .inflight
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect();
        for call in calls {
            call.cancel().await;
        }
    }
}

/// One line read from the client.
pub(super) enum Line {
    /// The client closed its end.
    Eof,
    /// A line (without its newline) of at most the limit.
    Text(Vec<u8>),
    /// A line over the limit: it was dropped unread, up to its end.
    TooLong,
}

/// Reads one line of at most `max` bytes (newline excluded). A longer one is not kept: what is
/// read of it is dropped, and the rest of the line is read away in chunks and dropped too, so a
/// client cannot make the server hold more than `max` bytes of one request.
pub(super) async fn read_line<R>(reader: &mut R, max: usize) -> std::io::Result<Line>
where
    R: AsyncBufRead + Unpin,
{
    let mut buf = Vec::new();
    // One byte more than `max`: a full line plus its newline fits, anything longer shows.
    let read = (&mut *reader)
        .take(max as u64 + 1)
        .read_until(b'\n', &mut buf)
        .await?;
    if read == 0 {
        return Ok(Line::Eof);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        return Ok(Line::Text(buf));
    }
    if buf.len() <= max {
        // The client closed its end in the middle of a line: that is still a line.
        return Ok(Line::Text(buf));
    }
    drop(buf);
    discard_line(reader).await?;
    Ok(Line::TooLong)
}

/// Reads and drops everything up to and including the next newline (or the end of the input).
async fn discard_line<R>(reader: &mut R) -> std::io::Result<()>
where
    R: AsyncBufRead + Unpin,
{
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            return Ok(());
        }
        let step = chunk.len().min(DISCARD_CHUNK);
        match chunk[..step].iter().position(|b| *b == b'\n') {
            Some(at) => {
                reader.consume(at + 1);
                return Ok(());
            }
            None => reader.consume(step),
        }
    }
}

/// Splits what `serve` does with a line it has no free slot for: the replies for the requests in
/// it (`Too many requests in flight`, with their ids) and what is left to handle (notifications
/// such as `notifications/cancelled` must still be processed, the more so while the server is busy).
pub(super) fn refuse_busy(line: &str) -> (Vec<String>, Option<String>) {
    let Ok(msg) = serde_json::from_str::<Json>(line) else {
        // A parse error is answered by `handle_line`, which costs nothing.
        return (Vec::new(), Some(line.to_string()));
    };
    let request_id = |m: &Json| match m {
        Json::Object(o) if o.contains_key("method") => {
            o.get("id").filter(|id| !id.is_null()).cloned()
        }
        _ => None,
    };
    match msg {
        Json::Array(batch) => {
            let mut refused = Vec::new();
            let mut rest = Vec::new();
            for m in batch {
                match request_id(&m) {
                    Some(id) => refused.push(error(&id, SERVER_BUSY, BUSY_MESSAGE)),
                    None => rest.push(m),
                }
            }
            // The answer to a batch is a batch, in one line.
            let replies = if refused.is_empty() {
                Vec::new()
            } else {
                vec![Json::Array(refused).to_string()]
            };
            let rest = (!rest.is_empty()).then(|| Json::Array(rest).to_string());
            (replies, rest)
        }
        other => match request_id(&other) {
            Some(id) => (
                vec![error(&id, SERVER_BUSY, BUSY_MESSAGE).to_string()],
                None,
            ),
            None => (Vec::new(), Some(line.to_string())),
        },
    }
}

/// Serves `reader` / `writer` until the client closes its end, then closes every connection.
pub async fn serve<R, W>(server: Arc<Server>, reader: R, writer: W)
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    serve_with(server, reader, writer, Limits::default()).await
}

/// [`serve`] with explicit limits: a request line of more than `max_line_bytes` is answered with
/// `Request too large` (-32600) and dropped, and more than `max_in_flight` requests at once are
/// answered with `Too many requests in flight` (-32000) instead of being queued. The reader never
/// waits for a free slot: a `notifications/cancelled` behind a busy server is still read.
pub async fn serve_with<R, W>(server: Arc<Server>, mut reader: R, writer: W, limits: Limits)
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let writer_task = tokio::spawn(async move {
        let mut writer = writer;
        while let Some(line) = rx.recv().await {
            let written = async {
                writer.write_all(line.as_bytes()).await?;
                writer.write_all(b"\n").await?;
                writer.flush().await
            }
            .await;
            if let Err(e) = written {
                log::warn!("mcp: cannot write to the client: {e}");
                break;
            }
        }
    });
    let reaper = tokio::spawn({
        let server = server.clone();
        async move {
            loop {
                tokio::time::sleep(REAP_EVERY).await;
                server.ctx.pool.reap().await;
            }
        }
    });
    let slots = Arc::new(Semaphore::new(limits.max_in_flight));
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        let bytes = match read_line(&mut reader, limits.max_line_bytes).await {
            Ok(Line::Eof) => break,
            Ok(Line::Text(bytes)) => bytes,
            Ok(Line::TooLong) => {
                let _ = tx.send(error(&Json::Null, INVALID_REQUEST, TOO_LARGE_MESSAGE).to_string());
                continue;
            }
            Err(e) => {
                log::warn!("mcp: cannot read from the client: {e}");
                break;
            }
        };
        let line = String::from_utf8_lossy(&bytes).trim().to_string();
        drop(bytes);
        if line.is_empty() {
            continue;
        }
        let permit = slots.clone().try_acquire_owned().ok();
        let line = if permit.is_some() {
            line
        } else {
            let (replies, rest) = refuse_busy(&line);
            for reply in replies {
                let _ = tx.send(reply);
            }
            match rest {
                Some(rest) => rest,
                None => continue,
            }
        };
        let (server, tx) = (server.clone(), tx.clone());
        tasks.spawn(async move {
            // Held until the request is answered (None: a notification, which needs no slot).
            let _permit: Option<OwnedSemaphorePermit> = permit;
            if let Some(reply) = server.handle_line(&line).await {
                let _ = tx.send(reply);
            }
        });
        while tasks.try_join_next().is_some() {}
    }
    server.cancel_all().await;
    let _ = tokio::time::timeout(SHUTDOWN_GRACE, async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    tasks.abort_all();
    reaper.abort();
    server.ctx.pool.close_all().await;
    drop(tx);
    let _ = writer_task.await;
}
