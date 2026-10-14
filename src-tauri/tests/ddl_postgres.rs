//! DDL integration test for PostgreSQL: plans with `plan_ddl`, runs with `apply_plan`, and
//! re-introspects after every step. Runs only when `TABLER_TEST_PG_URL` is set; everything
//! happens inside a scratch schema `tabler_ddl_<random>` (the grouped-ALTER tests: `fx3_<random>`)
//! that is dropped at the end.

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
    in_prefixed_schema("tabler_ddl", body).await;
}

/// `in_scratch_schema` with the schema name `<stem>_<random>`.
async fn in_prefixed_schema<F, Fut>(stem: &str, body: F)
where
    F: FnOnce(Arc<dyn DbSession>, String) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let Some(session) = open().await else {
        return;
    };
    let schema = format!(
        "{stem}_{}",
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

// --- grouped ALTER TABLE (the ops of one table are one statement) ------------------------------

/// What a schema comparison looks at: columns (in order) with their type, nullability, default
/// and comment, then the indexes, checks and foreign keys.
fn shape(meta: &TableMeta) -> String {
    let mut out = String::new();
    for c in &meta.columns {
        out += &format!(
            "col {} {} null={} default={:?} extra={:?} comment={:?}\n",
            c.name, c.data_type, c.nullable, c.default_expr, c.extra, c.comment
        );
    }
    for i in &meta.indexes {
        out += &format!(
            "idx {} {:?} unique={} primary={}\n",
            i.name, i.columns, i.unique, i.primary
        );
    }
    for k in &meta.checks {
        out += &format!("check {} {}\n", k.name, k.expression);
    }
    for f in &meta.foreign_keys {
        out += &format!(
            "fk {} {:?} -> {}.{:?}\n",
            f.name, f.columns, f.ref_table.name, f.ref_columns
        );
    }
    out
}

/// One `ALTER TABLE` with several clauses is all or nothing, and its result reads "All changes
/// were reverted": the table stays exactly as it was when the last clause is refused.
#[tokio::test]
async fn a_grouped_plan_whose_last_clause_fails_leaves_the_table_as_it_was() {
    in_prefixed_schema("fx3", |session, schema| async move {
        let table = TableRef::new(Some(&schema), "a");
        session
            .execute(
                &format!(
                    "CREATE TABLE \"{schema}\".a (id int PRIMARY KEY, name text, qty int NOT NULL DEFAULT 0)"
                ),
                &[],
            )
            .await
            .expect("create");
        session
            .execute(&format!("INSERT INTO \"{schema}\".a VALUES (1, 'x', 5)"), &[])
            .await
            .expect("insert");
        let before = session.table_meta(&table).await.expect("meta");
        let ops = vec![
            DdlOp::AddColumn {
                table: table.clone(),
                column: col("extra", "integer", true),
                after: None,
            },
            DdlOp::AlterColumnDefault {
                table: table.clone(),
                name: "qty".into(),
                default_expr: Some("9".into()),
            },
            DdlOp::AlterColumnNullable {
                table: table.clone(),
                name: "name".into(),
                nullable: false,
            },
            // the row has qty = 5: refused, the last clause
            DdlOp::AddCheck {
                table: table.clone(),
                check: tabler_lib::types::ddl::CheckDef {
                    name: Some("fx3_qty_big".into()),
                    expression: "qty > 100".into(),
                },
            },
        ];
        let plan = plan_ddl(Dialect::Postgres, Some(&before), &ops).expect("plan");
        assert_eq!(plan.statements.len(), 1, "{:#?}", plan.statements);
        assert!(plan.transactional);
        let result = apply_plan(session.as_ref(), &plan).await;
        assert_eq!(result.failed_index, Some(0), "{result:?}");
        assert!(result.executed.is_empty(), "everything was reverted: {result:?}");
        let err = result.error.expect("error payload");
        assert_eq!(err.sqlstate.as_deref(), Some("23514"), "{err:?}");
        let after = session.table_meta(&table).await.expect("meta");
        assert_eq!(shape(&after), shape(&before));

        // without the bad clause the same plan applies
        apply_ok(&session, Some(&before), ops[..3].to_vec()).await;
        let after = session.table_meta(&table).await.expect("meta");
        assert_eq!(names(&after), ["id", "name", "qty", "extra"]);
        assert!(!column(&after, "name").nullable);
        assert_eq!(column(&after, "qty").default_expr.as_deref(), Some("9"));
    })
    .await;
}

/// The grouped plan of a mixed edit leaves the same schema as the old one-statement-per-op plan,
/// including a primary-key swap, an edited CHECK and a drop followed by an add of one column.
#[tokio::test]
async fn a_grouped_plan_gives_the_schema_of_the_per_op_plan() {
    in_prefixed_schema("fx3", |session, schema| async move {
        session
            .execute(
                &format!("CREATE TABLE \"{schema}\".ref (id int PRIMARY KEY)"),
                &[],
            )
            .await
            .expect("ref");
        for name in ["g", "o"] {
            session
                .execute(
                    &format!(
                        "CREATE TABLE \"{schema}\".{name} (\
                           id int PRIMARY KEY, \
                           name text, \
                           qty int NOT NULL DEFAULT 0, \
                           note varchar(10), \
                           legacy int, \
                           ref_id int, \
                           CONSTRAINT {name}_qty_chk CHECK (qty >= 0))"
                    ),
                    &[],
                )
                .await
                .expect("create");
            session
                .execute(
                    &format!("INSERT INTO \"{schema}\".{name} VALUES (1, 'n', 3, 'abc', 1, NULL)"),
                    &[],
                )
                .await
                .expect("insert");
        }
        let ops = |name: &str| {
            let t = TableRef::new(Some(&schema), name);
            vec![
                DdlOp::DropColumn {
                    table: t.clone(),
                    name: "legacy".into(),
                },
                // dropped and added again: the server drops first, which is the written order
                DdlOp::DropColumn {
                    table: t.clone(),
                    name: "ref_id".into(),
                },
                DdlOp::AddColumn {
                    table: t.clone(),
                    column: col("ref_id", "bigint", true),
                    after: None,
                },
                DdlOp::AddColumn {
                    table: t.clone(),
                    column: col("stamp", "timestamptz", true),
                    after: None,
                },
                DdlOp::AlterColumnNullable {
                    table: t.clone(),
                    name: "name".into(),
                    nullable: false,
                },
                DdlOp::AlterColumnDefault {
                    table: t.clone(),
                    name: "name".into(),
                    default_expr: Some("'anon'".into()),
                },
                DdlOp::AlterColumnType {
                    table: t.clone(),
                    name: "note".into(),
                    new_type: "varchar(40)".into(),
                    using: None,
                },
                DdlOp::AlterColumnType {
                    table: t.clone(),
                    name: "qty".into(),
                    new_type: "bigint".into(),
                    using: None,
                },
                DdlOp::AlterColumnDefault {
                    table: t.clone(),
                    name: "qty".into(),
                    default_expr: Some("7".into()),
                },
                // an edited CHECK and a swapped key
                DdlOp::DropCheck {
                    table: t.clone(),
                    name: format!("{name}_qty_chk"),
                },
                DdlOp::AddCheck {
                    table: t.clone(),
                    check: tabler_lib::types::ddl::CheckDef {
                        name: Some(format!("{name}_qty_chk")),
                        expression: "qty >= 1".into(),
                    },
                },
                DdlOp::DropPrimaryKey { table: t.clone() },
                DdlOp::AddPrimaryKey {
                    table: t.clone(),
                    columns: vec!["id".into(), "qty".into()],
                },
                DdlOp::AddForeignKey {
                    table: t.clone(),
                    fk: FkDef {
                        name: Some(format!("{name}_ref_fk")),
                        columns: vec!["ref_id".into()],
                        ref_table: TableRef::new(Some(&schema), "ref"),
                        ref_columns: vec!["id".into()],
                        on_update: None,
                        on_delete: None,
                    },
                },
                DdlOp::SetColumnComment {
                    table: t.clone(),
                    name: "note".into(),
                    comment: Some("free text".into()),
                },
            ]
        };
        let grouped = TableRef::new(Some(&schema), "g");
        let per_op = TableRef::new(Some(&schema), "o");

        let meta = session.table_meta(&grouped).await.expect("meta");
        let result = apply_ok(&session, Some(&meta), ops("g")).await;
        // three statements instead of sixteen: the type of qty ends the first (its DEFAULT
        // follows), the column comment cannot join
        assert_eq!(result.executed.len(), 3, "{:#?}", result.executed);

        for op in ops("o") {
            let meta = session.table_meta(&per_op).await.expect("meta");
            let plan = plan_ddl(Dialect::Postgres, Some(&meta), &[op]).expect("plan");
            let result = apply_plan(session.as_ref(), &plan).await;
            assert!(
                result.error.is_none(),
                "{:?}\n{:#?}",
                result.error,
                plan.statements
            );
        }

        let a = session.table_meta(&grouped).await.expect("meta");
        let b = session.table_meta(&per_op).await.expect("meta");
        let normal = |m: &TableMeta, name: &str| shape(m).replace(&format!("{name}_"), "T_");
        assert_eq!(normal(&a, "g"), normal(&b, "o"));
        assert_eq!(a.primary_key, ["id", "qty"]);
        assert_eq!(column(&a, "qty").data_type, "bigint");
        assert_eq!(column(&a, "qty").default_expr.as_deref(), Some("7"));
        assert_eq!(column(&a, "note").data_type, "character varying(40)");
        assert_eq!(column(&a, "note").comment.as_deref(), Some("free text"));
        assert_eq!(
            column(&a, "name").default_expr.as_deref(),
            Some("'anon'::text")
        );
        assert!(!column(&a, "name").nullable);
        assert!(a.columns.iter().all(|c| c.name != "legacy"));
    })
    .await;
}

/// PostgreSQL runs the sub-commands of one `ALTER TABLE` in phases, not in the written order:
/// `ALTER COLUMN … TYPE` before `ADD COLUMN`. A type change that reads a column added just before
/// (`USING x`) fails as one statement, so the planner ends the statement at it; and a type change
/// never shares a statement with another clause on its own column.
#[tokio::test]
async fn a_type_change_keeps_its_place_in_the_order_of_the_ops() {
    in_prefixed_schema("fx3", |session, schema| async move {
        let table = TableRef::new(Some(&schema), "t");
        session
            .execute(
                &format!(
                    "CREATE TABLE \"{schema}\".t (id int PRIMARY KEY, c text DEFAULT 'abc', d text)"
                ),
                &[],
            )
            .await
            .expect("create");
        session
            .execute(
                &format!("INSERT INTO \"{schema}\".t VALUES (1, 'abc', '12')"),
                &[],
            )
            .await
            .expect("insert");
        // the failure the rule prevents: the type change runs before the column exists
        let err = session
            .execute(
                &format!(
                    "ALTER TABLE \"{schema}\".t ADD COLUMN x int NOT NULL DEFAULT 2, ALTER COLUMN d TYPE integer USING length(d) * x"
                ),
                &[],
            )
            .await
            .expect_err("PostgreSQL runs TYPE before ADD COLUMN");
        assert!(err.to_string().contains("\"x\""), "{err}");

        let meta = session.table_meta(&table).await.expect("meta");
        let mut x = col("x", "integer", false);
        x.default_expr = Some("2".into());
        let ops = vec![
            DdlOp::AddColumn {
                table: table.clone(),
                column: x,
                after: None,
            },
            DdlOp::AlterColumnType {
                table: table.clone(),
                name: "d".into(),
                new_type: "integer".into(),
                using: Some("length(d) * x".into()),
            },
        ];
        let plan = plan_ddl(Dialect::Postgres, Some(&meta), &ops).expect("plan");
        assert_eq!(plan.statements.len(), 2, "{:#?}", plan.statements);
        apply_ok(&session, Some(&meta), ops).await;
        let after = session.table_meta(&table).await.expect("meta");
        assert_eq!(column(&after, "d").data_type, "integer");
        assert_eq!(
            scalar(&session, &format!("SELECT d FROM \"{schema}\".t")).await,
            4
        );

        // a type, a default and a nullability of one column: the type stands alone
        let ops = vec![
            DdlOp::AlterColumnType {
                table: table.clone(),
                name: "c".into(),
                new_type: "varchar(20)".into(),
                using: None,
            },
            DdlOp::AlterColumnDefault {
                table: table.clone(),
                name: "c".into(),
                default_expr: Some("'xyz'".into()),
            },
            DdlOp::AlterColumnNullable {
                table: table.clone(),
                name: "c".into(),
                nullable: false,
            },
        ];
        let plan = plan_ddl(Dialect::Postgres, Some(&after), &ops).expect("plan");
        assert_eq!(plan.statements.len(), 2, "{:#?}", plan.statements);
        apply_ok(&session, Some(&after), ops).await;
        let after = session.table_meta(&table).await.expect("meta");
        let c = column(&after, "c");
        assert_eq!(c.data_type, "character varying(20)");
        assert!(!c.nullable);
        assert!(
            c.default_expr.as_deref().unwrap_or("").starts_with("'xyz'"),
            "{:?}",
            c.default_expr
        );

        // other columns share a statement with it, before and after
        let ops = vec![
            DdlOp::AlterColumnNullable {
                table: table.clone(),
                name: "c".into(),
                nullable: true,
            },
            DdlOp::AlterColumnType {
                table: table.clone(),
                name: "d".into(),
                new_type: "bigint".into(),
                using: None,
            },
            DdlOp::AlterColumnDefault {
                table: table.clone(),
                name: "c".into(),
                default_expr: Some("'q'".into()),
            },
        ];
        let plan = plan_ddl(Dialect::Postgres, Some(&after), &ops).expect("plan");
        assert_eq!(plan.statements.len(), 1, "{:#?}", plan.statements);
        apply_ok(&session, Some(&after), ops).await;
        let after = session.table_meta(&table).await.expect("meta");
        assert_eq!(column(&after, "d").data_type, "bigint");
        assert!(column(&after, "c").nullable);
    })
    .await;
}

/// What MySQL refuses in one statement is fine here (the same plans are two statements there):
/// a column re-added under the name of a dropped one that an index covers (the index goes with
/// the column), and a foreign key dropped and added under one name. Both stay one statement.
#[tokio::test]
async fn a_dropped_and_re_added_column_or_foreign_key_is_one_statement() {
    in_prefixed_schema("fx3", |session, schema| async move {
        let table = TableRef::new(Some(&schema), "t");
        for sql in [
            format!("CREATE TABLE \"{schema}\".ref (id int PRIMARY KEY)"),
            format!(
                "CREATE TABLE \"{schema}\".t (id int PRIMARY KEY, a int, b int, \
                   CONSTRAINT t_fk FOREIGN KEY (a) REFERENCES \"{schema}\".ref (id))"
            ),
            format!("CREATE INDEX t_ix ON \"{schema}\".t (a, b)"),
        ] {
            session.execute(&sql, &[]).await.expect(&sql);
        }
        let meta = session.table_meta(&table).await.expect("meta");
        let ops = vec![
            DdlOp::DropForeignKey {
                table: table.clone(),
                name: "t_fk".into(),
            },
            DdlOp::AddForeignKey {
                table: table.clone(),
                fk: FkDef {
                    name: Some("t_fk".into()),
                    columns: vec!["a".into()],
                    ref_table: TableRef::new(Some(&schema), "ref"),
                    ref_columns: vec!["id".into()],
                    on_update: None,
                    on_delete: Some("cascade".into()),
                },
            },
            DdlOp::DropColumn {
                table: table.clone(),
                name: "b".into(),
            },
            DdlOp::AddColumn {
                table: table.clone(),
                column: col("b", "bigint", true),
                after: None,
            },
        ];
        let plan = plan_ddl(Dialect::Postgres, Some(&meta), &ops).expect("plan");
        assert_eq!(plan.statements.len(), 1, "{:#?}", plan.statements);
        apply_ok(&session, Some(&meta), ops).await;
        let after = session.table_meta(&table).await.expect("meta");
        assert_eq!(column(&after, "b").data_type, "bigint");
        assert_eq!(after.foreign_keys.len(), 1);
        assert_eq!(after.foreign_keys[0].name, "t_fk");
        assert_eq!(
            after.foreign_keys[0].on_delete.to_ascii_uppercase(),
            "CASCADE"
        );
        // the index covered the dropped column and went with it
        assert!(
            after.indexes.iter().all(|i| i.name != "t_ix"),
            "{:?}",
            after.indexes
        );
    })
    .await;
}

/// `ADD CONSTRAINT ck CHECK (b > 0), DROP COLUMN b` in one `ALTER TABLE` is refused by the server
/// (PostgreSQL runs the drop pass first); the planner keeps them in two statements, in order.
#[tokio::test]
async fn a_check_added_before_the_drop_of_its_column_is_two_statements() {
    in_prefixed_schema("fx3", |session, schema| async move {
        let table = TableRef::new(Some(&schema), "t");
        session
            .execute(
                &format!("CREATE TABLE \"{schema}\".t (id int PRIMARY KEY, a int, b int)"),
                &[],
            )
            .await
            .expect("create");
        let meta = session.table_meta(&table).await.expect("meta");
        let ops = vec![
            DdlOp::AddCheck {
                table: table.clone(),
                check: tabler_lib::types::ddl::CheckDef {
                    name: Some("ck_b".into()),
                    expression: "b > 0".into(),
                },
            },
            DdlOp::DropColumn {
                table: table.clone(),
                name: "b".into(),
            },
        ];
        let plan = plan_ddl(Dialect::Postgres, Some(&meta), &ops).expect("plan");
        assert_eq!(plan.statements.len(), 2, "{:#?}", plan.statements);
        apply_ok(&session, Some(&meta), ops).await;
        let after = session.table_meta(&table).await.expect("meta");
        assert!(after.columns.iter().all(|c| c.name != "b"));
    })
    .await;
}
