//! The Redis session: a `NativeSession` whose `TableSource` is the key browser.
//!
//! Connections: one per database index for the grid's reads (shared and pipelined, opened when a
//! database is first read, at most `MAX_READERS`; the profile's own database also answers the
//! introspection and sends cancels), one for edits (WATCH holds per connection, so it is used by
//! one apply at a time and SELECTs the database it writes), and one for the console, where
//! SELECT, MULTI and WATCH persist between commands like in redis-cli. A link that lost its
//! socket is opened again on next use (once, for reads).
//!
//! Read-only is reported as `ReadOnlyMode::ClassifierOnly` (no server-side mode); the session
//! itself refuses every write path on a read-only profile (edits, write commands in the console,
//! write calls, killing clients), whatever its caller checked before.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use async_trait::async_trait;

use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::native::{CallKind, NativePlan, NativeSession, TableSource};
use crate::drivers::{
    CancelHandle, CancelSlot, ConnectTarget, RowSink, ServerInfo, SqlTrace, StatementOutcome,
    TxState, cancel_portable,
};
use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::native::redis::{self as cmd, Command, Gates};
use crate::sqlgen::split::SplitStatement;
use crate::types::admin::{KillOutcome, KillScope, ProcessRow};
use crate::types::browse::{
    BrowsePage, BrowseRequest, FilterGroup, FilterOp, RowCount, RowKey, TraceCategory,
};
use crate::types::caps::{
    Affected, CountMode, DataCaps, Feature, LifecycleCaps, NamespaceCaps, NamespaceLabels, Paging,
    SchemaMode, SessionCaps, TxMode,
};
use crate::types::mutate::{ApplyRequest, ApplyResult};
use crate::types::profile::Driver;
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

use super::classify::{ServerReads, classify_line};
use super::conn::{Dial, Link, REPLY_TIMEOUT, ok_reply, trace};
use super::info::{self, Info, Server};
use super::keys::{self, Limits};
use super::params::{RedisParams, database_index};

type Resp = ::redis::Value;

/// Databases whose grid connection stays open at once; the least recently used one closes.
const MAX_READERS: usize = 8;

/// How often the app pings an idle session (the descriptor's keep-alive).
const KEEPALIVE_SECS: u32 = 30;

/// The filter bar operators the key browser maps onto `MATCH` globs (`scan::key_filter`); the
/// type column takes `=` only.
const FILTER_OPS: [FilterOp; 9] = [
    FilterOp::Eq,
    FilterOp::Like,
    FilterOp::Ilike,
    FilterOp::Contains,
    FilterOp::ContainsCase,
    FilterOp::HasPrefix,
    FilterOp::HasPrefixCase,
    FilterOp::HasSuffix,
    FilterOp::HasSuffixCase,
];

pub struct RedisSession {
    pub(super) dial: Dial,
    pub(super) server: Server,
    /// The read commands the server declares (`COMMAND`); `None` when it refused to say.
    pub(super) reads: Option<ServerReads>,
    pub(super) databases: u32,
    pub(super) default_db: u32,
    pub(super) read_only: bool,
    pub(super) params: RedisParams,
    pub(super) limits: Limits,
    user: Option<String>,
    /// Grid connections by database, most recently used first.
    readers: Mutex<Vec<Arc<Link>>>,
    opening: tokio::sync::Mutex<()>,
    /// The edit connection and the database it has selected.
    pub(super) writer: Mutex<Option<(Arc<Link>, u32)>>,
    /// Held by one apply at a time (WATCH … EXEC must not interleave).
    pub(super) writer_lock: tokio::sync::Mutex<()>,
    /// The console's connection.
    pub(super) console: Mutex<Option<Arc<Link>>>,
    /// The database the console has selected (kept when its connection is opened again).
    pub(super) console_db: Mutex<u32>,
    /// Per command queued in the console's MULTI, the database it SELECTs (EXEC runs them).
    pub(super) console_queue: Mutex<Vec<Option<u32>>>,
    /// Set once the console has had a connection: opening another one is said in a notice.
    pub(super) console_opened: AtomicBool,
    console_lock: tokio::sync::Mutex<()>,
    /// The console's MULTI.
    pub(super) tx: TxState,
    pub(super) trace: SqlTrace,
    closed: AtomicBool,
}

