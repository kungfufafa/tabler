//! An in-process HTTP/1.1 server for the unit tests of the HTTP drivers: canned replies per
//! route, every request recorded, and slow, streamed or broken replies for the cancel, retry and
//! token-refresh paths. One request per connection (`connection: close`), like the streaming
//! server of the assistant's tests.
//!
//! ```ignore
//! let server = FakeServer::start(vec![
//!     Route::new("POST", "/query").reply(Reply::json(200, &json!({"rows": []}))),
//!     Route::new("GET", "/slow").reply(Reply::ok().delay(Duration::from_secs(30))),
//! ])
//! .await;
//! let url = server.url("/query");
//! // … drive the client, then:
//! assert_eq!(server.requests()[0].header("authorization"), Some("Bearer t"));
//! ```
//!
//! A route can also answer through a handler, so an in-memory engine (a driver's fake) is served
//! over a real socket: `Route::new("*", "/*").handler(move |req| engine.reply(req))`.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// One piece of a chunked reply body.
#[derive(Debug, Clone)]
pub enum Piece {
    /// Bytes, sent as one chunk.
    Data(Vec<u8>),
    /// A pause; the server notices a client that goes away meanwhile.
    Wait(Duration),
    /// The server drops the connection here, before the body is complete.
    Close,
}

#[derive(Debug, Clone)]
pub enum Body {
    /// Sent whole, with `content-length`.
    Full(Vec<u8>),
    /// Sent piece by piece with `transfer-encoding: chunked`.
    Chunked(Vec<Piece>),
}

/// A canned reply.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// Waited before anything is sent.
    pub delay: Duration,
    pub body: Body,
}

impl Reply {
    pub fn new(status: u16) -> Reply {
        Reply {
            status,
            headers: Vec::new(),
            delay: Duration::ZERO,
            body: Body::Full(Vec::new()),
        }
    }

    pub fn ok() -> Reply {
        Reply::new(200)
    }

    pub fn text(status: u16, body: impl Into<String>) -> Reply {
        Reply::new(status)
            .header("content-type", "text/plain; charset=utf-8")
            .body(body.into())
    }

    pub fn json(status: u16, value: &serde_json::Value) -> Reply {
        Reply::new(status)
            .header("content-type", "application/json")
            .body(value.to_string())
    }

    /// 401 with a bearer challenge: what a server answers an expired or wrong token.
    pub fn unauthorized() -> Reply {
        Reply::text(401, "unauthorized").header("www-authenticate", "Bearer")
    }

    pub fn header(mut self, name: &str, value: &str) -> Reply {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Reply {
        self.body = Body::Full(body.into());
        self
    }

    pub fn delay(mut self, delay: Duration) -> Reply {
        self.delay = delay;
        self
    }

    pub fn chunked(mut self, pieces: Vec<Piece>) -> Reply {
        self.body = Body::Chunked(pieces);
        self
    }
}

/// Makes a route's reply from the request.
#[derive(Clone)]
pub struct Handler(Arc<dyn Fn(&Request) -> Reply + Send + Sync>);

impl std::fmt::Debug for Handler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Handler")
    }
}

/// The replies of one method and path. `method` `"*"` takes any method; a `path` ending in `*`
/// takes every path with that prefix; the query string is not part of the match. Replies are
/// used in order and the last one repeats (none: an empty 200), unless a handler makes them.
#[derive(Debug, Clone)]
pub struct Route {
    method: String,
    path: String,
    /// The `authorization` header the route requires; a request without it gets a 401.
    auth: Option<String>,
    replies: Vec<Reply>,
    handler: Option<Handler>,
    served: usize,
}

/// What a matched route answers: a canned reply, or its handler's for the request.
enum Next {
    Reply(Reply),
    Handle(Handler),
}

impl Route {
    pub fn new(method: &str, path: &str) -> Route {
        Route {
            method: method.to_string(),
            path: path.to_string(),
            auth: None,
            replies: Vec::new(),
            handler: None,
            served: 0,
        }
    }

    pub fn reply(mut self, reply: Reply) -> Route {
        self.replies.push(reply);
        self
    }

