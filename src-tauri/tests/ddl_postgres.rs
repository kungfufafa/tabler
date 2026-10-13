//! DDL integration test for PostgreSQL: plans with `plan_ddl`, runs with `apply_plan`, and
//! re-introspects after every step. Runs only when `TABLER_TEST_PG_URL` is set; everything
//! happens inside a scratch schema `tabler_ddl_<random>` that is dropped at the end.

use std::sync::Arc;

use tabler_lib::commands::ddl::{DDL_LOCK_TIMEOUT_SECS, apply_plan, plan_on};
use tabler_lib::drivers::{CancelSlot, CollectSink, ConnectTarget, DbSession, connect};
use tabler_lib::sqlgen::Dialect;
use tabler_lib::sqlgen::ddl::{create_table_sql, plan_ddl};
use tabler_lib::types::ddl::{ColumnDef, DdlOp, DdlResult, FkDef, IndexDef};
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
use tabler_lib::types::schema::{ColumnMeta, TableMeta, TableRef};
use tabler_lib::types::value::Value;

/// Parses `postgres://user[:pass]@host[:port]/db` into a profile plus the password.
fn profile_from_url(url: &str) -> (ConnectionProfile, Option<String>) {
    let rest = url
        .strip_prefix("postgres://")
        .or_else(|| url.strip_prefix("postgresql://"))
        .expect("TABLER_TEST_PG_URL must start with postgres://");
    let (auth, hostdb) = match rest.rsplit_once('@') {
        Some((a, h)) => (Some(a), h),
        None => (None, rest),
    };
    let (user, password) = match auth {
        Some(a) => match a.split_once(':') {
            Some((u, p)) => (Some(u.to_string()), Some(p.to_string())),
            None => (Some(a.to_string()), None),
        },
        None => (None, None),
    };
    let (hostport, db) = match hostdb.split_once('/') {
        Some((h, d)) => (h, Some(d.split('?').next().unwrap_or("").to_string())),
        None => (hostdb, None),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
        None => (hostport.to_string(), None),
    };
    let profile = ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "pg-ddl-test".into(),
        color: None,
        group: None,
        driver: Driver::Postgres,
        host: Some(host),
        port,
        user,
        database: db.filter(|d| !d.is_empty()),
        ssl: SslMode::Prefer,
        ssl_ca_path: None,
        sqlite_path: None,
        ssh: None,
        options: ProfileOptions::default(),
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    };
    (profile, password)
}

async fn open() -> Option<Arc<dyn DbSession>> {
    let Ok(url) = std::env::var("TABLER_TEST_PG_URL") else {
        println!("skipped: TABLER_TEST_PG_URL unset");
        return None;
    };
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let (profile, password) = profile_from_url(&url);
    let session = connect(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .expect("connect");
    Some(session)
}

fn col(name: &str, ty: &str, nullable: bool) -> ColumnDef {
    ColumnDef {
        name: name.into(),
        data_type: ty.into(),
        nullable,
        default_expr: None,
        auto_increment: false,
        comment: None,
        collation: None,
        unsigned: false,
        on_update: None,
    }
}

fn column<'a>(meta: &'a TableMeta, name: &str) -> &'a ColumnMeta {
    meta.columns
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("column {name} missing from {:?}", names(meta)))
}

fn names(meta: &TableMeta) -> Vec<&str> {
    meta.columns.iter().map(|c| c.name.as_str()).collect()
}

/// Plans and applies `ops`, failing the test when either step fails.
async fn apply_ok(
    session: &Arc<dyn DbSession>,
    meta: Option<&TableMeta>,
    ops: Vec<DdlOp>,
) -> DdlResult {
    let plan = plan_ddl(Dialect::Postgres, meta, &ops).expect("plan");
    assert!(plan.transactional, "{plan:?}");
    let result = apply_plan(session.as_ref(), &plan).await;
    assert!(
        result.error.is_none(),
        "apply failed: {:?}\nplan: {:#?}",
        result.error,
        plan.statements
    );
    assert_eq!(result.executed, plan.statements);
    result
}

async fn scalar(session: &Arc<dyn DbSession>, sql: &str) -> i64 {
    let rs = session.query(sql, &[]).await.expect(sql);
    match rs.rows.first().and_then(|r| r.first()) {
        Some(Value::Int(i)) => *i,
        Some(Value::BigInt(s) | Value::Decimal(s) | Value::Text(s)) => s.parse().expect(sql),
        other => panic!("{sql}: unexpected {other:?}"),
    }
}

