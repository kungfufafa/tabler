//! The shared integration-test helpers (`tests/common`) themselves: test URLs become the
//! profiles they name, and a test whose variable is unset is skipped. Needs no server.

mod common;

use tabler_lib::types::profile::{Driver, SslMode};

#[test]
fn a_postgres_url_names_every_field() {
    let (p, password) =
        common::profile_from_url("postgres://app%40corp:p%3Ass@db.example:5433/tabler_dev?x=1");
    assert_eq!(p.driver, Driver::Postgres);
    assert_eq!(p.user.as_deref(), Some("app@corp"));
    assert_eq!(password.as_deref(), Some("p:ss"));
    assert_eq!(p.host.as_deref(), Some("db.example"));
    assert_eq!(p.port, Some(5433));
    assert_eq!(p.database.as_deref(), Some("tabler_dev"));
    // The rest keeps the profile defaults.
    assert_eq!(p.ssl, SslMode::default());
    assert_eq!(p.options.connect_timeout_secs, Some(10));
    assert!(p.ssh.is_none());
}

#[test]
fn the_parts_of_a_url_are_optional() {
    let (p, password) = common::profile_from_url("postgresql://127.0.0.1");
    assert_eq!(p.driver, Driver::Postgres);
    assert_eq!((p.user, password), (None, None));
    assert_eq!((p.host.as_deref(), p.port), (Some("127.0.0.1"), None));
    assert_eq!(p.database, None);

    let (p, password) = common::profile_from_url("mysql://root@[::1]:3306/");
    assert_eq!(p.driver, Driver::Mysql);
    assert_eq!(p.user.as_deref(), Some("root"));
    assert_eq!(password, None);
    assert_eq!((p.host.as_deref(), p.port), (Some("::1"), Some(3306)));
    assert_eq!(p.database, None);

    let (p, _) = common::profile_from_url("mysql://u:@[::1]?ssl=0");
    assert_eq!((p.host.as_deref(), p.port), (Some("::1"), None));
}

#[test]
fn a_sqlite_url_is_its_file() {
    let (p, password) = common::profile_from_url("sqlite://C:/data/app.db");
    assert_eq!(p.driver, Driver::Sqlite);
    assert_eq!(p.sqlite_path.as_deref(), Some("C:/data/app.db"));
    assert_eq!(password, None);
    let (p, _) = common::profile_from_url("sqlite:///tmp/app.db");
    assert_eq!(p.sqlite_path.as_deref(), Some("/tmp/app.db"));
}

#[tokio::test]
async fn a_test_without_its_variable_is_skipped() {
    let var = "TABLER_TEST_COMMON_HELPERS_NEVER_SET_URL";
    assert!(std::env::var_os(var).is_none());
    assert!(common::url(var).is_none());
    assert!(common::open(var).await.is_none());
}
