//! The drivers this build has, and the one place that hands a profile to its driver. Every
//! driver beyond PostgreSQL, MySQL and SQLite sits behind a cargo feature (`drv-*`); for each one
//! a module of shim functions below calls into its `entry` module when the feature is on and
//! answers [`not_built`] (or has nothing to check) when it is off, so no other code needs a `cfg`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Map, Value};

use crate::drivers::native::{CallKind, unknown_call};
use crate::drivers::session::{Session, not_sql};
use crate::drivers::{ConnectTarget, DbSession, mysql, postgres, sqlite};
use crate::error::AppError;
use crate::types::driver::{
    DriverDescriptor, KINDS, KindDescriptor, KindId, ProfileField, SessionKind,
};
use crate::types::profile::{ConnectionProfile, Driver, PasswordMode};

/// The lane features, and whether this build has each.
const LANE_FEATURES: [(&str, bool); 14] = [
    ("drv-pg-flavors", cfg!(feature = "drv-pg-flavors")),
    ("drv-libsql", cfg!(feature = "drv-libsql")),
    ("drv-d1", cfg!(feature = "drv-d1")),
    ("drv-duckdb", cfg!(feature = "drv-duckdb")),
    ("drv-sqlserver", cfg!(feature = "drv-sqlserver")),
    ("drv-clickhouse", cfg!(feature = "drv-clickhouse")),
    ("drv-snowflake", cfg!(feature = "drv-snowflake")),
    ("drv-bigquery", cfg!(feature = "drv-bigquery")),
    ("drv-vertica", cfg!(feature = "drv-vertica")),
    ("drv-cassandra", cfg!(feature = "drv-cassandra")),
    ("drv-redis", cfg!(feature = "drv-redis")),
    ("drv-mongodb", cfg!(feature = "drv-mongodb")),
    ("drv-elasticsearch", cfg!(feature = "drv-elasticsearch")),
    ("drv-dynamodb", cfg!(feature = "drv-dynamodb")),
];

fn feature_built(name: &str) -> bool {
    LANE_FEATURES.iter().any(|(f, on)| *f == name && *on)
}

/// Whether this build has `driver`.
pub fn built(driver: Driver) -> bool {
    driver.descriptor().cargo_feature.is_none_or(feature_built)
}

/// The drivers this build has (the core three, plus the lanes built in).
pub fn built_drivers() -> Vec<Driver> {
    Driver::ALL.into_iter().filter(|d| built(*d)).collect()
}

/// Whether this build has the database `kind`: its driver, and its flavor's feature (CockroachDB
/// is the PostgreSQL driver plus `drv-pg-flavors`).
fn kind_built(kind: &KindDescriptor) -> bool {
    built(kind.driver) && kind.cargo_feature.is_none_or(feature_built)
}

/// The databases of the new-connection picker this build can connect to, in the picker's order.
pub fn built_kinds() -> Vec<KindId> {
    KINDS
        .iter()
        .filter(|k| kind_built(k))
        .map(|k| k.kind)
        .collect()
}

/// The error for a profile of a driver this build does not have (a newer or fuller Tabler saved
/// it).
pub fn not_built(driver: Driver) -> AppError {
    not_built_named(driver.label())
}

fn not_built_named(label: &str) -> AppError {
    AppError::Unsupported(format!("{label} is not built into this Tabler"))
}

/// Refuses a profile of a database this build does not have: a driver without its feature, or a
/// flavor without its own (a Redshift profile must not open as plain PostgreSQL, whose builders
/// do not fit it). Connects and tests call it before anything else of theirs: the bootstrap
/// script, the password command and the SSH tunnel would run for a connection that cannot open.
pub fn ensure_built(profile: &ConnectionProfile) -> Result<(), AppError> {
    // A driver's own kind has its label ("Redis"); a flavor names itself ("Amazon Redshift").
    let kind = profile.kind().descriptor();
    if kind_built(kind) {
        Ok(())
    } else {
        Err(not_built_named(kind.label))
    }
}