async fn steps(session: Arc<dyn DbSession>, schema: String) {
    let s = Some(schema.as_str());
    let owners = TableRef::new(s, "tabler_ddl_owners");
    let items = TableRef::new(s, "tabler_ddl_items");
    let q_items = format!("\"{schema}\".\"tabler_ddl_items\"");

    // 1. create both tables (no metadata needed)
    let mut owner_id = col("id", "integer", false);
    owner_id.auto_increment = true;
    let mut id = col("id", "bigint", false);
    id.auto_increment = true;
    let mut qty = col("qty", "integer", false);
    qty.default_expr = Some("0".into());
    apply_ok(
        &session,
        None,
        vec![
            DdlOp::CreateTable {
                table: owners.clone(),
                columns: vec![owner_id, col("label", "text", true)],
                primary_key: vec!["id".into()],
                if_not_exists: false,
            },
            DdlOp::CreateTable {
                table: items.clone(),
                columns: vec![
                    id,
                    col("name", "text", false),
                    qty,
                    col("code", "text", true),
                ],
                primary_key: vec!["id".into()],
                if_not_exists: true,
            },
        ],
    )
    .await;
    session
        .execute(
            &format!(
                "INSERT INTO {q_items} (name, qty, code) VALUES ('a', 1, '10'), ('b', 2, '20')"
            ),
            &[],
        )
        .await
        .expect("insert");
    session
        .execute(
            &format!("INSERT INTO \"{schema}\".tabler_ddl_owners (label) VALUES ('o1')"),
            &[],
        )
        .await
        .expect("insert owner");
    let meta = session.table_meta(&items).await.expect("meta");
    assert_eq!(names(&meta), ["id", "name", "qty", "code"]);
    assert_eq!(meta.primary_key, ["id"]);
    assert!(column(&meta, "id").is_auto_increment);
    assert_eq!(column(&meta, "id").extra.as_deref(), Some("BY DEFAULT"));
    assert_eq!(column(&meta, "qty").default_expr.as_deref(), Some("0"));

    // 2. add columns (default + not null + comment; a nullable FK column)
    let mut note = col("note", "varchar(20)", false);
    note.default_expr = Some("'n/a'".into());
    note.comment = Some("free text".into());
    apply_ok(
        &session,
        Some(&meta),
        vec![
            DdlOp::AddColumn {
                table: items.clone(),
                column: note,
                after: None,
            },
            DdlOp::AddColumn {
                table: items.clone(),
                column: col("owner_id", "integer", true),
                after: None,
            },
        ],
    )
    .await;
    let meta = session.table_meta(&items).await.expect("meta");
    let note = column(&meta, "note");
    assert!(!note.nullable);
    assert_eq!(note.data_type, "character varying(20)");
    assert!(
        note.default_expr.as_deref().unwrap_or("").contains("'n/a'"),
        "{:?}",
        note.default_expr
    );
    assert_eq!(note.comment.as_deref(), Some("free text"));
    assert_eq!(
        scalar(
            &session,
            &format!("SELECT count(*)::text FROM {q_items} WHERE note = 'n/a'")
        )
        .await,
        2
    );

    // 3. rename a column
    apply_ok(
        &session,
        Some(&meta),
        vec![DdlOp::RenameColumn {
            table: items.clone(),
            name: "note".into(),
            new_name: "remark".into(),
        }],
    )
    .await;
    let meta = session.table_meta(&items).await.expect("meta");
    assert_eq!(
        names(&meta),
        ["id", "name", "qty", "code", "remark", "owner_id"]
    );

    // 4. unique partial index
    apply_ok(
        &session,
        Some(&meta),
        vec![DdlOp::AddIndex {
            table: items.clone(),
            index: IndexDef {
                name: "tabler_ddl_items_name_live".into(),
                columns: vec!["name".into()],
                unique: true,
                method: Some("btree".into()),
                predicate: Some("qty > 0".into()),
                comment: None,
                include: vec![],
            },
        }],
    )
    .await;
    let meta = session.table_meta(&items).await.expect("meta");
    let idx = meta
        .indexes
        .iter()
        .find(|i| i.name == "tabler_ddl_items_name_live")
        .expect("index created");
    assert!(idx.unique && !idx.primary);
    assert_eq!(idx.columns, ["name"]);
    assert_eq!(idx.method.as_deref(), Some("btree"));
    assert!(
        idx.predicate.as_deref().unwrap_or("").contains("qty"),
        "{idx:?}"
    );

    // 5. change types (USING converts text to integer)
    apply_ok(
        &session,
        Some(&meta),
        vec![
            DdlOp::AlterColumnType {
                table: items.clone(),
                name: "code".into(),
                new_type: "integer".into(),
                using: Some("code::integer".into()),
            },
            DdlOp::AlterColumnType {
                table: items.clone(),
                name: "qty".into(),
                new_type: "bigint".into(),
                using: None,
            },
        ],
    )
    .await;
    let meta = session.table_meta(&items).await.expect("meta");
    assert_eq!(column(&meta, "code").data_type, "integer");
    assert_eq!(column(&meta, "qty").data_type, "bigint");
    assert_eq!(
        scalar(&session, &format!("SELECT sum(code)::text FROM {q_items}")).await,
        30
    );

    // 6. foreign key to the second table
    apply_ok(
        &session,
        Some(&meta),
        vec![DdlOp::AddForeignKey {
            table: items.clone(),
            fk: FkDef {
                name: Some("tabler_ddl_items_owner_fk".into()),
                columns: vec!["owner_id".into()],
                ref_table: owners.clone(),
                ref_columns: vec!["id".into()],
                on_update: None,
                on_delete: Some("CASCADE".into()),
            },
        }],
    )
    .await;
    let meta = session.table_meta(&items).await.expect("meta");
    assert_eq!(meta.foreign_keys.len(), 1);
    let fk = &meta.foreign_keys[0];
    assert_eq!(fk.name, "tabler_ddl_items_owner_fk");
    assert_eq!(fk.ref_table.name, "tabler_ddl_owners");
    assert_eq!(fk.on_delete, "CASCADE");

    // 7. drop a column
    apply_ok(
        &session,
        Some(&meta),
        vec![DdlOp::DropColumn {
            table: items.clone(),
            name: "remark".into(),
        }],
    )
    .await;
    let meta = session.table_meta(&items).await.expect("meta");
    assert_eq!(names(&meta), ["id", "name", "qty", "code", "owner_id"]);

    // 8. a failing transactional plan rolls back completely
    let plan = plan_ddl(
        Dialect::Postgres,
        Some(&meta),
        &[
            DdlOp::SetTableComment {
                table: items.clone(),
                comment: Some("should roll back".into()),
            },
            // owner_id holds NULLs
            DdlOp::AlterColumnNullable {
                table: items.clone(),
                name: "owner_id".into(),
                nullable: false,
            },
        ],
    )
    .expect("plan");
    let result = apply_plan(session.as_ref(), &plan).await;
    assert_eq!(result.failed_index, Some(1), "{result:?}");
    assert!(result.executed.is_empty());
    let err = result.error.expect("error payload");
    assert_eq!(err.statement_index, Some(1));
    assert_eq!(err.sqlstate.as_deref(), Some("23502"), "{err:?}");
    let meta = session.table_meta(&items).await.expect("meta");
    assert_eq!(meta.comment, None);

    // 9. primary key swap, comments, defaults
    apply_ok(
        &session,
        Some(&meta),
        vec![
            DdlOp::DropPrimaryKey {
                table: items.clone(),
            },
            DdlOp::AddPrimaryKey {
                table: items.clone(),
                columns: vec!["id".into(), "name".into()],
            },
            DdlOp::SetTableComment {
                table: items.clone(),
                comment: Some("stock".into()),
            },
            DdlOp::SetColumnComment {
                table: items.clone(),
                name: "qty".into(),
                comment: Some("how many".into()),
            },
            DdlOp::AlterColumnDefault {
                table: items.clone(),
                name: "qty".into(),
                default_expr: None,
            },
        ],
    )
    .await;
    let meta = session.table_meta(&items).await.expect("meta");
    assert_eq!(meta.primary_key, ["id", "name"]);
    assert_eq!(meta.comment.as_deref(), Some("stock"));
    assert_eq!(column(&meta, "qty").comment.as_deref(), Some("how many"));
    assert_eq!(column(&meta, "qty").default_expr, None);

    // 10. reconstructed DDL recreates an equivalent table
    let ddl = create_table_sql(Dialect::Postgres, &meta);
    assert!(ddl.contains("GENERATED BY DEFAULT AS IDENTITY"), "{ddl}");
    assert!(
        ddl.contains("CREATE UNIQUE INDEX tabler_ddl_items_name_live"),
        "{ddl}"
    );
    assert!(
        ddl.contains("ADD CONSTRAINT \"tabler_ddl_items_owner_fk\" FOREIGN KEY"),
        "{ddl}"
    );
    let copy_schema = format!("{schema}_copy");
    session
        .execute(&format!("CREATE SCHEMA \"{copy_schema}\""), &[])
        .await
        .expect("copy schema");
    let copy_ddl = ddl
        .replace(&format!("\"{schema}\""), &format!("\"{copy_schema}\""))
        .replace(&format!("{schema}."), &format!("\"{copy_schema}\"."));
    let copied = async {
        session
            .execute(
                &format!(
                    "CREATE TABLE \"{copy_schema}\".tabler_ddl_owners (id integer PRIMARY KEY)"
                ),
                &[],
            )
            .await?;
        for statement in copy_ddl.split(";\n") {
            let statement = statement.trim().trim_end_matches(';');
            if !statement.is_empty() {
                session.execute(statement, &[]).await?;
            }
        }
        session
            .table_meta(&TableRef::new(Some(&copy_schema), "tabler_ddl_items"))
            .await
    }
    .await;
    let _ = session
        .execute(&format!("DROP SCHEMA \"{copy_schema}\" CASCADE"), &[])
        .await;
    let copy = copied.expect("replay reconstructed DDL");
    assert_eq!(names(&copy), names(&meta));
    assert_eq!(copy.primary_key, meta.primary_key);
    assert_eq!(copy.comment.as_deref(), Some("stock"));

    // 11. truncate and drop
    apply_ok(
        &session,
        Some(&meta),
        vec![DdlOp::TruncateTable {
            table: items.clone(),
            restart_identity: true,
            cascade: false,
        }],
    )
    .await;
    assert_eq!(
        scalar(&session, &format!("SELECT count(*)::text FROM {q_items}")).await,
        0
    );
    apply_ok(
        &session,
        None,
        vec![
            DdlOp::DropTable {
                table: items.clone(),
                if_exists: false,
                cascade: false,
            },
            DdlOp::DropTable {
                table: owners.clone(),
                if_exists: true,
                cascade: true,
            },
        ],
    )
    .await;
    let tables = session.list_tables(&schema).await.expect("list");
    assert!(tables.is_empty(), "{tables:?}");
}

