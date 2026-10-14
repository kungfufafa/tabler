//! Metrics Board (TablePlus's dashboards, spec §8.4): the boards kept in the app database, the
//! queries their objects run, and `.tablerboard` files (Import / Export Board…).
//!
//! An object's query is read-only unless the object allows data changes: a statement that writes
//! (or a SELECT that calls a function with side effects, see
//! `classify::calls_side_effect_function`) is refused here as well as in the renderer. Results
//! keep at most [`MAX_ROWS`] rows, and a statement, read or write, that runs longer than the
//! object's query timeout is cancelled ([`run_with_timeout`]).
//!
//! Pure apart from the app database and the `DbSession` it is handed: no Tauri (see
//! `commands::metrics`).

use std::collections::HashSet;
use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use aws_lc_rs::{aead, pbkdf2, rand};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::appdb::AppDb;
use crate::drivers::{CancelSlot, DbSession, RowSink, request_cancel};
use crate::error::AppError;
use crate::sqlgen::quote::MysqlLiteralMode;
use crate::sqlgen::split::{is_single_statement, split_statements};
use crate::sqlgen::{Dialect, classify};
use crate::types::metrics::{MetricsBoard, MetricsBoardInput, MetricsQuery, MetricsResult};
use crate::types::query::ResultColumn;
use crate::types::value::Value;

/// Rows a board object keeps from its query.
pub const MAX_ROWS: usize = 5_000;
/// Longest board name accepted.
const NAME_MAX: usize = 200;
/// Largest board document accepted (bytes of JSON).
const OBJECTS_MAX_BYTES: usize = 4 * 1024 * 1024;

// ---- object queries -------------------------------------------------------------------------

/// Puts the dialect's parameters where the variables were (`MetricsQuery::parts`): MySQL `?` and
/// SQLite `?n` bind the values; PostgreSQL gets quoted literals instead, so the server types them
/// from their context (`id = '42'` against an integer, a uuid or an enum column alike) and the
/// statement runs over the text protocol, which decodes every type (the extended protocol's
/// binary rows cannot show `numeric`). Each one is padded with spaces so it never runs into the
/// SQL around it (`{{a}}0` must not read as `?10`, nor two literals as one `'a''b'`).
pub fn bind_template(
    dialect: Dialect,
    parts: &[String],
    values: &[Value],
) -> Result<(String, Vec<Value>), AppError> {
    if parts.len() != values.len() + 1 {
        return Err(AppError::InvalidInput(
            "the query's variables do not match their values".into(),
        ));
    }
    let mut sql = String::new();
    let mut params = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        sql.push_str(part);
        let Some(value) = values.get(i) else {
            break;
        };
        let filled = match dialect {
            Dialect::Postgres => pg_literal(value)?,
            Dialect::Mysql | Dialect::Sqlite => {
                params.push(value.clone());
                dialect.placeholder(params.len())
            }
        };
        sql.push(' ');
        sql.push_str(&filled);
        sql.push(' ');
    }
    Ok((sql, params))
}

/// A variable's value as a PostgreSQL literal of unknown type (`NULL` for no value).
fn pg_literal(value: &Value) -> Result<String, AppError> {
    match value {
        Value::Null | Value::Default => Ok("NULL".into()),
        Value::Bytes(_) => Err(AppError::InvalidInput(
            "a binary value cannot fill a variable".into(),
        )),
        other => Ok(Dialect::Postgres.quote_literal_with(
            &other.as_text().unwrap_or_default(),
            MysqlLiteralMode::default(),
        )),
    }
}

/// An object query ready to run.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub sql: String,
    pub params: Vec<Value>,
    /// The statement may change data (`allow_writes` was set).
    pub writes: bool,
}

/// The message of a refused write.
pub const WRITE_REFUSED: &str =
    "This query changes data. Turn on \"Allow data changes\" in the object's settings to run it.";

/// The message of a refused SELECT that calls a function with side effects (`setval`,
/// `pg_terminate_backend`, `GET_LOCK`, `SLEEP`…).
pub const SIDE_EFFECT_REFUSED: &str = "This query calls a function that changes data or holds a lock. Turn on 'Allow data changes' in the object's settings to run it.";

/// Binds the variables and checks the statement: exactly one, and a READ unless the object
/// allows data changes. A SELECT counts as a write for the object when it calls a function that
/// changes the server (`classify::calls_side_effect_function`): the statement-level classifier
/// (Safe Mode) calls it a read, and a board runs reads on open and on every refresh. `writes` of
/// the result stays what `classify::writes` says: it decides how the statement runs.
pub fn prepare(dialect: Dialect, query: &MetricsQuery) -> Result<Prepared, AppError> {
    let (sql, params) = bind_template(dialect, &query.parts, &query.values)?;
    let statements = split_statements(&sql, dialect);
    let statement = match statements.as_slice() {
        [] => return Err(AppError::InvalidInput("The query is empty".into())),
        [one] if is_single_statement(&one.sql, dialect) => one.sql.clone(),
        _ => {
            return Err(AppError::InvalidInput(
                "A board object runs one statement. Remove the others.".into(),
            ));
        }
    };
    let writes = classify::writes(&statement, dialect);
    if writes && !query.allow_writes {
        return Err(AppError::InvalidInput(WRITE_REFUSED.into()));
    }
    if !query.allow_writes && classify::calls_side_effect_function(&statement, dialect) {
        return Err(AppError::InvalidInput(SIDE_EFFECT_REFUSED.into()));
    }
    Ok(Prepared {
        sql: statement,
        params,
        writes,
    })
}

/// Keeps the first result set's columns and at most `max` of its rows; asks the driver to stop
/// past them.
struct CappedSink {
    columns: Option<Vec<ResultColumn>>,
    rows: Vec<Vec<Value>>,
    max: usize,
    truncated: bool,
    sets: usize,
}

impl CappedSink {
    fn new(max: usize) -> CappedSink {
        CappedSink {
            columns: None,
            rows: Vec::new(),
            max,
            truncated: false,
            sets: 0,
        }
    }
}

impl RowSink for CappedSink {
    fn begin_set(&mut self, columns: Vec<ResultColumn>) {
        self.sets += 1;
        if self.columns.is_none() {
            self.columns = Some(columns);
        }
    }

    fn rows(&mut self, rows: Vec<Vec<Value>>) -> ControlFlow<()> {
        if self.sets > 1 {
            return ControlFlow::Break(());
        }
        let room = self.max.saturating_sub(self.rows.len());
        if rows.len() > room {
            self.rows.extend(rows.into_iter().take(room));
            self.truncated = true;
            return ControlFlow::Break(());
        }
        self.rows.extend(rows);
        ControlFlow::Continue(())
    }
}

/// The row cap of a request: `max_rows`, at most [`MAX_ROWS`] (0 = the cap).
pub fn row_cap(max_rows: u32) -> usize {
    match max_rows as usize {
        0 => MAX_ROWS,
        n => n.min(MAX_ROWS),
    }
}

/// Runs a prepared object query on a worker connection: reads stream (stopped past the cap and
/// cancellable through `cancel`), writes run for their affected-row count (and rows, if any) and
/// are cancellable through `cancel` as well (`DbSession::execute_cancellable`).
pub async fn run(
    session: &dyn DbSession,
    prepared: &Prepared,
    max_rows: usize,
    cancel: CancelSlot,
) -> Result<MetricsResult, AppError> {
    let started = Instant::now();
    if prepared.writes {
        let outcome = session
            .execute_cancellable(&prepared.sql, &prepared.params, cancel)
            .await?;
        let (columns, mut rows) = outcome
            .rows
            .map(|set| (set.columns, set.rows))
            .unwrap_or_default();
        let truncated = rows.len() > max_rows;
        rows.truncate(max_rows);
        return Ok(MetricsResult {
            columns,
            rows,
            truncated,
            affected: Some(outcome.affected),
            duration_ms: started.elapsed().as_millis() as u64,
            sql: prepared.sql.clone(),
        });
    }
    let mut sink = CappedSink::new(max_rows);
    session
        .stream_query(&prepared.sql, &prepared.params, &mut sink, cancel)
        .await?;
    Ok(MetricsResult {
        columns: sink.columns.unwrap_or_default(),
        rows: sink.rows,
        truncated: sink.truncated,
        affected: None,
        duration_ms: started.elapsed().as_millis() as u64,
        sql: prepared.sql.clone(),
    })
}

