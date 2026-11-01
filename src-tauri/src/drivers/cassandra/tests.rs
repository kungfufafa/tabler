//! The session over a scripted node (`fake`): connect, introspection, the grid's pages, counts
//! and cells, edits as lightweight transactions, the editor and its cqlsh commands, cancel, the
//! query time out and read-only.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use ::scylla::frame::types::Consistency;

use super::entry::{confirm_of, dial};
use super::fake::{FakeConnector, FakeWire, KvNode, kv_wire, rows};
use super::session::{CassandraSession, settings_of};
use super::testing::t;
use crate::conn_options::effective_read_only;
use crate::drivers::native::TableSource;
use crate::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, TxStatement, request_cancel,
};
use crate::error::AppError;
use crate::sqlgen::narrow::Narrowing;
use crate::sqlgen::sql::{TableTarget, change_on};
use crate::types::browse::{
    BrowseRequest, Conjunction, FilterGroup, FilterNode, FilterOp, RowIdentity, RowKey,
};
use crate::types::caps::{CountMode, Feature, Paging, TxMode};
use crate::types::driver::ReadOnlyMode;
use crate::types::mutate::StagedChange;
use crate::types::profile::ConnectionProfile;
use crate::types::query::StatementKind;
use crate::types::schema::{TableKind, TableMeta, TableRef};
use crate::types::value::Value;

fn profile(json: serde_json::Value) -> ConnectionProfile {
    let mut base = serde_json::json!({
        "id": "6f9619ff-8b86-d011-b42d-00c04fc964ff", "name": "cass", "color": null,
        "group": null, "driver": "cassandra", "host": "127.0.0.1", "port": 9042,
        "user": "cassandra", "database": "shop", "sslCaPath": null, "sqlitePath": null,
        "ssh": null, "lastConnectedAt": null
    });
    for (k, v) in json.as_object().expect("fields") {
        base[k] = v.clone();
    }
    serde_json::from_value(base).expect("a profile")
}

struct Opened {
    session: CassandraSession,
    wire: Arc<FakeWire>,
    connector: FakeConnector,
}

async fn open_on(node: Arc<KvNode>, json: serde_json::Value) -> Result<Opened, AppError> {
    let p = profile(json);
    let target = ConnectTarget {
        profile: &p,
        password: Some("secret"),
        host_override: None,
    };
    let (dial, params) = dial(&target)?;
    let wire = kv_wire(node);
    let connector = FakeConnector::new(wire.clone());
    let session = CassandraSession::open(
        &connector,
        &dial,
        params,
        effective_read_only(&p),
        confirm_of(&p),
        p.options.startup_sql.as_deref(),
    )
    .await?;
    Ok(Opened {
        session,
        wire,
        connector,
    })
}

async fn open(node: Arc<KvNode>) -> Opened {
    open_on(node, serde_json::json!({})).await.expect("opens")
}

fn kv() -> TableRef {
    TableRef::new(Some("shop"), "kv")
}

async fn kv_meta(s: &CassandraSession) -> TableMeta {
    s.table_meta(&kv()).await.expect("kv")
}

fn page_request(limit: u32, cursor: Option<String>) -> BrowseRequest {
    BrowseRequest {
        table: kv(),
        columns: None,
        filter: None,
        sort: Vec::new(),
        limit,
        offset: 0,
        cursor,
    }
}

fn int_key(id: i64) -> RowKey {
    RowKey::Pk(BTreeMap::from([
        ("id".to_string(), Value::Int(id)),
        ("seq".to_string(), Value::Int(0)),
    ]))
}

fn condition(column: &str, op: FilterOp, values: Vec<Value>) -> FilterNode {
    FilterNode::Condition {
        column: column.into(),
        op,
        values,
    }
}