static UNATTENDED: AtomicBool = AtomicBool::new(false);

/// Marks this process as the MCP server (`tabler --mcp`), where nobody watches what runs: drivers
/// open their sessions hardened (`unattended`).
pub fn set_unattended() {
    UNATTENDED.store(true, Ordering::Relaxed);
}

pub fn unattended() -> bool {
    UNATTENDED.load(Ordering::Relaxed)
}

/// What a driver's call from the connection form gets (no session exists yet): the profile being
/// edited and its secret.
pub struct UtilInput<'a> {
    pub profile: &'a ConnectionProfile,
    pub secret: Option<&'a str>,
}

/// Opens the session of `target`'s profile: a SQL session, or a native store's for the drivers
/// that speak no SQL.
pub async fn open(target: ConnectTarget<'_>) -> Result<Session, AppError> {
    ensure_built(target.profile)?;
    match target.profile.driver {
        // A SQLite profile's password is its SQLCipher encryption key.
        Driver::Sqlite => Ok(Session::Sql(Arc::new(
            sqlite::LiteSession::open_with_key(target.profile, target.password).await?,
        ))),
        Driver::Postgres => postgres::connect(target).await.map(Session::Sql),
        Driver::Mysql => mysql::connect(target).await.map(Session::Sql),
        Driver::Sqlserver => sqlserver_lane::open(target).await,
        Driver::Libsql => libsql_lane::open(target).await,
        Driver::D1 => d1_lane::open(target).await,
        Driver::Duckdb => duckdb_lane::open(target).await,
        Driver::Clickhouse => clickhouse_lane::open(target).await,
        Driver::Snowflake => snowflake_lane::open(target).await,
        Driver::Bigquery => bigquery_lane::open(target).await,
        Driver::Vertica => vertica_lane::open(target).await,
        Driver::Cassandra => cassandra_lane::open(target).await,
        Driver::Redis => redis_lane::open(target).await,
        Driver::Mongodb => mongodb_lane::open(target).await,
        Driver::Elasticsearch => elasticsearch_lane::open(target).await,
        Driver::Dynamodb => dynamodb_lane::open(target).await,
    }
}

/// Opens a SQL session for `target`'s profile, for a caller that builds SQL. A store that speaks
/// no SQL is refused before anything is opened.
pub async fn connect(target: ConnectTarget<'_>) -> Result<Arc<dyn DbSession>, AppError> {
    ensure_built(target.profile)?;
    ensure_sql(target.profile)?;
    let driver = target.profile.driver;
    match open(target).await? {
        Session::Sql(session) => Ok(session),
        // A driver that opened the other kind of session: never used.
        native @ Session::Native(_) => {
            native.close().await;
            Err(not_sql(driver))
        }
    }
}

/// Refuses a profile of a store that speaks no SQL, for a caller that only takes SQL sessions
/// (`connect`, the MCP pool): before its bootstrap script, password command or tunnel run.
pub fn ensure_sql(profile: &ConnectionProfile) -> Result<(), AppError> {
    match profile.driver.descriptor().session {
        SessionKind::Native => Err(not_sql(profile.driver)),
        SessionKind::Sql => Ok(()),
    }
}

