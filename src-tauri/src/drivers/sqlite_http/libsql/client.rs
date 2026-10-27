//! The Hrana-over-HTTP client of the LibSQL driver.
//!
//! Every unit of work is one Hrana `batch` sent in one pipeline request. On a new stream the
//! batch starts with the session's prelude (when read-only `PRAGMA query_only = 1`, then
//! `PRAGMA foreign_keys`, the startup commands, and `PRAGMA query_only = 1` again after them),
//! and the work's steps only run when the prelude succeeded: a read-only stream whose
//! `query_only` failed never runs the user's statement.
//!
//! Worker calls use a new stream each time and close it in the same request. The editor holds
//! its stream (`StreamState`) across calls, so `BEGIN` and PRAGMAs persist, follows the
//! server's `base_url` on the same origin, and learns the transaction state from
//! `get_autocommit` (Hrana 3).

use std::sync::{Arc, Mutex};

use futures::StreamExt as _;

use crate::drivers::TxStatement;
use crate::drivers::sqlite_http::catalog::{CatalogQuery, Rows};
use crate::drivers::sqlite_http::guard::{
    GUARD_SQL, GuardFailure, guard_error, guard_params, parse_guard_failure,
};
use crate::drivers::sqlite_http::transport::{ByteStream, HttpRequest, Transport};
use crate::error::AppError;
use crate::types::query::ExecOutcome;
use crate::types::value::Value;

use super::decode::{encode_values, exec_outcome, plain_rows};
use super::error::{ENGINE, HttpFailure, STREAM_LOST_HINT, hrana_error};
use super::hrana::{
    Batch, BatchCond, BatchResult, BatchStep, Col, CursorEntry, CursorHead, CursorRequest,
    DescribeResult, HranaError, HranaValue, LineBuffer, PipelineRequest, PipelineResponse, Stmt,
    StmtResult, StreamRequest, StreamResponse, StreamResult, parse_line,
};
use super::url::{endpoint, follow_base_url};

/// The protocol version a server speaks over HTTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    V2,
    /// Adds `get_autocommit` and the streaming cursor.
    V3,
}

impl Version {
    fn pipeline_path(self) -> &'static str {
        match self {
            Version::V2 => "v2/pipeline",
            Version::V3 => "v3/pipeline",
        }
    }
}

/// What every new stream runs first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prelude {
    /// `PRAGMA foreign_keys`, when the profile sets it.
    pub foreign_keys: Option<bool>,
    /// The profile's startup commands, one statement per step.
    pub startup: Vec<String>,
    /// `PRAGMA query_only = 1`.
    pub read_only: bool,
}

impl Prelude {
    /// From a profile's settings: its startup commands split into statements as SQLite reads
    /// them (Hrana wants one statement per step).
    pub fn new(foreign_keys: Option<bool>, startup_sql: Option<&str>, read_only: bool) -> Prelude {
        let startup = startup_sql
            .map(|sql| {
                crate::sqlgen::split::split_statements(sql, crate::sqlgen::Dialect::Sqlite)
                    .into_iter()
                    .map(|s| s.sql)
                    .filter(|s| !s.trim().is_empty())
                    .collect()
            })
            .unwrap_or_default();
        Prelude {
            foreign_keys,
            startup,
            read_only,
        }
    }

    /// `query_only` comes first, so a read-only session's startup commands cannot write either,
    /// and again after them, so one of them (`PRAGMA query_only = 0`) cannot turn it off.
    pub fn steps(&self) -> Vec<Stmt> {
        const QUERY_ONLY: &str = "PRAGMA query_only = 1";
        let mut steps = Vec::new();
        if self.read_only {
            steps.push(Stmt::no_rows(QUERY_ONLY));
        }
        match self.foreign_keys {
            Some(true) => steps.push(Stmt::no_rows("PRAGMA foreign_keys = ON")),
            Some(false) => steps.push(Stmt::no_rows("PRAGMA foreign_keys = OFF")),
            None => {}
        }
        steps.extend(self.startup.iter().map(Stmt::no_rows));
        if self.read_only && !self.startup.is_empty() {
            steps.push(Stmt::no_rows(QUERY_ONLY));
        }
        steps
    }
}