pub(super) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// The refusal of a write on a read-only profile.
pub(super) fn read_only_error() -> AppError {
    AppError::InvalidInput(READ_ONLY_MESSAGE.into())
}

impl RedisSession {
    /// Connects to the profile's server: the first connection on its database, then what the
    /// session needs to know of the server (`INFO`, the number of databases, the read commands).
    pub async fn connect(target: ConnectTarget<'_>) -> Result<RedisSession, AppError> {
        RedisSession::connect_as(target, crate::drivers::registry::unattended()).await
    }

    /// `connect`, told whether it runs unattended (the MCP process).
    pub(super) async fn connect_as(
        target: ConnectTarget<'_>,
        unattended: bool,
    ) -> Result<RedisSession, AppError> {
        let profile = target.profile;
        // MCP refuses the native stores in v1; the driver refuses too, should one get here.
        if unattended {
            return Err(AppError::Unsupported(
                "Redis: MCP is not available for native stores".into(),
            ));
        }
        let read_only = crate::conn_options::effective_read_only(profile);
        let params = RedisParams::from_driver_params(profile.options.driver_params.as_ref())?;
        let default_db = database_index(profile)?;
        let dial = Dial::new(&target, params.protocol, read_only)?;
        let first = Arc::new(dial.open(default_db, Some(REPLY_TIMEOUT)).await?);
        let replies = first
            .pipeline(&[
                cmd::info(None),
                cmd::config_get("databases"),
                cmd::command_table(),
            ])
            .await?;
        let [info_reply, config, commands] = replies.as_slice() else {
            return Err(AppError::Internal(
                "Redis: the introspection went missing".into(),
            ));
        };
        let info = Info::from_reply(info_reply);
        let server = Server::identify(&info);
        if server.mode == "sentinel" {
            return Err(AppError::Connect(
                "the server is a Sentinel: connect to the primary it monitors".into(),
            ));
        }
        let databases = if server.mode == "cluster" {
            // A cluster node has database 0 only.
            1
        } else {
            info::database_count(info::config_databases(config), &info::keyspace(&info))
        };
        if default_db >= databases {
            return Err(AppError::InvalidInput(format!(
                "the server has {databases} database(s): DB index {default_db} does not exist"
            )));
        }
        let reads = match commands {
            Resp::ServerError(_) => None,
            reply => Some(ServerReads::from_reply(reply)).filter(|r| !r.is_empty()),
        };
        let limits = params.limits();
        Ok(RedisSession {
            dial,
            server,
            reads,
            databases,
            default_db,
            read_only,
            params,
            limits,
            user: profile
                .user
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string),
            readers: Mutex::new(vec![first]),
            opening: tokio::sync::Mutex::new(()),
            writer: Mutex::new(None),
            writer_lock: tokio::sync::Mutex::new(()),
            console: Mutex::new(None),
            console_db: Mutex::new(default_db),
            console_queue: Mutex::new(Vec::new()),
            console_opened: AtomicBool::new(false),
            console_lock: tokio::sync::Mutex::new(()),
            tx: TxState::new(Some(false)),
            trace: SqlTrace::default(),
            closed: AtomicBool::new(false),
        })
    }

    pub(super) fn gates(&self) -> Gates {
        self.server.gates()
    }

    pub(super) fn ensure_open(&self) -> Result<(), AppError> {
        if self.closed.load(Ordering::SeqCst) {
            Err(AppError::NotConnected)
        } else {
            Ok(())
        }
    }

    /// The database a key table is (`db3`), one the server has.
    pub(super) fn db_of(&self, table: &TableRef) -> Result<u32, AppError> {
        match info::parse_db_name(&table.name) {
            Some(db) if db < self.databases => Ok(db),
            _ => Err(AppError::NotFound(format!(
                "Redis has no database \u{201c}{}\u{201d}",
                table.name
            ))),
        }
    }

    /// The grid connection of `db`, opened when there is none (or it lost its socket).
    pub(super) async fn reader(&self, db: u32) -> Result<Arc<Link>, AppError> {
        self.ensure_open()?;
        if let Some(link) = self.cached_reader(db) {
            return Ok(link);
        }
        let _one = self.opening.lock().await;
        if let Some(link) = self.cached_reader(db) {
            return Ok(link);
        }
        let link = Arc::new(self.dial.open(db, Some(REPLY_TIMEOUT)).await?);
        self.ensure_open()?;
        let mut readers = lock(&self.readers);
        readers.retain(|l| l.db != db);
        readers.insert(0, link.clone());
        // The profile's database stays: it answers the introspection and the cancels.
        while readers.len() > MAX_READERS {
            match readers.iter().rposition(|l| l.db != self.default_db) {
                Some(i) => drop(readers.remove(i)),
                None => break,
            }
        }
        Ok(link)
    }

    fn cached_reader(&self, db: u32) -> Option<Arc<Link>> {
        let mut readers = lock(&self.readers);
        let i = readers.iter().position(|l| l.db == db && l.alive())?;
        let link = readers.remove(i);
        readers.insert(0, link.clone());
        Some(link)
    }

    fn forget_reader(&self, link: &Arc<Link>) {
        lock(&self.readers).retain(|l| !Arc::ptr_eq(l, link));
    }

    /// `f` on the grid connection of `db`; once more on a new connection when that one turned
    /// out to be lost (reads only: they are safe to repeat).
    pub(super) async fn read<T, F, Fut>(&self, db: u32, f: F) -> Result<T, AppError>
    where
        F: Fn(Arc<Link>) -> Fut,
        Fut: std::future::Future<Output = Result<T, AppError>>,
    {
        let link = self.reader(db).await?;
        match f(link.clone()).await {
            Err(AppError::NotConnected) => {
                self.forget_reader(&link);
                f(self.reader(db).await?).await
            }
            other => other,
        }
    }

    /// One command of the app's own on the grid connection of `db`, traced; an error reply is the
    /// command's error.
    pub(super) async fn read_one(
        &self,
        db: u32,
        category: TraceCategory,
        c: &Command,
    ) -> Result<Resp, AppError> {
        let started = Instant::now();
        let out = self
            .read(db, |link| async move { link.send(c).await })
            .await
            .and_then(ok_reply);
        trace(
            &self.trace,
            category,
            std::slice::from_ref(c),
            started,
            &out,
            |_| None,
        );
        out
    }

    /// Commands of the app's own pipelined on the grid connection of `db`, traced; error replies
    /// stay values.
    pub(super) async fn read_many(
        &self,
        db: u32,
        category: TraceCategory,
        cmds: &[Command],
    ) -> Result<Vec<Resp>, AppError> {
        let started = Instant::now();
        let out = self
            .read(db, |link| async move { link.pipeline(cmds).await })
            .await;
        trace(&self.trace, category, cmds, started, &out, |_| None);
        out
    }

    /// The `CLIENT ID`s of the session's own connections.
    pub(super) fn own_ids(&self) -> Vec<i64> {
        let mut ids: Vec<i64> = lock(&self.readers).iter().filter_map(|l| l.id).collect();
        ids.extend(lock(&self.writer).as_ref().and_then(|(l, _)| l.id));
        ids.extend(lock(&self.console).as_ref().and_then(|l| l.id));
        ids
    }

    /// The protocol the session's connections speak (2 or 3).
    pub(super) fn protocol(&self) -> u8 {
        if self.dial.resp3() { 3 } else { 2 }
    }

    async fn keyspace(&self) -> Result<Vec<info::Keyspace>, AppError> {
        let reply = self
            .read_one(
                self.default_db,
                TraceCategory::Structure,
                &cmd::info(Some("keyspace")),
            )
            .await?;
        Ok(info::keyspace(&Info::from_reply(&reply)))
    }
}

