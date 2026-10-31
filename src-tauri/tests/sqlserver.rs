//! SQL Server against a real server: the sqlserver lane's suite (docs/drivers/sqlserver.md).
//! Built only with `drv-sqlserver` (`required-features` in Cargo.toml) and run with
//! `node dev/scripts/lane.mjs sqlserver it`, which loads `.env`. Each test skips itself while
//! `TABLER_TEST_SQLSERVER_URL` (`sqlserver://user:password@host:1433/database`) is unset, and is
//! `#[ignore]`d so that a plain `cargo test --all-features` never reaches a server.
//!
//! The tests make their own tables (`tabler_it_<uuid>` in the URL's database) and drop them.
//! Besides what the fake server proves, they check what only a server can: `@@ROWCOUNT` after a
//! trigger that writes, an ODBC `{call p}` escape inside a batch, the session setup, the login
//! of a switched database, the plan XML of Explain (a read-only one of a DML statement that does
//! not run it), a read-only session that leaves a sequence where it was, and the server's own
//! cancel and time out.

#![cfg(feature = "drv-sqlserver")]
#![allow(clippy::result_large_err)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use tabler_lib::commands::browse::{browse_page, fetch_cell_inner};
use tabler_lib::commands::query::{explain_inner, query_limited};
use tabler_lib::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, StatementOutcome, TxStatement, connect,
    request_cancel,
};
use tabler_lib::error::AppError;
use tabler_lib::export;
use tabler_lib::sqlgen::dialect::SqlDialect;
use tabler_lib::sqlgen::narrow::Narrowing;
use tabler_lib::sqlgen::sql::{self, TableTarget};
use tabler_lib::types::browse::{BrowseRequest, Conjunction, FilterGroup, FilterNode, RowKey};
use tabler_lib::types::caps::Feature;
use tabler_lib::types::mutate::StagedChange;
use tabler_lib::types::query::{ExplainMode, StatementKind};
use tabler_lib::types::schema::{TableMeta, TableRef};
use tabler_lib::types::value::Value;

const VAR: &str = "TABLER_TEST_SQLSERVER_URL";
const BIG: i64 = (1 << 53) + 1;

/// A name for this run's objects.
fn scratch(what: &str) -> String {
    format!("tabler_it_{what}_{}", uuid::Uuid::new_v4().simple())
}

/// One batch on the editor, its rows kept.
async fn run(s: &dyn DbSession, sql: &str) -> Result<(StatementOutcome, CollectSink), AppError> {
    let mut sink = CollectSink::default();
    let outcome = s
        .run_statement(sql, 10_000, &mut sink, CancelSlot::default())
        .await?;
    Ok((outcome, sink))
}

async fn drop_table(s: &dyn DbSession, name: &str) {
    if let Err(e) = run(s, &format!("DROP TABLE IF EXISTS dbo.[{name}]")).await {
        println!("dropping {name}: {e}");
    }
}

fn first_cell(sink: &CollectSink) -> Value {
    sink.sets
        .first()
        .and_then(|s| s.rows.first())
        .and_then(|r| r.first())
        .cloned()
        .unwrap_or(Value::Null)
}

fn int(v: &Value) -> i64 {
    match v {
        Value::Int(n) => *n,
        Value::BigInt(s) | Value::Decimal(s) | Value::Text(s) => s.parse().unwrap_or(-1),
        _ => -1,
    }
}

fn update(s: &dyn DbSession, meta: &TableMeta, id: i64, name: &str) -> TxStatement {
    let narrowing = Narrowing::for_caps(&s.capabilities());
    let target = TableTarget {
        table: &meta.table,
        columns: &meta.columns,
    };
    let change = StagedChange::Update {
        row_id: id.to_string(),
        key: RowKey::Pk([("id".to_string(), Value::Int(id))].into()),
        changes: [("name".to_string(), Value::Text(name.into()))].into(),
        exprs: None,
    };
    let (built, expect) =
        sql::change_on(&narrowing, SqlDialect::Tsql, &target, &change, false).unwrap();
    let mut st = TxStatement::new(built.sql, built.params);
    st.expect_affected = expect;
    st
}

