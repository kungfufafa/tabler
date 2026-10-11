//! DDL integration test for SQLite: plans with `plan_ddl`, runs with `apply_plan` (including the
//! 12-step table rebuild), re-introspects after every step. Runs only when
//! `TABLER_TEST_SQLITE_PATH` is set; works on a temporary copy of that database.

use std::path::PathBuf;
use std::sync::Arc;

use tabler_lib::commands::ddl::apply_plan;
use tabler_lib::drivers::{ConnectTarget, DbSession, TxStatement, connect};
use tabler_lib::sqlgen::Dialect;
use tabler_lib::sqlgen::ddl::{is_fk_check, plan_ddl};
use tabler_lib::types::ddl::{ColumnDef, DdlOp, DdlResult, FkDef, IndexDef};
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
use tabler_lib::types::schema::{ColumnMeta, TableMeta, TableRef};
use tabler_lib::types::value::Value;

const ENV: &str = "TABLER_TEST_SQLITE_PATH";

/// Opens a temporary copy of the fixture database (the original is never modified).
async fn open() -> Option<(Arc<dyn DbSession>, tempfile::TempDir)> {
    let Ok(path) = std::env::var(ENV) else {
        println!("skipped: {ENV} unset");
        return None;
    };
    let mut source = PathBuf::from(&path);
    if source.is_relative() {
        // relative to the repository root (the crate lives in `src-tauri/`)
        source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(source);
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let copy = dir.path().join("tabler_ddl.sqlite");
    std::fs::copy(&source, &copy).unwrap_or_else(|e| panic!("copy {}: {e}", source.display()));
    let profile = ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "sqlite-ddl-test".into(),
        color: None,
        group: None,
        driver: Driver::Sqlite,
        host: None,
        port: None,
        user: None,
        database: None,
        ssl: SslMode::Disable,
        ssl_ca_path: None,
        sqlite_path: Some(copy.to_string_lossy().into_owned()),
        ssh: None,
        options: ProfileOptions {
            sqlite_foreign_keys: true,
            ..ProfileOptions::default()
        },
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    };
    let session = connect(ConnectTarget {
        profile: &profile,
        password: None,
        host_override: None,
    })
    .await
    .expect("open sqlite copy");
    Some((session, dir))
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
) -> (DdlResult, bool) {
    let plan = plan_ddl(Dialect::Sqlite, meta, &ops).expect("plan");
    let result = apply_plan(session.as_ref(), &plan).await;
    assert!(
        result.error.is_none(),
        "apply failed: {:?}\nplan: {:#?}",
        result.error,
        plan.statements
    );
    assert_eq!(result.executed, plan.statements);
    (result, plan.transactional)
}

async fn scalar(session: &Arc<dyn DbSession>, sql: &str) -> i64 {
    let rs = session.query(sql, &[]).await.expect(sql);
    match rs.rows.first().and_then(|r| r.first()) {
        Some(Value::Int(i)) => *i,
        Some(Value::BigInt(s) | Value::Decimal(s) | Value::Text(s)) => s.parse().expect(sql),
        other => panic!("{sql}: unexpected {other:?}"),
    }
}