#[async_trait]
impl TableSource for RedisSession {
    async fn browse(
        &self,
        meta: &TableMeta,
        req: &BrowseRequest,
        cancel: CancelSlot,
    ) -> Result<BrowsePage, AppError> {
        super::grid::browse(self, meta, req, cancel).await
    }

    async fn count(
        &self,
        meta: &TableMeta,
        filter: Option<&FilterGroup>,
        exact: bool,
        cancel: CancelSlot,
    ) -> Result<RowCount, AppError> {
        super::grid::count(self, meta, filter, exact, cancel).await
    }

    async fn fetch_cell(
        &self,
        meta: &TableMeta,
        key: &RowKey,
        column: &str,
    ) -> Result<Value, AppError> {
        super::grid::fetch_cell(self, meta, key, column).await
    }
}

#[async_trait]
impl NativeSession for RedisSession {
    fn driver(&self) -> Driver {
        Driver::Redis
    }

    /// The key browser (cursor pages, counts, edits by key and type), the console, the server's
    /// clients as the process list, and the definition of each database. No server-side cancel:
    /// a running command finishes on the server (the console stops waiting and drops its
    /// connection). `SafetyCaps.read_only` is `ClassifierOnly` (no server-side read-only mode);
    /// the session still refuses every write itself on a read-only profile (edits, console
    /// writes, write calls, kills).
    fn capabilities(&self) -> SessionCaps {
        let mut caps = SessionCaps::minimal(Driver::Redis).with(&[
            Feature::Editor,
            Feature::EditRows,
            Feature::InsertRows,
            Feature::DeleteRows,
            Feature::ExactCount,
            Feature::ViewDefinition,
            Feature::ProcessList,
            Feature::KillProcess,
            Feature::StartupCommands,
        ]);
        caps.namespaces = NamespaceCaps {
            databases: false,
            // Every database is a table of keys in one list (TablePlus shows db0, db1, …).
            schemas: SchemaMode::Single,
            default_schema: None,
            labels: NamespaceLabels {
                database: "Database".into(),
                schema: "Schema".into(),
                table: "Database".into(),
                tables: "Databases".into(),
            },
        };
        caps.data = DataCaps {
            paging: Paging::Cursor,
            // DBSIZE is free; a filtered count scans, so it is estimated unless asked for.
            count: CountMode::Estimate,
            // SCAN has no order: a sort could only reorder one page.
            sort: false,
            filter_ops: FILTER_OPS.to_vec(),
            raw_filter: None,
            // Each change in its own MULTI … EXEC behind its WATCH: a result per change.
            transactions: TxMode::Batch,
            affected: Affected::Exact,
            default_keyword: false,
            upsert: false,
            server_exprs: Vec::new(),
            max_params: 0,
        };
        caps.session = LifecycleCaps {
            keepalive_secs: Some(KEEPALIVE_SECS),
            ping_billed: false,
        };
        caps
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        self.ensure_open()?;
        Ok(ServerInfo {
            version: self.server.label(),
            current_database: Some(info::db_name(self.default_db)),
            current_user: Some(self.user.clone().unwrap_or_else(|| "default".into())),
        })
    }