    /// Answers every request with what `handle` makes of it (canned replies are then unused).
    /// It runs on the server's task, outside the server's locks, once per request.
    pub fn handler(mut self, handle: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Route {
        self.handler = Some(Handler(Arc::new(handle)));
        self
    }

    pub fn auth(mut self, authorization: &str) -> Route {
        self.auth = Some(authorization.to_string());
        self
    }

    fn matches(&self, req: &Request) -> bool {
        let method = self.method == "*" || self.method.eq_ignore_ascii_case(&req.method);
        let path = match self.path.strip_suffix('*') {
            Some(prefix) => req.path.starts_with(prefix),
            None => req.path == self.path,
        };
        method && path
    }

    fn next_reply(&mut self, req: &Request) -> Next {
        if let Some(auth) = &self.auth
            && req.header("authorization") != Some(auth.as_str())
        {
            return Next::Reply(Reply::unauthorized());
        }
        let served = self.served;
        self.served += 1;
        if let Some(handler) = &self.handler {
            return Next::Handle(handler.clone());
        }
        Next::Reply(match self.replies.len() {
            0 => Reply::ok(),
            n => self.replies[served.min(n - 1)].clone(),
        })
    }
}

/// What became of a request's reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Still being sent (or waiting out a delay).
    Replying,
    /// Sent completely.
    Sent,
    /// The client went away before the reply was complete.
    Aborted,
    /// The server broke the reply off (`Piece::Close`).
    Closed,
}

/// A recorded request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    /// The path without the query string.
    pub path: String,
    pub query: Option<String>,
    /// In arrival order, names lower-cased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub outcome: Outcome,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("a JSON request body")
    }
}

/// The server; it stops when dropped.
pub struct FakeServer {
    addr: SocketAddr,
    log: Arc<Mutex<Vec<Request>>>,
    accept: JoinHandle<()>,
}

impl FakeServer {
    /// Listens on a free port of 127.0.0.1. A request no route takes gets a 404.
    pub async fn start(routes: Vec<Route>) -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local address");
        let routes = Arc::new(Mutex::new(routes));
        let log = Arc::new(Mutex::new(Vec::new()));
        let accept = {
            let log = log.clone();
            tokio::spawn(async move {
                while let Ok((sock, _)) = listener.accept().await {
                    tokio::spawn(serve(sock, routes.clone(), log.clone()));
                }
            })
        };
        FakeServer { addr, log, accept }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://127.0.0.1:<port><path>`.
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// The requests so far, in arrival order.
    pub fn requests(&self) -> Vec<Request> {
        self.log.lock().unwrap().clone()
    }

