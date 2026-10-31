//! The REST client the session runs on: every request is an `EsRequest` from
//! `sqlgen::native::elastic` (or the console), sent through a [`Transport`] (the shared HTTP
//! client adds the credential), and every reply read with the total parsers of this module's
//! siblings. The app's own requests go into the session's trace; the console's do not.
//!
//! Browsing pages through `from`/`size` while the page ends within `index.max_result_window`;
//! deeper pages walk a point in time with `search_after` (Elasticsearch 7.12+, which sorts on
//! `_shard_doc` last) or a scroll (OpenSearch, older Elasticsearch). Every full page hands the
//! renderer a cursor where the next one starts. A walk that expired, or a scroll that moved on
//! (an earlier page read again), walks again from the cursor's position.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::drivers::{SqlTrace, TRACE_SQL_MAX_CHARS, TraceCounts};
use crate::error::AppError;
use crate::sqlgen::native::elastic::{
    self as build, BrowseSpec, EditPlan, MAX_PAGE, PageCursor, PagePlan, WalkMode,
};
use crate::types::browse::TraceCategory;
use crate::types::schema::TableSummary;

use super::catalog::{self, IndexMapping, IndexSettings};
use super::console::{EsRequest, Method};
use super::errors;
use super::info::{ClusterInfo, Features};
use super::json::{self, Json};
use super::processes::{self, Task};
use super::reply::{self, Grid};
use super::transport::{BaseUrl, HttpRequest, HttpResponse, Transport};

#[derive(Clone)]
pub struct EsClient {
    transport: Arc<dyn Transport>,
    base: BaseUrl,
    /// `x-opaque-id` of the requests (tasks started by them can be found by it).
    opaque_id: String,
    trace: Option<SqlTrace>,
    /// Open scrolls of the grid and the position each one reads next.
    scrolls: Arc<Mutex<HashMap<String, u64>>>,
}

/// A page of the grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub hits: Vec<Json>,
    pub next_cursor: Option<String>,
    /// The search that read the hits (the last of a walk), as console text.
    pub request: String,
}

/// Whether an error says a point in time or scroll is gone (expired, or closed at a last page).
pub fn is_context_missing(e: &AppError) -> bool {
    match e {
        AppError::Db {
            message,
            detail,
            vendor_code: Some(404),
            ..
        } => {
            detail
                .as_deref()
                .is_some_and(|d| d.contains("search_context_missing"))
                || message.to_ascii_lowercase().contains("no search context")
        }
        _ => false,
    }
}

