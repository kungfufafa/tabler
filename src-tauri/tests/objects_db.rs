//! Integration test of the sidebar objects (M8) against real servers: routine / sequence listing
//! and definitions, the Item Overview, and the drop / truncate / clone / comment operations.
//! Runs only when `TABLER_TEST_PG_URL` / `TABLER_TEST_MYSQL_URL` are set; every object it creates
//! is named `obj_<random>_*` and dropped at the end.

use std::sync::Arc;

use tabler_lib::commands::ddl::apply_plan;
use tabler_lib::drivers::{ConnectTarget, DbSession, connect};
use tabler_lib::objects::{
    item_overview, list_routines, list_sequences, plan_op, routine_definition, run_mysql_plan,
    sequence_definition, system_schemas,
};
use tabler_lib::sqlgen::Dialect;
use tabler_lib::sqlgen::split::split_statements;
use tabler_lib::types::ddl::DdlResult;
use tabler_lib::types::objects::{ObjectOp, ObjectTarget, RoutineKind, RoutineRef};
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
use tabler_lib::types::schema::{TableKind, TableRef};
use tabler_lib::types::value::Value;

/// `scheme://user[:pass]@host:port/db` into a profile plus the password.
fn profile_from_url(url: &str, driver: Driver) -> (ConnectionProfile, Option<String>) {
    let rest = url.split_once("://").map(|(_, r)| r).expect("a URL");
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
        name: "objects-test".into(),
        color: None,
        group: None,
        driver,
        host: Some(host),
        port,
        user,
        database: db.filter(|d| !d.is_empty()),
        ssl: if driver == Driver::Postgres {
            SslMode::Prefer
        } else {
            SslMode::Disable
        },
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
    (profile, password)
}

async fn open(env: &str, driver: Driver) -> Option<Arc<dyn DbSession>> {
    let Ok(url) = std::env::var(env) else {
        println!("skipped: {env} unset");
        return None;
    };
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let (profile, password) = profile_from_url(&url, driver);
    Some(
        connect(ConnectTarget {
            profile: &profile,
            password: password.as_deref(),
            host_override: None,
        })
        .await
        .expect("connect"),
    )
}

