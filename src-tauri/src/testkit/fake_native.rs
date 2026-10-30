//! A native store that reaches no server: two collections of documents (`orders`, `users`) in
//! memory, keyed by `_id` and paged by cursor, with a line-per-command console (whose `wait`
//! runs until its `CancelHandle::Custom` fires), commits applied like a MULTI … EXEC batch, and
//! one read and one write call. Like `FakeSession` it records every call that would reach a
//! server in its `SqlTrace`, so a test can prove that a refused command sent nothing.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::future::BoxFuture;
use serde_json::json;

use crate::drivers::native::{CallKind, NativePlan, NativeSession, TableSource, unknown_call};
use crate::drivers::{
    CancelAction, CancelHandle, CancelSlot, RowSink, ServerInfo, SqlTrace, StatementOutcome,
    arm_cancel, cancel_portable,
};
use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::split::SplitStatement;
use crate::types::admin::{KillOutcome, KillScope, ProcessRow};
use crate::types::browse::{
    BrowsePage, BrowseRequest, FilterGroup, RowCount, RowIdentity, RowKey, TraceCategory, TracedSql,
};
use crate::types::caps::SessionCaps;
use crate::types::mutate::{ApplyRequest, ApplyResult, RowResult, StagedChange};
use crate::types::profile::Driver;
use crate::types::query::{ResultColumn, StatementKind};
use crate::types::schema::{
    ColumnMeta, DatabaseInfo, SchemaInfo, TableKind, TableMeta, TableRef, TableSummary, ValueKind,
};
use crate::types::value::Value;

type Doc = BTreeMap<String, Value>;

pub struct FakeStore {
    caps: SessionCaps,
    trace: SqlTrace,
    collections: Mutex<BTreeMap<String, Vec<Doc>>>,
    closed: AtomicBool,
    wake: Arc<Wake>,
}

/// What the console's `wait` waits for: its cancel request.
#[derive(Default)]
struct Wake {
    fired: AtomicUsize,
    notify: tokio::sync::Notify,
}

/// The store's own cancel request (`CancelHandle::Custom`), as a driver whose server has no
/// cancel of its own would send one.
struct FakeCancel(Arc<Wake>);

impl CancelAction for FakeCancel {
    fn fire(&self) -> BoxFuture<'static, Result<(), AppError>> {
        self.0.fired.fetch_add(1, Ordering::SeqCst);
        self.0.notify.notify_one();
        Box::pin(async { Ok(()) })
    }

    fn label(&self) -> &'static str {
        "fake cancel"
    }
}

fn doc(id: i64, name: &str) -> Doc {
    BTreeMap::from([
        ("_id".to_string(), Value::Int(id)),
        ("name".to_string(), Value::Text(name.to_string())),
    ])
}

fn column(name: &str, ordinal: u32, kind: ValueKind, is_pk: bool) -> ColumnMeta {
    serde_json::from_value(json!({
        "name": name, "ordinal": ordinal, "dataType": format!("{kind:?}").to_lowercase(),
        "kind": kind, "nullable": !is_pk, "defaultExpr": null, "isPk": is_pk,
        "pkOrdinal": is_pk.then_some(1), "isAutoIncrement": false, "isGenerated": false,
        "isUnsigned": false, "comment": null, "enumValues": null, "charMaxLen": null,
        "numericPrecision": null, "numericScale": null, "collation": null, "arrayElemType": null,
        "extra": null,
    }))
    .expect("a column")
}

fn id_of(key: &RowKey) -> Option<&Value> {
    match key {
        RowKey::Pk(values) => values.get("_id"),
        RowKey::Rowid(_) | RowKey::AllColumns(_) => None,
    }
}

impl FakeStore {
    /// A store of `caps.driver` with these caps: `orders` holds 3 documents, `users` 2.
    pub fn new(caps: SessionCaps) -> FakeStore {
        let collections = BTreeMap::from([
            (
                "orders".to_string(),
                vec![doc(1, "first"), doc(2, "second"), doc(3, "third")],
            ),
            ("users".to_string(), vec![doc(1, "ada"), doc(2, "bob")]),
        ]);
        FakeStore {
            caps,
            trace: SqlTrace::default(),
            collections: Mutex::new(collections),
            closed: AtomicBool::new(false),
            wake: Arc::default(),
        }
    }

    /// How many cancel requests the console's `wait` received.
    pub fn cancels_fired(&self) -> usize {
        self.wake.fired.load(Ordering::SeqCst)
    }

