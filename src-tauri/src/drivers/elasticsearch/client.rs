//! The REST client the session runs on: every request is an `EsRequest` from
//! `sqlgen::native::elastic` (or the console), sent through a [`Transport`] with its
//! credentials, and every reply read with the total parsers of this module's siblings.
//!
//! Browsing pages through `from`/`size` while the page ends within `index.max_result_window`;
//! deeper pages walk a point in time with `search_after` (Elasticsearch 7.12+, which sorts on
//! `_shard_doc` last) or a scroll (OpenSearch, older Elasticsearch), and hand the renderer a
//! cursor so the next page continues where this one stopped. A jump to a far page reads every
//! page before it, without `_source` on a point in time.

use std::sync::Arc;

use chrono::Utc;

use crate::error::AppError;
use crate::sqlgen::native::elastic::{
    self as build, BrowseSpec, EditPlan, MAX_PAGE, PageCursor, PagePlan, WalkMode,
};

use super::auth::Auth;
use super::catalog::{self, IndexMapping};
use super::console::{EsRequest, Method};
use super::errors;
use super::info::{ClusterInfo, Features};
use super::json::{self, Json};
use super::reply::{self, Grid};
use super::transport::{BaseUrl, HttpRequest, HttpResponse, Transport};

pub struct EsClient {
    transport: Arc<dyn Transport>,
    base: BaseUrl,
    auth: Auth,
    /// `x-opaque-id` of the requests (tasks started by this session can be found by it).
    opaque_id: String,
}

/// A page of the grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub hits: Vec<Json>,
    pub next_cursor: Option<String>,
}

impl EsClient {
    pub fn new(
        transport: Arc<dyn Transport>,
        base: BaseUrl,
        auth: Auth,
        opaque_id: &str,
    ) -> EsClient {
        EsClient {
            transport,
            base,
            auth,
            opaque_id: opaque_id.to_string(),
        }
    }

    pub fn base(&self) -> &BaseUrl {
        &self.base
    }

    /// The HTTP request of `req`, with its body type and credentials.
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
        self.auth.authorize(&mut http, Utc::now());
        http
    }

    /// Sends a request; any HTTP status is a reply.
    pub async fn send(&self, req: &EsRequest) -> Result<HttpResponse, AppError> {
        self.transport.send(self.http_request(req)).await
    }

    /// Sends a request and reads its JSON reply; an error reply is an error.
    pub async fn json(&self, req: &EsRequest) -> Result<Json, AppError> {
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
                super::transport::body_excerpt(&reply.body)
            ))
        })
    }

    /// `GET /`: the cluster and what its version allows. Free (no search, no billing), so it is
    /// also the ping.
    pub async fn info(&self) -> Result<ClusterInfo, AppError> {
        let info = ClusterInfo::parse(&self.json(&build::info()).await?)?;
        info.check_supported()?;
        Ok(info)
    }

    /// The sidebar's tables: indices, aliases and (where the cluster has them) data streams.
    pub async fn tables(
        &self,
        schema: &str,
        show_system: bool,
        features: Features,
    ) -> Result<Vec<crate::types::schema::TableSummary>, AppError> {
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

    /// The smallest `index.max_result_window` of a target's indices (10,000 by default).
    pub async fn result_window(&self, target: &str) -> Result<u64, AppError> {
        let settings = catalog::parse_settings(&self.json(&build::settings(target)).await?)?;
        Ok(settings
            .iter()
            .map(|(_, s)| {
                s.max_result_window
                    .unwrap_or(catalog::DEFAULT_RESULT_WINDOW)
            })
            .min()
            .unwrap_or(catalog::DEFAULT_RESULT_WINDOW))
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

    /// The page at `offset` (see the module doc). `preference` keeps `from`/`size` pages on the
    /// same shard copies.
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
                let reply = self
                    .json(&build::search_page(spec, from, size, preference))
                    .await?;
                Ok(Page {
                    hits: build::hits_of(&reply).to_vec(),
                    next_cursor: None,
                })
            }
            PagePlan::Continue(c) => match c.mode {
                WalkMode::Pit => {
                    self.pit_from(spec, c.id, c.after, offset, limit, &fingerprint, features)
                        .await
                }
                WalkMode::Scroll => {
                    let reply = self.json(&build::scroll_next(&c.id)).await?;
                    self.scroll_page(reply, c.position, offset, limit, &fingerprint)
                        .await
                }
            },
            PagePlan::Walk { skip } => match features.pit.filter(|_| features.shard_doc) {
                Some(api) => {
                    let reply = self.json(&build::open_pit(api, &spec.target)).await?;
                    let mut id = build::pit_id(&reply)
                        .ok_or_else(|| AppError::db("unexpected reply opening a point in time"))?;
                    let mut after: Option<Json> = None;
                    let mut skipped = 0;
                    while skipped < skip {
                        let n = (skip - skipped).min(MAX_PAGE);
                        let reply = self
                            .json(&build::pit_page(spec, &id, after.as_ref(), n, false))
                            .await?;
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
                            });
                        }
                    }
                    self.pit_from(spec, id, after, offset, limit, &fingerprint, features)
                        .await
                }
                None => {
                    let reply = self.json(&build::scroll_start(spec, limit)).await?;
                    self.scroll_page(reply, 0, offset, limit, &fingerprint)
                        .await
                }
            },
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
        let reply = self
            .json(&build::pit_page(spec, &id, after.as_ref(), limit, true))
            .await?;
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
        Ok(Page { hits, next_cursor })
    }

    /// Continues a scroll whose next batch starts at `pos` until the rows from `offset` are
    /// read (a batch holds `limit` rows, so pages at multiples of `limit` line up).
    async fn scroll_page(
        &self,
        mut reply: Json,
        mut pos: u64,
        offset: u64,
        limit: u64,
        fingerprint: &str,
    ) -> Result<Page, AppError> {
        let mut out = Vec::new();
        loop {
            let id = reply
                .get("_scroll_id")
                .and_then(Json::as_str)
                .map(str::to_string);
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
                    Some(id) if !done => Some(
                        PageCursor {
                            mode: WalkMode::Scroll,
                            id,
                            after: None,
                            position: pos,
                            fingerprint: fingerprint.to_string(),
                        }
                        .encode(),
                    ),
                    Some(id) => {
                        self.close_quietly(&build::clear_scroll(&id)).await;
                        None
                    }
                    None => None,
                };
                return Ok(Page {
                    hits: out,
                    next_cursor,
                });
            }
            let id = id.ok_or_else(|| AppError::db("the scroll reply has no _scroll_id"))?;
            reply = self.json(&build::scroll_next(&id)).await?;
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