impl EsClient {
    pub fn new(transport: Arc<dyn Transport>, base: BaseUrl, opaque_id: &str) -> EsClient {
        EsClient {
            transport,
            base,
            opaque_id: opaque_id.to_string(),
            trace: None,
            scrolls: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Records the app's requests into `trace`.
    pub fn with_trace(mut self, trace: SqlTrace) -> EsClient {
        self.trace = Some(trace);
        self
    }

    /// The client of one console run: the same connection, its own opaque id, untraced.
    pub fn for_run(&self, run: u64) -> EsClient {
        EsClient {
            opaque_id: format!("{}/{run}", self.opaque_id),
            trace: None,
            ..self.clone()
        }
    }

    pub fn opaque_id(&self) -> &str {
        &self.opaque_id
    }

    /// The HTTP request of `req`, with its body type.
    pub fn http_request(&self, req: &EsRequest) -> HttpRequest {
        let mut http = HttpRequest {
            method: req.method,
            url: self.base.url(req),
            headers: vec![("x-opaque-id".to_string(), self.opaque_id.clone())],
            body: None,
        };
        if let Some(body) = &req.body {
            http.set_header("content-type", body.content_type());
            http.body = Some(body.to_bytes());
        }
        http
    }

    /// Sends a request; any HTTP status is a reply.
    pub async fn send(&self, req: &EsRequest) -> Result<HttpResponse, AppError> {
        self.transport.send(self.http_request(req)).await
    }

    /// Sends a request and reads its JSON reply; an error reply is an error. Not traced.
    pub async fn fetch_json(&self, req: &EsRequest) -> Result<Json, AppError> {
        let reply = self.send(req).await?;
        if let Some(e) = errors::reply_error(build::ENGINE, reply.status, &reply.body) {
            return Err(e);
        }
        json::parse_bytes(&reply.body).map_err(|e| {
            AppError::db(format!(
                "unexpected reply to {} {}: {} ({})",
                req.method.as_str(),
                req.path(),
                e.message,
                crate::drivers::common::http::body_excerpt(
                    &reply.body,
                    reply.content_type.as_deref()
                )
            ))
        })
    }

    /// [`EsClient::fetch_json`], recorded in the trace.
    pub async fn json(&self, req: &EsRequest) -> Result<Json, AppError> {
        let started = Instant::now();
        let result = self.fetch_json(req).await;
        if let Some(trace) = &self.trace {
            trace.finish(category(req), trace_text(req), started, &result, counts);
        }
        result
    }

    /// `GET /`: the cluster and what its version allows. Free (no search, no billing), so it is
    /// also the ping.
    pub async fn info(&self) -> Result<ClusterInfo, AppError> {
        let info = ClusterInfo::parse(&self.fetch_json(&build::info()).await?)?;
        info.check_supported()?;
        Ok(info)
    }

    /// The sidebar's tables: indices, aliases and (where the cluster has them) data streams.
    pub async fn tables(
        &self,
        schema: Option<&str>,
        show_system: bool,
        features: Features,
    ) -> Result<Vec<TableSummary>, AppError> {
        let indices =
            catalog::parse_cat_indices(&self.json(&build::cat_indices(show_system)).await?)?;
        let aliases = catalog::parse_cat_aliases(&self.json(&build::cat_aliases()).await?)?;
        let streams = if features.data_streams {
            match self.json(&build::data_streams(show_system)).await {
                Ok(r) => catalog::parse_data_streams(&r)?,
                // A user without the `view_index_metadata` privilege on streams, or a cluster
                // without the API (OSS 7.x): no streams rather than no sidebar.
                Err(AppError::Db { .. } | AppError::NotFound(_)) => Vec::new(),
                Err(e) => return Err(e),
            }
        } else {
            Vec::new()
        };
        Ok(catalog::tables(
            schema,
            &indices,
            &aliases,
            &streams,
            show_system,
        ))
    }

    pub async fn mappings(&self, target: &str) -> Result<Vec<(String, IndexMapping)>, AppError> {
        catalog::parse_mappings(&self.json(&build::mapping(target)).await?)
    }

    pub async fn settings(&self, target: &str) -> Result<Vec<(String, IndexSettings)>, AppError> {
        catalog::parse_settings(&self.json(&build::settings(target)).await?)
    }

    /// The smallest `index.max_result_window` of a target's indices (10,000 by default).
    pub async fn result_window(&self, target: &str) -> Result<u64, AppError> {
        Ok(self
            .settings(target)
            .await?
            .iter()
            .map(|(_, s)| {
                s.max_result_window
                    .unwrap_or(catalog::DEFAULT_RESULT_WINDOW)
            })
            .min()
            .unwrap_or(catalog::DEFAULT_RESULT_WINDOW))
    }

    /// The documents of a target's indices as `_cat/indices` counts them (an estimate: deleted
    /// documents not yet merged away, nested documents).
    pub async fn doc_count(&self, target: &str) -> Result<Option<i64>, AppError> {
        let rows = catalog::parse_cat_indices(&self.json(&build::cat_indices_of(target)).await?)?;
        Ok(rows.iter().map(|r| r.docs).sum())
    }

    pub async fn count(&self, target: &str, query: &Json) -> Result<i64, AppError> {
        let reply = self.json(&build::count(target, query)).await?;
        build::count_of(&reply).ok_or_else(|| AppError::db("unexpected _count reply: no count"))
    }

    /// One document by id (`fetch_cell`), `None` when it is gone.
    pub async fn document(
        &self,
        index: &str,
        type_name: Option<&str>,
        id: &str,
        routing: Option<&str>,
        field: Option<&str>,
    ) -> Result<Option<Json>, AppError> {
        let reply = self
            .json(&build::get_doc(index, type_name, id, routing, field))
            .await?;
        Ok(build::doc_of(&reply))
    }

    /// A page of `limit` rows: where `cursor` says, else at `offset` (see the module doc).
    /// `preference` keeps `from`/`size` pages on the same shard copies.
    #[allow(clippy::too_many_arguments)]
    pub async fn browse(
        &self,
        spec: &BrowseSpec,
        features: Features,
        offset: u64,
        limit: u64,
        window: u64,
        cursor: Option<&str>,
        preference: &str,
    ) -> Result<Page, AppError> {
        let limit = limit.clamp(1, MAX_PAGE);
        let fingerprint = spec.fingerprint();
        match build::plan_page(offset, limit, window, cursor, &fingerprint)? {
            PagePlan::FromSize { from, size } => {
                let req = build::search_page(spec, from, size, preference);
                let reply = self.json(&req).await?;
                let hits = build::hits_of(&reply).to_vec();
                let next_cursor = (hits.len() as u64 >= size)
                    .then(|| PageCursor::from_position(from + size, &fingerprint).encode());
                Ok(Page {
                    hits,
                    next_cursor,
                    request: req.to_console_text(),
                })
            }
            PagePlan::Continue(c) => {
                let position = c.position;
                let continued = match c.mode {
                    WalkMode::Pit => {
                        self.pit_from(spec, c.id, c.after, position, limit, &fingerprint, features)
                            .await
                    }
                    WalkMode::Scroll if self.scroll_at(&c.id) == Some(position) => {
                        let req = build::scroll_next(&c.id);
                        match self.json(&req).await {
                            Ok(reply) => {
                                let sent = req.to_console_text();
                                self.scroll_page(
                                    reply,
                                    sent,
                                    position,
                                    position,
                                    limit,
                                    &fingerprint,
                                )
                                .await
                            }
                            Err(e) => {
                                self.forget_scroll(&c.id);
                                Err(e)
                            }
                        }
                    }
                    // A scroll reads forward only: one that moved on (an earlier page read
                    // again) or that this session does not know is walked again.
                    WalkMode::Scroll | WalkMode::From => {
                        if self.forget_scroll(&c.id) {
                            self.close_quietly(&build::clear_scroll(&c.id)).await;
                        }
                        return self
                            .walk(spec, features, position, limit, &fingerprint)
                            .await;
                    }
                };
                match continued {
                    Err(e) if is_context_missing(&e) => {
                        self.walk(spec, features, position, limit, &fingerprint)
                            .await
                    }
                    other => other,
                }
            }
            PagePlan::Walk { skip } => self.walk(spec, features, skip, limit, &fingerprint).await,
        }
    }

    /// Opens a point in time (or a scroll) and reads the page `skip` rows in.
    async fn walk(
        &self,
        spec: &BrowseSpec,
        features: Features,
        skip: u64,
        limit: u64,
        fingerprint: &str,
    ) -> Result<Page, AppError> {
        match features.pit.filter(|_| features.shard_doc) {
            Some(api) => {
                let reply = self.json(&build::open_pit(api, &spec.target)).await?;
                let mut id = build::pit_id(&reply)
                    .ok_or_else(|| AppError::db("unexpected reply opening a point in time"))?;
                let mut after: Option<Json> = None;
                let mut skipped = 0;
                while skipped < skip {
                    let n = (skip - skipped).min(MAX_PAGE);
                    let req = build::pit_page(spec, &id, after.as_ref(), n, false);
                    let reply = self.json(&req).await?;
                    if let Some(next) = build::pit_id(&reply) {
                        id = next;
                    }
                    let hits = build::hits_of(&reply);
                    skipped += hits.len() as u64;
                    after = hits.last().and_then(|h| h.get("sort")).cloned();
                    if (hits.len() as u64) < n {
                        self.close_quietly(&build::close_pit(api, &id)).await;
                        return Ok(Page {
                            hits: Vec::new(),
                            next_cursor: None,
                            request: req.to_console_text(),
                        });
                    }
                }
                self.pit_from(spec, id, after, skip, limit, fingerprint, features)
                    .await
            }
            None => {
                let req = build::scroll_start(spec, limit);
                let reply = self.json(&req).await?;
                self.scroll_page(reply, req.to_console_text(), 0, skip, limit, fingerprint)
                    .await
            }
        }
    }

    /// Reads `limit` rows of a point in time after `after` and hands back where it stopped.
    #[allow(clippy::too_many_arguments)]
    async fn pit_from(
        &self,
        spec: &BrowseSpec,
        id: String,
        after: Option<Json>,
        offset: u64,
        limit: u64,
        fingerprint: &str,
        features: Features,
    ) -> Result<Page, AppError> {
        let req = build::pit_page(spec, &id, after.as_ref(), limit, true);
        let reply = self.json(&req).await?;
        let id = build::pit_id(&reply).unwrap_or(id);
        let hits = build::hits_of(&reply).to_vec();
        let next_cursor = if (hits.len() as u64) < limit {
            if let Some(api) = features.pit {
                self.close_quietly(&build::close_pit(api, &id)).await;
            }
            None
        } else {
            Some(
                PageCursor {
                    mode: WalkMode::Pit,
                    id,
                    after: hits.last().and_then(|h| h.get("sort")).cloned(),
                    position: offset + hits.len() as u64,
                    fingerprint: fingerprint.to_string(),
                }
                .encode(),
            )
        };
        Ok(Page {
            hits,
            next_cursor,
            request: req.to_console_text(),
        })
    }

    /// Continues a scroll whose next batch (`reply`, to the request `sent`) starts at `pos`
    /// until the rows from `offset` are read (a batch holds `limit` rows, so pages at multiples
    /// of `limit` line up).
    async fn scroll_page(
        &self,
        mut reply: Json,
        mut sent: String,
        mut pos: u64,
        offset: u64,
        limit: u64,
        fingerprint: &str,
    ) -> Result<Page, AppError> {
        let mut out = Vec::new();
        let mut previous: Option<String> = None;
        loop {
            let id = reply
                .get("_scroll_id")
                .and_then(Json::as_str)
                .map(str::to_string);
            // The server may hand out a new id with each batch.
            if let Some(old) = previous.take().filter(|old| Some(old) != id.as_ref()) {
                self.forget_scroll(&old);
            }
            let hits = build::hits_of(&reply);
            let n = hits.len() as u64;
            for (i, h) in hits.iter().enumerate() {
                let at = pos + i as u64;
                if at >= offset && (out.len() as u64) < limit {
                    out.push(h.clone());
                }
            }
            pos += n;
            let done = n == 0 || n < limit;
            if done || out.len() as u64 >= limit {
                let next_cursor = match id {
                    Some(id) if !done => {
                        self.remember_scroll(&id, pos);
                        Some(
                            PageCursor {
                                mode: WalkMode::Scroll,
                                id,
                                after: None,
                                position: pos,
                                fingerprint: fingerprint.to_string(),
                            }
                            .encode(),
                        )
                    }
                    Some(id) => {
                        self.forget_scroll(&id);
                        self.close_quietly(&build::clear_scroll(&id)).await;
                        None
                    }
                    None => None,
                };
                return Ok(Page {
                    hits: out,
                    next_cursor,
                    request: sent,
                });
            }
            let id = id.ok_or_else(|| AppError::db("the scroll reply has no _scroll_id"))?;
            let req = build::scroll_next(&id);
            reply = self.json(&req).await?;
            sent = req.to_console_text();
            previous = Some(id);
        }
    }

    fn scrolls(&self) -> std::sync::MutexGuard<'_, HashMap<String, u64>> {
        self.scrolls.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn scroll_at(&self, id: &str) -> Option<u64> {
        self.scrolls().get(id).copied()
    }

    fn remember_scroll(&self, id: &str, pos: u64) {
        self.scrolls().insert(id.to_string(), pos);
    }

    /// Forgets a scroll; whether it was known.
    fn forget_scroll(&self, id: &str) -> bool {
        self.scrolls().remove(id).is_some()
    }

    /// Frees the scrolls still open (the session closes).
    pub async fn close_walks(&self) {
        let ids: Vec<String> = self.scrolls().drain().map(|(id, _)| id).collect();
        if !ids.is_empty() {
            self.close_quietly(&build::clear_scrolls(&ids)).await;
        }
    }

    /// Frees a point in time or scroll; a failure only leaves it to expire.
    async fn close_quietly(&self, req: &EsRequest) {
        let _ = self.send(req).await;
    }

    /// Applies a plan as one `_bulk` request and reads it back per change. A request that fails
    /// as a whole fails every change (none was applied).
    pub async fn apply(
        &self,
        plan: &EditPlan,
    ) -> Result<Vec<Result<build::Applied, AppError>>, AppError> {
        if plan.ops.is_empty() {
            return Ok(Vec::new());
        }
        let reply = self.json(&plan.request()).await?;
        Ok(build::bulk_outcomes(&reply, &plan.ops))
    }

    /// The running tasks.
    pub async fn tasks(&self) -> Result<Vec<Task>, AppError> {
        Ok(processes::parse_tasks(&self.json(&build::tasks()).await?))
    }

    /// Cancels the tasks a request with the opaque id `opaque` started (a console run's
    /// `_update_by_query`, `_reindex`…, which closing their connection does not stop).
    pub async fn cancel_tasks_of(&self, opaque: &str) -> Result<(), AppError> {
        let tasks = processes::parse_tasks(&self.fetch_json(&build::tasks()).await?);
        for task in processes::of_run(&tasks, opaque) {
            if let Ok(req) = build::cancel_task(&task.id) {
                self.close_quietly(&req).await;
            }
        }
        Ok(())
    }

    /// Runs a console request and reads its reply as grids. An error reply is an error; a
    /// `HEAD` answers with its status.
    pub async fn console(&self, req: &EsRequest, max_rows: usize) -> Result<Vec<Grid>, AppError> {
        let reply = self.send(req).await?;
        if req.method == Method::Head {
            return Ok(vec![reply::text_grid(&format!(
                "{} {}",
                reply.status,
                if (200..300).contains(&reply.status) {
                    "OK (exists)"
                } else {
                    "(does not exist)"
                }
            ))]);
        }
        if let Some(e) = errors::reply_error(build::ENGINE, reply.status, &reply.body) {
            return Err(e);
        }
        match json::parse_bytes(&reply.body) {
            Ok(j) => Ok(reply::grids(req, &j, max_rows)),
            Err(_) => Ok(vec![reply::text_grid(&String::from_utf8_lossy(
                &reply.body,
            ))]),
        }
    }
}

/// What the console log files a request under.
fn category(req: &EsRequest) -> TraceCategory {
    const DATA: &[&str] = &[
        "_search", "_count", "_mget", "_bulk", "_pit", "_sql", "_doc", "_update",
    ];
    if req.segments.iter().any(|s| DATA.contains(&s.as_str())) {
        TraceCategory::Data
    } else {
        TraceCategory::Structure
    }
}

/// A request as the console log shows it, capped.
fn trace_text(req: &EsRequest) -> String {
    let text = req.to_console_text();
    match text.char_indices().nth(TRACE_SQL_MAX_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text,
    }
}

/// The rows a reply returned (hits) or changed (bulk items).
fn counts(reply: &Json) -> TraceCounts {
    if let Some(items) = reply.get("items").and_then(Json::as_array) {
        return TraceCounts {
            rows: None,
            affected: Some(items.len() as u64),
        };
    }
    match reply.at(&["hits", "hits"]).and_then(Json::as_array) {
        Some(hits) => TraceCounts::rows(hits.len()),
        None => TraceCounts::NONE,
    }
}
