//! One SQL Server connection: a tokio task (an actor) that owns the tiberius client and runs one
//! job at a time.
//!
//! The actor exists for the cancel. tiberius cancels with a TDS Attention (`cancel_query`), which
//! needs the client mutably, while a running query's stream borrows it; and dropping a query's
//! future does not stop the server. So the actor races every await of a statement against the
//! statement's `Stop`: when it fires, the actor drops the stream, sends the Attention and drains
//! the server's acknowledgement, and the statement ends with `Cancelled`. A `Stop` belongs to
//! one statement, so a cancel that arrives late cannot hit the next one. A connection whose
//! transport failed, or whose cancel or rollback did not complete, is marked broken: the actor
//! drops the client at once (the server then ends the session and any transaction it left open)
//! and every later job fails with `NotConnected`, so the session opens a new one.
//!
//! Rows travel to the caller through a bounded channel, which pushes back on a slow sink; a caller
//! that drops its receiver (the row cap of a statement that must finish) lets the statement run
//! to its end without forwarding more rows.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ::tiberius::{Client, ColumnData, QueryItem, ToSql};
use futures::{AsyncRead, AsyncWrite, TryStreamExt};
use tokio::sync::{Notify, mpsc, oneshot};

use super::decode::{Bytes, decode_row, result_columns};
use super::errors::{Phase, map_error};
use crate::error::AppError;
use crate::sqlgen::dialect::tsql;
use crate::types::query::{ResultColumn, ResultSet};
use crate::types::value::Value;

/// How long the Attention's acknowledgement may take before the connection counts as broken.
pub const CANCEL_LIMIT: Duration = Duration::from_secs(15);

/// Rows (and result-set starts) in flight between the actor and its caller.
const CHANNEL_ROWS: usize = 1024;

/// Asks one statement to stop. Cheap to clone; firing it twice, or after the statement ended,
/// does nothing.
#[derive(Clone, Default)]
pub struct Stop(Arc<StopInner>);

#[derive(Default)]
struct StopInner {
    fired: AtomicBool,
    notify: Notify,
}

impl Stop {
    pub fn new() -> Stop {
        Stop::default()
    }

    pub fn fire(&self) {
        self.0.fired.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }

    pub fn is_fired(&self) -> bool {
        self.0.fired.load(Ordering::SeqCst)
    }

    /// Resolves once `fire` was called.
    pub async fn fired(&self) {
        loop {
            let notified = self.0.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_fired() {
                return;
            }
            notified.await;
        }
    }
}

impl std::fmt::Debug for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Stop({})", self.is_fired())
    }
}

/// What a running statement sends: the start of a result set, then its rows.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Columns(Vec<ResultColumn>),
    Row(Vec<Value>),
}

/// How a statement is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A SQL batch, as written (the editor's text between GO lines; no parameters).
    Batch,
    /// `sp_executesql` with `@P1…` parameters.
    Rpc,
}

/// How a statement ended: its result sets and rows, counted whether forwarded or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    pub sets: u32,
    pub rows: u64,
}

/// A statement running on the actor: its items, then its end.
pub struct Streaming {
    pub items: mpsc::Receiver<Item>,
    pub end: oneshot::Receiver<Result<Summary, AppError>>,
}

impl Streaming {
    /// Waits for the end, dropping whatever items are still coming (they are not forwarded).
    pub async fn finish(self) -> Result<Summary, AppError> {
        drop(self.items);
        self.end.await.unwrap_or(Err(AppError::NotConnected))
    }
}

/// One statement of a transaction, with the rows it must affect (`TxStatement::expect_affected`)
/// and whether its OUTPUT rows are wanted.
#[derive(Debug, Clone)]
pub struct TxRequest {
    pub sql: String,
    pub params: Vec<ColumnData<'static>>,
    pub expect_affected: Option<u64>,
    pub returning: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TxDone {
    pub affected: u64,
    pub rows: Option<ResultSet>,
}

enum Job {
    Stream {
        sql: String,
        params: Vec<ColumnData<'static>>,
        mode: Mode,
        bytes: Bytes,
        stop: Stop,
        items: mpsc::Sender<Item>,
        end: oneshot::Sender<Result<Summary, AppError>>,
    },
    Execute {
        sql: String,
        params: Vec<ColumnData<'static>>,
        stop: Stop,
        end: oneshot::Sender<Result<Vec<u64>, AppError>>,
    },
    Transaction {
        statements: Vec<TxRequest>,
        stop: Stop,
        end: oneshot::Sender<Result<Vec<TxDone>, AppError>>,
    },
    Close {
        end: oneshot::Sender<()>,
    },
}

/// A handle on the actor of one connection.
#[derive(Clone)]
pub struct Conn {
    jobs: mpsc::Sender<Job>,
    broken: Arc<AtomicBool>,
}

impl Conn {
    /// Starts the actor of a logged-in client.
    pub fn spawn<S>(client: Client<S>) -> Conn
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (jobs, rx) = mpsc::channel(8);
        let broken = Arc::new(AtomicBool::new(false));
        tokio::spawn(run(client, rx, Arc::clone(&broken)));
        Conn { jobs, broken }
    }

