//! One connection of a TablePlus export as a Tabler profile and its secrets.
//!
//! The record is TablePlus's own (`Connection.ExportingDictionary` on Windows; the macOS export
//! uses the same keys plus a few of its own: `DatabasePath`, `DatabaseSocket`, `isUseSocket`,
//! `SafeModeLevel`…). The meaning of every number below was read from TablePlus for Windows
//! (its `SQLDriver` enum, connection forms and drivers); values a key takes on one database
//! mean something else on another (`tLSMode`, `DriverVersion`, `DatabaseHost`), so the reading
//! goes database by database.
//!
//! What does not come across as it was becomes a note the import preview shows on the
//! connection; a database Tabler does not have makes the connection a skipped one.

use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::profiles_io::{ExportedConnection, ExportedSecrets};
use crate::types::driver::{PgFlavor, StartupKind};
use crate::types::profile::{
    ConnectionProfile, ConnectionTag, Driver, DriverParams, MysqlVersion, PasswordMode,
    ProfileOptions, SafeModeLevel, SshAuth, SshConfig, SshPasswordMode, SslMode, SslNegotiation,
    TAG_NAME_MAX,
};

/// A connection read from the export.
#[derive(Debug, Clone)]
pub struct Converted {
    pub connection: ExportedConnection,
    /// What the preview tells the user about it.
    pub notes: Vec<String>,
    /// The text of the SSH private key TablePlus exported with it (`PrivateKeyData`): the import
    /// writes it to a key file of its own, since profiles keep a key path.
    pub ssh_key: Option<String>,
}

type Record = Map<String, Value>;

/// A trimmed, non-empty text value (a number reads as its digits).
fn text(o: &Record, key: &str) -> Option<String> {
    let s = match o.get(key)? {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    (!s.is_empty()).then_some(s)
}

/// The first of `keys` with a text value.
fn first_text(o: &Record, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| text(o, k))
}

/// An integer: TablePlus writes numbers, older or hand-made files strings or booleans.
fn int(o: &Record, key: &str) -> Option<i64> {
    match o.get(key)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" => Some(1),
            "false" | "no" => Some(0),
            s => s.parse().ok(),
        },
        Value::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

fn flag(o: &Record, key: &str) -> bool {
    int(o, key).is_some_and(|v| v != 0)
}

/// A port: 1..=65535, or none (blank, 0, anything else).
fn port_value(o: &Record, key: &str) -> Option<u16> {
    int(o, key)
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p > 0)
}

/// Whether a `TlsKeyPaths` entry names a file. The Windows forms keep the buttons' captions
/// ("Key...", "Trusted Cert...") in the same list when no file was chosen.
fn is_path(s: &str) -> bool {
    let b = s.as_bytes();
    s.starts_with('/')
        || s.starts_with('~')
        || s.starts_with("\\\\")
        || (b.len() > 2
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'\\' | b'/'))
}

/// A path on a local disk (`C:\…` or `/…`): not a network share, not `~`. What Tabler's BigQuery
/// key file must be (the same test as `drivers::bigquery::params::local_file_path`).
fn local_file_path(path: &str) -> bool {
    let sep = |c: char| c == '\\' || c == '/';
    let mut c = path.chars();
    match (c.next(), c.next(), c.next()) {
        (Some(d), Some(':'), Some(s)) => d.is_ascii_alphabetic() && sep(s),
        (Some(s), Some(n), _) => sep(s) && !sep(n) && n != '?',
        _ => false,
    }
}

/// Whether a PEM file holds a private key: `None` when it cannot be read here (another
/// computer's path). Reads at most the first 256 KB.
fn pem_has_key(path: &str) -> Option<bool> {
    use std::io::Read as _;
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(256 * 1024)
        .read_to_string(&mut text)
        .ok()?;
    Some(text.contains("PRIVATE KEY-----"))
}

/// `TlsKeyPaths[index]`, when it names a file.
fn tls_path(o: &Record, index: usize) -> Option<String> {
    let s = o
        .get("TlsKeyPaths")?
        .as_array()?
        .get(index)?
        .as_str()?
        .trim();
    is_path(s).then(|| s.to_string())
}

/// What TablePlus calls the database: its `Driver` string, read as TablePlus reads it (any
/// case; spaces, dashes and underscores ignored).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Postgres(Option<PgFlavor>),
    Mysql,
    Mariadb,
    Sqlite,
    Duckdb,
    Sqlserver,
    Vertica,
    Clickhouse,
    Bigquery,
    Libsql,
    D1,
    Dynamodb,
    Elasticsearch,
    Cassandra,
    Redis,
    Mongodb,
    Snowflake,
}

impl Kind {
    fn of(name: &str) -> Option<Kind> {
        let key: String = name
            .chars()
            .filter(|c| !c.is_whitespace() && !matches!(c, '-' | '_'))
            .flat_map(char::to_lowercase)
            .collect();
        Some(match key.as_str() {
            "postgresql" | "postgres" => Kind::Postgres(None),
            "greenplum" => Kind::Postgres(Some(PgFlavor::Greenplum)),
            "cockroach" | "cockroachdb" => Kind::Postgres(Some(PgFlavor::Cockroachdb)),
            "redshift" | "amazonredshift" => Kind::Postgres(Some(PgFlavor::Redshift)),
            "mysql" => Kind::Mysql,
            "mariadb" => Kind::Mariadb,
            "sqlite" => Kind::Sqlite,
            "duckdb" => Kind::Duckdb,
            "microsoftsqlserver" | "sqlserver" | "mssql" => Kind::Sqlserver,
            "vertica" => Kind::Vertica,
            "clickhouse" => Kind::Clickhouse,
            "bigquery" | "googlebigquery" => Kind::Bigquery,
            "libsql" => Kind::Libsql,
            "cloudflared1" | "d1" => Kind::D1,
            "dynamodb" | "dynamo" => Kind::Dynamodb,
            "elasticsearch" | "opensearch" => Kind::Elasticsearch,
            "cassandra" | "scylladb" => Kind::Cassandra,
            "redis" | "valkey" => Kind::Redis,
            "mongodb" | "mongo" => Kind::Mongodb,
            "snowflake" => Kind::Snowflake,
            _ => return None,
        })
    }