fn prefix() -> String {
    format!("obj_{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
}

async fn exec(session: &Arc<dyn DbSession>, sql: &str) {
    session
        .execute(sql, &[])
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

async fn count(session: &Arc<dyn DbSession>, sql: &str) -> i64 {
    let rs = session.query(sql, &[]).await.expect(sql);
    match rs.rows.first().and_then(|r| r.first()) {
        Some(Value::Int(n)) => *n,
        Some(Value::Text(s)) | Some(Value::BigInt(s)) | Some(Value::Decimal(s)) => {
            s.parse().unwrap()
        }
        other => panic!("count of {sql}: {other:?}"),
    }
}

async fn run(session: &Arc<dyn DbSession>, op: ObjectOp) -> DdlResult {
    let plan = plan_op(session.as_ref(), &op).await.expect("plan");
    let result = match session.dialect().core().unwrap() {
        Dialect::Mysql => run_mysql_plan(session.as_ref(), &plan).await,
        _ => apply_plan(session.as_ref(), &plan).await,
    };
    assert!(
        result.error.is_none(),
        "{op:?} failed: {:?}\n{:#?}",
        result.error,
        plan.statements
    );
    result
}

#[tokio::test]
async fn postgres_objects() {
    let Some(s) = open("TABLER_TEST_PG_URL", Driver::Postgres).await else {
        return;
    };
    let p = prefix();
    let schema = "public";
    let t = |name: &str| TableRef::new(Some(schema), &format!("{p}_{name}"));
    let parent = t("parent");
    let clone = t("parent_copy");
    for sql in [
        format!(
            "CREATE TABLE public.{p}_parent (id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY, \
             name text, doubled integer GENERATED ALWAYS AS (id * 2) STORED)"
        ),
        format!("INSERT INTO public.{p}_parent (name) VALUES ('a'), ('b'), ('c')"),
        format!("CREATE VIEW public.{p}_view AS SELECT id FROM public.{p}_parent"),
        format!(
            "CREATE FUNCTION public.{p}_add(a integer, b integer) RETURNS integer LANGUAGE sql AS 'SELECT a + b'"
        ),
        format!(
            "CREATE FUNCTION public.{p}_add(a numeric, b numeric) RETURNS numeric LANGUAGE sql AS 'SELECT a + b'"
        ),
        format!(
            "CREATE PROCEDURE public.{p}_proc() LANGUAGE plpgsql AS $$ BEGIN PERFORM 1; END $$"
        ),
        format!(
            "CREATE AGGREGATE public.{p}_sum(numeric) (SFUNC = numeric_add, STYPE = numeric, INITCOND = '0')"
        ),
        format!("CREATE SEQUENCE public.{p}_seq AS integer INCREMENT BY 5 START WITH 10"),
    ] {
        exec(&s, &sql).await;
    }

    // Routines: overloads apart, kinds, signatures.
    let routines: Vec<_> = list_routines(s.as_ref(), schema)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.name.starts_with(&p))
        .collect();
    let sig: Vec<_> = routines
        .iter()
        .map(|r| {
            (
                r.name.trim_start_matches(&p).to_string(),
                r.kind,
                r.identity_arguments.clone(),
            )
        })
        .collect();
    assert_eq!(
        sig,
        [
            (
                "_add".into(),
                RoutineKind::Function,
                "a integer, b integer".into()
            ),
            (
                "_add".into(),
                RoutineKind::Function,
                "a numeric, b numeric".into()
            ),
            ("_proc".into(), RoutineKind::Procedure, String::new()),
            ("_sum".into(), RoutineKind::Aggregate, "numeric".into()),
        ]
    );
    assert_eq!(routines[0].return_type.as_deref(), Some("integer"));
    assert_eq!(routines[2].return_type, None);
    assert_eq!(routines[0].language.as_deref(), Some("sql"));

    let rref = |name: &str, kind, args: &str| RoutineRef {
        schema: Some(schema.into()),
        name: format!("{p}_{name}"),
        kind,
        identity_arguments: args.into(),
    };
    let def = routine_definition(
        s.as_ref(),
        &rref("add", RoutineKind::Function, "a numeric, b numeric"),
    )
    .await
    .unwrap();
    assert!(def.starts_with("CREATE OR REPLACE FUNCTION"), "{def}");
    assert!(def.contains("numeric"), "{def}");
    exec(&s, &def).await;
    let agg = routine_definition(s.as_ref(), &rref("sum", RoutineKind::Aggregate, "numeric"))
        .await
        .unwrap();
    assert!(agg.contains("SFUNC = numeric_add"), "{agg}");
    exec(&s, &agg).await;
    assert!(
        routine_definition(s.as_ref(), &rref("add", RoutineKind::Function, "text"))
            .await
            .is_err()
    );

    // Sequences.
    let seqs = list_sequences(s.as_ref(), schema).await.unwrap();
    assert!(seqs.iter().any(|x| x.name == format!("{p}_seq")));
    let identity = seqs
        .iter()
        .find(|x| x.name.starts_with(&format!("{p}_parent")))
        .expect("the identity sequence");
    assert_eq!(
        identity.owned_by.as_deref(),
        Some(&*format!("{p}_parent.id"))
    );
    let seq_ddl = sequence_definition(s.as_ref(), &t("seq")).await.unwrap();
    assert!(
        seq_ddl.contains("INCREMENT BY 5") && seq_ddl.contains("START WITH 10"),
        "{seq_ddl}"
    );
    exec(&s, &format!("DROP SEQUENCE public.{p}_seq")).await;
    for stmt in split_statements(&seq_ddl, Dialect::Postgres) {
        exec(&s, &stmt.sql).await;
    }
    assert!(
        system_schemas(s.as_ref())
            .await
            .unwrap()
            .iter()
            .any(|x| x.name == "pg_catalog")
    );

    // Comment, overview.
    run(
        &s,
        ObjectOp::SetComment {
            target: ObjectTarget {
                table: parent.clone(),
                kind: TableKind::Table,
            },
            comment: Some("it's the parent".into()),
        },
    )
    .await;
    run(
        &s,
        ObjectOp::SetComment {
            target: ObjectTarget {
                table: t("view"),
                kind: TableKind::View,
            },
            comment: Some("a view".into()),
        },
    )
    .await;
    let overview = item_overview(s.as_ref(), schema).await.unwrap();
    let item = overview.iter().find(|i| i.name == parent.name).unwrap();
    assert_eq!(item.comment.as_deref(), Some("it's the parent"));
    assert!(item.total_size.unwrap_or(0) > 0);
    assert!(item.owner.is_some());
    let view = overview
        .iter()
        .find(|i| i.name == format!("{p}_view"))
        .unwrap();
    assert_eq!(
        (view.kind, view.comment.as_deref()),
        (TableKind::View, Some("a view"))
    );

    // Clone with data: identity values and generated columns survive, the identity continues.
    run(
        &s,
        ObjectOp::CloneTable {
            source: parent.clone(),
            new_name: clone.name.clone(),
            with_data: true,
        },
    )
    .await;
    assert_eq!(
        count(&s, &format!("SELECT count(*) FROM public.{p}_parent_copy")).await,
        3
    );
    exec(
        &s,
        &format!("INSERT INTO public.{p}_parent_copy (name) VALUES ('d')"),
    )
    .await;
    assert_eq!(
        count(&s, &format!("SELECT max(id) FROM public.{p}_parent_copy")).await,
        4
    );

    // Truncate several tables, restarting identities.
    run(
        &s,
        ObjectOp::TruncateTables {
            tables: vec![parent.clone(), clone.clone()],
            cascade: false,
            restart_identity: true,
            ignore_foreign_keys: false,
        },
    )
    .await;
    assert_eq!(
        count(&s, &format!("SELECT count(*) FROM public.{p}_parent")).await,
        0
    );

    // Drop one overload, then everything.
    run(
        &s,
        ObjectOp::DropRoutines {
            routines: vec![rref("add", RoutineKind::Function, "a integer, b integer")],
            cascade: false,
        },
    )
    .await;
    let left = list_routines(s.as_ref(), schema).await.unwrap();
    assert!(left.iter().any(|r| r.name == format!("{p}_add")));
    // An argument list that is not the server's own is refused before anything runs.
    assert!(
        plan_op(
            s.as_ref(),
            &ObjectOp::DropRoutines {
                routines: vec![rref("add", RoutineKind::Function, "integer, integer")],
                cascade: false,
            },
        )
        .await
        .is_err()
    );
    run(
        &s,
        ObjectOp::DropRoutines {
            routines: vec![
                rref("add", RoutineKind::Function, "a numeric, b numeric"),
                rref("proc", RoutineKind::Procedure, ""),
                rref("sum", RoutineKind::Aggregate, "numeric"),
            ],
            cascade: false,
        },
    )
    .await;
    run(
        &s,
        ObjectOp::DropSequences {
            sequences: vec![t("seq")],
            cascade: false,
        },
    )
    .await;
    run(
        &s,
        ObjectOp::DropTables {
            tables: vec![
                ObjectTarget {
                    table: parent.clone(),
                    kind: TableKind::Table,
                },
                ObjectTarget {
                    table: clone.clone(),
                    kind: TableKind::Table,
                },
            ],
            cascade: true,
            ignore_foreign_keys: false,
        },
    )
    .await;
    let rest = list_routines(s.as_ref(), schema).await.unwrap();
    assert!(!rest.iter().any(|r| r.name.starts_with(&p)));
    let tables = s.list_tables(schema).await.unwrap();
    assert!(!tables.iter().any(|x| x.name.starts_with(&p)), "{tables:?}");
}

#[tokio::test]
async fn mysql_objects() {
    let Some(s) = open("TABLER_TEST_MYSQL_URL", Driver::Mysql).await else {
        return;
    };
    let p = prefix();
    let db = match s.query("SELECT DATABASE()", &[]).await.unwrap().rows[0][0].clone() {
        Value::Text(d) => d,
        other => panic!("no database selected: {other:?}"),
    };
    let t = |name: &str| TableRef::new(Some(&db), &format!("{p}_{name}"));
    let (parent, child, clone) = (t("parent"), t("child"), t("parent_copy"));
    for sql in [
        format!(
            "CREATE TABLE {p}_parent (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(20), \
             name_len INT GENERATED ALWAYS AS (CHAR_LENGTH(name))) COMMENT 'old'"
        ),
        format!(
            "CREATE TABLE {p}_child (id INT PRIMARY KEY, parent_id INT, \
             FOREIGN KEY (parent_id) REFERENCES {p}_parent (id))"
        ),
        format!("INSERT INTO {p}_parent (name) VALUES ('a'), ('b')"),
        format!("INSERT INTO {p}_child VALUES (1, 1)"),
        format!("CREATE FUNCTION {p}_fn(a INT) RETURNS INT DETERMINISTIC RETURN a + 1"),
        format!("CREATE PROCEDURE {p}_proc(IN n INT)\nBEGIN\n  SELECT n;\n  SELECT n + 1;\nEND"),
    ] {
        exec(&s, &sql).await;
    }

    let routines: Vec<_> = list_routines(s.as_ref(), &db)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.name.starts_with(&p))
        .collect();
    assert_eq!(routines.len(), 2, "{routines:?}");
    let f = routines
        .iter()
        .find(|r| r.kind == RoutineKind::Function)
        .unwrap();
    assert_eq!(f.arguments, "a int");
    assert_eq!(f.return_type.as_deref(), Some("int"));
    let proc = routines
        .iter()
        .find(|r| r.kind == RoutineKind::Procedure)
        .unwrap();
    assert_eq!(proc.arguments, "IN n int");

    // Opening a procedure: DROP … IF EXISTS + CREATE, which the editor runs as two statements.
    let script = routine_definition(
        s.as_ref(),
        &RoutineRef {
            schema: Some(db.clone()),
            name: proc.name.clone(),
            kind: RoutineKind::Procedure,
            identity_arguments: String::new(),
        },
    )
    .await
    .unwrap();
    assert!(script.starts_with("DROP PROCEDURE IF EXISTS"), "{script}");
    let parts = split_statements(&script, Dialect::Mysql);
    assert_eq!(parts.len(), 2, "{parts:?}");
    for part in parts {
        exec(&s, &part.sql).await;
    }

    let overview = item_overview(s.as_ref(), &db).await.unwrap();
    let item = overview.iter().find(|i| i.name == parent.name).unwrap();
    assert_eq!(item.engine.as_deref(), Some("InnoDB"));
    assert!(item.charset.is_some() && item.collation.is_some());
    assert_eq!(item.comment.as_deref(), Some("old"));
    run(
        &s,
        ObjectOp::SetComment {
            target: ObjectTarget {
                table: parent.clone(),
                kind: TableKind::Table,
            },
            comment: Some("it's new".into()),
        },
    )
    .await;

    run(
        &s,
        ObjectOp::CloneTable {
            source: parent.clone(),
            new_name: clone.name.clone(),
            with_data: true,
        },
    )
    .await;
    assert_eq!(
        count(&s, &format!("SELECT COUNT(*) FROM {p}_parent_copy")).await,
        2
    );

    // TRUNCATE of a referenced table needs the checks off, in the same session.
    let plain = ObjectOp::TruncateTables {
        tables: vec![parent.clone()],
        cascade: false,
        restart_identity: false,
        ignore_foreign_keys: false,
    };
    let plan = plan_op(s.as_ref(), &plain).await.unwrap();
    assert!(run_mysql_plan(s.as_ref(), &plan).await.error.is_some());
    run(
        &s,
        ObjectOp::TruncateTables {
            tables: vec![parent.clone()],
            cascade: false,
            restart_identity: false,
            ignore_foreign_keys: true,
        },
    )
    .await;
    assert_eq!(
        count(&s, &format!("SELECT COUNT(*) FROM {p}_parent")).await,
        0
    );
    // The pool hands out connections with the checks back on.
    assert_eq!(count(&s, "SELECT @@foreign_key_checks").await, 1);

    run(
        &s,
        ObjectOp::DropRoutines {
            routines: routines
                .iter()
                .map(|r| RoutineRef {
                    schema: Some(db.clone()),
                    name: r.name.clone(),
                    kind: r.kind,
                    identity_arguments: String::new(),
                })
                .collect(),
            cascade: false,
        },
    )
    .await;
    // The parent goes first while the child still references it.
    run(
        &s,
        ObjectOp::DropTables {
            tables: [&parent, &clone, &child]
                .into_iter()
                .map(|t| ObjectTarget {
                    table: t.clone(),
                    kind: TableKind::Table,
                })
                .collect(),
            cascade: false,
            ignore_foreign_keys: true,
        },
    )
    .await;
    let tables = s.list_tables(&db).await.unwrap();
    assert!(!tables.iter().any(|x| x.name.starts_with(&p)), "{tables:?}");
    assert!(
        !list_routines(s.as_ref(), &db)
            .await
            .unwrap()
            .iter()
            .any(|r| r.name.starts_with(&p))
    );
}

