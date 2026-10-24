//! What Tabler knows about each driver and each database of the new-connection picker, as data:
//! ports, labels, the fields a profile needs, how secrets and TLS work, the read-only mechanism.
//! Always compiled, whatever drivers a build has: shared code reads these values instead of
//! matching on `Driver`, and the renderer gets them from `driverCatalog.ts` (written by the
//! `export_bindings_driver_catalog` test next to the ts-rs bindings).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::sqlgen::SqlDialect;
use crate::types::caps::Feature;
use crate::types::profile::{Driver, MysqlVersion, PasswordMode};

/// The PostgreSQL-compatible engine of a `postgres` profile (`ProfileOptions::pg_flavor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum PgFlavor {
    Cockroachdb,
    Greenplum,
    Redshift,
}

/// The server family a session found at connect, when its driver serves several.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Flavor {
    Mariadb,
    Cockroachdb,
    Greenplum,
    Redshift,
}

/// A database of the new-connection picker: a driver, or a flavor one serves (MariaDB is the
/// MySQL driver, CockroachDB the PostgreSQL one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, rename = "DatabaseKind")]
pub enum KindId {
    Postgres,
    Cockroachdb,
    Greenplum,
    Redshift,
    Mysql,
    Mariadb,
    Sqlite,
    Libsql,
    D1,
    Duckdb,
    Sqlserver,
    Clickhouse,
    Snowflake,
    Bigquery,
    Vertica,
    Cassandra,
    Redis,
    Mongodb,
    Elasticsearch,
    Dynamodb,
}

impl KindId {
    /// The kind of a profile with these settings.
    pub fn of(driver: Driver, pg_flavor: Option<PgFlavor>, mysql: Option<MysqlVersion>) -> KindId {
        match driver {
            Driver::Postgres => match pg_flavor {
                None => KindId::Postgres,
                Some(PgFlavor::Cockroachdb) => KindId::Cockroachdb,
                Some(PgFlavor::Greenplum) => KindId::Greenplum,
                Some(PgFlavor::Redshift) => KindId::Redshift,
            },
            Driver::Mysql if mysql == Some(MysqlVersion::Mariadb) => KindId::Mariadb,
            Driver::Mysql => KindId::Mysql,
            Driver::Sqlite => KindId::Sqlite,
            Driver::Sqlserver => KindId::Sqlserver,
            Driver::Libsql => KindId::Libsql,
            Driver::D1 => KindId::D1,
            Driver::Duckdb => KindId::Duckdb,
            Driver::Clickhouse => KindId::Clickhouse,
            Driver::Snowflake => KindId::Snowflake,
            Driver::Bigquery => KindId::Bigquery,
            Driver::Vertica => KindId::Vertica,
            Driver::Cassandra => KindId::Cassandra,
            Driver::Redis => KindId::Redis,
            Driver::Mongodb => KindId::Mongodb,
            Driver::Elasticsearch => KindId::Elasticsearch,
            Driver::Dynamodb => KindId::Dynamodb,
        }
    }

    pub fn descriptor(self) -> &'static KindDescriptor {
        KINDS
            .iter()
            .find(|k| k.kind == self)
            .expect("every kind is in KINDS")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Family {
    Sql,
    KeyValue,
    Document,
    Search,
    WideColumn,
}

/// What the editor (or console) of a connection speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum QueryLanguage {
    Sql,
    Cql,
    RedisCli,
    MongoShell,
    EsConsole,
    Partiql,
}

/// Whether the driver's sessions are SQL sessions (`DbSession`) or native stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SessionKind {
    Sql,
    Native,
}

/// Where the database is: a server (host and port), a local file, or a cloud account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "DriverLocation")]
pub enum Location {
    Host,
    File,
    Cloud,
}

/// How SSH reaches the database: a port-forwarding tunnel (one node of a cluster), SFTP (the
/// database file is copied), or not at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SshSupport {
    Tunnel,
    Sftp,
    None,
}

/// Which SSL mode choices the form offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TlsStyle {
    Libpq,
    Mysql,
    Tds,
    OnOff,
    Https,
    None,
}

/// What the profile's secret (`SecretKind::DbPassword`) is called in the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SecretLabel {
    Password,
    AuthToken,
    ApiToken,
    ApiKey,
    SecretAccessKey,
    EncryptionKey,
    AccessToken,
    PasswordOrToken,
}

impl SecretLabel {
    pub fn text(self) -> &'static str {
        match self {
            SecretLabel::Password => "Password",
            SecretLabel::AuthToken => "Auth Token",
            SecretLabel::ApiToken => "API Token",
            SecretLabel::ApiKey => "API Key",
            SecretLabel::SecretAccessKey => "Secret Access Key",
            SecretLabel::EncryptionKey => "Encryption Key",
            SecretLabel::AccessToken => "Access Token",
            SecretLabel::PasswordOrToken => "Password or Token",
        }
    }
}