/// An object's "Query timeout" when it sets none, and the bounds it is kept within (seconds).
pub const DEFAULT_TIMEOUT_SECS: u32 = 60;
pub const MIN_TIMEOUT_SECS: u32 = 5;
pub const MAX_TIMEOUT_SECS: u32 = 600;

/// The time limit of a query: `requested` seconds (absent = [`DEFAULT_TIMEOUT_SECS`]), kept
/// between [`MIN_TIMEOUT_SECS`] and [`MAX_TIMEOUT_SECS`]. Every object query has one.
pub fn query_timeout(requested: Option<u32>) -> Duration {
    let secs = requested
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS);
    Duration::from_secs(secs.into())
}

/// How often a cancel that has not ended the statement yet is sent again (see
/// [`run_with_timeout`]).
const CANCEL_RETRY: Duration = Duration::from_secs(5);

/// What the object shows when its query hit the time limit. `writes`: the statement was a write.
pub fn timeout_error(limit: Duration, writes: bool) -> AppError {
    let rollback = if writes {
        " A write is rolled back by the server unless its table cannot be (MyISAM)."
    } else {
        ""
    };
    AppError::Db {
        message: format!("Query timed out after {} s", limit.as_secs()),
        detail: None,
        hint: Some(format!(
            "The statement was cancelled.{rollback} Raise the object's Query timeout in its \
             settings (5 to 600 s) or make the query cheaper."
        )),
        sqlstate: None,
        vendor_code: None,
        position: None,
        statement_index: None,
    }
}

/// [`run`] under a time limit. A statement still running when `limit` passes is cancelled the way
/// `cancel_query` cancels it (through `cancel`, the slot the command registered) and fails with
/// [`timeout_error`]; its end is awaited, so the worker connection is back in the pool when this
/// returns. This holds for writes too: a write runs through `DbSession::execute_cancellable`, and
/// the server rolls back a cancelled statement.
///
/// A cancel can be lost (SQLite drops an `interrupt()` that arrives before its statement's first
/// step, PostgreSQL ignores a cancel that reaches an idle backend), so one that has not ended the
/// statement is sent again every [`CANCEL_RETRY`].
///
/// What comes back once the limit has passed:
/// - a read counts as the timeout whatever it returns, also an answer: MySQL's `SLEEP()` returns 1
///   instead of failing when its statement is killed;
/// - a write that fails with `Cancelled` is the timeout; a write that finished before the cancel
///   reached it has happened, so its own outcome is returned rather than a "timed out" that would
///   hide the change.
pub async fn run_with_timeout(
    session: &dyn DbSession,
    prepared: &Prepared,
    max_rows: usize,
    cancel: CancelSlot,
    limit: Duration,
) -> Result<MetricsResult, AppError> {
    run_with_timeout_retrying(session, prepared, max_rows, cancel, limit, CANCEL_RETRY).await
}

/// [`run_with_timeout`] with the interval of the repeated cancel given (tests shorten it).
async fn run_with_timeout_retrying(
    session: &dyn DbSession,
    prepared: &Prepared,
    max_rows: usize,
    cancel: CancelSlot,
    limit: Duration,
    retry: Duration,
) -> Result<MetricsResult, AppError> {
    let work = run(session, prepared, max_rows, cancel.clone());
    tokio::pin!(work);
    tokio::select! {
        result = &mut work => return result,
        () = tokio::time::sleep(limit) => {}
    }
    let cancelling = async {
        loop {
            if let Some(handle) = request_cancel(&cancel)
                && let Err(e) = session.cancel(handle).await
            {
                log::warn!("metrics: cancelling a query that timed out failed: {e}");
            }
            tokio::time::sleep(retry).await;
        }
    };
    let result = tokio::select! {
        result = &mut work => result,
        _ = cancelling => unreachable!("the cancel loop does not end"),
    };
    after_the_limit(result, prepared.writes, limit)
}

/// What a statement that was still running when its time limit passed, and was cancelled then,
/// comes back as (see [`run_with_timeout`]).
fn after_the_limit(
    result: Result<MetricsResult, AppError>,
    writes: bool,
    limit: Duration,
) -> Result<MetricsResult, AppError> {
    match result {
        Err(AppError::Cancelled) => Err(timeout_error(limit, writes)),
        // A read that answers anyway has been cut short (MySQL's killed `SLEEP()`).
        Ok(_) if !writes => Err(timeout_error(limit, false)),
        // A write that finished before the cancel reached it has happened: report it.
        other => other,
    }
}

// ---- boards in the app database -------------------------------------------------------------

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// A trimmed, non-empty name of at most `NAME_MAX` characters.
fn board_name(name: &str) -> Result<String, AppError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::InvalidInput("The board needs a name".into()));
    }
    if name.chars().count() > NAME_MAX {
        return Err(AppError::InvalidInput(format!(
            "The board name is longer than {NAME_MAX} characters"
        )));
    }
    Ok(name.to_string())
}

/// The board document as stored: a JSON array of objects.
fn objects_json(objects: &serde_json::Value) -> Result<String, AppError> {
    if !objects.is_array() {
        return Err(AppError::InvalidInput(
            "The board's objects must be a list".into(),
        ));
    }
    let text = serde_json::to_string(objects)?;
    if text.len() > OBJECTS_MAX_BYTES {
        return Err(AppError::InvalidInput("The board is too large".into()));
    }
    Ok(text)
}

const BOARD_COLUMNS: &str = "id, profile_id, name, objects, created_at, updated_at";

