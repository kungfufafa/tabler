//! Generate Diagram…'s catalog query against real servers, through `tabler_lib::diagram`.
//! PostgreSQL / MySQL are gated on `TABLER_TEST_PG_URL` / `TABLER_TEST_MYSQL_URL` (each test
//! prints a skip notice and passes when unset); every object lives in a schema / database named
//! `diagram_<random>` or `fx_diag_<random>`, dropped at the end. SQLite runs on a temporary file.

use std::sync::Arc;

use tabler_lib::diagram::schema_relations;
use tabler_lib::drivers::{ConnectTarget, DbSession, connect};
use tabler_lib::types::diagram::DiagramSchema;
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
use tabler_lib::types::schema::{TableKind, TableRef};
use tabler_lib::types::value::Value;

fn profile(driver: Driver) -> ConnectionProfile {
    ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "diagram-test".into(),
        color: None,
        group: None,
        driver,
        host: None,
        port: None,
        user: None,
        database: None,
        ssl: SslMode::Disable,
        ssl_ca_path: None,
        sqlite_path: None,
        ssh: None,
        options: ProfileOptions {
            sqlite_foreign_keys: true,
            ..ProfileOptions::default()
        },
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    }
}

/// `scheme://user[:pass]@host[:port]/db` → profile + password.
fn profile_from_url(url: &str, driver: Driver) -> (ConnectionProfile, Option<String>) {
    let rest = url.split_once("://").map(|(_, r)| r).expect("a URL");
    let (auth, hostdb) = match rest.rsplit_once('@') {
        Some((a, h)) => (Some(a), h),
        None => (None, rest),
    };
    let (user, password) = match auth.map(|a| a.split_once(':')) {
        Some(Some((u, p))) => (Some(u.to_string()), Some(p.to_string())),
        Some(None) => (auth.map(str::to_string), None),
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
    let mut p = profile(driver);
    p.host = Some(host);
    p.port = port;
    p.user = user;
    p.database = db.filter(|d| !d.is_empty());
    if driver == Driver::Postgres {
        p.ssl = SslMode::Prefer;
    }
    (p, password)
}

async fn open(var: &str, driver: Driver) -> Option<Arc<dyn DbSession>> {
    let Ok(url) = std::env::var(var) else {
        println!("skipped: {var} unset");
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

fn suffix() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

async fn exec(s: &Arc<dyn DbSession>, sql: &str) {
    s.execute(sql, &[])
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

fn table<'a>(d: &'a DiagramSchema, name: &str) -> &'a tabler_lib::types::diagram::DiagramTable {
    d.tables
        .iter()
        .find(|t| t.table.name == name)
        .unwrap_or_else(|| panic!("table {name} in {:?}", d.tables))
}

fn rel<'a>(
    d: &'a DiagramSchema,
    from: &str,
) -> Vec<&'a tabler_lib::types::diagram::DiagramRelation> {
    d.relations.iter().filter(|r| r.from.name == from).collect()
}

/// The checks every dialect shares (the same shop schema in each).
fn check_shop(d: &DiagramSchema, schema: &str) {
    let names: Vec<&str> = d.tables.iter().map(|t| t.table.name.as_str()).collect();
    assert_eq!(
        names,
        ["lines", "orders", "shipments", "users", "v_users"],
        "sorted tables and views"
    );
    let users = table(d, "users");
    assert_eq!(users.kind, TableKind::Table);
    let cols: Vec<(&str, bool, bool)> = users
        .columns
        .iter()
        .map(|c| (c.name.as_str(), c.is_pk, c.nullable))
        .collect();
    assert_eq!(
        cols,
        [
            ("id", true, false),
            ("email", false, false),
            ("manager_id", false, true)
        ]
    );
    assert!(!users.columns[1].data_type.is_empty());
    assert_eq!(table(d, "v_users").kind, TableKind::View);

    let orders = rel(d, "orders");
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].from_columns, ["user_id"]);
    assert_eq!(orders[0].to, TableRef::new(Some(schema), "users"));
    assert_eq!(orders[0].to_columns, ["id"]);
    assert_eq!(orders[0].on_delete, "CASCADE");

    // Composite key, in key order.
    let lines = table(d, "lines");
    assert!(lines.columns.iter().filter(|c| c.is_pk).count() == 2);
    let ship = rel(d, "shipments");
    assert_eq!(ship.len(), 1);
    assert_eq!(ship[0].from_columns, ["order_id", "line_no"]);
    assert_eq!(ship[0].to.name, "lines");
    assert_eq!(ship[0].to_columns, ["order_id", "line_no"]);
    assert_eq!(ship[0].on_delete, "SET NULL");

    // Self reference.
    let me = rel(d, "users");
    assert_eq!(me.len(), 1);
    assert_eq!(me[0].to.name, "users");
    assert_eq!(me[0].from_columns, ["manager_id"]);
}

