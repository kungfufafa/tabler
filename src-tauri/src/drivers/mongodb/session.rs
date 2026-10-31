//! The MongoDB session: a `NativeSession` and its `TableSource` over the crate's `Client`.
//!
//! - Every command is a BSON document from `sqlgen::native::mongo`, run with `runCommand`; the
//!   app's own ones are traced as the console would read them back.
//! - Cursors (the grid's pages, samples, listings, the console's results) are read through the
//!   crate's cursors, in an explicit session where the server has sessions: a `getMore` goes to
//!   the member and the session that opened the cursor, and a cursor left early is killed.
//! - Cancel: what runs carries Tabler's `comment` and its session; the cancel stops waiting at
//!   once and kills the operation on the server (`$currentOp` + `killOp`), keeping the session
//!   from the next operation until it is done.
//! - Read-only: the profile's (`effective_read_only`) is enforced here too, before anything is
//!   sent: edits, the console's writes (its classifier is default-deny) and the write calls.
//! - Command Line passwords: when the server refuses the password (a rotated secret), the
//!   command runs again and the client signs in anew, once.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use ::mongodb::bson::{Bson, Document};
use ::mongodb::error::{TRANSIENT_TRANSACTION_ERROR, UNKNOWN_TRANSACTION_COMMIT_RESULT};
use ::mongodb::options::SelectionCriteria;
use ::mongodb::{Client, ClientSession, Cursor, SessionCursor};
use async_trait::async_trait;
use futures::StreamExt as _;
use futures::future::BoxFuture;
use tokio::sync::watch;

use super::catalog::{self, CollectionInfo, Topology};
use super::classify;
use super::connect::{self, Login};
use super::errors;
use super::literal::int_of;
use super::params::MongoParams;
use super::schema;
use super::values;
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::native::{CallKind, NativePlan, NativeSession, TableSource};
use crate::drivers::{
    CancelAction, CancelHandle, CancelSlot, ConnectTarget, RowSink, ServerInfo, SqlTrace,
    StatementOutcome, TraceCounts, arm_cancel, cancel_portable,
};
use crate::error::{AppError, ErrorPayload};
use crate::secrets::CommandPassword;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::native::mongo::{self, EditPlan, FindSpec, ID, LoadedTypes};
use crate::sqlgen::split::SplitStatement;
use crate::types::admin::{KillOutcome, KillScope, ProcessRow};
use crate::types::browse::{
    BrowsePage, BrowseRequest, FilterGroup, FilterOp, RowCount, RowIdentity, RowKey, TraceCategory,
};
use crate::types::caps::{
    Affected, CountMode, DataCaps, Feature, LifecycleCaps, NamespaceCaps, NamespaceLabels, Paging,
    RawFilter, SafeConfirm, SafetyCaps, SchemaMode, SessionCaps, TxMode,
};
use crate::types::driver::ReadOnlyMode;
use crate::types::mutate::{ApplyRequest, ApplyResult, RowResult, ServerExpr};
use crate::types::profile::Driver;
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

/// How often the app pings an idle session (the descriptor's keep-alive).
const KEEPALIVE_SECS: u32 = 30;
/// `close` gives up waiting for the client after this long.
const CLOSE_LIMIT: Duration = Duration::from_secs(2);
/// Documents whose field types the session remembers per collection, for edits; beyond it the
/// oldest pages are forgotten and an edit falls back to its column's type.
const LOADED_DOCUMENTS: usize = 200_000;
/// The console's database when the profile names none (mongosh's).
const DEFAULT_CONSOLE_DB: &str = "test";

/// The filter operators `mongo::filter_document` writes (JSON containment is PostgreSQL's).
const FILTER_OPS: [FilterOp; 23] = [
    FilterOp::Eq,
    FilterOp::Ne,
    FilterOp::Gt,
    FilterOp::Lt,
    FilterOp::Ge,
    FilterOp::Le,
    FilterOp::Like,
    FilterOp::NotLike,
    FilterOp::In,
    FilterOp::NotIn,
    FilterOp::IsNull,
    FilterOp::IsNotNull,
    FilterOp::Between,
    FilterOp::NotBetween,
    FilterOp::Contains,
    FilterOp::NotContains,
    FilterOp::ContainsCase,
    FilterOp::NotContainsCase,
    FilterOp::HasPrefix,
    FilterOp::HasSuffix,
    FilterOp::HasPrefixCase,
    FilterOp::HasSuffixCase,
    FilterOp::Ilike,
];

/// The client in use and how many times the session signed in anew.
#[derive(Clone)]
struct Live {
    client: Client,
    generation: u64,
}

pub struct MongoSession {
    live: RwLock<Live>,
    renewing: tokio::sync::Mutex<()>,
    login: Login,
    /// The read preference of reads.
    reads: SelectionCriteria,
    sample_size: u32,
    app_name: String,
    caps: SessionCaps,
    info: ServerInfo,
    topology: Topology,
    /// The server has sessions (`logicalSessionTimeoutMinutes`): cursors and cancels use them.
    sessions: bool,
    read_only: bool,
    /// The profile's database (or the connection string's): the sidebar's and the grid's.
    database: Option<String>,
    /// The console's database (`use x` changes it).
    console_db: Mutex<String>,
    console: tokio::sync::Mutex<()>,
    trace: SqlTrace,
    /// The field types of the documents the grid loaded, by database and collection.
    loaded: Mutex<HashMap<(String, String), LoadedTypes>>,
    /// The sampled fields of the collections whose structure was read, for completion.
    fields: Mutex<BTreeMap<String, BTreeMap<String, Vec<String>>>>,
}

