//! Regression tests (against real servers) of the object and admin fixes: drop / truncate of
//! tables that reference each other or are partitions, clones that keep serial sequences,
//! self-references and indexes of their own, MySQL literals under `NO_BACKSLASH_ESCAPES`,
//! database-level grants on names with `_`, and PostgreSQL passwords sent as SCRAM verifiers.
//! Gated on `TABLER_TEST_PG_URL` / `TABLER_TEST_MYSQL_URL` (the SQLite test always runs); every
//! schema, database, table and account it creates is named `fix_obj_<random>…` and dropped at
//! the end.

use std::sync::Arc;

use base64::Engine as _;
use tabler_lib::admin;
use tabler_lib::commands::ddl::apply_plan;
use tabler_lib::drivers::{ConnectTarget, DbSession, connect};
use tabler_lib::objects::{plan_op, run_mysql_plan};
use tabler_lib::sqlgen::Dialect;
use tabler_lib::sqlgen::admin::{mysql_db_pattern, scram_sha_256_verifier};
use tabler_lib::sqlgen::quote::MysqlLiteralMode;
use tabler_lib::types::admin::{ObjectPrivileges, UserChange, UserRef, UserState};
use tabler_lib::types::ddl::DdlResult;
use tabler_lib::types::objects::{ObjectOp, ObjectTarget};
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
use tabler_lib::types::schema::{TableKind, TableRef};
use tabler_lib::types::value::Value;

/// `scheme://user[:pass]@host[:port]/db` into a profile plus the password.
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
        name: "fix-obj-test".into(),
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

async fn open_profile(
    profile: &ConnectionProfile,
    password: Option<&str>,
) -> Result<Arc<dyn DbSession>, String> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    connect(ConnectTarget {
        profile,
        password,
        host_override: None,
    })
    .await
    .map_err(|e| e.to_string())
}

fn suffix() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

fn cell_text(v: &Value) -> Option<String> {
    match v {
        Value::Bytes(b) => base64::engine::general_purpose::STANDARD
            .decode(&b.base64)
            .ok()
            .map(|raw| String::from_utf8_lossy(&raw).into_owned()),
        other => other.as_text(),
    }
}