    /// Waits until `done` holds for the requests (the server records an outcome a little after
    /// the client has seen it); panics after `timeout`.
    pub async fn wait_until(
        &self,
        timeout: Duration,
        done: impl Fn(&[Request]) -> bool,
    ) -> Vec<Request> {
        let deadline = Instant::now() + timeout;
        loop {
            let requests = self.requests();
            if done(&requests) {
                return requests;
            }
            if Instant::now() >= deadline {
                panic!("timed out after {timeout:?}; requests: {requests:#?}");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

async fn serve(mut sock: TcpStream, routes: Arc<Mutex<Vec<Route>>>, log: Arc<Mutex<Vec<Request>>>) {
    let Some(req) = read_request(&mut sock).await else {
        return;
    };
    let next = {
        let mut routes = routes.lock().unwrap();
        match routes.iter_mut().find(|r| r.matches(&req)) {
            Some(route) => route.next_reply(&req),
            None => Next::Reply(Reply::text(
                404,
                format!("no route for {} {}", req.method, req.path),
            )),
        }
    };
    // A handler may take its engine's own locks: the routes' is released by now.
    let reply = match next {
        Next::Reply(reply) => reply,
        Next::Handle(Handler(handle)) => handle(&req),
    };
    let index = {
        let mut log = log.lock().unwrap();
        log.push(req);
        log.len() - 1
    };
    let outcome = send(&mut sock, &reply).await;
    log.lock().unwrap()[index].outcome = outcome;
}

/// Reads one request (a `content-length` or chunked body); `None` when the client went away.
async fn read_request(sock: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::new();
    let head_end = loop {
        if let Some(i) = find(&buf, b"\r\n\r\n") {
            break i;
        }
        read_more(sock, &mut buf).await?;
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut rest = buf.split_off(head_end + 4);
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_string();
    let target = request_line.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let header = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    if header("expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue")) {
        sock.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
            .await
            .ok()?;
    }
    let body = if header("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        loop {
            if let Some(body) = dechunk(&rest) {
                break body;
            }
            read_more(sock, &mut rest).await?;
        }
    } else {
        let len: usize = header("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        while rest.len() < len {
            read_more(sock, &mut rest).await?;
        }
        rest.truncate(len);
        rest
    };
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (target, None),
    };
    Some(Request {
        method,
        path,
        query,
        headers,
        body,
        outcome: Outcome::Replying,
    })
}

async fn read_more(sock: &mut TcpStream, buf: &mut Vec<u8>) -> Option<()> {
    let mut chunk = [0u8; 8192];
    match sock.read(&mut chunk).await {
        Ok(0) | Err(_) => None,
        Ok(n) => {
            buf.extend_from_slice(&chunk[..n]);
            Some(())
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// The body of a complete chunked message in `buf`; `None` while more is needed.
fn dechunk(buf: &[u8]) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    let mut at = 0;
    loop {
        let line_end = at + find(&buf[at..], b"\r\n")?;
        let size_text = String::from_utf8_lossy(&buf[at..line_end]);
        // Chunk extensions (`;name=value`) are ignored.
        let size_text = size_text.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16).ok()?;
        at = line_end + 2;
        if size == 0 {
            // The end, or trailers up to an empty line.
            return (buf[at..].starts_with(b"\r\n") || find(&buf[at..], b"\r\n\r\n").is_some())
                .then_some(body);
        }
        if buf.len() < at + size + 2 {
            return None;
        }
        body.extend_from_slice(&buf[at..at + size]);
        at += size + 2;
    }
}

/// Waits `d`; `false` as soon as the client goes away.
async fn wait(sock: &mut TcpStream, d: Duration) -> bool {
    let deadline = Instant::now() + d;
    let mut scratch = [0u8; 1024];
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return true,
            read = sock.read(&mut scratch) => match read {
                Ok(0) | Err(_) => return false,
                Ok(_) => {} // the client may send more; nothing waits for it
            },
        }
    }
}

async fn send(sock: &mut TcpStream, reply: &Reply) -> Outcome {
    if !reply.delay.is_zero() && !wait(sock, reply.delay).await {
        return Outcome::Aborted;
    }
    let mut head = format!("HTTP/1.1 {} {}\r\n", reply.status, reason(reply.status));
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    match &reply.body {
        Body::Full(bytes) => head.push_str(&format!("content-length: {}\r\n", bytes.len())),
        Body::Chunked(_) => head.push_str("transfer-encoding: chunked\r\n"),
    }
    head.push_str("connection: close\r\n\r\n");
    if sock.write_all(head.as_bytes()).await.is_err() {
        return Outcome::Aborted;
    }
    match &reply.body {
        Body::Full(bytes) => {
            if sock.write_all(bytes).await.is_err() {
                return Outcome::Aborted;
            }
        }
        Body::Chunked(pieces) => {
            for piece in pieces {
                match piece {
                    // An empty chunk would end the body.
                    Piece::Data(bytes) if bytes.is_empty() => {}
                    Piece::Data(bytes) => {
                        let size = format!("{:x}\r\n", bytes.len());
                        let frame = [size.as_bytes(), bytes.as_slice(), &b"\r\n"[..]].concat();
                        if sock.write_all(&frame).await.is_err() || sock.flush().await.is_err() {
                            return Outcome::Aborted;
                        }
                    }
                    Piece::Wait(d) => {
                        if !wait(sock, *d).await {
                            return Outcome::Aborted;
                        }
                    }
                    Piece::Close => return Outcome::Closed,
                }
            }
            if sock.write_all(b"0\r\n\r\n").await.is_err() {
                return Outcome::Aborted;
            }
        }
    }
    let _ = sock.shutdown().await;
    Outcome::Sent
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::install_crypto;

    fn client() -> reqwest::Client {
        install_crypto();
        // No system proxy: the requests must reach 127.0.0.1 directly.
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("a client")
    }

    #[tokio::test]
    async fn routes_reply_in_order_and_requests_are_recorded() {
        let server = FakeServer::start(vec![
            Route::new("GET", "/a")
                .reply(Reply::json(200, &serde_json::json!({"n": 1})))
                .reply(Reply::text(200, "two")),
            Route::new("POST", "/items").reply(Reply::new(201).header("location", "/items/1")),
            Route::new("*", "/prefix/*").reply(Reply::text(200, "p")),
        ])
        .await;
        let http = client();
        let first = http
            .get(server.url("/a?x=1"))
            .header("x-test", "yes")
            .send()
            .await
            .unwrap();
        assert_eq!(first.status(), 200);
        assert_eq!(first.text().await.unwrap(), r#"{"n":1}"#);
        // The last reply repeats.
        for _ in 0..2 {
            let r = http.get(server.url("/a")).send().await.unwrap();
            assert_eq!(r.text().await.unwrap(), "two");
        }
        let created = http
            .post(server.url("/items"))
            .body(r#"{"name":"x"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(created.status(), 201);
        assert_eq!(created.headers()["location"], "/items/1");
        let r = http.delete(server.url("/prefix/abc")).send().await.unwrap();
        assert_eq!(r.text().await.unwrap(), "p");
        let missing = http.get(server.url("/nope")).send().await.unwrap();
        assert_eq!(missing.status(), 404);

        let requests = server
            .wait_until(Duration::from_secs(5), |r| {
                r.len() == 6 && r.iter().all(|r| r.outcome == Outcome::Sent)
            })
            .await;
        let paths: Vec<_> = requests
            .iter()
            .map(|r| format!("{} {}", r.method, r.path))
            .collect();
        assert_eq!(
            paths,
            [
                "GET /a",
                "GET /a",
                "GET /a",
                "POST /items",
                "DELETE /prefix/abc",
                "GET /nope"
            ]
        );
        assert_eq!(requests[0].query.as_deref(), Some("x=1"));
        assert_eq!(requests[0].header("X-Test"), Some("yes"));
        assert_eq!(requests[1].query, None);
        assert_eq!(requests[3].json(), serde_json::json!({"name": "x"}));
        assert_eq!(requests[3].header("content-length"), Some("12"));
    }

    #[tokio::test]
    async fn a_route_with_auth_answers_401_to_any_other_credential() {
        let server = FakeServer::start(vec![
            Route::new("GET", "/secure")
                .auth("Bearer good")
                .reply(Reply::text(200, "secret")),
            // A token that expires after its first use: 401 once, then fine.
            Route::new("GET", "/expiring")
                .reply(Reply::unauthorized())
                .reply(Reply::text(200, "fresh")),
        ])
        .await;
        let http = client();
        let anonymous = http.get(server.url("/secure")).send().await.unwrap();
        assert_eq!(anonymous.status(), 401);
        assert_eq!(anonymous.headers()["www-authenticate"], "Bearer");
        let wrong = http
            .get(server.url("/secure"))
            .bearer_auth("bad")
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), 401);
        let right = http
            .get(server.url("/secure"))
            .bearer_auth("good")
            .send()
            .await
            .unwrap();
        assert_eq!(right.status(), 200);
        assert_eq!(right.text().await.unwrap(), "secret");

        let first = http.get(server.url("/expiring")).send().await.unwrap();
        assert_eq!(first.status(), 401);
        let retry = http.get(server.url("/expiring")).send().await.unwrap();
        assert_eq!(retry.text().await.unwrap(), "fresh");
        let requests = server
            .wait_until(Duration::from_secs(5), |r| r.len() == 5)
            .await;
        assert_eq!(requests[2].header("authorization"), Some("Bearer good"));
    }

    /// An in-memory engine behind a handler route: replies made from each request and the
    /// engine's state, the route's credential checked first, other routes still canned.
    #[tokio::test]
    async fn a_handler_route_serves_an_engine_over_the_socket() {
        let items = Arc::new(Mutex::new(Vec::<String>::new()));
        let engine = items.clone();
        let server = FakeServer::start(vec![
            Route::new("GET", "/health").reply(Reply::text(200, "ok")),
            Route::new("*", "/v1/*")
                .auth("Bearer t")
                .handler(move |req| {
                    let mut items = engine.lock().unwrap();
                    match (req.method.as_str(), req.path.as_str()) {
                        ("POST", "/v1/items") => {
                            items.push(req.text());
                            Reply::json(201, &serde_json::json!({ "n": items.len() }))
                        }
                        ("GET", "/v1/items") => {
                            let all = items.join(",");
                            Reply::text(
                                200,
                                format!("{all}?{}", req.query.as_deref().unwrap_or("")),
                            )
                        }
                        _ => Reply::text(404, "unknown"),
                    }
                }),
        ])
        .await;
        let http = client();
        for (body, n) in [("a", 1), ("b", 2)] {
            let r = http
                .post(server.url("/v1/items"))
                .bearer_auth("t")
                .body(body)
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 201);
            let reply: serde_json::Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
            assert_eq!(reply["n"], n);
        }
        let listed = http
            .get(server.url("/v1/items?page=2"))
            .bearer_auth("t")
            .send()
            .await
            .unwrap();
        assert_eq!(listed.text().await.unwrap(), "a,b?page=2");
        let anonymous = http
            .post(server.url("/v1/items"))
            .body("c")
            .send()
            .await
            .unwrap();
        assert_eq!(anonymous.status(), 401);
        let other = http
            .delete(server.url("/v1/x"))
            .bearer_auth("t")
            .send()
            .await
            .unwrap();
        assert_eq!(other.status(), 404);
        let health = http.get(server.url("/health")).send().await.unwrap();
        assert_eq!(health.text().await.unwrap(), "ok");
        // The refused request never reached the engine.
        assert_eq!(*items.lock().unwrap(), ["a", "b"]);
        let requests = server
            .wait_until(Duration::from_secs(5), |r| {
                r.len() == 6 && r.iter().all(|r| r.outcome == Outcome::Sent)
            })
            .await;
        assert_eq!(requests[0].text(), "a");
    }