/// A Tabler session, shared by what runs in it and the cancel of that.
pub(super) type Held = Arc<tokio::sync::Mutex<ClientSession>>;

/// What an operation carries so that a cancel finds it on the server: Tabler's comment and,
/// where the server has sessions, the session it runs in.
#[derive(Clone)]
pub(super) struct OpTag {
    pub comment: String,
    pub lsid: Option<Document>,
    session: Option<Held>,
}

impl OpTag {
    /// The tag of an operation run in `session` (none: the crate's implicit one).
    pub(super) fn new(session: Option<ClientSession>) -> OpTag {
        OpTag {
            comment: mongo::op_comment(&uuid::Uuid::new_v4().simple().to_string()),
            lsid: session.as_ref().map(|s| s.id().clone()),
            session: session.map(|s| Arc::new(tokio::sync::Mutex::new(s))),
        }
    }

    /// The session, for one command.
    pub(super) async fn session(&self) -> Option<tokio::sync::MutexGuard<'_, ClientSession>> {
        match &self.session {
            Some(held) => Some(held.lock().await),
            None => None,
        }
    }
}

/// The cancel of one operation: stop waiting for it now, and kill it on the server.
struct KillOp {
    client: Client,
    tag: OpTag,
    stop: watch::Sender<bool>,
}

impl CancelAction for KillOp {
    fn fire(&self) -> BoxFuture<'static, Result<(), AppError>> {
        self.stop.send_replace(true);
        let client = self.client.clone();
        let find = mongo::current_op_matching(&self.tag.comment, self.tag.lsid.as_ref());
        // The session stays out of the crate's pool until the listing stops matching its id:
        // the pool hands the session returned last to the next operation, which a stale match
        // would kill.
        let held = self.tag.session.clone();
        Box::pin(async move {
            // A server that stalls `$currentOp` or `killOp` must not hold the cancelled run (a
            // query time out waits for its kill): past the limit the run is stopped here only.
            let killed = tokio::time::timeout(KILL_LIMIT, kill_matching(&client, find))
                .await
                .unwrap_or(Ok(()));
            drop(held);
            killed
        })
    }

    fn label(&self) -> &'static str {
        "killOp"
    }
}

/// How long a cancel looks for its operation on the server, and how long the whole kill may
/// take.
const KILL_SEARCH: Duration = Duration::from_secs(2);
const KILL_LIMIT: Duration = Duration::from_secs(5);