    /// The id Tabler stores for the driver (`Driver`'s serde name): this module names the lane
    /// drivers through their ids, not their variants (`tests/driver_guard.rs`).
    fn driver_id(self) -> &'static str {
        match self {
            Kind::Postgres(_) => "postgres",
            Kind::Mysql | Kind::Mariadb => "mysql",
            Kind::Sqlite => "sqlite",
            Kind::Duckdb => "duckdb",
            Kind::Sqlserver => "sqlserver",
            Kind::Vertica => "vertica",
            Kind::Clickhouse => "clickhouse",
            Kind::Bigquery => "bigquery",
            Kind::Libsql => "libsql",
            Kind::D1 => "d1",
            Kind::Dynamodb => "dynamodb",
            Kind::Elasticsearch => "elasticsearch",
            Kind::Cassandra => "cassandra",
            Kind::Redis => "redis",
            Kind::Mongodb => "mongodb",
            Kind::Snowflake => "snowflake",
        }
    }

    fn driver(self) -> Driver {
        serde_json::from_value(Value::from(self.driver_id())).expect("a driver id Tabler stores")
    }
}

/// The profile colour: the theme swatch TablePlus's `statusColorIndex` picks (0–4, the same
/// five Tabler has as presets), else its `statusColor` hex (`#RRGGBB`, or WPF's `#AARRGGBB`).
fn color(o: &Record) -> Option<String> {
    let preset = match int(o, "statusColorIndex") {
        Some(0) => Some("green"),
        Some(1) => Some("gray"),
        Some(2) => Some("blue"),
        Some(3) => Some("yellow"),
        Some(4) => Some("red"),
        _ => None,
    };
    if let Some(p) = preset {
        return Some(p.to_string());
    }
    let hex = text(o, "statusColor")?;
    let hex = hex.trim_start_matches('#');
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let hex = match hex.len() {
        6 => hex,
        8 => &hex[2..],
        _ => return None,
    };
    hex.chars()
        .all(|c| c.is_ascii_hexdigit())
        .then(|| format!("#{}", hex.to_ascii_lowercase()))
}

/// The environment tag (`Enviroment`, sic; `Environment` read too). TablePlus's built-in tags
/// are Tabler's; anything else is a custom tag.
fn tag(o: &Record) -> Option<ConnectionTag> {
    let name = first_text(o, &["Enviroment", "Environment"])?;
    // A WPF control's name a TablePlus for Windows build once saved for "no tag".
    if name.starts_with("System.Windows.") {
        return None;
    }
    let name: String = name.chars().take(TAG_NAME_MAX).collect();
    Some(ConnectionTag::from_name(&name))
}

/// `SafeMode` / `AdvancedSafeMode` (Windows) or `SafeModeLevel` / `AdvancedSafeModeLevel`
/// (macOS): 0 silent, 1 alert, 2 safe (password); the advanced flag skips reads.
fn safe_mode(o: &Record, notes: &mut Vec<String>) -> Option<SafeModeLevel> {
    let mode = int(o, "SafeMode").or_else(|| int(o, "SafeModeLevel"))?;
    let skip_reads = int(o, "AdvancedSafeMode")
        .or_else(|| int(o, "AdvancedSafeModeLevel"))
        .is_some_and(|v| v == 1);
    Some(match (mode, skip_reads) {
        (0, _) => SafeModeLevel::Silent,
        (1, false) => SafeModeLevel::Alert,
        (1, true) => SafeModeLevel::AlertNonSelect,
        (2, false) => SafeModeLevel::Safe,
        (2, true) => SafeModeLevel::SafeNonSelect,
        _ => {
            notes.push("Its TablePlus safe mode is one Tabler does not know: it asks for the password before every query".into());
            SafeModeLevel::Safe
        }
    })
}

/// The SSL mode a `tLSMode` index stands for, database by database (each TablePlus form lists
/// its own modes).
fn ssl_mode(kind: Kind, o: &Record, notes: &mut Vec<String>) -> SslMode {
    let mode = int(o, "tLSMode").unwrap_or(0);
    let mut review = |what: &str| {
        notes.push(format!(
            "Check its SSL mode: TablePlus's “{what}” became the closest Tabler mode"
        ))
    };
    match kind {
        Kind::Postgres(_) | Kind::Vertica => match mode {
            1 => SslMode::Disable,
            2 => SslMode::Require,
            3 => {
                review("ALLOW");
                SslMode::Prefer
            }
            4 => SslMode::VerifyCa,
            5 => SslMode::VerifyFull,
            _ => SslMode::Prefer,
        },
        Kind::Mysql => match mode {
            1 => SslMode::Disable,
            2 => SslMode::Require,
            3 => {
                review("ALLOW");
                SslMode::VerifyCa
            }
            4 => SslMode::VerifyFull,
            5 => {
                review("VERIFY-IDENTITY");
                SslMode::VerifyFull
            }
            _ => SslMode::Prefer,
        },
        // PREFERRED, ENFORCE, VERIFY-SERVER-CERT: Tabler's MariaDB list.
        Kind::Mariadb => match mode {
            1 => SslMode::Require,
            2 => SslMode::VerifyFull,
            _ => SslMode::Prefer,
        },
        // NO SSL (the connection string decides), USE SSL, USE SSL allowing any certificate.
        Kind::Mongodb => match mode {
            1 => SslMode::VerifyFull,
            2 => SslMode::Require,
            _ => SslMode::Prefer,
        },
        Kind::Clickhouse => match mode {
            1 => SslMode::VerifyFull,
            2 => SslMode::Require,
            _ => SslMode::Disable,
        },
        // SSL off, or on without verifying the certificate.
        Kind::Redis => {
            if mode > 0 {
                SslMode::Require
            } else {
                SslMode::Disable
            }
        }
        // The form shows PostgreSQL's names; the driver turns TLS on for every mode but the
        // first, verifying as listed.
        Kind::Cassandra => match mode {
            1 => {
                review("DISABLED (which still used TLS)");
                SslMode::Require
            }
            2 => SslMode::VerifyCa,
            3 | 4 => SslMode::VerifyFull,
            5 => {
                review("VERIFY-FULL");
                SslMode::VerifyFull
            }
            _ => SslMode::Prefer,
        },
        // Disabled, system CAs, a custom CA, any certificate.
        Kind::Elasticsearch => match mode {
            0 => SslMode::Disable,
            3 => SslMode::Require,
            _ => SslMode::VerifyFull,
        },
        _ => SslMode::Prefer,
    }
}

/// Splits `scheme://rest` (lowercased scheme).
fn split_scheme(url: &str) -> (Option<String>, &str) {
    match url.split_once("://") {
        Some((scheme, rest))
            if !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '+') =>
        {
            (Some(scheme.to_ascii_lowercase()), rest)
        }
        _ => (None, url),
    }
}

/// `host[:port][/path]` of a URL without its scheme.
fn host_port_path(rest: &str) -> (String, Option<u16>, Option<String>) {
    let (authority, path) = match rest.find('/') {
        Some(i) => (
            &rest[..i],
            Some(rest[i..].trim_end_matches('/').to_string()),
        ),
        None => (rest, None),
    };
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => match p.parse() {
            Ok(port) => (h, Some(port)),
            Err(_) => (authority, None),
        },
        _ => (authority, None),
    };
    (
        host.trim_matches(|c| c == '[' || c == ']').to_string(),
        port,
        path.filter(|p| !p.is_empty()),
    )
}