/// A call of the connection form, where no session exists yet (DuckDB's "New…", the D1 and
/// BigQuery pickers, the Snowflake probe). The core drivers have none.
pub async fn util(input: UtilInput<'_>, method: &str, args: Value) -> Result<Value, AppError> {
    ensure_built(input.profile)?;
    match input.profile.driver {
        driver @ (Driver::Postgres | Driver::Mysql | Driver::Sqlite) => {
            Err(unknown_call(driver, method))
        }
        Driver::Sqlserver => sqlserver_lane::util(input, method, args).await,
        Driver::Libsql => libsql_lane::util(input, method, args).await,
        Driver::D1 => d1_lane::util(input, method, args).await,
        Driver::Duckdb => duckdb_lane::util(input, method, args).await,
        Driver::Clickhouse => clickhouse_lane::util(input, method, args).await,
        Driver::Snowflake => snowflake_lane::util(input, method, args).await,
        Driver::Bigquery => bigquery_lane::util(input, method, args).await,
        Driver::Vertica => vertica_lane::util(input, method, args).await,
        Driver::Cassandra => cassandra_lane::util(input, method, args).await,
        Driver::Redis => redis_lane::util(input, method, args).await,
        Driver::Mongodb => mongodb_lane::util(input, method, args).await,
        Driver::Elasticsearch => elasticsearch_lane::util(input, method, args).await,
        Driver::Dynamodb => dynamodb_lane::util(input, method, args).await,
    }
}

/// Whether `util(method)` of `driver` only reads (a read-only profile may make it); anything
/// unknown, and every call of a driver this build lacks, changes something.
pub fn util_kind(driver: Driver, method: &str) -> CallKind {
    match driver {
        Driver::Postgres | Driver::Mysql | Driver::Sqlite => CallKind::Write,
        Driver::Sqlserver => sqlserver_lane::util_kind(method),
        Driver::Libsql => libsql_lane::util_kind(method),
        Driver::D1 => d1_lane::util_kind(method),
        Driver::Duckdb => duckdb_lane::util_kind(method),
        Driver::Clickhouse => clickhouse_lane::util_kind(method),
        Driver::Snowflake => snowflake_lane::util_kind(method),
        Driver::Bigquery => bigquery_lane::util_kind(method),
        Driver::Vertica => vertica_lane::util_kind(method),
        Driver::Cassandra => cassandra_lane::util_kind(method),
        Driver::Redis => redis_lane::util_kind(method),
        Driver::Mongodb => mongodb_lane::util_kind(method),
        Driver::Elasticsearch => elasticsearch_lane::util_kind(method),
        Driver::Dynamodb => dynamodb_lane::util_kind(method),
    }
}

/// The checks of a profile of a driver beyond the core three, before it is saved or tested: the
/// fields its descriptor requires, its required `driver_params` and no secret among them, its
/// password mode, then the driver's own checks (when it is built).
pub fn validate(profile: &ConnectionProfile) -> Result<(), AppError> {
    // The core profiles have their own checks (`commands::profiles::validate`): a MySQL socket
    // profile needs no host, which the descriptor cannot say.
    if profile.driver.is_core() {
        return Ok(());
    }
    let desc = profile.driver.descriptor();
    for field in desc.required {
        if !has_field(profile, *field) {
            return Err(AppError::InvalidInput(format!(
                "{} is required",
                field.label(&desc.labels)
            )));
        }
    }
    let params = profile.options.driver_params.as_ref().map(|p| &p.0);
    for key in desc.required_params {
        if params.and_then(|p| p.get(*key)).is_none_or(is_blank) {
            return Err(AppError::InvalidInput(format!(
                "{}: {key} is required",
                desc.label
            )));
        }
    }
    if let Some(params) = params {
        check_params(desc, params)?;
    }
    let mode = profile.options.password_mode();
    if !desc.password_modes.contains(&mode) {
        return Err(AppError::InvalidInput(format!(
            "{} connections cannot use this password mode",
            desc.label
        )));
    }
    if mode == PasswordMode::Command && profile.options.password_command().is_none() {
        return Err(AppError::InvalidInput(
            "Enter the command that prints the password (Command Line mode)".into(),
        ));
    }
    match profile.driver {
        // Returned above.
        Driver::Postgres | Driver::Mysql | Driver::Sqlite => Ok(()),
        Driver::Sqlserver => sqlserver_lane::validate(profile),
        Driver::Libsql => libsql_lane::validate(profile),
        Driver::D1 => d1_lane::validate(profile),
        Driver::Duckdb => duckdb_lane::validate(profile),
        Driver::Clickhouse => clickhouse_lane::validate(profile),
        Driver::Snowflake => snowflake_lane::validate(profile),
        Driver::Bigquery => bigquery_lane::validate(profile),
        Driver::Vertica => vertica_lane::validate(profile),
        Driver::Cassandra => cassandra_lane::validate(profile),
        Driver::Redis => redis_lane::validate(profile),
        Driver::Mongodb => mongodb_lane::validate(profile),
        Driver::Elasticsearch => elasticsearch_lane::validate(profile),
        Driver::Dynamodb => dynamodb_lane::validate(profile),
    }
}