/// Kills the operations `find` lists. The operation may not have reached the server yet when
/// the cancel comes (a loaded machine, a slow link), so the listing is tried again until
/// `KILL_SEARCH` has passed.
async fn kill_matching(client: &Client, find: Document) -> Result<(), AppError> {
    let admin = client.database("admin");
    let until = Instant::now() + KILL_SEARCH;
    loop {
        let reply = admin
            .run_command(find.clone())
            .await
            .map_err(|e| errors::from_driver(&e))?;
        let (ops, _) = catalog::cursor_batch(&reply);
        let ids: Vec<Bson> = ops.iter().filter_map(|o| o.get("opid").cloned()).collect();
        if !ids.is_empty() {
            for id in ids {
                admin
                    .run_command(mongo::kill_op(id))
                    .await
                    .map_err(|e| errors::from_driver(&e))?;
            }
            return Ok(());
        }
        if Instant::now() >= until {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A cursor read in Tabler's explicit session, or in the crate's implicit one.
pub(super) enum Docs {
    InSession(SessionCursor<Document>, Held),
    Implicit(Cursor<Document>),
}

impl Docs {
    pub(super) async fn next(&mut self) -> Option<Result<Document, AppError>> {
        let item = match self {
            Docs::InSession(cursor, held) => {
                let mut session = held.lock().await;
                cursor.next(&mut session).await
            }
            Docs::Implicit(cursor) => cursor.next().await,
        };
        item.map(|r| r.map_err(|e| errors::from_driver(&e)))
    }
}

/// Whether a command reads (the read preference applies) or writes (the primary).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Run {
    Read,
    Write,
}

fn non_blank(v: &Option<String>) -> Option<String> {
    v.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn read_only_error() -> AppError {
    AppError::InvalidInput(READ_ONLY_MESSAGE.into())
}

impl MongoSession {
    /// Connects and signs in, then reads what the server is (`hello`, `buildInfo`, who signed
    /// in). `unattended` (the MCP process) is refused: MCP serves no native store in v1.
    pub async fn open(
        target: ConnectTarget<'_>,
        unattended: bool,
    ) -> Result<MongoSession, AppError> {
        if unattended {
            return Err(AppError::Unsupported(
                "MongoDB is not available to MCP clients in this Tabler".into(),
            ));
        }
        let ttl = Duration::from_secs(target.profile.driver.descriptor().command_ttl_secs.into());
        let command = CommandPassword::for_profile_ttl(target.profile, target.password, ttl);
        MongoSession::open_with(target, command).await
    }

    /// `open` with the Command Line password's command, which printed `target.password`.
    pub(super) async fn open_with(
        target: ConnectTarget<'_>,
        command: Option<Arc<CommandPassword>>,
    ) -> Result<MongoSession, AppError> {
        let profile = target.profile;
        let params = MongoParams::of_profile(profile)?;
        let mut options = connect::client_options(&target, &params, command.clone()).await?;
        let reads = connect::read_preference(&params, options.selection_criteria.take());
        let database = non_blank(&profile.database).or_else(|| options.default_database.clone());
        let client = Client::with_options(options.clone()).map_err(|e| errors::from_driver(&e))?;
        let trace = SqlTrace::default();
        let hello = match server_facts(&client, &trace).await {
            Ok(facts) => facts,
            Err(e) => {
                shut_down(client).await;
                return Err(e);
            }
        };
        let (topology, sessions, info) = hello;
        let info = ServerInfo {
            current_database: database.clone(),
            ..info
        };
        let read_only = crate::conn_options::effective_read_only(profile);
        let confirm = if crate::commands::safety::confirms_with_passcode(profile) {
            SafeConfirm::Passcode
        } else {
            SafeConfirm::Password
        };
        let caps = capabilities(&topology, database.as_deref(), confirm);
        Ok(MongoSession {
            live: RwLock::new(Live {
                client,
                generation: 0,
            }),
            renewing: tokio::sync::Mutex::new(()),
            login: Login { options, command },
            reads,
            sample_size: params.sample_size(),
            app_name: params.app_name().to_string(),
            caps,
            info,
            topology,
            sessions,
            read_only,
            console_db: Mutex::new(
                database
                    .clone()
                    .unwrap_or_else(|| DEFAULT_CONSOLE_DB.to_string()),
            ),
            database,
            console: tokio::sync::Mutex::new(()),
            trace,
            loaded: Mutex::default(),
            fields: Mutex::default(),
        })
    }

    fn live(&self) -> Live {
        self.live.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub(super) fn client(&self) -> Client {
        self.live().client
    }

    /// The client in use and its generation, for `renew`.
    pub(super) fn live_client(&self) -> (Client, u64) {
        let live = self.live();
        (live.client, live.generation)
    }

    pub(super) fn read_only(&self) -> bool {
        self.read_only
    }

    pub(super) fn app_name(&self) -> &str {
        &self.app_name
    }

    pub(super) fn console_db(&self) -> String {
        self.console_db
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub(super) fn set_console_db(&self, db: &str) {
        *self.console_db.lock().unwrap_or_else(|p| p.into_inner()) = db.to_string();
    }

    /// The database of `table` (its schema), else the session's.
    pub(super) fn database_of(&self, table: &TableRef) -> Result<String, AppError> {
        table
            .schema
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| self.database.clone())
            .ok_or_else(|| AppError::InvalidInput("MongoDB: choose a database first".into()))
    }

    /// Signs in anew with the password command's fresh output, when the client of `generation`
    /// was refused and nobody has renewed it since. Whether there is a new client to try.
    pub(super) async fn renew(&self, generation: u64) -> bool {
        if !self.login.renewable() {
            return false;
        }
        let _one_at_a_time = self.renewing.lock().await;
        if self.live().generation != generation {
            return true;
        }
        let options = match self.login.renewed().await {
            Ok(o) => o,
            Err(e) => {
                log::warn!("MongoDB: the password command failed while signing in again: {e}");
                return false;
            }
        };
        let client = match Client::with_options(options) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("MongoDB: signing in again failed: {e}");
                return false;
            }
        };
        let old = {
            let mut live = self.live.write().unwrap_or_else(|p| p.into_inner());
            let old = live.client.clone();
            *live = Live {
                client,
                generation: generation + 1,
            };
            old
        };
        tokio::spawn(shut_down(old));
        true
    }

    /// Runs `op` on the current client, and once more on a renewed one when the server refused
    /// the password: sign-in happens before a command is sent, so nothing ran twice.
    pub(super) async fn retrying<T, F, Fut>(&self, op: F) -> Result<T, AppError>
    where
        F: Fn(Client) -> Fut,
        Fut: Future<Output = Result<T, AppError>>,
    {
        let live = self.live();
        match op(live.client).await {
            Err(AppError::Auth(m)) => {
                if self.renew(live.generation).await {
                    op(self.client()).await
                } else {
                    Err(AppError::Auth(m))
                }
            }
            other => other,
        }
    }

    /// One command on `db`, in `session` when given.
    pub(super) async fn run(
        &self,
        client: &Client,
        db: &str,
        command: Document,
        run: Run,
        session: Option<&mut ClientSession>,
    ) -> Result<Document, AppError> {
        let database = client.database(db);
        let mut action = database.run_command(command);
        if run == Run::Read {
            action = action.selection_criteria(self.reads.clone());
        }
        let reply = match session {
            Some(s) => action.session(s).await,
            None => action.await,
        };
        reply.map_err(|e| errors::from_driver(&e))
    }

    /// A command Tabler sends for itself, traced; a read is tried again after a refused sign-in.
    pub(super) async fn traced(
        &self,
        category: TraceCategory,
        db: &str,
        command: Document,
        run: Run,
    ) -> Result<Document, AppError> {
        let text = mongo::command_text(Some(db), &command);
        let started = Instant::now();
        let result = self
            .retrying(|client| {
                let command = command.clone();
                async move { self.run(&client, db, command, run, None).await }
            })
            .await;
        self.trace
            .finish(category, text, started, &result, |_| TraceCounts::NONE);
        result
    }

    /// A Tabler session for `client`, where the server has them.
    pub(super) async fn start_session(
        &self,
        client: &Client,
    ) -> Result<Option<ClientSession>, AppError> {
        if !self.sessions {
            return Ok(None);
        }
        client
            .start_session()
            .await
            .map(Some)
            .map_err(|e| errors::from_driver(&e))
    }

    /// Opens the cursor of `command` (find, aggregate, listCollections, listIndexes) in the
    /// tag's session, its `getMore`s tagged like it and asking `batch` documents each.
    pub(super) async fn open_cursor(
        &self,
        client: &Client,
        db: &str,
        command: Document,
        run: Run,
        tag: &OpTag,
        batch: Option<u32>,
    ) -> Result<Docs, AppError> {
        let database = client.database(db);
        let command = mongo::with_comment(command, &tag.comment);
        let comment = command.get("comment").cloned();
        let mut action = database.run_cursor_command(command);
        if run == Run::Read {
            action = action.selection_criteria(self.reads.clone());
        }
        if let Some(n) = batch.filter(|n| *n > 0) {
            action = action.batch_size(n);
        }
        if let Some(c) = comment {
            action = action.comment(c);
        }
        let docs = match &tag.session {
            Some(held) => {
                let mut session = held.lock().await;
                let cursor = action.session(&mut *session).await;
                drop(session);
                cursor.map(|c| Docs::InSession(c, held.clone()))
            }
            None => action.await.map(Docs::Implicit),
        };
        docs.map_err(|e| errors::from_driver(&e))
    }

    /// Up to `limit` documents (0: all) of a cursor command that reads, traced, in a session of
    /// their own.
    pub(super) async fn read_docs(
        &self,
        category: TraceCategory,
        db: &str,
        command: Document,
        limit: usize,
    ) -> Result<Vec<Document>, AppError> {
        self.read_docs_on(category, db, command, limit, Run::Read)
            .await
    }

    /// `read_docs` on the members `run` picks (the primary for what a later command acts on).
    pub(super) async fn read_docs_on(
        &self,
        category: TraceCategory,
        db: &str,
        command: Document,
        limit: usize,
        run: Run,
    ) -> Result<Vec<Document>, AppError> {
        let text = mongo::command_text(Some(db), &command);
        let started = Instant::now();
        let result = self
            .retrying(|client| {
                let command = command.clone();
                async move {
                    let tag = OpTag::new(self.start_session(&client).await?);
                    let mut docs = self
                        .open_cursor(&client, db, command, run, &tag, None)
                        .await?;
                    let mut out = Vec::new();
                    while limit == 0 || out.len() < limit {
                        match docs.next().await {
                            Some(d) => out.push(d?),
                            None => break,
                        }
                    }
                    Ok(out)
                }
            })
            .await;
        self.trace.finish(category, text, started, &result, |docs| {
            TraceCounts::rows(docs.len())
        });
        result
    }

    /// Runs `op` until it ends or the run is cancelled through `slot`, which kills it on the
    /// server.
    pub(super) async fn cancellable<T>(
        &self,
        slot: &CancelSlot,
        client: &Client,
        tag: &OpTag,
        op: impl Future<Output = Result<T, AppError>>,
    ) -> Result<T, AppError> {
        let (stop, mut stopped) = watch::channel(false);
        let action = KillOp {
            client: client.clone(),
            tag: tag.clone(),
            stop,
        };
        let _armed = arm_cancel(slot, CancelHandle::Custom(Arc::new(action)))?;
        let cancelled = async {
            if stopped.wait_for(|s| *s).await.is_err() {
                // The action is gone with the slot: nothing can cancel any more.
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            biased;
            () = cancelled => Err(AppError::Cancelled),
            result = op => result,
        }
    }

    /// Remembers the field types of documents the grid loaded from `db.coll`.
    fn record_loaded(&self, db: &str, coll: &str, docs: &[Document]) {
        let mut loaded = self.loaded.lock().unwrap_or_else(|p| p.into_inner());
        let types = loaded
            .entry((db.to_string(), coll.to_string()))
            .or_default();
        if types.len() + docs.len() > LOADED_DOCUMENTS {
            types.clear();
        }
        for d in docs {
            types.record(d);
        }
    }

    fn loaded_types(&self, db: &str, coll: &str) -> LoadedTypes {
        self.loaded
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&(db.to_string(), coll.to_string()))
            .cloned()
            .unwrap_or_default()
    }

    /// A collection's `listCollections` entry; a user who may not list collections with their
    /// options gets the plain entry of its name.
    async fn collection(&self, db: &str, name: &str) -> Result<CollectionInfo, AppError> {
        let entries = self
            .read_docs(
                TraceCategory::Structure,
                db,
                mongo::collection_info(name),
                0,
            )
            .await;
        let entries = match entries {
            Err(AppError::Db {
                vendor_code: Some(13),
                ..
            }) => {
                let mut by_name = mongo::list_collections(true);
                by_name.insert("filter", ::mongodb::bson::doc! { "name": name });
                self.read_docs(TraceCategory::Structure, db, by_name, 0)
                    .await?
            }
            other => other?,
        };
        entries
            .iter()
            .find_map(CollectionInfo::from_entry)
            .ok_or_else(|| AppError::NotFound(format!("collection {db}.{name}")))
    }

    async fn indexes(&self, db: &str, coll: &str) -> Vec<crate::types::schema::IndexMeta> {
        // A view has no indexes (the server says so with an error).
        match self
            .read_docs(TraceCategory::Structure, db, mongo::list_indexes(coll), 0)
            .await
        {
            Ok(entries) => catalog::indexes(coll, &entries),
            Err(_) => Vec::new(),
        }
    }

    /// The sample the columns are inferred from: `$sample`, else the first documents.
    async fn sample(&self, db: &str, coll: &str) -> Result<Vec<Document>, AppError> {
        let size = self.sample_size;
        match self
            .read_docs(
                TraceCategory::Structure,
                db,
                mongo::sample(coll, size),
                size as usize,
            )
            .await
        {
            Ok(docs) => Ok(docs),
            Err(AppError::Cancelled) => Err(AppError::Cancelled),
            Err(_) => {
                self.read_docs(
                    TraceCategory::Structure,
                    db,
                    mongo::sample_fallback(coll, size),
                    size as usize,
                )
                .await
            }
        }
    }
}

/// `hello`, `buildInfo` and `connectionStatus` on a new client: the topology, whether the server
/// has sessions, and what the status pill shows. The first command is where the client finds a
/// server and signs in.
async fn server_facts(
    client: &Client,
    trace: &SqlTrace,
) -> Result<(Topology, bool, ServerInfo), AppError> {
    let admin = client.database("admin");
    let run = |command: Document| {
        let admin = admin.clone();
        let text = mongo::command_text(Some("admin"), &command);
        trace.run_async(
            TraceCategory::Structure,
            text,
            |_| TraceCounts::NONE,
            async move {
                admin
                    .run_command(command)
                    .await
                    .map_err(|e| errors::from_driver(&e))
            },
        )
    };
    let hello = run(mongo::hello()).await?;
    let build = run(mongo::build_info()).await?;
    // Some compatible servers do not answer it; the user is then not shown.
    let status = run(mongo::connection_status()).await.ok();
    let topology = Topology::from_hello(&hello);
    let sessions = hello
        .get("logicalSessionTimeoutMinutes")
        .and_then(int_of)
        .is_some();
    let info = ServerInfo {
        version: catalog::server_version(&build),
        current_database: None,
        current_user: status.as_ref().and_then(catalog::current_user),
    };
    Ok((topology, sessions, info))
}

/// Shuts a client down without waiting for its operations, within `CLOSE_LIMIT`.
async fn shut_down(client: Client) {
    let _ = tokio::time::timeout(CLOSE_LIMIT, client.shutdown().immediate(true)).await;
}

/// What a MongoDB session offers, by what this driver implements and its tests prove.
/// `confirm`: how Safe Mode confirms a write (`commands::safety::confirms_with_passcode`).
pub fn capabilities(
    topology: &Topology,
    database: Option<&str>,
    confirm: SafeConfirm,
) -> SessionCaps {
    let mut caps = SessionCaps::minimal(Driver::Mongodb).with(&[
        Feature::Editor,
        Feature::CancelOnServer,
        Feature::EditRows,
        Feature::InsertRows,
        Feature::DeleteRows,
        Feature::ServerExpressions,
        Feature::ExactCount,
        Feature::ViewDefinition,
        Feature::SwitchDatabase,
        Feature::ProcessList,
        Feature::KillProcess,
    ]);
    caps.namespaces = NamespaceCaps {
        databases: true,
        schemas: SchemaMode::DatabaseIsSchema,
        default_schema: database.map(str::to_string),
        labels: NamespaceLabels {
            database: "Database".into(),
            schema: "Database".into(),
            table: "Collection".into(),
            tables: "Collections".into(),
        },
    };
    caps.data = DataCaps {
        paging: Paging::Offset,
        count: CountMode::Estimate,
        sort: true,
        filter_ops: FILTER_OPS.to_vec(),
        raw_filter: Some(RawFilter::Json),
        // One transaction on a replica set or mongos; one change after the other on a
        // standalone server, stopping at the first that fails.
        transactions: if topology.transactions() {
            TxMode::Atomic
        } else {
            TxMode::Sequential
        },
        affected: Affected::Exact,
        default_keyword: false,
        upsert: false,
        server_exprs: vec![ServerExpr::Now, ServerExpr::CurrentTimestamp],
        max_params: 0,
    };
    caps.safety = SafetyCaps {
        read_only: ReadOnlyMode::ClassifierOnly,
        hardened: false,
        confirm,
    };
    caps.session = LifecycleCaps {
        keepalive_secs: Some(KEEPALIVE_SECS),
        ping_billed: false,
    };
    caps
}

/// The message of a write whose reply counted another number of documents than expected.
fn unexpected_n(op: &str, n: Option<i64>) -> AppError {
    match (op, n) {
        ("insert", _) => AppError::db("the document was not inserted"),
        (_, Some(0)) => AppError::db(
            "no document matched: it was changed or deleted since it was loaded; reload and try again",
        ),
        (_, n) => AppError::db(format!(
            "the server reported {} documents where one was expected",
            n.map_or_else(|| "no count of".to_string(), |n| n.to_string())
        )),
    }
}

#[async_trait]
impl TableSource for MongoSession {
    async fn browse(
        &self,
        meta: &TableMeta,
        req: &BrowseRequest,
        cancel: CancelSlot,
    ) -> Result<BrowsePage, AppError> {
        let db = self.database_of(&meta.table)?;
        let coll = meta.table.name.as_str();
        let columns: Vec<_> = match &req.columns {
            Some(names) => meta
                .columns
                .iter()
                .filter(|c| c.name == ID || names.contains(&c.name))
                .cloned()
                .collect(),
            None => meta.columns.clone(),
        };
        let spec = FindSpec {
            filter: mongo::filter_document(req.filter.as_ref(), &meta.columns)?,
            sort: mongo::sort_document(&req.sort),
            projection: mongo::projection(req.columns.as_deref()),
            skip: req.offset,
            limit: req.limit,
        };
        let command = mongo::find(coll, &spec);
        let text = mongo::command_text(Some(&db), &command);
        let started = Instant::now();
        let limit = req.limit as usize;
        let result = self
            .retrying(|client| {
                let command = command.clone();
                let db = db.as_str();
                let cancel = &cancel;
                async move {
                    let tag = OpTag::new(self.start_session(&client).await?);
                    let read = async {
                        let mut docs = self
                            .open_cursor(&client, db, command, Run::Read, &tag, None)
                            .await?;
                        let mut out = Vec::with_capacity(limit.min(10_000));
                        while out.len() < limit {
                            match docs.next().await {
                                Some(d) => out.push(d?),
                                None => break,
                            }
                        }
                        Ok(out)
                    };
                    self.cancellable(cancel, &client, &tag, read).await
                }
            })
            .await;
        self.trace.finish(
            TraceCategory::Data,
            text.clone(),
            started,
            &result,
            |docs| TraceCounts::rows(docs.len()),
        );
        let docs = result?;
        self.record_loaded(&db, coll, &docs);
        let rows = docs
            .iter()
            .map(|d| {
                columns
                    .iter()
                    .map(|c| values::field_value(d.get(&c.name)))
                    .collect()
            })
            .collect();
        Ok(BrowsePage {
            columns,
            rows,
            pk_columns: meta.primary_key.clone(),
            identity: if meta.primary_key.is_empty() {
                RowIdentity::None
            } else {
                RowIdentity::Pk
            },
            row_ids: None,
            offset: req.offset,
            limit: req.limit,
            sql: text,
            duration_ms: started.elapsed().as_millis() as u64,
            next_cursor: None,
        })
    }

    /// Without a filter and unless asked to count, the collection's own count (metadata, kept
    /// by the server); otherwise the documents counted (`countDocuments`).
    async fn count(
        &self,
        meta: &TableMeta,
        filter: Option<&FilterGroup>,
        exact: bool,
        cancel: CancelSlot,
    ) -> Result<RowCount, AppError> {
        let db = self.database_of(&meta.table)?;
        let coll = meta.table.name.as_str();
        let exact = exact || filter.is_some();
        let command = if exact {
            mongo::count_documents(coll, mongo::filter_document(filter, &meta.columns)?)
        } else {
            mongo::estimated_count(coll)
        };
        let text = mongo::command_text(Some(&db), &command);
        let started = Instant::now();
        let result = self
            .retrying(|client| {
                let command = command.clone();
                let db = db.as_str();
                let cancel = &cancel;
                async move {
                    let tag = OpTag::new(None);
                    let command = mongo::with_comment(command, &tag.comment);
                    let run = self.run(&client, db, command, Run::Read, None);
                    self.cancellable(cancel, &client, &tag, run).await
                }
            })
            .await;
        self.trace
            .finish(TraceCategory::Data, text, started, &result, |_| {
                TraceCounts::rows(1)
            });
        let count = catalog::count_of(&result?)
            .ok_or_else(|| AppError::db("the server's count reply has no count"))?;
        Ok(RowCount { count, exact })
    }

    async fn fetch_cell(
        &self,
        meta: &TableMeta,
        key: &RowKey,
        column: &str,
    ) -> Result<Value, AppError> {
        let db = self.database_of(&meta.table)?;
        let id = mongo::key_id(key, &meta.columns)?;
        let command = mongo::fetch_field(&meta.table.name, id, column);
        let reply = self
            .traced(TraceCategory::Data, &db, command, Run::Read)
            .await?;
        let (docs, _) = catalog::cursor_batch(&reply);
        let doc = docs
            .first()
            .ok_or_else(|| AppError::NotFound("no document with this _id".into()))?;
        Ok(doc
            .get(column)
            .map_or_else(values::missing, values::to_value_full))
    }
}

#[async_trait]
impl NativeSession for MongoSession {
    fn driver(&self) -> Driver {
        Driver::Mongodb
    }

    fn capabilities(&self) -> SessionCaps {
        self.caps.clone()
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        Ok(self.info.clone())
    }

    /// `ping` on `admin`: free, and the server's cheapest command.
    /// Untraced, as the core drivers ping: a keep-alive every 30 s would fill the trace.
    async fn ping(&self) -> Result<(), AppError> {
        self.retrying(|client| async move {
            self.run(&client, "admin", mongo::ping(), Run::Read, None)
                .await
        })
        .await
        .map(|_| ())
    }

    async fn close(&self) {
        shut_down(self.client()).await;
    }

    async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError> {
        cancel_portable(handle).await.unwrap_or(Ok(()))
    }

    fn sql_trace(&self) -> Option<&SqlTrace> {
        Some(&self.trace)
    }

    /// The databases the user may use (`authorizedDatabases`); without even that, the
    /// session's own.
    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        let reply = self
            .traced(
                TraceCategory::Structure,
                "admin",
                mongo::list_databases(true),
                Run::Read,
            )
            .await;
        match reply {
            Ok(reply) => Ok(catalog::databases(&reply, self.database.as_deref())),
            Err(AppError::Db {
                vendor_code: Some(13),
                ..
            }) if self.database.is_some() => Ok(self
                .database
                .iter()
                .map(|name| DatabaseInfo {
                    name: name.clone(),
                    owner: None,
                    encoding: None,
                    is_current: true,
                })
                .collect()),
            Err(e) => Err(e),
        }
    }

    /// The session's database; without one, every database but the server's own.
    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        if let Some(db) = &self.database {
            return Ok(vec![SchemaInfo {
                name: db.clone(),
                is_default: true,
            }]);
        }
        let desc = Driver::Mongodb.descriptor();
        Ok(self
            .list_databases()
            .await?
            .into_iter()
            .filter(|d| !desc.is_system_schema(&d.name))
            .map(|d| SchemaInfo {
                name: d.name,
                is_default: false,
            })
            .collect())
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        let db = if schema.is_empty() {
            self.database_of(&TableRef::new(None, ""))?
        } else {
            schema.to_string()
        };
        let entries = match self
            .read_docs(
                TraceCategory::Structure,
                &db,
                mongo::list_collections(false),
                0,
            )
            .await
        {
            // Without the privilege to list collections with their options, their names.
            Err(AppError::Db {
                vendor_code: Some(13),
                ..
            }) => {
                self.read_docs(
                    TraceCategory::Structure,
                    &db,
                    mongo::list_collections(true),
                    0,
                )
                .await?
            }
            other => other?,
        };
        Ok(catalog::collections(&entries, &db))
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let db = self.database_of(table)?;
        let coll = table.name.as_str();
        let info = self.collection(&db, coll).await?;
        let sample = self.sample(&db, coll).await?;
        let fields = schema::infer(&sample);
        let columns = schema::columns(&fields, sample.len());
        let indexes = self.indexes(&db, coll).await;
        let estimate = if info.is_view() {
            None
        } else {
            self.read_docs(TraceCategory::Structure, &db, mongo::coll_stats(coll), 1)
                .await
                .ok()
                .and_then(|stats| stats.first().and_then(|s| catalog::coll_stats(s).0))
        };
        self.fields
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(db.clone())
            .or_default()
            .insert(coll.to_string(), schema::field_names(&fields));
        Ok(catalog::table_meta(&db, &info, columns, indexes, estimate))
    }

    /// The collection's create text with its options and validator, then its indexes.
    async fn definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        let db = self.database_of(table)?;
        let info = self.collection(&db, &table.name).await?;
        let indexes = self.indexes(&db, &table.name).await;
        Ok(Some(catalog::ddl(&info, &indexes)))
    }

    /// The collections of the session's database, with the fields sampled so far.
    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        let Some(db) = self.database.clone() else {
            return Ok(CompletionMetadata::default());
        };
        let names = self.list_tables(&db).await?;
        let sampled = self
            .fields
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&db)
            .cloned()
            .unwrap_or_default();
        let collections = names
            .into_iter()
            .map(|t| {
                let fields = sampled.get(&t.name).cloned().unwrap_or_default();
                (t.name, fields)
            })
            .collect();
        Ok(CompletionMetadata {
            default_schema: Some(db.clone()),
            schemas: BTreeMap::from([(db, collections)]),
        })
    }

    /// The commands of the staged changes by `_id`, each expecting `n: 1`; the preview is their
    /// text. Views, system collections and read-only collections have no `_id` key in their
    /// structure and are refused.
    fn plan_changes(&self, meta: &TableMeta, req: &ApplyRequest) -> Result<NativePlan, AppError> {
        if self.read_only {
            return Err(read_only_error());
        }
        if meta.primary_key.is_empty() {
            return Err(AppError::Unsupported(format!(
                "{} cannot be edited: views, system collections and read-only collections are \
                 read-only",
                meta.table.name
            )));
        }
        let mut table = meta.table.clone();
        let db = self.database_of(&table)?;
        table.schema = Some(db.clone());
        let loaded = self.loaded_types(&db, &table.name);
        let plan = mongo::plan_changes(&table, &meta.columns, &loaded, &req.changes)?;
        Ok(NativePlan {
            preview: plan.preview(self.database.as_deref()),
            ops: Box::new(plan),
        })
    }

    async fn apply_changes(
        &self,
        _meta: &TableMeta,
        req: &ApplyRequest,
        plan: NativePlan,
    ) -> ApplyResult {
        let sql = plan.preview.iter().map(|(_, t)| t.clone()).collect();
        let failed = |error: AppError| ApplyResult {
            success: false,
            results: Vec::new(),
            sql: Vec::new(),
            error: Some(error.to_payload()),
            duration_ms: 0,
        };
        if self.read_only {
            return ApplyResult {
                sql,
                ..failed(read_only_error())
            };
        }
        let Ok(plan) = plan.ops.downcast::<EditPlan>() else {
            return failed(AppError::Internal("not a MongoDB plan".into()));
        };
        let mut outcome = self.commit(&plan, req).await;
        // Refused at sign-in before anything was sent: once more with a fresh password.
        if let Commit::SignIn(generation) = outcome
            && self.renew(generation).await
        {
            outcome = self.commit(&plan, req).await;
        }
        let (results, error) = match outcome {
            Commit::Done(results, error) => (results, error.map(|e| *e)),
            Commit::SignIn(_) => {
                let error = AppError::Auth("the server refused the password".into());
                (Vec::new(), Some(error.to_payload()))
            }
        };
        ApplyResult {
            success: error.is_none(),
            results,
            sql,
            error,
            duration_ms: 0,
        }
    }

    fn split_console(&self, text: &str) -> Vec<SplitStatement> {
        super::shell::split(text)
    }

    fn classify_console(&self, command: &str) -> StatementClass {
        classify::classify_text(command)
    }

    async fn run_console(
        &self,
        command: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError> {
        super::console::run(self, command, max_rows, sink, &cancel).await
    }

    fn console_lock(&self) -> Option<&tokio::sync::Mutex<()>> {
        Some(&self.console)
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
        Some(super::calls::list_processes(self).await)
    }

    async fn kill_processes(&self, ids: &[String], scope: KillScope) -> Option<Vec<KillOutcome>> {
        Some(super::calls::kill_processes(self, ids, scope).await)
    }
}

