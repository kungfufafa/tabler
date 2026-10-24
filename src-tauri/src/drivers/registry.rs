//! The drivers this build has, and the one place that hands a profile to its driver. Every
//! driver beyond PostgreSQL, MySQL and SQLite sits behind a cargo feature (`drv-*`); for each one
//! a pair of shim functions below calls into its module when the feature is on and answers
//! [`not_built`] (or has nothing to check) when it is off, so no other code needs a `cfg`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Map, Value};

use crate::drivers::{ConnectTarget, DbSession, mysql, postgres, sqlite};
use crate::error::AppError;
use crate::types::driver::{DriverDescriptor, KINDS, KindDescriptor, KindId, ProfileField};
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

fn not_sql(driver: Driver) -> AppError {
    AppError::Unsupported(format!(
        "{} is not a SQL database: it has no SQL session",
        driver.label()
    ))
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

/// Opens a SQL session for `target`'s profile.
pub async fn connect(target: ConnectTarget<'_>) -> Result<Arc<dyn DbSession>, AppError> {
    ensure_built(target.profile)?;
    let driver = target.profile.driver;
    match driver {
        // A SQLite profile's password is its SQLCipher encryption key.
        Driver::Sqlite => Ok(Arc::new(
            sqlite::LiteSession::open_with_key(target.profile, target.password).await?,
        )),
        Driver::Postgres => postgres::connect(target).await,
        Driver::Mysql => mysql::connect(target).await,
        Driver::Redis | Driver::Mongodb | Driver::Elasticsearch | Driver::Dynamodb => {
            Err(not_sql(driver))
        }
        Driver::Sqlserver => open_sqlserver(target).await,
        Driver::Libsql => open_libsql(target).await,
        Driver::D1 => open_d1(target).await,
        Driver::Duckdb => open_duckdb(target).await,
        Driver::Clickhouse => open_clickhouse(target).await,
        Driver::Snowflake => open_snowflake(target).await,
        Driver::Bigquery => open_bigquery(target).await,
        Driver::Vertica => open_vertica(target).await,
        Driver::Cassandra => open_cassandra(target).await,
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
        Driver::Sqlserver => validate_sqlserver(profile),
        Driver::Libsql => validate_libsql(profile),
        Driver::D1 => validate_d1(profile),
        Driver::Duckdb => validate_duckdb(profile),
        Driver::Clickhouse => validate_clickhouse(profile),
        Driver::Snowflake => validate_snowflake(profile),
        Driver::Bigquery => validate_bigquery(profile),
        Driver::Vertica => validate_vertica(profile),
        Driver::Cassandra => validate_cassandra(profile),
        Driver::Redis => validate_redis(profile),
        Driver::Mongodb => validate_mongodb(profile),
        Driver::Elasticsearch => validate_elasticsearch(profile),
        Driver::Dynamodb => validate_dynamodb(profile),
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

/// The two shims of a lane SQL driver: its `entry` module (a path under `crate::drivers`) when
/// its feature is on, else [`not_built`] and no checks of its own.
macro_rules! sql_lane {
    ($feature:literal, $driver:ident, $open:ident, $validate:ident, $($entry:ident)::+) => {
        #[cfg(feature = $feature)]
        async fn $open(target: ConnectTarget<'_>) -> Result<Arc<dyn DbSession>, AppError> {
            crate::drivers::$($entry)::+::open(target).await
        }

        #[cfg(not(feature = $feature))]
        async fn $open(_target: ConnectTarget<'_>) -> Result<Arc<dyn DbSession>, AppError> {
            Err(not_built(Driver::$driver))
        }

        lane_validate!($feature, $validate, $($entry)::+);
    };
}

/// The validate shim of a lane.
macro_rules! lane_validate {
    ($feature:literal, $validate:ident, $($entry:ident)::+) => {
        #[cfg(feature = $feature)]
        fn $validate(profile: &ConnectionProfile) -> Result<(), AppError> {
            crate::drivers::$($entry)::+::validate(profile)
        }

        #[cfg(not(feature = $feature))]
        fn $validate(_profile: &ConnectionProfile) -> Result<(), AppError> {
            Ok(())
        }
    };
}

sql_lane!(
    "drv-sqlserver",
    Sqlserver,
    open_sqlserver,
    validate_sqlserver,
    sqlserver::entry
);
sql_lane!(
    "drv-libsql",
    Libsql,
    open_libsql,
    validate_libsql,
    sqlite_http::libsql::entry
);
sql_lane!("drv-d1", D1, open_d1, validate_d1, sqlite_http::d1::entry);
sql_lane!(
    "drv-duckdb",
    Duckdb,
    open_duckdb,
    validate_duckdb,
    duckdb::entry
);
sql_lane!(
    "drv-clickhouse",
    Clickhouse,
    open_clickhouse,
    validate_clickhouse,
    clickhouse::entry
);
sql_lane!(
    "drv-snowflake",
    Snowflake,
    open_snowflake,
    validate_snowflake,
    snowflake::entry
);
sql_lane!(
    "drv-bigquery",
    Bigquery,
    open_bigquery,
    validate_bigquery,
    bigquery::entry
);
sql_lane!(
    "drv-vertica",
    Vertica,
    open_vertica,
    validate_vertica,
    vertica::entry
);
sql_lane!(
    "drv-cassandra",
    Cassandra,
    open_cassandra,
    validate_cassandra,
    cassandra::entry
);
// The native stores open no SQL session; only their checks are wired for now.
lane_validate!("drv-redis", validate_redis, redis::entry);
lane_validate!("drv-mongodb", validate_mongodb, mongodb::entry);
lane_validate!(
    "drv-elasticsearch",
    validate_elasticsearch,
    elasticsearch::entry
);
lane_validate!("drv-dynamodb", validate_dynamodb, dynamodb::entry);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::driver::{PgFlavor, SessionKind};
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
}