#[tokio::test]
#[ignore = "set TABLER_TEST_SQLSERVER_URL"]
async fn connects_lists_and_reports_the_server() {
    let Some((s, profile)) = common::open(VAR).await else {
        return;
    };
    let info = s.server_info().await.unwrap();
    assert!(
        info.version.starts_with("SQL Server") || info.version.starts_with("Azure"),
        "{}",
        info.version
    );
    let dbs = s.list_databases().await.unwrap();
    assert!(dbs.iter().any(|d| d.is_current), "{dbs:?}");
    let schemas = s.list_schemas().await.unwrap();
    assert!(schemas.iter().any(|x| x.name == "dbo"), "{schemas:?}");
    s.ping().await.unwrap();
    let caps = s.capabilities();
    assert!(caps.has(Feature::Editor) && caps.has(Feature::CancelOnServer));
    // The session setup ran on the editor (ARITHABORT on, TEXTSIZE whole), Azure Synapse
    // included when the URL names one.
    let (_, sink) = run(
        s.as_ref(),
        "SELECT CAST(SESSIONPROPERTY('ARITHABORT') AS INT), @@TEXTSIZE, \
         CAST(SERVERPROPERTY('EngineEdition') AS INT)",
    )
    .await
    .unwrap();
    let row = &sink.sets[0].rows[0];
    assert_eq!(int(&row[0]), 1, "ARITHABORT");
    assert_eq!(int(&row[1]), 2_147_483_647, "TEXTSIZE");
    println!(
        "engine edition {} ({}), database {:?}",
        int(&row[2]),
        info.version,
        profile.database
    );
    let processes = s.list_processes().await.unwrap().unwrap();
    assert!(
        processes.iter().any(|p| p.current && p.own),
        "{processes:?}"
    );
    let details = s.session_details().await.unwrap().unwrap();
    println!("encrypted: {}", details.tls);
    s.close().await;
    // Switch Database opens a session from the profile with the other database, which the login
    // names (Azure SQL Database refuses a contained user's login to master).
    if !info.version.starts_with("Azure") {
        let url = common::url(VAR).expect("the variable is set");
        let (_, password) = common::profile_from_url(&url);
        let mut master = profile.clone();
        master.database = Some("master".into());
        let m = connect(ConnectTarget {
            profile: &master,
            password: password.as_deref(),
            host_override: None,
        })
        .await
        .unwrap();
        let switched = m.server_info().await.unwrap();
        assert_eq!(switched.current_database.as_deref(), Some("master"));
        m.close().await;
    }
}