/// A stream the caller keeps between calls (the editor's).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StreamState {
    baton: Option<String>,
    base_url: Option<String>,
    /// A request was sent and its reply never read (the call was cancelled): the server has
    /// moved the baton on, so the stream cannot be used again.
    in_flight: bool,
    /// The server dropped the stream the caller held, and with it an open transaction.
    lost: bool,
    /// From the last `get_autocommit`; `None` before the first, or on Hrana 2.
    pub autocommit: Option<bool>,
}

impl StreamState {
    pub fn is_open(&self) -> bool {
        self.baton.is_some()
    }

    /// Whether a held stream was dropped since the last call; the session then forgets its
    /// transaction (and says so).
    pub fn take_lost(&mut self) -> bool {
        std::mem::take(&mut self.lost)
    }

    /// Forgets the stream; a stream that was held (or had a request out) counts as lost.
    fn drop_stream(&mut self) {
        let lost = self.lost || self.is_open() || self.in_flight;
        *self = StreamState {
            lost,
            ..StreamState::default()
        };
    }
}

/// The outcome of one step of a batch.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Done(StmtResult),
    Failed(HranaError),
    /// Its condition did not hold.
    Skipped,
}

impl Step {
    /// The step as a result, or the error the server reported for it.
    pub fn into_result(self) -> Result<StmtResult, AppError> {
        match self {
            Step::Done(r) => Ok(r),
            Step::Failed(e) => Err(hrana_error(&e)),
            Step::Skipped => Err(AppError::db(format!(
                "{ENGINE} skipped the statement after an earlier failure"
            ))),
        }
    }
}

pub struct HranaClient {
    transport: Arc<dyn Transport>,
    base: String,
    prelude: Prelude,
    version: Mutex<Option<Version>>,
}

