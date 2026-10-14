//! DDL integration test for MySQL / MariaDB: plans with `plan_ddl`, runs with `apply_plan`
//! (statement by statement: MySQL DDL auto-commits; a plan of one statement runs in a
//! transaction and fails as a whole), re-introspects after every step. Runs only when
//! `TABLER_TEST_MYSQL_URL` is set; scratch tables are `tabler_ddl_<random>_*` (and
//! `fx3_<random>_*` for the grouped-ALTER tests).

use std::sync::Arc;

use tabler_lib::commands::ddl::{DDL_LOCK_TIMEOUT_SECS, apply_plan};
use tabler_lib::drivers::{CancelSlot, CollectSink, ConnectTarget, DbSession, connect};
use tabler_lib::sqlgen::Dialect;
use tabler_lib::sqlgen::ddl::{create_table_sql, plan_ddl};
use tabler_lib::types::ddl::{ColumnDef, DdlOp, DdlResult, FkDef, IndexDef};
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
use tabler_lib::types::schema::{ColumnMeta, TableMeta, TableRef};
use tabler_lib::types::value::Value;

const ENV: &str = "TABLER_TEST_MYSQL_URL";

fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parses `mysql://user[:pass]@host:port/db` into a profile plus the password.
fn profile_from_url(url: &str) -> (ConnectionProfile, Option<String>) {
    let rest = url
        .strip_prefix("mysql://")
        .unwrap_or_else(|| panic!("{ENV} must start with mysql://"));
    let (creds, hostdb) = match rest.rsplit_once('@') {
        Some((c, h)) => (Some(c), h),
        None => (None, rest),
    };
    let (user, pass) = match creds {
        Some(c) => match c.split_once(':') {
            Some((u, p)) => (Some(pct_decode(u)), Some(pct_decode(p))),
            None => (Some(pct_decode(c)), None),
        },
        None => (None, None),
    };
    let (hostport, db) = match hostdb.split_once('/') {
        Some((h, d)) => (h, Some(d.to_string()).filter(|d| !d.is_empty())),
        None => (hostdb, None),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
        None => (hostport.to_string(), None),
    };
    let profile = ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "mysql-ddl-test".into(),
        color: None,
        group: None,
        driver: Driver::Mysql,
        host: Some(host),
        port,
        user,
        database: db,
        ssl: SslMode::Disable,
        ssl_ca_path: None,
        sqlite_path: None,
        ssh: None,
        options: ProfileOptions {
            connect_timeout_secs: Some(10),
            ..ProfileOptions::default()
        },
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    };
    (profile, pass)
}

