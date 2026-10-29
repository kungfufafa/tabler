//! The ClickHouse HTTP client: one request per statement, written against `Transport`.
//!
//! - `run` streams a statement's rows into a `RowSink` as they arrive and stops reading when the
//!   sink says so (the row cap): the connection closes and the server stops the read
//!   (`cancel_http_readonly_queries_on_client_close`).
//! - Every request gets a query id, so `kill` can stop it from another request
//!   (`KILL QUERY … ASYNC`); a user may always stop their own queries.
//! - A user whose profile is `readonly=1` may change no setting: the first request that tries
//!   is refused before it runs, and the client sends no settings from then on (the server's
//!   `readonly=1` is stricter than the `readonly=2` it replaces). A constraint on an output
//!   setting only drops the output settings. Neither is taken from the refusal alone, which a
//!   statement's own SETTINGS clause provokes too: probes decide (`open`).
//! - Every request that may carry settings pins `dialect=clickhouse` and direct reads of stream
//!   tables off (`request::PINNED_SETTINGS`), so a `SET` in the editor's session cannot make the
//!   server read another language than Safe Mode does.
//! - `apply` runs one staged edit with its guard: an edit whose count the server does not report
//!   (a mutation, a lightweight delete or update) is preceded by a count of the rows its key
//!   matches, and refused unless that is the expected count.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt as _;

use crate::drivers::{CollectSink, RowChunker, RowSink, TxStatement, check_affected};
use crate::error::AppError;
use crate::sqlgen::dialect::clickhouse::{
    EditKind, KILL_QUERY, edit_kind, guard_count, inserts_one_row,
};
use crate::types::query::{ExecOutcome, ResultSet};
use crate::types::value::Value;

use super::catalog::{self, ClickhouseServerInfo};
use super::format::{Event, ResultReader, Summary, settings_refusal, status_error};
use super::request::{
    self, ConnSettings, Credentials, Endpoint, OUTPUT_FORMAT, OUTPUT_SETTINGS, PINNED_SETTINGS,
    Query,
};
use super::transport::{StreamingResponse, Transport, header};

/// Settings an uncounted edit runs with: it returns once the mutation is done on every replica.
const EDIT_SETTINGS: &[(&str, &str)] = &[("mutations_sync", "2")];

/// Sent with a refused request's settings and none of its statement: whether the settings alone
/// are refused.
pub const SETTINGS_PROBE: &str = "SELECT 1 AS settings_probe";
/// Sent without any setting: the `readonly` level of the user's profile.
pub const READONLY_PROBE: &str = "SELECT toUInt8(getSetting('readonly')) AS readonly";

fn is_output_setting(name: &str) -> bool {
    OUTPUT_SETTINGS
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case(name))
}

pub struct Client {
    transport: Arc<dyn Transport>,
    pub endpoint: Endpoint,
    creds: Credentials,
    conn: ConnSettings,
    /// Cleared when a constraint forbids one of the output settings themselves.
    output_settings: AtomicBool,
    /// Set when a probe without any setting found the user's profile `readonly=1`: no setting
    /// at all may be sent, and the server refuses writes on every request by itself.
    settings_locked: AtomicBool,
}

/// What a run reports besides the rows it streamed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutcome {
    pub query_id: String,
    /// The statement announced a result (a SELECT that found nothing too); a statement without
    /// output sends an empty body.
    pub result_set: bool,
    pub rows: u64,
    /// The sink stopped reading before the end (the row cap).
    pub stopped: bool,
    pub summary: Option<Summary>,
    /// `X-ClickHouse-Timezone`: the session's time zone, which DateTime values without their
    /// own are shown in.
    pub timezone: Option<String>,
}

pub fn new_query_id() -> String {
    format!("tabler-{}", uuid::Uuid::new_v4())
}

impl Client {
    pub fn new(
        transport: Arc<dyn Transport>,
        endpoint: Endpoint,
        creds: Credentials,
        conn: ConnSettings,
    ) -> Client {
        let output = conn.output_settings;
        Client {
            transport,
            endpoint,
            creds,
            conn,
            output_settings: AtomicBool::new(output),
            settings_locked: AtomicBool::new(false),
        }
    }