fn board_row(r: &rusqlite::Row<'_>) -> Result<MetricsBoard, rusqlite::Error> {
    let id: String = r.get(0)?;
    let profile: Option<String> = r.get(1)?;
    let objects: String = r.get(3)?;
    Ok(MetricsBoard {
        id: Uuid::parse_str(&id).unwrap_or_default(),
        profile_id: profile.and_then(|p| Uuid::parse_str(&p).ok()),
        name: r.get(2)?,
        objects: serde_json::from_str(&objects)
            .unwrap_or_else(|_| serde_json::Value::Array(Vec::new())),
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

/// The boards of a connection profile and the boards of every connection, in the board list's
/// order. `None`: every board.
pub fn list_boards(db: &AppDb, profile_id: Option<Uuid>) -> Result<Vec<MetricsBoard>, AppError> {
    db.with_conn(|conn| {
        let sql = format!(
            "SELECT {BOARD_COLUMNS} FROM metrics_boards
             WHERE ?1 IS NULL OR profile_id IS NULL OR profile_id = ?1
             ORDER BY sort_order, name COLLATE NOCASE, created_at"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![profile_id.map(|p| p.to_string())], board_row)?;
        rows.collect()
    })
}

pub fn get_board(db: &AppDb, id: Uuid) -> Result<Option<MetricsBoard>, AppError> {
    db.with_conn(|conn| {
        conn.query_row(
            &format!("SELECT {BOARD_COLUMNS} FROM metrics_boards WHERE id = ?1"),
            params![id.to_string()],
            board_row,
        )
        .optional()
    })
}

/// Inserts a board, or replaces the one with the same id (`name` and `objects` already checked).
fn upsert_board(
    conn: &rusqlite::Connection,
    id: Uuid,
    profile_id: Option<Uuid>,
    name: &str,
    objects: &str,
    now: &str,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO metrics_boards
             (id, profile_id, name, objects, sort_order, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4,
                 (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM metrics_boards), ?5, ?5)
         ON CONFLICT(id) DO UPDATE SET
             profile_id = excluded.profile_id,
             name = excluded.name,
             objects = excluded.objects,
             updated_at = excluded.updated_at",
        params![
            id.to_string(),
            profile_id.map(|p| p.to_string()),
            name,
            objects,
            now
        ],
    )?;
    Ok(())
}

/// Creates or replaces a board (its creation time and place in the list stay).
pub fn save_board(db: &AppDb, input: &MetricsBoardInput) -> Result<MetricsBoard, AppError> {
    let name = board_name(&input.name)?;
    let objects = objects_json(&input.objects)?;
    let id = input.id.unwrap_or_else(Uuid::new_v4);
    let now = now_rfc3339();
    db.with_conn(|conn| upsert_board(conn, id, input.profile_id, &name, &objects, &now))?;
    get_board(db, id)?.ok_or_else(|| AppError::NotFound("board".into()))
}

/// Deletes a board; false when there was none.
pub fn delete_board(db: &AppDb, id: Uuid) -> Result<bool, AppError> {
    db.with_conn(|conn| {
        conn.execute(
            "DELETE FROM metrics_boards WHERE id = ?1",
            params![id.to_string()],
        )
    })
    .map(|n| n > 0)
}

/// `name`, or `name 2`, `name 3`… when it is taken (ignoring case).
pub fn unique_name(name: &str, taken: &HashSet<String>) -> String {
    let free = |n: &str| !taken.contains(&n.to_lowercase());
    if free(name) {
        return name.to_string();
    }
    (2..)
        .map(|i| format!("{name} {i}"))
        .find(|n| free(n))
        .unwrap_or_else(|| name.to_string())
}

// ---- `.tablerboard` files ---------------------------------------------------------------------

/// `format` of every board file.
pub const FORMAT: &str = "tabler-metrics-board";
/// File layout version written by this build (files from newer builds are refused).
pub const VERSION: u32 = 1;
/// The file name extension (without the dot).
pub const FILE_EXTENSION: &str = "tablerboard";

/// PBKDF2-HMAC-SHA256 rounds for new files (as `.tablerconnections`).
pub const PBKDF2_ITERATIONS: u32 = 600_000;
const MIN_ITERATIONS: u32 = 1_000;
const MAX_ITERATIONS: u32 = 10_000_000;
const SALT_LEN: usize = 16;
const KEY_LEN: usize = 32;
const KDF: &str = "pbkdf2-hmac-sha256";
const CIPHER: &str = "aes-256-gcm";
/// Binds the ciphertext to this file kind (a sealed connections file does not open as a board).
const AAD: &[u8] = b"tabler-metrics-board/1";

/// One board in a file: what it is called and its objects (no ids, no connection).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardRecord {
    pub name: String,
    pub objects: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    format: String,
    version: u32,
    #[serde(default)]
    exported_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    boards: Option<Vec<BoardRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    encrypted: Option<Sealed>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Sealed {
    kdf: String,
    iterations: u32,
    salt: String,
    cipher: String,
    nonce: String,
    data: String,
}

/// Outcome of reading a board file.
#[derive(Debug, Clone, PartialEq)]
pub enum ReadOutcome {
    /// The file is encrypted and no password was given.
    NeedsPassword,
    Opened(Vec<BoardRecord>),
}

fn derive_key(password: &str, salt: &[u8], iterations: u32) -> Result<aead::LessSafeKey, AppError> {
    let rounds = NonZeroU32::new(iterations)
        .ok_or_else(|| AppError::InvalidInput("invalid key derivation settings".into()))?;
    let mut key = [0u8; KEY_LEN];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        rounds,
        salt,
        password.as_bytes(),
        &mut key,
    );
    let unbound = aead::UnboundKey::new(&aead::AES_256_GCM, &key)
        .map_err(|_| AppError::Internal("cannot set up AES-256-GCM".into()))?;
    Ok(aead::LessSafeKey::new(unbound))
}

fn random<const N: usize>() -> Result<[u8; N], AppError> {
    let mut out = [0u8; N];
    rand::fill(&mut out).map_err(|_| AppError::Internal("no secure random numbers".into()))?;
    Ok(out)
}

/// Serialises boards as a board file, encrypted when a `password` is given (an empty one counts
/// as none).
pub fn write_file(
    boards: &[BoardRecord],
    password: Option<&str>,
    exported_at: &str,
) -> Result<Vec<u8>, AppError> {
    write_with_iterations(boards, password, exported_at, PBKDF2_ITERATIONS)
}

fn write_with_iterations(
    boards: &[BoardRecord],
    password: Option<&str>,
    exported_at: &str,
    iterations: u32,
) -> Result<Vec<u8>, AppError> {
    let mut envelope = Envelope {
        format: FORMAT.into(),
        version: VERSION,
        exported_at: exported_at.into(),
        boards: None,
        encrypted: None,
    };
    match password.filter(|p| !p.is_empty()) {
        None => envelope.boards = Some(boards.to_vec()),
        Some(password) => {
            let salt: [u8; SALT_LEN] = random()?;
            let nonce: [u8; aead::NONCE_LEN] = random()?;
            let key = derive_key(password, &salt, iterations)?;
            let mut data = serde_json::to_vec(boards)?;
            key.seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(AAD),
                &mut data,
            )
            .map_err(|_| AppError::Internal("encryption failed".into()))?;
            envelope.encrypted = Some(Sealed {
                kdf: KDF.into(),
                iterations,
                salt: B64.encode(salt),
                cipher: CIPHER.into(),
                nonce: B64.encode(nonce),
                data: B64.encode(&data),
            });
        }
    }
    let mut out = serde_json::to_vec_pretty(&envelope)?;
    out.push(b'\n');
    Ok(out)
}

fn not_a_file() -> AppError {
    AppError::InvalidInput("This is not a Tabler board file".into())
}

/// The message of a wrong password (or a modified file).
pub const WRONG_PASSWORD: &str = "The file password is wrong, or the file was modified";

/// Reads a board file. An encrypted file needs `password` (an empty one counts as none).
pub fn read_file(bytes: &[u8], password: Option<&str>) -> Result<ReadOutcome, AppError> {
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| not_a_file())?;
    if envelope.format != FORMAT {
        return Err(not_a_file());
    }
    if envelope.version > VERSION {
        return Err(AppError::InvalidInput(
            "This board file was made by a newer version of Tabler".into(),
        ));
    }
    let boards = match (envelope.boards, envelope.encrypted) {
        (Some(boards), None) => boards,
        (None, Some(sealed)) => {
            let Some(password) = password.filter(|p| !p.is_empty()) else {
                return Ok(ReadOutcome::NeedsPassword);
            };
            open_sealed(&sealed, password)?
        }
        _ => return Err(not_a_file()),
    };
    if boards.iter().any(|b| !b.objects.is_array()) {
        return Err(not_a_file());
    }
    Ok(ReadOutcome::Opened(boards))
}

fn open_sealed(sealed: &Sealed, password: &str) -> Result<Vec<BoardRecord>, AppError> {
    if sealed.kdf != KDF || sealed.cipher != CIPHER || sealed.iterations < MIN_ITERATIONS {
        return Err(AppError::InvalidInput(
            "This board file uses an unsupported encryption".into(),
        ));
    }
    if sealed.iterations > MAX_ITERATIONS {
        return Err(not_a_file());
    }
    let salt = B64.decode(&sealed.salt).map_err(|_| not_a_file())?;
    let nonce: [u8; aead::NONCE_LEN] = B64
        .decode(&sealed.nonce)
        .ok()
        .and_then(|n| n.try_into().ok())
        .ok_or_else(not_a_file)?;
    let mut data = B64.decode(&sealed.data).map_err(|_| not_a_file())?;
    let key = derive_key(password, &salt, sealed.iterations)?;
    let plain = key
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(AAD),
            &mut data,
        )
        .map_err(|_| AppError::InvalidInput(WRONG_PASSWORD.into()))?;
    serde_json::from_slice(plain).map_err(|_| not_a_file())
}

/// The boards to write for `ids` (unknown ids are skipped).
pub fn export_records(db: &AppDb, ids: &[Uuid]) -> Result<Vec<BoardRecord>, AppError> {
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(board) = get_board(db, *id)? {
            out.push(BoardRecord {
                name: board.name,
                objects: board.objects,
            });
        }
    }
    Ok(out)
}

/// The objects of an imported board with "Allow data changes" switched off on every one: a file
/// from someone else must not run statements that change data on its own (a board runs its
/// objects when it opens and on their timers). The user turns it on again per object in the
/// inspector. Everything else in the objects is kept, and so are entries that are not objects.
fn without_write_opt_in(objects: &serde_json::Value) -> serde_json::Value {
    let mut objects = objects.clone();
    if let Some(list) = objects.as_array_mut() {
        for object in list {
            if let Some(map) = object.as_object_mut() {
                map.insert("allowWrites".into(), serde_json::Value::Bool(false));
            }
        }
    }
    objects
}