    /// The transport failed or a cancel did not complete: the connection cannot be used again.
    pub fn is_broken(&self) -> bool {
        self.broken.load(Ordering::SeqCst) || self.jobs.is_closed()
    }

    async fn submit(&self, job: Job) -> Result<(), AppError> {
        if self.is_broken() {
            return Err(AppError::NotConnected);
        }
        self.jobs
            .send(job)
            .await
            .map_err(|_| AppError::NotConnected)
    }

    /// Runs a statement, streaming its result sets.
    pub async fn stream(
        &self,
        sql: impl Into<String>,
        params: Vec<ColumnData<'static>>,
        mode: Mode,
        bytes: Bytes,
        stop: Stop,
    ) -> Result<Streaming, AppError> {
        let (items, items_rx) = mpsc::channel(CHANNEL_ROWS);
        let (end, end_rx) = oneshot::channel();
        self.submit(Job::Stream {
            sql: sql.into(),
            params,
            mode,
            bytes,
            stop,
            items,
            end,
        })
        .await?;
        Ok(Streaming {
            items: items_rx,
            end: end_rx,
        })
    }

    /// Runs a statement and collects its result sets.
    pub async fn query(
        &self,
        sql: impl Into<String>,
        params: Vec<ColumnData<'static>>,
        mode: Mode,
        bytes: Bytes,
        stop: Stop,
    ) -> Result<Vec<ResultSet>, AppError> {
        let mut running = self.stream(sql, params, mode, bytes, stop).await?;
        let sets = collect(&mut running.items).await;
        running.finish().await?;
        Ok(sets)
    }

    /// Runs a statement for its side effects: the row counts the server reports, in order.
    pub async fn execute(
        &self,
        sql: impl Into<String>,
        params: Vec<ColumnData<'static>>,
        stop: Stop,
    ) -> Result<Vec<u64>, AppError> {
        let (end, end_rx) = oneshot::channel();
        self.submit(Job::Execute {
            sql: sql.into(),
            params,
            stop,
            end,
        })
        .await?;
        end_rx.await.unwrap_or(Err(AppError::NotConnected))
    }

    /// Runs statements in one transaction, which rolls back at the first error or count that
    /// does not match (the error names the statement's index).
    pub async fn transaction(
        &self,
        statements: Vec<TxRequest>,
        stop: Stop,
    ) -> Result<Vec<TxDone>, AppError> {
        let (end, end_rx) = oneshot::channel();
        self.submit(Job::Transaction {
            statements,
            stop,
            end,
        })
        .await?;
        end_rx.await.unwrap_or(Err(AppError::NotConnected))
    }