/// A second secret some drivers need besides the password.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ExtraSecret {
    KeyPassphrase,
    SessionToken,
}

/// A common field of a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ProfileField {
    Host,
    Port,
    User,
    Database,
    File,
}

impl ProfileField {
    /// The field's name in the form, with the driver's label override.
    pub fn label(self, labels: &FieldLabels) -> &'static str {
        match self {
            ProfileField::Host => labels.host.unwrap_or("Host"),
            ProfileField::Port => "Port",
            ProfileField::User => labels.user.unwrap_or("User"),
            ProfileField::Database => labels.database.unwrap_or("Database"),
            ProfileField::File => labels.file.unwrap_or("Database file"),
        }
    }
}

/// Form labels of the common fields that a driver names differently (`None`: the usual name).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FieldLabels {
    pub host: Option<&'static str>,
    pub user: Option<&'static str>,
    pub database: Option<&'static str>,
    pub file: Option<&'static str>,
}

const NO_LABELS: FieldLabels = FieldLabels {
    host: None,
    user: None,
    database: None,
    file: None,
};

/// What the profile's "Bootstrap commands" are: SQL, the store's own commands, or not offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum StartupKind {
    Sql,
    Commands,
    None,
}

/// How a read-only profile is enforced: by the server for the whole session, on every request,
/// or only by Tabler's statement classifier (which refuses anything it does not know is a read).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ReadOnlyMode {
    Server,
    PerRequest,
    ClassifierOnly,
}

/// Whether two sessions of one database may be open at once in this process (DuckDB opens one
/// instance per file).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Sharing {
    Independent,
    SingleProcess,
}

/// The fixed facts of a driver.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DriverDescriptor {
    pub driver: Driver,
    pub label: &'static str,
    /// Two-letter badge.
    pub short: &'static str,
    pub family: Family,
    pub language: QueryLanguage,
    pub session: SessionKind,
    /// The SQL dialect of its sessions; None for native stores (`session == Native`).
    pub dialect: Option<SqlDialect>,
    pub location: Location,
    pub default_port: Option<u16>,
    /// The port when TLS is on, where it differs from `default_port`.
    pub tls_port: Option<u16>,
    pub ssh: SshSupport,
    pub tls: TlsStyle,
    /// Offers "Use socket".
    pub socket: bool,
    #[ts(as = "Vec<PasswordMode>")]
    pub password_modes: &'static [PasswordMode],
    pub secret: SecretLabel,
    /// The secret is a token, not something Safe Mode can ask the user to type again.
    pub secret_is_token: bool,
    #[ts(as = "Vec<ExtraSecret>")]
    pub extra_secrets: &'static [ExtraSecret],
    /// How long a Command Line password (a token) is reused before the command runs again; 0 for
    /// drivers without Command Line mode.
    pub command_ttl_secs: u32,
    #[ts(as = "Vec<ProfileField>")]
    pub required: &'static [ProfileField],
    /// Keys that must be set in `ProfileOptions::driver_params`.
    #[ts(as = "Vec<String>")]
    pub required_params: &'static [&'static str],
    pub labels: FieldLabels,
    /// Has a connection encoding (sessions in another one than UTF-8 are read-only).
    pub encoding: bool,
    pub timezone: bool,
    pub startup: StartupKind,
    pub bootstrap_script: bool,
    /// Tables without a primary key are addressed by their rowid in the grid (SQLite). A lane
    /// turns this on only where the engine guarantees one.
    pub rowid: bool,
    #[ts(as = "Vec<String>")]
    pub url_schemes: &'static [&'static str],
    /// Database file extensions, without the dot.
    #[ts(as = "Vec<String>")]
    pub file_extensions: &'static [&'static str],
    /// Schemas left out of lists; a trailing `*` matches any rest (`pg_*`).
    #[ts(as = "Vec<String>")]
    pub system_schemas: &'static [&'static str],
    pub read_only: ReadOnlyMode,
    pub sharing: Sharing,
    /// Core-only features (DDL planner, backups…) a non-core driver may use with the core SQL
    /// generators of its dialect; empty for every driver in v1.
    #[ts(as = "Vec<Feature>")]
    pub core_reuse: &'static [Feature],
    /// The cargo feature that builds the driver; None for the drivers of every build.
    pub cargo_feature: Option<&'static str>,
}

impl DriverDescriptor {
    /// Whether `schema` is one of the driver's system schemas.
    pub fn is_system_schema(&self, schema: &str) -> bool {
        self.system_schemas
            .iter()
            .any(|pattern| match pattern.strip_suffix('*') {
                Some(prefix) => schema.starts_with(prefix),
                None => schema == *pattern,
            })
    }
}

