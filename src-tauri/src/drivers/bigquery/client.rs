//! The REST v2 client: every call carries a bearer token from the session's `TokenSource`
//! (fetched again once after a 401), lists follow their page tokens, and query jobs are polled
//! with `jobs.getQueryResults` until they finish, with no client-side limit on how long a job
//! may run (BigQuery answers each poll after about 10 s at most). A run stops when its
//! `CancellationToken` fires; `JobCancel` fires it and asks BigQuery to cancel the job.

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;

use super::auth::TokenSource;
use super::catalog::{
    DatasetItem, DatasetList, ProjectItem, ProjectList, Routine, RoutineList, Table, TableData,
    TableItem, TableList,
};
use super::errors::{http_error, job_error};
use super::jobs::{CancelReply, Job, JobList, JobReference, QueryOptions, QueryResponse};
use super::transport::{HttpRequest, HttpResponse, Transport, credentials_may_go_to, encode};
use crate::drivers::CancelAction;
use crate::error::AppError;

/// Pages one listing follows at most (1,000 items each): a server that keeps sending page
/// tokens cannot keep the sidebar busy forever.
const MAX_PAGES: usize = 100;
const LIST_PAGE_SIZE: u32 = 1000;
/// How long one `jobs.getQueryResults` waits on the server for the job to finish.
pub const POLL_WAIT_MS: u32 = 10_000;
/// The pause between polls that came back early without the job finishing.
const POLL_PAUSE: Duration = Duration::from_millis(200);
const POLL_PAUSE_MAX: Duration = Duration::from_secs(2);

/// A page of a finished query's results.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResultsPage {
    pub page_token: Option<String>,
    pub start_index: Option<u64>,
    pub max_results: Option<u32>,
}

/// A page of `tabledata.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DataPage {
    pub page_token: Option<String>,
    pub start_index: Option<u64>,
    pub max_results: Option<u32>,
    /// Top-level columns to read (all when empty).
    pub selected_fields: Vec<String>,
}

/// A query job that finished: its reference and the first page of what it returned.
#[derive(Debug, Clone)]
pub struct QueryRun {
    pub job: JobReference,
    pub first: QueryResponse,
}

/// What a dry run says: the bytes the query would process (and bill) and its result's schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DryRun {
    pub bytes_processed: Option<u64>,
    pub schema: Option<super::decode::TableSchema>,
}

pub struct Client {
    transport: Arc<dyn Transport>,
    tokens: Arc<dyn TokenSource>,
    /// The REST base, without a trailing slash.
    base: String,
    /// The project jobs run (and are billed) in.
    project: String,
    /// User credentials (`authorized_user`) bill API calls to this project.
    quota_project: Option<String>,
}

impl Client {
    pub fn new(
        transport: Arc<dyn Transport>,
        tokens: Arc<dyn TokenSource>,
        base: &str,
        project: &str,
    ) -> Client {
        Client {
            transport,
            tokens,
            base: base.trim_end_matches('/').to_string(),
            project: project.to_string(),
            quota_project: None,
        }
    }