    /// Ends the connection; returns once the actor has closed it, or after 2 s, also when the
    /// queue in front of the actor is full.
    pub async fn close(&self) {
        let (end, end_rx) = oneshot::channel();
        let closing = async {
            if self.jobs.send(Job::Close { end }).await.is_ok() {
                let _ = end_rx.await;
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(2), closing).await;
    }
}

/// The result sets of a stream, until its sender is gone.
async fn collect(items: &mut mpsc::Receiver<Item>) -> Vec<ResultSet> {
    let mut sets: Vec<ResultSet> = Vec::new();
    while let Some(item) = items.recv().await {
        match item {
            Item::Columns(columns) => sets.push(ResultSet {
                columns,
                rows: Vec::new(),
                truncated: false,
            }),
            Item::Row(row) => {
                if let Some(set) = sets.last_mut() {
                    set.rows.push(row);
                }
            }
        }
    }
    sets
}

async fn run<S>(client: Client<S>, mut jobs: mpsc::Receiver<Job>, broken: Arc<AtomicBool>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    serve(client, &mut jobs, &broken).await;
    // The client went with its socket, so the server ends the session now, and with it a
    // transaction left open and its locks, not once the last handle is dropped.
    broken.store(true, Ordering::SeqCst);
    jobs.close();
    while let Ok(job) = jobs.try_recv() {
        refuse(job);
    }
}

/// Runs jobs until the connection is closed, lost or broken; returning drops the client.
async fn serve<S>(mut client: Client<S>, jobs: &mut mpsc::Receiver<Job>, broken: &AtomicBool)
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    while let Some(job) = jobs.recv().await {
        match job {
            Job::Stream {
                sql,
                params,
                mode,
                bytes,
                stop,
                items,
                end,
            } => {
                let st = Statement {
                    sql: &sql,
                    params: &params,
                    mode,
                    bytes,
                };
                let result = stream(&mut client, &st, &stop, &items, broken).await;
                mark(broken, &result);
                let _ = end.send(result);
            }
            Job::Execute {
                sql,
                params,
                stop,
                end,
            } => {
                let result = execute(&mut client, &sql, &params, &stop, broken).await;
                mark(broken, &result);
                let _ = end.send(result);
            }
            Job::Transaction {
                statements,
                stop,
                end,
            } => {
                let result = transaction(&mut client, &statements, &stop, broken).await;
                mark(broken, &result);
                let _ = end.send(result);
            }
            Job::Close { end } => {
                let _ = tokio::time::timeout(Duration::from_secs(2), client.close()).await;
                broken.store(true, Ordering::SeqCst);
                let _ = end.send(());
                return;
            }
        }
        if broken.load(Ordering::SeqCst) {
            return;
        }
    }
}

fn refuse(job: Job) {
    match job {
        Job::Stream { end, .. } => {
            let _ = end.send(Err(AppError::NotConnected));
        }
        Job::Execute { end, .. } => {
            let _ = end.send(Err(AppError::NotConnected));
        }
        Job::Transaction { end, .. } => {
            let _ = end.send(Err(AppError::NotConnected));
        }
        Job::Close { end } => {
            let _ = end.send(());
        }
    }
}

/// A lost transport ends the connection.
fn mark<T>(broken: &AtomicBool, result: &Result<T, AppError>) {
    if matches!(result, Err(AppError::NotConnected | AppError::Timeout)) {
        broken.store(true, Ordering::SeqCst);
    }
}

fn refs<'a>(params: &'a [ColumnData<'static>]) -> Vec<&'a dyn ToSql> {
    params.iter().map(|p| p as &dyn ToSql).collect()
}

/// Sends the Attention and drains its acknowledgement. The statement ends `Cancelled` either
/// way; a cancel that failed or took too long leaves the connection broken.
async fn cancel<S>(client: &mut Client<S>, broken: &AtomicBool) -> AppError
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    match tokio::time::timeout(CANCEL_LIMIT, client.cancel_query()).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            log::warn!("SQL Server cancel failed: {e}");
            broken.store(true, Ordering::SeqCst);
        }
        Err(_) => {
            log::warn!("SQL Server did not acknowledge the cancel");
            broken.store(true, Ordering::SeqCst);
        }
    }
    AppError::Cancelled
}

/// What `stream` runs.
#[derive(Clone, Copy)]
struct Statement<'a> {
    sql: &'a str,
    params: &'a [ColumnData<'static>],
    mode: Mode,
    bytes: Bytes,
}

async fn stream<S>(
    client: &mut Client<S>,
    st: &Statement<'_>,
    stop: &Stop,
    items: &mpsc::Sender<Item>,
    broken: &AtomicBool,
) -> Result<Summary, AppError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    if stop.is_fired() {
        return Err(AppError::Cancelled);
    }
    if st.mode == Mode::Batch && !st.params.is_empty() {
        return Err(AppError::Internal("a SQL batch takes no parameters".into()));
    }
    // The stream borrows the client until `forward` returns; only then can it cancel.
    match forward(client, st, stop, items).await? {
        Some(summary) => Ok(summary),
        None => Err(cancel(client, broken).await),
    }
}