/// The fixed facts of a database of the picker.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KindDescriptor {
    pub kind: KindId,
    pub driver: Driver,
    pub label: &'static str,
    pub short: &'static str,
    /// Tile colour (an accent preset name).
    pub tint: &'static str,
    pub pg_flavor: Option<PgFlavor>,
    pub mysql_version: Option<MysqlVersion>,
    pub default_port: Option<u16>,
    pub default_database: Option<&'static str>,
    pub default_user: Option<&'static str>,
    /// Backup and Restore are offered.
    pub backup: bool,
    pub cargo_feature: Option<&'static str>,
}

const ALL_MODES: &[PasswordMode] = &[
    PasswordMode::Keychain,
    PasswordMode::Ask,
    PasswordMode::None,
    PasswordMode::Command,
];
/// An encryption key is typed, never a command's output.
const KEY_MODES: &[PasswordMode] = &[
    PasswordMode::Keychain,
    PasswordMode::Ask,
    PasswordMode::None,
];
/// A token is always needed.
const TOKEN_MODES: &[PasswordMode] = &[
    PasswordMode::Keychain,
    PasswordMode::Ask,
    PasswordMode::Command,
];

/// The descriptor fields most drivers share; each entry below spells out what differs.
const BASE: DriverDescriptor = DriverDescriptor {
    driver: Driver::Postgres,
    label: "",
    short: "",
    family: Family::Sql,
    language: QueryLanguage::Sql,
    session: SessionKind::Sql,
    dialect: None,
    location: Location::Host,
    default_port: None,
    tls_port: None,
    ssh: SshSupport::Tunnel,
    tls: TlsStyle::OnOff,
    socket: false,
    password_modes: ALL_MODES,
    secret: SecretLabel::Password,
    secret_is_token: false,
    extra_secrets: &[],
    command_ttl_secs: 600,
    required: &[ProfileField::Host],
    required_params: &[],
    labels: NO_LABELS,
    encoding: false,
    timezone: false,
    startup: StartupKind::Sql,
    bootstrap_script: true,
    rowid: false,
    url_schemes: &[],
    file_extensions: &[],
    system_schemas: &[],
    read_only: ReadOnlyMode::ClassifierOnly,
    sharing: Sharing::Independent,
    core_reuse: &[],
    cargo_feature: None,
};

static POSTGRES: DriverDescriptor = DriverDescriptor {
    driver: Driver::Postgres,
    label: "PostgreSQL",
    short: "PG",
    dialect: Some(SqlDialect::Postgres),
    default_port: Some(5432),
    tls: TlsStyle::Libpq,
    required: &[ProfileField::Host, ProfileField::User],
    encoding: true,
    timezone: true,
    url_schemes: &["postgres", "postgresql", "pgsql"],
    system_schemas: &["pg_*", "information_schema"],
    read_only: ReadOnlyMode::Server,
    ..BASE
};

static MYSQL: DriverDescriptor = DriverDescriptor {
    driver: Driver::Mysql,
    label: "MySQL",
    short: "MY",
    dialect: Some(SqlDialect::Mysql),
    default_port: Some(3306),
    tls: TlsStyle::Mysql,
    socket: true,
    required: &[ProfileField::Host, ProfileField::User],
    encoding: true,
    timezone: true,
    url_schemes: &["mysql", "mariadb"],
    system_schemas: &["information_schema", "mysql", "performance_schema", "sys"],
    read_only: ReadOnlyMode::Server,
    ..BASE
};

static SQLITE: DriverDescriptor = DriverDescriptor {
    driver: Driver::Sqlite,
    label: "SQLite",
    short: "SQ",
    dialect: Some(SqlDialect::Sqlite),
    location: Location::File,
    ssh: SshSupport::Sftp,
    tls: TlsStyle::None,
    password_modes: KEY_MODES,
    secret: SecretLabel::EncryptionKey,
    required: &[ProfileField::File],
    rowid: true,
    url_schemes: &["sqlite", "sqlite3"],
    file_extensions: &["sqlite", "db", "sqlite3", "db3"],
    read_only: ReadOnlyMode::Server,
    ..BASE
};

static SQLSERVER: DriverDescriptor = DriverDescriptor {
    driver: Driver::Sqlserver,
    label: "SQL Server",
    short: "MS",
    dialect: Some(SqlDialect::Tsql),
    default_port: Some(1433),
    tls: TlsStyle::Tds,
    command_ttl_secs: 3000,
    url_schemes: &["sqlserver", "mssql"],
    system_schemas: &["sys", "INFORMATION_SCHEMA"],
    cargo_feature: Some("drv-sqlserver"),
    ..BASE
};