#[tokio::test]
#[ignore = "set TABLER_TEST_SQLSERVER_URL"]
async fn browses_counts_fetches_and_commits() {
    let Some((s, _)) = common::open(VAR).await else {
        return;
    };
    let name = scratch("grid");
    let result = async {
        run(
            s.as_ref(),
            &format!(
                "CREATE TABLE dbo.[{name}] (id bigint IDENTITY PRIMARY KEY, name nvarchar(50) \
                 NULL, big bigint NULL, amount money NULL, data varbinary(max) NULL, at \
                 datetimeoffset(7) NULL, CONSTRAINT [CK_{name}] CHECK (name <> N'bad'))"
            ),
        )
        .await?;
        let (insert, _) = run(
            s.as_ref(),
            &format!(
                "INSERT INTO dbo.[{name}] (name, big, amount, data, at) VALUES \
                 (N'a', {BIG}, 12.3456, CAST(REPLICATE(CAST(0x07 AS varbinary(max)), 70000) AS \
                 varbinary(max)), '2024-01-02 15:30:00.1234567 +05:30'), (N'b', 2, NULL, NULL, \
                 NULL), (N'c', 3, NULL, NULL, NULL), (N'd', 4, NULL, NULL, NULL), (N'e', 5, \
                 NULL, NULL, NULL)"
            ),
        )
        .await?;
        assert_eq!(insert.kind, StatementKind::Affected);
        assert_eq!(insert.affected, Some(5));

        let meta = s.table_meta(&TableRef::new(Some("dbo"), &name)).await?;
        assert_eq!(meta.primary_key, ["id"]);
        let ddl = s.table_ddl(&meta.table).await.unwrap()?;
        assert!(
            ddl.starts_with(&format!("CREATE TABLE [dbo].[{name}]")),
            "{ddl}"
        );

        let mut req = BrowseRequest {
            table: meta.table.clone(),
            columns: None,
            filter: None,
            sort: Vec::new(),
            limit: 2,
            offset: 0,
            cursor: None,
        };
        let page = browse_page(s.as_ref(), &meta, &req, None).await?;
        let first = &page.rows[0];
        assert_eq!(first[2], Value::BigInt(BIG.to_string()));
        assert_eq!(first[3], Value::Decimal("12.3456".into()));
        assert!(matches!(&first[4], Value::Bytes(b) if b.truncated && b.len == 70_000));
        assert_eq!(
            first[5],
            Value::TimestampTz("2024-01-02 15:30:00.1234567 +05:30".into())
        );
        req.offset = 4;
        assert_eq!(
            browse_page(s.as_ref(), &meta, &req, None).await?.rows.len(),
            1
        );

        let count = sql::count_on(
            &Narrowing::for_caps(&s.capabilities()),
            SqlDialect::Tsql,
            &meta.table,
            &meta.columns,
            None,
        )?;
        let counted = s.query(&count.sql, &count.params).await?;
        assert_eq!(int(&counted.rows[0][0]), 5);

        // A raw filter that adds a statement is refused, writable as this session is, and the
        // rows stay.
        let sneaky = BrowseRequest {
            filter: Some(FilterGroup {
                conjunction: Conjunction::And,
                items: vec![FilterNode::Raw {
                    sql: format!(
                        "1=1) DELETE FROM dbo.[{name}] WHERE (1=1) SELECT 1 AS a FROM dbo.[{name}] \
                         WHERE (1=1"
                    ),
                }],
            }),
            offset: 0,
            ..req.clone()
        };
        let refused = browse_page(s.as_ref(), &meta, &sneaky, None).await;
        assert!(
            matches!(refused, Err(AppError::InvalidInput(_))),
            "{refused:?}"
        );
        // So is an export of that filter, and a query export that holds a second statement.
        let mut exports = vec![export::table_query(
            SqlDialect::Tsql,
            &meta,
            None,
            sneaky.filter.as_ref(),
            &[],
            None,
            0,
            false,
        )?];
        for text in [
            format!("SELECT 1 AS a;DELETE FROM dbo.[{name}]"),
            format!("SELECT 1 AS a DELETE FROM dbo.[{name}]"),
        ] {
            exports.push(export::sql_query(&text, SqlDialect::Tsql)?);
        }
        for q in &exports {
            let refused = s
                .stream_query(
                    &q.sql,
                    &q.params,
                    &mut CollectSink::default(),
                    CancelSlot::default(),
                )
                .await;
            assert!(
                matches!(refused, Err(AppError::InvalidInput(_))),
                "{}: {refused:?}",
                q.sql
            );
        }
        let counted = s.query(&count.sql, &count.params).await?;
        assert_eq!(int(&counted.rows[0][0]), 5);

        let key = RowKey::Pk([("id".to_string(), Value::Int(1))].into());
        let whole = fetch_cell_inner(s.as_ref(), &meta, "data", &key).await?;
        assert!(matches!(whole, Value::Bytes(b) if !b.truncated && b.len == 70_000));

        // A commit with one failing change: nothing of it stays.
        let failed = s
            .transaction(vec![
                update(s.as_ref(), &meta, 1, "x"),
                update(s.as_ref(), &meta, 2, "bad"),
            ])
            .await;
        assert!(
            matches!(
                failed,
                Err(AppError::Db {
                    statement_index: Some(1),
                    ..
                })
            ),
            "{failed:?}"
        );
        let (_, sink) = run(
            s.as_ref(),
            &format!("SELECT name FROM dbo.[{name}] WHERE id = 1"),
        )
        .await?;
        assert_eq!(first_cell(&sink), Value::Text("a".into()));
        // A row that is gone fails its count.
        let gone = s
            .transaction(vec![update(s.as_ref(), &meta, 99, "x")])
            .await;
        assert!(
            matches!(&gone, Err(AppError::Db { message, .. }) if message.contains("matched 0")),
            "{gone:?}"
        );
        let done = s
            .transaction(vec![
                update(s.as_ref(), &meta, 1, "x"),
                update(s.as_ref(), &meta, 2, "y"),
            ])
            .await?;
        assert_eq!(done.iter().map(|d| d.affected).collect::<Vec<_>>(), [1, 1]);
        Ok::<_, AppError>(())
    }
    .await;
    drop_table(s.as_ref(), &name).await;
    s.close().await;
    result.unwrap();
}