#[tokio::test]
async fn connects_with_the_profile_and_reads_who_the_node_is() {
    let o = open(KvNode::new(3)).await;
    let dials = o.connector.dials.lock().unwrap().clone();
    assert_eq!(dials.len(), 1);
    assert_eq!(dials[0].keyspace.as_deref(), Some("shop"));
    assert_eq!(
        dials[0].credentials.as_ref().map(|c| c.user.as_str()),
        Some("cassandra")
    );
    let info = o.session.server_info().await.unwrap();
    assert_eq!(info.version, "Cassandra 4.1.5");
    assert_eq!(info.current_database.as_deref(), Some("shop"));
    assert_eq!(info.current_user.as_deref(), Some("cassandra"));
    o.session.ping().await.unwrap();

    let scylla = open(KvNode::with(1, true)).await;
    assert_eq!(
        scylla.session.server_info().await.unwrap().version,
        "ScyllaDB 5.4.0-0.20231108"
    );

    let refused = open_on(KvNode::new(1), serde_json::json!({ "user": "intruder" })).await;
    assert!(matches!(refused, Err(AppError::Auth(_))));

    // Closed, the session sends nothing more.
    o.session.close().await;
    assert!(matches!(
        o.session.ping().await,
        Err(AppError::NotConnected)
    ));
    assert!(matches!(
        o.session.list_databases().await,
        Err(AppError::NotConnected)
    ));
}