    #[tokio::test]
    async fn a_client_that_gives_up_on_a_delayed_reply_is_recorded_as_aborted() {
        let server = FakeServer::start(vec![
            Route::new("GET", "/slow")
                .reply(Reply::text(200, "late").delay(Duration::from_secs(30))),
        ])
        .await;
        let started = std::time::Instant::now();
        let gave_up = tokio::time::timeout(
            Duration::from_millis(200),
            client().get(server.url("/slow")).send(),
        )
        .await;
        assert!(gave_up.is_err(), "the reply waits 30 s");
        // Dropping the request closes the connection: the server stops waiting.
        let requests = server
            .wait_until(Duration::from_secs(10), |r| {
                r.first().is_some_and(|r| r.outcome == Outcome::Aborted)
            })
            .await;
        assert_eq!(requests.len(), 1);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn chunked_bodies_stream_stall_and_break_off() {
        let server = FakeServer::start(vec![
            Route::new("GET", "/stream").reply(Reply::ok().chunked(vec![
                Piece::Data(b"a".to_vec()),
                Piece::Wait(Duration::from_millis(50)),
                Piece::Data(Vec::new()),
                Piece::Data(b"bc".to_vec()),
            ])),
            Route::new("GET", "/broken")
                .reply(Reply::ok().chunked(vec![Piece::Data(b"part".to_vec()), Piece::Close])),
            Route::new("GET", "/stalled").reply(Reply::ok().chunked(vec![
                Piece::Data(b"first".to_vec()),
                Piece::Wait(Duration::from_secs(30)),
                Piece::Data(b"never".to_vec()),
            ])),
            Route::new("POST", "/upload").reply(Reply::ok()),
        ])
        .await;
        let http = client();
        let whole = http.get(server.url("/stream")).send().await.unwrap();
        assert_eq!(whole.text().await.unwrap(), "abc");

        let broken = http.get(server.url("/broken")).send().await.unwrap();
        assert!(
            broken.bytes().await.is_err(),
            "a body cut short is an error"
        );

        // A client that cancels mid-body.
        let mut stalled = http.get(server.url("/stalled")).send().await.unwrap();
        assert_eq!(
            stalled.chunk().await.unwrap().as_deref(),
            Some(&b"first"[..])
        );
        let next = tokio::time::timeout(Duration::from_millis(200), stalled.chunk()).await;
        assert!(next.is_err(), "the rest waits 30 s");
        drop(stalled);

        // A request body sent in chunks arrives whole.
        let pieces: Vec<Result<&'static [u8], std::io::Error>> = vec![Ok(b"he"), Ok(b"llo")];
        let upload = http
            .post(server.url("/upload"))
            .body(reqwest::Body::wrap_stream(futures::stream::iter(pieces)))
            .send()
            .await
            .unwrap();
        assert_eq!(upload.status(), 200);

        let requests = server
            .wait_until(Duration::from_secs(10), |r| {
                r.len() == 4 && r.iter().all(|r| r.outcome != Outcome::Replying)
            })
            .await;
        let outcomes: Vec<_> = requests.iter().map(|r| r.outcome).collect();
        assert_eq!(
            outcomes,
            [
                Outcome::Sent,
                Outcome::Closed,
                Outcome::Aborted,
                Outcome::Sent
            ]
        );
        assert_eq!(requests[3].header("transfer-encoding"), Some("chunked"));
        assert_eq!(requests[3].text(), "hello");
    }

    #[test]
    fn chunked_request_bodies_are_decoded_once_complete() {
        assert_eq!(
            dechunk(b"5\r\nhello\r\n0\r\n\r\n").as_deref(),
            Some(&b"hello"[..])
        );
        assert_eq!(
            dechunk(b"2;x=y\r\nhe\r\n3\r\nllo\r\n0\r\ntrailer: 1\r\n\r\n").as_deref(),
            Some(&b"hello"[..])
        );
        assert_eq!(dechunk(b"5\r\nhel"), None);
        assert_eq!(dechunk(b"5\r\nhello\r\n0\r\n"), None);
        assert_eq!(dechunk(b""), None);
    }
}