static LIBSQL: DriverDescriptor = DriverDescriptor {
    driver: Driver::Libsql,
    label: "LibSQL",
    short: "LS",
    dialect: Some(SqlDialect::Sqlite),
    // Plain HTTP (TLS off) is the local `sqld` port.
    default_port: Some(8080),
    tls_port: Some(443),
    tls: TlsStyle::Https,
    secret: SecretLabel::AuthToken,
    secret_is_token: true,
    command_ttl_secs: 3000,
    // The host or the `url` param: the lane checks.
    required: &[],
    url_schemes: &["libsql"],
    read_only: ReadOnlyMode::PerRequest,
    cargo_feature: Some("drv-libsql"),
    ..BASE
};

static D1: DriverDescriptor = DriverDescriptor {
    driver: Driver::D1,
    label: "Cloudflare D1",
    short: "D1",
    dialect: Some(SqlDialect::Sqlite),
    location: Location::Cloud,
    ssh: SshSupport::None,
    tls: TlsStyle::Https,
    password_modes: TOKEN_MODES,
    secret: SecretLabel::ApiToken,
    secret_is_token: true,
    command_ttl_secs: 3000,
    required: &[ProfileField::Database],
    required_params: &["accountId"],
    labels: FieldLabels {
        user: Some("Account ID"),
        ..NO_LABELS
    },
    startup: StartupKind::None,
    url_schemes: &["d1", "cloudflared1"],
    cargo_feature: Some("drv-d1"),
    ..BASE
};

static DUCKDB: DriverDescriptor = DriverDescriptor {
    driver: Driver::Duckdb,
    label: "DuckDB",
    short: "DK",
    dialect: Some(SqlDialect::Duckdb),
    location: Location::File,
    ssh: SshSupport::None,
    tls: TlsStyle::None,
    password_modes: KEY_MODES,
    secret: SecretLabel::EncryptionKey,
    command_ttl_secs: 0,
    required: &[ProfileField::File],
    url_schemes: &["duckdb"],
    file_extensions: &["duckdb", "ddb"],
    system_schemas: &["information_schema", "pg_catalog"],
    read_only: ReadOnlyMode::Server,
    sharing: Sharing::SingleProcess,
    cargo_feature: Some("drv-duckdb"),
    ..BASE
};

static CLICKHOUSE: DriverDescriptor = DriverDescriptor {
    driver: Driver::Clickhouse,
    label: "ClickHouse",
    short: "CH",
    dialect: Some(SqlDialect::Clickhouse),
    default_port: Some(8123),
    tls_port: Some(8443),
    startup: StartupKind::None,
    url_schemes: &["clickhouse"],
    system_schemas: &["system", "INFORMATION_SCHEMA", "information_schema"],
    read_only: ReadOnlyMode::PerRequest,
    cargo_feature: Some("drv-clickhouse"),
    ..BASE
};

static SNOWFLAKE: DriverDescriptor = DriverDescriptor {
    driver: Driver::Snowflake,
    label: "Snowflake",
    short: "SF",
    dialect: Some(SqlDialect::Snowflake),
    location: Location::Cloud,
    ssh: SshSupport::None,
    tls: TlsStyle::Https,
    secret: SecretLabel::PasswordOrToken,
    secret_is_token: true,
    extra_secrets: &[ExtraSecret::KeyPassphrase],
    command_ttl_secs: 3000,
    required: &[ProfileField::User],
    required_params: &["account"],
    url_schemes: &["snowflake"],
    system_schemas: &["INFORMATION_SCHEMA"],
    cargo_feature: Some("drv-snowflake"),
    ..BASE
};

static BIGQUERY: DriverDescriptor = DriverDescriptor {
    driver: Driver::Bigquery,
    label: "BigQuery",
    short: "BQ",
    dialect: Some(SqlDialect::Bigquery),
    location: Location::Cloud,
    ssh: SshSupport::None,
    tls: TlsStyle::Https,
    secret: SecretLabel::AccessToken,
    secret_is_token: true,
    command_ttl_secs: 3000,
    required: &[ProfileField::Database],
    labels: FieldLabels {
        database: Some("Project"),
        ..NO_LABELS
    },
    startup: StartupKind::None,
    url_schemes: &["bigquery"],
    // The read-only scope is not proven to stop DML yet: the classifier decides.
    read_only: ReadOnlyMode::ClassifierOnly,
    cargo_feature: Some("drv-bigquery"),
    ..BASE
};

static VERTICA: DriverDescriptor = DriverDescriptor {
    driver: Driver::Vertica,
    label: "Vertica",
    short: "VE",
    dialect: Some(SqlDialect::Vertica),
    default_port: Some(5433),
    tls: TlsStyle::Libpq,
    required: &[
        ProfileField::Host,
        ProfileField::User,
        ProfileField::Database,
    ],
    url_schemes: &["vertica"],
    system_schemas: &[
        "v_catalog",
        "v_monitor",
        "v_internal",
        "v_func",
        "v_txtindex",
    ],
    read_only: ReadOnlyMode::Server,
    cargo_feature: Some("drv-vertica"),
    ..BASE
};