/// Adds the boards of a file for `profile_id` (`None`: every connection); a name already shown
/// for that connection gets a number. Every board is checked before any is saved and the boards
/// are saved together, so a file with a bad board (blank or over-long name, objects too large)
/// adds none of them. Imported objects never keep "Allow data changes" (see
/// [`without_write_opt_in`]); [`export_records`] carries the user's own setting.
pub fn import_records(
    db: &AppDb,
    records: &[BoardRecord],
    profile_id: Option<Uuid>,
) -> Result<Vec<MetricsBoard>, AppError> {
    let mut taken: HashSet<String> = list_boards(db, profile_id)?
        .into_iter()
        .map(|b| b.name.to_lowercase())
        .collect();
    let mut checked = Vec::with_capacity(records.len());
    for record in records {
        let name = unique_name(&board_name(&record.name)?, &taken);
        taken.insert(name.to_lowercase());
        let objects = objects_json(&without_write_opt_in(&record.objects))?;
        checked.push((Uuid::new_v4(), name, objects));
    }
    let now = now_rfc3339();
    db.with_conn(|conn| {
        let tx = conn.transaction()?;
        for (id, name, objects) in &checked {
            upsert_board(&tx, *id, profile_id, name, objects, &now)?;
        }
        tx.commit()
    })?;
    checked
        .iter()
        .map(|(id, ..)| get_board(db, *id)?.ok_or_else(|| AppError::NotFound("board".into())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parts(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn query(sql_parts: &[&str], values: Vec<Value>, allow_writes: bool) -> MetricsQuery {
        MetricsQuery {
            parts: parts(sql_parts),
            values,
            allow_writes,
            max_rows: 0,
            timeout_secs: None,
        }
    }

    #[test]
    fn binds_variables_per_dialect() {
        let p = parts(&["SELECT * FROM t WHERE a = ", " AND b > ", " LIMIT 5"]);
        let values = vec![Value::Text("it's".into()), Value::Int(3)];

        let (sql, params) = bind_template(Dialect::Mysql, &p, &values).unwrap();
        assert_eq!(sql, "SELECT * FROM t WHERE a =  ?  AND b >  ?  LIMIT 5");
        assert_eq!(params, values);

        let (sql, params) = bind_template(Dialect::Sqlite, &p, &values).unwrap();
        assert_eq!(sql, "SELECT * FROM t WHERE a =  ?1  AND b >  ?2  LIMIT 5");
        assert_eq!(params, values);

        let (sql, params) = bind_template(Dialect::Postgres, &p, &values).unwrap();
        assert_eq!(
            sql,
            "SELECT * FROM t WHERE a =  'it''s'  AND b >  '3'  LIMIT 5"
        );
        assert!(params.is_empty());
    }

    #[test]
    fn postgres_literals_are_safe() {
        let p = parts(&["SELECT ", ", ", ", ", ""]);
        let values = vec![
            Value::Null,
            Value::Text(r"a\'; DROP TABLE x; --".into()),
            Value::Date("2026-01-02".into()),
        ];
        let (sql, _) = bind_template(Dialect::Postgres, &p, &values).unwrap();
        assert_eq!(
            sql,
            r"SELECT  NULL ,  E'a\\''; DROP TABLE x; --' ,  '2026-01-02' "
        );
        // Still one statement, and a READ.
        let prepared = prepare(
            Dialect::Postgres,
            &query(&["SELECT ", ""], vec![values[1].clone()], false),
        )
        .unwrap();
        assert!(!prepared.writes);
        assert!(
            bind_template(
                Dialect::Postgres,
                &p[..2],
                &[Value::Bytes(crate::types::value::BytesValue {
                    base64: String::new(),
                    len: 0,
                    truncated: false,
                })]
            )
            .is_err()
        );
    }

    #[test]
    fn adjacent_placeholders_stay_apart() {
        let (sql, _) =
            bind_template(Dialect::Sqlite, &parts(&["SELECT ", "0"]), &[Value::Int(1)]).unwrap();
        assert_eq!(sql, "SELECT  ?1 0");
        let (sql, _) = bind_template(
            Dialect::Postgres,
            &parts(&["SELECT ", "", ""]),
            &[Value::Text("a".into()), Value::Text("b".into())],
        )
        .unwrap();
        assert_eq!(sql, "SELECT  'a'  'b' ");
    }

    #[test]
    fn rejects_mismatched_templates() {
        assert!(bind_template(Dialect::Mysql, &parts(&["a", "b"]), &[]).is_err());
        assert!(bind_template(Dialect::Mysql, &[], &[]).is_err());
    }

    #[test]
    fn refuses_writes_unless_allowed() {
        let update = query(&["UPDATE t SET a = ", ""], vec![Value::Int(1)], false);
        let err = prepare(Dialect::Mysql, &update).unwrap_err();
        assert_eq!(err.to_string(), WRITE_REFUSED);
        let allowed = prepare(
            Dialect::Mysql,
            &MetricsQuery {
                allow_writes: true,
                ..update
            },
        )
        .unwrap();
        assert!(allowed.writes);
        assert_eq!(allowed.sql, "UPDATE t SET a =  ?");

        let read = prepare(Dialect::Postgres, &query(&["SELECT 1;"], vec![], false)).unwrap();
        assert_eq!(read.sql, "SELECT 1");
        assert!(!read.writes);
        // A write hidden in a CTE is still a write.
        let cte = query(
            &["WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d"],
            vec![],
            false,
        );
        assert!(prepare(Dialect::Postgres, &cte).is_err());
    }

    #[test]
    fn runs_one_statement() {
        let two = query(&["SELECT 1; SELECT 2"], vec![], false);
        assert!(prepare(Dialect::Sqlite, &two).is_err());
        let empty = query(&["  -- nothing\n"], vec![], false);
        assert!(prepare(Dialect::Sqlite, &empty).is_err());
        // A `;` inside a value is not a statement end.
        let semi = query(
            &["SELECT ", ""],
            vec![Value::Text("a; DELETE FROM t".into())],
            false,
        );
        assert!(prepare(Dialect::Postgres, &semi).is_ok());
    }

    #[test]
    fn caps_rows() {
        assert_eq!(row_cap(0), MAX_ROWS);
        assert_eq!(row_cap(10), 10);
        assert_eq!(row_cap(1_000_000), MAX_ROWS);
        let mut sink = CappedSink::new(3);
        sink.begin_set(vec![]);
        let row = || vec![Value::Int(1)];
        assert!(sink.rows(vec![row(), row()]).is_continue());
        assert!(sink.rows(vec![row()]).is_continue());
        assert!(!sink.truncated);
        assert!(sink.rows(vec![row()]).is_break());
        assert!(sink.truncated);
        assert_eq!(sink.rows.len(), 3);
        // Only the first result set counts.
        let mut sink = CappedSink::new(3);
        sink.begin_set(vec![]);
        sink.begin_set(vec![]);
        assert!(sink.rows(vec![row()]).is_break());
    }

    #[test]
    fn crud_round_trip() {
        let db = AppDb::open_in_memory().unwrap();
        let profile = Uuid::new_v4();
        let other = Uuid::new_v4();
        let a = save_board(
            &db,
            &MetricsBoardInput {
                id: None,
                profile_id: Some(profile),
                name: "  Sales  ".into(),
                objects: json!([{ "id": "o1", "kind": "bar" }]),
            },
        )
        .unwrap();
        assert_eq!(a.name, "Sales");
        assert_eq!(a.created_at, a.updated_at);
        let all = save_board(
            &db,
            &MetricsBoardInput {
                id: None,
                profile_id: None,
                name: "Everywhere".into(),
                objects: json!([]),
            },
        )
        .unwrap();
        save_board(
            &db,
            &MetricsBoardInput {
                id: None,
                profile_id: Some(other),
                name: "Theirs".into(),
                objects: json!([]),
            },
        )
        .unwrap();

        let names = |list: Vec<MetricsBoard>| list.into_iter().map(|b| b.name).collect::<Vec<_>>();
        assert_eq!(
            names(list_boards(&db, Some(profile)).unwrap()),
            ["Sales", "Everywhere"]
        );
        assert_eq!(list_boards(&db, None).unwrap().len(), 3);

        // Replacing keeps the id, creation time and place.
        let renamed = save_board(
            &db,
            &MetricsBoardInput {
                id: Some(a.id),
                profile_id: Some(profile),
                name: "Revenue".into(),
                objects: json!([{ "id": "o1" }, { "id": "o2" }]),
            },
        )
        .unwrap();
        assert_eq!(renamed.id, a.id);
        assert_eq!(renamed.created_at, a.created_at);
        assert_eq!(renamed.objects.as_array().unwrap().len(), 2);
        assert_eq!(
            names(list_boards(&db, Some(profile)).unwrap()),
            ["Revenue", "Everywhere"]
        );

        assert!(delete_board(&db, all.id).unwrap());
        assert!(!delete_board(&db, all.id).unwrap());
        assert!(get_board(&db, all.id).unwrap().is_none());

        let bad = |name: &str, objects: serde_json::Value| {
            save_board(
                &db,
                &MetricsBoardInput {
                    id: None,
                    profile_id: None,
                    name: name.into(),
                    objects,
                },
            )
        };
        assert!(bad(" ", json!([])).is_err());
        assert!(bad("x", json!({ "not": "a list" })).is_err());
    }

    fn records() -> Vec<BoardRecord> {
        vec![BoardRecord {
            name: "Sales".into(),
            objects: json!([{ "id": "o1", "kind": "scoreboard", "sql": "SELECT count(*) FROM orders" }]),
        }]
    }

    #[test]
    fn file_round_trip_in_the_clear() {
        let bytes = write_file(&records(), None, "2026-10-13T00:00:00Z").unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.contains("SELECT count(*) FROM orders"));
        assert_eq!(
            read_file(&bytes, None).unwrap(),
            ReadOutcome::Opened(records())
        );
        // An empty password counts as none.
        let bytes = write_file(&records(), Some(""), "").unwrap();
        assert_eq!(
            read_file(&bytes, None).unwrap(),
            ReadOutcome::Opened(records())
        );
    }

    #[test]
    fn file_round_trip_encrypted() {
        let bytes = write_with_iterations(&records(), Some("s3cret"), "", MIN_ITERATIONS).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(!text.contains("orders"));
        assert_eq!(read_file(&bytes, None).unwrap(), ReadOutcome::NeedsPassword);
        assert_eq!(
            read_file(&bytes, Some("s3cret")).unwrap(),
            ReadOutcome::Opened(records())
        );
        let err = read_file(&bytes, Some("wrong")).unwrap_err();
        assert_eq!(err.to_string(), WRONG_PASSWORD);
    }

    #[test]
    fn rejects_other_files() {
        assert!(read_file(b"not json", None).is_err());
        assert!(read_file(br#"{"format":"tabler-connections","version":1}"#, None).is_err());
        let newer = format!(r#"{{"format":"{FORMAT}","version":99,"boards":[]}}"#);
        assert!(read_file(newer.as_bytes(), None).is_err());
        let bad = format!(
            r#"{{"format":"{FORMAT}","version":1,"boards":[{{"name":"x","objects":{{}}}}]}}"#
        );
        assert!(read_file(bad.as_bytes(), None).is_err());
    }

    #[test]
    fn imports_with_unique_names() {
        let db = AppDb::open_in_memory().unwrap();
        let profile = Uuid::new_v4();
        let first = import_records(&db, &records(), Some(profile)).unwrap();
        let second = import_records(&db, &records(), Some(profile)).unwrap();
        assert_eq!(first[0].name, "Sales");
        assert_eq!(second[0].name, "Sales 2");
        assert_eq!(second[0].profile_id, Some(profile));
        // The objects are stored as they came, plus "Allow data changes" switched off.
        let exported = export_records(&db, &[first[0].id, Uuid::new_v4()]).unwrap();
        assert_eq!(exported.len(), 1);
        assert_eq!(exported[0].name, "Sales");
        assert_eq!(
            exported[0].objects,
            json!([{
                "id": "o1",
                "kind": "scoreboard",
                "sql": "SELECT count(*) FROM orders",
                "allowWrites": false
            }])
        );
        let taken: HashSet<String> = ["a".to_string(), "a 2".to_string()].into();
        assert_eq!(unique_name("A", &taken), "A 3");
        assert_eq!(unique_name("b", &taken), "b");
    }

    // ---- imported boards never write on their own --------------------------------------------

    fn dangerous() -> BoardRecord {
        BoardRecord {
            name: "From a stranger".into(),
            objects: json!([
                {
                    "id": "wipe",
                    "kind": "table",
                    "title": "Wipe",
                    "sql": "DELETE FROM users",
                    "allowWrites": true,
                    "refresh": { "mode": "interval", "seconds": 1 }
                },
                { "id": "count", "kind": "scoreboard", "sql": "SELECT 1", "allowWrites": false },
                { "id": "plain", "kind": "bar", "sql": "SELECT 2" },
                "not an object",
                7
            ]),
        }
    }

    #[test]
    fn imported_objects_never_keep_allow_writes() {
        let db = AppDb::open_in_memory().unwrap();
        let profile = Uuid::new_v4();
        let imported = import_records(&db, &[dangerous()], Some(profile)).unwrap();
        let stored = get_board(&db, imported[0].id).unwrap().unwrap();
        for board in [&imported[0], &stored] {
            let objects = board.objects.as_array().unwrap();
            assert_eq!(objects.len(), 5);
            // Every object is switched off, and nothing else about it changed.
            assert_eq!(
                objects[0],
                json!({
                    "id": "wipe",
                    "kind": "table",
                    "title": "Wipe",
                    "sql": "DELETE FROM users",
                    "allowWrites": false,
                    "refresh": { "mode": "interval", "seconds": 1 }
                })
            );
            assert_eq!(objects[1]["allowWrites"], json!(false));
            assert_eq!(objects[2]["allowWrites"], json!(false));
            assert_eq!(objects[2]["sql"], json!("SELECT 2"));
            // Entries that are not objects are left alone.
            assert_eq!(objects[3], json!("not an object"));
            assert_eq!(objects[4], json!(7));
        }
        // The user's own boards keep the setting, through a save and through an export.
        let mine = save_board(
            &db,
            &MetricsBoardInput {
                id: None,
                profile_id: Some(profile),
                name: "Mine".into(),
                objects: json!([{ "id": "o", "sql": "UPDATE t SET a = 1", "allowWrites": true }]),
            },
        )
        .unwrap();
        assert_eq!(mine.objects[0]["allowWrites"], json!(true));
        let exported = export_records(&db, &[mine.id]).unwrap();
        assert_eq!(exported[0].objects[0]["allowWrites"], json!(true));
        // A file made from it opens with the setting off again.
        let again = import_records(&db, &exported, Some(profile)).unwrap();
        assert_eq!(again[0].objects[0]["allowWrites"], json!(false));
    }

    #[test]
    fn importing_a_file_with_a_bad_board_adds_nothing() {
        let db = AppDb::open_in_memory().unwrap();
        let before = list_boards(&db, None).unwrap();
        let bad_name = |name: &str| BoardRecord {
            name: name.into(),
            objects: json!([]),
        };
        let too_large = BoardRecord {
            name: "Huge".into(),
            objects: json!([{ "sql": "x".repeat(OBJECTS_MAX_BYTES) }]),
        };
        let cases = [
            vec![records()[0].clone(), bad_name("   ")],
            vec![records()[0].clone(), bad_name(&"n".repeat(NAME_MAX + 1))],
            vec![records()[0].clone(), too_large],
            vec![
                records()[0].clone(),
                BoardRecord {
                    name: "Not a list".into(),
                    objects: json!({ "a": 1 }),
                },
            ],
        ];
        for file in cases {
            assert!(import_records(&db, &file, None).is_err());
            // The first board of the file is not left behind, so a retry cannot make "Sales 2".
            assert_eq!(list_boards(&db, None).unwrap(), before);
        }
        // The same file without the bad board imports as "Sales".
        let ok = import_records(&db, &records(), None).unwrap();
        assert_eq!(ok[0].name, "Sales");
    }

    #[test]
    fn imports_several_boards_in_the_order_of_the_file() {
        let db = AppDb::open_in_memory().unwrap();
        let file = [
            BoardRecord {
                name: "Zeta".into(),
                objects: json!([]),
            },
            BoardRecord {
                name: "Alpha".into(),
                objects: json!([]),
            },
            BoardRecord {
                name: "Alpha".into(),
                objects: json!([]),
            },
        ];
        let added = import_records(&db, &file, None).unwrap();
        let names: Vec<_> = added.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["Zeta", "Alpha", "Alpha 2"]);
        let listed: Vec<_> = list_boards(&db, None)
            .unwrap()
            .into_iter()
            .map(|b| b.name)
            .collect();
        assert_eq!(listed, ["Zeta", "Alpha", "Alpha 2"]);
    }

    // ---- reads that change the server -------------------------------------------------------

    #[test]
    fn refuses_functions_with_side_effects_unless_allowed() {
        let cases = [
            (Dialect::Postgres, "SELECT setval('s', 1)"),
            (Dialect::Postgres, "SELECT pg_catalog.nextval('s')"),
            (
                Dialect::Postgres,
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE pid <> pg_backend_pid()",
            ),
            (Dialect::Postgres, "SELECT lo_unlink(1234)"),
            (Dialect::Postgres, "SELECT pg_advisory_lock(1)"),
            (
                Dialect::Postgres,
                "SELECT dblink_exec('x', 'DELETE FROM t')",
            ),
            (Dialect::Mysql, "SELECT GET_LOCK('a', 10)"),
            (Dialect::Mysql, "SELECT SLEEP(100000)"),
        ];
        for (dialect, sql) in cases {
            // They are reads for the statement classifier, so `writes` stays false ...
            assert!(!classify::writes(sql, dialect), "{sql}");
            let err = prepare(dialect, &query(&[sql], vec![], false)).unwrap_err();
            assert_eq!(err.to_string(), SIDE_EFFECT_REFUSED, "{sql}");
            // ... and with "Allow data changes" they run as the read they are.
            let allowed = prepare(dialect, &query(&[sql], vec![], true)).unwrap();
            assert_eq!(allowed.sql, sql);
            assert!(!allowed.writes, "{sql}");
        }
        // A variable's value that looks like a call is data, not a call.
        let value = Value::Text("setval('s', 1)".into());
        let bound = query(&["SELECT ", ""], vec![value], false);
        assert!(prepare(Dialect::Postgres, &bound).is_ok());
        // Plain reads are unaffected, also with a column named like a function.
        for (dialect, sql) in [
            (Dialect::Postgres, "SELECT count(*) AS nextval FROM t"),
            (Dialect::Postgres, "SELECT now(), currval('s') FROM t"),
            (Dialect::Mysql, "SELECT sleep FROM t WHERE a = 'SLEEP(1)'"),
            (Dialect::Sqlite, "SELECT 1"),
        ] {
            let read = prepare(dialect, &query(&[sql], vec![], false)).unwrap();
            assert!(!read.writes);
        }
    }

    /// A call written so that a keyword scan misses it, which the server still runs as the
    /// function (MySQL 9.6 and PostgreSQL 17 return 3 for `SELECT LENGTH/*!*/('abc')` and
    /// `SELECT U&"\006cength"('abc')`): refused like the plain call.
    #[test]
    fn refuses_a_side_effect_call_written_in_disguise() {
        let cases = [
            (Dialect::Mysql, "SELECT SLEEP/*!*/(100000)"),
            (Dialect::Mysql, "SELECT /*!50000 GET_LOCK*/('a', 10)"),
            (Dialect::Mysql, "SELECT /*!SLEEP*/(1)"),
            (Dialect::Mysql, "SELECT /*!50000SLEEP*/(1)"),
            (Dialect::Mysql, "SELECT /*M!100000 SETVAL*/(s, 10)"),
            (Dialect::Mysql, "SELECT /*M!100100SLEEP*/(1)"),
            (Dialect::Mysql, "SELECT NEXT /*!*/ VALUE FOR s"),
            (Dialect::Postgres, r#"SELECT U&"set\0076al"('s', 1)"#),
            (
                Dialect::Postgres,
                r#"SELECT U&"pg_\+000074erminate_backend"(pid) FROM pg_stat_activity"#,
            ),
            (Dialect::Postgres, r#"SELECT U&"pg_s\006ceep"(100)"#),
            (Dialect::Sqlite, "SELECT [load_extension]('x')"),
        ];
        for (dialect, sql) in cases {
            assert!(!classify::writes(sql, dialect), "{sql}");
            let err = prepare(dialect, &query(&[sql], vec![], false)).unwrap_err();
            assert_eq!(err.to_string(), SIDE_EFFECT_REFUSED, "{sql}");
            let allowed = prepare(dialect, &query(&[sql], vec![], true)).unwrap();
            assert_eq!(allowed.sql, sql);
            assert!(!allowed.writes, "{sql}");
        }
        // A name whose escape character is set by UESCAPE cannot be worked out here: such a call
        // is refused as one that changes data, and runs only when the object allows that.
        for sql in [
            r#"SELECT U&"set!0076al" UESCAPE '!'('s', 1)"#,
            r#"SELECT U&"set!0076al" UESCAPE $$!$$('s', 1)"#,
            r#"SELECT U&"set!0076al" UESCAPE E'!'('s', 1)"#,
        ] {
            let err = prepare(Dialect::Postgres, &query(&[sql], vec![], false)).unwrap_err();
            assert!(
                [WRITE_REFUSED, SIDE_EFFECT_REFUSED].contains(&err.to_string().as_str()),
                "{sql}"
            );
            assert!(
                prepare(Dialect::Postgres, &query(&[sql], vec![], true)).is_ok(),
                "{sql}"
            );
        }
        // A version number glued to the keyword used to hide INTO: the server writes the file.
        let outfile = "SELECT 1 /*!50000INTO*/ OUTFILE '/tmp/x'";
        let err = prepare(Dialect::Mysql, &query(&[outfile], vec![], false)).unwrap_err();
        assert_eq!(err.to_string(), WRITE_REFUSED);
        assert!(
            prepare(Dialect::Mysql, &query(&[outfile], vec![], true))
                .unwrap()
                .writes
        );
        // The same text on the other servers, where a `/*! … */` is a plain comment, is a read.
        for dialect in [Dialect::Postgres, Dialect::Sqlite] {
            let read = prepare(
                dialect,
                &query(&["SELECT /*!50000 SLEEP*/(1)"], vec![], false),
            );
            assert!(!read.unwrap().writes);
        }
    }

    /// PostgreSQL ends a `--` comment at a lone CR and reads a non-ASCII character as part of a
    /// name (so `€$$` opens no dollar quote): the code that the scanner used to take for comment
    /// or string text runs on the server, so it is refused like any other write or side effect.
    #[test]
    fn refuses_what_the_postgres_lexer_reads_differently() {
        let writes = [
            "WITH x AS (-- c\r DELETE FROM t RETURNING 1) SELECT count(*) AS deleted FROM x",
            "SELECT 1 AS €$$; DELETE FROM users; SELECT 3 AS a$$",
            "SELECT 1 --x\r; DELETE FROM t",
        ];
        for sql in writes {
            let err = prepare(Dialect::Postgres, &query(&[sql], vec![], false)).unwrap_err();
            let said = err.to_string();
            assert!(
                said == WRITE_REFUSED || said.contains("one statement"),
                "{sql}: {said}"
            );
        }
        let calls = [
            "SELECT 1 --x\r, pg_sleep(100)",
            "SELECT 1 AS €$$, pg_sleep(100) AS a$$",
        ];
        for sql in calls {
            let err = prepare(Dialect::Postgres, &query(&[sql], vec![], false)).unwrap_err();
            assert_eq!(err.to_string(), SIDE_EFFECT_REFUSED, "{sql}");
        }
        // Several statements are refused whether or not the second one hides behind a CR.
        assert!(
            prepare(
                Dialect::Postgres,
                &query(&["SELECT 1 --x\r; SELECT pg_sleep(100)"], vec![], false)
            )
            .is_err()
        );
        // MySQL and SQLite read to the LF: the rest of the line is a comment, so it is a read.
        for dialect in [Dialect::Mysql, Dialect::Sqlite] {
            let sql = "SELECT 1 -- x\r, pg_sleep(100)";
            assert!(
                !prepare(dialect, &query(&[sql], vec![], false))
                    .unwrap()
                    .writes
            );
        }
        // A real dollar-quoted body and a plain comment still hide what they hold.
        for sql in [
            "SELECT $$pg_sleep(100)$$ AS s",
            "SELECT 1 --x, pg_sleep(100)",
        ] {
            assert!(
                prepare(Dialect::Postgres, &query(&[sql], vec![], false)).is_ok(),
                "{sql}"
            );
        }
    }

    // ---- query timeout ------------------------------------------------------------------------

    #[test]
    fn query_timeout_is_kept_between_five_and_six_hundred_seconds() {
        let secs = |requested| query_timeout(requested).as_secs();
        assert_eq!(secs(None), 60);
        assert_eq!(secs(Some(0)), 5);
        assert_eq!(secs(Some(1)), 5);
        assert_eq!(secs(Some(5)), 5);
        assert_eq!(secs(Some(90)), 90);
        assert_eq!(secs(Some(600)), 600);
        assert_eq!(secs(Some(601)), 600);
        assert_eq!(secs(Some(u32::MAX)), 600);
    }

    #[test]
    fn timeout_error_names_the_limit() {
        let err = timeout_error(Duration::from_secs(30), false);
        assert_eq!(err.to_string(), "Query timed out after 30 s");
        // Not a dropped connection: the renderer must not mistake it for one.
        assert_eq!(err.code(), "DB_ERROR");
        let payload = err.to_payload();
        assert!(payload.sqlstate.is_none() && payload.vendor_code.is_none());
        let hint = payload.hint.unwrap();
        assert!(hint.contains("Query timeout") && !hint.contains("rolled back"));
        // A write says what became of it.
        let write = timeout_error(Duration::from_secs(30), true);
        assert_eq!(write.to_string(), "Query timed out after 30 s");
        assert!(write.to_payload().hint.unwrap().contains("rolled back"));
    }

    async fn sqlite_session() -> (crate::drivers::sqlite::LiteSession, tempfile::TempDir) {
        use crate::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        let profile = ConnectionProfile {
            id: Uuid::new_v4(),
            name: "t".into(),
            color: None,
            group: None,
            driver: Driver::Sqlite,
            host: None,
            port: None,
            user: None,
            database: None,
            ssl: SslMode::Disable,
            ssl_ca_path: None,
            sqlite_path: Some(path.to_string_lossy().into_owned()),
            ssh: None,
            options: ProfileOptions::default(),
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        };
        let session = crate::drivers::sqlite::LiteSession::open(&profile)
            .await
            .unwrap();
        (session, dir)
    }

    /// A read that never ends on its own.
    const ENDLESS: &str =
        "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r) SELECT max(n) FROM r";

    #[tokio::test]
    async fn a_read_past_its_timeout_is_cancelled_and_reported() {
        let (session, _dir) = sqlite_session().await;
        let prepared = prepare(Dialect::Sqlite, &query(&[ENDLESS], vec![], false)).unwrap();
        let started = Instant::now();
        let err = run_with_timeout(
            &session,
            &prepared,
            MAX_ROWS,
            CancelSlot::default(),
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "Query timed out after 1 s");
        // Stopped by the cancel, not by waiting the statement out.
        assert!(started.elapsed() < Duration::from_secs(20));
        // The session is fine afterwards.
        let quick = prepare(
            Dialect::Sqlite,
            &query(&["SELECT 41 + 1 AS n"], vec![], false),
        )
        .unwrap();
        let result = run_with_timeout(
            &session,
            &quick,
            MAX_ROWS,
            CancelSlot::default(),
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        assert_eq!(result.rows, vec![vec![Value::Int(42)]]);
    }

    #[tokio::test]
    async fn a_read_within_its_timeout_is_not_disturbed() {
        let (session, _dir) = sqlite_session().await;
        let prepared = prepare(
            Dialect::Sqlite,
            &query(&["SELECT 1 AS a UNION ALL SELECT 2"], vec![], false),
        )
        .unwrap();
        let slot = CancelSlot::default();
        let result = run_with_timeout(
            &session,
            &prepared,
            MAX_ROWS,
            slot.clone(),
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        assert_eq!(result.rows.len(), 2);
        // No cancel was requested on the way.
        assert!(!crate::drivers::cancel_requested(&slot));
    }

    #[tokio::test]
    async fn the_user_can_still_cancel_a_run_that_has_a_timeout() {
        let (session, _dir) = sqlite_session().await;
        let prepared = prepare(Dialect::Sqlite, &query(&[ENDLESS], vec![], false)).unwrap();
        let slot = CancelSlot::default();
        // Waits until the statement is running, then cancels it the way `cancel_query` does, and
        // again until the run is over: SQLite drops an `interrupt()` that arrives between the
        // statement being armed and its first step (the endless aggregate has no row to poll the
        // slot with), so a single cancel right after `is_armed()` can be lost. The Cancel button
        // can be pressed again just so.
        let stop = async {
            while !slot.is_armed() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            loop {
                if let Some(handle) = request_cancel(&slot) {
                    session.cancel(handle).await.unwrap();
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        let run = run_with_timeout(
            &session,
            &prepared,
            MAX_ROWS,
            slot.clone(),
            Duration::from_secs(300),
        );
        // Bounded: if the user's cancel did not end the run, this fails in a minute instead of
        // waiting for the 300 s timeout.
        let result = tokio::time::timeout(Duration::from_secs(60), async {
            tokio::select! {
                result = run => result,
                _ = stop => unreachable!("the cancel loop does not end"),
            }
        })
        .await
        .expect("the user's cancel ends the run long before its timeout");
        // Cancelled by the user: not reported as a timeout.
        assert!(matches!(result, Err(AppError::Cancelled)), "{result:?}");
    }

    #[tokio::test]
    async fn a_write_past_its_timeout_is_cancelled_and_rolled_back() {
        let (session, _dir) = sqlite_session().await;
        session
            .execute("CREATE TABLE t (id INTEGER PRIMARY KEY, v INTEGER)", &[])
            .await
            .unwrap();
        // An INSERT that never gets to insert: its source never ends.
        let sql = format!("INSERT INTO t (v) {ENDLESS}");
        let prepared = prepare(Dialect::Sqlite, &query(&[&sql], vec![], true)).unwrap();
        assert!(prepared.writes);
        let started = Instant::now();
        let err = run_with_timeout(
            &session,
            &prepared,
            MAX_ROWS,
            CancelSlot::default(),
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "Query timed out after 1 s");
        assert!(err.to_payload().hint.unwrap().contains("rolled back"));
        // Stopped by the cancel, not by waiting the statement out.
        assert!(started.elapsed() < Duration::from_secs(20));
        // Nothing was written, and the session (its worker connection) is fine afterwards.
        let rows = session.query("SELECT count(*) FROM t", &[]).await.unwrap();
        assert_eq!(rows.rows, vec![vec![Value::Int(0)]]);
        let write = prepare(
            Dialect::Sqlite,
            &query(&["INSERT INTO t (v) VALUES (7)"], vec![], true),
        )
        .unwrap();
        let done = run_with_timeout(
            &session,
            &write,
            MAX_ROWS,
            CancelSlot::default(),
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        assert_eq!(done.affected, Some(1));
    }

    #[tokio::test]
    async fn a_write_can_be_cancelled_by_the_user_too() {
        let (session, _dir) = sqlite_session().await;
        session
            .execute("CREATE TABLE t (id INTEGER PRIMARY KEY, v INTEGER)", &[])
            .await
            .unwrap();
        let sql = format!("INSERT INTO t (v) {ENDLESS}");
        let prepared = prepare(Dialect::Sqlite, &query(&[&sql], vec![], true)).unwrap();
        let slot = CancelSlot::default();
        let stop = async {
            while !slot.is_armed() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            loop {
                if let Some(handle) = request_cancel(&slot) {
                    session.cancel(handle).await.unwrap();
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        let run = run(&session, &prepared, MAX_ROWS, slot.clone());
        let result = tokio::time::timeout(Duration::from_secs(60), async {
            tokio::select! {
                result = run => result,
                _ = stop => unreachable!("the cancel loop does not end"),
            }
        })
        .await
        .expect("cancelling a write ends it");
        assert!(matches!(result, Err(AppError::Cancelled)), "{result:?}");
        let rows = session.query("SELECT count(*) FROM t", &[]).await.unwrap();
        assert_eq!(rows.rows, vec![vec![Value::Int(0)]]);
    }

    /// A session whose statement ignores the first `ignore` cancel requests (a lost cancel) and
    /// otherwise runs for `runs`. Its plain `execute` fails: a write must take the cancellable
    /// path.
    struct Stubborn {
        runs: Duration,
        ignore: usize,
        cancels: std::sync::atomic::AtomicUsize,
    }

    impl Stubborn {
        fn new(runs: Duration, ignore: usize) -> Stubborn {
            Stubborn {
                runs,
                ignore,
                cancels: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn cancels(&self) -> usize {
            self.cancels.load(std::sync::atomic::Ordering::SeqCst)
        }

        async fn statement(&self, cancel: &CancelSlot) -> Result<(), AppError> {
            let _armed = crate::drivers::arm_cancel(
                cancel,
                crate::drivers::CancelHandle::Mysql { conn_id: 1 },
            )?;
            let started = Instant::now();
            loop {
                if self.cancels() > self.ignore {
                    return Err(AppError::Cancelled);
                }
                if started.elapsed() >= self.runs {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
    }

    fn unsupported<T>() -> Result<T, AppError> {
        Err(AppError::Unsupported("stubborn session".into()))
    }

    #[async_trait::async_trait]
    impl DbSession for Stubborn {
        fn driver(&self) -> crate::types::profile::Driver {
            crate::types::profile::Driver::Mysql
        }
        fn dialect(&self) -> Dialect {
            Dialect::Mysql
        }
        async fn server_info(&self) -> Result<crate::drivers::ServerInfo, AppError> {
            unsupported()
        }
        async fn ping(&self) -> Result<(), AppError> {
            Ok(())
        }
        async fn close(&self) {}
        async fn list_databases(
            &self,
        ) -> Result<Vec<crate::types::schema::DatabaseInfo>, AppError> {
            unsupported()
        }
        async fn list_schemas(&self) -> Result<Vec<crate::types::schema::SchemaInfo>, AppError> {
            unsupported()
        }
        async fn list_tables(
            &self,
            _schema: &str,
        ) -> Result<Vec<crate::types::schema::TableSummary>, AppError> {
            unsupported()
        }
        async fn table_meta(
            &self,
            _table: &crate::types::schema::TableRef,
        ) -> Result<crate::types::schema::TableMeta, AppError> {
            unsupported()
        }
        async fn view_definition(
            &self,
            _table: &crate::types::schema::TableRef,
        ) -> Result<Option<String>, AppError> {
            unsupported()
        }
        async fn completion_metadata(
            &self,
        ) -> Result<crate::types::schema::CompletionMetadata, AppError> {
            unsupported()
        }
        async fn query(
            &self,
            _sql: &str,
            _params: &[Value],
        ) -> Result<crate::types::query::ResultSet, AppError> {
            unsupported()
        }
        async fn execute(
            &self,
            _sql: &str,
            _params: &[Value],
        ) -> Result<crate::types::query::ExecOutcome, AppError> {
            Err(AppError::internal(
                "a board write ran without a cancel point",
            ))
        }
        async fn execute_cancellable(
            &self,
            _sql: &str,
            _params: &[Value],
            cancel: CancelSlot,
        ) -> Result<crate::types::query::ExecOutcome, AppError> {
            self.statement(&cancel).await?;
            Ok(crate::types::query::ExecOutcome {
                affected: 3,
                last_insert_id: None,
                rows: None,
            })
        }
        async fn transaction(
            &self,
            _statements: Vec<crate::drivers::TxStatement>,
        ) -> Result<Vec<crate::types::query::ExecOutcome>, AppError> {
            unsupported()
        }
        async fn run_statement(
            &self,
            _sql: &str,
            _max_rows: usize,
            _sink: &mut dyn RowSink,
            _cancel: CancelSlot,
        ) -> Result<crate::drivers::StatementOutcome, AppError> {
            unsupported()
        }
        async fn stream_query(
            &self,
            _sql: &str,
            _params: &[Value],
            _sink: &mut dyn RowSink,
            cancel: CancelSlot,
        ) -> Result<u64, AppError> {
            self.statement(&cancel).await?;
            Ok(0)
        }
        async fn cancel(&self, _handle: crate::drivers::CancelHandle) -> Result<(), AppError> {
            self.cancels
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        fn in_transaction(&self) -> Option<bool> {
            Some(false)
        }
    }

    const SHORT: Duration = Duration::from_millis(40);
    const RETRY: Duration = Duration::from_millis(15);

    async fn run_stubborn(
        session: &Stubborn,
        sql: &str,
        allow_writes: bool,
    ) -> Result<MetricsResult, AppError> {
        let prepared = prepare(Dialect::Mysql, &query(&[sql], vec![], allow_writes)).unwrap();
        run_with_timeout_retrying(
            session,
            &prepared,
            MAX_ROWS,
            CancelSlot::default(),
            SHORT,
            RETRY,
        )
        .await
    }

    #[tokio::test]
    async fn a_cancel_that_was_lost_is_sent_again() {
        // The first two cancel requests do nothing; the statement would run for eight seconds.
        let session = Stubborn::new(Duration::from_secs(8), 2);
        let started = Instant::now();
        let err = run_stubborn(&session, "SELECT 1", false).await.unwrap_err();
        assert!(
            err.to_string().starts_with("Query timed out after"),
            "{err}"
        );
        assert!(started.elapsed() < Duration::from_secs(4));
        assert_eq!(session.cancels(), 3);
    }

    #[tokio::test]
    async fn a_write_takes_the_cancellable_path_and_is_cancelled_at_its_limit() {
        // `Stubborn::execute` fails, so an uncancellable write would show that error instead.
        let session = Stubborn::new(Duration::from_secs(60), 0);
        let err = run_stubborn(&session, "UPDATE t SET a = 1", true)
            .await
            .unwrap_err();
        assert!(
            err.to_string().starts_with("Query timed out after"),
            "{err}"
        );
        assert!(err.to_payload().hint.unwrap().contains("rolled back"));
        assert_eq!(session.cancels(), 1);
    }

    #[tokio::test]
    async fn a_write_that_finishes_after_its_limit_reports_what_it_did() {
        // The cancel never takes (it came too late): the write ran to its end.
        let session = Stubborn::new(Duration::from_millis(250), usize::MAX);
        let result = run_stubborn(&session, "DELETE FROM t", true).await.unwrap();
        assert_eq!(result.affected, Some(3));
        assert!(session.cancels() >= 1);
    }

    #[tokio::test]
    async fn a_read_that_answers_after_its_limit_is_a_timeout() {
        // MySQL's killed `SLEEP()` answers instead of failing.
        let session = Stubborn::new(Duration::from_millis(250), usize::MAX);
        let err = run_stubborn(&session, "SELECT 1", false).await.unwrap_err();
        assert!(
            err.to_string().starts_with("Query timed out after"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_statement_within_its_limit_is_never_cancelled() {
        let session = Stubborn::new(Duration::from_millis(5), 0);
        let prepared = prepare(
            Dialect::Mysql,
            &query(&["UPDATE t SET a = 1"], vec![], true),
        )
        .unwrap();
        let result = run_with_timeout_retrying(
            &session,
            &prepared,
            MAX_ROWS,
            CancelSlot::default(),
            Duration::from_secs(30),
            RETRY,
        )
        .await
        .unwrap();
        assert_eq!(result.affected, Some(3));
        assert_eq!(session.cancels(), 0);
    }

    #[test]
    fn other_errors_after_the_limit_stay_what_they_are() {
        let limit = Duration::from_secs(5);
        let failed = after_the_limit(Err(AppError::NotConnected), true, limit);
        assert!(matches!(failed, Err(AppError::NotConnected)), "{failed:?}");
        let failed = after_the_limit(Err(AppError::NotConnected), false, limit);
        assert!(matches!(failed, Err(AppError::NotConnected)), "{failed:?}");
        let cancelled = after_the_limit(Err(AppError::Cancelled), true, limit);
        assert_eq!(
            cancelled.unwrap_err().to_string(),
            "Query timed out after 5 s"
        );
    }
}