    async fn ping(&self) -> Result<(), AppError> {
        let link = self.reader(self.default_db).await?;
        match link.send(&cmd::ping()).await.and_then(ok_reply) {
            Err(AppError::NotConnected) => {
                // An idle connection the server closed (its `timeout`): try a new one.
                self.forget_reader(&link);
                self.reader(self.default_db)
                    .await?
                    .send(&cmd::ping())
                    .await
                    .and_then(ok_reply)
                    .map(|_| ())
            }
            other => other.map(|_| ()),
        }
    }

    async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        // Aborting the links' tasks closes their sockets at once, also under a running command.
        for link in lock(&self.readers).drain(..) {
            link.kill();
        }
        if let Some((link, _)) = lock(&self.writer).take() {
            link.kill();
        }
        if let Some(link) = lock(&self.console).take() {
            link.kill();
        }
    }

    async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError> {
        cancel_portable(handle).await.unwrap_or(Ok(()))
    }

    fn in_transaction(&self) -> Option<bool> {
        self.tx.get()
    }

    fn sql_trace(&self) -> Option<&SqlTrace> {
        Some(&self.trace)
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        self.ensure_open()?;
        Ok(info::databases(self.databases, self.default_db))
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        self.ensure_open()?;
        Ok(Vec::new())
    }

    /// Every database as a table of keys, with its key count from `INFO keyspace`.
    async fn list_tables(&self, _schema: &str) -> Result<Vec<TableSummary>, AppError> {
        let keyspace = self.keyspace().await?;
        Ok(info::tables(self.databases, &keyspace))
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let db = self.db_of(table)?;
        let keys = self
            .keyspace()
            .await?
            .iter()
            .find(|k| k.db == db)
            .map_or(0, |k| k.keys);
        Ok(keys::table_meta(table, Some(keys)))
    }

    /// The database and the server it is on, as JSON.
    async fn definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        let db = self.db_of(table)?;
        let keyspace = self.keyspace().await?;
        let ks = keyspace.iter().find(|k| k.db == db);
        let doc = serde_json::json!({
            "database": db,
            "keys": ks.map_or(0, |k| k.keys),
            "keysWithTtl": ks.map_or(0, |k| k.expires),
            "averageTtlMs": ks.map_or(0, |k| k.avg_ttl),
            "server": self.server.label(),
            "mode": self.server.mode,
            "protocol": format!("RESP{}", self.protocol()),
            "primaryKey": keys::primary_key(),
        });
        serde_json::to_string_pretty(&doc)
            .map(Some)
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        self.ensure_open()?;
        Ok(CompletionMetadata::default())
    }

    fn plan_changes(&self, meta: &TableMeta, req: &ApplyRequest) -> Result<NativePlan, AppError> {
        if self.read_only {
            return Err(read_only_error());
        }
        let db = self.db_of(&meta.table)?;
        let edits = cmd::plan_changes(&req.changes, &self.gates())?;
        Ok(NativePlan {
            preview: cmd::preview(&edits),
            ops: Box::new(super::apply::Plan { db, edits }),
        })
    }

    async fn apply_changes(
        &self,
        _meta: &TableMeta,
        req: &ApplyRequest,
        plan: NativePlan,
    ) -> ApplyResult {
        super::apply::apply_changes(self, req, plan).await
    }

    fn split_console(&self, text: &str) -> Vec<SplitStatement> {
        super::cli::split_console(text)
    }

    fn classify_console(&self, command: &str) -> StatementClass {
        classify_line(command, self.reads.as_ref())
    }

    async fn run_console(
        &self,
        command: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError> {
        super::console::run(self, command, max_rows, sink, cancel).await
    }

    fn console_lock(&self) -> Option<&tokio::sync::Mutex<()>> {
        Some(&self.console_lock)
    }

    async fn call(
        &self,
        method: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        super::calls::call(self, method, args).await
    }

    fn call_kind(&self, method: &str) -> CallKind {
        super::calls::call_kind(method)
    }

    async fn list_processes(&self) -> Option<Result<Vec<ProcessRow>, AppError>> {
        Some(super::clients::list(self).await)
    }

    async fn kill_processes(&self, ids: &[String], _scope: KillScope) -> Option<Vec<KillOutcome>> {
        Some(super::clients::kill(self, ids).await)
    }
}