static CASSANDRA: DriverDescriptor = DriverDescriptor {
    driver: Driver::Cassandra,
    label: "Cassandra",
    short: "CA",
    family: Family::WideColumn,
    language: QueryLanguage::Cql,
    dialect: Some(SqlDialect::Cql),
    default_port: Some(9042),
    labels: FieldLabels {
        database: Some("Keyspace"),
        ..NO_LABELS
    },
    url_schemes: &["cassandra"],
    system_schemas: &["system", "system_*"],
    cargo_feature: Some("drv-cassandra"),
    ..BASE
};

static REDIS: DriverDescriptor = DriverDescriptor {
    driver: Driver::Redis,
    label: "Redis",
    short: "RE",
    family: Family::KeyValue,
    language: QueryLanguage::RedisCli,
    session: SessionKind::Native,
    default_port: Some(6379),
    labels: FieldLabels {
        database: Some("DB index"),
        ..NO_LABELS
    },
    startup: StartupKind::Commands,
    url_schemes: &["redis", "rediss"],
    cargo_feature: Some("drv-redis"),
    ..BASE
};

static MONGODB: DriverDescriptor = DriverDescriptor {
    driver: Driver::Mongodb,
    label: "MongoDB",
    short: "MO",
    family: Family::Document,
    language: QueryLanguage::MongoShell,
    session: SessionKind::Native,
    default_port: Some(27017),
    // The host or the `uri` param: the lane checks.
    required: &[],
    startup: StartupKind::None,
    url_schemes: &["mongodb", "mongodb+srv"],
    cargo_feature: Some("drv-mongodb"),
    ..BASE
};

static ELASTICSEARCH: DriverDescriptor = DriverDescriptor {
    driver: Driver::Elasticsearch,
    label: "Elasticsearch",
    short: "ES",
    family: Family::Search,
    language: QueryLanguage::EsConsole,
    session: SessionKind::Native,
    default_port: Some(9200),
    tls: TlsStyle::Https,
    secret: SecretLabel::PasswordOrToken,
    secret_is_token: true,
    command_ttl_secs: 3000,
    // The host, or the `urls` or `cloudId` param: the lane checks.
    required: &[],
    startup: StartupKind::None,
    url_schemes: &["elasticsearch"],
    cargo_feature: Some("drv-elasticsearch"),
    ..BASE
};

static DYNAMODB: DriverDescriptor = DriverDescriptor {
    driver: Driver::Dynamodb,
    label: "DynamoDB",
    short: "DY",
    family: Family::Document,
    language: QueryLanguage::Partiql,
    session: SessionKind::Native,
    location: Location::Cloud,
    ssh: SshSupport::None,
    tls: TlsStyle::Https,
    secret: SecretLabel::SecretAccessKey,
    secret_is_token: true,
    extra_secrets: &[ExtraSecret::SessionToken],
    required: &[ProfileField::Database],
    labels: FieldLabels {
        user: Some("Access key ID"),
        database: Some("Region"),
        ..NO_LABELS
    },
    startup: StartupKind::None,
    url_schemes: &["dynamodb"],
    cargo_feature: Some("drv-dynamodb"),
    ..BASE
};

/// The descriptor of `driver`: the one exhaustive match on `Driver` that engine facts need.
pub fn descriptor(driver: Driver) -> &'static DriverDescriptor {
    match driver {
        Driver::Postgres => &POSTGRES,
        Driver::Mysql => &MYSQL,
        Driver::Sqlite => &SQLITE,
        Driver::Sqlserver => &SQLSERVER,
        Driver::Libsql => &LIBSQL,
        Driver::D1 => &D1,
        Driver::Duckdb => &DUCKDB,
        Driver::Clickhouse => &CLICKHOUSE,
        Driver::Snowflake => &SNOWFLAKE,
        Driver::Bigquery => &BIGQUERY,
        Driver::Vertica => &VERTICA,
        Driver::Cassandra => &CASSANDRA,
        Driver::Redis => &REDIS,
        Driver::Mongodb => &MONGODB,
        Driver::Elasticsearch => &ELASTICSEARCH,
        Driver::Dynamodb => &DYNAMODB,
    }
}