    /// What reached the "server", oldest first (the trace is not drained).
    pub fn calls(&self) -> Vec<String> {
        let calls = self.trace.drain();
        for call in &calls {
            self.trace.record(call.clone());
        }
        calls.into_iter().map(|c| c.sql).collect()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// The documents of `collection`, in `_id` order.
    pub fn docs(&self, collection: &str) -> Vec<Doc> {
        self.collections
            .lock()
            .unwrap()
            .get(collection)
            .cloned()
            .unwrap_or_default()
    }

    fn touch(&self, what: impl Into<String>) {
        self.trace.record(TracedSql {
            sql: what.into(),
            category: TraceCategory::Data,
            duration_ms: 0,
            rows: None,
            affected: None,
            error: None,
        });
    }

    fn open(&self) -> Result<(), AppError> {
        if self.is_closed() {
            Err(AppError::db("the fake store is closed"))
        } else {
            Ok(())
        }
    }

    fn with_docs<T>(
        &self,
        collection: &str,
        f: impl FnOnce(&mut Vec<Doc>) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let mut collections = self.collections.lock().unwrap();
        let docs = collections
            .get_mut(collection)
            .ok_or_else(|| AppError::NotFound(collection.to_string()))?;
        f(docs)
    }

    fn apply_one(&self, collection: &str, change: &StagedChange) -> Result<u64, AppError> {
        self.with_docs(collection, |docs| match change {
            StagedChange::Update { key, changes, .. } => {
                let doc = docs
                    .iter_mut()
                    .find(|d| id_of(key).is_some_and(|id| d.get("_id") == Some(id)))
                    .ok_or_else(|| AppError::NotFound("no document with this _id".into()))?;
                doc.extend(changes.clone());
                Ok(1)
            }
            StagedChange::Insert { values, .. } => {
                if docs.iter().any(|d| d.get("_id") == values.get("_id")) {
                    return Err(AppError::db("duplicate _id"));
                }
                docs.push(values.clone());
                Ok(1)
            }
            StagedChange::Delete { key, .. } => {
                let before = docs.len();
                docs.retain(|d| id_of(key).is_none_or(|id| d.get("_id") != Some(id)));
                match before - docs.len() {
                    0 => Err(AppError::NotFound("no document with this _id".into())),
                    n => Ok(n as u64),
                }
            }
        })
    }
}

#[async_trait]
impl TableSource for FakeStore {
    async fn browse(
        &self,
        meta: &TableMeta,
        req: &BrowseRequest,
        _cancel: CancelSlot,
    ) -> Result<BrowsePage, AppError> {
        self.touch(format!("browse {}", meta.table.name));
        self.open()?;
        // The cursor is where the page starts; without one the offset is.
        let start = match &req.cursor {
            Some(cursor) => cursor
                .parse()
                .map_err(|_| AppError::InvalidInput(format!("not a cursor: {cursor}")))?,
            None => req.offset as usize,
        };
        let docs = self.docs(&meta.table.name);
        let end = docs.len().min(start.saturating_add(req.limit as usize));
        let rows = docs
            .iter()
            .take(end)
            .skip(start)
            .map(|d| {
                meta.columns
                    .iter()
                    .map(|c| d.get(&c.name).cloned().unwrap_or(Value::Null))
                    .collect()
            })
            .collect();
        Ok(BrowsePage {
            columns: meta.columns.clone(),
            rows,
            pk_columns: meta.primary_key.clone(),
            identity: RowIdentity::Pk,
            row_ids: None,
            offset: req.offset,
            limit: req.limit,
            sql: format!("find {}", meta.table.name),
            duration_ms: 0,
            next_cursor: (end < docs.len()).then(|| end.to_string()),
        })
    }

    async fn count(
        &self,
        meta: &TableMeta,
        _filter: Option<&FilterGroup>,
        _exact: bool,
        _cancel: CancelSlot,
    ) -> Result<RowCount, AppError> {
        self.touch(format!("count {}", meta.table.name));
        self.open()?;
        Ok(RowCount {
            count: self.docs(&meta.table.name).len() as i64,
            exact: true,
        })
    }

    async fn fetch_cell(
        &self,
        meta: &TableMeta,
        key: &RowKey,
        column: &str,
    ) -> Result<Value, AppError> {
        self.touch(format!("fetch {} {column}", meta.table.name));
        self.open()?;
        self.docs(&meta.table.name)
            .into_iter()
            .find(|d| id_of(key).is_some_and(|id| d.get("_id") == Some(id)))
            .and_then(|mut d| d.remove(column))
            .ok_or_else(|| AppError::NotFound(column.to_string()))
    }
}

#[async_trait]
impl NativeSession for FakeStore {
    fn driver(&self) -> Driver {
        self.caps.driver
    }