#[tokio::test]
async fn lists_keyspaces_tables_and_their_structure() {
    let o = open(KvNode::new(3)).await;
    let s = &o.session;
    let dbs = s.list_databases().await.unwrap();
    let names: Vec<&str> = dbs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["Other", "shop"], "system keyspaces are hidden");
    assert!(dbs[1].is_current && !dbs[0].is_current);
    let schemas = s.list_schemas().await.unwrap();
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].name, "shop");
    let tables = s.list_tables("shop").await.unwrap();
    let kinds: Vec<(&str, TableKind)> = tables.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    assert_eq!(
        kinds,
        [
            ("events", TableKind::Table),
            ("hits", TableKind::Table),
            ("kv", TableKind::Table),
            ("kv_by_val", TableKind::MaterializedView)
        ]
    );
    // The catalog binds the keyspace, never formats it into the text.
    let seen = o.wire.seen();
    let tables_query = seen
        .iter()
        .find(|q| q.cql == super::catalog::TABLES)
        .unwrap();
    assert_eq!(tables_query.params, [Value::Text("shop".into())]);

    let events = s
        .table_meta(&TableRef::new(Some("shop"), "events"))
        .await
        .unwrap();
    assert_eq!(events.primary_key, ["user_id", "bucket", "ts", "seq"]);
    assert_eq!(events.row_estimate, Some(200));
    assert_eq!(events.indexes.len(), 1);
    let ddl = events.ddl.clone().unwrap();
    assert!(ddl.starts_with("CREATE TABLE \"shop\".\"events\""), "{ddl}");
    // Show DDL: rebuilt, since this node has no server DESCRIBE.
    let shown = s
        .table_ddl(&TableRef::new(Some("shop"), "events"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(shown, ddl);
    // A table without a keyspace is the session's.
    let meta = s.table_meta(&TableRef::new(None, "kv")).await.unwrap();
    assert_eq!(meta.table.schema.as_deref(), Some("shop"));
    assert_eq!(meta.primary_key, ["id", "seq"]);
    assert!(
        s.view_definition(&kv()).await.unwrap().is_none(),
        "a table has no view definition"
    );
    let view = s
        .view_definition(&TableRef::new(Some("shop"), "kv_by_val"))
        .await
        .unwrap()
        .unwrap();
    assert!(
        view.starts_with("CREATE MATERIALIZED VIEW \"shop\".\"kv_by_val\""),
        "{view}"
    );
    assert!(matches!(
        s.table_meta(&TableRef::new(Some("shop"), "nope")).await,
        Err(AppError::NotFound(_))
    ));
    let completion = s.completion_metadata().await.unwrap();
    assert_eq!(completion.schemas["shop"]["kv"], ["id", "val"]);
}

#[tokio::test]
async fn browses_page_after_page_with_the_paging_state() {
    let o = open(KvNode::new(5)).await;
    let s = &o.session;
    let meta = kv_meta(s).await;
    o.wire.clear();
    let first = s
        .browse(&meta, &page_request(2, None), CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(first.rows.len(), 2);
    assert_eq!(
        first.rows[0],
        [Value::Int(1), Value::Int(0), Value::Text("v1".into())]
    );
    assert_eq!(first.identity, RowIdentity::Pk);
    assert_eq!(first.pk_columns, ["id", "seq"]);
    assert_eq!(
        first.sql,
        "SELECT \"id\", \"seq\", \"val\" FROM \"shop\".\"kv\""
    );
    // No LIMIT: the page size is the driver's, so the server's paging state carries on.
    let sent = o.wire.seen();
    assert_eq!(sent[0].page.as_ref().map(|p| p.1), Some(2));
    let cursor = first.next_cursor.clone().expect("a next page");
    let second = s
        .browse(
            &meta,
            &page_request(2, Some(cursor.clone())),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(second.rows[0][0], Value::Int(3));
    let third = s
        .browse(
            &meta,
            &page_request(2, second.next_cursor.clone()),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(third.rows.len(), 1);
    assert_eq!(third.next_cursor, None, "the last page has no next");

    // A cursor of another query (the filter changed) is refused, not followed.
    let mut filtered = page_request(2, Some(cursor));
    filtered.filter = Some(FilterGroup {
        conjunction: Conjunction::And,
        items: vec![condition(
            "val",
            FilterOp::Eq,
            vec![Value::Text("v4".into())],
        )],
    });
    assert!(matches!(
        s.browse(&meta, &filtered, CancelSlot::default()).await,
        Err(AppError::InvalidInput(m)) if m.contains("another query")
    ));
    let mut skipped = page_request(2, None);
    skipped.offset = 2;
    assert!(
        s.browse(&meta, &skipped, CancelSlot::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn filters_with_and_and_allow_filtering_only() {
    let o = open(KvNode::new(5)).await;
    let s = &o.session;
    let meta = kv_meta(s).await;
    let mut req = page_request(10, None);
    req.filter = Some(FilterGroup {
        conjunction: Conjunction::And,
        items: vec![condition(
            "val",
            FilterOp::Eq,
            vec![Value::Text("v4".into())],
        )],
    });
    let page = s.browse(&meta, &req, CancelSlot::default()).await.unwrap();
    assert_eq!(page.rows.len(), 1);
    assert_eq!(
        page.sql,
        "SELECT \"id\", \"seq\", \"val\" FROM \"shop\".\"kv\" WHERE \"val\" = 'v4' ALLOW FILTERING"
    );
    let refused = |items: Vec<FilterNode>, conjunction| {
        let mut r = page_request(10, None);
        r.filter = Some(FilterGroup { conjunction, items });
        r
    };
    for bad in [
        refused(
            vec![
                condition("val", FilterOp::Eq, vec![Value::Text("a".into())]),
                condition("id", FilterOp::Eq, vec![Value::Int(1)]),
            ],
            Conjunction::Or,
        ),
        refused(
            vec![condition("val", FilterOp::IsNull, vec![])],
            Conjunction::And,
        ),
        refused(
            vec![condition("val", FilterOp::Eq, vec![Value::Null])],
            Conjunction::And,
        ),
        // The raw WHERE is the user's text: it may only read.
        refused(
            vec![FilterNode::Raw {
                sql: "id = 1; DELETE FROM shop.kv WHERE id = 1".into(),
            }],
            Conjunction::And,
        ),
    ] {
        assert!(
            s.browse(&meta, &bad, CancelSlot::default()).await.is_err(),
            "{:?}",
            bad.filter
        );
    }
    assert!(
        !o.wire.texts().iter().any(|q| q.contains("DELETE")),
        "nothing that writes was sent"
    );
}

#[tokio::test]
async fn counts_by_estimate_unless_asked_and_fetches_whole_cells() {
    let o = open(KvNode::new(4)).await;
    let s = &o.session;
    let meta = kv_meta(s).await;
    let estimate = s
        .count(&meta, None, false, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!((estimate.count, estimate.exact), (1000, false));
    let exact = s
        .count(&meta, None, true, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!((exact.count, exact.exact), (4, true));
    assert!(
        o.wire
            .texts()
            .contains(&"SELECT count(*) FROM \"shop\".\"kv\"".to_string())
    );
    let cell = s.fetch_cell(&meta, &int_key(2), "val").await.unwrap();
    assert_eq!(cell, Value::Text("v2".into()));
    assert!(matches!(
        s.fetch_cell(&meta, &int_key(9), "val").await,
        Err(AppError::NotFound(_))
    ));
    let partial = RowKey::Pk(BTreeMap::from([("id".to_string(), Value::Int(2))]));
    assert!(s.fetch_cell(&meta, &partial, "val").await.is_err());
}

/// The statements the grid's commit sends, built as `commands::mutate` builds them.
fn planned(s: &CassandraSession, meta: &TableMeta, changes: &[StagedChange]) -> Vec<TxStatement> {
    let narrowing = Narrowing::for_caps(&s.capabilities());
    let target = TableTarget {
        table: &meta.table,
        columns: &meta.columns,
    };
    changes
        .iter()
        .map(|c| {
            let (built, expect) = change_on(&narrowing, s.dialect(), &target, c, false).unwrap();
            let mut st = TxStatement::new(built.sql, built.params);
            st.expect_affected = expect;
            st
        })
        .collect()
}

fn update(id: i64, val: &str) -> StagedChange {
    StagedChange::Update {
        row_id: format!("r{id}"),
        key: int_key(id),
        changes: BTreeMap::from([("val".to_string(), Value::Text(val.into()))]),
        exprs: None,
    }
}

#[tokio::test]
async fn a_commit_stops_at_the_change_that_did_not_apply() {
    let node = KvNode::new(3);
    let o = open(node.clone()).await;
    let s = &o.session;
    let meta = kv_meta(s).await;
    let statements = planned(
        s,
        &meta,
        &[update(1, "one"), update(7, "gone"), update(2, "two")],
    );
    assert_eq!(
        statements[0].sql,
        "UPDATE \"shop\".\"kv\" SET \"val\" = ? WHERE \"id\" = ? AND \"seq\" = ? IF EXISTS"
    );
    let err = s.transaction(statements).await.unwrap_err();
    assert!(
        matches!(&err, AppError::Db { statement_index: Some(1), message, .. } if message.contains("no longer exists")),
        "{err:?}"
    );
    // The first change stays applied (CQL has no transaction); the third was never sent.
    let data = node.data();
    assert_eq!(data[0].2.as_deref(), Some("one"));
    assert_eq!(data[1].2.as_deref(), Some("v2"));
    assert!(
        !o.wire
            .seen()
            .iter()
            .any(|q| q.params.first() == Some(&Value::Text("two".into())))
    );
    assert_eq!(s.capabilities().data.transactions, TxMode::Sequential);

    // An insert of a key that exists does not overwrite it; a delete of a missing row fails.
    let insert = StagedChange::Insert {
        row_id: "n".into(),
        values: BTreeMap::from([
            ("id".to_string(), Value::Int(2)),
            ("seq".to_string(), Value::Int(0)),
            ("val".to_string(), Value::Text("dup".into())),
        ]),
        exprs: None,
    };
    let err = s
        .transaction(planned(s, &meta, &[insert]))
        .await
        .unwrap_err();
    assert!(matches!(&err, AppError::Db { message, .. } if message.contains("already exists")));
    assert_eq!(node.data()[1].2.as_deref(), Some("v2"));
    let delete = StagedChange::Delete {
        row_id: "r3".into(),
        key: int_key(3),
    };
    let outcomes = s
        .transaction(planned(s, &meta, std::slice::from_ref(&delete)))
        .await
        .unwrap();
    assert_eq!(outcomes[0].affected, 1);
    assert!(s.transaction(planned(s, &meta, &[delete])).await.is_err());
    assert_eq!(node.data().len(), 2);
}

#[tokio::test]
async fn the_editor_pages_to_its_cap_and_runs_cqlsh_commands_itself() {
    let o = open(KvNode::new(5)).await;
    let s = &o.session;
    o.wire.clear();
    let mut sink = CollectSink::default();
    let out = s
        .run_statement("SELECT * FROM kv", 3, &mut sink, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(out.kind, StatementKind::Rows);
    assert_eq!((out.row_count, out.truncated), (3, true));
    assert_eq!(sink.sets[0].rows.len(), 3);
    // cqlsh's page size (100) is the editor's default.
    assert_eq!(o.wire.seen()[0].page.as_ref().map(|p| p.1), Some(100));
    assert_eq!(o.wire.seen()[0].options.consistency, Consistency::LocalOne);

    // CONSISTENCY, PAGING and TRACING are the session's: nothing is sent.
    o.wire.clear();
    for command in ["CONSISTENCY QUORUM", "PAGING 2", "TRACING ON;", "EXPAND ON"] {
        let out = s
            .run_statement(
                command,
                10,
                &mut CollectSink::default(),
                CancelSlot::default(),
            )
            .await
            .unwrap();
        assert_eq!(out.notices.len(), 1, "{command}");
    }
    assert!(o.wire.seen().is_empty());
    assert!(
        s.run_statement(
            "CONSISTENCY SOMETIMES",
            10,
            &mut CollectSink::default(),
            CancelSlot::default()
        )
        .await
        .is_err()
    );
    let settings = settings_of(s);
    assert_eq!(settings.options.consistency, Consistency::Quorum);
    assert_eq!(settings.page_size, 2);
    let mut sink = CollectSink::default();
    let out = s
        .run_statement("SELECT * FROM kv", 100, &mut sink, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!((out.row_count, out.truncated), (5, false));
    let sent = o.wire.seen();
    assert_eq!(sent.len(), 3, "three pages of two");
    assert!(
        sent.iter()
            .all(|q| q.options.consistency == Consistency::Quorum && q.options.tracing)
    );
    // The app's own requests keep the profile's level.
    s.ping().await.unwrap();
    assert_eq!(
        o.wire.seen().last().unwrap().options.consistency,
        Consistency::LocalOne
    );

    // USE switches every connection; DDL waits for the nodes to agree.
    let out = s
        .run_statement(
            "USE \"Other\"",
            10,
            &mut CollectSink::default(),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(out.notices, ["Now using keyspace Other."]);
    assert_eq!(
        o.wire.uses.lock().unwrap().clone(),
        [("Other".to_string(), true)]
    );
    assert_eq!(
        s.server_info().await.unwrap().current_database.as_deref(),
        Some("Other")
    );
    assert!(
        s.run_statement(
            "USE missing",
            10,
            &mut CollectSink::default(),
            CancelSlot::default()
        )
        .await
        .is_err()
    );
    let out = s
        .run_statement(
            "CREATE TABLE t (id int PRIMARY KEY)",
            10,
            &mut CollectSink::default(),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(out.kind, StatementKind::Empty);
    assert_eq!(o.wire.agreements.load(Ordering::SeqCst), 1);
    // A server error reaches the editor as it is.
    assert!(matches!(
        s.run_statement(
            "SELEC nonsense",
            10,
            &mut CollectSink::default(),
            CancelSlot::default()
        )
        .await,
        Err(AppError::Db { .. })
    ));
}

#[tokio::test]
async fn traced_runs_bring_their_trace_back_as_notices() {
    let wire = FakeWire::new(|s| {
        let mut reply = rows(&[("v", "text")], vec![vec![t("x")]]);
        if s.options.tracing {
            reply.tracing_id = Some(uuid::Uuid::nil());
        }
        reply.warnings = vec!["Aggregation query used without partition key".into()];
        Ok(reply)
    });
    let connector = FakeConnector::new(wire.clone());
    let p = profile(serde_json::json!({ "options": { "startupSql": "TRACING ON" } }));
    let target = ConnectTarget {
        profile: &p,
        password: None,
        host_override: None,
    };
    let (d, params) = dial(&target).unwrap();
    let s = CassandraSession::open(
        &connector,
        &d,
        params,
        false,
        confirm_of(&p),
        Some("TRACING ON"),
    )
    .await
    .unwrap();
    let out = s
        .run_statement(
            "SELECT v FROM t",
            10,
            &mut CollectSink::default(),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(out.notices.len(), 2, "{:?}", out.notices);
    assert!(out.notices[1].starts_with("Tracing session 00000000-"));
}

#[tokio::test]
async fn cancel_drops_the_run_and_the_time_out_stops_a_read() {
    let o = open(KvNode::new(5)).await;
    let s = Arc::new(o.session);
    o.wire.hang_on("slow");
    let slot = CancelSlot::default();
    let run = {
        let s = s.clone();
        let slot = slot.clone();
        tokio::spawn(async move {
            s.run_statement("SELECT * FROM slow", 10, &mut CollectSink::default(), slot)
                .await
        })
    };
    while !slot.is_armed() {
        tokio::task::yield_now().await;
    }
    let handle = request_cancel(&slot).expect("the run is armed");
    s.cancel(handle).await.unwrap();
    assert!(matches!(run.await.unwrap(), Err(AppError::Cancelled)));

    // The query time out of the grid and the editor cancels through the session.
    let limited = crate::commands::query::query_limited(
        s.as_ref(),
        "SELECT * FROM slow",
        &[],
        Some(Duration::from_millis(50)),
    )
    .await;
    assert!(
        matches!(limited, Err(AppError::QueryTimeout { .. })),
        "{limited:?}"
    );
    // The session is still usable afterwards.
    s.ping().await.unwrap();
}

#[tokio::test]
async fn read_only_refuses_every_write_before_sending_it() {
    let node = KvNode::new(3);
    let o = open_on(
        node.clone(),
        serde_json::json!({ "options": { "readOnly": true } }),
    )
    .await
    .unwrap();
    let s = &o.session;
    let caps = s.capabilities();
    assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);
    let meta = kv_meta(s).await;
    o.wire.clear();
    let ro = |r: Result<(), AppError>| {
        assert!(
            matches!(&r, Err(AppError::InvalidInput(m)) if m.contains("read-only")),
            "{r:?}"
        );
    };
    ro(s.query("INSERT INTO kv (id, seq) VALUES (9, 0)", &[])
        .await
        .map(|_| ()));
    ro(s.execute("DELETE FROM kv WHERE id = 1", &[])
        .await
        .map(|_| ()));
    ro(
        s.execute_cancellable("TRUNCATE kv", &[], CancelSlot::default())
            .await
            .map(|_| ()),
    );
    ro(s.transaction(planned(s, &meta, &[update(1, "x")]))
        .await
        .map(|_| ()));
    ro(s.run_statement(
        "BEGIN BATCH DELETE FROM kv WHERE id = 1; APPLY BATCH",
        10,
        &mut CollectSink::default(),
        CancelSlot::default(),
    )
    .await
    .map(|_| ()));
    ro(s.run_statement(
        "GRANT ALL ON KEYSPACE shop TO bob",
        10,
        &mut CollectSink::default(),
        CancelSlot::default(),
    )
    .await
    .map(|_| ()));
    ro(s.stream_query(
        "UPDATE kv SET val = 'x' WHERE id = 1 AND seq = 0",
        &[],
        &mut CollectSink::default(),
        CancelSlot::default(),
    )
    .await
    .map(|_| ()));
    assert!(o.wire.seen().is_empty(), "nothing reached the node");
    assert_eq!(node.data()[0].2.as_deref(), Some("v1"));
    // Reads and the session's own commands still run.
    s.run_statement(
        "SELECT * FROM kv",
        10,
        &mut CollectSink::default(),
        CancelSlot::default(),
    )
    .await
    .unwrap();
    s.run_statement(
        "CONSISTENCY ONE",
        10,
        &mut CollectSink::default(),
        CancelSlot::default(),
    )
    .await
    .unwrap();
    s.browse(&meta, &page_request(2, None), CancelSlot::default())
        .await
        .unwrap();

    // Startup commands that write keep a read-only connection from opening at all.
    let refused = open_on(
        KvNode::new(1),
        serde_json::json!({ "options": { "readOnly": true, "startupSql": "CONSISTENCY ONE; DROP TABLE kv" } }),
    )
    .await;
    assert!(matches!(refused, Err(AppError::InvalidInput(m)) if m.contains("read-only")));
}

#[tokio::test]
async fn startup_commands_run_at_connect() {
    let o = open_on(
        KvNode::new(1),
        serde_json::json!({ "options": { "startupSql": "CONSISTENCY LOCAL_QUORUM;\nUSE \"Other\";\nCREATE TABLE IF NOT EXISTS t (id int PRIMARY KEY)" } }),
    )
    .await
    .unwrap();
    assert_eq!(
        settings_of(&o.session).options.consistency,
        Consistency::LocalQuorum
    );
    assert_eq!(o.wire.uses.lock().unwrap().len(), 1);
    assert!(
        o.wire
            .texts()
            .iter()
            .any(|q| q.starts_with("CREATE TABLE IF NOT EXISTS"))
    );
    assert_eq!(o.wire.agreements.load(Ordering::SeqCst), 1);
    let failing = open_on(
        KvNode::new(1),
        serde_json::json!({ "options": { "startupSql": "ALTER nothing" } }),
    )
    .await;
    assert!(matches!(failing, Err(AppError::InvalidInput(m)) if m.contains("startup commands")));
}

#[tokio::test]
async fn caps_grant_what_the_session_does() {
    let o = open(KvNode::new(1)).await;
    let caps = o.session.capabilities();
    for f in [
        Feature::Editor,
        Feature::StructureView,
        Feature::EditRows,
        Feature::InsertRows,
        Feature::DeleteRows,
        Feature::ServerExpressions,
        Feature::ExactCount,
        Feature::TableDdl,
        Feature::ViewDefinition,
        Feature::SwitchDatabase,
        Feature::StartupCommands,
    ] {
        assert!(caps.has(f), "{f:?}");
    }
    for f in [
        Feature::CancelOnServer,
        Feature::EditWithoutKey,
        Feature::Returning,
        Feature::Explain,
        Feature::Mcp,
        Feature::StructureEdit,
        Feature::ExportTables,
    ] {
        assert!(!caps.has(f), "{f:?}");
    }
    assert_eq!(caps.data.paging, Paging::Cursor);
    assert_eq!(caps.data.count, CountMode::Costly);
    assert!(!caps.data.sort);
    assert_eq!(caps.namespaces.default_schema.as_deref(), Some("shop"));
    assert_eq!(caps.session.keepalive_secs, Some(30));
    assert!(o.session.table_source().is_some());
    assert_eq!(o.session.in_transaction(), Some(false));
}

#[tokio::test]
async fn counter_tables_and_key_changes_are_refused_before_anything_is_sent() {
    let o = open(KvNode::new(1)).await;
    let s = &o.session;
    let hits = s
        .table_meta(&TableRef::new(Some("shop"), "hits"))
        .await
        .unwrap();
    let narrowing = Narrowing::for_caps(&s.capabilities());
    let target = TableTarget {
        table: &hits.table,
        columns: &hits.columns,
    };
    let bump = StagedChange::Update {
        row_id: "r".into(),
        key: RowKey::Pk(BTreeMap::from([(
            "page".to_string(),
            Value::Text("/".into()),
        )])),
        changes: BTreeMap::from([("n".to_string(), Value::Int(3))]),
        exprs: None,
    };
    assert!(change_on(&narrowing, s.dialect(), &target, &bump, false).is_err());
    let meta = kv_meta(s).await;
    let key_change = StagedChange::Update {
        row_id: "r".into(),
        key: int_key(1),
        changes: BTreeMap::from([("seq".to_string(), Value::Int(5))]),
        exprs: None,
    };
    let target = TableTarget {
        table: &meta.table,
        columns: &meta.columns,
    };
    assert!(change_on(&narrowing, s.dialect(), &target, &key_change, false).is_err());
}