#[tokio::test]
async fn postgres_schema_relations() {
    let Some(s) = open("TABLER_TEST_PG_URL", Driver::Postgres).await else {
        return;
    };
    let schema = format!("diagram_{}", suffix());
    exec(&s, &format!("CREATE SCHEMA {schema}")).await;
    let result = async {
        for sql in [
            format!(
                "CREATE TABLE {schema}.users (id bigint PRIMARY KEY, email text NOT NULL, \
                 manager_id bigint REFERENCES {schema}.users (id))"
            ),
            format!(
                "CREATE TABLE {schema}.orders (id bigint PRIMARY KEY, \
                 user_id bigint NOT NULL REFERENCES {schema}.users (id) ON DELETE CASCADE)"
            ),
            format!(
                "CREATE TABLE {schema}.lines (line_no int, order_id bigint REFERENCES {schema}.orders, \
                 PRIMARY KEY (order_id, line_no))"
            ),
            format!(
                "CREATE TABLE {schema}.shipments (id serial PRIMARY KEY, order_id bigint, line_no int, \
                 FOREIGN KEY (order_id, line_no) REFERENCES {schema}.lines (order_id, line_no) \
                 ON DELETE SET NULL)"
            ),
            format!("CREATE VIEW {schema}.v_users AS SELECT id, email FROM {schema}.users"),
        ] {
            exec(&s, &sql).await;
        }
        schema_relations(s.as_ref(), &schema).await
    }
    .await;
    exec(&s, &format!("DROP SCHEMA {schema} CASCADE")).await;
    let d = result.expect("schema_relations");
    check_shop(&d, &schema);
    // lines → orders, with the PK order of `lines` (order_id first).
    let lines = rel(&d, "lines");
    assert_eq!(lines[0].from_columns, ["order_id"]);
    assert_eq!(lines[0].on_delete, "NO ACTION");
    assert_eq!(table(&d, "users").row_estimate, None);
    // An unknown schema is empty, not an error.
    let none = schema_relations(s.as_ref(), "diagram_missing")
        .await
        .unwrap();
    assert!(none.tables.is_empty() && none.relations.is_empty());
}

#[tokio::test]
async fn mysql_schema_relations() {
    let Some(s) = open("TABLER_TEST_MYSQL_URL", Driver::Mysql).await else {
        return;
    };
    let db = format!("diagram_{}", suffix());
    exec(&s, &format!("CREATE DATABASE `{db}`")).await;
    let result = async {
        for sql in [
            format!(
                "CREATE TABLE `{db}`.users (id bigint PRIMARY KEY, email varchar(255) NOT NULL, \
                 manager_id bigint NULL, FOREIGN KEY (manager_id) REFERENCES `{db}`.users (id))"
            ),
            format!(
                "CREATE TABLE `{db}`.orders (id bigint PRIMARY KEY, user_id bigint NOT NULL, \
                 FOREIGN KEY (user_id) REFERENCES `{db}`.users (id) ON DELETE CASCADE)"
            ),
            format!(
                "CREATE TABLE `{db}`.lines (line_no int, order_id bigint, PRIMARY KEY (order_id, line_no), \
                 FOREIGN KEY (order_id) REFERENCES `{db}`.orders (id))"
            ),
            format!(
                "CREATE TABLE `{db}`.shipments (id int AUTO_INCREMENT PRIMARY KEY, order_id bigint, \
                 line_no int, FOREIGN KEY (order_id, line_no) REFERENCES `{db}`.lines (order_id, line_no) \
                 ON DELETE SET NULL)"
            ),
            format!("CREATE VIEW `{db}`.v_users AS SELECT id, email FROM `{db}`.users"),
        ] {
            exec(&s, &sql).await;
        }
        schema_relations(s.as_ref(), &db).await
    }
    .await;
    exec(&s, &format!("DROP DATABASE `{db}`")).await;
    let d = result.expect("schema_relations");
    check_shop(&d, &db);
    assert!(
        table(&d, "users").columns[1]
            .data_type
            .starts_with("varchar")
    );
}