    pub fn with_quota_project(mut self, project: Option<&str>) -> Client {
        self.quota_project = project.filter(|p| !p.is_empty()).map(str::to_string);
        self
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    /// `base/seg/seg?k=v`, each part percent-encoded.
    fn url(&self, segments: &[&str], query: &[(&str, String)]) -> String {
        let mut url = self.base.clone();
        for s in segments {
            url.push('/');
            url.push_str(&encode(s));
        }
        let mut sep = '?';
        for (k, v) in query {
            url.push(sep);
            url.push_str(&encode(k));
            url.push('=');
            url.push_str(&encode(v));
            sep = '&';
        }
        url
    }

    /// Sends `req` with a bearer token; on a 401 forgets the token and tries once more with a
    /// new one. A status that is not a success is the API's error (`sql`: for its position).
    async fn send(&self, req: HttpRequest, sql: Option<&str>) -> Result<HttpResponse, AppError> {
        // `BigqueryParams::validate` refuses such an endpoint already; a profile that skipped it
        // still never sends the bearer in clear text to another machine.
        if !credentials_may_go_to(&req.url) {
            return Err(AppError::InvalidInput(
                "BigQuery: the API endpoint must be an https URL (plain http only to this \
                 machine)."
                    .into(),
            ));
        }
        let mut retried = false;
        loop {
            let token = self.tokens.token().await?;
            let mut r = req
                .clone()
                .with_header("authorization", &format!("Bearer {token}"));
            if let Some(q) = &self.quota_project {
                r = r.with_header("x-goog-user-project", q);
            }
            let reply = self.transport.send(r).await?;
            if reply.status == 401 && !retried && self.tokens.invalidate() {
                retried = true;
                continue;
            }
            if !reply.is_success() {
                return Err(http_error(
                    reply.status,
                    &reply.body,
                    reply.content_type.as_deref(),
                    sql,
                ));
            }
            return Ok(reply);
        }
    }

    async fn get<T: DeserializeOwned>(&self, url: String, what: &str) -> Result<T, AppError> {
        self.send(HttpRequest::get(url), None).await?.parse(what)
    }

    async fn post<T: DeserializeOwned>(
        &self,
        url: String,
        body: &serde_json::Value,
        what: &str,
        sql: Option<&str>,
    ) -> Result<T, AppError> {
        self.send(HttpRequest::post_json(url, body), sql)
            .await?
            .parse(what)
    }

    /// Follows a listing's page tokens. `page` reads one page into its items and next token.
    async fn list_all<P, T>(
        &self,
        segments: &[&str],
        extra: &[(&str, String)],
        what: &str,
        page: impl Fn(P) -> (Vec<T>, Option<String>),
    ) -> Result<Vec<T>, AppError>
    where
        P: DeserializeOwned,
    {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut query: Vec<(&str, String)> = vec![("maxResults", LIST_PAGE_SIZE.to_string())];
            query.extend(extra.iter().cloned());
            if let Some(t) = &token {
                query.push(("pageToken", t.clone()));
            }
            let (items, next) = page(self.get(self.url(segments, &query), what).await?);
            out.extend(items);
            match next.filter(|n| !n.is_empty()) {
                Some(n) if Some(&n) != token.as_ref() => token = Some(n),
                _ => break,
            }
        }
        Ok(out)
    }

    /// The projects the account can see (`projects.list`).
    pub async fn list_projects(&self) -> Result<Vec<ProjectItem>, AppError> {
        self.list_all(&["projects"], &[], "projects.list", |p: ProjectList| {
            (p.projects, p.next_page_token)
        })
        .await
    }

    pub async fn list_datasets(&self, project: &str) -> Result<Vec<DatasetItem>, AppError> {
        self.list_all(
            &["projects", project, "datasets"],
            &[],
            "datasets.list",
            |p: DatasetList| (p.datasets, p.next_page_token),
        )
        .await
    }

    pub async fn list_tables(
        &self,
        project: &str,
        dataset: &str,
    ) -> Result<Vec<TableItem>, AppError> {
        self.list_all(
            &["projects", project, "datasets", dataset, "tables"],
            &[],
            "tables.list",
            |p: TableList| (p.tables, p.next_page_token),
        )
        .await
    }

    pub async fn get_table(
        &self,
        project: &str,
        dataset: &str,
        table: &str,
    ) -> Result<Table, AppError> {
        self.get(
            self.url(
                &["projects", project, "datasets", dataset, "tables", table],
                &[],
            ),
            "tables.get",
        )
        .await
    }

    pub async fn list_routines(
        &self,
        project: &str,
        dataset: &str,
    ) -> Result<Vec<Routine>, AppError> {
        self.list_all(
            &["projects", project, "datasets", dataset, "routines"],
            &[],
            "routines.list",
            |p: RoutineList| (p.routines, p.next_page_token),
        )
        .await
    }

    pub async fn get_routine(
        &self,
        project: &str,
        dataset: &str,
        routine: &str,
    ) -> Result<Routine, AppError> {
        self.get(
            self.url(
                &[
                    "projects", project, "datasets", dataset, "routines", routine,
                ],
                &[],
            ),
            "routines.get",
        )
        .await
    }