/// Percent-decodes URL text (invalid escapes stay as they are).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Query options of a MongoDB connection string that carry a secret: Tabler keeps those out of
/// its saved URIs (and refuses one that has them).
const MONGO_SECRET_OPTIONS: [&str; 3] = [
    "tlscertificatekeyfilepassword",
    "proxypassword",
    // Holds AWS_SESSION_TOKEN for MONGODB-AWS.
    "authmechanismproperties",
];

/// A MongoDB connection string, split.
struct MongoUri {
    /// What Tabler keeps: no user, password or secret option in it.
    uri: String,
    user: Option<String>,
    password: Option<String>,
    /// Its one server (`mongodb://` with a single host; port 27017 when it names none): what an
    /// SSH tunnel goes to.
    node: Option<(String, u16)>,
}

/// Splits a MongoDB connection string (see [`MongoUri`]).
fn mongo_uri(uri: &str, notes: &mut Vec<String>) -> Option<MongoUri> {
    let (scheme, rest) = split_scheme(uri);
    let scheme = scheme.filter(|s| s == "mongodb" || s == "mongodb+srv")?;
    let (before_query, query) = match rest.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (rest, None),
    };
    let (userinfo, hosts) = match before_query.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, before_query),
    };
    let (user, password) = match userinfo {
        Some(u) => match u.split_once(':') {
            Some((name, pw)) => (Some(percent_decode(name)), Some(percent_decode(pw))),
            None => (Some(percent_decode(u)), None),
        },
        None => (None, None),
    };
    let mut kept = Vec::new();
    for pair in query
        .unwrap_or_default()
        .split('&')
        .filter(|p| !p.is_empty())
    {
        let name = pair
            .split('=')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if MONGO_SECRET_OPTIONS.contains(&name.as_str()) {
            notes.push(format!(
                "Its connection string had a secret option ({}), which is not imported: enter it again in the connection's settings",
                pair.split('=').next().unwrap_or_default()
            ));
        } else {
            kept.push(pair);
        }
    }
    let mut out = format!("{scheme}://{hosts}");
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    let servers = hosts.split('/').next().unwrap_or_default();
    let node = (scheme == "mongodb" && !servers.is_empty() && !servers.contains(','))
        .then(|| host_port_path(servers))
        .map(|(host, port, _)| (host, port.unwrap_or(27017)));
    Some(MongoUri {
        uri: out,
        user: user.filter(|u| !u.is_empty()),
        password: password.filter(|p| !p.is_empty()),
        node,
    })
}

fn params(map: Map<String, Value>) -> Option<DriverParams> {
    (!map.is_empty()).then_some(DriverParams(map))
}