#[tokio::test]
async fn sqlite_schema_relations() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut p = profile(Driver::Sqlite);
    p.sqlite_path = Some(
        dir.path()
            .join("diagram.sqlite")
            .to_string_lossy()
            .into_owned(),
    );
    let s = connect(ConnectTarget {
        profile: &p,
        password: None,
        host_override: None,
    })
    .await
    .expect("open");
    for sql in [
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL, \
         manager_id INTEGER REFERENCES users)",
        "CREATE TABLE orders (id INTEGER PRIMARY KEY, \
         user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE)",
        "CREATE TABLE lines (line_no INTEGER, order_id INTEGER REFERENCES orders, \
         PRIMARY KEY (order_id, line_no))",
        "CREATE TABLE shipments (id INTEGER PRIMARY KEY, order_id INTEGER, line_no INTEGER, \
         FOREIGN KEY (order_id, line_no) REFERENCES lines ON DELETE SET NULL)",
        "CREATE VIEW v_users AS SELECT id, email FROM users",
    ] {
        exec(&s, sql).await;
    }
    let d = schema_relations(s.as_ref(), "main")
        .await
        .expect("schema_relations");
    check_shop(&d, "main");
    // `REFERENCES users` without columns resolves to the primary key; keys get stable names.
    assert_eq!(rel(&d, "users")[0].name, "users_fk0");
    // An attached database that does not exist is an error, not an empty diagram.
    assert!(schema_relations(s.as_ref(), "nope").await.is_err());
}

/// A partitioned table is one box: its partitions are left out, and so are the foreign keys
/// PostgreSQL clones per partition (of a referenced or a referencing partitioned table).
#[tokio::test]
async fn postgres_partitions_are_one_table() {
    let Some(s) = open("TABLER_TEST_PG_URL", Driver::Postgres).await else {
        return;
    };
    let schema = format!("fx_diag_{}", suffix());
    exec(&s, &format!("CREATE SCHEMA {schema}")).await;
    let result = async {
        for sql in [
            format!(
                "CREATE TABLE {schema}.parent (id int, region text, PRIMARY KEY (id, region)) \
                 PARTITION BY LIST (region)"
            ),
            format!(
                "CREATE TABLE {schema}.parent_1 PARTITION OF {schema}.parent FOR VALUES IN ('a')"
            ),
            format!(
                "CREATE TABLE {schema}.parent_2 PARTITION OF {schema}.parent FOR VALUES IN ('b')"
            ),
            format!(
                "CREATE TABLE {schema}.parent_3 PARTITION OF {schema}.parent FOR VALUES IN ('c') \
                 PARTITION BY LIST (id)"
            ),
            format!("CREATE TABLE {schema}.parent_3a PARTITION OF {schema}.parent_3 DEFAULT"),
            format!(
                "CREATE TABLE {schema}.child (id int PRIMARY KEY, pid int, region text, \
                 FOREIGN KEY (pid, region) REFERENCES {schema}.parent)"
            ),
            format!(
                "CREATE TABLE {schema}.pchild (id int, pid int, region text, \
                 PRIMARY KEY (id, region), FOREIGN KEY (pid, region) REFERENCES {schema}.parent) \
                 PARTITION BY LIST (region)"
            ),
            format!(
                "CREATE TABLE {schema}.pchild_1 PARTITION OF {schema}.pchild FOR VALUES IN ('a')"
            ),
        ] {
            exec(&s, &sql).await;
        }
        schema_relations(s.as_ref(), &schema).await
    }
    .await;
    exec(&s, &format!("DROP SCHEMA {schema} CASCADE")).await;
    let d = result.expect("schema_relations");
    let names: Vec<&str> = d.tables.iter().map(|t| t.table.name.as_str()).collect();
    assert_eq!(names, ["child", "parent", "pchild"]);
    assert_eq!(table(&d, "parent").kind, TableKind::Partitioned);
    assert_eq!(table(&d, "pchild").kind, TableKind::Partitioned);
    let edges: Vec<(&str, &str, &str)> = d
        .relations
        .iter()
        .map(|r| (r.from.name.as_str(), r.name.as_str(), r.to.name.as_str()))
        .collect();
    assert_eq!(
        edges,
        [
            ("child", "child_pid_region_fkey", "parent"),
            ("pchild", "pchild_pid_region_fkey", "parent"),
        ]
    );
    assert_eq!(d.relations[0].to_columns, ["id", "region"]);
}