/// A kind served by `driver` with the driver's own label, port and feature.
const fn plain_kind(
    kind: KindId,
    driver: &'static DriverDescriptor,
    tint: &'static str,
    default_database: Option<&'static str>,
    default_user: Option<&'static str>,
) -> KindDescriptor {
    KindDescriptor {
        kind,
        driver: driver.driver,
        label: driver.label,
        short: driver.short,
        tint,
        pg_flavor: None,
        mysql_version: None,
        default_port: driver.default_port,
        default_database,
        default_user,
        backup: false,
        cargo_feature: driver.cargo_feature,
    }
}

/// The feature that builds CockroachDB, Greenplum and Redshift on the PostgreSQL driver.
const PG_FLAVORS: Option<&str> = Some("drv-pg-flavors");

/// The databases of the new-connection picker, in its order (the 4 core tiles first among
/// themselves: PostgreSQL, MySQL, MariaDB, SQLite).
pub static KINDS: [KindDescriptor; 20] = [
    KindDescriptor {
        backup: true,
        ..plain_kind(KindId::Postgres, &POSTGRES, "blue", Some("postgres"), None)
    },
    KindDescriptor {
        label: "CockroachDB",
        short: "CR",
        pg_flavor: Some(PgFlavor::Cockroachdb),
        default_port: Some(26257),
        cargo_feature: PG_FLAVORS,
        ..plain_kind(
            KindId::Cockroachdb,
            &POSTGRES,
            "purple",
            Some("defaultdb"),
            Some("root"),
        )
    },
    KindDescriptor {
        label: "Greenplum",
        short: "GP",
        pg_flavor: Some(PgFlavor::Greenplum),
        cargo_feature: PG_FLAVORS,
        ..plain_kind(
            KindId::Greenplum,
            &POSTGRES,
            "green",
            Some("postgres"),
            None,
        )
    },
    KindDescriptor {
        label: "Amazon Redshift",
        short: "RS",
        pg_flavor: Some(PgFlavor::Redshift),
        default_port: Some(5439),
        cargo_feature: PG_FLAVORS,
        ..plain_kind(KindId::Redshift, &POSTGRES, "red", Some("dev"), None)
    },
    KindDescriptor {
        backup: true,
        ..plain_kind(KindId::Mysql, &MYSQL, "orange", None, None)
    },
    KindDescriptor {
        label: "MariaDB",
        short: "MA",
        mysql_version: Some(MysqlVersion::Mariadb),
        backup: true,
        ..plain_kind(KindId::Mariadb, &MYSQL, "purple", None, None)
    },
    // Backed up by copying the file.
    KindDescriptor {
        backup: true,
        ..plain_kind(KindId::Sqlite, &SQLITE, "teal", None, None)
    },
    plain_kind(KindId::Libsql, &LIBSQL, "teal", None, None),
    plain_kind(KindId::D1, &D1, "orange", None, None),
    plain_kind(KindId::Duckdb, &DUCKDB, "yellow", None, None),
    plain_kind(KindId::Sqlserver, &SQLSERVER, "red", Some("master"), None),
    plain_kind(
        KindId::Clickhouse,
        &CLICKHOUSE,
        "yellow",
        Some("default"),
        Some("default"),
    ),
    plain_kind(KindId::Snowflake, &SNOWFLAKE, "blue", None, None),
    plain_kind(KindId::Bigquery, &BIGQUERY, "blue", None, None),
    plain_kind(KindId::Vertica, &VERTICA, "gray", None, None),
    plain_kind(KindId::Cassandra, &CASSANDRA, "teal", None, None),
    plain_kind(KindId::Redis, &REDIS, "red", None, None),
    plain_kind(KindId::Mongodb, &MONGODB, "green", None, None),
    plain_kind(KindId::Elasticsearch, &ELASTICSEARCH, "yellow", None, None),
    plain_kind(KindId::Dynamodb, &DYNAMODB, "purple", None, None),
];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::types::profile::{ConnectionProfile, ProfileOptions, SslMode};

    /// The ids profiles store and IPC carries: changing one strands every saved connection.
    #[test]
    fn ids_are_stable() {
        let ids = |v: serde_json::Value| -> Vec<String> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(
            ids(serde_json::to_value(Driver::ALL).unwrap()),
            [
                "postgres",
                "mysql",
                "sqlite",
                "sqlserver",
                "libsql",
                "d1",
                "duckdb",
                "clickhouse",
                "snowflake",
                "bigquery",
                "vertica",
                "cassandra",
                "redis",
                "mongodb",
                "elasticsearch",
                "dynamodb",
            ]
        );
        assert_eq!(
            ids(serde_json::to_value([
                PgFlavor::Cockroachdb,
                PgFlavor::Greenplum,
                PgFlavor::Redshift
            ])
            .unwrap()),
            ["cockroachdb", "greenplum", "redshift"]
        );
        assert_eq!(
            ids(serde_json::to_value(KINDS.iter().map(|k| k.kind).collect::<Vec<_>>()).unwrap()),
            [
                "postgres",
                "cockroachdb",
                "greenplum",
                "redshift",
                "mysql",
                "mariadb",
                "sqlite",
                "libsql",
                "d1",
                "duckdb",
                "sqlserver",
                "clickhouse",
                "snowflake",
                "bigquery",
                "vertica",
                "cassandra",
                "redis",
                "mongodb",
                "elasticsearch",
                "dynamodb",
            ]
        );
        for d in Driver::ALL {
            let back: Driver = serde_json::from_value(serde_json::to_value(d).unwrap()).unwrap();
            assert_eq!(back, d);
        }
    }

    #[test]
    fn every_driver_and_kind_has_one_descriptor() {
        assert_eq!(Driver::ALL.into_iter().collect::<HashSet<_>>().len(), 16);
        for d in Driver::ALL {
            assert_eq!(d.descriptor().driver, d, "{d:?}");
            assert!(!d.label().is_empty() && d.descriptor().short.len() == 2);
            // Every driver has a kind of its own.
            assert!(KINDS.iter().any(|k| k.driver == d && k.pg_flavor.is_none()));
        }
        let kinds: HashSet<_> = KINDS.iter().map(|k| k.kind).collect();
        assert_eq!(kinds.len(), KINDS.len());
        for k in &KINDS {
            assert_eq!(k.kind.descriptor().kind, k.kind);
            assert_eq!(
                KindId::of(k.driver, k.pg_flavor, k.mysql_version),
                k.kind,
                "{:?}",
                k.kind
            );
            assert!(
                [
                    "red", "orange", "yellow", "green", "teal", "blue", "purple", "gray"
                ]
                .contains(&k.tint)
            );
        }
    }

    #[test]
    fn descriptors_are_consistent() {
        for d in Driver::ALL {
            let desc = d.descriptor();
            // A SQL dialect exactly for SQL sessions.
            assert_eq!(
                desc.dialect.is_none(),
                desc.session == SessionKind::Native,
                "{d:?}"
            );
            assert_eq!(desc.cargo_feature.is_none(), d.is_core(), "{d:?}");
            assert!(
                desc.core_reuse.is_empty(),
                "{d:?}: v1 reuses no core feature"
            );
            assert_eq!(
                desc.default_port.is_none(),
                desc.location != Location::Host,
                "{d:?}"
            );
            assert_eq!(desc.ssh == SshSupport::Sftp, d == Driver::Sqlite);
            assert!(desc.password_modes.contains(&PasswordMode::Keychain));
            if desc.password_modes.contains(&PasswordMode::Command) {
                assert!(desc.command_ttl_secs > 0, "{d:?}");
            }
            for scheme in desc.url_schemes {
                assert_eq!(
                    Driver::ALL
                        .iter()
                        .filter(|o| o.descriptor().url_schemes.contains(scheme))
                        .count(),
                    1,
                    "{scheme} names one driver"
                );
            }
        }
        for k in &KINDS {
            if k.cargo_feature.is_none() {
                assert!(k.driver.is_core(), "{:?}", k.kind);
            }
            if k.pg_flavor.is_none() {
                assert_eq!(k.cargo_feature, k.driver.descriptor().cargo_feature);
            }
        }
    }

    fn profile(driver: Driver) -> ConnectionProfile {
        ConnectionProfile {
            id: uuid::Uuid::nil(),
            name: "x".into(),
            color: None,
            group: None,
            driver,
            host: Some("db".into()),
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

    #[test]
    fn default_ports_follow_the_flavor_and_tls() {
        // The core drivers: as before.
        assert_eq!(profile(Driver::Postgres).default_port(), Some(5432));
        assert_eq!(profile(Driver::Mysql).default_port(), Some(3306));
        assert_eq!(profile(Driver::Sqlite).default_port(), None);
        for ssl in [SslMode::Disable, SslMode::Require, SslMode::VerifyFull] {
            let mut p = profile(Driver::Postgres);
            p.ssl = ssl;
            assert_eq!(p.default_port(), Some(5432));
        }
        let mut maria = profile(Driver::Mysql);
        maria.options.mysql_version = Some(MysqlVersion::Mariadb);
        assert_eq!(maria.kind(), KindId::Mariadb);
        assert_eq!(maria.default_port(), Some(3306));

        let flavored = |f: PgFlavor| {
            let mut p = profile(Driver::Postgres);
            p.options.pg_flavor = Some(f);
            p
        };
        assert_eq!(flavored(PgFlavor::Cockroachdb).default_port(), Some(26257));
        assert_eq!(flavored(PgFlavor::Greenplum).default_port(), Some(5432));
        assert_eq!(flavored(PgFlavor::Redshift).default_port(), Some(5439));
        assert_eq!(
            flavored(PgFlavor::Cockroachdb).default_database(),
            Some("defaultdb")
        );
        assert_eq!(flavored(PgFlavor::Redshift).default_database(), Some("dev"));
        assert_eq!(
            profile(Driver::Postgres).default_database(),
            Some("postgres")
        );
        assert_eq!(
            profile(Driver::Sqlserver).default_database(),
            Some("master")
        );

        let mut ch = profile(Driver::Clickhouse);
        ch.ssl = SslMode::Disable;
        assert_eq!(ch.default_port(), Some(8123));
        ch.ssl = SslMode::VerifyFull;
        assert_eq!(ch.default_port(), Some(8443));
        assert_eq!(profile(Driver::D1).default_port(), None);
        let mut libsql = profile(Driver::Libsql);
        assert_eq!(libsql.default_port(), Some(443));
        libsql.ssl = SslMode::Disable;
        assert_eq!(libsql.default_port(), Some(8080));
    }

    #[test]
    fn new_profile_options_are_left_out_unless_set() {
        // Profiles of the core drivers are stored as before.
        let json = serde_json::to_value(profile(Driver::Postgres).options).unwrap();
        assert!(json.get("pgFlavor").is_none() && json.get("driverParams").is_none());
        let options: ProfileOptions = serde_json::from_value(serde_json::json!({
            "pgFlavor": "redshift",
            "driverParams": { "accountId": "a1", "retries": 3 },
        }))
        .unwrap();
        assert_eq!(options.pg_flavor, Some(PgFlavor::Redshift));
        assert!(options.unknown.is_empty());
        let back = serde_json::to_value(&options).unwrap();
        assert_eq!(back["pgFlavor"], "redshift");
        assert_eq!(
            back["driverParams"],
            serde_json::json!({ "accountId": "a1", "retries": 3 })
        );
    }

    #[test]
    fn system_schema_patterns() {
        let pg = Driver::Postgres.descriptor();
        for (schema, system) in [
            ("public", false),
            ("audit", false),
            ("pg_catalog", true),
            ("pg_toast", true),
            ("pg_temp_3", true),
            ("information_schema", true),
            ("information_schema2", false),
        ] {
            assert_eq!(pg.is_system_schema(schema), system, "{schema}");
        }
        let cql = Driver::Cassandra.descriptor();
        assert!(cql.is_system_schema("system") && cql.is_system_schema("system_auth"));
        assert!(!cql.is_system_schema("systems"));
    }

    /// Writes `driverCatalog.ts` next to the ts-rs bindings: the descriptors the renderer reads
    /// synchronously (its tiles, labels and ports), keyed by driver and by kind. Its `satisfies`
    /// has tsc check each entry against the generated types and that none is missing, so the
    /// renderer needs no cast. Named like ts-rs's export tests so the same filters skip it.
    #[test]
    fn export_bindings_driver_catalog() {
        /// A JSON object in the order of its entries (serde_json's own map sorts the keys, and
        /// the kinds' order is the picker's).
        struct Keyed<K, V: 'static>(Vec<(K, &'static V)>);
        impl<K: Serialize, V: Serialize> Serialize for Keyed<K, V> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_map(self.0.iter().map(|(k, v)| (k, v)))
            }
        }
        #[derive(Serialize)]
        struct Catalog {
            drivers: Keyed<Driver, DriverDescriptor>,
            kinds: Keyed<KindId, KindDescriptor>,
        }
        let catalog = Catalog {
            drivers: Keyed(Driver::ALL.iter().map(|d| (*d, descriptor(*d))).collect()),
            kinds: Keyed(KINDS.iter().map(|k| (k.kind, k)).collect()),
        };
        let json = serde_json::to_string_pretty(&catalog).unwrap();
        let text = format!(
            "// This file was generated by the export_bindings_driver_catalog test \
             (src-tauri/src/types/driver.rs). Do not edit this file manually.\n\
             import type {{ DatabaseKind }} from \"./DatabaseKind\";\n\
             import type {{ Driver }} from \"./Driver\";\n\
             import type {{ DriverDescriptor }} from \"./DriverDescriptor\";\n\
             import type {{ KindDescriptor }} from \"./KindDescriptor\";\n\
             \n\
             /**\n * The facts of every driver and of every database of the new-connection picker \
             (in its order).\n */\n\
             export const DRIVER_CATALOG = {json} satisfies {{\n  \
             drivers: Record<Driver, DriverDescriptor>;\n  \
             kinds: Record<DatabaseKind, KindDescriptor>;\n}};\n"
        );
        let dir = std::env::var("TS_RS_EXPORT_DIR").unwrap_or_else(|_| "./bindings".into());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(std::path::Path::new(&dir).join("driverCatalog.ts"), text).unwrap();
    }
}