fn has_field(profile: &ConnectionProfile, field: ProfileField) -> bool {
    let set = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.trim().is_empty());
    match field {
        ProfileField::Host => set(&profile.host),
        ProfileField::Port => profile.port.is_some(),
        ProfileField::User => set(&profile.user),
        ProfileField::Database => set(&profile.database),
        ProfileField::File => set(&profile.sqlite_path),
    }
}

fn is_blank(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        _ => false,
    }
}

/// `driver_params` are stored as plain text in the app database, so they never hold a secret:
/// secrets go in the keychain-backed fields, and large credentials stay in files the params name
/// (a key file path).
fn check_params(desc: &DriverDescriptor, params: &Map<String, Value>) -> Result<(), AppError> {
    params
        .iter()
        .try_for_each(|(key, value)| check_param(desc, key, value))
}

fn check_param(desc: &DriverDescriptor, key: &str, value: &Value) -> Result<(), AppError> {
    if secret_name(key) && can_be_secret(value) {
        return Err(AppError::InvalidInput(format!(
            "{}: \u{201c}{key}\u{201d} is a secret: put it in the {} field",
            desc.label,
            desc.secret.text()
        )));
    }
    match value {
        Value::String(s) if url_has_password(s) => Err(AppError::InvalidInput(format!(
            "{}: \u{201c}{key}\u{201d} holds a password: put the password in the {} field",
            desc.label,
            desc.secret.text()
        ))),
        Value::String(s) if s.contains("-----BEGIN") && s.contains("PRIVATE KEY") => {
            Err(AppError::InvalidInput(format!(
                "{}: \u{201c}{key}\u{201d} holds a private key: save it to a file and give its path",
                desc.label
            )))
        }
        Value::Array(items) => items.iter().try_for_each(|v| check_param(desc, key, v)),
        Value::Object(map) => check_params(desc, map),
        _ => Ok(()),
    }
}

/// A value a secret could be: text, a number or an object of them. A switch (`useToken: true`)
/// or an empty value is not one. Arrays are checked item by item.
fn can_be_secret(value: &Value) -> bool {
    match value {
        Value::String(s) => !s.trim().is_empty(),
        Value::Number(_) => true,
        Value::Object(map) => !map.is_empty(),
        Value::Null | Value::Bool(_) | Value::Array(_) => false,
    }
}

/// A param name that stands for a secret (`password`, `apiKey`, `sessionToken`…). Names of where
/// a secret is or of what it is are fine: a file or path (`privateKeyPath`), an id (`apiKeyId`),
/// an address (`tokenUrl`), a name, type, mode or environment variable (`secretName`).
fn secret_name(key: &str) -> bool {
    let name: String = key
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let about_a_secret = [
        "path", "file", "id", "url", "uri", "endpoint", "name", "type", "mode", "env",
    ];
    if about_a_secret.iter().any(|suffix| name.ends_with(suffix)) {
        return false;
    }
    [
        "password",
        "passwd",
        "pwd",
        "secret",
        "token",
        "apikey",
        "privatekey",
        "passphrase",
    ]
    .iter()
    .any(|word| name.contains(word))
}