/// How a commit went: each change's result and the error, or a sign-in refused before any
/// command was sent (with the client's generation).
enum Commit {
    Done(Vec<RowResult>, Option<Box<ErrorPayload>>),
    SignIn(u64),
}

impl MongoSession {
    /// Applies a plan: in one transaction on a replica set or mongos (a failure undoes every
    /// change), else one change after the other, stopping at the first that fails.
    async fn commit(&self, plan: &EditPlan, req: &ApplyRequest) -> Commit {
        let live = self.live();
        let client = live.client.clone();
        let db = plan
            .db
            .clone()
            .or_else(|| self.database.clone())
            .unwrap_or_default();
        let atomic = self.topology.transactions();
        let mut session = match self.start_session(&client).await {
            Ok(s) => s,
            Err(e) => return Commit::Done(Vec::new(), Some(Box::new(e.to_payload()))),
        };
        if atomic {
            let started = match session.as_mut() {
                Some(s) => s
                    .start_transaction()
                    .await
                    .map_err(|e| errors::from_driver(&e)),
                None => Err(AppError::Unsupported(
                    "MongoDB: this server has no sessions, so no transactions".into(),
                )),
            };
            if let Err(e) = started {
                return Commit::Done(Vec::new(), Some(Box::new(e.to_payload())));
            }
        }
        let mut done: Vec<u64> = Vec::with_capacity(plan.ops.len());
        let mut failure: Option<(usize, AppError)> = None;
        for (i, op) in plan.ops.iter().enumerate() {
            let text = mongo::command_text(Some(&db), &op.command);
            let started = Instant::now();
            let reply = self
                .run(
                    &client,
                    &db,
                    op.command.clone(),
                    Run::Write,
                    session.as_mut(),
                )
                .await;
            let outcome = reply.and_then(|reply| match errors::from_reply(&reply) {
                Some(e) => Err(e),
                None => {
                    let n = catalog::reply_n(&reply);
                    if n == Some(op.expect_n) {
                        Ok(op.expect_n as u64)
                    } else {
                        Err(unexpected_n(op.op, n))
                    }
                }
            });
            self.trace
                .finish(TraceCategory::Data, text, started, &outcome, |n| {
                    TraceCounts {
                        rows: None,
                        affected: Some(*n),
                    }
                });
            match outcome {
                Ok(n) => done.push(n),
                Err(AppError::Auth(_)) if i == 0 => {
                    if let Some(s) = session.as_mut().filter(|_| atomic) {
                        let _ = s.abort_transaction().await;
                    }
                    return Commit::SignIn(live.generation);
                }
                Err(e) => {
                    failure = Some((i, e));
                    break;
                }
            }
        }
        if atomic && let Some(s) = session.as_mut() {
            let ended = if failure.is_some() {
                s.abort_transaction().await
            } else {
                s.commit_transaction().await
            };
            if let (Err(e), None) = (ended, &failure) {
                let error = Box::new(commit_failed(&e));
                return Commit::Done(results(plan, req, &[], None, atomic), Some(error));
            }
        }
        match failure {
            None => Commit::Done(results(plan, req, &done, None, atomic), None),
            Some((at, e)) => {
                let payload = e.to_payload();
                let results = results(plan, req, &done, Some((at, &payload)), atomic);
                Commit::Done(results, Some(Box::new(payload)))
            }
        }
    }
}