/// SQLite reports a referenced table and column as `REFERENCES` spells them; they resolve to the
/// table's own spelling.
#[tokio::test]
async fn sqlite_references_in_another_case() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut p = profile(Driver::Sqlite);
    p.sqlite_path = Some(
        dir.path()
            .join("fx_diag.sqlite")
            .to_string_lossy()
            .into_owned(),
    );
    let s = connect(ConnectTarget {
        profile: &p,
        password: None,
        host_override: None,
    })
    .await
    .expect("open");
    for sql in [
        "CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT)",
        "CREATE TABLE albums (id INTEGER PRIMARY KEY, artist INTEGER, \
         FOREIGN KEY (ARTIST) REFERENCES Artists (ID))",
        "CREATE TABLE tracks (id INTEGER PRIMARY KEY, album INTEGER REFERENCES ALBUMS)",
    ] {
        exec(&s, sql).await;
    }
    let d = schema_relations(s.as_ref(), "main")
        .await
        .expect("schema_relations");
    let albums = rel(&d, "albums");
    assert_eq!(albums[0].from_columns, ["artist"]);
    assert_eq!(albums[0].to, TableRef::new(Some("main"), "artists"));
    assert_eq!(albums[0].to_columns, ["id"]);
    let tracks = rel(&d, "tracks");
    assert_eq!(tracks[0].to, TableRef::new(Some("main"), "albums"));
    assert_eq!(tracks[0].to_columns, ["id"]);
}

/// MySQL with `lower_case_table_names` 1 or 2 (the macOS default) reports the referenced schema
/// and table as `REFERENCES` spells them. Skipped on case-sensitive servers (0), which refuse
/// such a reference.
#[tokio::test]
async fn mysql_references_in_another_case() {
    let Some(s) = open("TABLER_TEST_MYSQL_URL", Driver::Mysql).await else {
        return;
    };
    let lctn = s
        .query("SELECT @@lower_case_table_names", &[])
        .await
        .expect("lower_case_table_names")
        .rows;
    let case_sensitive = match lctn.first().and_then(|r| r.first()) {
        Some(Value::Int(n)) => *n == 0,
        Some(Value::Text(t) | Value::BigInt(t) | Value::Decimal(t)) => t.trim() == "0",
        other => panic!("lower_case_table_names: {other:?}"),
    };
    if case_sensitive {
        println!("skipped: lower_case_table_names = 0");
        return;
    }
    let db = format!("fx_diag_{}", suffix());
    exec(&s, &format!("CREATE DATABASE `{db}`")).await;
    let upper = db.to_ascii_uppercase();
    let result = async {
        for sql in [
            format!("CREATE TABLE `{db}`.Artists (id int PRIMARY KEY)"),
            format!(
                "CREATE TABLE `{db}`.Albums (id int PRIMARY KEY, artist int, \
                 FOREIGN KEY (artist) REFERENCES `{upper}`.ARTISTS (id))"
            ),
        ] {
            exec(&s, &sql).await;
        }
        schema_relations(s.as_ref(), &db).await
    }
    .await;
    exec(&s, &format!("DROP DATABASE `{db}`")).await;
    let d = result.expect("schema_relations");
    // `lower_case_table_names = 1` stores the names in lowercase, 2 as created.
    let name = |n: &str| {
        d.tables
            .iter()
            .map(|t| t.table.name.clone())
            .find(|t| t.eq_ignore_ascii_case(n))
            .unwrap_or_else(|| panic!("{n} in {:?}", d.tables))
    };
    let albums = rel(&d, &name("albums"));
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].to, TableRef::new(Some(&db), &name("artists")));
    assert_eq!(albums[0].to_columns, ["id"]);
}