/// A URL with a password in it (`mongodb://user:secret@host`), also one pasted unencoded, whose
/// `/`, `?` or `#` ends the authority early (`mongodb://user:pa/ss@host`).
fn url_has_password(s: &str) -> bool {
    let Some((_, rest)) = s.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    match authority.rsplit_once('@') {
        Some((userinfo, _)) => userinfo
            .split_once(':')
            .is_some_and(|(_, password)| !password.is_empty()),
        // `user:pa` with an `@` further on, unless what follows the colon is a port
        // (`es:9200/_search?q=a@b`) or the host is an IPv6 address.
        None => {
            !authority.starts_with('[')
                && rest[authority.len()..].contains('@')
                && authority
                    .rsplit_once(':')
                    .is_some_and(|(_, after)| !after.bytes().all(|b| b.is_ascii_digit()))
        }
    }
}

/// The shims of a lane, in a module of their own: `open`, `validate`, `util` and `util_kind` of
/// its `entry` module (a path under `crate::drivers`) when its feature is on; otherwise
/// [`not_built`], no checks of its own and no call it knows.
macro_rules! lane {
    ($feature:literal, $driver:ident, $shim:ident, $($entry:ident)::+) => {
        mod $shim {
            use super::*;

            #[cfg(feature = $feature)]
            pub(super) async fn open(target: ConnectTarget<'_>) -> Result<Session, AppError> {
                crate::drivers::$($entry)::+::open(target).await
            }

            #[cfg(not(feature = $feature))]
            pub(super) async fn open(_target: ConnectTarget<'_>) -> Result<Session, AppError> {
                Err(not_built(Driver::$driver))
            }

            #[cfg(feature = $feature)]
            pub(super) fn validate(profile: &ConnectionProfile) -> Result<(), AppError> {
                crate::drivers::$($entry)::+::validate(profile)
            }

            #[cfg(not(feature = $feature))]
            pub(super) fn validate(_profile: &ConnectionProfile) -> Result<(), AppError> {
                Ok(())
            }

            #[cfg(feature = $feature)]
            pub(super) async fn util(
                input: UtilInput<'_>,
                method: &str,
                args: Value,
            ) -> Result<Value, AppError> {
                crate::drivers::$($entry)::+::util(input, method, args).await
            }

            #[cfg(not(feature = $feature))]
            pub(super) async fn util(
                _input: UtilInput<'_>,
                _method: &str,
                _args: Value,
            ) -> Result<Value, AppError> {
                Err(not_built(Driver::$driver))
            }

            #[cfg(feature = $feature)]
            pub(super) fn util_kind(method: &str) -> CallKind {
                crate::drivers::$($entry)::+::util_kind(method)
            }

            #[cfg(not(feature = $feature))]
            pub(super) fn util_kind(_method: &str) -> CallKind {
                CallKind::Write
            }
        }
    };
}