/// `@@ROWCOUNT` counts the statement's own rows, not those its trigger writes: the edit guard and
/// the editor's count rely on it.
#[tokio::test]
#[ignore = "set TABLER_TEST_SQLSERVER_URL"]
async fn rowcount_after_a_trigger_that_writes() {
    let Some((s, _)) = common::open(VAR).await else {
        return;
    };
    let (name, audit) = (scratch("trg"), scratch("audit"));
    let result = async {
        run(
            s.as_ref(),
            &format!(
                "CREATE TABLE dbo.[{name}] (id bigint PRIMARY KEY, name nvarchar(50) NULL); \
                 CREATE TABLE dbo.[{audit}] (n int); INSERT INTO dbo.[{name}] VALUES (1, N'a')"
            ),
        )
        .await?;
        run(
            s.as_ref(),
            &format!(
                "CREATE TRIGGER dbo.[t_{name}] ON dbo.[{name}] AFTER UPDATE AS \
                 INSERT INTO dbo.[{audit}] VALUES (1), (2), (3)"
            ),
        )
        .await?;
        let meta = s.table_meta(&TableRef::new(Some("dbo"), &name)).await?;
        let done = s
            .transaction(vec![update(s.as_ref(), &meta, 1, "b")])
            .await?;
        assert_eq!(done[0].affected, 1);
        let (outcome, _) = run(
            s.as_ref(),
            &format!("UPDATE dbo.[{name}] SET name = N'c' WHERE id = 1"),
        )
        .await?;
        assert_eq!(outcome.affected, Some(1));
        Ok::<_, AppError>(())
    }
    .await;
    drop_table(s.as_ref(), &name).await;
    drop_table(s.as_ref(), &audit).await;
    s.close().await;
    result.unwrap();
}

/// Whether a batch runs an ODBC `{call p}` escape after another statement: if it does, the
/// shared classifier must call it a write (CALL is not one of T-SQL's write words; the session
/// refuses it already, `tsql::hides_statement`).
#[tokio::test]
#[ignore = "set TABLER_TEST_SQLSERVER_URL"]
async fn an_odbc_call_escape_inside_a_batch() {
    let Some((s, _)) = common::open(VAR).await else {
        return;
    };
    let (marks, proc) = (scratch("marks"), scratch("proc"));
    let result = async {
        run(s.as_ref(), &format!("CREATE TABLE dbo.[{marks}] (n int)")).await?;
        run(
            s.as_ref(),
            &format!("CREATE PROCEDURE dbo.[{proc}] AS INSERT INTO dbo.[{marks}] VALUES (1)"),
        )
        .await?;
        let batch = format!("SELECT 1 AS a {{call dbo.[{proc}]}}");
        let ran = run(s.as_ref(), &batch).await;
        let (_, sink) = run(s.as_ref(), &format!("SELECT COUNT(*) FROM dbo.[{marks}]")).await?;
        let wrote = int(&first_cell(&sink)) > 0;
        println!(
            "{batch}: {:?}, wrote: {wrote}",
            ran.as_ref().map(|r| r.0.kind)
        );
        if wrote {
            assert!(
                sql::writes(SqlDialect::Tsql, &batch),
                "the classifier reads {batch}"
            );
        }
        run(s.as_ref(), &format!("DROP PROCEDURE dbo.[{proc}]")).await?;
        Ok::<_, AppError>(())
    }
    .await;
    drop_table(s.as_ref(), &marks).await;
    s.close().await;
    result.unwrap();
}

#[tokio::test]
#[ignore = "set TABLER_TEST_SQLSERVER_URL"]
async fn cancel_time_out_and_explain() {
    let Some((s, _)) = common::open(VAR).await else {
        return;
    };
    let slot = CancelSlot::default();
    let running = {
        let s = Arc::clone(&s);
        let slot = slot.clone();
        tokio::spawn(async move {
            s.run_statement(
                "WAITFOR DELAY '00:00:30'",
                10,
                &mut CollectSink::default(),
                slot,
            )
            .await
        })
    };
    // Cancel once the statement runs (a cancel before it starts would only refuse it).
    while !slot.is_armed() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let handle = request_cancel(&slot).expect("the statement runs");
    s.cancel(handle).await.unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .expect("the cancel is answered")
        .unwrap();
    assert!(matches!(outcome, Err(AppError::Cancelled)), "{outcome:?}");
    // The editor goes on, and the grid's read under the time out is stopped on the server.
    run(s.as_ref(), "SELECT 1").await.unwrap();
    let limited = query_limited(
        s.as_ref(),
        "WAITFOR DELAY '00:00:30' SELECT 1",
        &[],
        Some(Duration::from_secs(1)),
    )
    .await;
    assert!(
        matches!(limited, Err(AppError::QueryTimeout { .. })),
        "{limited:?}"
    );

    for mode in [ExplainMode::Plan, ExplainMode::Analyze] {
        let plan = explain_inner(
            s.as_ref(),
            "SELECT name FROM sys.objects WHERE object_id < 100",
            mode,
            &CancelSlot::default(),
        )
        .await
        .unwrap();
        assert!(
            plan.text
                .as_deref()
                .is_some_and(|t| t.contains("ShowPlanXML")),
            "{mode:?}: {:?}",
            plan.text
        );
    }
    // Plan output is off again: a statement runs.
    let (_, sink) = run(s.as_ref(), "SELECT 41 + 1").await.unwrap();
    assert_eq!(first_cell(&sink), Value::Int(42));
    assert_eq!(s.in_transaction(), Some(false));
    s.close().await;
}