    fn capabilities(&self) -> SessionCaps {
        self.caps.clone()
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        self.touch("server_info");
        self.open()?;
        Ok(ServerInfo {
            version: "fake 1.0".into(),
            current_database: Some("0".into()),
            current_user: None,
        })
    }

    async fn ping(&self) -> Result<(), AppError> {
        self.touch("ping");
        self.open()
    }

    async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError> {
        self.touch("cancel");
        cancel_portable(handle).await.unwrap_or(Ok(()))
    }

    fn sql_trace(&self) -> Option<&SqlTrace> {
        Some(&self.trace)
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        self.touch("list_databases");
        self.open()?;
        Ok(vec![DatabaseInfo {
            name: "0".into(),
            owner: None,
            encoding: None,
            is_current: true,
        }])
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        self.touch("list_schemas");
        self.open()?;
        Ok(Vec::new())
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        self.touch(format!("list_tables {schema}"));
        self.open()?;
        let collections = self.collections.lock().unwrap();
        Ok(collections
            .iter()
            .map(|(name, docs)| TableSummary {
                name: name.clone(),
                schema: None,
                kind: TableKind::Table,
                row_estimate: Some(docs.len() as i64),
                size_bytes: None,
                comment: None,
            })
            .collect())
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        self.touch(format!("table_meta {}", table.name));
        self.open()?;
        let rows = self.with_docs(&table.name, |docs| Ok(docs.len() as i64))?;
        Ok(TableMeta {
            table: table.clone(),
            kind: TableKind::Table,
            columns: vec![
                column("_id", 1, ValueKind::Int, true),
                column("name", 2, ValueKind::Text, false),
            ],
            primary_key: vec!["_id".into()],
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
            triggers: Vec::new(),
            comment: None,
            row_estimate: Some(rows),
            ddl: None,
            checks: Vec::new(),
            options: None,
        })
    }

    async fn definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        self.touch(format!("definition {}", table.name));
        self.open()?;
        Ok(Some(format!("{{ \"collection\": \"{}\" }}", table.name)))
    }

    /// One line of preview per change, from the same plan `apply_changes` runs.
    fn plan_changes(&self, meta: &TableMeta, req: &ApplyRequest) -> Result<NativePlan, AppError> {
        let preview = req
            .changes
            .iter()
            .enumerate()
            .map(|(i, c)| {
                (
                    i,
                    format!("{} {} {}", c.op_name(), meta.table.name, c.row_id()),
                )
            })
            .collect();
        Ok(NativePlan {
            preview,
            ops: Box::new(req.changes.clone()),
        })
    }

    /// Applies the changes as one MULTI … EXEC batch: one that fails keeps its error, the others
    /// are applied.
    async fn apply_changes(
        &self,
        meta: &TableMeta,
        _req: &ApplyRequest,
        plan: NativePlan,
    ) -> ApplyResult {
        let changes = plan
            .ops
            .downcast::<Vec<StagedChange>>()
            .map(|b| *b)
            .unwrap_or_default();
        self.touch("MULTI");
        let results: Vec<RowResult> = changes
            .iter()
            .map(|change| {
                self.touch(format!("{} {}", change.op_name(), change.row_id()));
                let outcome = self
                    .open()
                    .and_then(|()| self.apply_one(&meta.table.name, change));
                RowResult {
                    row_id: change.row_id().to_string(),
                    op: change.op_name().to_string(),
                    affected: *outcome.as_ref().unwrap_or(&0),
                    returned: None,
                    error: outcome.err().map(|e| e.to_payload()),
                }
            })
            .collect();
        self.touch("EXEC");
        let error = results.iter().find_map(|r| r.error.clone());
        ApplyResult {
            success: error.is_none(),
            results,
            sql: plan.preview.into_iter().map(|(_, text)| text).collect(),
            error,
            duration_ms: 0,
        }
    }