/// Runs a statement and forwards its items until it ends (`Some`) or `stop` fires (`None`).
async fn forward<S>(
    client: &mut Client<S>,
    st: &Statement<'_>,
    stop: &Stop,
    items: &mpsc::Sender<Item>,
) -> Result<Option<Summary>, AppError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let Statement {
        sql,
        params,
        mode,
        bytes,
    } = *st;
    let refs = refs(params);
    let request = async {
        match mode {
            Mode::Batch => client.simple_query(sql).await,
            Mode::Rpc => client.query(sql, &refs).await,
        }
    };
    let started = tokio::select! {
        biased;
        _ = stop.fired() => None,
        r = request => Some(r),
    };
    let mut rows = match started {
        None => return Ok(None),
        Some(Err(e)) => return Err(map_error(e, Phase::Run, Some(sql))),
        Some(Ok(rows)) => rows,
    };
    let mut summary = Summary::default();
    let mut forwarding = true;
    loop {
        let next = tokio::select! {
            biased;
            _ = stop.fired() => None,
            item = rows.try_next() => Some(item),
        };
        let item = match next {
            None => return Ok(None),
            Some(Err(e)) => return Err(map_error(e, Phase::Run, Some(sql))),
            Some(Ok(None)) => break,
            Some(Ok(Some(item))) => item,
        };
        let out = match item {
            QueryItem::Metadata(meta) => {
                summary.sets += 1;
                Item::Columns(result_columns(meta.columns()))
            }
            QueryItem::Row(row) => {
                summary.rows += 1;
                if !forwarding {
                    continue;
                }
                Item::Row(decode_row(row, bytes))
            }
        };
        if forwarding {
            let sent = tokio::select! {
                biased;
                _ = stop.fired() => None,
                r = items.send(out) => Some(r.is_ok()),
            };
            match sent {
                None => return Ok(None),
                // The caller stopped listening: run to the end without forwarding.
                Some(false) => forwarding = false,
                Some(true) => {}
            }
        }
    }
    Ok(Some(summary))
}

async fn execute<S>(
    client: &mut Client<S>,
    sql: &str,
    params: &[ColumnData<'static>],
    stop: &Stop,
    broken: &AtomicBool,
) -> Result<Vec<u64>, AppError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    if stop.is_fired() {
        return Err(AppError::Cancelled);
    }
    let refs = refs(params);
    let done = {
        let request = client.execute(sql, &refs);
        tokio::select! {
            biased;
            _ = stop.fired() => None,
            r = request => Some(r),
        }
    };
    match done {
        None => Err(cancel(client, broken).await),
        Some(Ok(result)) => Ok(result.rows_affected().to_vec()),
        Some(Err(e)) => Err(map_error(e, Phase::Run, Some(sql))),
    }
}

/// One statement of a transaction: its result sets, the last of which is `@@ROWCOUNT`
/// (`tsql::counted`), which counts the statement's own rows where the DONE counts tiberius sums
/// would add those of triggers.
async fn counted<S>(
    client: &mut Client<S>,
    st: &TxRequest,
    stop: &Stop,
    broken: &AtomicBool,
) -> Result<TxDone, AppError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let sql = tsql::counted(&st.sql);
    let (items, mut items_rx) = mpsc::channel(CHANNEL_ROWS);
    let statement = Statement {
        sql: &sql,
        params: &st.params,
        mode: Mode::Rpc,
        bytes: Bytes::Whole,
    };
    let run = async {
        let r = stream(client, &statement, stop, &items, broken).await;
        drop(items);
        r
    };
    let (summary, mut sets) = tokio::join!(run, collect(&mut items_rx));
    summary?;
    let affected = sets
        .pop()
        .and_then(|set| set.rows.into_iter().next())
        .and_then(|row| super::introspect::int(&row, 0))
        .and_then(|n| u64::try_from(n).ok())
        .ok_or_else(|| AppError::Internal("SQL Server sent no row count".into()))?;
    let rows = if st.returning { sets.pop() } else { None };
    Ok(TxDone { affected, rows })
}