/// Runs `body` inside a fresh scratch schema `tabler_ddl_<random>` and drops it afterwards.
async fn in_scratch_schema<F, Fut>(body: F)
where
    F: FnOnce(Arc<dyn DbSession>, String) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let Some(session) = open().await else {
        return;
    };
    let schema = format!(
        "tabler_ddl_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    session
        .execute(&format!("CREATE SCHEMA \"{schema}\""), &[])
        .await
        .expect("create scratch schema");
    let outcome = tokio::spawn(body(session.clone(), schema.clone())).await;
    // the editor may still hold a transaction when the body failed
    let mut sink = CollectSink::default();
    let _ = session
        .run_statement("ROLLBACK", 10, &mut sink, CancelSlot::default())
        .await;
    let _ = session
        .execute(&format!("DROP SCHEMA IF EXISTS \"{schema}\" CASCADE"), &[])
        .await;
    session.close().await;
    if let Err(e) = outcome {
        std::panic::resume_unwind(e.into_panic());
    }
}

async fn editor(session: &Arc<dyn DbSession>, sql: &str) {
    let mut sink = CollectSink::default();
    session
        .run_statement(sql, 100, &mut sink, CancelSlot::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

/// PostgreSQL refuses DROP INDEX on the index of a UNIQUE constraint ("constraint … requires
/// it"); dropping that index drops the constraint.
#[tokio::test]
async fn dropping_the_index_of_a_unique_constraint() {
    in_scratch_schema(|session, schema| async move {
        let table = TableRef::new(Some(&schema), "t");
        session
            .execute(
                &format!(
                    "CREATE TABLE \"{schema}\".t (id int PRIMARY KEY, email text UNIQUE, code text)"
                ),
                &[],
            )
            .await
            .expect("create");
        session
            .execute(
                &format!("CREATE INDEX t_code ON \"{schema}\".t (code)"),
                &[],
            )
            .await
            .expect("index");
        let meta = session.table_meta(&table).await.expect("meta");
        assert!(meta.indexes.iter().any(|i| i.name == "t_email_key"));
        let ops = [
            DdlOp::DropIndex {
                table: table.clone(),
                name: "t_email_key".into(),
            },
            DdlOp::DropIndex {
                table: table.clone(),
                name: "t_code".into(),
            },
        ];
        let plan = plan_on(session.as_ref(), Some(&meta), &ops)
            .await
            .expect("plan");
        assert_eq!(
            plan.statements,
            [
                format!("ALTER TABLE \"{schema}\".\"t\" DROP CONSTRAINT \"t_email_key\""),
                format!("DROP INDEX \"{schema}\".\"t_code\""),
            ]
        );
        let result = apply_plan(session.as_ref(), &plan).await;
        assert!(result.error.is_none(), "{result:?}");
        let meta = session.table_meta(&table).await.expect("meta");
        let left: Vec<&str> = meta.indexes.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(left, ["t_pkey"]);
        // duplicates are allowed now
        session
            .execute(
                &format!("INSERT INTO \"{schema}\".t VALUES (1, 'a', NULL), (2, 'a', NULL)"),
                &[],
            )
            .await
            .expect("duplicate emails");
    })
    .await;
}

/// DDL used to wait without limit behind a lock, here the SQL editor's own open transaction,
/// while every other reader of the table queued up behind it.
#[tokio::test]
async fn ddl_behind_the_editor_transaction_times_out() {
    in_scratch_schema(|session, schema| async move {
        let table = TableRef::new(Some(&schema), "t");
        session
            .execute(&format!("CREATE TABLE \"{schema}\".t (id int)"), &[])
            .await
            .expect("create");
        let meta = session.table_meta(&table).await.expect("meta");
        let plan = plan_ddl(
            Dialect::Postgres,
            Some(&meta),
            &[DdlOp::AddColumn {
                table: table.clone(),
                column: col("extra", "int", true),
                after: None,
            }],
        )
        .expect("plan");

        editor(&session, "BEGIN").await;
        editor(&session, &format!("SELECT * FROM \"{schema}\".t")).await;
        let started = std::time::Instant::now();
        let result = apply_plan(session.as_ref(), &plan).await;
        let waited = started.elapsed();
        assert!(
            waited < std::time::Duration::from_secs(DDL_LOCK_TIMEOUT_SECS + 10),
            "{waited:?}"
        );
        assert_eq!(result.failed_index, Some(0), "{result:?}");
        let err = result.error.expect("lock timeout");
        assert_eq!(err.sqlstate.as_deref(), Some("55P03"), "{err:?}");
        assert!(
            err.hint.as_deref().unwrap_or("").contains("SQL editor"),
            "{err:?}"
        );
        // other readers are not blocked once the DDL gave up
        let count = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            session.query(&format!("SELECT count(*)::text FROM \"{schema}\".t"), &[]),
        )
        .await
        .expect("reader not blocked");
        assert!(count.is_ok(), "{count:?}");
        editor(&session, "ROLLBACK").await;

        let result = apply_plan(session.as_ref(), &plan).await;
        assert!(result.error.is_none(), "{result:?}");
        let meta = session.table_meta(&table).await.expect("meta");
        assert_eq!(names(&meta), ["id", "extra"]);
    })
    .await;
}

#[tokio::test]
async fn plans_and_applies_ddl() {
    let Some(session) = open().await else {
        return;
    };
    let schema = format!(
        "tabler_ddl_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    session
        .execute(&format!("CREATE SCHEMA \"{schema}\""), &[])
        .await
        .expect("create scratch schema");
    let outcome = tokio::spawn(steps(session.clone(), schema.clone())).await;
    for s in [schema.clone(), format!("{schema}_copy")] {
        let _ = session
            .execute(&format!("DROP SCHEMA IF EXISTS \"{s}\" CASCADE"), &[])
            .await;
    }
    session.close().await;
    if let Err(e) = outcome {
        std::panic::resume_unwind(e.into_panic());
    }
}
