//! Test helpers of the lane: a client of `FakePg` for the hooks, a session on it through the
//! real `PgSession::open` (which picks this lane's hooks), and the catalog rows the core
//! `table_meta` reads.

use tokio_postgres::Client;

use crate::drivers::postgres::PgSession;
use crate::drivers::postgres::testing::{Cell, Col, FakePg};
use crate::drivers::{ConnectTarget, SqlTrace};
use crate::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};

pub(super) const COCKROACH: &str =
    "CockroachDB CCL v24.2.4 (x86_64-pc-linux-gnu, built 2024/10/14, go1.22.5)";
pub(super) const GREENPLUM6: &str = "PostgreSQL 9.4.26 (Greenplum Database 6.27.1 build \
                                     commit:1d9f0a3) on x86_64-unknown-linux-gnu, compiled by gcc";
pub(super) const GREENPLUM7: &str =
    "PostgreSQL 12.12 (Greenplum Database 7.1.0 build commit:e7c2b1f) on x86_64";
pub(super) const CLOUDBERRY: &str = "PostgreSQL 14.4 (Apache Cloudberry 2.0.0-incubating build 1)";
pub(super) const REDSHIFT: &str = "PostgreSQL 8.0.2 on i686-pc-linux-gnu, compiled by GCC gcc \
                                   (GCC) 3.4.2 20041017 (Red Hat 3.4.2-6.fc3), Redshift 1.0.77467";

/// A client of `pg` and a trace for `Traced`.
pub(super) async fn client(pg: &FakePg) -> (Client, SqlTrace) {
    crate::testkit::install_crypto();
    let mut config = tokio_postgres::Config::new();
    config
        .host("127.0.0.1")
        .port(pg.port())
        .user("u")
        .password("pw")
        .dbname("d");
    let (client, connection) = config.connect(tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(connection);
    (client, SqlTrace::default())
}

/// A profile of the fake server on `port`.
pub(super) fn profile(port: u16) -> ConnectionProfile {
    ConnectionProfile {
        id: uuid::Uuid::nil(),
        name: "fake".into(),
        color: None,
        group: None,
        driver: Driver::Postgres,
        host: Some("127.0.0.1".into()),
        port: Some(port),
        user: Some("tabler".into()),
        database: None,
        ssl: SslMode::Disable,
        ssl_ca_path: None,
        sqlite_path: None,
        ssh: None,
        options: ProfileOptions::default(),
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    }
}

/// A session of `profile` through the driver's own open, which picks the engine's hooks.
pub(super) async fn session(profile: &ConnectionProfile) -> PgSession {
    crate::testkit::install_crypto();
    PgSession::open(ConnectTarget {
        profile,
        password: Some("pw"),
        host_override: None,
    })
    .await
    .unwrap()
}

/// The statements `pg` received whose text contains `part`.
pub(super) fn sent(pg: &FakePg, part: &str) -> Vec<String> {
    pg.queries()
        .into_iter()
        .filter(|q| q.contains(part))
        .collect()
}

/// One column of the core `table_meta` column query (`introspect::table_meta`), as a server of
/// the PostgreSQL catalog answers it.
pub(super) struct CoreColumn<'a> {
    pub attnum: i64,
    pub name: &'a str,
    pub data_type: &'a str,
    pub typname: &'a str,
    pub nullable: bool,
    pub default: Option<&'a str>,
    pub typmod: i64,
}

/// `FakePg` answering the core `table_meta` queries for a table of `columns` keyed by `pk`.
pub(super) fn core_table(pg: FakePg, relkind: &str, columns: &[CoreColumn], pk: &[&str]) -> FakePg {
    let text = |s: &str| Cell::text(s);
    let opt = |s: Option<&str>| s.map_or(Cell::Null, Cell::text);
    let rows = columns
        .iter()
        .map(|c| {
            vec![
                Cell::Int(c.attnum),
                text(c.name),
                text(c.data_type),
                text(c.typname),
                text("b"),
                text(if c.typname == "varchar" { "S" } else { "N" }),
                Cell::Null,
                Cell::Null,
                Cell::Null,
                Cell::Null,
                Cell::Bool(c.nullable),
                opt(c.default),
                Cell::Null,
                Cell::Bool(false),
                Cell::Null,
                Cell::Null,
                Cell::Null,
                Cell::Null,
                Cell::Int(c.typmod),
                Cell::Int(-1),
                Cell::Null,
            ]
        })
        .collect();
    let col_types = vec![
        Col::int4("attnum"),
        Col::text("attname"),
        Col::text("format_type"),
        Col::text("typname"),
        Col::text("typtype"),
        Col::text("typcategory"),
        Col::text("btypname"),
        Col::text("btyptype"),
        Col::text("btypcategory"),
        Col::text("etypname"),
        Col::bool("nullable"),
        Col::text("default"),
        Col::text("identity"),
        Col::bool("generated"),
        Col::text("comment"),
        Col::text("enum"),
        Col::text("elem_enum"),
        Col::text("collation"),
        Col::int4("atttypmod"),
        Col::int4("domain_typmod"),
        Col::text("base_type"),
    ];
    let pk_rows = pk
        .iter()
        .enumerate()
        .map(|(i, n)| vec![text(n), Cell::Int(i as i64 + 1)])
        .collect();
    pg.rows(
        "SELECT c.relkind::text, obj_description",
        vec![
            Col::text("relkind"),
            Col::text("comment"),
            Col::int8("reltuples"),
        ],
        vec![vec![text(relkind), Cell::Null, Cell::Null]],
    )
    .rows(
        "WITH ORDINALITY AS k(attnum, ord) \
             JOIN pg_attribute a ON a.attrelid = c.oid",
        vec![Col::text("attname"), Col::int4("ord")],
        pk_rows,
    )
    .rows(
        "format_type(a.atttypid, a.atttypmod)::text",
        col_types,
        rows,
    )
}