#[tokio::test]
async fn sqlite_objects() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("objects.sqlite");
    let profile = ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "sqlite-objects-test".into(),
        color: None,
        group: None,
        driver: Driver::Sqlite,
        host: None,
        port: None,
        user: None,
        database: None,
        ssl: SslMode::Disable,
        ssl_ca_path: None,
        sqlite_path: Some(path.to_string_lossy().into_owned()),
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
    let s = connect(ConnectTarget {
        profile: &profile,
        password: None,
        host_override: None,
    })
    .await
    .expect("open");
    for sql in [
        "CREATE TABLE \"obj items\" (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, \
         name_len INTEGER GENERATED ALWAYS AS (length(name)) VIRTUAL)",
        "CREATE INDEX obj_items_name ON \"obj items\" (name)",
        "INSERT INTO \"obj items\" (name) VALUES ('a'), ('bb')",
        "CREATE VIEW obj_view AS SELECT id FROM \"obj items\"",
    ] {
        exec(&s, sql).await;
    }
    assert!(list_routines(s.as_ref(), "main").await.unwrap().is_empty());
    let source = TableRef::new(Some("main"), "obj items");
    run(
        &s,
        ObjectOp::CloneTable {
            source: source.clone(),
            new_name: "obj items copy".into(),
            with_data: true,
        },
    )
    .await;
    assert_eq!(
        count(&s, "SELECT count(*) FROM \"obj items copy\"").await,
        2
    );
    assert_eq!(
        count(&s, "SELECT max(name_len) FROM \"obj items copy\"").await,
        2
    );
    // AUTOINCREMENT continues after the copied ids.
    exec(&s, "INSERT INTO \"obj items copy\" (name) VALUES ('c')").await;
    assert_eq!(count(&s, "SELECT max(id) FROM \"obj items copy\"").await, 3);

    let overview = item_overview(s.as_ref(), "main").await.unwrap();
    let item = overview.iter().find(|i| i.name == "obj items").unwrap();
    assert_eq!(item.kind, TableKind::Table);
    let view = overview.iter().find(|i| i.name == "obj_view").unwrap();
    assert_eq!((view.kind, view.total_size), (TableKind::View, None));

    run(
        &s,
        ObjectOp::TruncateTables {
            tables: vec![source.clone()],
            cascade: false,
            restart_identity: false,
            ignore_foreign_keys: false,
        },
    )
    .await;
    assert_eq!(count(&s, "SELECT count(*) FROM \"obj items\"").await, 0);
    run(
        &s,
        ObjectOp::DropTables {
            tables: vec![
                ObjectTarget {
                    table: TableRef::new(Some("main"), "obj_view"),
                    kind: TableKind::View,
                },
                ObjectTarget {
                    table: source,
                    kind: TableKind::Table,
                },
            ],
            cascade: false,
            ignore_foreign_keys: false,
        },
    )
    .await;
    let names: Vec<_> = s
        .list_tables("main")
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["obj items copy"]);
}