    /// One page of a table's rows (free: no job, no bytes billed). Native tables only; a view
    /// or an external table needs a query.
    pub async fn list_table_data(
        &self,
        project: &str,
        dataset: &str,
        table: &str,
        page: &DataPage,
    ) -> Result<TableData, AppError> {
        let mut q: Vec<(&str, String)> = vec![("formatOptions.useInt64Timestamp", "true".into())];
        if let Some(m) = page.max_results {
            q.push(("maxResults", m.to_string()));
        }
        if let Some(t) = page.page_token.as_ref().filter(|t| !t.is_empty()) {
            q.push(("pageToken", t.clone()));
        } else if let Some(i) = page.start_index {
            q.push(("startIndex", i.to_string()));
        }
        if !page.selected_fields.is_empty() {
            q.push(("selectedFields", page.selected_fields.join(",")));
        }
        self.get(
            self.url(
                &[
                    "projects", project, "datasets", dataset, "tables", table, "data",
                ],
                &q,
            ),
            "tabledata.list",
        )
        .await
    }

    /// `jobs.query`: runs `body` (`jobs::query_body`) and waits up to its `timeoutMs`. A reply
    /// that reports errors is the first one.
    pub async fn query(
        &self,
        body: &serde_json::Value,
        sql: &str,
    ) -> Result<QueryResponse, AppError> {
        let r: QueryResponse = self
            .post(
                self.url(&["projects", &self.project, "queries"], &[]),
                body,
                "jobs.query",
                Some(sql),
            )
            .await?;
        if let Some(first) = r.errors.first()
            && r.complete()
        {
            return Err(job_error(first, &r.errors, Some(sql)));
        }
        Ok(r)
    }

    /// `jobs.insert` of a query job (`jobs::insert_body`). A job that failed at once (a syntax
    /// error, the cost guard of a dry run) is its error.
    pub async fn insert_job(&self, body: &serde_json::Value, sql: &str) -> Result<Job, AppError> {
        let job: Job = self
            .post(
                self.url(&["projects", &self.project, "jobs"], &[]),
                body,
                "jobs.insert",
                Some(sql),
            )
            .await?;
        if let Some((first, all)) = job.error() {
            return Err(job_error(first, all, Some(sql)));
        }
        Ok(job)
    }