async fn exec(s: &dyn DbSession, sql: &str) {
    s.execute(sql, &[])
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

/// First column of every row, as text.
async fn column(s: &dyn DbSession, sql: &str, params: &[Value]) -> Vec<Option<String>> {
    let rs = s
        .query(sql, params)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
    rs.rows
        .iter()
        .map(|r| r.first().and_then(cell_text))
        .collect()
}

async fn scalar(s: &dyn DbSession, sql: &str) -> Option<String> {
    column(s, sql, &[]).await.into_iter().next().flatten()
}

async fn try_run(s: &dyn DbSession, op: &ObjectOp) -> DdlResult {
    let plan = plan_op(s, op).await.expect("plan");
    match s.dialect().core().unwrap() {
        Dialect::Mysql => run_mysql_plan(s, &plan).await,
        _ => apply_plan(s, &plan).await,
    }
}

async fn run(s: &dyn DbSession, op: ObjectOp) {
    let plan = plan_op(s, &op).await.expect("plan");
    let result = try_run(s, &op).await;
    assert!(
        result.error.is_none(),
        "{op:?} failed: {:?}\n{:#?}",
        result.error,
        plan.statements
    );
}

fn table(t: &TableRef, kind: TableKind) -> ObjectTarget {
    ObjectTarget {
        table: t.clone(),
        kind,
    }
}

fn privs(object: &str, list: &[&str]) -> ObjectPrivileges {
    ObjectPrivileges {
        object: object.into(),
        privileges: list.iter().map(|s| s.to_string()).collect(),
    }
}

#[tokio::test]
async fn fix_obj_postgres() {
    let Ok(url) = std::env::var("TABLER_TEST_PG_URL") else {
        println!("skipped: TABLER_TEST_PG_URL unset");
        return;
    };
    let (profile, password) = profile_from_url(&url, Driver::Postgres);
    let session = open_profile(&profile, password.as_deref())
        .await
        .expect("connect");
    let s = session.as_ref();
    let id = suffix();
    let schema = format!("fix_obj_{id}");
    let role = format!("fix_obj_{id}_role");
    let q = |n: &str| format!("\"{schema}\".\"{n}\"");
    let t = |n: &str| TableRef::new(Some(&schema), n);
    exec(s, &format!("CREATE SCHEMA \"{schema}\"")).await;

    for sql in [
        format!(
            "CREATE TABLE {} (id serial PRIMARY KEY, name text)",
            q("parent")
        ),
        format!(
            "CREATE TABLE {} (id int GENERATED ALWAYS AS IDENTITY PRIMARY KEY, \
             parent_id int REFERENCES {}(id))",
            q("child"),
            q("parent")
        ),
        format!(
            "INSERT INTO {} (name) VALUES ('a'), ('b'), ('c')",
            q("parent")
        ),
        format!("INSERT INTO {} (parent_id) VALUES (1), (2)", q("child")),
    ] {
        exec(s, &sql).await;
    }

    // TRUNCATE of a parent and its child: one statement, so no CASCADE is needed.
    run(
        s,
        ObjectOp::TruncateTables {
            tables: vec![t("parent"), t("child")],
            cascade: false,
            restart_identity: true,
            ignore_foreign_keys: false,
        },
    )
    .await;
    assert_eq!(
        scalar(s, &format!("SELECT count(*)::text FROM {}", q("child"))).await,
        Some("0".into())
    );
    exec(
        s,
        &format!("INSERT INTO {} (name) VALUES ('x'), ('y')", q("parent")),
    )
    .await;

    // Clone of a serial column: a sequence of its own, owned by the clone, continuing.
    run(
        s,
        ObjectOp::CloneTable {
            source: t("parent"),
            new_name: "parent_copy".into(),
            with_data: true,
        },
    )
    .await;
    run(
        s,
        ObjectOp::CloneTable {
            source: t("child"),
            new_name: "child_copy".into(),
            with_data: false,
        },
    )
    .await;
    let seq_of = |table: &str| {
        format!(
            "SELECT pg_get_serial_sequence('{}', 'id')",
            q(table).replace('\'', "''")
        )
    };
    let parent_seq = scalar(s, &seq_of("parent")).await.unwrap();
    let copy_seq = scalar(s, &seq_of("parent_copy")).await.unwrap();
    assert_ne!(parent_seq, copy_seq);
    assert!(copy_seq.ends_with("parent_copy_id_seq"), "{copy_seq}");
    // Identity columns already got a sequence of their own from LIKE.
    assert_ne!(
        scalar(s, &seq_of("child")).await,
        scalar(s, &seq_of("child_copy")).await
    );

    // Dropping a parent together with its child: one statement, no CASCADE needed. The clone
    // keeps working without the original's sequence.
    run(
        s,
        ObjectOp::DropTables {
            tables: vec![
                table(&t("parent"), TableKind::Table),
                table(&t("child"), TableKind::Table),
            ],
            cascade: false,
            ignore_foreign_keys: false,
        },
    )
    .await;
    exec(
        s,
        &format!("INSERT INTO {} (name) VALUES ('z')", q("parent_copy")),
    )
    .await;
    assert_eq!(
        scalar(
            s,
            &format!("SELECT max(id)::text FROM {}", q("parent_copy"))
        )
        .await,
        Some("3".into())
    );

    // A partitioned table dropped with some of its partitions (one level down and two).
    for sql in [
        format!("CREATE TABLE {} (k int) PARTITION BY RANGE (k)", q("p")),
        format!(
            "CREATE TABLE {} PARTITION OF {} FOR VALUES FROM (0) TO (10)",
            q("p1"),
            q("p")
        ),
        format!(
            "CREATE TABLE {} PARTITION OF {} FOR VALUES FROM (10) TO (20) PARTITION BY RANGE (k)",
            q("p2"),
            q("p")
        ),
        format!(
            "CREATE TABLE {} PARTITION OF {} FOR VALUES FROM (10) TO (15)",
            q("p2_1"),
            q("p2")
        ),
    ] {
        exec(s, &sql).await;
    }
    let drop_partitioned = ObjectOp::DropTables {
        tables: vec![
            table(&t("p1"), TableKind::Table),
            table(&t("p"), TableKind::Partitioned),
            table(&t("p2_1"), TableKind::Table),
        ],
        cascade: false,
        ignore_foreign_keys: false,
    };
    let plan = plan_op(s, &drop_partitioned).await.unwrap();
    assert_eq!(plan.statements, [format!("DROP TABLE {}", q("p"))]);
    assert_eq!(
        plan.warnings,
        ["2 partitions are dropped with their partitioned table"]
    );
    run(s, drop_partitioned).await;

    // A view and the table it reads, with CASCADE: the view goes first.
    exec(
        s,
        &format!(
            "CREATE VIEW {} AS SELECT * FROM {}",
            q("v"),
            q("parent_copy")
        ),
    )
    .await;
    run(
        s,
        ObjectOp::DropTables {
            tables: vec![
                table(&t("parent_copy"), TableKind::Table),
                table(&t("v"), TableKind::View),
                table(&t("child_copy"), TableKind::Table),
            ],
            cascade: true,
            ignore_foreign_keys: false,
        },
    )
    .await;

    // A comment with a backslash and a quote reads back as written.
    exec(s, &format!("CREATE TABLE {} (id int)", q("c"))).await;
    let comment = r"C:\temp it's";
    run(
        s,
        ObjectOp::SetComment {
            target: table(&t("c"), TableKind::Table),
            comment: Some(comment.into()),
        },
    )
    .await;
    assert_eq!(
        column(
            s,
            "SELECT obj_description($1::text::regclass, 'pg_class')",
            &[Value::Text(q("c"))]
        )
        .await,
        [Some(comment.to_string())]
    );

    // A role's password goes out as a SCRAM verifier the server stores as is.
    let secret = r"pa'ss\word";
    let create = UserChange {
        original: None,
        edited: Some(UserState {
            name: role.clone(),
            attributes: Some(tabler_lib::types::admin::RoleAttributes {
                login: true,
                ..Default::default()
            }),
            ..UserState::default()
        }),
        password: Some(secret.into()),
    };
    let preview = admin::preview_user_on(s, &create).await.unwrap();
    assert!(preview.iter().all(|p| !p.contains("word")), "{preview:?}");
    let r = admin::apply_user(s, &create).await.unwrap();
    assert!(r.error.is_none(), "{:?}", r.error);
    let scram = scalar(s, "SELECT current_setting('password_encryption')").await;
    let stored = s
        .query(
            "SELECT rolpassword::text FROM pg_authid WHERE rolname = $1",
            &[Value::Text(role.clone())],
        )
        .await
        .ok()
        .and_then(|rs| rs.rows.first().and_then(|r| r.first()).and_then(cell_text));
    match (scram.as_deref(), stored) {
        (Some("scram-sha-256"), Some(stored)) => {
            let salt = stored.split(['$', ':']).nth(2).unwrap();
            let salt: [u8; 16] = base64::engine::general_purpose::STANDARD
                .decode(salt)
                .unwrap()
                .try_into()
                .unwrap();
            assert_eq!(stored, scram_sha_256_verifier(secret, &salt));
        }
        other => println!("note: verifier not checked ({other:?})"),
    }
    // Logging in with it (meaningful only where pg_hba asks for a password).
    let mut as_role = profile.clone();
    as_role.user = Some(role.clone());
    let wrong = open_profile(&as_role, Some("not it")).await;
    if wrong.is_err() {
        let right = open_profile(&as_role, Some(secret)).await.expect("login");
        right.close().await;
    } else {
        println!("note: the server trusts local logins; password login not checked");
    }

    exec(s, &format!("DROP ROLE \"{role}\"")).await;
    exec(s, &format!("DROP SCHEMA \"{schema}\" CASCADE")).await;
}

#[tokio::test]
async fn fix_obj_mysql() {
    let Ok(url) = std::env::var("TABLER_TEST_MYSQL_URL") else {
        println!("skipped: TABLER_TEST_MYSQL_URL unset");
        return;
    };
    for (i, no_backslash_escapes) in [false, true].into_iter().enumerate() {
        let (mut profile, password) = profile_from_url(&url, Driver::Mysql);
        if no_backslash_escapes {
            profile.options.startup_sql = Some(
                "SET SESSION sql_mode = CONCAT(@@SESSION.sql_mode, ',NO_BACKSLASH_ESCAPES')".into(),
            );
        }
        let session = open_profile(&profile, password.as_deref())
            .await
            .expect("connect");
        let s = session.as_ref();
        assert_eq!(
            admin::mysql_literal_mode(s).await.unwrap(),
            if no_backslash_escapes {
                MysqlLiteralMode::NoBackslashEscapes
            } else {
                MysqlLiteralMode::BackslashEscapes
            }
        );
        let id = suffix();
        let db = format!("fix_obj_{id}_{i}");
        let user = format!("fix_obj_{id}_{i}");
        let t = |n: &str| TableRef::new(Some(&db), n);
        for sql in [
            format!("CREATE DATABASE `{db}`"),
            format!("CREATE TABLE `{db}`.parent (id int PRIMARY KEY)"),
            format!(
                "CREATE TABLE `{db}`.child (id int PRIMARY KEY, pid int, \
                 FOREIGN KEY (pid) REFERENCES `{db}`.parent (id))"
            ),
            format!("INSERT INTO `{db}`.parent VALUES (1)"),
            format!("INSERT INTO `{db}`.child VALUES (1, 1)"),
        ] {
            exec(s, &sql).await;
        }

        // A comment with a backslash and a quote reads back as written in either mode.
        let comment = r"C:\temp it's";
        run(
            s,
            ObjectOp::SetComment {
                target: table(&t("parent"), TableKind::Table),
                comment: Some(comment.into()),
            },
        )
        .await;
        assert_eq!(
            column(
                s,
                "SELECT TABLE_COMMENT FROM information_schema.TABLES \
                 WHERE TABLE_SCHEMA = ? AND TABLE_NAME = 'parent'",
                &[Value::Text(db.clone())]
            )
            .await,
            [Some(comment.to_string())]
        );

        // A database-level grant on a name with `_` covers that database only, and the editor
        // reads it back under its real name.
        let secret = r"a\b'c";
        let state = UserState {
            name: user.clone(),
            host: Some("%".into()),
            limits: Some(Default::default()),
            database_privileges: vec![privs(&db, &["SELECT"])],
            ..UserState::default()
        };
        let create = UserChange {
            original: None,
            edited: Some(state.clone()),
            password: Some(secret.into()),
        };
        let r = admin::apply_user(s, &create).await.unwrap();
        assert!(r.error.is_none(), "{:?}", r.error);
        assert!(r.executed.iter().all(|p| !p.contains("b'c")));
        let params = [Value::Text(user.clone()), Value::Text("%".into())];
        let stored = column(
            s,
            "SELECT Db FROM mysql.db WHERE User = ? AND Host = ?",
            &params,
        )
        .await;
        assert_eq!(stored, [Some(mysql_db_pattern(&db))]);
        let reference = UserRef {
            name: user.clone(),
            host: Some("%".into()),
        };
        let loaded = admin::load_user(s, &reference).await.unwrap().state;
        assert_eq!(loaded.database_privileges, [privs(&db, &["SELECT"])]);

        // The password is stored as typed: logging in with it works, another does not.
        let mut as_user = profile.clone();
        as_user.user = Some(user.clone());
        as_user.options.startup_sql = None;
        as_user.database = None;
        match open_profile(&as_user, Some(secret)).await {
            Ok(login) => {
                login.ping().await.expect("ping");
                login.close().await;
                assert!(open_profile(&as_user, Some(r"a\\b'c")).await.is_err());
            }
            Err(e) => println!("note: login not checked ({e})"),
        }

        // A grant stored unescaped (a wildcard pattern) is revoked under its stored pattern.
        exec(s, &format!("GRANT INSERT ON `{db}%`.* TO '{user}'@'%'")).await;
        let loaded = admin::load_user(s, &reference).await.unwrap().state;
        assert!(
            loaded
                .database_privileges
                .contains(&privs(&format!("{db}%"), &["INSERT"])),
            "{:?}",
            loaded.database_privileges
        );
        let mut edited = loaded.clone();
        edited.database_privileges.clear();
        let revoke = UserChange {
            original: Some(loaded),
            edited: Some(edited),
            password: None,
        };
        let r = admin::apply_user(s, &revoke).await.unwrap();
        assert!(r.error.is_none(), "{:?} {:?}", r.error, r.executed);
        assert!(
            column(
                s,
                "SELECT Db FROM mysql.db WHERE User = ? AND Host = ?",
                &params
            )
            .await
            .is_empty()
        );

        // Dropping a parent with its child in one statement, foreign-key checks on.
        run(
            s,
            ObjectOp::DropTables {
                tables: vec![
                    table(&t("parent"), TableKind::Table),
                    table(&t("child"), TableKind::Table),
                ],
                cascade: false,
                ignore_foreign_keys: false,
            },
        )
        .await;

        exec(s, &format!("DROP USER '{user}'@'%'")).await;
        exec(s, &format!("DROP DATABASE `{db}`")).await;
        session.close().await;
    }
}

#[tokio::test]
async fn fix_obj_sqlite() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fix_obj.sqlite");
    let profile = ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "fix-obj-sqlite".into(),
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
    let session = open_profile(&profile, None).await.expect("open");
    let s = session.as_ref();
    let t = |n: &str| TableRef::new(Some("main"), n);
    for sql in [
        "CREATE TABLE dept (id INTEGER PRIMARY KEY)",
        "CREATE TABLE member (id INTEGER PRIMARY KEY, dept INTEGER REFERENCES Dept(id))",
        "INSERT INTO dept VALUES (1)",
        "INSERT INTO member VALUES (1, 1)",
        "CREATE TABLE emp (id INTEGER PRIMARY KEY, boss INTEGER REFERENCES emp(id), name TEXT)",
        "CREATE INDEX emp_name ON emp (name)",
        "CREATE TRIGGER emp_t AFTER INSERT ON emp BEGIN SELECT 1; END",
        "INSERT INTO emp VALUES (1, NULL, 'a'), (2, 1, 'b')",
    ] {
        exec(s, sql).await;
    }

    // Parent selected first: the child is emptied / dropped first all the same.
    run(
        s,
        ObjectOp::TruncateTables {
            tables: vec![t("dept"), t("member")],
            cascade: false,
            restart_identity: false,
            ignore_foreign_keys: false,
        },
    )
    .await;
    exec(s, "INSERT INTO dept VALUES (1)").await;
    exec(s, "INSERT INTO member VALUES (1, 1)").await;
    run(
        s,
        ObjectOp::DropTables {
            tables: vec![
                table(&t("dept"), TableKind::Table),
                table(&t("member"), TableKind::Table),
            ],
            cascade: false,
            ignore_foreign_keys: false,
        },
    )
    .await;

    // A clone references itself and has its own index; the original can then be dropped.
    let clone = ObjectOp::CloneTable {
        source: t("emp"),
        new_name: "emp2".into(),
        with_data: true,
    };
    let plan = plan_op(s, &clone).await.unwrap();
    assert_eq!(plan.warnings, ["Triggers are not copied to the clone"]);
    run(s, clone).await;
    assert_eq!(
        column(
            s,
            "SELECT \"table\" FROM pragma_foreign_key_list('emp2')",
            &[]
        )
        .await,
        [Some("emp2".to_string())]
    );
    assert_eq!(
        column(
            s,
            "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'emp2'",
            &[]
        )
        .await,
        [Some("emp2_name".to_string())]
    );
    run(
        s,
        ObjectOp::DropTables {
            tables: vec![table(&t("emp"), TableKind::Table)],
            cascade: false,
            ignore_foreign_keys: false,
        },
    )
    .await;
    exec(s, "INSERT INTO emp2 VALUES (3, 2, 'c')").await;
    assert!(
        s.execute("INSERT INTO emp2 VALUES (4, 99, 'd')", &[])
            .await
            .is_err(),
        "the clone's foreign key points at the clone"
    );
    session.close().await;
}