/// The error of a `commitTransaction` that failed. Only a transient transaction error says that
/// the server aborted it (nothing applied, every change stays staged); after any other one (a
/// lost reply, a write concern error) the changes may have been applied, and committing them
/// again would apply them twice (an insert without an `_id` gets a new one).
fn commit_failed(e: &::mongodb::error::Error) -> ErrorPayload {
    let mut payload = errors::from_driver(e).to_payload();
    if e.contains_label(TRANSIENT_TRANSACTION_ERROR)
        && !e.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT)
    {
        return payload;
    }
    payload.message = format!(
        "The commit's outcome is unknown, its changes may have been applied: {}",
        payload.message
    );
    payload.hint = Some(
        "Refresh the data before committing again: an insert without an _id would be added a \
         second time"
            .into(),
    );
    payload
}

/// One result per change: the applied ones with their count; on a failure, the failed one with
/// its error and, one after the other, those after it "not applied" (in a transaction nothing
/// was applied and only the failed one carries an error).
fn results(
    plan: &EditPlan,
    req: &ApplyRequest,
    done: &[u64],
    failed: Option<(usize, &ErrorPayload)>,
    atomic: bool,
) -> Vec<RowResult> {
    let not_sent =
        AppError::InvalidInput("not applied: an earlier change failed".into()).to_payload();
    plan.ops
        .iter()
        .enumerate()
        .map(|(i, op)| {
            let change = req.changes.get(op.change);
            let error = match failed {
                Some((at, payload)) if i == at => Some(payload.clone()),
                Some((at, _)) if i > at && !atomic => Some(not_sent.clone()),
                _ => None,
            };
            let applied = failed.is_none() || !atomic;
            RowResult {
                row_id: change.map_or_else(|| op.row_id.clone(), |c| c.row_id().to_string()),
                op: op.op.to_string(),
                affected: if applied {
                    done.get(i).copied().unwrap_or(0)
                } else {
                    0
                },
                returned: None,
                error,
            }
        })
        .collect()
}