    /// Whether the session is read-only (by `readonly=2`, or by the user's own `readonly=1`).
    pub fn read_only(&self) -> bool {
        self.conn.read_only || self.settings_locked.load(Ordering::Relaxed)
    }

    /// Whether every request carries a read-only restriction the server enforces: `readonly=2`,
    /// or the profile's `readonly=1` once a probe found it (`open`).
    pub fn hardened(&self) -> bool {
        self.conn.read_only
    }

    fn settings_now(&self) -> ConnSettings {
        let locked = self.settings_locked.load(Ordering::Relaxed);
        ConnSettings {
            read_only: self.conn.read_only && !locked,
            output_settings: self.output_settings.load(Ordering::Relaxed) && !locked,
            ..self.conn.clone()
        }
    }

    /// Sends `q` and returns the reply once its status is 200; any other status is the error it
    /// carries.
    ///
    /// A refused setting is no proof of whose setting it was: the statement's own SETTINGS
    /// clause (`SELECT 1 SETTINGS readonly = 0` under `readonly=2`) is refused the same way as
    /// the driver's. So before anything is dropped, a probe with the driver's settings and no
    /// statement of the user's must be refused too (the request then never reached its
    /// statement, so sending it again runs it once). Only then: `readonly=2` and every other
    /// setting go only when a probe without any setting finds the user's profile `readonly=1`,
    /// which the server enforces on every request; the output settings go when a constraint
    /// names one of them.
    pub async fn open(&self, q: &Query<'_>) -> Result<StreamingResponse, AppError> {
        // At most two retries: one without the output settings, one without any.
        for _ in 0..3 {
            let locked = self.settings_locked.load(Ordering::Relaxed);
            let conn = self.settings_now();
            // The pinned settings go with the statement's, and like them not once the profile
            // refuses every setting (then no `SET` can change them either).
            let settings: Vec<(&str, &str)> = if locked {
                Vec::new()
            } else {
                PINNED_SETTINGS.iter().chain(q.settings).copied().collect()
            };
            let q = Query {
                settings: &settings,
                ..q.clone()
            };
            let req = request::build(&self.endpoint, &self.creds, &conn, &q)?;
            let resp = self.transport.send(req).await?;
            if resp.head.status == 200 {
                return Ok(resp);
            }
            let (head, body) = resp.collect().await?;
            let err = status_error(&head, &body);
            if locked || settings_refusal(&err).is_none() {
                return Err(err);
            }
            // Refused for the driver's settings alone?
            let probe = Query {
                sql: SETTINGS_PROBE,
                settings: q.settings,
                session: q.session,
                ..Query::default()
            };
            let refusal = match self.probe(&conn, &probe).await {
                Err(e) => settings_refusal(&e),
                Ok(_) => None,
            };
            let Some(refusal) = refusal else {
                return Err(err);
            };
            if refusal.readonly_mode {
                if self.profile_readonly(&conn).await != Some(1) {
                    return Err(err);
                }
                self.settings_locked.store(true, Ordering::Relaxed);
            } else if conn.output_settings
                && refusal.setting.as_deref().is_some_and(is_output_setting)
            {
                self.output_settings.store(false, Ordering::Relaxed);
            } else {
                return Err(err);
            }
        }
        Err(AppError::db(
            "ClickHouse kept refusing the connection's settings",
        ))
    }

    /// The rows of a small request sent as it is (no retry).
    async fn probe(&self, conn: &ConnSettings, q: &Query<'_>) -> Result<Vec<Vec<Value>>, AppError> {
        let req = request::build(&self.endpoint, &self.creds, conn, q)?;
        let (head, body) = self.transport.send(req).await?.collect().await?;
        if head.status != 200 {
            return Err(status_error(&head, &body));
        }
        let mut reader = ResultReader::new(&head);
        let mut events = Vec::new();
        reader.feed(&body, &mut events)?;
        reader.finish(&mut events)?;
        Ok(events
            .into_iter()
            .filter_map(|e| match e {
                Event::Row(row) => Some(row),
                Event::Columns(_) => None,
            })
            .collect())
    }