lane!("drv-sqlserver", Sqlserver, sqlserver_lane, sqlserver::entry);
lane!(
    "drv-libsql",
    Libsql,
    libsql_lane,
    sqlite_http::libsql::entry
);
lane!("drv-d1", D1, d1_lane, sqlite_http::d1::entry);
lane!("drv-duckdb", Duckdb, duckdb_lane, duckdb::entry);
lane!(
    "drv-clickhouse",
    Clickhouse,
    clickhouse_lane,
    clickhouse::entry
);
lane!("drv-snowflake", Snowflake, snowflake_lane, snowflake::entry);
lane!("drv-bigquery", Bigquery, bigquery_lane, bigquery::entry);
lane!("drv-vertica", Vertica, vertica_lane, vertica::entry);
lane!("drv-cassandra", Cassandra, cassandra_lane, cassandra::entry);
lane!("drv-redis", Redis, redis_lane, redis::entry);
lane!("drv-mongodb", Mongodb, mongodb_lane, mongodb::entry);
lane!(
    "drv-elasticsearch",
    Elasticsearch,
    elasticsearch_lane,
    elasticsearch::entry
);
lane!("drv-dynamodb", Dynamodb, dynamodb_lane, dynamodb::entry);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::driver::PgFlavor;
    use crate::types::profile::{DriverParams, MysqlVersion, ProfileOptions, SslMode};

    fn profile(driver: Driver) -> ConnectionProfile {
        ConnectionProfile {
            id: uuid::Uuid::new_v4(),
            name: "x".into(),
            color: None,
            group: None,
            driver,
            host: None,
            port: None,
            user: None,
            database: None,
            ssl: SslMode::Prefer,
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

    fn params(json: serde_json::Value) -> Option<DriverParams> {
        match json {
            Value::Object(map) => Some(DriverParams(map)),
            _ => panic!("params are an object"),
        }
    }

    fn message(r: Result<(), AppError>) -> String {
        r.unwrap_err().to_string()
    }

    #[test]
    fn the_core_drivers_are_always_built_and_lanes_only_with_their_feature() {
        for d in Driver::ALL {
            let feature = d.descriptor().cargo_feature;
            assert_eq!(built(d), feature.is_none_or(feature_built), "{d:?}");
            assert_eq!(feature.is_none(), d.is_core());
        }
        assert!(built_drivers().starts_with(&[Driver::Postgres, Driver::Mysql, Driver::Sqlite]));
        if LANE_FEATURES.iter().all(|(_, on)| !on) {
            assert_eq!(
                built_drivers(),
                [Driver::Postgres, Driver::Mysql, Driver::Sqlite]
            );
            assert_eq!(
                built_kinds(),
                [
                    KindId::Postgres,
                    KindId::Mysql,
                    KindId::Mariadb,
                    KindId::Sqlite
                ]
            );
        }
    }

    /// Every feature a descriptor names is a lane feature, declared in Cargo.toml and part of
    /// `all-drivers`.
    #[test]
    fn descriptor_features_match_the_manifest() {
        let manifest =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
        let all = manifest
            .split_once("\nall-drivers = [")
            .and_then(|(_, rest)| rest.split_once(']'))
            .expect("an all-drivers feature")
            .0;
        for (feature, _) in LANE_FEATURES {
            assert!(
                manifest
                    .lines()
                    .any(|l| l.starts_with(&format!("{feature} ="))),
                "{feature} is declared"
            );
            assert!(all.contains(&format!("\"{feature}\"")), "{feature}");
        }
        let named = Driver::ALL
            .iter()
            .filter_map(|d| d.descriptor().cargo_feature)
            .chain(KINDS.iter().filter_map(|k| k.cargo_feature));
        for feature in named {
            assert!(
                LANE_FEATURES.iter().any(|(f, _)| *f == feature),
                "{feature}"
            );
        }
    }

    #[tokio::test]
    async fn a_driver_this_build_lacks_is_refused_before_connecting() {
        let refused = Driver::ALL.into_iter().filter(|d| {
            !built(*d) || (!d.is_core() && d.descriptor().session == SessionKind::Native)
        });
        for d in refused {
            let mut p = profile(d);
            p.host = Some("127.0.0.1".into());
            p.port = Some(1);
            let err = connect(ConnectTarget {
                profile: &p,
                password: None,
                host_override: None,
            })
            .await
            .err()
            .expect("no session");
            assert_eq!(err.code(), "UNSUPPORTED", "{d:?}");
            let text = err.to_string();
            assert!(text.contains(d.label()), "{text}");
            if built(d) {
                assert!(text.contains("not a SQL database"), "{text}");
            } else {
                assert!(text.contains("is not built into this Tabler"), "{text}");
            }
        }
        if !cfg!(feature = "drv-redis") {
            let err = connect(ConnectTarget {
                profile: &profile(Driver::Redis),
                password: None,
                host_override: None,
            })
            .await
            .err()
            .unwrap();
            assert_eq!(
                err.to_string(),
                "unsupported: Redis is not built into this Tabler"
            );
        }
    }

    #[tokio::test]
    async fn ensure_built_refuses_the_drivers_and_flavors_this_build_lacks() {
        for d in Driver::ALL {
            match ensure_built(&profile(d)) {
                Ok(()) => assert!(built(d), "{d:?}"),
                Err(e) => {
                    assert!(!built(d), "{d:?}");
                    assert_eq!(e.to_string(), not_built(d).to_string());
                }
            }
        }
        let mut mariadb = profile(Driver::Mysql);
        mariadb.options.mysql_version = Some(MysqlVersion::Mariadb);
        ensure_built(&mariadb).unwrap();

        // A flavor without its feature does not open as plain PostgreSQL.
        let mut redshift = profile(Driver::Postgres);
        redshift.host = Some("127.0.0.1".into());
        redshift.port = Some(1);
        redshift.options.pg_flavor = Some(PgFlavor::Redshift);
        if cfg!(feature = "drv-pg-flavors") {
            ensure_built(&redshift).unwrap();
        } else {
            let refused = "unsupported: Amazon Redshift is not built into this Tabler";
            assert_eq!(message(ensure_built(&redshift)), refused);
            let err = connect(ConnectTarget {
                profile: &redshift,
                password: None,
                host_override: None,
            })
            .await
            .err()
            .expect("no session");
            assert_eq!(err.to_string(), refused);
        }
    }

    #[test]
    fn validate_checks_the_descriptor_fields_and_params() {
        let mut redis = profile(Driver::Redis);
        assert_eq!(message(validate(&redis)), "Host is required");
        redis.host = Some("cache.local".into());
        validate(&redis).unwrap();

        let mut d1 = profile(Driver::D1);
        d1.database = Some("6c8b…".into());
        assert_eq!(
            message(validate(&d1)),
            "Cloudflare D1: accountId is required"
        );
        d1.options.driver_params = params(serde_json::json!({ "accountId": "  " }));
        assert!(validate(&d1).is_err());
        d1.options.driver_params = params(serde_json::json!({ "accountId": "023e105f" }));
        validate(&d1).unwrap();
        // A token is always needed.
        d1.options.password_mode = Some(PasswordMode::None);
        assert!(message(validate(&d1)).contains("password mode"));
        d1.options.password_mode = Some(PasswordMode::Command);
        assert!(message(validate(&d1)).contains("Command Line"));
        d1.options.password_command = Some("wrangler whoami".into());
        validate(&d1).unwrap();

        let mut dynamo = profile(Driver::Dynamodb);
        assert_eq!(message(validate(&dynamo)), "Region is required");
        dynamo.database = Some("eu-west-1".into());
        validate(&dynamo).unwrap();
    }

    #[test]
    fn validate_refuses_secrets_in_params() {
        let mut mongo = profile(Driver::Mongodb);
        mongo.options.driver_params = params(serde_json::json!({ "uri": "mongodb://u:p@h" }));
        let text = message(validate(&mongo));
        assert!(
            text.contains("put the password in the Password field"),
            "{text}"
        );
        for ok in [
            serde_json::json!({ "uri": "mongodb://u@h1,h2/db?authSource=admin" }),
            serde_json::json!({ "uri": "mongodb://u:@h/db" }),
            serde_json::json!({ "uri": "mongodb+srv://cluster0.example.net/" }),
            serde_json::json!({ "privateKeyPath": "C:/keys/rsa_key.p8", "password": "" }),
            serde_json::json!({ "accessKeyId": "AKIA…", "nested": { "region": "x" } }),
            // Names of where a secret is or what it is, and switches.
            serde_json::json!({ "apiKeyId": "VuaCfGcB", "tokenUrl": "https://oauth2.example/t" }),
            serde_json::json!({ "secretName": "prod/db", "token_type": "Bearer" }),
            serde_json::json!({ "useToken": true, "password": null }),
            // A port, an IPv6 host: the `@` further on is the path's or the query's.
            serde_json::json!({ "hosts": ["http://es:9200/_search?q=a@b", "http://[::1]/x@y"] }),
        ] {
            mongo.options.driver_params = params(ok.clone());
            validate(&mongo).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for bad in [
            serde_json::json!({ "password": "hunter2" }),
            serde_json::json!({ "password": 123456 }),
            serde_json::json!({ "passwords": ["hunter2"] }),
            serde_json::json!({ "secret": { "value": "s" } }),
            serde_json::json!({ "sessionToken": "FwoG…" }),
            serde_json::json!({ "api_key": "k" }),
            serde_json::json!({ "hosts": ["http://elastic:changeme@es:9200"] }),
            serde_json::json!({ "auth": { "clientSecret": "s" } }),
            serde_json::json!({ "key": "-----BEGIN PRIVATE KEY-----\nMII…" }),
            // Unencoded passwords with `/`, `?` or `#` in them.
            serde_json::json!({ "uri": "mongodb://u:pa/ss@h/db" }),
            serde_json::json!({ "uri": "mongodb://u:p?ss@h" }),
            serde_json::json!({ "uri": "redis://:p#ss@h:6379" }),
        ] {
            mongo.options.driver_params = params(bad.clone());
            assert!(validate(&mongo).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn open_gives_the_core_drivers_a_sql_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut lite = profile(Driver::Sqlite);
        lite.sqlite_path = Some(dir.path().join("a.sqlite").to_string_lossy().into_owned());
        let session = open(ConnectTarget {
            profile: &lite,
            password: None,
            host_override: None,
        })
        .await
        .unwrap();
        assert!(matches!(session, Session::Sql(_)));
        assert_eq!(session.driver(), Driver::Sqlite);
        let sql = session.sql().unwrap();
        sql.execute("CREATE TABLE t (x)", &[]).await.unwrap();
        session.close().await;
        assert!(sql.execute("SELECT 1", &[]).await.is_err());
    }

    #[tokio::test]
    async fn open_refuses_a_driver_this_build_lacks() {
        for d in Driver::ALL.into_iter().filter(|d| !built(*d)) {
            let mut p = profile(d);
            p.host = Some("127.0.0.1".into());
            p.port = Some(1);
            let err = open(ConnectTarget {
                profile: &p,
                password: None,
                host_override: None,
            })
            .await
            .err()
            .expect("no session");
            assert_eq!(err.to_string(), not_built(d).to_string());
        }
    }

    /// Only the SQL drivers open a session for a SQL-only caller, built or not.
    #[test]
    fn ensure_sql_refuses_the_native_stores() {
        for d in Driver::ALL {
            let native = d.descriptor().session == SessionKind::Native;
            match ensure_sql(&profile(d)) {
                Ok(()) => assert!(!native, "{d:?}"),
                Err(e) => {
                    assert!(native, "{d:?}");
                    assert_eq!(e.to_string(), not_sql(d).to_string());
                }
            }
        }
        assert!(ensure_sql(&profile(Driver::Redis)).is_err());
        assert!(ensure_sql(&profile(Driver::Postgres)).is_ok());
    }

    /// The form's calls: the core drivers know none, a driver this build lacks is refused, and
    /// a call nobody knows changes something.
    #[tokio::test]
    async fn util_reaches_only_a_built_lane() {
        for d in Driver::ALL {
            let p = profile(d);
            let input = UtilInput {
                profile: &p,
                secret: None,
            };
            let err = util(input, "no-such-call", Value::Null)
                .await
                .expect_err("nobody knows it");
            assert_eq!(err.code(), "UNSUPPORTED", "{d:?}");
            let text = err.to_string();
            if built(d) {
                assert!(text.contains(d.label()), "{text}");
            } else {
                assert_eq!(text, not_built(d).to_string());
            }
            assert_eq!(util_kind(d, "no-such-call"), CallKind::Write, "{d:?}");
        }
        let core = UtilInput {
            profile: &profile(Driver::Mysql),
            secret: None,
        };
        assert_eq!(
            util(core, "probe", Value::Null)
                .await
                .unwrap_err()
                .to_string(),
            "unsupported: MySQL: unknown call \u{201c}probe\u{201d}"
        );
    }
}