/// Reads one connection record. `Err` is why it cannot be imported.
pub fn connection(o: &Record, id: Uuid, group: &str) -> Result<Converted, String> {
    let driver_name = text(o, "Driver").unwrap_or_default();
    let Some(kind) = Kind::of(&driver_name) else {
        return Err(if driver_name.is_empty() {
            "has no database type".into()
        } else {
            format!("is a {driver_name} connection, which Tabler does not support")
        });
    };
    if kind == Kind::Redis && int(o, "DriverVersion") == Some(1) {
        return Err("uses Redis Sentinel, which Tabler does not support".into());
    }
    let driver = kind.driver();
    let descriptor = driver.descriptor();
    let mut notes = Vec::new();
    let mut secrets = ExportedSecrets::default();
    let mut options = ProfileOptions {
        tag: tag(o),
        safe_mode: safe_mode(o, &mut notes),
        sqlite_foreign_keys: true,
        ..ProfileOptions::default()
    };
    let mut driver_params = Map::new();

    let mut host = text(o, "DatabaseHost");
    let mut port = port_value(o, "DatabasePort");
    let mut user = text(o, "DatabaseUser");
    let mut database = text(o, "DatabaseName");
    let mut sqlite_path = None;
    let mut ssl = ssl_mode(kind, o, &mut notes);
    let mut ssl_ca_path = None;
    let db_password = o
        .get("DatabasePassword")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .map(str::to_string);
    // What the database password is for: TablePlus keeps a MongoDB connection's whole
    // connection string there.
    let mut password = db_password.clone();

    // ----- the password's storage --------------------------------------------------------
    let password_mode = match int(o, "DatabasePasswordMode").unwrap_or(0) {
        1 => PasswordMode::Ask,
        2 => PasswordMode::None,
        3 => PasswordMode::Command,
        _ => PasswordMode::Keychain,
    };
    options.password_mode = Some(password_mode);

    // Whether it ends up going through an SSH tunnel (see the SSH part below).
    let over_ssh = flag(o, "isOverSSH") && text(o, "ServerAddress").is_some();

    // ----- per database ------------------------------------------------------------------
    match kind {
        Kind::Postgres(flavor) => {
            options.pg_flavor = flavor;
            if flavor.is_none() && int(o, "tLSNegotiation") == Some(1) {
                if matches!(
                    ssl,
                    SslMode::Require | SslMode::VerifyCa | SslMode::VerifyFull
                ) {
                    options.pg_ssl_negotiation = Some(SslNegotiation::Direct);
                } else {
                    notes.push("Its direct SSL negotiation needs SSL mode Require or stricter in Tabler: it uses the usual negotiation".into());
                }
            }
        }
        Kind::Vertica => {}
        Kind::Mysql | Kind::Mariadb => {
            if kind == Kind::Mariadb {
                options.mysql_version = Some(MysqlVersion::Mariadb);
            }
            if flag(o, "UseSock") {
                // Windows: the pipe or socket name is in the host field.
                options.use_socket = Some(true);
                options.socket_path = host.take();
                notes.push("It connects through a socket: check the socket file in its settings (a Windows named pipe does not work here)".into());
            } else if flag(o, "isUseSocket") {
                // macOS: its own field; empty, the usual paths are tried.
                options.use_socket = Some(true);
                options.socket_path = text(o, "DatabaseSocket");
                host = None;
            }
            let other: Vec<String> = o
                .get("OtherOptions")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            for opt in &other {
                match opt.as_str() {
                    "LIBMYSQL_ENABLE_CLEARTEXT_PLUGIN" => {
                        options.mysql_cleartext_plugin = Some(true)
                    }
                    "MYSQL_OPT_LOCAL_INFILE" => options.mysql_local_infile = Some(true),
                    "" => {}
                    other => notes.push(format!(
                        "Its TablePlus option {other} has no Tabler equivalent"
                    )),
                }
            }
            if let Some(sni) = text(o, "ServerNameIndication") {
                notes.push(format!(
                    "Its SNI host name ({sni}) is not used: Tabler sends the host name"
                ));
            }
        }
        Kind::Sqlite | Kind::Duckdb => {
            // macOS keeps the file in DatabasePath, Windows in DatabaseHost.
            sqlite_path = text(o, "DatabasePath").or_else(|| host.take());
            host = None;
            port = None;
            user = None;
            database = None;
            if kind == Kind::Duckdb
                && text(o, "DatabaseType").is_some_and(|t| t.eq_ignore_ascii_case("read_only"))
            {
                options.read_only = true;
            }
            if kind == Kind::Sqlite
                && let Some(key) = o
                    .get("DatabaseKeyPassword")
                    .and_then(Value::as_str)
                    .filter(|k| !k.is_empty())
            {
                password = Some(key.to_string());
            }
        }
        Kind::Sqlserver => {
            if let Some(h) = host.take() {
                match h.split_once('\\') {
                    Some((server, instance)) => {
                        host = Some(server.trim().to_string()).filter(|s| !s.is_empty());
                        let instance = instance.trim();
                        if !instance.is_empty() {
                            driver_params.insert("instance".into(), json!(instance));
                        }
                    }
                    None => host = Some(h),
                }
            }
            let role = text(o, "DatabaseUserRole")
                .unwrap_or_default()
                .to_lowercase();
            let authenticator = text(o, "Authenticator").unwrap_or_default().to_uppercase();
            let auth = if authenticator.starts_with("MICROSOFT_ENTRA_ID") || role.contains("entra")
            {
                if !authenticator.ends_with("TOKEN_ONLY") && !role.contains("token only") {
                    notes.push("It signed in with Microsoft Entra ID in a browser: Tabler takes an Entra access token instead (from a command or typed in)".into());
                }
                "entraToken"
            } else if role.contains("window") {
                notes.push(
                    "It uses Windows authentication, which works in Tabler for Windows only".into(),
                );
                "integrated"
            } else {
                "sqlServer"
            };
            driver_params.insert("auth".into(), json!(auth));
        }
        Kind::Clickhouse => {
            let http = int(o, "DriverVersion") == Some(1);
            if let Some(h) = host.take() {
                let (scheme, rest) = split_scheme(&h);
                let (name, url_port, _) = host_port_path(rest);
                host = Some(name).filter(|n| !n.is_empty());
                port = port.or(url_port);
                if http {
                    ssl = if scheme.as_deref() == Some("https") {
                        SslMode::VerifyFull
                    } else {
                        SslMode::Disable
                    };
                }
            }
            if !http {
                // Tabler talks to ClickHouse over HTTP: the native ports are not its.
                if port.is_none_or(|p| p == 9000 || p == 9440) {
                    port = None;
                    notes.push("TablePlus used ClickHouse's native protocol; Tabler uses its HTTP interface (port 8123, or 8443 with SSL)".into());
                } else {
                    notes.push("TablePlus used ClickHouse's native protocol; Tabler uses its HTTP interface: check the port".into());
                }
            }
            ssl_ca_path = tls_path(o, 0);
        }
        Kind::Bigquery => {
            // Project id in the host field, default dataset in the database field.
            database = host.take();
            port = None;
            user = None;
            if let Some(dataset) = text(o, "DatabaseName") {
                driver_params.insert("defaultDataset".into(), json!(dataset));
            }
            // Tabler reads the key file from a local disk only (no network share, no `~`).
            match tls_path(o, 0).filter(|p| local_file_path(p)) {
                Some(key_file) => {
                    driver_params.insert("auth".into(), json!("serviceAccount"));
                    driver_params.insert("keyFilePath".into(), json!(key_file));
                }
                None => {
                    driver_params.insert("auth".into(), json!("applicationDefault"));
                    notes.push("Its service account key file did not come across: choose one on this computer in the connection's settings".into());
                }
            }
        }
        Kind::Libsql => {
            if let Some(h) = host.take() {
                let url = if split_scheme(&h).0.is_some() {
                    h
                } else {
                    format!("http://{h}")
                };
                let (scheme, rest) = split_scheme(&url);
                let plain = matches!(scheme.as_deref(), Some("http") | Some("ws"));
                ssl = if plain {
                    SslMode::Disable
                } else {
                    SslMode::VerifyFull
                };
                if over_ssh {
                    // A tunnel goes to a host and port; Tabler refuses a URL over SSH.
                    let (name, url_port, _) = host_port_path(rest);
                    host = Some(name).filter(|n| !n.is_empty());
                    port = url_port.or(Some(if plain { 80 } else { 443 }));
                } else {
                    driver_params.insert("url".into(), json!(url));
                    port = None;
                }
            } else {
                port = None;
            }
            database = None;
        }
        Kind::D1 => {
            if let Some(account) = host.take() {
                driver_params.insert("accountId".into(), json!(account));
            }
            port = None;
            user = None;
        }
        Kind::Dynamodb => {
            // Region in the host field, access key id as the user, secret key as the password.
            let region = host.take();
            if let Some(r) = region.as_deref().filter(|r| r.contains("://")) {
                // An emulator's address (DynamoDB Local takes any region).
                driver_params.insert("endpoint".into(), json!(r));
                database = Some("us-east-1".into());
                notes.push(
                    "Its endpoint is a URL: it uses the region us-east-1, check it in its settings"
                        .into(),
                );
            } else {
                database = region;
            }
            port = None;
            driver_params.insert("auth".into(), json!("accessKey"));
            if database.as_deref() == Some("dynamodb") {
                database = None;
            }
        }
        Kind::Elasticsearch => {
            if let Some(h) = host.take() {
                let (scheme, rest) = split_scheme(&h);
                let (name, url_port, path) = host_port_path(rest);
                host = Some(name).filter(|n| !n.is_empty());
                port = url_port.or(port);
                if scheme.as_deref() == Some("http") {
                    ssl = SslMode::Disable;
                }
                if let Some(path) = path {
                    driver_params.insert("pathPrefix".into(), json!(path));
                }
            }
            if ssl == SslMode::VerifyFull {
                ssl_ca_path = tls_path(o, 0);
            }
            let basic = user.is_some() || password.is_some();
            driver_params.insert("auth".into(), json!(if basic { "basic" } else { "none" }));
            database = None;
        }
        Kind::Cassandra => {
            if let Some(h) = host.take() {
                let mut points = h.split(',').map(str::trim).filter(|p| !p.is_empty());
                host = points.next().map(str::to_string);
                if points.next().is_some() {
                    notes.push(format!("TablePlus listed several contact points ({h}); Tabler connects to the first"));
                }
            }
        }
        Kind::Redis => {
            // TablePlus's database field is a fixed caption, not a DB index.
            database = None;
        }
        Kind::Mongodb => {
            // TablePlus keeps the whole connection string in the password field.
            match db_password
                .as_deref()
                .and_then(|uri| mongo_uri(uri, &mut notes))
            {
                Some(parsed) => {
                    driver_params.insert("uri".into(), json!(parsed.uri));
                    user = parsed.user.or(user);
                    password = parsed.password;
                    // Over SSH the tunnel goes to the URI's server, which the profile names.
                    match parsed.node.filter(|_| over_ssh) {
                        Some((node_host, node_port)) => {
                            host = Some(node_host);
                            port = Some(node_port);
                        }
                        None => {
                            host = None;
                            port = None;
                        }
                    }
                }
                None => {
                    password = None;
                    notes.push("Its connection string was not in the export (it is saved with the passwords): check the host and enter the password".into());
                }
            }
            // TablePlus hands the first file to the driver as the client certificate and key,
            // the second as the CA; the Windows form's one button ("Trusted Cert...") fills only
            // the first. A file without a private key in it is a CA.
            match tls_path(o, 0) {
                Some(first) if pem_has_key(&first) == Some(false) => {
                    notes.push("Its trusted certificate file is used as the CA file".into());
                    ssl_ca_path = Some(first);
                }
                Some(first) => {
                    if pem_has_key(&first).is_none() {
                        notes.push(format!("Its certificate file ({first}) is not on this computer: check it is the client certificate with its key, not a CA"));
                    }
                    options.ssl_cert_path = Some(first);
                }
                None => {}
            }
            if let Some(ca) = tls_path(o, 1) {
                ssl_ca_path = Some(ca);
            }
        }
        Kind::Snowflake => {
            if let Some(account) = host.take() {
                driver_params.insert("account".into(), json!(account));
            }
            port = None;
            if let Some(role) = text(o, "DatabaseUserRole") {
                driver_params.insert("role".into(), json!(role));
            }
            if let Some(warehouse) = text(o, "DatabaseWarehouse") {
                driver_params.insert("warehouse".into(), json!(warehouse));
            }
            let authenticator = text(o, "Authenticator").unwrap_or_default().to_lowercase();
            let auth = match authenticator.as_str() {
                "" | "snowflake" => "password",
                "oauth" => "oauth",
                "programmatic_access_token" => "pat",
                "snowflake_jwt" => {
                    notes.push("It signs in with a key pair: choose the private key file in the connection's settings".into());
                    "keyPair"
                }
                other => {
                    notes.push(format!("Its sign-in method ({other}) is not available in Tabler: it signs in with the password"));
                    "password"
                }
            };
            driver_params.insert("auth".into(), json!(auth));
        }
    }

    // ----- TLS files (key, certificate, CA) of the databases whose form has the three -------
    if matches!(
        kind,
        Kind::Postgres(_) | Kind::Vertica | Kind::Mysql | Kind::Mariadb | Kind::Cassandra
    ) {
        options.ssl_key_path = tls_path(o, 0);
        options.ssl_cert_path = tls_path(o, 1);
        ssl_ca_path = tls_path(o, 2);
    }
    if text(o, "TlsKeyPassword").is_some() {
        notes.push(
            "Its SSL key password is not imported: Tabler needs an unencrypted key file".into(),
        );
    }
    if text(o, "TlsKeyName").is_some() {
        notes.push("Choose its SSL certificate files again in the connection's settings".into());
    }

    // ----- the password itself --------------------------------------------------------------
    match password_mode {
        PasswordMode::Keychain => secrets.password = password,
        PasswordMode::Command => {
            // The command TablePlus runs is in the password field.
            options.password_command = db_password.filter(|_| kind != Kind::Mongodb);
            if options.password_command.is_none() {
                options.password_mode = Some(PasswordMode::Ask);
                notes.push("It got its password from a command that was not in the export: Tabler asks for the password".into());
            }
        }
        PasswordMode::Ask | PasswordMode::None => {}
    }
    // A mode this database does not offer (a token is always needed, an encryption key never
    // comes from a command): asked on connect instead.
    if let Some(mode) = options.password_mode
        && !descriptor.password_modes.contains(&mode)
    {
        options.password_mode = Some(PasswordMode::Ask);
        options.password_command = None;
        notes.push("Its password setting is not available for this database: Tabler asks for it on connect".into());
    }

    // ----- SSH -------------------------------------------------------------------------------
    let mut ssh_key = None;
    let ssh = if flag(o, "isOverSSH") {
        match text(o, "ServerAddress") {
            None => {
                notes.push(
                    "It was set to connect over SSH, but has no SSH server: it connects directly"
                        .into(),
                );
                None
            }
            Some(ssh_host) => {
                let key_auth = flag(o, "isUsePrivateKey");
                let mut key_path = None;
                if key_auth {
                    let data = o
                        .get("PrivateKeyData")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|d| !d.is_empty());
                    match data {
                        Some(d) if d.starts_with("PuTTY") => notes.push("Its SSH key is a PuTTY key: convert it to OpenSSH format (PuTTYgen ▸ Export OpenSSH key) and choose it in the connection's settings".into()),
                        Some(d) => ssh_key = Some(format!("{d}\n")),
                        None => match text(o, "ServerPrivateKeyName").filter(|n| is_path(n)) {
                            Some(path) => key_path = Some(path),
                            None => notes.push("Its SSH private key was not in the export: choose it in the connection's settings (until then, Tabler tries your default SSH keys)".into()),
                        },
                    }
                    secrets.ssh_passphrase = o
                        .get("PrivateKeyPass")
                        .and_then(Value::as_str)
                        .filter(|p| !p.is_empty())
                        .map(str::to_string);
                }
                let password_mode = match int(o, "ServerPasswordMode").unwrap_or(0) {
                    1 => SshPasswordMode::Ask,
                    2 => SshPasswordMode::None,
                    _ => SshPasswordMode::Keychain,
                };
                if !key_auth && password_mode == SshPasswordMode::Keychain {
                    secrets.ssh_password = o
                        .get("ServerPassword")
                        .and_then(Value::as_str)
                        .filter(|p| !p.is_empty())
                        .map(str::to_string);
                }
                Some(SshConfig {
                    host: ssh_host,
                    port: port_value(o, "ServerPort").unwrap_or(22),
                    user: text(o, "ServerUser").unwrap_or_default(),
                    auth: if key_auth {
                        SshAuth::Key
                    } else {
                        SshAuth::Password
                    },
                    key_path,
                    local_port: None,
                    keepalive_secs: None,
                    password_mode: (!key_auth).then_some(password_mode),
                })
            }
        }
    } else {
        None
    };

    // ----- what every database shares -------------------------------------------------------
    if let Some(sql) = text(o, "StartupCommands") {
        if descriptor.startup == StartupKind::None {
            notes.push(
                "Its startup commands are not imported: Tabler does not run any for this database"
                    .into(),
            );
        } else {
            options.startup_sql = Some(sql);
        }
    }
    if text(o, "StartupBashScript").is_some() {
        notes.push(
            "Its startup shell script is not imported: add it again under Advanced if you need it"
                .into(),
        );
    }
    if descriptor.encoding {
        options.encoding = text(o, "DatabaseEncoding");
    }
    options.default_row_limit = int(o, "LimitRowsReturned")
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0);
    if flag(o, "LazyLoadSchemaItems") {
        options.lazy_load = Some(true);
    }
    options.driver_params = params(driver_params);

    let profile = ConnectionProfile {
        id,
        name: text(o, "ConnectionName").unwrap_or_else(|| "Untitled".into()),
        color: color(o),
        group: Some(group.to_string()).filter(|g| !g.is_empty()),
        driver,
        host,
        port,
        user,
        database,
        ssl,
        ssl_ca_path,
        sqlite_path,
        ssh,
        options,
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    };
    Ok(Converted {
        connection: ExportedConnection { profile, secrets },
        notes,
        ssh_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(v: Value) -> Record {
        v.as_object().unwrap().clone()
    }

    fn convert(v: Value) -> Converted {
        connection(&rec(v), Uuid::nil(), "Team").unwrap()
    }

    #[test]
    fn postgres_over_ssh_with_passwords() {
        let c = convert(json!({
            "ConnectionName": "Orders", "Driver": "PostgreSQL", "Enviroment": "production",
            "DatabaseHost": "db.example.test", "DatabasePort": "5433", "DatabaseUser": "app",
            "DatabaseName": "orders", "DatabasePassword": "pw", "DatabasePasswordMode": 0,
            "tLSMode": 5, "tLSNegotiation": 1,
            "TlsKeyPaths": ["/k/client.key", "/k/client.crt", "Trusted Cert..."],
            "isOverSSH": 1, "ServerAddress": "bastion", "ServerPort": "2222",
            "ServerUser": "deploy", "ServerPassword": "ssh-pw", "ServerPasswordMode": 0,
            "isUsePrivateKey": 0, "statusColor": "#A3793E", "statusColorIndex": 3,
            "SafeMode": 2, "AdvancedSafeMode": 1, "StartupCommands": "SET x = 1;",
            "LimitRowsReturned": 500, "DatabaseEncoding": "UTF8"
        }));
        let p = &c.connection.profile;
        assert_eq!(p.driver, Driver::Postgres);
        assert_eq!(
            (p.host.as_deref(), p.port),
            (Some("db.example.test"), Some(5433))
        );
        assert_eq!(p.ssl, SslMode::VerifyFull);
        assert_eq!(p.options.pg_ssl_negotiation, Some(SslNegotiation::Direct));
        assert_eq!(p.options.ssl_key_path.as_deref(), Some("/k/client.key"));
        assert_eq!(p.options.ssl_cert_path.as_deref(), Some("/k/client.crt"));
        assert_eq!(p.ssl_ca_path, None, "a button caption is not a file");
        assert_eq!(p.color.as_deref(), Some("yellow"));
        assert_eq!(p.group.as_deref(), Some("Team"));
        assert_eq!(p.options.tag, Some(ConnectionTag::Production));
        assert_eq!(p.options.safe_mode, Some(SafeModeLevel::SafeNonSelect));
        assert_eq!(p.options.startup_sql.as_deref(), Some("SET x = 1;"));
        assert_eq!(p.options.default_row_limit, Some(500));
        assert_eq!(p.options.encoding.as_deref(), Some("UTF8"));
        let ssh = p.ssh.as_ref().unwrap();
        assert_eq!(
            (ssh.host.as_str(), ssh.port, ssh.user.as_str()),
            ("bastion", 2222, "deploy")
        );
        assert_eq!(ssh.auth, SshAuth::Password);
        assert_eq!(c.connection.secrets.password.as_deref(), Some("pw"));
        assert_eq!(c.connection.secrets.ssh_password.as_deref(), Some("ssh-pw"));
        assert!(c.notes.is_empty(), "{:?}", c.notes);
    }

    #[test]
    fn database_names_as_tableplus_writes_them() {
        for (name, driver, flavor) in [
            ("Cockroach", Driver::Postgres, Some(PgFlavor::Cockroachdb)),
            ("Greenplum", Driver::Postgres, Some(PgFlavor::Greenplum)),
            ("Redshift", Driver::Postgres, Some(PgFlavor::Redshift)),
            ("MicrosoftSQLServer", Driver::Sqlserver, None),
            ("CloudflareD1", Driver::D1, None),
            ("ElasticSearch", Driver::Elasticsearch, None),
            ("LibSQL", Driver::Libsql, None),
            ("DuckDB", Driver::Duckdb, None),
        ] {
            let p = convert(json!({"ConnectionName": "x", "Driver": name}))
                .connection
                .profile;
            assert_eq!((p.driver, p.options.pg_flavor), (driver, flavor), "{name}");
        }
        let maria = convert(json!({"Driver": "MariaDB", "tLSMode": 1}))
            .connection
            .profile;
        assert_eq!(maria.options.mysql_version, Some(MysqlVersion::Mariadb));
        assert_eq!(maria.ssl, SslMode::Require, "MariaDB's ENFORCE");
    }

    #[test]
    fn unsupported_databases_are_skipped() {
        let err = |v| connection(&rec(v), Uuid::nil(), "").unwrap_err();
        assert!(err(json!({"Driver": "Oracle"})).contains("Oracle"));
        assert!(err(json!({})).contains("no database type"));
        assert!(err(json!({"Driver": "Redis", "DriverVersion": 1})).contains("Sentinel"));
    }

    #[test]
    fn sqlite_and_duckdb_files() {
        let win = convert(
            json!({"Driver": "SQLite", "DatabaseHost": "C:\\d\\a.sqlite", "DatabaseName": "a.sqlite"}),
        );
        let p = &win.connection.profile;
        assert_eq!(p.sqlite_path.as_deref(), Some("C:\\d\\a.sqlite"));
        assert_eq!((p.host.as_deref(), p.database.as_deref()), (None, None));
        let mac = convert(
            json!({"Driver": "SQLite", "DatabasePath": "/Users/me/a.db", "DatabaseKeyPassword": "k"}),
        );
        assert_eq!(
            mac.connection.profile.sqlite_path.as_deref(),
            Some("/Users/me/a.db")
        );
        assert_eq!(mac.connection.secrets.password.as_deref(), Some("k"));
        let duck = convert(
            json!({"Driver": "DuckDB", "DatabaseHost": "/d/x.duckdb", "DatabaseType": "read_only"}),
        );
        assert!(duck.connection.profile.options.read_only);
    }

    #[test]
    fn password_modes() {
        let mode = |m: i64, pw: &str| {
            let c = convert(
                json!({"Driver": "MySQL", "DatabasePasswordMode": m, "DatabasePassword": pw}),
            );
            (
                c.connection.profile.options.password_mode,
                c.connection.profile.options.password_command,
                c.connection.secrets.password,
            )
        };
        assert_eq!(
            mode(0, "pw"),
            (Some(PasswordMode::Keychain), None, Some("pw".into()))
        );
        assert_eq!(mode(1, "pw"), (Some(PasswordMode::Ask), None, None));
        assert_eq!(mode(2, ""), (Some(PasswordMode::None), None, None));
        assert_eq!(
            mode(3, "aws rds token"),
            (
                Some(PasswordMode::Command),
                Some("aws rds token".into()),
                None
            )
        );
        assert_eq!(mode(3, "").0, Some(PasswordMode::Ask));
    }

    #[test]
    fn ssh_keys() {
        let c = convert(json!({
            "Driver": "PostgreSQL", "isOverSSH": 1, "ServerAddress": "b", "ServerPort": "",
            "isUsePrivateKey": 1, "PrivateKeyData": "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----",
            "PrivateKeyPass": "phrase", "ServerPassword": "ignored"
        }));
        let ssh = c.connection.profile.ssh.unwrap();
        assert_eq!((ssh.auth, ssh.port, ssh.key_path), (SshAuth::Key, 22, None));
        assert!(
            c.ssh_key
                .unwrap()
                .ends_with("-----END OPENSSH PRIVATE KEY-----\n")
        );
        assert_eq!(
            c.connection.secrets.ssh_passphrase.as_deref(),
            Some("phrase")
        );
        assert_eq!(c.connection.secrets.ssh_password, None);

        let putty = convert(
            json!({"Driver": "MySQL", "isOverSSH": 1, "ServerAddress": "b", "isUsePrivateKey": 1, "PrivateKeyData": "PuTTY-User-Key-File-3: ssh-ed25519"}),
        );
        assert!(putty.ssh_key.is_none());
        assert!(putty.notes[0].contains("PuTTY"));

        let none = convert(
            json!({"Driver": "MySQL", "isOverSSH": 1, "ServerAddress": "", "ServerUser": "x"}),
        );
        assert!(none.connection.profile.ssh.is_none());
    }

    #[test]
    fn mongodb_connection_string() {
        let c = convert(json!({
            "Driver": "MongoDB", "DatabaseHost": "h", "DatabaseName": "app",
            "DatabasePassword": "mongodb+srv://ann:p%40ss@cluster0.example.net/app?retryWrites=true&proxyPassword=x"
        }));
        let p = &c.connection.profile;
        let uri = p.options.driver_params.as_ref().unwrap().0["uri"]
            .as_str()
            .unwrap();
        assert_eq!(
            uri,
            "mongodb+srv://cluster0.example.net/app?retryWrites=true"
        );
        assert_eq!(p.user.as_deref(), Some("ann"));
        assert_eq!(c.connection.secrets.password.as_deref(), Some("p@ss"));
        assert_eq!(p.host, None);
        assert_eq!(c.notes.len(), 1, "the dropped proxyPassword");
    }

    #[test]
    fn cloud_databases() {
        let sf = convert(
            json!({"Driver": "Snowflake", "DatabaseHost": "xy123.eu", "DatabaseUserRole": "ANALYST", "DatabaseWarehouse": "WH", "Authenticator": "", "DatabaseName": "DB"}),
        );
        let params = &sf.connection.profile.options.driver_params.unwrap().0;
        assert_eq!(params["account"], "xy123.eu");
        assert_eq!(params["auth"], "password");
        assert_eq!(
            (params["role"].as_str(), params["warehouse"].as_str()),
            (Some("ANALYST"), Some("WH"))
        );

        let bq = convert(
            json!({"Driver": "BigQuery", "DatabaseHost": "my-project", "DatabaseName": "sales", "TlsKeyPaths": ["C:\\keys\\sa.json"]}),
        );
        let p = bq.connection.profile;
        assert_eq!(p.database.as_deref(), Some("my-project"));
        let params = p.options.driver_params.unwrap().0;
        assert_eq!(
            (
                params["defaultDataset"].as_str(),
                params["keyFilePath"].as_str()
            ),
            (Some("sales"), Some("C:\\keys\\sa.json"))
        );

        let d1 = convert(
            json!({"Driver": "CloudflareD1", "DatabaseHost": "acc", "DatabaseName": "dbid", "DatabasePassword": "tok"}),
        );
        assert_eq!(
            d1.connection.profile.options.driver_params.unwrap().0["accountId"],
            "acc"
        );
        assert_eq!(d1.connection.secrets.password.as_deref(), Some("tok"));

        let ls = convert(
            json!({"Driver": "LibSQL", "DatabaseHost": "libsql://db-me.turso.io", "DatabaseName": "LibSQL"}),
        );
        assert_eq!(ls.connection.profile.ssl, SslMode::VerifyFull);
        assert_eq!(ls.connection.profile.database, None);

        let ms = convert(
            json!({"Driver": "MicrosoftSQLServer", "DatabaseHost": "srv\\SQLEXPRESS", "DatabaseUserRole": "Default"}),
        );
        let p = ms.connection.profile;
        assert_eq!(p.host.as_deref(), Some("srv"));
        let params = p.options.driver_params.unwrap().0;
        assert_eq!(
            (params["instance"].as_str(), params["auth"].as_str()),
            (Some("SQLEXPRESS"), Some("sqlServer"))
        );

        let es = convert(
            json!({"Driver": "ElasticSearch", "DatabaseHost": "https://es.local:9243/proxy", "DatabasePort": "9200", "tLSMode": 1}),
        );
        let p = es.connection.profile;
        assert_eq!(
            (p.host.as_deref(), p.port, p.ssl),
            (Some("es.local"), Some(9243), SslMode::VerifyFull)
        );
        assert_eq!(p.options.driver_params.unwrap().0["pathPrefix"], "/proxy");
    }

    #[test]
    fn clickhouse_native_becomes_http() {
        let c = convert(
            json!({"Driver": "ClickHouse", "DatabaseHost": "ch", "DatabasePort": "9000", "DriverVersion": 0}),
        );
        assert_eq!(c.connection.profile.port, None);
        assert_eq!(c.notes.len(), 1);
        let http = convert(
            json!({"Driver": "ClickHouse", "DatabaseHost": "https://ch", "DatabasePort": "8443", "DriverVersion": 1}),
        );
        assert_eq!(
            (http.connection.profile.port, http.connection.profile.ssl),
            (Some(8443), SslMode::VerifyFull)
        );
    }

    #[test]
    fn colors_tags_and_odd_values() {
        let c = convert(
            json!({"Driver": "MySQL", "statusColor": "#FF336699", "statusColorIndex": -999, "Enviroment": "Client A", "DatabasePort": 3307, "isOverSSH": "false"}),
        );
        let p = c.connection.profile;
        assert_eq!(p.color.as_deref(), Some("#336699"));
        assert_eq!(
            p.options.tag,
            Some(ConnectionTag::Custom("Client A".into()))
        );
        assert_eq!(p.port, Some(3307));
        assert!(p.ssh.is_none());
        assert_eq!(p.name, "Untitled");
        let none =
            convert(json!({"Driver": "MySQL", "Enviroment": "System.Windows.Controls.Separator"}));
        assert_eq!(none.connection.profile.options.tag, None);
    }

    #[test]
    fn mongodb_over_ssh_names_the_tunnel_target() {
        let c = convert(json!({
            "Driver": "MongoDB", "isOverSSH": 1, "ServerAddress": "bastion",
            "DatabasePassword": "mongodb://u:p@db.internal/app?authMechanismProperties=AWS_SESSION_TOKEN:x"
        }));
        let p = &c.connection.profile;
        assert_eq!(
            (p.host.as_deref(), p.port),
            (Some("db.internal"), Some(27017))
        );
        let uri = p.options.driver_params.as_ref().unwrap().0["uri"]
            .as_str()
            .unwrap();
        assert_eq!(
            uri, "mongodb://db.internal/app",
            "the session token is a secret"
        );
        // Without SSH (or with several servers) the URI alone says where to go.
        let srv = convert(
            json!({"Driver": "MongoDB", "DatabasePassword": "mongodb+srv://c.example.net"}),
        );
        assert_eq!(srv.connection.profile.host, None);
    }

    #[test]
    fn mongodb_trusted_certificate_is_the_ca() {
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(
            &ca,
            "-----BEGIN CERTIFICATE-----\nx\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        let client = dir.path().join("client.pem");
        std::fs::write(
            &client,
            "-----BEGIN PRIVATE KEY-----\nx\n-----END PRIVATE KEY-----\n",
        )
        .unwrap();
        let p = |paths: Value| {
            convert(json!({"Driver": "MongoDB", "TlsKeyPaths": paths, "tLSMode": 1}))
                .connection
                .profile
        };
        let one = p(json!([ca.to_str().unwrap()]));
        assert_eq!(one.ssl_ca_path.as_deref(), ca.to_str());
        assert_eq!(one.options.ssl_cert_path, None);
        let two = p(json!([client.to_str().unwrap(), ca.to_str().unwrap()]));
        assert_eq!(two.options.ssl_cert_path.as_deref(), client.to_str());
        assert_eq!(two.ssl_ca_path.as_deref(), ca.to_str());
    }

    #[test]
    fn libsql_over_ssh_uses_host_and_port() {
        let c = convert(json!({
            "Driver": "LibSQL", "DatabaseHost": "https://db.example.com",
            "isOverSSH": 1, "ServerAddress": "bastion"
        }));
        let p = &c.connection.profile;
        assert_eq!(
            (p.host.as_deref(), p.port),
            (Some("db.example.com"), Some(443))
        );
        assert_eq!(p.options.driver_params, None);
        assert_eq!(p.ssl, SslMode::VerifyFull);
    }

    #[test]
    fn macos_socket_without_a_path_uses_the_usual_ones() {
        let c = convert(json!({
            "Driver": "MySQL", "isUseSocket": 1, "DatabaseSocket": "", "DatabaseHost": "127.0.0.1"
        }));
        let o = &c.connection.profile.options;
        assert_eq!((o.use_socket, o.socket_path.as_deref()), (Some(true), None));
        assert_eq!(c.connection.profile.host, None);
        let win = convert(json!({"Driver": "MySQL", "UseSock": 1, "DatabaseHost": "MySQL"}));
        assert_eq!(
            win.connection.profile.options.socket_path.as_deref(),
            Some("MySQL")
        );
    }

    #[test]
    fn password_modes_the_database_does_not_offer() {
        // Cloudflare D1 always needs its token.
        let c = convert(
            json!({"Driver": "CloudflareD1", "DatabaseHost": "a", "DatabasePasswordMode": 2}),
        );
        assert_eq!(
            c.connection.profile.options.password_mode,
            Some(PasswordMode::Ask)
        );
        assert_eq!(c.notes.len(), 1);
    }

    #[test]
    fn odd_inputs_do_not_panic() {
        let c =
            convert(json!({"Driver": "MySQL", "statusColor": "#ééé", "statusColorIndex": -999}));
        assert_eq!(c.connection.profile.color, None);
        let dynamo =
            convert(json!({"Driver": "DynamoDB", "DatabaseHost": "http://localhost:8000"}));
        assert_eq!(
            dynamo.connection.profile.database.as_deref(),
            Some("us-east-1")
        );
        let bq = convert(
            json!({"Driver": "BigQuery", "DatabaseHost": "p", "TlsKeyPaths": ["\\\\share\\sa.json"]}),
        );
        let params = bq.connection.profile.options.driver_params.unwrap().0;
        assert_eq!(params["auth"], "applicationDefault");
    }

    #[test]
    fn every_database_maps_to_a_stored_driver_id() {
        for name in [
            "PostgreSQL",
            "Greenplum",
            "Cockroach",
            "Redshift",
            "MySQL",
            "MariaDB",
            "SQLite",
            "DuckDB",
            "MicrosoftSQLServer",
            "Vertica",
            "ClickHouse",
            "BigQuery",
            "LibSQL",
            "CloudflareD1",
            "DynamoDB",
            "ElasticSearch",
            "Cassandra",
            "Redis",
            "MongoDB",
            "Snowflake",
        ] {
            let kind = Kind::of(name).unwrap();
            assert_eq!(kind.driver().descriptor().driver, kind.driver(), "{name}");
        }
    }

    #[test]
    fn helpers() {
        assert_eq!(percent_decode("a%40b%zz%4"), "a@b%zz%4");
        assert!(is_path("C:\\x") && is_path("/x") && is_path("~/x") && !is_path("Key..."));
        assert_eq!(
            host_port_path("[::1]:9200/p/"),
            ("::1".into(), Some(9200), Some("/p".into()))
        );
    }
}