    /// The `readonly` level of the user's profile: asked without any setting, outside any
    /// session, so nothing but the profile (and the roles) decides it.
    async fn profile_readonly(&self, conn: &ConnSettings) -> Option<i64> {
        let bare = ConnSettings {
            read_only: false,
            output_settings: false,
            settings: Vec::new(),
            ..conn.clone()
        };
        let rows = self
            .probe(
                &bare,
                &Query {
                    sql: READONLY_PROBE,
                    ..Query::default()
                },
            )
            .await
            .ok()?;
        match rows.first()?.first()? {
            Value::Int(n) => Some(*n),
            Value::Text(s) | Value::BigInt(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    /// Runs one statement, streaming its rows into `sink`.
    pub async fn run(&self, q: &Query<'_>, sink: &mut dyn RowSink) -> Result<RunOutcome, AppError> {
        let query_id = q.query_id.map_or_else(new_query_id, str::to_string);
        let q = Query {
            query_id: Some(&query_id),
            ..q.clone()
        };
        let resp = self.open(&q).await?;
        let head = resp.head;
        let mut outcome = RunOutcome {
            query_id: query_id.clone(),
            summary: Summary::from_head(&head),
            timezone: head.header(header::TIMEZONE).map(str::to_string),
            ..RunOutcome::default()
        };
        // Before 26.8 a FORMAT clause in the statement beats the one asked for.
        if let Some(format) = head.header(header::FORMAT)
            && !format.eq_ignore_ascii_case(OUTPUT_FORMAT)
        {
            return Err(AppError::db(format!(
                "the statement asks for the {format} format, which the result grid cannot \
                 show; remove its FORMAT clause"
            )));
        }
        let mut reader = ResultReader::new(&head);
        let mut chunker = RowChunker::new(sink);
        let mut events = Vec::new();
        let mut body = resp.body;
        let mut failed = None;
        while let Some(piece) = body.next().await {
            if let Err(e) = piece.and_then(|p| reader.feed(&p, &mut events)) {
                failed = Some(e);
            }
            // The rows before an error still reach the sink.
            if drain(&mut chunker, &mut outcome, &mut events) {
                outcome.stopped = true;
                break;
            }
            if failed.is_some() {
                break;
            }
        }
        // Closing the connection stops a read the sink no longer wants.
        drop(body);
        if failed.is_none() && !outcome.stopped {
            match reader.finish(&mut events) {
                Ok(announced) => outcome.result_set |= announced,
                Err(e) => failed = Some(e),
            }
            outcome.stopped = drain(&mut chunker, &mut outcome, &mut events);
        }
        let _ = chunker.flush();
        match failed {
            Some(e) => Err(e),
            None => Ok(outcome),
        }
    }

    /// The rows of a statement (the catalog's, a count).
    pub async fn rows(&self, q: &Query<'_>) -> Result<Vec<Vec<Value>>, AppError> {
        Ok(self
            .result(q)
            .await?
            .0
            .map(|set| set.rows)
            .unwrap_or_default())
    }

    async fn result(&self, q: &Query<'_>) -> Result<(Option<ResultSet>, RunOutcome), AppError> {
        let mut sink = CollectSink::default();
        let outcome = self.run(q, &mut sink).await?;
        Ok((sink.sets.pop(), outcome))
    }

    /// A catalog query with its named parameters (`{db:String}`).
    pub async fn catalog(
        &self,
        sql: &str,
        named: &[(&str, &str)],
    ) -> Result<Vec<Vec<Value>>, AppError> {
        self.rows(&Query {
            sql,
            named,
            ..Query::default()
        })
        .await
    }

    /// Runs a statement for its effect. The count is the summary's `written_rows` (an insert);
    /// a statement whose count the server does not report is `affected_unknown`.
    pub async fn execute(
        &self,
        sql: &str,
        params: &[Value],
        settings: &[(&str, &str)],
    ) -> Result<ExecOutcome, AppError> {
        let (set, outcome) = self
            .result(&Query {
                sql,
                params,
                settings,
                ..Query::default()
            })
            .await?;
        let kind = edit_kind(sql);
        let written = outcome.summary.map(|s| s.written_rows);
        Ok(ExecOutcome {
            affected: written.unwrap_or(0),
            last_insert_id: None,
            rows: set,
            affected_unknown: kind.uncounted() || (kind == EditKind::Insert && written.is_none()),
        })
    }

    /// Applies one staged edit (`index` its place in the commit) so that it changes exactly the
    /// rows it expects, or nothing. ClickHouse has no transactions: the guard of an uncounted
    /// edit is a count right before it, so a row added between the two is not seen.
    pub async fn apply(&self, st: &TxStatement, index: usize) -> Result<ExecOutcome, AppError> {
        let kind = edit_kind(&st.sql);
        let at = |e: AppError| e.with_statement_index(index);
        match (kind.uncounted(), st.expect_affected) {
            (true, Some(expected)) => {
                let count_sql = guard_count(&st.sql).ok_or_else(|| {
                    at(AppError::Unsupported(
                        "ClickHouse cannot confirm how many rows this statement changes".into(),
                    ))
                })?;
                let rows = self
                    .rows(&Query {
                        sql: &count_sql,
                        params: &st.params,
                        ..Query::default()
                    })
                    .await
                    .map_err(at)?;
                let found = catalog::parse_count(&rows).map_err(at)?;
                if found != expected {
                    return Err(at(AppError::db(format!(
                        "the row's key matches {found} row(s), not {expected}: ClickHouse keys \
                         are not unique, so the change was not applied"
                    ))));
                }
                let mut out = self
                    .execute(&st.sql, &st.params, EDIT_SETTINGS)
                    .await
                    .map_err(at)?;
                out.affected = found;
                out.affected_unknown = false;
                Ok(out)
            }
            _ => {
                let settings = if kind.uncounted() { EDIT_SETTINGS } else { &[] };
                let mut out = self
                    .execute(&st.sql, &st.params, settings)
                    .await
                    .map_err(at)?;
                // The summary also counts what materialized views wrote: an insert that can only
                // write one row wrote one when it wrote any.
                if kind == EditKind::Insert && inserts_one_row(&st.sql) && out.affected > 1 {
                    out.affected = 1;
                }
                check_affected(st, &out, index)?;
                Ok(out)
            }
        }
    }

    /// Stops the query `query_id` (sent beside it, outside any session, which the running query
    /// holds).
    pub async fn kill(&self, query_id: &str) -> Result<(), AppError> {
        self.rows(&Query {
            sql: KILL_QUERY,
            named: &[("id", query_id)],
            ..Query::default()
        })
        .await
        .map(|_| ())
    }

    /// `GET /ping`: free, needs no credentials.
    pub async fn ping(&self) -> Result<(), AppError> {
        let resp = self.transport.send(request::ping(&self.endpoint)).await?;
        let (head, body) = resp.collect().await?;
        if head.status == 200 && body.starts_with(b"Ok.") {
            Ok(())
        } else {
            Err(status_error(&head, &body))
        }
    }

    pub async fn server_info(&self) -> Result<ClickhouseServerInfo, AppError> {
        catalog::parse_server_info(&self.catalog(catalog::SERVER_INFO, &[]).await?)
    }
}

/// Hands the decoded events to the sink; true when it wants no more.
fn drain(chunker: &mut RowChunker<'_>, outcome: &mut RunOutcome, events: &mut Vec<Event>) -> bool {
    events
        .drain(..)
        .any(|e| take(chunker, outcome, e).is_break())
}

fn take(chunker: &mut RowChunker<'_>, outcome: &mut RunOutcome, e: Event) -> ControlFlow<()> {
    match e {
        Event::Columns(columns) => {
            outcome.result_set = true;
            chunker.begin_set(columns)
        }
        Event::Row(row) => {
            outcome.rows += 1;
            chunker.push(row)
        }
    }
}