impl HranaClient {
    pub fn new(
        transport: Arc<dyn Transport>,
        base: impl Into<String>,
        prelude: Prelude,
    ) -> HranaClient {
        HranaClient {
            transport,
            base: base.into(),
            prelude,
            version: Mutex::new(None),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// `GET v3`, else `GET v2`; a 2xx says the server speaks it. Asked once per client. Every
    /// other answer means Hrana 2 over `v2/pipeline` (Turso documents only that one), except a
    /// refused token, which is the connection's error.
    pub async fn version(&self) -> Result<Version, AppError> {
        if let Some(v) = *self.version.lock().unwrap_or_else(|p| p.into_inner()) {
            return Ok(v);
        }
        let mut found = Version::V2;
        for (path, version) in [("v3", Version::V3), ("v2", Version::V2)] {
            let reply = self
                .transport
                .send(HttpRequest::get(endpoint(&self.base, path)))
                .await?;
            if reply.is_success() {
                found = version;
                break;
            }
            if matches!(reply.status, 401 | 403) {
                return Err(HttpFailure::from_response(&reply).to_error());
            }
        }
        *self.version.lock().unwrap_or_else(|p| p.into_inner()) = Some(found);
        Ok(found)
    }

    /// One pipeline request on `stream`. An error status ends the stream (the spec), and so
    /// does a transport failure (whether the server ran the request is unknown).
    async fn pipeline(
        &self,
        stream: &mut StreamState,
        requests: Vec<StreamRequest>,
    ) -> Result<Vec<StreamResult>, AppError> {
        let version = self.version().await?;
        if stream.in_flight {
            stream.drop_stream();
        }
        let base = stream.base_url.clone().unwrap_or_else(|| self.base.clone());
        let count = requests.len();
        let body = PipelineRequest {
            baton: stream.baton.clone(),
            requests,
        };
        let req = HttpRequest::post_json(endpoint(&base, version.pipeline_path()), &body)?;
        stream.in_flight = true;
        let reply = self.transport.send(req).await;
        stream.in_flight = false;
        let parsed = reply.and_then(|reply| {
            if reply.is_success() {
                reply.parse::<PipelineResponse>("pipeline")
            } else {
                Err(HttpFailure::from_response(&reply).to_error())
            }
        });
        let parsed = match parsed {
            Ok(p) => p,
            Err(e) => {
                stream.drop_stream();
                return Err(e);
            }
        };
        stream.baton = parsed.baton;
        follow(stream, &base, parsed.base_url.as_deref());
        if parsed.results.len() != count {
            return Err(AppError::db(format!(
                "{ENGINE} answered {} result(s) to {count} request(s)",
                parsed.results.len()
            )));
        }
        Ok(parsed.results)
    }

    /// Runs `work` as one batch: on a new stream (closed in the same request) when `stream` is
    /// `None`, else on the caller's stream. The prelude runs first on a stream that is new.
    pub async fn run(
        &self,
        stream: Option<&mut StreamState>,
        work: Batch,
    ) -> Result<Vec<Step>, AppError> {
        let mut own = StreamState::default();
        let worker = stream.is_none();
        let stream = stream.unwrap_or(&mut own);
        if stream.in_flight {
            stream.drop_stream();
        }
        let prelude = if stream.is_open() {
            Vec::new()
        } else {
            self.prelude.steps()
        };
        let (batch, offset) = with_prelude(&prelude, work);
        let mut requests = vec![StreamRequest::Batch { batch }];
        let version = self.version().await?;
        if worker {
            requests.push(StreamRequest::Close);
        } else if version == Version::V3 {
            requests.push(StreamRequest::GetAutocommit);
        }
        let mut results = self.pipeline(stream, requests).await?.into_iter();
        let batch = match results.next() {
            Some(StreamResult::Ok {
                response: StreamResponse::Batch { result },
            }) => result,
            Some(StreamResult::Error { error }) => return Err(hrana_error(&error)),
            other => {
                return Err(AppError::db(format!(
                    "{ENGINE} answered a batch with {other:?}"
                )));
            }
        };
        if let Some(StreamResult::Ok {
            response: StreamResponse::GetAutocommit { is_autocommit },
        }) = results.next()
        {
            stream.autocommit = Some(is_autocommit);
        }
        steps_of(batch, offset)
    }

    /// One statement on a new stream.
    pub async fn execute(&self, sql: &str, params: &[Value]) -> Result<StmtResult, AppError> {
        let steps = self.run(None, single(sql, params)?).await?;
        first(steps)
    }

    /// One statement on the caller's stream (the editor). When the server had dropped the
    /// stream, the statement ran on a new one and `take_lost` says so; its error says so too.
    pub async fn execute_on(
        &self,
        stream: &mut StreamState,
        sql: &str,
        params: &[Value],
    ) -> Result<StmtResult, AppError> {
        let result = match self.run(Some(stream), single(sql, params)?).await {
            Ok(steps) => first(steps),
            Err(e) => Err(e),
        };
        result.map_err(|e| if stream.lost { lost_hint(e) } else { e })
    }

    /// The catalog statements of one call, in one round trip.
    pub async fn catalog(&self, queries: &[CatalogQuery]) -> Result<Vec<Rows>, AppError> {
        let steps = queries
            .iter()
            .map(|q| {
                Ok(BatchStep {
                    condition: None,
                    stmt: Stmt::new(q.sql, encode_values(&q.params)?),
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        self.run(None, Batch { steps })
            .await?
            .into_iter()
            .map(|s| s.into_result().map(|r| plain_rows(&r)))
            .collect()
    }

    /// The staged changes as one batch: `BEGIN`, each statement only if the previous step
    /// succeeded, a guard after each that expects a row count, `COMMIT` if everything did, and
    /// `ROLLBACK` if the commit did not happen.
    pub async fn transaction(
        &self,
        statements: &[TxStatement],
    ) -> Result<Vec<ExecOutcome>, AppError> {
        let plan = tx_plan(statements)?;
        let steps = self.run(None, plan.batch.clone()).await?;
        tx_outcome(statements, &plan, &steps)
    }

    /// How the server reads `sql` (parameters, columns, whether it only reads), on a new
    /// stream.
    pub async fn describe(&self, sql: &str) -> Result<DescribeResult, AppError> {
        let mut stream = StreamState::default();
        let results = self
            .pipeline(
                &mut stream,
                vec![
                    StreamRequest::Describe {
                        sql: sql.to_string(),
                    },
                    StreamRequest::Close,
                ],
            )
            .await?;
        match results.into_iter().next() {
            Some(StreamResult::Ok {
                response: StreamResponse::Describe { result },
            }) => Ok(result),
            Some(StreamResult::Error { error }) => Err(hrana_error(&error)),
            other => Err(AppError::db(format!(
                "{ENGINE} answered describe with {other:?}"
            ))),
        }
    }

    /// Closes a held stream; the server would expire it anyway, so failures are ignored.
    pub async fn close(&self, stream: &mut StreamState) {
        if stream.is_open() && !stream.in_flight {
            let _ = self.pipeline(stream, vec![StreamRequest::Close]).await;
        }
        *stream = StreamState::default();
    }

    /// Runs `work` through `v3/cursor`, its rows arriving as the server sends them. Hrana 2
    /// has no cursor: `Unsupported`, and the caller uses `run`. A held stream stays unusable
    /// until `Cursor::update` gives it the baton the cursor's first line carries.
    pub async fn cursor(
        &self,
        stream: Option<&mut StreamState>,
        work: Batch,
    ) -> Result<Cursor, AppError> {
        if self.version().await? != Version::V3 {
            return Err(AppError::Unsupported(format!(
                "{ENGINE} server without Hrana 3 has no cursor"
            )));
        }
        let mut own = StreamState::default();
        let stream = stream.unwrap_or(&mut own);
        if stream.in_flight {
            stream.drop_stream();
        }
        let prelude = if stream.is_open() {
            Vec::new()
        } else {
            self.prelude.steps()
        };
        let (batch, offset) = with_prelude(&prelude, work);
        let base = stream.base_url.clone().unwrap_or_else(|| self.base.clone());
        let body = CursorRequest {
            baton: stream.baton.clone(),
            batch,
        };
        stream.in_flight = true;
        let reply = self
            .transport
            .send_streaming(HttpRequest::post_json(endpoint(&base, "v3/cursor"), &body)?)
            .await?;
        if !(200..300).contains(&reply.status) {
            return Err(HttpFailure::from_response(&reply.collect().await).to_error());
        }
        Ok(Cursor {
            base,
            body: reply.body,
            lines: LineBuffer::default(),
            offset,
            head: None,
            ended: false,
            in_prelude: offset > 0,
        })
    }
}

/// The stream's base after a reply to a request sent to `base`: the server's `base_url` when it
/// names one on the same origin (otherwise `base` stays), none once the stream has ended.
fn follow(stream: &mut StreamState, base: &str, base_url: Option<&str>) {
    if let Some(next) = base_url.and_then(|u| follow_base_url(base, u)) {
        stream.base_url = Some(next);
    }
    if stream.baton.is_none() {
        stream.base_url = None;
    }
}

/// A batch of one statement.
fn single(sql: &str, params: &[Value]) -> Result<Batch, AppError> {
    Ok(Batch {
        steps: vec![BatchStep {
            condition: None,
            stmt: Stmt::new(sql, encode_values(params)?),
        }],
    })
}

fn first(steps: Vec<Step>) -> Result<StmtResult, AppError> {
    steps
        .into_iter()
        .next()
        .unwrap_or(Step::Skipped)
        .into_result()
}

/// An error on a stream the server dropped says so.
fn lost_hint(e: AppError) -> AppError {
    match e {
        AppError::Db {
            message,
            detail,
            hint: None,
            sqlstate,
            vendor_code,
            position,
            statement_index,
        } => AppError::Db {
            message,
            detail,
            hint: Some(STREAM_LOST_HINT.to_string()),
            sqlstate,
            vendor_code,
            position,
            statement_index,
        },
        other => other,
    }
}

/// `work` behind `prelude`: the prelude steps chained on each other, and each step of the work
/// also conditioned on the last prelude step. Returns the batch and where the work starts.
pub fn with_prelude(prelude: &[Stmt], work: Batch) -> (Batch, u32) {
    let offset = prelude.len() as u32;
    let mut steps: Vec<BatchStep> = prelude
        .iter()
        .enumerate()
        .map(|(i, stmt)| BatchStep {
            condition: (i > 0).then(|| BatchCond::Ok { step: i as u32 - 1 }),
            stmt: stmt.clone(),
        })
        .collect();
    let gate = offset.checked_sub(1).map(|step| BatchCond::Ok { step });
    for step in work.steps {
        let own = step.condition.map(|c| shift(c, offset));
        let condition = match (gate.clone(), own) {
            (None, own) => own,
            (Some(g), None) => Some(g),
            (Some(g), Some(c)) => Some(BatchCond::And { conds: vec![g, c] }),
        };
        steps.push(BatchStep {
            condition,
            stmt: step.stmt,
        });
    }
    (Batch { steps }, offset)
}

fn shift(cond: BatchCond, by: u32) -> BatchCond {
    match cond {
        BatchCond::Ok { step } => BatchCond::Ok { step: step + by },
        BatchCond::Error { step } => BatchCond::Error { step: step + by },
        BatchCond::Not { cond } => BatchCond::Not {
            cond: Box::new(shift(*cond, by)),
        },
        BatchCond::And { conds } => BatchCond::And {
            conds: conds.into_iter().map(|c| shift(c, by)).collect(),
        },
        BatchCond::Or { conds } => BatchCond::Or {
            conds: conds.into_iter().map(|c| shift(c, by)).collect(),
        },
        BatchCond::IsAutocommit => BatchCond::IsAutocommit,
    }
}

/// The work's steps of a batch result; a failed prelude step fails the call.
fn steps_of(result: BatchResult, offset: u32) -> Result<Vec<Step>, AppError> {
    let BatchResult {
        step_results,
        step_errors,
    } = result;
    let mut errors = step_errors.into_iter();
    let mut steps = Vec::with_capacity(step_results.len());
    for (i, result) in step_results.into_iter().enumerate() {
        let error = errors.next().flatten();
        let step = match (result, error) {
            (_, Some(e)) => Step::Failed(e),
            (Some(r), None) => Step::Done(r),
            (None, None) => Step::Skipped,
        };
        if (i as u32) < offset {
            if let Step::Failed(e) = &step {
                let mut err = hrana_error(e);
                if let AppError::Db { message, .. } = &mut err {
                    *message = format!("the session's startup statements failed: {message}");
                }
                return Err(err);
            }
            continue;
        }
        steps.push(step);
    }
    Ok(steps)
}

/// What each step of a transaction batch is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Begin,
    Statement(usize),
    Guard(usize),
    Commit,
    Rollback,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TxPlan {
    pub batch: Batch,
    pub roles: Vec<Role>,
}

pub fn tx_plan(statements: &[TxStatement]) -> Result<TxPlan, AppError> {
    let mut steps = vec![BatchStep {
        condition: None,
        stmt: Stmt::no_rows("BEGIN"),
    }];
    let mut roles = vec![Role::Begin];
    for (i, st) in statements.iter().enumerate() {
        let prev = steps.len() as u32 - 1;
        steps.push(BatchStep {
            condition: Some(BatchCond::Ok { step: prev }),
            stmt: Stmt::new(st.sql.clone(), encode_values(&st.params)?),
        });
        roles.push(Role::Statement(i));
        if let Some(n) = st.expect_affected {
            steps.push(BatchStep {
                condition: Some(BatchCond::Ok { step: prev + 1 }),
                stmt: Stmt {
                    want_rows: false,
                    ..Stmt::new(GUARD_SQL, encode_values(&guard_params(n, i))?)
                },
            });
            roles.push(Role::Guard(i));
        }
    }
    let last = steps.len() as u32 - 1;
    steps.push(BatchStep {
        condition: Some(BatchCond::Ok { step: last }),
        stmt: Stmt::no_rows("COMMIT"),
    });
    roles.push(Role::Commit);
    steps.push(BatchStep {
        condition: Some(BatchCond::And {
            conds: vec![
                BatchCond::Ok { step: 0 },
                BatchCond::Not {
                    cond: Box::new(BatchCond::Ok { step: last + 1 }),
                },
            ],
        }),
        stmt: Stmt::no_rows("ROLLBACK"),
    });
    roles.push(Role::Rollback);
    Ok(TxPlan {
        batch: Batch { steps },
        roles,
    })
}

/// The outcome of a transaction batch: each statement's, once `COMMIT` ran, or the first
/// failure (a failed guard reads like `check_affected`).
pub fn tx_outcome(
    statements: &[TxStatement],
    plan: &TxPlan,
    steps: &[Step],
) -> Result<Vec<ExecOutcome>, AppError> {
    if steps.len() != plan.roles.len() {
        return Err(AppError::db(format!(
            "{ENGINE} answered {} step(s) of a {}-step transaction",
            steps.len(),
            plan.roles.len()
        )));
    }
    let affected_of = |i: usize| -> u64 {
        plan.roles
            .iter()
            .position(|r| *r == Role::Statement(i))
            .and_then(|p| match &steps[p] {
                Step::Done(r) => Some(r.affected_row_count),
                _ => None,
            })
            .unwrap_or(0)
    };
    for (role, step) in plan.roles.iter().zip(steps) {
        let Step::Failed(e) = step else { continue };
        let error = match role {
            Role::Statement(i) => hrana_error(e).with_statement_index(*i),
            Role::Guard(i) => {
                let failure = parse_guard_failure(&e.message).unwrap_or(GuardFailure {
                    index: *i,
                    expected: statements
                        .get(*i)
                        .and_then(|s| s.expect_affected)
                        .unwrap_or(0),
                    changed: affected_of(*i),
                });
                guard_error(statements, failure)
            }
            Role::Begin | Role::Commit => hrana_error(e),
            // A failed rollback after a failure: the failure is what matters.
            Role::Rollback => continue,
        };
        return Err(error);
    }
    let committed = plan
        .roles
        .iter()
        .position(|r| *r == Role::Commit)
        .is_some_and(|p| matches!(steps[p], Step::Done(_)));
    if !committed {
        return Err(AppError::db(format!(
            "{ENGINE} did not commit the changes, and reported no error"
        )));
    }
    let mut outcomes = Vec::with_capacity(statements.len());
    for (role, step) in plan.roles.iter().zip(steps) {
        if let (Role::Statement(i), Step::Done(r)) = (role, step) {
            outcomes.push(exec_outcome(&statements[*i].sql, r));
        }
    }
    Ok(outcomes)
}

/// What a cursor reports, step indexes counted from the work's first step.
#[derive(Debug, Clone, PartialEq)]
pub enum CursorEvent {
    Begin {
        step: u32,
        cols: Vec<Col>,
    },
    Row(Vec<HranaValue>),
    End {
        affected_row_count: u64,
        last_insert_rowid: Option<String>,
    },
    StepError {
        step: u32,
        error: HranaError,
    },
}

/// The entries of a `v3/cursor` reply, read as they arrive. Dropping it mid-way closes the
/// connection; the stream it ran on is then unusable.
pub struct Cursor {
    /// Where the request went.
    base: String,
    body: ByteStream,
    lines: LineBuffer,
    offset: u32,
    head: Option<CursorHead>,
    ended: bool,
    /// Inside a step of the prelude, whose rows and end are not the caller's.
    in_prelude: bool,
}

impl Cursor {
    /// The stream's new baton and base URL, once the first line has been read.
    pub fn head(&self) -> Option<&CursorHead> {
        self.head.as_ref()
    }

    /// Stores the stream the cursor ran on into `stream`, for the next call.
    pub fn update(&self, stream: &mut StreamState) {
        if let Some(head) = &self.head {
            stream.baton = head.baton.clone();
            follow(stream, &self.base, head.base_url.as_deref());
            stream.in_flight = false;
        }
    }

    async fn line(&mut self) -> Result<Option<Vec<u8>>, AppError> {
        loop {
            if let Some(line) = self.lines.next_line() {
                return Ok(Some(line));
            }
            if self.ended {
                return Ok(self.lines.finish());
            }
            match self.body.next().await {
                Some(Ok(piece)) => self.lines.push(&piece),
                Some(Err(e)) => return Err(e),
                None => self.ended = true,
            }
        }
    }

    /// The next event; `None` at the end. The prelude's events are read here: its failure is
    /// the cursor's error.
    pub async fn next_event(&mut self) -> Result<Option<CursorEvent>, AppError> {
        if self.head.is_none() {
            let Some(line) = self.line().await? else {
                return Err(AppError::db(format!("{ENGINE} sent an empty cursor")));
            };
            self.head = Some(parse_line(&line, "cursor head")?);
        }
        loop {
            let Some(line) = self.line().await? else {
                return Ok(None);
            };
            let entry: CursorEntry = parse_line(&line, "cursor entry")?;
            match entry {
                CursorEntry::StepBegin { step, cols } => {
                    self.in_prelude = step < self.offset;
                    if !self.in_prelude {
                        return Ok(Some(CursorEvent::Begin {
                            step: step - self.offset,
                            cols,
                        }));
                    }
                }
                CursorEntry::StepError { step, error } if step < self.offset => {
                    let mut err = hrana_error(&error);
                    if let AppError::Db { message, .. } = &mut err {
                        *message = format!("the session's startup statements failed: {message}");
                    }
                    return Err(err);
                }
                CursorEntry::StepError { step, error } => {
                    self.in_prelude = false;
                    return Ok(Some(CursorEvent::StepError {
                        step: step - self.offset,
                        error,
                    }));
                }
                CursorEntry::Row { row } if !self.in_prelude => {
                    return Ok(Some(CursorEvent::Row(row)));
                }
                CursorEntry::StepEnd {
                    affected_row_count,
                    last_insert_rowid,
                } if !self.in_prelude => {
                    return Ok(Some(CursorEvent::End {
                        affected_row_count,
                        last_insert_rowid,
                    }));
                }
                CursorEntry::Error { error } => return Err(hrana_error(&error)),
                // Prelude rows and ends, and entries a newer server added.
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cond(c: &Option<BatchCond>) -> serde_json::Value {
        serde_json::to_value(c).unwrap()
    }

    #[test]
    fn the_prelude_gates_the_work() {
        let prelude = Prelude {
            foreign_keys: Some(true),
            startup: vec!["PRAGMA cache_size = 100".into()],
            read_only: true,
        };
        let work = Batch {
            steps: vec![
                BatchStep {
                    condition: None,
                    stmt: Stmt::new("SELECT 1", vec![]),
                },
                BatchStep {
                    condition: Some(BatchCond::Not {
                        cond: Box::new(BatchCond::Ok { step: 0 }),
                    }),
                    stmt: Stmt::new("SELECT 2", vec![]),
                },
            ],
        };
        let (batch, offset) = with_prelude(&prelude.steps(), work.clone());
        assert_eq!(offset, 4);
        let sql: Vec<&str> = batch.steps.iter().map(|s| s.stmt.sql.as_str()).collect();
        assert_eq!(
            sql,
            [
                "PRAGMA query_only = 1",
                "PRAGMA foreign_keys = ON",
                "PRAGMA cache_size = 100",
                "PRAGMA query_only = 1",
                "SELECT 1",
                "SELECT 2"
            ]
        );
        assert_eq!(cond(&batch.steps[0].condition), json!(null));
        assert_eq!(
            cond(&batch.steps[2].condition),
            json!({"type": "ok", "step": 1})
        );
        assert_eq!(
            cond(&batch.steps[3].condition),
            json!({"type": "ok", "step": 2})
        );
        assert_eq!(
            cond(&batch.steps[4].condition),
            json!({"type": "ok", "step": 3})
        );
        assert_eq!(
            cond(&batch.steps[5].condition),
            json!({"type": "and", "conds": [{"type": "ok", "step": 3},
                {"type": "not", "cond": {"type": "ok", "step": 4}}]})
        );
        // Without startup commands nothing can undo the first `query_only`.
        let plain = Prelude {
            read_only: true,
            ..Prelude::default()
        };
        assert_eq!(plain.steps().len(), 1);
        // Without a prelude the work is unchanged.
        let (same, offset) = with_prelude(&[], work.clone());
        assert_eq!((same, offset), (work, 0));
        assert!(Prelude::default().steps().is_empty());
        let from_profile = Prelude::new(
            None,
            Some("PRAGMA cache_size = 10; -- note\nCREATE TEMP TABLE x (a);\n"),
            false,
        );
        assert_eq!(
            from_profile.startup,
            ["PRAGMA cache_size = 10", "CREATE TEMP TABLE x (a)"]
        );
        assert!(Prelude::new(Some(true), None, false).startup.is_empty());
    }

    #[test]
    fn transactions_chain_guard_commit_and_roll_back() {
        let stmts = vec![
            TxStatement::new(
                "UPDATE t SET a = ?1 WHERE id = ?2",
                vec![Value::Int(1), Value::Int(2)],
            )
            .expecting(1),
            TxStatement::new("INSERT INTO t (a) VALUES (3)", vec![]),
        ];
        let plan = tx_plan(&stmts).unwrap();
        assert_eq!(
            plan.roles,
            [
                Role::Begin,
                Role::Statement(0),
                Role::Guard(0),
                Role::Statement(1),
                Role::Commit,
                Role::Rollback
            ]
        );
        let c: Vec<serde_json::Value> = plan
            .batch
            .steps
            .iter()
            .map(|s| cond(&s.condition))
            .collect();
        assert_eq!(c[1], json!({"type": "ok", "step": 0}));
        assert_eq!(c[2], json!({"type": "ok", "step": 1}));
        assert_eq!(c[3], json!({"type": "ok", "step": 2}));
        assert_eq!(c[4], json!({"type": "ok", "step": 3}));
        assert_eq!(
            c[5],
            json!({"type": "and", "conds": [{"type": "ok", "step": 0},
                {"type": "not", "cond": {"type": "ok", "step": 4}}]})
        );
        assert_eq!(plan.batch.steps[2].stmt.sql, GUARD_SQL);

        let done = |affected: u64, rowid: Option<&str>| {
            Step::Done(StmtResult {
                affected_row_count: affected,
                last_insert_rowid: rowid.map(str::to_string),
                ..StmtResult::default()
            })
        };
        let ok = vec![
            done(0, None),
            done(1, None),
            done(0, None),
            done(1, Some("7")),
            done(0, None),
            Step::Skipped,
        ];
        let outcomes = tx_outcome(&stmts, &plan, &ok).unwrap();
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0].affected, 1);
        assert_eq!(outcomes[1].last_insert_id.as_deref(), Some("7"));

        let guard_failed = vec![
            done(0, None),
            done(0, None),
            Step::Failed(HranaError {
                message: "bad JSON path: 'tabler: statement 0 expected 1 changed 0'".into(),
                code: Some("SQLITE_ERROR".into()),
            }),
            Step::Skipped,
            Step::Skipped,
            done(0, None),
        ];
        let e = tx_outcome(&stmts, &plan, &guard_failed).unwrap_err();
        assert!(
            e.to_string()
                .starts_with("expected to affect 1 row(s) but matched 0"),
            "{e}"
        );
        // A guard error whose text says nothing still names the statement and its count.
        let mut unreadable = guard_failed.clone();
        unreadable[2] = Step::Failed(HranaError {
            message: "malformed".into(),
            code: None,
        });
        let e = tx_outcome(&stmts, &plan, &unreadable).unwrap_err();
        assert!(
            e.to_string()
                .starts_with("expected to affect 1 row(s) but matched 0"),
            "{e}"
        );

        let statement_failed = vec![
            done(0, None),
            done(1, None),
            done(0, None),
            Step::Failed(HranaError {
                message: "UNIQUE constraint failed: t.a".into(),
                code: Some("SQLITE_CONSTRAINT_UNIQUE".into()),
            }),
            Step::Skipped,
            done(0, None),
        ];
        match tx_outcome(&stmts, &plan, &statement_failed).unwrap_err() {
            AppError::Db {
                statement_index,
                vendor_code,
                ..
            } => {
                assert_eq!(statement_index, Some(1));
                assert_eq!(vendor_code, Some(2067));
            }
            other => panic!("{other:?}"),
        }
        let silent = vec![
            done(0, None),
            done(1, None),
            done(0, None),
            done(1, None),
            Step::Skipped,
            Step::Skipped,
        ];
        assert!(tx_outcome(&stmts, &plan, &silent).is_err());
        assert!(tx_outcome(&stmts, &plan, &silent[..3]).is_err());
    }

    #[test]
    fn failed_prelude_steps_fail_the_call() {
        let result = BatchResult {
            step_results: vec![None, None, Some(StmtResult::default())],
            step_errors: vec![
                Some(HranaError {
                    message: "no such pragma".into(),
                    code: None,
                }),
                None,
                None,
            ],
        };
        let e = steps_of(result.clone(), 2).unwrap_err();
        assert!(e.to_string().contains("startup statements failed"), "{e}");
        let steps = steps_of(result, 0).unwrap();
        assert!(matches!(steps[0], Step::Failed(_)));
        assert_eq!(steps[1], Step::Skipped);
        assert!(matches!(steps[2], Step::Done(_)));
    }
}