#[tokio::test]
async fn plans_and_applies_ddl_with_rebuilds() {
    let Some((session, _dir)) = open().await else {
        return;
    };
    let prefix = format!(
        "tabler_ddl_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..10]
    );
    let owners = TableRef::new(None, &format!("{prefix}_owners"));
    let items = TableRef::new(None, &format!("{prefix}_items"));
    let q_items = format!("\"{prefix}_items\"");
    let index_name = format!("{prefix}_name_idx");
    let trigger_name = format!("{prefix}_touch");

    // 1. create both tables
    let mut owner_id = col("id", "INTEGER", false);
    owner_id.auto_increment = true;
    let mut id = col("id", "INTEGER", false);
    id.auto_increment = true;
    let mut qty = col("qty", "INT", true);
    qty.default_expr = Some("0".into());
    let (_, transactional) = apply_ok(
        &session,
        None,
        vec![
            DdlOp::CreateTable {
                table: owners.clone(),
                columns: vec![owner_id, col("label", "TEXT", true)],
                primary_key: vec!["id".into()],
                if_not_exists: false,
            },
            DdlOp::CreateTable {
                table: items.clone(),
                columns: vec![
                    id,
                    col("name", "TEXT", false),
                    qty,
                    col("code", "TEXT", true),
                ],
                primary_key: vec!["id".into()],
                if_not_exists: false,
            },
        ],
    )
    .await;
    assert!(transactional);
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
            &format!("INSERT INTO \"{prefix}_owners\" (label) VALUES ('o1')"),
            &[],
        )
        .await
        .expect("insert owner");
    let meta = session.table_meta(&items).await.expect("meta");
    assert_eq!(names(&meta), ["id", "name", "qty", "code"]);
    assert_eq!(column(&meta, "id").extra.as_deref(), Some("AUTOINCREMENT"));

    // 2. add columns in place (constant default)
    let mut note = col("note", "TEXT", false);
    note.default_expr = Some("'n/a'".into());
    let (_, transactional) = apply_ok(
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
                column: col("owner_id", "INTEGER", true),
                after: None,
            },
        ],
    )
    .await;
    assert!(
        transactional,
        "ADD COLUMN with a constant default runs in place"
    );
    let meta = session.table_meta(&items).await.expect("meta");
    assert!(!column(&meta, "note").nullable);
    assert_eq!(column(&meta, "note").default_expr.as_deref(), Some("'n/a'"));

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

    // 4. unique partial index, plus a trigger (raw SQL) the rebuild has to replay
    apply_ok(
        &session,
        Some(&meta),
        vec![
            DdlOp::AddIndex {
                table: items.clone(),
                index: IndexDef {
                    name: index_name.clone(),
                    columns: vec!["name".into()],
                    unique: true,
                    method: None,
                    predicate: Some("qty >= 0".into()),
                },
            },
            DdlOp::RawSql {
                sql: format!(
                    "CREATE TRIGGER \"{trigger_name}\" AFTER UPDATE OF qty ON {q_items} \
                     BEGIN UPDATE {q_items} SET remark = 'touched' WHERE id = NEW.id; END"
                ),
            },
        ],
    )
    .await;
    let meta = session.table_meta(&items).await.expect("meta");
    let idx = meta
        .indexes
        .iter()
        .find(|i| i.name == index_name)
        .expect("index");
    assert!(idx.unique);
    assert_eq!(meta.triggers.len(), 1);

    // 5. type change → 12-step rebuild; data, order, index, trigger and AUTOINCREMENT survive
    let (result, transactional) = apply_ok(
        &session,
        Some(&meta),
        vec![
            DdlOp::AlterColumnType {
                table: items.clone(),
                name: "qty".into(),
                new_type: "BIGINT".into(),
                using: None,
            },
            DdlOp::AlterColumnDefault {
                table: items.clone(),
                name: "code".into(),
                default_expr: Some("'none'".into()),
            },
        ],
    )
    .await;
    assert!(!transactional);
    assert_eq!(
        result.executed.first().map(String::as_str),
        Some("PRAGMA foreign_keys = OFF")
    );
    let meta = session.table_meta(&items).await.expect("meta");
    assert_eq!(
        names(&meta),
        ["id", "name", "qty", "code", "remark", "owner_id"]
    );
    assert_eq!(column(&meta, "qty").data_type, "BIGINT");
    assert_eq!(
        column(&meta, "code").default_expr.as_deref(),
        Some("'none'")
    );
    assert_eq!(column(&meta, "id").extra.as_deref(), Some("AUTOINCREMENT"));
    assert!(
        meta.indexes
            .iter()
            .any(|i| i.name == index_name && i.unique)
    );
    assert_eq!(
        meta.triggers
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        [trigger_name.as_str()]
    );
    assert_eq!(
        scalar(&session, &format!("SELECT COUNT(*) FROM {q_items}")).await,
        2
    );
    assert_eq!(
        scalar(&session, &format!("SELECT SUM(qty) FROM {q_items}")).await,
        3
    );
    let leftovers = scalar(
        &session,
        &format!("SELECT COUNT(*) FROM sqlite_master WHERE name LIKE '%{prefix}%__tabler_new%'"),
    )
    .await;
    assert_eq!(leftovers, 0);
    // The replayed trigger still fires.
    session
        .execute(
            &format!("UPDATE {q_items} SET qty = 5 WHERE name = 'a'"),
            &[],
        )
        .await
        .expect("update");
    assert_eq!(
        scalar(
            &session,
            &format!("SELECT COUNT(*) FROM {q_items} WHERE remark = 'touched'")
        )
        .await,
        1
    );

    // 6. adding a foreign key that existing rows violate fails and rolls back …
    session
        .execute(
            &format!("UPDATE {q_items} SET owner_id = 999 WHERE name = 'a'"),
            &[],
        )
        .await
        .expect("dangling owner");
    let fk_op = DdlOp::AddForeignKey {
        table: items.clone(),
        fk: FkDef {
            name: Some(format!("{prefix}_owner_fk")),
            columns: vec!["owner_id".into()],
            ref_table: owners.clone(),
            ref_columns: vec!["id".into()],
            on_update: None,
            on_delete: Some("CASCADE".into()),
        },
    };
    let plan = plan_ddl(Dialect::Sqlite, Some(&meta), std::slice::from_ref(&fk_op)).expect("plan");
    let result = apply_plan(session.as_ref(), &plan).await;
    let failed = result.failed_index.expect("violation detected");
    assert!(
        plan.statements[failed].contains("foreign_key_check"),
        "{result:?}"
    );
    assert_eq!(
        result.executed,
        [
            "PRAGMA foreign_keys = OFF",
            "PRAGMA legacy_alter_table = OFF",
            "PRAGMA foreign_keys = ON"
        ],
        "{result:?}"
    );
    let meta = session.table_meta(&items).await.expect("meta");
    assert!(meta.foreign_keys.is_empty());
    assert_eq!(
        names(&meta),
        ["id", "name", "qty", "code", "remark", "owner_id"]
    );

    // … and succeeds once the data is fixed
    session
        .execute(
            &format!("UPDATE {q_items} SET owner_id = 1 WHERE name = 'a'"),
            &[],
        )
        .await
        .expect("fix owner");
    apply_ok(&session, Some(&meta), vec![fk_op]).await;
    let meta = session.table_meta(&items).await.expect("meta");
    assert_eq!(meta.foreign_keys.len(), 1);
    let fk = &meta.foreign_keys[0];
    assert_eq!(fk.ref_table.name, owners.name);
    assert_eq!(fk.columns, ["owner_id"]);
    assert_eq!(fk.on_delete, "CASCADE");
    // enforcement is back on after the rebuild
    let err = session
        .execute(&format!("UPDATE {q_items} SET owner_id = 42"), &[])
        .await
        .expect_err("foreign keys enforced");
    assert!(
        err.to_string().to_lowercase().contains("foreign key"),
        "{err}"
    );

    // 7. drop columns: in place, then an indexed one (rebuild drops its index too)
    let (_, transactional) = apply_ok(
        &session,
        Some(&meta),
        vec![DdlOp::DropColumn {
            table: items.clone(),
            name: "code".into(),
        }],
    )
    .await;
    assert!(transactional);
    let meta = session.table_meta(&items).await.expect("meta");
    assert_eq!(names(&meta), ["id", "name", "qty", "remark", "owner_id"]);
    let plan = plan_ddl(
        Dialect::Sqlite,
        Some(&meta),
        &[DdlOp::DropColumn {
            table: items.clone(),
            name: "name".into(),
        }],
    )
    .expect("plan");
    assert!(
        plan.warnings.iter().any(|w| w.contains(&index_name)),
        "{plan:?}"
    );
    apply_ok(
        &session,
        Some(&meta),
        vec![DdlOp::DropColumn {
            table: items.clone(),
            name: "name".into(),
        }],
    )
    .await;
    let meta = session.table_meta(&items).await.expect("meta");
    assert_eq!(names(&meta), ["id", "qty", "remark", "owner_id"]);
    assert!(!meta.indexes.iter().any(|i| i.name == index_name));
    assert_eq!(meta.foreign_keys.len(), 1);
    assert_eq!(
        scalar(&session, &format!("SELECT COUNT(*) FROM {q_items}")).await,
        2
    );

    // 8. truncate (resets AUTOINCREMENT) and drop
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
    session
        .execute(&format!("INSERT INTO {q_items} (qty) VALUES (1)"), &[])
        .await
        .expect("insert after truncate");
    assert_eq!(
        scalar(&session, &format!("SELECT MAX(id) FROM {q_items}")).await,
        1
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
                cascade: false,
            },
        ],
    )
    .await;
    let tables = session.list_tables("main").await.expect("list");
    assert!(
        !tables.iter().any(|t| t.name.starts_with(&prefix)),
        "{tables:?}"
    );
    session.close().await;
}