    fn job_query(
        job: &JobReference,
        extra: Vec<(&'static str, String)>,
    ) -> Vec<(&'static str, String)> {
        let mut q = extra;
        if let Some(l) = job.location.as_ref().filter(|l| !l.is_empty()) {
            q.push(("location", l.clone()));
        }
        q
    }

    /// `jobs.getQueryResults`: waits up to `wait_ms` for the job, then returns a page.
    pub async fn get_query_results(
        &self,
        job: &JobReference,
        page: &ResultsPage,
        wait_ms: u32,
        sql: Option<&str>,
    ) -> Result<QueryResponse, AppError> {
        let mut extra = vec![
            ("timeoutMs", wait_ms.to_string()),
            ("formatOptions.useInt64Timestamp", "true".to_string()),
        ];
        if let Some(m) = page.max_results {
            extra.push(("maxResults", m.to_string()));
        }
        if let Some(t) = page.page_token.as_ref().filter(|t| !t.is_empty()) {
            extra.push(("pageToken", t.clone()));
        } else if let Some(i) = page.start_index {
            extra.push(("startIndex", i.to_string()));
        }
        let q = Self::job_query(job, extra);
        let project = if job.project_id.is_empty() {
            &self.project
        } else {
            &job.project_id
        };
        let r: QueryResponse = self
            .send(
                HttpRequest::get(self.url(&["projects", project, "queries", &job.job_id], &q)),
                sql,
            )
            .await?
            .parse("jobs.getQueryResults")?;
        if let Some(first) = r.errors.first()
            && r.complete()
        {
            return Err(job_error(first, &r.errors, sql));
        }
        Ok(r)
    }

    pub async fn get_job(&self, job: &JobReference) -> Result<Job, AppError> {
        let project = if job.project_id.is_empty() {
            &self.project
        } else {
            &job.project_id
        };
        self.get(
            self.url(
                &["projects", project, "jobs", &job.job_id],
                &Self::job_query(job, Vec::new()),
            ),
            "jobs.get",
        )
        .await
    }

    /// Asks BigQuery to cancel the job. A job that has finished or does not exist (it was never
    /// created) is no error.
    pub async fn cancel_job(&self, job: &JobReference) -> Result<(), AppError> {
        let project = if job.project_id.is_empty() {
            &self.project
        } else {
            &job.project_id
        };
        let url = self.url(
            &["projects", project, "jobs", &job.job_id, "cancel"],
            &Self::job_query(job, Vec::new()),
        );
        match self
            .post::<CancelReply>(url, &serde_json::json!({}), "jobs.cancel", None)
            .await
        {
            Ok(_) | Err(AppError::NotFound(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// The project's running and pending jobs, with their SQL (`jobs.list`, one page).
    pub async fn list_running_jobs(&self, max: u32) -> Result<Vec<Job>, AppError> {
        let mut out = Vec::new();
        for state in ["running", "pending"] {
            let page: JobList = self
                .get(
                    self.url(
                        &["projects", &self.project, "jobs"],
                        &[
                            ("stateFilter", state.into()),
                            ("projection", "full".into()),
                            ("maxResults", max.to_string()),
                        ],
                    ),
                    "jobs.list",
                )
                .await?;
            out.extend(page.jobs);
        }
        Ok(out)
    }

    /// Runs `sql` as a job with the id `job_id` and waits for it (`stop` ends the wait). The
    /// job's first page holds at most `max_results` rows; `next_page` reads the others.
    pub async fn run_query(
        &self,
        job_id: &str,
        sql: &str,
        params: &[serde_json::Value],
        opts: &QueryOptions,
        max_results: Option<u32>,
        stop: &CancellationToken,
    ) -> Result<QueryRun, AppError> {
        let body = super::jobs::insert_body(&self.project, job_id, sql, params, opts);
        let inserted = tokio::select! {
            biased;
            _ = stop.cancelled() => return Err(AppError::Cancelled),
            r = self.insert_job(&body, sql) => r?,
        };
        let job = inserted.job_reference.unwrap_or_else(|| JobReference {
            project_id: self.project.clone(),
            job_id: job_id.to_string(),
            location: opts.location.clone(),
        });
        let first = self
            .wait_for_results(&job, max_results, Some(sql), stop)
            .await?;
        Ok(QueryRun { job, first })
    }

    /// Polls the job until it has finished, then returns its first page.
    pub async fn wait_for_results(
        &self,
        job: &JobReference,
        max_results: Option<u32>,
        sql: Option<&str>,
        stop: &CancellationToken,
    ) -> Result<QueryResponse, AppError> {
        let page = ResultsPage {
            max_results,
            ..ResultsPage::default()
        };
        let mut pause = POLL_PAUSE;
        loop {
            let r = tokio::select! {
                biased;
                _ = stop.cancelled() => return Err(AppError::Cancelled),
                r = self.get_query_results(job, &page, POLL_WAIT_MS, sql) => r?,
            };
            if r.complete() {
                return Ok(r);
            }
            tokio::select! {
                biased;
                _ = stop.cancelled() => return Err(AppError::Cancelled),
                _ = tokio::time::sleep(pause) => {}
            }
            pause = (pause * 2).min(POLL_PAUSE_MAX);
        }
    }

    /// Another page of a finished job's results.
    pub async fn next_page(
        &self,
        job: &JobReference,
        page_token: &str,
        max_results: Option<u32>,
    ) -> Result<QueryResponse, AppError> {
        let page = ResultsPage {
            page_token: Some(page_token.to_string()),
            start_index: None,
            max_results,
        };
        self.get_query_results(job, &page, 0, None).await
    }

    /// What `sql` would process: a dry run creates no job and bills nothing.
    pub async fn dry_run(
        &self,
        sql: &str,
        params: &[serde_json::Value],
        opts: &QueryOptions,
    ) -> Result<DryRun, AppError> {
        let opts = QueryOptions {
            dry_run: true,
            ..opts.clone()
        };
        let body = super::jobs::query_body(sql, params, &opts, None, None);
        let r = self.query(&body, sql).await?;
        Ok(DryRun {
            bytes_processed: r.total_bytes_processed,
            schema: r.schema,
        })
    }
}

/// Stops a running query: the local wait first, then the job on the server.
pub struct JobCancel {
    pub client: Arc<Client>,
    pub job: JobReference,
    pub stop: CancellationToken,
}

impl CancelAction for JobCancel {
    fn fire(&self) -> BoxFuture<'static, Result<(), AppError>> {
        self.stop.cancel();
        let client = Arc::clone(&self.client);
        let job = self.job.clone();
        Box::pin(async move { client.cancel_job(&job).await })
    }

    fn label(&self) -> &'static str {
        "job cancel"
    }
}