async fn transaction<S>(
    client: &mut Client<S>,
    statements: &[TxRequest],
    stop: &Stop,
    broken: &AtomicBool,
) -> Result<Vec<TxDone>, AppError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    if stop.is_fired() {
        return Err(AppError::Cancelled);
    }
    client
        .begin_transaction()
        .await
        .map_err(|e| map_error(e, Phase::Run, None))?;
    let mut out = Vec::with_capacity(statements.len());
    for (index, st) in statements.iter().enumerate() {
        let done = match counted(client, st, stop, broken).await {
            Ok(done) => done,
            Err(e) => {
                rollback(client, broken).await;
                return Err(e.with_statement_index(index));
            }
        };
        if let Some(n) = st.expect_affected
            && done.affected != n
        {
            rollback(client, broken).await;
            return Err(AppError::db(format!(
                "expected to affect {n} row(s) but matched {}; the row may have been changed or \
                 deleted by someone else",
                done.affected
            ))
            .with_statement_index(index));
        }
        out.push(done);
    }
    if let Err(e) = client.commit_transaction().await {
        let e = map_error(e, Phase::Run, None);
        rollback(client, broken).await;
        return Err(e);
    }
    Ok(out)
}

/// Rolls back after a failed statement. A connection that cannot even do that is broken, so it
/// never runs anything inside a transaction left half done.
async fn rollback<S>(client: &mut Client<S>, broken: &AtomicBool)
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    if broken.load(Ordering::SeqCst) {
        return;
    }
    let undone = tokio::time::timeout(CANCEL_LIMIT, async {
        // The server may have ended the transaction itself (a severe error, XACT_ABORT): its
        // refusal to roll back is no failure, and the batch after it makes sure.
        match client.rollback_transaction().await {
            Ok(()) | Err(::tiberius::error::Error::Server(_)) => {}
            Err(e) => return Err(e),
        }
        client
            .simple_query(tsql::ROLLBACK_IF_OPEN)
            .await?
            .into_results()
            .await?;
        Ok::<_, ::tiberius::error::Error>(())
    })
    .await;
    if !matches!(undone, Ok(Ok(()))) {
        broken.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::sqlserver::decode::to_param;
    use crate::drivers::sqlserver::testing::{
        self, Cell, DONE_COUNT, DONE_MORE, Reply, Request, Seen, Ty, done, failed, result,
    };

    async fn conn(handler: impl FnMut(&Request) -> Reply + Send + 'static) -> (Conn, Seen) {
        let (client, seen) = testing::client(handler).await.expect("a fake login");
        (Conn::spawn(client), seen)
    }

    /// Waits until the server got a request matching `f`.
    async fn until(seen: &Seen, f: impl Fn(&Request) -> bool) {
        for _ in 0..500 {
            if seen.requests().iter().any(&f) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the server never got the request: {:?}", seen.requests());
    }

    fn is_batch(r: &Request, sql: &str) -> bool {
        matches!(r, Request::Batch(s) if s == sql)
    }

    #[tokio::test]
    async fn streams_result_sets_with_bound_parameters() {
        let (conn, seen) = conn(|r: &Request| match r {
            Request::Rpc { .. } => {
                let mut tokens = result(
                    &[
                        ("id", Ty::BigInt),
                        ("name", Ty::NVarChar),
                        ("price", Ty::Money),
                    ],
                    &[
                        vec![Cell::Int(1 << 60), Cell::Text("a"), Cell::Money(123_456)],
                        vec![Cell::Int(2), Cell::Null, Cell::Null],
                    ],
                    true,
                );
                tokens.extend(result(&[("ok", Ty::Bit)], &[vec![Cell::Bit(true)]], false));
                Reply::Tokens(tokens)
            }
            _ => testing::ok(r),
        })
        .await;
        let params = vec![
            to_param(&Value::Int(7), None).unwrap(),
            to_param(&Value::Text("x".into()), None).unwrap(),
            to_param(&Value::Null, None).unwrap(),
        ];
        let sets = conn
            .query(
                "SELECT @P1, @P2, @P3",
                params,
                Mode::Rpc,
                Bytes::Preview,
                Stop::new(),
            )
            .await
            .unwrap();
        assert_eq!(sets.len(), 2);
        assert_eq!(sets[0].columns[2].data_type, "money");
        assert_eq!(
            sets[0].rows[0],
            vec![
                Value::BigInt((1i64 << 60).to_string()),
                Value::Text("a".into()),
                Value::Decimal("12.3456".into()),
            ]
        );
        assert_eq!(
            sets[0].rows[1],
            vec![Value::Int(2), Value::Null, Value::Null]
        );
        assert_eq!(sets[1].rows, vec![vec![Value::Bool(true)]]);
        assert!(seen.requests().contains(&Request::Rpc {
            sql: "SELECT @P1, @P2, @P3".into(),
            params: vec!["7".into(), "x".into(), "NULL".into()],
        }));
        // A batch takes no parameters.
        let batch = conn
            .query(
                "SELECT 1",
                vec![ColumnData::I32(Some(1))],
                Mode::Batch,
                Bytes::Preview,
                Stop::new(),
            )
            .await;
        assert!(batch.is_err());
    }

    #[tokio::test]
    async fn cancels_a_statement_that_has_not_answered() {
        const SLOW: &str = "WAITFOR DELAY '01:00'";
        let (conn, seen) = conn(|r: &Request| match r {
            Request::Batch(s) if s == SLOW => Reply::Hang(Vec::new()),
            Request::Batch(s) if s == "SELECT 2" => {
                Reply::Tokens(result(&[("n", Ty::Int)], &[vec![Cell::Int(2)]], false))
            }
            _ => testing::ok(r),
        })
        .await;
        let stop = Stop::new();
        let running = {
            let conn = conn.clone();
            let stop = stop.clone();
            tokio::spawn(async move {
                conn.query(SLOW, Vec::new(), Mode::Batch, Bytes::Preview, stop)
                    .await
            })
        };
        until(&seen, |r| is_batch(r, SLOW)).await;
        stop.fire();
        let outcome = tokio::time::timeout(Duration::from_secs(5), running)
            .await
            .expect("the cancel returns")
            .expect("the task ends");
        assert!(matches!(outcome, Err(AppError::Cancelled)), "{outcome:?}");
        assert_eq!(seen.attentions(), 1);
        // The connection is clean: the next statement runs; the spent stop stops nothing else.
        assert!(!conn.is_broken());
        let spent = conn
            .query("SELECT 2", Vec::new(), Mode::Batch, Bytes::Preview, stop)
            .await;
        assert!(matches!(spent, Err(AppError::Cancelled)), "{spent:?}");
        let sets = conn
            .query(
                "SELECT 2",
                Vec::new(),
                Mode::Batch,
                Bytes::Preview,
                Stop::new(),
            )
            .await
            .unwrap();
        assert_eq!(sets[0].rows, vec![vec![Value::Int(2)]]);
    }

    #[tokio::test]
    async fn cancels_between_rows() {
        let (conn, seen) = conn(|r: &Request| match r {
            Request::Batch(s) if s == "SELECT slowly" => {
                let mut tokens = testing::columns(&[("n", Ty::Int)]);
                tokens.extend(testing::row(&[Ty::Int], &[Cell::Int(1)]));
                tokens.extend(testing::row(&[Ty::Int], &[Cell::Int(2)]));
                Reply::Hang(tokens)
            }
            _ => testing::ok(r),
        })
        .await;
        let stop = Stop::new();
        let mut running = conn
            .stream(
                "SELECT slowly",
                Vec::new(),
                Mode::Batch,
                Bytes::Preview,
                stop.clone(),
            )
            .await
            .unwrap();
        assert!(matches!(running.items.recv().await, Some(Item::Columns(_))));
        assert_eq!(
            running.items.recv().await,
            Some(Item::Row(vec![Value::Int(1)]))
        );
        assert_eq!(
            running.items.recv().await,
            Some(Item::Row(vec![Value::Int(2)]))
        );
        stop.fire();
        let end = tokio::time::timeout(Duration::from_secs(5), running.finish())
            .await
            .expect("the cancel returns");
        assert!(matches!(end, Err(AppError::Cancelled)), "{end:?}");
        assert_eq!(seen.attentions(), 1);
        assert!(!conn.is_broken());
    }

    #[tokio::test]
    async fn a_caller_that_stops_listening_lets_the_statement_finish() {
        let rows: Vec<Vec<Cell<'static>>> = (0..3000).map(|i| vec![Cell::Int(i)]).collect();
        let tokens = result(&[("n", Ty::Int)], &rows, false);
        let (conn, seen) = conn(move |r: &Request| match r {
            Request::Batch(s) if s == "SELECT many" => Reply::Tokens(tokens.clone()),
            _ => testing::ok(r),
        })
        .await;
        let mut running = conn
            .stream(
                "SELECT many",
                Vec::new(),
                Mode::Batch,
                Bytes::Preview,
                Stop::new(),
            )
            .await
            .unwrap();
        assert!(matches!(running.items.recv().await, Some(Item::Columns(_))));
        let summary = running.finish().await.unwrap();
        assert_eq!(
            summary,
            Summary {
                sets: 1,
                rows: 3000
            }
        );
        assert_eq!(seen.attentions(), 0);
    }

    #[tokio::test]
    async fn server_errors_keep_their_number_and_line() {
        const SQL: &str = "SELECT 1\nSELECT * FROM nope";
        let (conn, _) = conn(|r: &Request| match r {
            Request::Batch(s) if s == SQL => {
                let mut tokens = result(&[("x", Ty::Int)], &[vec![Cell::Int(1)]], true);
                tokens.extend(failed(208, 2, "Invalid object name 'nope'."));
                Reply::Tokens(tokens)
            }
            _ => testing::ok(r),
        })
        .await;
        let err = conn
            .query(SQL, Vec::new(), Mode::Batch, Bytes::Preview, Stop::new())
            .await
            .unwrap_err();
        match err {
            AppError::Db {
                message,
                vendor_code,
                position,
                detail,
                ..
            } => {
                assert_eq!(message, "Invalid object name 'nope'.");
                assert_eq!(vendor_code, Some(208));
                assert_eq!(position.map(|p| p.line), Some(2));
                assert!(detail.is_some_and(|d| d.starts_with("Msg 208, Level 16")));
            }
            other => panic!("{other:?}"),
        }
        assert!(!conn.is_broken());
    }

    #[tokio::test]
    async fn execute_reports_the_counts() {
        let (conn, _) = conn(|r: &Request| match r {
            Request::Rpc { .. } => {
                let mut tokens = done(DONE_COUNT | DONE_MORE, 2);
                tokens.extend(done(DONE_COUNT, 3));
                Reply::Tokens(tokens)
            }
            _ => testing::ok(r),
        })
        .await;
        let counts = conn
            .execute(
                "DELETE FROM t WHERE a = @P1",
                vec![ColumnData::I32(Some(1))],
                Stop::new(),
            )
            .await
            .unwrap();
        assert_eq!(counts, [2, 3]);
    }

    fn rowcount(n: i64) -> Vec<u8> {
        let mut tokens = done(DONE_COUNT | DONE_MORE, n.unsigned_abs());
        tokens.extend(result(&[("", Ty::Int)], &[vec![Cell::Int(n)]], false));
        tokens
    }

    fn update() -> TxRequest {
        TxRequest {
            sql: "UPDATE t SET a = @P1 WHERE id = @P2".into(),
            params: vec![ColumnData::I32(Some(5)), ColumnData::I32(Some(1))],
            expect_affected: Some(1),
            returning: false,
        }
    }

    #[tokio::test]
    async fn a_transaction_commits_when_every_count_holds() {
        let (conn, seen) = conn(|r: &Request| match r {
            Request::Rpc { sql, .. } if sql.ends_with("SELECT @@ROWCOUNT") => {
                Reply::Tokens(rowcount(1))
            }
            _ => testing::ok(r),
        })
        .await;
        let done = conn
            .transaction(vec![update(), update()], Stop::new())
            .await
            .unwrap();
        assert_eq!(done.iter().map(|d| d.affected).collect::<Vec<_>>(), [1, 1]);
        let requests = seen.requests();
        assert!(requests.contains(&Request::Transaction(5)));
        assert_eq!(requests.last(), Some(&Request::Transaction(7)));
        assert!(requests.contains(&Request::Rpc {
            sql: "UPDATE t SET a = @P1 WHERE id = @P2;\nSELECT @@ROWCOUNT".into(),
            params: vec!["5".into(), "1".into()],
        }));
    }

    #[tokio::test]
    async fn a_transaction_rolls_back_on_a_count_that_does_not_hold() {
        let mut calls = 0;
        let (conn, seen) = conn(move |r: &Request| match r {
            Request::Rpc { .. } => {
                calls += 1;
                Reply::Tokens(rowcount(if calls == 1 { 1 } else { 0 }))
            }
            _ => testing::ok(r),
        })
        .await;
        let err = conn
            .transaction(vec![update(), update()], Stop::new())
            .await
            .unwrap_err();
        assert!(
            matches!(
                &err,
                AppError::Db { statement_index: Some(1), message, .. } if message.contains("matched 0")
            ),
            "{err:?}"
        );
        let requests = seen.requests();
        assert!(requests.contains(&Request::Transaction(8)));
        assert!(!requests.contains(&Request::Transaction(7)));
        assert_eq!(
            requests.last(),
            Some(&Request::Batch(tsql::ROLLBACK_IF_OPEN.into()))
        );
        assert!(!conn.is_broken());
    }

    #[tokio::test]
    async fn a_failed_statement_rolls_the_transaction_back() {
        let (conn, seen) = conn(|r: &Request| match r {
            Request::Rpc { .. } => Reply::Tokens(failed(
                547,
                1,
                "The UPDATE statement conflicted with the CHECK constraint.",
            )),
            _ => testing::ok(r),
        })
        .await;
        let err = conn
            .transaction(vec![update()], Stop::new())
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                AppError::Db {
                    vendor_code: Some(547),
                    statement_index: Some(0),
                    ..
                }
            ),
            "{err:?}"
        );
        assert!(seen.requests().contains(&Request::Transaction(8)));
    }

    #[tokio::test]
    async fn a_lost_connection_is_broken_for_good() {
        let (conn, _) = conn(|r: &Request| match r {
            Request::Batch(s) if s == "SELECT 1" => Reply::Close,
            _ => testing::ok(r),
        })
        .await;
        let first = conn
            .query(
                "SELECT 1",
                Vec::new(),
                Mode::Batch,
                Bytes::Preview,
                Stop::new(),
            )
            .await;
        assert!(matches!(first, Err(AppError::NotConnected)), "{first:?}");
        assert!(conn.is_broken());
        let again = conn.execute("SELECT 2", Vec::new(), Stop::new()).await;
        assert!(matches!(again, Err(AppError::NotConnected)));
        conn.close().await;
    }

    /// Polls until the fake server's session is over.
    async fn until_ended(seen: &Seen) {
        for _ in 0..500 {
            if seen.ended() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the client never hung up");
    }

    #[tokio::test(start_paused = true)]
    async fn a_broken_connection_lets_go_of_its_socket() {
        const STUCK: &str = "SELECT stuck";
        let (conn, seen) = conn(|r: &Request| match r {
            Request::Batch(s) if s == STUCK => Reply::Stuck(Vec::new()),
            _ => testing::ok(r),
        })
        .await;
        let stop = Stop::new();
        let running = {
            let conn = conn.clone();
            let stop = stop.clone();
            tokio::spawn(async move {
                conn.query(STUCK, Vec::new(), Mode::Batch, Bytes::Preview, stop)
                    .await
            })
        };
        until(&seen, |r| is_batch(r, STUCK)).await;
        stop.fire();
        // The Attention goes unanswered for CANCEL_LIMIT.
        let outcome = running.await.expect("the task ends");
        assert!(matches!(outcome, Err(AppError::Cancelled)), "{outcome:?}");
        assert!(conn.is_broken());
        assert_eq!(seen.attentions(), 1);
        // A handle is still alive, yet the socket is closed: the server can end the session.
        until_ended(&seen).await;
        let after = conn.execute("SELECT 1", Vec::new(), Stop::new()).await;
        assert!(matches!(after, Err(AppError::NotConnected)), "{after:?}");
        conn.close().await;
    }

    #[tokio::test(start_paused = true)]
    async fn close_gives_up_behind_a_full_queue() {
        const SLOW: &str = "WAITFOR DELAY '01:00'";
        let (conn, seen) = conn(|r: &Request| match r {
            Request::Batch(s) if s == SLOW => Reply::Hang(Vec::new()),
            _ => testing::ok(r),
        })
        .await;
        // One statement runs on the actor and never ends; the queue behind it fills up.
        for _ in 0..10 {
            let conn = conn.clone();
            tokio::spawn(async move {
                conn.query(SLOW, Vec::new(), Mode::Batch, Bytes::Preview, Stop::new())
                    .await
            });
        }
        until(&seen, |r| is_batch(r, SLOW)).await;
        tokio::time::timeout(Duration::from_secs(3), conn.close())
            .await
            .expect("close returns after its 2 s");
    }

    #[tokio::test]
    async fn close_ends_the_actor() {
        let (conn, _) = conn(testing::ok).await;
        tokio::time::timeout(Duration::from_secs(3), conn.close())
            .await
            .expect("close returns");
        assert!(conn.is_broken());
        let after = conn.execute("SELECT 1", Vec::new(), Stop::new()).await;
        assert!(matches!(after, Err(AppError::NotConnected)));
    }
}