// --- regressions ------------------------------------------------------------------------------

fn scratch() -> String {
    format!(
        "tabler_ddl_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..10]
    )
}

async fn exec_all(session: &Arc<dyn DbSession>, statements: &[String]) {
    for sql in statements {
        session
            .execute(sql, &[])
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
}

async fn stored_sql(session: &Arc<dyn DbSession>, name: &str) -> String {
    let rs = session
        .query(
            "SELECT sql FROM sqlite_master WHERE name = ?",
            &[Value::Text(name.into())],
        )
        .await
        .expect("sqlite_master");
    match rs.rows.first().and_then(|r| r.first()) {
        Some(Value::Text(s)) => s.clone(),
        other => panic!("no sql for {name}: {other:?}"),
    }
}

/// A view on the table and another table's trigger writing into it used to make the rename
/// step of the rebuild fail ("error in view …: no such table").
#[tokio::test]
async fn rebuilds_a_table_that_views_and_other_triggers_name() {
    let Some((session, _dir)) = open().await else {
        return;
    };
    let x = scratch();
    let (p, v, log, tr) = (
        format!("{x}_p"),
        format!("{x}_v"),
        format!("{x}_log"),
        format!("{x}_tr"),
    );
    exec_all(
        &session,
        &[
            format!("CREATE TABLE \"{p}\" (id INTEGER PRIMARY KEY, name TEXT)"),
            format!("CREATE VIEW \"{v}\" AS SELECT id, name FROM \"{p}\""),
            format!("CREATE TABLE \"{log}\" (msg TEXT)"),
            format!(
                "CREATE TRIGGER \"{tr}\" AFTER INSERT ON \"{log}\" \
                 BEGIN INSERT INTO \"{p}\" (name) VALUES (NEW.msg); END"
            ),
            format!("INSERT INTO \"{p}\" (name) VALUES ('a')"),
        ],
    )
    .await;
    let table = TableRef::new(None, &p);
    let meta = session.table_meta(&table).await.expect("meta");
    apply_ok(
        &session,
        Some(&meta),
        vec![
            DdlOp::AlterColumnType {
                table: table.clone(),
                name: "name".into(),
                new_type: "VARCHAR(20)".into(),
                using: None,
            },
            DdlOp::AlterColumnNullable {
                table: table.clone(),
                name: "name".into(),
                nullable: false,
            },
        ],
    )
    .await;
    let meta = session.table_meta(&table).await.expect("meta");
    assert_eq!(column(&meta, "name").data_type, "VARCHAR(20)");
    assert!(!column(&meta, "name").nullable);
    // the view and the other table's trigger work against the rebuilt table
    session
        .execute(&format!("INSERT INTO \"{log}\" VALUES ('b')"), &[])
        .await
        .expect("trigger");
    assert_eq!(
        scalar(&session, &format!("SELECT COUNT(*) FROM \"{v}\"")).await,
        2
    );
    assert_eq!(scalar(&session, "PRAGMA legacy_alter_table").await, 0);
    session.close().await;
}

/// A rebuild that removes a key a child table references used to commit and leave every later
/// write failing with "foreign key mismatch".
#[tokio::test]
async fn a_rebuild_breaking_child_foreign_keys_is_rolled_back() {
    let Some((session, _dir)) = open().await else {
        return;
    };
    let x = scratch();
    let (p, c, p2, c2) = (
        format!("{x}_p"),
        format!("{x}_c"),
        format!("{x}_p2"),
        format!("{x}_c2"),
    );
    exec_all(
        &session,
        &[
            format!("CREATE TABLE \"{p}\" (id INTEGER PRIMARY KEY, code TEXT UNIQUE)"),
            format!("CREATE TABLE \"{c}\" (pcode TEXT REFERENCES \"{p}\"(code))"),
            format!("INSERT INTO \"{p}\" VALUES (1, 'a')"),
            format!("INSERT INTO \"{c}\" VALUES ('a')"),
            format!("CREATE TABLE \"{p2}\" (id INTEGER PRIMARY KEY, name TEXT)"),
            format!("CREATE TABLE \"{c2}\" (pid INTEGER REFERENCES \"{p2}\"(id))"),
            format!("INSERT INTO \"{p2}\" VALUES (1, 'x')"),
            format!("INSERT INTO \"{c2}\" VALUES (1)"),
        ],
    )
    .await;
    let cases = [
        (
            p.clone(),
            c.clone(),
            DdlOp::DropColumn {
                table: TableRef::new(None, &p),
                name: "code".into(),
            },
            "'a'",
        ),
        (
            p2.clone(),
            c2.clone(),
            DdlOp::DropPrimaryKey {
                table: TableRef::new(None, &p2),
            },
            "1",
        ),
    ];
    for (parent, child, op, value) in cases {
        let before = stored_sql(&session, &parent).await;
        let meta = session
            .table_meta(&TableRef::new(None, &parent))
            .await
            .expect("meta");
        let plan = plan_ddl(Dialect::Sqlite, Some(&meta), &[op]).expect("plan");
        let result = apply_plan(session.as_ref(), &plan).await;
        let failed = result.failed_index.expect("broken child detected");
        assert!(is_fk_check(&plan.statements[failed]), "{result:?}");
        let err = result.error.expect("error");
        assert!(err.message.contains("foreign key mismatch"), "{err:?}");
        assert_eq!(stored_sql(&session, &parent).await, before);
        session
            .execute(&format!("INSERT INTO \"{child}\" VALUES ({value})"), &[])
            .await
            .expect("the child still works");
        assert_eq!(scalar(&session, "PRAGMA foreign_keys").await, 1);
    }
    session.close().await;
}

/// Renames next to a rebuild run as SQLite renames after it: child foreign keys, views and
/// triggers (also ones naming another table's same-named column) follow, in either op order.
#[tokio::test]
async fn renames_next_to_a_rebuild_keep_children_views_and_triggers_working() {
    let Some((session, _dir)) = open().await else {
        return;
    };
    let x = scratch();
    let (p, c, v, audit, tr) = (
        format!("{x}_p"),
        format!("{x}_c"),
        format!("{x}_v"),
        format!("{x}_audit"),
        format!("{x}_tr"),
    );
    exec_all(
        &session,
        &[
            format!("CREATE TABLE \"{p}\" (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT)"),
            format!(
                "CREATE TABLE \"{c}\" (cid INTEGER PRIMARY KEY, pid INTEGER REFERENCES \"{p}\"(id))"
            ),
            format!("CREATE VIEW \"{v}\" AS SELECT id, name FROM \"{p}\""),
            format!("CREATE TABLE \"{audit}\" (id INT, note TEXT)"),
            format!(
                "CREATE TRIGGER \"{tr}\" AFTER UPDATE ON \"{p}\" \
                 BEGIN INSERT INTO \"{audit}\" (id, note) VALUES (NEW.id, 'u'); END"
            ),
            format!("INSERT INTO \"{p}\" (name) VALUES ('a'), ('b')"),
            format!("INSERT INTO \"{c}\" (pid) VALUES (1)"),
        ],
    )
    .await;
    let table = TableRef::new(None, &p);
    let steps = [
        // rename first (the trigger used to be replayed as `audit("item_id", …)`)
        vec![
            DdlOp::RenameColumn {
                table: table.clone(),
                name: "id".into(),
                new_name: "item_id".into(),
            },
            DdlOp::AlterColumnNullable {
                table: table.clone(),
                name: "name".into(),
                nullable: false,
            },
        ],
        // rename last (it used to be folded into the new table, leaving children pointing at
        // a column that no longer exists)
        vec![
            DdlOp::AlterColumnType {
                table: table.clone(),
                name: "name".into(),
                new_type: "VARCHAR(9)".into(),
                using: None,
            },
            DdlOp::RenameColumn {
                table: table.clone(),
                name: "item_id".into(),
                new_name: "pkey".into(),
            },
        ],
    ];
    for (i, ops) in steps.into_iter().enumerate() {
        let meta = session.table_meta(&table).await.expect("meta");
        apply_ok(&session, Some(&meta), ops).await;
        let key = if i == 0 { "item_id" } else { "pkey" };
        let meta = session.table_meta(&table).await.expect("meta");
        assert_eq!(names(&meta), [key, "name"]);
        assert_eq!(column(&meta, key).extra.as_deref(), Some("AUTOINCREMENT"));
        let child = stored_sql(&session, &c).await;
        assert!(child.contains(key), "{child}");
        session
            .execute(&format!("INSERT INTO \"{c}\" (pid) VALUES (2)"), &[])
            .await
            .expect("child insert");
        let err = session
            .execute(&format!("INSERT INTO \"{c}\" (pid) VALUES (99)"), &[])
            .await
            .expect_err("foreign key enforced");
        assert!(err.to_string().contains("FOREIGN KEY"), "{err}");
        session
            .execute(&format!("UPDATE \"{p}\" SET name = 'z{i}'"), &[])
            .await
            .expect("update fires the trigger");
        assert_eq!(
            scalar(&session, &format!("SELECT COUNT(*) FROM \"{audit}\"")).await,
            2 * (i as i64 + 1)
        );
        assert_eq!(
            scalar(&session, &format!("SELECT COUNT(*) FROM \"{v}\"")).await,
            2
        );
    }
    session.close().await;
}

/// The rebuild used to reset the AUTOINCREMENT counter to the largest remaining id.
#[tokio::test]
async fn a_rebuild_keeps_the_autoincrement_counter() {
    let Some((session, _dir)) = open().await else {
        return;
    };
    let x = scratch();
    for (suffix, keep) in [("p", 2), ("e", 0)] {
        let name = format!("{x}_{suffix}");
        exec_all(
            &session,
            &[
                format!("CREATE TABLE \"{name}\" (id INTEGER PRIMARY KEY AUTOINCREMENT, a TEXT)"),
                format!("INSERT INTO \"{name}\" (a) VALUES ('a'), ('b'), ('c')"),
                format!("DELETE FROM \"{name}\" WHERE id > {keep}"),
            ],
        )
        .await;
        let seq = format!("SELECT seq FROM sqlite_sequence WHERE name = '{name}'");
        assert_eq!(scalar(&session, &seq).await, 3);
        let table = TableRef::new(None, &name);
        let meta = session.table_meta(&table).await.expect("meta");
        apply_ok(
            &session,
            Some(&meta),
            vec![DdlOp::AlterColumnType {
                table: table.clone(),
                name: "a".into(),
                new_type: "VARCHAR(50)".into(),
                using: None,
            }],
        )
        .await;
        assert_eq!(scalar(&session, &seq).await, 3, "{name}");
        session
            .execute(&format!("INSERT INTO \"{name}\" (a) VALUES ('d')"), &[])
            .await
            .expect("insert");
        assert_eq!(
            scalar(&session, &format!("SELECT MAX(id) FROM \"{name}\"")).await,
            4,
            "{name}: the id of a deleted row was handed out again"
        );
    }
    session.close().await;
}

/// Dropping a rowid-alias primary key used to leave an undeclared NOT NULL behind.
#[tokio::test]
async fn dropping_a_primary_key_adds_no_not_null() {
    let Some((session, _dir)) = open().await else {
        return;
    };
    let name = format!("{}_p", scratch());
    exec_all(
        &session,
        &[
            format!("CREATE TABLE \"{name}\" (id INTEGER PRIMARY KEY, name TEXT)"),
            format!("INSERT INTO \"{name}\" (name) VALUES ('a')"),
        ],
    )
    .await;
    let table = TableRef::new(None, &name);
    let meta = session.table_meta(&table).await.expect("meta");
    apply_ok(
        &session,
        Some(&meta),
        vec![DdlOp::DropPrimaryKey {
            table: table.clone(),
        }],
    )
    .await;
    let meta = session.table_meta(&table).await.expect("meta");
    assert!(meta.primary_key.is_empty());
    assert!(column(&meta, "id").nullable);
    session
        .execute(&format!("INSERT INTO \"{name}\" (name) VALUES ('b')"), &[])
        .await
        .expect("insert without an id");
    session.close().await;
}

/// The rebuild block used to run as separate jobs on the shared worker connection: concurrent
/// commits failed ("cannot start a transaction within a transaction"), readers saw the table
/// missing, and another plan's `PRAGMA foreign_keys = ON` could slip in before `DROP TABLE`,
/// cascading into child tables.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn other_work_does_not_run_inside_a_rebuild() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let Some((session, _dir)) = open().await else {
        return;
    };
    let x = scratch();
    let (big, other, p, c, q) = (
        format!("{x}_big"),
        format!("{x}_other"),
        format!("{x}_p"),
        format!("{x}_c"),
        format!("{x}_q"),
    );
    exec_all(
        &session,
        &[
            format!("CREATE TABLE \"{big}\" (id INTEGER PRIMARY KEY, a INT, b TEXT)"),
            format!(
                "INSERT INTO \"{big}\" (a, b) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL \
                 SELECT i + 1 FROM n WHERE i < 200000) SELECT i, 'row ' || i FROM n"
            ),
            format!("CREATE TABLE \"{other}\" (v INT)"),
            format!("CREATE TABLE \"{p}\" (id INTEGER PRIMARY KEY, v INT)"),
            format!("CREATE TABLE \"{c}\" (pid INTEGER REFERENCES \"{p}\"(id) ON DELETE CASCADE)"),
            format!("CREATE TABLE \"{q}\" (id INTEGER PRIMARY KEY, v INT)"),
            format!(
                "INSERT INTO \"{p}\" (v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL \
                 SELECT i + 1 FROM n WHERE i < 50) SELECT i FROM n"
            ),
            format!("INSERT INTO \"{c}\" SELECT id FROM \"{p}\""),
            format!("INSERT INTO \"{q}\" SELECT id, v FROM \"{p}\""),
        ],
    )
    .await;
    let type_change = |name: &str, column: &str, ty: &str| DdlOp::AlterColumnType {
        table: TableRef::new(None, name),
        name: column.into(),
        new_type: ty.into(),
        using: None,
    };
    let plan_for = |name: String, op: DdlOp| {
        let session = session.clone();
        async move {
            let meta = session
                .table_meta(&TableRef::new(None, &name))
                .await
                .expect("meta");
            Arc::new(plan_ddl(Dialect::Sqlite, Some(&meta), &[op]).expect("plan"))
        }
    };

    // 1. commits and reads issued while the rebuild runs wait for it instead of failing
    let big_plan = plan_for(big.clone(), type_change(&big, "b", "VARCHAR(20)")).await;
    let done = Arc::new(AtomicBool::new(false));
    let commits = Arc::new(AtomicUsize::new(0));
    let mut workers = Vec::new();
    for _ in 0..4 {
        let (session, done, commits) = (session.clone(), done.clone(), commits.clone());
        let (big, other) = (big.clone(), other.clone());
        workers.push(tokio::spawn(async move {
            let mut errors = Vec::new();
            while !done.load(Ordering::SeqCst) {
                match session
                    .transaction(vec![TxStatement::new(
                        format!("INSERT INTO \"{other}\" VALUES (1)"),
                        vec![],
                    )])
                    .await
                {
                    Ok(_) => {
                        commits.fetch_add(1, Ordering::SeqCst);
                    }
                    Err(e) => errors.push(format!("commit: {e}")),
                }
                if let Err(e) = session
                    .query(&format!("SELECT COUNT(*) FROM \"{big}\""), &[])
                    .await
                {
                    errors.push(format!("read: {e}"));
                }
            }
            errors
        }));
    }
    for _ in 0..3 {
        let result = apply_plan(session.as_ref(), &big_plan).await;
        assert!(result.error.is_none(), "{result:?}");
    }
    done.store(true, Ordering::SeqCst);
    for w in workers {
        let errors = w.await.expect("worker");
        assert!(errors.is_empty(), "{errors:?}");
    }
    assert_eq!(
        scalar(&session, &format!("SELECT COUNT(*) FROM \"{other}\"")).await as usize,
        commits.load(Ordering::SeqCst)
    );
    assert_eq!(
        scalar(&session, &format!("SELECT COUNT(*) FROM \"{big}\"")).await,
        200000
    );

    // 2. two plans on one session never interleave their foreign-key toggles
    let p_plan = plan_for(p.clone(), type_change(&p, "v", "BIGINT")).await;
    let q_plan = plan_for(q.clone(), type_change(&q, "v", "BIGINT")).await;
    for _ in 0..20 {
        let runs = [p_plan.clone(), q_plan.clone()].map(|plan| {
            let session = session.clone();
            tokio::spawn(async move { apply_plan(session.as_ref(), &plan).await })
        });
        for run in runs {
            let result = run.await.expect("apply task");
            assert!(result.error.is_none(), "{result:?}");
        }
        assert_eq!(
            scalar(&session, &format!("SELECT COUNT(*) FROM \"{c}\"")).await,
            50,
            "child rows were deleted by a cascading DROP TABLE"
        );
    }

    // 3. a delete issued while the rebuild copies rows still cascades: it used to run between
    //    the block and `PRAGMA foreign_keys = ON`, with foreign keys off
    //    (the rebuild takes some tens of ms here: the delete is issued at several offsets so
    //    that some land while the block runs)
    let (p3, c3) = (format!("{x}_p3"), format!("{x}_c3"));
    exec_all(
        &session,
        &[
            format!("CREATE TABLE \"{p3}\" (id INTEGER PRIMARY KEY)"),
            format!(
                "CREATE TABLE \"{c3}\" (pid INTEGER REFERENCES \"{p3}\"(id) ON DELETE CASCADE)"
            ),
        ],
    )
    .await;
    for (i, offset) in [0u64, 5, 10, 20, 30, 40].into_iter().enumerate() {
        exec_all(
            &session,
            &[
                format!(
                    "INSERT INTO \"{p3}\" WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL \
                     SELECT i + 1 FROM n WHERE i < 50) SELECT i FROM n"
                ),
                format!("INSERT INTO \"{c3}\" SELECT id FROM \"{p3}\""),
            ],
        )
        .await;
        let ty = if i % 2 == 0 { "BIGINT" } else { "INT" };
        let big_plan = plan_for(big.clone(), type_change(&big, "a", ty)).await;
        let rebuild = {
            let (session, plan) = (session.clone(), big_plan.clone());
            tokio::spawn(async move { apply_plan(session.as_ref(), &plan).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(offset)).await;
        let deleted = session
            .transaction(vec![TxStatement::new(
                format!("DELETE FROM \"{p3}\""),
                vec![],
            )])
            .await
            .expect("delete parents");
        assert_eq!(deleted[0].affected, 50);
        let result = rebuild.await.expect("apply task");
        assert!(result.error.is_none(), "{result:?}");
        assert_eq!(
            scalar(&session, &format!("SELECT COUNT(*) FROM \"{c3}\"")).await,
            0,
            "delete at +{offset} ms ran with foreign keys off: its cascade was skipped"
        );
        assert_eq!(scalar(&session, "PRAGMA foreign_keys").await, 1);
    }
    session.close().await;
}

/// Dropping a column through a rebuild used to commit even when a view or trigger still used
/// the column, leaving them failing on every use (and every later rename of any table).
#[tokio::test]
async fn dropping_a_column_a_view_or_trigger_uses_is_rolled_back() {
    let Some((session, _dir)) = open().await else {
        return;
    };
    let x = scratch();
    let (t, v, log, tr) = (
        format!("{x}_t"),
        format!("{x}_v"),
        format!("{x}_log"),
        format!("{x}_tr"),
    );
    exec_all(
        &session,
        &[
            format!("CREATE TABLE \"{t}\" (id INTEGER PRIMARY KEY, a TEXT, b TEXT)"),
            // the index makes DROP COLUMN a rebuild
            format!("CREATE INDEX \"{t}_b\" ON \"{t}\" (b)"),
            format!("CREATE VIEW \"{v}\" AS SELECT id, b FROM \"{t}\""),
            format!("CREATE TABLE \"{log}\" (msg TEXT)"),
            format!(
                "CREATE TRIGGER \"{tr}\" AFTER UPDATE OF a ON \"{t}\" \
                 BEGIN INSERT INTO \"{log}\" VALUES (NEW.b); END"
            ),
            format!("INSERT INTO \"{t}\" (a, b) VALUES ('x', 'y')"),
        ],
    )
    .await;
    let table = TableRef::new(None, &t);
    let drop_b = || DdlOp::DropColumn {
        table: table.clone(),
        name: "b".into(),
    };
    for (user, drop_user) in [
        (format!("view {v}"), format!("DROP VIEW \"{v}\"")),
        (format!("trigger {tr}"), format!("DROP TRIGGER \"{tr}\"")),
    ] {
        let meta = session.table_meta(&table).await.expect("meta");
        let plan = plan_ddl(Dialect::Sqlite, Some(&meta), &[drop_b()]).expect("plan");
        assert!(
            plan.warnings.iter().any(|w| w.contains("view or trigger")),
            "{:?}",
            plan.warnings
        );
        let result = apply_plan(session.as_ref(), &plan).await;
        let err = result.error.expect("the change is refused");
        assert!(
            err.message.contains(&format!("error in {user}"))
                && err.message.contains("no such column"),
            "{err:?}"
        );
        // nothing changed, and the connection is fine
        let meta = session.table_meta(&table).await.expect("meta");
        assert_eq!(names(&meta), ["id", "a", "b"]);
        assert_eq!(scalar(&session, "PRAGMA foreign_keys").await, 1);
        assert_eq!(scalar(&session, "PRAGMA legacy_alter_table").await, 0);
        session
            .execute(&format!("UPDATE \"{t}\" SET a = 'z'"), &[])
            .await
            .expect("the trigger still works");
        exec_all(&session, &[drop_user]).await;
    }
    // with the view and the trigger gone the column can go
    let meta = session.table_meta(&table).await.expect("meta");
    apply_ok(&session, Some(&meta), vec![drop_b()]).await;
    let meta = session.table_meta(&table).await.expect("meta");
    assert_eq!(names(&meta), ["id", "a"]);
    assert_eq!(
        scalar(
            &session,
            "SELECT COUNT(*) FROM sqlite_master WHERE instr(name, '__tabler') > 0"
        )
        .await,
        0
    );
    session.close().await;
}

/// SQLite refuses DROP INDEX on a primary key's autoindex; dropping that index drops the key
/// through a rebuild instead.
#[tokio::test]
async fn dropping_the_primary_key_index_drops_the_key() {
    let Some((session, _dir)) = open().await else {
        return;
    };
    let k = format!("{}_k", scratch());
    exec_all(
        &session,
        &[
            format!("CREATE TABLE \"{k}\" (code TEXT PRIMARY KEY, v TEXT)"),
            format!("INSERT INTO \"{k}\" VALUES ('a', '1')"),
        ],
    )
    .await;
    let table = TableRef::new(None, &k);
    let meta = session.table_meta(&table).await.expect("meta");
    let pk_index = meta
        .indexes
        .iter()
        .find(|i| i.primary)
        .expect("primary-key autoindex")
        .name
        .clone();
    apply_ok(
        &session,
        Some(&meta),
        vec![DdlOp::DropIndex {
            table: table.clone(),
            name: pk_index,
        }],
    )
    .await;
    let meta = session.table_meta(&table).await.expect("meta");
    assert!(meta.primary_key.is_empty(), "{meta:?}");
    assert!(meta.indexes.is_empty(), "{:?}", meta.indexes);
    // the former key accepts duplicates now
    session
        .execute(&format!("INSERT INTO \"{k}\" VALUES ('a', '2')"), &[])
        .await
        .expect("duplicate code");
    session.close().await;
}