#[tokio::test]
#[ignore = "set TABLER_TEST_SQLSERVER_URL"]
async fn read_only_refuses_writes() {
    let Some(url) = common::url(VAR) else {
        return;
    };
    common::install_crypto();
    let (mut profile, password) = common::profile_from_url(&url);
    profile.options.read_only = true;
    let s = connect(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .unwrap();
    let name = scratch("ro");
    for result in [
        s.execute(&format!("CREATE TABLE dbo.[{name}] (n int)"), &[])
            .await
            .map(|_| ()),
        run(s.as_ref(), &format!("SELECT 1 AS n INTO dbo.[{name}]"))
            .await
            .map(|_| ()),
        run(s.as_ref(), "EXEC sp_who").await.map(|_| ()),
    ] {
        assert!(
            matches!(result, Err(AppError::InvalidInput(_))),
            "{result:?}"
        );
    }
    let tables = s.list_tables("dbo").await.unwrap();
    assert!(tables.iter().all(|t| t.name != name));
    // The plan of a DML statement that reaches another server is refused before anything is
    // sent, so no linked server is needed.
    let distributed = explain_inner(
        s.as_ref(),
        "DELETE FROM t WHERE id IN (SELECT id FROM OPENQUERY(srv, 'SELECT 1 AS id'))",
        ExplainMode::Plan,
        &CancelSlot::default(),
    )
    .await;
    assert!(
        matches!(distributed, Err(AppError::InvalidInput(_))),
        "{distributed:?}"
    );
    // The plan of one DML statement is shown, and the statement does not run.
    let Some((w, _)) = common::open(VAR).await else {
        return;
    };
    let kept = scratch("plan");
    run(
        w.as_ref(),
        &format!("CREATE TABLE dbo.[{kept}] (n int); INSERT dbo.[{kept}] VALUES (1)"),
    )
    .await
    .unwrap();
    let plan = explain_inner(
        s.as_ref(),
        &format!("DELETE FROM dbo.[{kept}]"),
        ExplainMode::Plan,
        &CancelSlot::default(),
    )
    .await;
    let (_, sink) = run(w.as_ref(), &format!("SELECT COUNT(*) FROM dbo.[{kept}]"))
        .await
        .unwrap();
    drop_table(w.as_ref(), &kept).await;
    // A read that takes a sequence's next value is refused too: no rollback gives it back, so
    // the writable session's first value is the sequence's first.
    let seq = scratch("seq");
    run(
        w.as_ref(),
        &format!("CREATE SEQUENCE dbo.[{seq}] AS bigint START WITH 1 INCREMENT BY 1"),
    )
    .await
    .unwrap();
    let next = run(s.as_ref(), &format!("SELECT NEXT VALUE FOR dbo.[{seq}]")).await;
    let taken = run(w.as_ref(), &format!("SELECT NEXT VALUE FOR dbo.[{seq}]")).await;
    if let Err(e) = run(w.as_ref(), &format!("DROP SEQUENCE dbo.[{seq}]")).await {
        println!("dropping {seq}: {e}");
    }
    w.close().await;
    assert!(matches!(next, Err(AppError::InvalidInput(_))), "{next:?}");
    assert_eq!(int(&first_cell(&taken.unwrap().1)), 1, "the sequence moved");
    let plan = plan.unwrap();
    assert!(
        plan.text
            .as_deref()
            .is_some_and(|t| t.contains("ShowPlanXML")),
        "{:?}",
        plan.text
    );
    assert_eq!(int(&first_cell(&sink)), 1, "the DELETE did not run");
    run(s.as_ref(), "SELECT 1").await.unwrap();
    s.close().await;
}