    /// A command per non-empty line.
    fn split_console(&self, text: &str) -> Vec<SplitStatement> {
        let mut out = Vec::new();
        let mut start = 0;
        for (i, line) in text.split('\n').enumerate() {
            let len = line.chars().count();
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let lead = line.chars().take_while(|c| c.is_whitespace()).count();
                out.push(SplitStatement {
                    sql: trimmed.to_string(),
                    start: start + lead,
                    end: start + lead + trimmed.chars().count(),
                    line: i as u32 + 1,
                });
            }
            start += len + 1;
        }
        out
    }

    /// `find`, `count` and `wait` read; everything else (`del`, unknown commands) writes.
    fn classify_console(&self, command: &str) -> StatementClass {
        match command.split_whitespace().next() {
            Some("find" | "count" | "wait") => StatementClass::Read,
            _ => StatementClass::Write,
        }
    }

    /// `find <collection>` streams its documents, `count <collection>` gives one row,
    /// `del <collection> <_id>` deletes a document and `wait` runs until it is cancelled.
    async fn run_console(
        &self,
        command: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError> {
        self.touch(command);
        self.open()?;
        let words: Vec<&str> = command.split_whitespace().collect();
        let result_column = |name: &str, kind: ValueKind| ResultColumn {
            name: name.into(),
            data_type: format!("{kind:?}").to_lowercase(),
            kind,
            nullable: None,
            table: None,
            origin: None,
        };
        match words.as_slice() {
            ["find", collection] => {
                let docs = self.with_docs(collection, |docs| Ok(docs.clone()))?;
                sink.begin_set(vec![
                    result_column("_id", ValueKind::Int),
                    result_column("name", ValueKind::Text),
                ]);
                let rows: Vec<Vec<Value>> = docs
                    .iter()
                    .take(max_rows)
                    .map(|d| vec![d["_id"].clone(), d["name"].clone()])
                    .collect();
                let row_count = rows.len() as u64;
                // One chunk: there is nothing after it for a sink that stops early to skip.
                let _ = sink.rows(rows);
                Ok(StatementOutcome {
                    kind: StatementKind::Rows,
                    result_sets: 1,
                    row_count,
                    truncated: docs.len() > max_rows,
                    ..StatementOutcome::default()
                })
            }
            ["count", collection] => {
                let n = self.with_docs(collection, |docs| Ok(docs.len() as i64))?;
                sink.begin_set(vec![result_column("count", ValueKind::Int)]);
                let _ = sink.rows(vec![vec![Value::Int(n)]]);
                Ok(StatementOutcome {
                    kind: StatementKind::Rows,
                    result_sets: 1,
                    row_count: 1,
                    ..StatementOutcome::default()
                })
            }
            ["wait"] => {
                let action = Arc::new(FakeCancel(self.wake.clone()));
                let _armed = arm_cancel(&cancel, CancelHandle::Custom(action))?;
                self.wake.notify.notified().await;
                Err(AppError::Cancelled)
            }
            ["del", collection, id] => {
                let id: i64 = id
                    .parse()
                    .map_err(|_| AppError::InvalidInput(format!("not an _id: {id}")))?;
                let key = RowKey::Pk(BTreeMap::from([("_id".to_string(), Value::Int(id))]));
                let change = StagedChange::Delete {
                    row_id: id.to_string(),
                    key,
                };
                let affected = self.apply_one(collection, &change)?;
                Ok(StatementOutcome {
                    kind: StatementKind::Affected,
                    affected: Some(affected),
                    ..StatementOutcome::default()
                })
            }
            _ => Err(AppError::InvalidInput(format!(
                "unknown command \u{201c}{command}\u{201d}"
            ))),
        }
    }

    /// `stats` reads (the documents per collection); `flush` empties every collection.
    async fn call(
        &self,
        method: &str,
        _args: serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        self.touch(format!("call {method}"));
        self.open()?;
        let mut collections = self.collections.lock().unwrap();
        match method {
            "stats" => Ok(json!(
                collections
                    .iter()
                    .map(|(name, docs)| (name.clone(), json!(docs.len())))
                    .collect::<serde_json::Map<_, _>>()
            )),
            "flush" => {
                collections.values_mut().for_each(Vec::clear);
                Ok(serde_json::Value::Null)
            }
            other => Err(unknown_call(self.driver(), other)),
        }
    }

    fn call_kind(&self, method: &str) -> CallKind {
        match method {
            "stats" => CallKind::Read,
            _ => CallKind::Write,
        }
    }

    /// One task, `task:1`, running `wait`.
    async fn list_processes(&self) -> Option<Result<Vec<ProcessRow>, AppError>> {
        self.touch("list_processes");
        Some(self.open().map(|()| {
            vec![ProcessRow {
                id: "task:1".into(),
                user: None,
                database: None,
                application: Some("tabler".into()),
                client: None,
                state: Some("running".into()),
                command: None,
                started_at: None,
                duration_ms: Some(5),
                wait_event: None,
                query: Some("wait".into()),
                current: false,
                own: true,
            }]
        }))
    }

    /// Stops `task:1`; any other id is not running.
    async fn kill_processes(&self, ids: &[String], scope: KillScope) -> Option<Vec<KillOutcome>> {
        self.touch(format!("kill {scope:?} {}", ids.join(" ")));
        let outcome = |id: &String| KillOutcome {
            id: id.clone(),
            ok: id == "task:1",
            message: (id != "task:1").then(|| format!("no task {id}")),
        };
        Some(ids.iter().map(outcome).collect())
    }
}