async fn open() -> Option<Arc<dyn DbSession>> {
    let Ok(url) = std::env::var(ENV) else {
        println!("skipped: {ENV} unset");
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

async fn apply_ok(
    session: &Arc<dyn DbSession>,
    meta: Option<&TableMeta>,
    ops: Vec<DdlOp>,
) -> DdlResult {
    let plan = plan_ddl(Dialect::Mysql, meta, &ops).expect("plan");
    // MySQL DDL auto-commits: only a plan of one statement (all or nothing) is transactional
    assert_eq!(plan.transactional, plan.statements.len() == 1, "{plan:?}");
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

async fn steps(session: Arc<dyn DbSession>, prefix: String) {
    let owners = TableRef::new(None, &format!("{prefix}_owners"));
    let items = TableRef::new(None, &format!("{prefix}_items"));
    let q_items = format!("`{prefix}_items`");

    // 1. create both tables
    let mut owner_id = col("id", "int", false);
    owner_id.auto_increment = true;
    let mut id = col("id", "int", false);
    id.auto_increment = true;
    id.unsigned = true;
    let mut qty = col("qty", "int", false);
    qty.unsigned = true;
    qty.default_expr = Some("0".into());
    qty.comment = Some("how many".into());
    let mut name = col("name", "varchar(50)", false);
    name.collation = Some("utf8mb4_bin".into());
    apply_ok(
        &session,
        None,
        vec![
            DdlOp::CreateTable {
                table: owners.clone(),
                columns: vec![owner_id, col("label", "varchar(20)", true)],
                primary_key: vec!["id".into()],
                if_not_exists: false,
            },
            DdlOp::CreateTable {
                table: items.clone(),
                columns: vec![id, name, qty, col("code", "varchar(10)", true)],
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
    let meta = session.table_meta(&items).await.expect("meta");
    assert_eq!(names(&meta), ["id", "name", "qty", "code"]);
    assert!(column(&meta, "id").is_auto_increment);
    assert!(column(&meta, "qty").is_unsigned);
    assert_eq!(
        column(&meta, "name").collation.as_deref(),
        Some("utf8mb4_bin")
    );

    // 2. add columns: positioned, with default / not null / comment
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
                after: Some("name".into()),
            },
            DdlOp::AddColumn {
                table: items.clone(),
                column: col("owner_id", "int", true),
                after: None,
            },
        ],
    )
    .await;
    let meta = session.table_meta(&items).await.expect("meta");
    assert_eq!(
        names(&meta),
        ["id", "name", "note", "qty", "code", "owner_id"]
    );
    let note = column(&meta, "note");
    assert!(!note.nullable);
    assert_eq!(note.default_expr.as_deref(), Some("n/a"));
    assert_eq!(note.comment.as_deref(), Some("free text"));

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
        ["id", "name", "remark", "qty", "code", "owner_id"]
    );

    // 4. unique index (prefix length on the name)
    apply_ok(
        &session,
        Some(&meta),
        vec![DdlOp::AddIndex {
            table: items.clone(),
            index: IndexDef {
                name: "tabler_ddl_items_name".into(),
                columns: vec!["name(20)".into(), "qty".into()],
                unique: true,
                method: None,
                predicate: None,
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
        .find(|i| i.name == "tabler_ddl_items_name")
        .expect("index created");
    assert!(idx.unique);
    assert_eq!(idx.columns, ["name(20)", "qty"]);

    // 5. MODIFY keeps unsigned / auto_increment / default / comment / collation
    apply_ok(
        &session,
        Some(&meta),
        vec![
            DdlOp::AlterColumnType {
                table: items.clone(),
                name: "qty".into(),
                new_type: "bigint".into(),
                using: None,
            },
            DdlOp::AlterColumnType {
                table: items.clone(),
                name: "id".into(),
                new_type: "bigint".into(),
                using: None,
            },
            DdlOp::AlterColumnNullable {
                table: items.clone(),
                name: "name".into(),
                nullable: true,
            },
            DdlOp::AlterColumnType {
                table: items.clone(),
                name: "code".into(),
                new_type: "int".into(),
                using: None,
            },
        ],
    )
    .await;
    let meta = session.table_meta(&items).await.expect("meta");
    let qty = column(&meta, "qty");
    assert_eq!(qty.data_type, "bigint unsigned");
    assert!(!qty.nullable);
    assert_eq!(qty.default_expr.as_deref(), Some("0"));
    assert_eq!(qty.comment.as_deref(), Some("how many"));
    let id = column(&meta, "id");
    assert_eq!(id.data_type, "bigint unsigned");
    assert!(id.is_auto_increment);
    let name = column(&meta, "name");
    assert!(name.nullable);
    assert_eq!(name.collation.as_deref(), Some("utf8mb4_bin"));
    assert_eq!(column(&meta, "code").data_type, "int");
    assert_eq!(
        scalar(
            &session,
            &format!("SELECT CAST(SUM(code) AS SIGNED) FROM {q_items}")
        )
        .await,
        30
    );

    // 6. foreign key to the second table
    apply_ok(
        &session,
        Some(&meta),
        vec![DdlOp::AddForeignKey {
            table: items.clone(),
            fk: FkDef {
                name: Some(format!("{prefix}_owner_fk")),
                columns: vec!["owner_id".into()],
                ref_table: owners.clone(),
                ref_columns: vec!["id".into()],
                on_update: Some("cascade".into()),
                on_delete: Some("cascade".into()),
            },
        }],
    )
    .await;
    let meta = session.table_meta(&items).await.expect("meta");
    assert_eq!(meta.foreign_keys.len(), 1, "{:?}", meta.foreign_keys);
    let fk = &meta.foreign_keys[0];
    assert_eq!(fk.ref_table.name, owners.name);
    assert_eq!(fk.columns, ["owner_id"]);
    assert_eq!(fk.on_delete, "CASCADE");
    assert_eq!(fk.on_update, "CASCADE");

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

    // 8. sequential plans stop at the first failure; earlier statements stay committed
    let plan = plan_ddl(
        Dialect::Mysql,
        Some(&meta),
        &[
            DdlOp::SetTableComment {
                table: items.clone(),
                comment: Some("stock".into()),
            },
            DdlOp::RawSql {
                sql: format!("ALTER TABLE {q_items} ADD COLUMN `x` int"),
            },
            DdlOp::RawSql {
                sql: format!("ALTER TABLE {q_items} ADD COLUMN `x` int"),
            },
            DdlOp::DropColumn {
                table: items.clone(),
                name: "x".into(),
            },
        ],
    )
    .expect("plan");
    let result = apply_plan(session.as_ref(), &plan).await;
    assert_eq!(result.failed_index, Some(2), "{result:?}");
    assert_eq!(result.executed, plan.statements[..2]);
    let err = result.error.expect("error payload");
    assert_eq!(err.vendor_code, Some(1060), "{err:?}"); // duplicate column
    assert_eq!(err.statement_index, Some(2));
    let meta = session.table_meta(&items).await.expect("meta");
    assert!(meta.columns.iter().any(|c| c.name == "x"));
    assert_eq!(meta.comment.as_deref(), Some("stock"));

    // 9. drop / re-add the primary key, defaults; the reconstruction mentions what we built
    apply_ok(
        &session,
        Some(&meta),
        vec![
            DdlOp::DropColumn {
                table: items.clone(),
                name: "x".into(),
            },
            DdlOp::AlterColumnDefault {
                table: items.clone(),
                name: "qty".into(),
                default_expr: Some("5".into()),
            },
        ],
    )
    .await;
    let meta = session.table_meta(&items).await.expect("meta");
    assert_eq!(column(&meta, "qty").default_expr.as_deref(), Some("5"));
    let ddl = create_table_sql(Dialect::Mysql, &meta);
    assert!(
        ddl.contains("`qty` bigint unsigned NOT NULL DEFAULT 5 COMMENT 'how many'"),
        "{ddl}"
    );
    assert!(
        ddl.contains(&format!("ADD CONSTRAINT `{prefix}_owner_fk` FOREIGN KEY")),
        "{ddl}"
    );

    // 10. truncate, rename, drop
    let renamed = TableRef::new(None, &format!("{prefix}_goods"));
    apply_ok(
        &session,
        Some(&meta),
        vec![
            DdlOp::TruncateTable {
                table: items.clone(),
                restart_identity: true,
                cascade: false,
            },
            DdlOp::RenameTable {
                table: items.clone(),
                new_name: renamed.name.clone(),
            },
        ],
    )
    .await;
    assert_eq!(
        scalar(
            &session,
            &format!("SELECT COUNT(*) FROM `{}`", renamed.name)
        )
        .await,
        0
    );
    apply_ok(
        &session,
        None,
        vec![
            DdlOp::DropTable {
                table: renamed.clone(),
                if_exists: false,
                cascade: false,
            },
            DdlOp::DropTable {
                table: owners.clone(),
                if_exists: true,
                cascade: false,
            },
        ],
    )
    .await;
}

/// Runs `body` with a fresh table-name prefix `tabler_ddl_<random>` and drops the tables named
/// `<prefix>_<suffix>` for each of `tables` afterwards.
async fn with_scratch_tables<F, Fut>(tables: &[&str], body: F)
where
    F: FnOnce(Arc<dyn DbSession>, String) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    with_prefixed_tables("tabler_ddl", tables, body).await;
}

/// `with_scratch_tables` with the prefix `<stem>_<random>`.
async fn with_prefixed_tables<F, Fut>(stem: &str, tables: &[&str], body: F)
where
    F: FnOnce(Arc<dyn DbSession>, String) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let Some(session) = open().await else {
        return;
    };
    let prefix = format!(
        "{stem}_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..10]
    );
    let outcome = tokio::spawn(body(session.clone(), prefix.clone())).await;
    // the editor may still hold a transaction (and metadata locks) when the body failed
    let mut sink = CollectSink::default();
    let _ = session
        .run_statement("ROLLBACK", 10, &mut sink, CancelSlot::default())
        .await;
    for table in tables {
        let _ = session
            .execute(&format!("DROP TABLE IF EXISTS `{prefix}_{table}`"), &[])
            .await;
    }
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

/// Dropping the primary key on its own fails when an AUTO_INCREMENT column needs it (1075):
/// a drop followed by an add runs as one statement.
#[tokio::test]
async fn primary_key_swap_on_an_auto_increment_table() {
    with_scratch_tables(&["k"], |session, prefix| async move {
        let table = TableRef::new(None, &format!("{prefix}_k"));
        session
            .execute(
                &format!(
                    "CREATE TABLE `{}` (id int NOT NULL AUTO_INCREMENT PRIMARY KEY, tenant int NOT NULL)",
                    table.name
                ),
                &[],
            )
            .await
            .expect("create");
        let meta = session.table_meta(&table).await.expect("meta");
        let result = apply_ok(
            &session,
            Some(&meta),
            vec![
                DdlOp::DropPrimaryKey {
                    table: table.clone(),
                },
                DdlOp::AddPrimaryKey {
                    table: table.clone(),
                    columns: vec!["id".into(), "tenant".into()],
                },
            ],
        )
        .await;
        assert_eq!(result.executed.len(), 1, "{result:?}");
        let meta = session.table_meta(&table).await.expect("meta");
        assert_eq!(meta.primary_key, ["id", "tenant"]);
        assert!(column(&meta, "id").is_auto_increment);
    })
    .await;
}

/// A type naming its own character set or collation used to get the old COLLATE appended
/// (1253 "COLLATION … is not valid for CHARACTER SET …", 1064 for two COLLATE clauses).
#[tokio::test]
async fn type_change_naming_a_character_set_or_collation() {
    with_scratch_tables(&["c"], |session, prefix| async move {
        let table = TableRef::new(None, &format!("{prefix}_c"));
        session
            .execute(
                &format!(
                    "CREATE TABLE `{}` (id int PRIMARY KEY, name varchar(10) COLLATE utf8mb4_bin)",
                    table.name
                ),
                &[],
            )
            .await
            .expect("create");
        session
            .execute(
                &format!("INSERT INTO `{}` VALUES (1, 'abc')", table.name),
                &[],
            )
            .await
            .expect("insert");
        for (new_type, collation) in [
            ("varchar(20) CHARACTER SET latin1", "latin1_"),
            ("varchar(30) COLLATE latin1_bin", "latin1_bin"),
            ("varchar(40)", "latin1_bin"),
        ] {
            let meta = session.table_meta(&table).await.expect("meta");
            apply_ok(
                &session,
                Some(&meta),
                vec![DdlOp::AlterColumnType {
                    table: table.clone(),
                    name: "name".into(),
                    new_type: new_type.into(),
                    using: None,
                }],
            )
            .await;
            let meta = session.table_meta(&table).await.expect("meta");
            let got = column(&meta, "name").collation.clone().unwrap_or_default();
            assert!(got.starts_with(collation), "{new_type}: {got}");
        }
        assert_eq!(
            scalar(
                &session,
                &format!("SELECT COUNT(*) FROM `{}` WHERE name = 'abc'", table.name)
            )
            .await,
            1
        );
    })
    .await;
}

/// DDL used to wait up to a year (lock_wait_timeout) behind a metadata lock, here the SQL
/// editor's own open transaction, while every other reader of the table queued behind it.
#[tokio::test]
async fn ddl_behind_the_editor_transaction_times_out() {
    with_scratch_tables(&["l"], |session, prefix| async move {
        let table = TableRef::new(None, &format!("{prefix}_l"));
        session
            .execute(&format!("CREATE TABLE `{}` (id int)", table.name), &[])
            .await
            .expect("create");
        let meta = session.table_meta(&table).await.expect("meta");
        let plan = plan_ddl(
            Dialect::Mysql,
            Some(&meta),
            &[DdlOp::AddColumn {
                table: table.clone(),
                column: col("extra", "int", true),
                after: None,
            }],
        )
        .expect("plan");

        editor(&session, "BEGIN").await;
        editor(&session, &format!("SELECT * FROM `{}`", table.name)).await;
        let started = std::time::Instant::now();
        let result = apply_plan(session.as_ref(), &plan).await;
        let waited = started.elapsed();
        assert!(
            waited < std::time::Duration::from_secs(DDL_LOCK_TIMEOUT_SECS + 10),
            "{waited:?}"
        );
        assert_eq!(result.failed_index, Some(0), "{result:?}");
        assert!(result.executed.is_empty());
        let err = result.error.expect("lock wait timeout");
        assert_eq!(err.vendor_code, Some(1205), "{err:?}");
        assert!(
            err.hint.as_deref().unwrap_or("").contains("SQL editor"),
            "{err:?}"
        );
        let count = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            session.query(&format!("SELECT COUNT(*) FROM `{}`", table.name), &[]),
        )
        .await
        .expect("reader not blocked");
        assert!(count.is_ok(), "{count:?}");
        editor(&session, "ROLLBACK").await;

        apply_ok(
            &session,
            Some(&meta),
            vec![DdlOp::AddColumn {
                table: table.clone(),
                column: col("extra", "int", true),
                after: None,
            }],
        )
        .await;
        let meta = session.table_meta(&table).await.expect("meta");
        assert_eq!(names(&meta), ["id", "extra"]);
        // the pooled connection went back with the server's own lock_wait_timeout
        assert!(
            scalar(&session, "SELECT @@SESSION.lock_wait_timeout").await
                > DDL_LOCK_TIMEOUT_SECS as i64
        );
    })
    .await;
}

#[tokio::test]
async fn plans_and_applies_ddl() {
    let Some(session) = open().await else {
        return;
    };
    let prefix = format!(
        "tabler_ddl_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..10]
    );
    let outcome = tokio::spawn(steps(session.clone(), prefix.clone())).await;
    for table in ["items", "goods", "owners"] {
        let _ = session
            .execute(&format!("DROP TABLE IF EXISTS `{prefix}_{table}`"), &[])
            .await;
    }
    session.close().await;
    if let Err(e) = outcome {
        std::panic::resume_unwind(e.into_panic());
    }
}

// --- grouped ALTER TABLE (the ops of one table are one statement) ------------------------------

fn idx(name: &str, columns: &[&str], unique: bool) -> IndexDef {
    IndexDef {
        name: name.into(),
        columns: columns.iter().map(|c| c.to_string()).collect(),
        unique,
        method: None,
        predicate: None,
        comment: None,
        include: vec![],
    }
}

/// What a schema comparison looks at: columns (in order) with their type, nullability, default,
/// extra and comment, then the indexes, checks and foreign keys.
fn shape(meta: &TableMeta) -> String {
    let mut out = String::new();
    for c in &meta.columns {
        out += &format!(
            "col {} {} null={} default={:?} extra={:?} comment={:?} collation={:?}\n",
            c.name, c.data_type, c.nullable, c.default_expr, c.extra, c.comment, c.collation
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

/// A grouped plan is one `ALTER TABLE`: when its last clause is refused the table stays exactly
/// as it was, which is what makes "All changes were reverted" true on MySQL.
#[tokio::test]
async fn a_grouped_plan_whose_last_clause_fails_leaves_the_table_as_it_was() {
    with_prefixed_tables("fx3", &["a"], |session, prefix| async move {
        let table = TableRef::new(None, &format!("{prefix}_a"));
        session
            .execute(
                &format!(
                    "CREATE TABLE `{}` (id int PRIMARY KEY, name varchar(20) NULL, body text NULL)",
                    table.name
                ),
                &[],
            )
            .await
            .expect("create");
        session
            .execute(
                &format!(
                    "INSERT INTO `{}` VALUES (1, 'dup', 'a'), (2, 'dup', 'b')",
                    table.name
                ),
                &[],
            )
            .await
            .expect("insert");
        let before = session.table_meta(&table).await.expect("meta");
        let ops = |unique: bool| {
            vec![
                DdlOp::AddColumn {
                    table: table.clone(),
                    column: col("extra", "int", true),
                    after: None,
                },
                DdlOp::AlterColumnNullable {
                    table: table.clone(),
                    name: "name".into(),
                    nullable: false,
                },
                DdlOp::AddIndex {
                    table: table.clone(),
                    index: idx("fx3_body_idx", &["body(10)"], false),
                },
                // the last clause: refused while two rows share a name (1062)
                DdlOp::AddIndex {
                    table: table.clone(),
                    index: idx("fx3_name_uq", &["name"], unique),
                },
            ]
        };
        let plan = plan_ddl(Dialect::Mysql, Some(&before), &ops(true)).expect("plan");
        assert_eq!(plan.statements.len(), 1, "{:#?}", plan.statements);
        assert!(plan.transactional, "one statement is all or nothing");
        let result = apply_plan(session.as_ref(), &plan).await;
        assert_eq!(result.failed_index, Some(0), "{result:?}");
        // nothing ran, nothing to undo: the apply reports that everything was reverted
        assert!(result.executed.is_empty(), "{result:?}");
        let err = result.error.expect("error payload");
        assert_eq!(err.vendor_code, Some(1062), "{err:?}");
        assert_eq!(err.statement_index, Some(0));
        let after = session.table_meta(&table).await.expect("meta");
        assert_eq!(shape(&after), shape(&before));
        assert_eq!(names(&after), ["id", "name", "body"]);
        assert!(column(&after, "name").nullable);

        // without the clause the server refuses, the plan applies as one statement
        let result = apply_ok(&session, Some(&before), ops(false)).await;
        assert_eq!(result.executed.len(), 1);
        let after = session.table_meta(&table).await.expect("meta");
        assert_eq!(names(&after), ["id", "name", "body", "extra"]);
        assert!(!column(&after, "name").nullable);
        let body_idx = after.indexes.iter().find(|i| i.name == "fx3_body_idx");
        assert_eq!(
            body_idx.map(|i| i.columns.clone()),
            Some(vec!["body(10)".to_string()])
        );
    })
    .await;
}

/// MySQL refuses a TEXT column in a plain index without a prefix length (1170); the planner says so
/// before anything is sent, and the prefixed entry is what the server takes.
#[tokio::test]
async fn a_text_column_needs_a_prefix_length_in_an_index() {
    with_prefixed_tables("fx3", &["t"], |session, prefix| async move {
        let table = TableRef::new(None, &format!("{prefix}_t"));
        session
            .execute(
                &format!(
                    "CREATE TABLE `{}` (id int PRIMARY KEY, body text NULL, name varchar(500) NULL)",
                    table.name
                ),
                &[],
            )
            .await
            .expect("create");
        let meta = session.table_meta(&table).await.expect("meta");
        let add = |columns: &[&str]| DdlOp::AddIndex {
            table: table.clone(),
            index: idx("fx3_ix", columns, false),
        };
        // what the server itself says without the length
        let err = session
            .execute(
                &format!("ALTER TABLE `{}` ADD INDEX fx3_raw (body)", table.name),
                &[],
            )
            .await
            .expect_err("1170");
        assert!(err.to_string().contains("key length"), "{err}");
        // the planner refuses it first, naming the column
        let refused = plan_ddl(Dialect::Mysql, Some(&meta), &[add(&["name", "body"])]);
        let msg = refused.expect_err("refused").to_string();
        assert!(msg.contains("body") && msg.contains("prefix length"), "{msg}");
        // a varchar needs none; with a length the TEXT column is indexed
        apply_ok(&session, Some(&meta), vec![add(&["name", "body(20)"])]).await;
        let meta = session.table_meta(&table).await.expect("meta");
        let ix = meta.indexes.iter().find(|i| i.name == "fx3_ix").expect("ix");
        assert_eq!(ix.columns, ["name", "body(20)"]);
    })
    .await;
}

/// Two tables are two statements: a failure in the second leaves the first applied (the plan is
/// not transactional, the partial-apply banner stays for it).
#[tokio::test]
async fn a_plan_over_two_tables_is_not_atomic() {
    with_prefixed_tables("fx3", &["p", "q"], |session, prefix| async move {
        let p = TableRef::new(None, &format!("{prefix}_p"));
        let q = TableRef::new(None, &format!("{prefix}_q"));
        for t in [&p, &q] {
            session
                .execute(
                    &format!("CREATE TABLE `{}` (id int PRIMARY KEY)", t.name),
                    &[],
                )
                .await
                .expect("create");
        }
        let meta = session.table_meta(&p).await.expect("meta");
        let ops = [
            DdlOp::AddColumn {
                table: p.clone(),
                column: col("x", "int", true),
                after: None,
            },
            DdlOp::AddColumn {
                table: q.clone(),
                column: col("id", "int", true), // exists: refused by the server
                after: None,
            },
        ];
        let plan = plan_ddl(Dialect::Mysql, Some(&meta), &ops).expect("plan");
        assert_eq!(plan.statements.len(), 2);
        assert!(!plan.transactional);
        let result = apply_plan(session.as_ref(), &plan).await;
        assert_eq!(result.failed_index, Some(1), "{result:?}");
        assert_eq!(result.executed, plan.statements[..1]);
        let meta = session.table_meta(&p).await.expect("meta");
        assert_eq!(names(&meta), ["id", "x"]);
    })
    .await;
}

/// The grouped plan of a mixed edit leaves the same schema as the old one-statement-per-op plan.
#[tokio::test]
async fn a_grouped_plan_gives_the_schema_of_the_per_op_plan() {
    with_prefixed_tables("fx3", &["g", "o", "ref"], |session, prefix| async move {
        let create = |name: &str| {
            format!(
                "CREATE TABLE `{prefix}_{name}` (\
                   id int NOT NULL AUTO_INCREMENT PRIMARY KEY, \
                   name varchar(30) NULL, \
                   qty int NOT NULL DEFAULT 0, \
                   note text NULL, \
                   legacy int NULL, \
                   ref_id int NULL, \
                   KEY fx3_qty (qty))"
            )
        };
        session
            .execute(
                &format!("CREATE TABLE `{prefix}_ref` (id int PRIMARY KEY)"),
                &[],
            )
            .await
            .expect("ref");
        for name in ["g", "o"] {
            session.execute(&create(name), &[]).await.expect("create");
        }
        let ops = |t: &TableRef| {
            let mut stamp = col("stamp", "datetime(3)", true);
            stamp.default_expr = Some("CURRENT_TIMESTAMP".into());
            stamp.on_update = Some("CURRENT_TIMESTAMP".into());
            vec![
                DdlOp::DropColumn {
                    table: t.clone(),
                    name: "legacy".into(),
                },
                DdlOp::AddColumn {
                    table: t.clone(),
                    column: col("code", "varchar(12)", true),
                    after: Some("name".into()),
                },
                DdlOp::AddColumn {
                    table: t.clone(),
                    column: stamp,
                    after: None,
                },
                DdlOp::AlterColumnType {
                    table: t.clone(),
                    name: "name".into(),
                    new_type: "varchar(60)".into(),
                    using: None,
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
                DdlOp::AlterColumnDefault {
                    table: t.clone(),
                    name: "qty".into(),
                    default_expr: Some("7".into()),
                },
                DdlOp::AddIndex {
                    table: t.clone(),
                    index: idx("fx3_code", &["code(6)", "qty"], true),
                },
                DdlOp::DropIndex {
                    table: t.clone(),
                    name: "fx3_qty".into(),
                },
                DdlOp::AddIndex {
                    table: t.clone(),
                    index: idx("fx3_note", &["note(20)"], false),
                },
                DdlOp::AddCheck {
                    table: t.clone(),
                    check: tabler_lib::types::ddl::CheckDef {
                        name: Some(format!("{}_qty_chk", t.name)),
                        expression: "qty >= 0".into(),
                    },
                },
                DdlOp::AddForeignKey {
                    table: t.clone(),
                    fk: FkDef {
                        name: Some(format!("{}_ref_fk", t.name)),
                        columns: vec!["ref_id".into()],
                        ref_table: TableRef::new(None, &format!("{prefix}_ref")),
                        ref_columns: vec!["id".into()],
                        on_update: None,
                        on_delete: Some("set null".into()),
                    },
                },
                // (the key goes after AUTO_INCREMENT does: a drop on its own is refused, 1075)
                DdlOp::AlterColumnAutoIncrement {
                    table: t.clone(),
                    name: "id".into(),
                    auto_increment: false,
                },
                DdlOp::DropPrimaryKey { table: t.clone() },
                DdlOp::AddPrimaryKey {
                    table: t.clone(),
                    columns: vec!["id".into(), "qty".into()],
                },
            ]
        };
        let grouped = TableRef::new(None, &format!("{prefix}_g"));
        let per_op = TableRef::new(None, &format!("{prefix}_o"));

        // grouped: the planner's own statements (one per table)
        let meta = session.table_meta(&grouped).await.expect("meta");
        let result = apply_ok(&session, Some(&meta), ops(&grouped)).await;
        // every op is a clause of the table's one ALTER TABLE
        assert_eq!(result.executed.len(), 1, "{:#?}", result.executed);

        // one statement per op, each planned against the table as the previous one left it
        for op in ops(&per_op) {
            let meta = session.table_meta(&per_op).await.expect("meta");
            let plan = plan_ddl(Dialect::Mysql, Some(&meta), &[op]).expect("plan");
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
        let normal = |m: &TableMeta, t: &TableRef| shape(m).replace(&t.name, "T");
        assert_eq!(normal(&a, &grouped), normal(&b, &per_op));
        // and the edits are there
        assert_eq!(
            names(&a),
            ["id", "name", "code", "qty", "note", "ref_id", "stamp"]
        );
        assert_eq!(column(&a, "name").default_expr.as_deref(), Some("anon"));
        assert_eq!(a.primary_key, ["id", "qty"]);
    })
    .await;
}

/// `ON UPDATE CURRENT_TIMESTAMP` and `DEFAULT CURRENT_TIMESTAMP` on a `datetime(3)` / `timestamp(6)`
/// column are refused by MySQL (1067 / 1294) unless they carry the column's precision.
#[tokio::test]
async fn current_timestamp_takes_the_precision_of_the_column() {
    with_prefixed_tables("fx3", &["ts"], |session, prefix| async move {
        let table = TableRef::new(None, &format!("{prefix}_ts"));
        session
            .execute(
                &format!(
                    "CREATE TABLE `{}` (id int PRIMARY KEY, d datetime(3) NULL, t timestamp(6) NULL, plain datetime NULL)",
                    table.name
                ),
                &[],
            )
            .await
            .expect("create");
        let meta = session.table_meta(&table).await.expect("meta");
        // what the structure editor stages: the bare function name
        let on_update = |name: &str| DdlOp::AlterColumnOnUpdate {
            table: table.clone(),
            name: name.into(),
            on_update: Some("CURRENT_TIMESTAMP".into()),
        };
        let default = |name: &str| DdlOp::AlterColumnDefault {
            table: table.clone(),
            name: name.into(),
            default_expr: Some("CURRENT_TIMESTAMP".into()),
        };
        let result = apply_ok(
            &session,
            Some(&meta),
            vec![
                on_update("d"),
                on_update("t"),
                on_update("plain"),
                default("d"),
            ],
        )
        .await;
        assert!(
            result.executed[0].contains("ON UPDATE CURRENT_TIMESTAMP(3)"),
            "{:?}",
            result.executed
        );
        let meta = session.table_meta(&table).await.expect("meta");
        let extra = |c: &str| {
            column(&meta, c)
                .extra
                .clone()
                .unwrap_or_default()
                .to_ascii_lowercase()
        };
        assert!(extra("d").contains("on update current_timestamp(3)"), "{}", extra("d"));
        assert!(extra("t").contains("on update current_timestamp(6)"), "{}", extra("t"));
        assert!(extra("plain").contains("on update current_timestamp"), "{}", extra("plain"));
        assert!(!extra("plain").contains("current_timestamp("), "{}", extra("plain"));
        assert_eq!(
            column(&meta, "d").default_expr.as_deref().map(str::to_ascii_lowercase),
            Some("current_timestamp(3)".to_string())
        );

        // changing the precision of the type re-emits the matching default and ON UPDATE
        for new_type in ["datetime(6)", "datetime", "datetime(2)"] {
            let meta = session.table_meta(&table).await.expect("meta");
            apply_ok(
                &session,
                Some(&meta),
                vec![DdlOp::AlterColumnType {
                    table: table.clone(),
                    name: "d".into(),
                    new_type: new_type.into(),
                    using: None,
                }],
            )
            .await;
        }
        let meta = session.table_meta(&table).await.expect("meta");
        assert!(
            column(&meta, "d")
                .extra
                .clone()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .contains("on update current_timestamp(2)"),
            "{:?}",
            column(&meta, "d").extra
        );

        // a new column with both
        let mut seen = col("seen", "timestamp(4)", true);
        seen.default_expr = Some("CURRENT_TIMESTAMP".into());
        seen.on_update = Some("CURRENT_TIMESTAMP".into());
        apply_ok(
            &session,
            Some(&meta),
            vec![DdlOp::AddColumn {
                table: table.clone(),
                column: seen,
                after: None,
            }],
        )
        .await;
    })
    .await;
}

/// What MySQL refuses inside one `ALTER TABLE` but takes as separate statements (1089 / 1171 /
/// 1283 for a column re-added under the name of a dropped one that a key covers, 1826 for a
/// foreign key dropped and added under one name). The grouped plan must apply and leave the
/// schema the old one-statement-per-op plan leaves, for every sequence below.
#[tokio::test]
async fn grouped_plans_match_the_per_op_plans_on_sequences_the_server_is_picky_about() {
    with_prefixed_tables("fx3", &["g", "o", "ref"], |session, prefix| async move {
        type Ops = Box<dyn Fn(&TableRef, &str) -> Vec<DdlOp> + Send + Sync>;
        fn boxed(f: impl Fn(&TableRef, &str) -> Vec<DdlOp> + Send + Sync + 'static) -> Ops {
            Box::new(f)
        }
        let base = "id int NOT NULL AUTO_INCREMENT, a int NULL, b varchar(30) NULL, c int NULL, d text NULL, PRIMARY KEY (id)";
        let swap_column: fn(&str) -> Ops = |column| {
            let column = column.to_string();
            boxed(move |t, _| {
                vec![
                    DdlOp::DropColumn {
                        table: t.clone(),
                        name: column.clone(),
                    },
                    DdlOp::AddColumn {
                        table: t.clone(),
                        column: col(&column, "bigint", true),
                        after: None,
                    },
                ]
            })
        };
        let fk_create = format!(
            "{base}, CONSTRAINT {{T}}_fk FOREIGN KEY (a) REFERENCES {{R}} (id)"
        );
        let scenarios: Vec<(&str, String, Ops, usize)> = vec![
            // (label, CREATE TABLE body, ops, statements the grouped plan has)
            ("re-add a column of a plain index", format!("{base}, KEY ix (a)"), swap_column("a"), 2),
            ("re-add a column of a composite index", format!("{base}, KEY ix (b, a)"), swap_column("a"), 2),
            ("re-add a column of a unique index", format!("{base}, UNIQUE KEY ux (a)"), swap_column("a"), 2),
            ("re-add a column of a fulltext index", format!("{base}, FULLTEXT KEY fx (b)"), swap_column("b"), 2),
            (
                "re-add a column of the primary key",
                "id int NOT NULL, a int NOT NULL, b varchar(30) NULL, PRIMARY KEY (id, a)".to_string(),
                swap_column("a"),
                2,
            ),
            ("re-add a column of no key", base.to_string(), swap_column("c"), 1),
            (
                "re-add a column of a foreign key (its backing index stays)",
                fk_create.clone(),
                boxed(move |t, p| {
                    vec![
                        DdlOp::DropForeignKey {
                            table: t.clone(),
                            name: format!("{}_fk", t.name),
                        },
                        DdlOp::DropColumn {
                            table: t.clone(),
                            name: "a".into(),
                        },
                        DdlOp::AddColumn {
                            table: t.clone(),
                            column: col("a", "int", true),
                            after: None,
                        },
                        DdlOp::AddForeignKey {
                            table: t.clone(),
                            fk: FkDef {
                                name: Some(format!("{}_fk2", t.name)),
                                columns: vec!["a".into()],
                                ref_table: TableRef::new(None, &format!("{p}_ref")),
                                ref_columns: vec!["id".into()],
                                on_update: None,
                                on_delete: None,
                            },
                        },
                    ]
                }),
                2,
            ),
            (
                "drop the index, then re-add its column",
                format!("{base}, KEY ix (a)"),
                boxed(move |t, _| {
                    vec![
                        DdlOp::DropIndex {
                            table: t.clone(),
                            name: "ix".into(),
                        },
                        DdlOp::DropColumn {
                            table: t.clone(),
                            name: "a".into(),
                        },
                        DdlOp::AddColumn {
                            table: t.clone(),
                            column: col("a", "int", true),
                            after: None,
                        },
                    ]
                }),
                1,
            ),
            (
                "drop the index, then its column",
                format!("{base}, KEY ix (a)"),
                boxed(move |t, _| {
                    vec![
                        DdlOp::DropIndex {
                            table: t.clone(),
                            name: "ix".into(),
                        },
                        DdlOp::DropColumn {
                            table: t.clone(),
                            name: "a".into(),
                        },
                    ]
                }),
                1,
            ),
            (
                "edit an index under its name",
                format!("{base}, KEY ix (a)"),
                boxed(move |t, _| {
                    vec![
                        DdlOp::DropIndex {
                            table: t.clone(),
                            name: "ix".into(),
                        },
                        DdlOp::AddIndex {
                            table: t.clone(),
                            index: idx("ix", &["a", "c"], false),
                        },
                    ]
                }),
                1,
            ),
            (
                "edit a CHECK under its name",
                format!("{base}, CONSTRAINT {{T}}_ck CHECK (c > 0)"),
                boxed(move |t, _| {
                    vec![
                        DdlOp::DropCheck {
                            table: t.clone(),
                            name: format!("{}_ck", t.name),
                        },
                        DdlOp::AddCheck {
                            table: t.clone(),
                            check: tabler_lib::types::ddl::CheckDef {
                                name: Some(format!("{}_ck", t.name)),
                                expression: "c > 1".into(),
                            },
                        },
                    ]
                }),
                1,
            ),
            (
                "edit a foreign key under its name",
                fk_create.clone(),
                boxed(move |t, p| {
                    vec![
                        DdlOp::DropForeignKey {
                            table: t.clone(),
                            name: format!("{}_fk", t.name),
                        },
                        DdlOp::AddForeignKey {
                            table: t.clone(),
                            fk: FkDef {
                                name: Some(format!("{}_fk", t.name)),
                                columns: vec!["a".into()],
                                ref_table: TableRef::new(None, &format!("{p}_ref")),
                                ref_columns: vec!["id".into()],
                                on_update: None,
                                on_delete: Some("cascade".into()),
                            },
                        },
                    ]
                }),
                2,
            ),
            (
                "edit a foreign key without naming it (what the structure editor stages)",
                fk_create.clone(),
                boxed(move |t, p| {
                    vec![
                        DdlOp::DropForeignKey {
                            table: t.clone(),
                            name: format!("{}_fk", t.name),
                        },
                        DdlOp::AddForeignKey {
                            table: t.clone(),
                            fk: FkDef {
                                name: None,
                                columns: vec!["a".into()],
                                ref_table: TableRef::new(None, &format!("{p}_ref")),
                                ref_columns: vec!["id".into()],
                                on_update: None,
                                on_delete: Some("set null".into()),
                            },
                        },
                    ]
                }),
                1,
            ),
            (
                "drop a foreign key and its column",
                fk_create.clone(),
                boxed(move |t, _| {
                    vec![
                        DdlOp::DropForeignKey {
                            table: t.clone(),
                            name: format!("{}_fk", t.name),
                        },
                        DdlOp::DropColumn {
                            table: t.clone(),
                            name: "a".into(),
                        },
                    ]
                }),
                1,
            ),
            (
                "retype, index, default, not null and a new indexed column",
                format!("{base}, KEY ix (a)"),
                boxed(move |t, p| {
                    vec![
                        DdlOp::AlterColumnType {
                            table: t.clone(),
                            name: "a".into(),
                            new_type: "bigint".into(),
                            using: None,
                        },
                        DdlOp::AddIndex {
                            table: t.clone(),
                            index: idx("ix2", &["a"], false),
                        },
                        DdlOp::AlterColumnDefault {
                            table: t.clone(),
                            name: "c".into(),
                            default_expr: Some("5".into()),
                        },
                        DdlOp::AlterColumnNullable {
                            table: t.clone(),
                            name: "b".into(),
                            nullable: false,
                        },
                        DdlOp::AddColumn {
                            table: t.clone(),
                            column: col("e", "int", true),
                            after: Some("a".into()),
                        },
                        DdlOp::AddIndex {
                            table: t.clone(),
                            index: idx("ie", &["e"], false),
                        },
                        DdlOp::AddForeignKey {
                            table: t.clone(),
                            fk: FkDef {
                                name: Some(format!("{}_efk", t.name)),
                                columns: vec!["e".into()],
                                ref_table: TableRef::new(None, &format!("{p}_ref")),
                                ref_columns: vec!["id".into()],
                                on_update: None,
                                on_delete: None,
                            },
                        },
                        DdlOp::DropColumn {
                            table: t.clone(),
                            name: "c".into(),
                        },
                    ]
                }),
                // (the last DROP COLUMN follows a change of the same column: a statement of its own)
                2,
            ),
            (
                "text column: longer type and a prefix index",
                base.to_string(),
                boxed(move |t, _| {
                    vec![
                        DdlOp::AlterColumnType {
                            table: t.clone(),
                            name: "d".into(),
                            new_type: "longtext".into(),
                            using: None,
                        },
                        DdlOp::AddIndex {
                            table: t.clone(),
                            index: idx("id_d", &["d(20)"], false),
                        },
                    ]
                }),
                1,
            ),
            (
                "comment, type and nullability of one column, a new column after another",
                base.to_string(),
                boxed(move |t, _| {
                    vec![
                        DdlOp::SetColumnComment {
                            table: t.clone(),
                            name: "a".into(),
                            comment: Some("the a".into()),
                        },
                        DdlOp::AlterColumnType {
                            table: t.clone(),
                            name: "a".into(),
                            new_type: "bigint".into(),
                            using: None,
                        },
                        DdlOp::AlterColumnNullable {
                            table: t.clone(),
                            name: "a".into(),
                            nullable: false,
                        },
                        DdlOp::AddColumn {
                            table: t.clone(),
                            column: col("z", "int", true),
                            after: Some("b".into()),
                        },
                    ]
                }),
                1,
            ),
            (
                "drop a column and add one after its neighbour",
                base.to_string(),
                boxed(move |t, _| {
                    vec![
                        DdlOp::DropColumn {
                            table: t.clone(),
                            name: "b".into(),
                        },
                        DdlOp::AddColumn {
                            table: t.clone(),
                            column: col("z", "int", true),
                            after: Some("a".into()),
                        },
                    ]
                }),
                1,
            ),
            (
                "re-add a column and its index under the index's old name",
                format!("{base}, KEY ixc (c)"),
                boxed(move |t, _| {
                    vec![
                        DdlOp::DropColumn {
                            table: t.clone(),
                            name: "c".into(),
                        },
                        DdlOp::AddColumn {
                            table: t.clone(),
                            column: col("c", "bigint", true),
                            after: None,
                        },
                        DdlOp::AddIndex {
                            table: t.clone(),
                            index: idx("ixc", &["c"], false),
                        },
                    ]
                }),
                2,
            ),
        ];
        session
            .execute(
                &format!("CREATE TABLE `{prefix}_ref` (id int PRIMARY KEY)"),
                &[],
            )
            .await
            .expect("ref");
        let grouped = TableRef::new(None, &format!("{prefix}_g"));
        let per_op = TableRef::new(None, &format!("{prefix}_o"));
        let mut failures = Vec::new();
        for (label, body, ops, statements) in &scenarios {
            for t in [&grouped, &per_op] {
                let _ = session
                    .execute(&format!("DROP TABLE IF EXISTS `{}`", t.name), &[])
                    .await;
                let sql = body
                    .replace("{T}", &t.name)
                    .replace("{R}", &format!("`{prefix}_ref`"));
                session
                    .execute(&format!("CREATE TABLE `{}` ({sql})", t.name), &[])
                    .await
                    .unwrap_or_else(|e| panic!("{label}: create: {e}"));
            }
            // the old plan: one statement per op, each planned against the table as it is
            let mut old_error = None;
            for op in ops(&per_op, &prefix) {
                let meta = session.table_meta(&per_op).await.expect("meta");
                let plan = match plan_ddl(Dialect::Mysql, Some(&meta), &[op]) {
                    Ok(plan) => plan,
                    Err(e) => {
                        old_error = Some(format!("plan: {e}"));
                        break;
                    }
                };
                let result = apply_plan(session.as_ref(), &plan).await;
                if let Some(e) = result.error {
                    old_error = Some(format!("{e:?}: {:#?}", plan.statements));
                    break;
                }
            }
            if let Some(e) = old_error {
                failures.push(format!("{label}: the per-op plan itself fails ({e})"));
                continue;
            }
            // the grouped plan
            let meta = session.table_meta(&grouped).await.expect("meta");
            let plan = match plan_ddl(Dialect::Mysql, Some(&meta), &ops(&grouped, &prefix)) {
                Ok(plan) => plan,
                Err(e) => {
                    failures.push(format!("{label}: plan {e}"));
                    continue;
                }
            };
            if plan.statements.len() != *statements {
                failures.push(format!(
                    "{label}: {} statements, expected {statements}: {:#?}",
                    plan.statements.len(),
                    plan.statements
                ));
            }
            assert_eq!(plan.transactional, plan.statements.len() == 1, "{label}");
            let result = apply_plan(session.as_ref(), &plan).await;
            if let Some(e) = result.error {
                failures.push(format!("{label}: the grouped plan fails: {e:?}\n{:#?}", plan.statements));
                continue;
            }
            let a = session.table_meta(&grouped).await.expect("meta");
            let b = session.table_meta(&per_op).await.expect("meta");
            let normal = |m: &TableMeta, t: &TableRef| shape(m).replace(&t.name, "T");
            if normal(&a, &grouped) != normal(&b, &per_op) {
                failures.push(format!(
                    "{label}: schemas differ\ngrouped:\n{}per-op:\n{}",
                    normal(&a, &grouped),
                    normal(&b, &per_op)
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    })
    .await;
}

/// `ADD CONSTRAINT ck CHECK (b > 0), DROP COLUMN b` in one `ALTER TABLE` is refused by MySQL
/// (1054: the CHECK is resolved against the table without `b`); the planner keeps them in two
/// statements, in order.
#[tokio::test]
async fn a_check_added_before_the_drop_of_its_column_is_two_statements() {
    with_prefixed_tables("fx3", &["t"], |session, prefix| async move {
        let name = format!("{prefix}_t");
        session
            .execute(
                &format!("CREATE TABLE `{name}` (id int PRIMARY KEY, a int, b int)"),
                &[],
            )
            .await
            .expect("create");
        let table = TableRef::new(None, &name);
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
        let plan = plan_ddl(Dialect::Mysql, Some(&meta), &ops).expect("plan");
        assert_eq!(plan.statements.len(), 2, "{:#?}", plan.statements);
        let result = apply_plan(session.as_ref(), &plan).await;
        assert!(
            result.error.is_none(),
            "{:?}\n{:#?}",
            result.error,
            plan.statements
        );
        let after = session.table_meta(&table).await.expect("meta");
        assert!(after.columns.iter().all(|c| c.name != "b"));
    })
    .await;
}
