//! DDL integration test for MySQL / MariaDB: plans with `plan_ddl`, runs with `apply_plan`
//! (statement by statement: MySQL DDL auto-commits), re-introspects after every step. Runs only
//! when `TABLER_TEST_MYSQL_URL` is set; scratch tables are `tabler_ddl_<random>_*`.

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
    assert!(!plan.transactional, "MySQL DDL auto-commits");
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
    let Some(session) = open().await else {
        return;
    };
    let prefix = format!(
        "tabler_ddl_{}",
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