#[cfg(test)]
mod tests {
    use std::ops::ControlFlow;

    use super::*;

    #[derive(Default)]
    struct Rows(Vec<Vec<Value>>);

    impl RowSink for Rows {
        fn begin_set(&mut self, _columns: Vec<ResultColumn>) {}
        fn rows(&mut self, rows: Vec<Vec<Value>>) -> ControlFlow<()> {
            self.0.extend(rows);
            ControlFlow::Continue(())
        }
    }

    fn store() -> FakeStore {
        FakeStore::new(SessionCaps::minimal(Driver::Redis))
    }

    /// The preview and the applied changes come from one plan; a change that fails keeps its
    /// error and the others are applied.
    #[tokio::test]
    async fn applies_a_plan_change_by_change() {
        let store = store();
        let meta = store
            .table_meta(&TableRef::new(None, "users"))
            .await
            .unwrap();
        let int = |n: i64| json!(Value::Int(n));
        let text = |s: &str| json!(Value::Text(s.into()));
        let req: ApplyRequest = serde_json::from_value(json!({
            "table": meta.table,
            "changes": [
                { "op": "update", "rowId": "a", "key": { "kind": "pk", "value": { "_id": int(1) } },
                  "changes": { "name": text("ada lovelace") } },
                { "op": "delete", "rowId": "b", "key": { "kind": "pk", "value": { "_id": int(9) } } },
                { "op": "insert", "rowId": "c", "values": { "_id": int(3), "name": text("cy") } },
            ],
        }))
        .unwrap();
        let plan = store.plan_changes(&meta, &req).unwrap();
        assert_eq!(
            plan.preview,
            [
                (0, "update users a".to_string()),
                (1, "delete users b".to_string()),
                (2, "insert users c".to_string()),
            ]
        );
        let result = store.apply_changes(&meta, &req, plan).await;
        assert!(!result.success);
        let errors: Vec<bool> = result.results.iter().map(|r| r.error.is_some()).collect();
        assert_eq!(errors, [false, true, false]);
        assert_eq!(result.sql.len(), 3);
        let names: Vec<Value> = store
            .docs("users")
            .into_iter()
            .map(|d| d["name"].clone())
            .collect();
        assert_eq!(
            names,
            [
                Value::Text("ada lovelace".into()),
                Value::Text("bob".into()),
                Value::Text("cy".into())
            ]
        );
    }

    #[tokio::test]
    async fn runs_console_commands_line_by_line() {
        let store = store();
        let split = store.split_console("find users\n\n  del users 2\n");
        let commands: Vec<(&str, usize, usize, u32)> = split
            .iter()
            .map(|s| (s.sql.as_str(), s.start, s.end, s.line))
            .collect();
        assert_eq!(
            commands,
            [("find users", 0, 10, 1), ("del users 2", 14, 25, 3)]
        );
        assert_eq!(store.classify_console("find users"), StatementClass::Read);
        assert_eq!(store.classify_console("del users 2"), StatementClass::Write);
        assert_eq!(store.classify_console("whatever"), StatementClass::Write);

        let mut rows = Rows::default();
        let found = store
            .run_console("find users", 1, &mut rows, CancelSlot::default())
            .await
            .unwrap();
        assert_eq!((found.row_count, found.truncated), (1, true));
        assert_eq!(rows.0, [[Value::Int(1), Value::Text("ada".into())]]);
        let deleted = store
            .run_console("del users 2", 10, &mut rows, CancelSlot::default())
            .await
            .unwrap();
        assert_eq!(deleted.affected, Some(1));
        assert_eq!(store.docs("users").len(), 1);
        assert!(
            store
                .run_console("drop everything", 10, &mut rows, CancelSlot::default())
                .await
                .is_err()
        );
        assert_eq!(
            store.calls(),
            ["find users", "del users 2", "drop everything"]
        );
    }
}
