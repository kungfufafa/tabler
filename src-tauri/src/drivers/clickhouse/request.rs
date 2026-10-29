//! The HTTP requests of a ClickHouse session: the SQL in a POST body, everything else in the URL
//! (database, query id, session, role, settings, bound values as `param_pN`) and in headers
//! (user and key, never in the URL, which proxies log; the output format).
//!
//! A read-only or unattended session sends `readonly=2` with every request: the server then
//! refuses every statement that writes, and still lets the request change its own settings
//! (`readonly=1` would refuse the output settings below). It goes first, so every later setting
//! is applied under it, and no setting of the profile may replace it.
//!
//! Safe Mode judges ClickHouse SQL as it reads: every request pins the language the server
//! parses and keeps reads of stream tables from consuming (`PINNED_SETTINGS`), over a `SET`
//! earlier in the session and over the user's server-side profile, and a connection's own
//! settings may name neither. Nor may they name a setting that reshapes every result (the
//! driver's catalog queries and edit guards too), holds SQL the server runs or makes a read reach
//! the server's files (`RESERVED`).

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

use crate::error::AppError;
use crate::types::value::Value;

use super::params::{param_text, placeholder_types};
use super::transport::{HttpRequest, Method, header};
use super::types::ChType;

/// Rows come as JSON arrays, one per line, after a line of names and a line of types.
pub const OUTPUT_FORMAT: &str = "JSONCompactEachRowWithNamesAndTypes";

/// What every request asks for when the user may change settings: integers of 64 bits and
/// more, decimals and non-finite floats quoted (so they arrive exact), invalid UTF-8 replaced,
/// and a read stopped when its connection closes (cancel and the row cap).
pub const OUTPUT_SETTINGS: &[(&str, &str)] = &[
    ("output_format_json_quote_64bit_integers", "1"),
    ("output_format_json_quote_decimals", "1"),
    ("output_format_json_quote_denormals", "1"),
    ("output_format_json_validate_utf8", "1"),
    ("cancel_http_readonly_queries_on_client_close", "1"),
];

/// Sent with every request that may carry settings (`Client::open` adds them to the
/// statement's): `dialect` other than `clickhouse` (kusto, prql, polyglot, trino, promql,
/// logsql, clickhouse_json) makes the server parse another language than the classifier reads,
/// and `stream_like_engine_allow_direct_select` makes a read of a Kafka, RabbitMQ, NATS, FileLog
/// or S3Queue table consume its messages. A URL setting wins over the session's (`SET`) and the
/// profile's; a statement's own SETTINGS clause still sets the second, and is a write. Every
/// supported server (23.3+) has both, and the constraints do not check a value that does not
/// change.
pub const PINNED_SETTINGS: &[(&str, &str)] = &[
    ("dialect", "clickhouse"),
    ("stream_like_engine_allow_direct_select", "0"),
];

/// URL parameters the driver sets itself, and settings that change what a statement means; a
/// profile setting may not name them.
const RESERVED: &[&str] = &[
    "query",
    "database",
    "default_format",
    "query_id",
    "session_id",
    "session_timeout",
    "session_check",
    "role",
    "user",
    "password",
    "quota_key",
    "readonly",
    // These change how the body is framed, which the reader would no longer understand.
    "compress",
    "decompress",
    "enable_http_compression",
    "wait_end_of_query",
    "send_progress_in_http_headers",
    "http_write_exception_in_output_format",
    "output_format_write_statistics",
    // 26.8+: the response's format, framing and compression.
    "output_format",
    "http_x_clickhouse_format_overrides_output_format",
    "framing_output_format",
    "compression",
    // The language the server parses, and what a read does (`PINNED_SETTINGS`).
    "dialect",
    "stream_like_engine_allow_direct_select",
    // A query without SELECT (`implicit_select`) or without FROM reading another table than
    // `system.one` (`implicit_table_at_top_level`, the driver's `SELECT version()` too).
    "implicit_select",
    "implicit_table_at_top_level",
    // 26.8+: settings that wrap every result-producing query (`SELECT <select> FROM (…) WHERE
    // <filter> ORDER BY <order> LIMIT … OFFSET …`) or replace its formats, the driver's catalog
    // queries and the edit guard's `SELECT count()` too (`select = '1'` would make every count
    // 1); `limit` and `offset` are older.
    "select",
    "filter",
    "order",
    "sort",
    "limit",
    "offset",
    "page",
    "format",
    "input_format",
    // Filters over every result or a table's rows (the guard would count fewer rows than the
    // edit changes), an expression run on every row, a query run for a format's schema: SQL
    // the server runs that Safe Mode never reads.
    "additional_result_filter",
    "additional_table_filters",
    "parallel_replicas_custom_key",
    "format_schema_source",
    // A read in a format of the user's FORMAT clause reads schema and template files of the
    // server or writes a schema file; one that parses CSV or TSV writes its errors to a file.
    "format_schema",
    "format_template_resultset",
    "format_template_row",
    "output_format_schema",
    "input_format_record_errors_file_path",
    // Replaces the response headers the driver reads (the exception tag, the summary).
    "http_response_headers",
    // Cached results: a guard count up to a minute old, and every `system.*` query refused
    // (`query_cache_system_table_handling`).
    "use_query_cache",
    // A count of other rows than the edit changes: FINAL merges the versions a mutation still
    // rewrites one by one (a SummingMergeTree key of two parts counts 1), and a replica's share
    // of a sampled table is a fraction of it.
    "final",
    "parallel_replicas_count",
    "parallel_replica_offset",
    // Reads of existing URL and S3 tables with relative URLs go to the host they name.
    "url_base",
    "s3_base",
];

/// `*_overflow_mode` and `*_overflow_mode_leaf`: `break` (or `any`) returns what was read so far
/// as if it were the whole result, a guard count included.
fn is_overflow_mode(name: &str) -> bool {
    name.ends_with("_overflow_mode") || name.ends_with("_overflow_mode_leaf")
}

/// Characters escaped in a query string component: everything but letters, digits and `-._~`.
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

fn enc(s: &str) -> String {
    utf8_percent_encode(s, COMPONENT).to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub tls: bool,
    /// As connected to (a tunnel's `127.0.0.1` too: the transport keeps the real host for TLS).
    pub host: String,
    pub port: u16,
}

impl Endpoint {
    /// `http(s)://host:port/`.
    pub fn base_url(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        let host = if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        format!("{scheme}://{host}:{}/", self.port)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum Credentials {
    /// `X-ClickHouse-User` and `X-ClickHouse-Key`.
    Password { user: String, password: String },
    /// X.509: the client certificate proves who `user` is (`IDENTIFIED WITH ssl_certificate`).
    Certificate { user: String },
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Credentials::Password { user, .. } => write!(f, "Password({user}, ***)"),
            Credentials::Certificate { user } => write!(f, "Certificate({user})"),
        }
    }
}

impl Credentials {
    pub fn user(&self) -> &str {
        match self {
            Credentials::Password { user, .. } | Credentials::Certificate { user } => user,
        }
    }

    fn headers(&self, out: &mut Vec<(String, String)>) {
        let user = if self.user().is_empty() {
            "default"
        } else {
            self.user()
        };
        out.push((header::USER.into(), user.into()));
        match self {
            Credentials::Password { password, .. } => {
                out.push((header::KEY.into(), password.clone()));
            }
            Credentials::Certificate { .. } => out.push((header::CERT_AUTH.into(), "on".into())),
        }
    }
}

/// An HTTP session (the editor's): `SET` and temporary tables persist across its requests,
/// which run one at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpSession {
    pub id: String,
    /// Seconds idle before the server drops it (60 by default).
    pub timeout_secs: Option<u32>,
}

/// What a connection sends with every request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnSettings {
    pub database: Option<String>,
    /// `readonly=2` on every request.
    pub read_only: bool,
    /// `role=` (ClickHouse 24.4+).
    pub roles: Vec<String>,
    /// The profile's own settings (validated by `validate_settings`).
    pub settings: Vec<(String, String)>,
    /// Whether `OUTPUT_SETTINGS` may be sent: false for a user whose profile is `readonly=1`,
    /// who may change no setting (then the decoders read the server's defaults).
    pub output_settings: bool,
}

/// One statement to send.
#[derive(Debug, Clone, Default)]
pub struct Query<'a> {
    pub sql: &'a str,
    /// The values of the placeholders `{p1:…}`, `{p2:…}`… in order.
    pub params: &'a [Value],
    /// Named parameters of fixed catalog queries (`{db:String}` with `("db", "x")`).
    pub named: &'a [(&'a str, &'a str)],
    pub query_id: Option<&'a str>,
    /// Sent in the editor's session.
    pub session: Option<&'a HttpSession>,
    /// Settings of this statement only (`mutations_sync=2` for an edit).
    pub settings: &'a [(&'a str, &'a str)],
}

/// Refuses a profile setting the driver sets itself, or a name that is not a setting's.
pub fn validate_settings(settings: &[(String, String)]) -> Result<(), AppError> {
    for (k, _) in settings {
        let k = k.trim();
        let valid = !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !valid {
            return Err(AppError::InvalidInput(format!(
                "\u{201c}{k}\u{201d} is not a ClickHouse setting name"
            )));
        }
        let lower = k.to_ascii_lowercase();
        if RESERVED.contains(&lower.as_str())
            || lower.starts_with("param_")
            || is_overflow_mode(&lower)
        {
            return Err(AppError::InvalidInput(format!(
                "\u{201c}{k}\u{201d} is set by Tabler or changes how statements are read, so it \
                 cannot be a connection setting"
            )));
        }
    }
    Ok(())
}

/// The POST of `q`.
pub fn build(
    endpoint: &Endpoint,
    creds: &Credentials,
    conn: &ConnSettings,
    q: &Query<'_>,
) -> Result<HttpRequest, AppError> {
    validate_settings(&conn.settings)?;
    let mut pairs: Vec<(String, String)> = Vec::new();
    if conn.read_only {
        pairs.push(("readonly".into(), "2".into()));
    }
    if let Some(db) = conn.database.as_deref().filter(|d| !d.is_empty()) {
        pairs.push(("database".into(), db.into()));
    }
    if let Some(id) = q.query_id {
        pairs.push(("query_id".into(), id.into()));
    }
    if let Some(s) = q.session {
        pairs.push(("session_id".into(), s.id.clone()));
        if let Some(t) = s.timeout_secs {
            pairs.push(("session_timeout".into(), t.to_string()));
        }
    }
    pairs.push(("default_format".into(), OUTPUT_FORMAT.into()));
    for role in &conn.roles {
        pairs.push(("role".into(), role.clone()));
    }
    // The statement's own settings and the output settings win over the profile's.
    let ours: Vec<&str> = q
        .settings
        .iter()
        .map(|(k, _)| *k)
        .chain(OUTPUT_SETTINGS.iter().map(|(k, _)| *k))
        .collect();
    for (k, v) in &conn.settings {
        if !ours.iter().any(|o| o.eq_ignore_ascii_case(k.trim())) {
            pairs.push((k.trim().to_string(), v.clone()));
        }
    }
    if conn.output_settings {
        pairs.extend(
            OUTPUT_SETTINGS
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string())),
        );
    }
    for (k, v) in q.settings {
        pairs.push(((*k).to_string(), (*v).to_string()));
    }
    pairs.extend(bound_params(q.sql, q.params)?);
    for (k, v) in q.named {
        pairs.push((format!("param_{k}"), (*v).to_string()));
    }
    let query: Vec<String> = pairs
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect();
    let mut headers = Vec::new();
    creds.headers(&mut headers);
    // From 26.8 this header beats a FORMAT clause in the statement; before, it is the default.
    headers.push((header::FORMAT.into(), OUTPUT_FORMAT.into()));
    headers.push(("content-type".into(), "text/plain; charset=utf-8".into()));
    Ok(HttpRequest {
        method: Method::Post,
        url: format!("{}?{}", endpoint.base_url(), query.join("&")),
        headers,
        body: q.sql.as_bytes().to_vec(),
    })
}

/// `param_pN` for each placeholder of `sql`, in the text form of its type.
fn bound_params(sql: &str, params: &[Value]) -> Result<Vec<(String, String)>, AppError> {
    let types = placeholder_types(sql);
    let mut out = Vec::with_capacity(types.len());
    for (n, ty) in types {
        let value = n
            .checked_sub(1)
            .and_then(|i| params.get(i))
            .ok_or_else(|| {
                AppError::Internal(format!(
                    "the statement uses parameter {n} but has {}",
                    params.len()
                ))
            })?;
        out.push((
            format!("param_p{n}"),
            param_text(value, &ChType::parse(&ty))?,
        ));
    }
    Ok(out)
}

/// `GET /ping`: free, no credentials, answers `Ok.`.
pub fn ping(endpoint: &Endpoint) -> HttpRequest {
    HttpRequest {
        method: Method::Get,
        url: format!("{}ping", endpoint.base_url()),
        headers: Vec::new(),
        body: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint() -> Endpoint {
        Endpoint {
            tls: false,
            host: "ch.example".into(),
            port: 8123,
        }
    }

    fn creds() -> Credentials {
        Credentials::Password {
            user: "alice".into(),
            password: "p@ss&word".into(),
        }
    }

    #[test]
    fn credentials_travel_in_headers_and_settings_in_the_url() {
        let conn = ConnSettings {
            database: Some("my db".into()),
            read_only: true,
            roles: vec!["analyst".into()],
            settings: vec![
                ("max_threads".into(), "4".into()),
                ("output_format_json_quote_decimals".into(), "0".into()),
            ],
            output_settings: true,
        };
        let session = HttpSession {
            id: "s-1".into(),
            timeout_secs: Some(600),
        };
        let params = [Value::Text("a&b=c".into()), Value::Null];
        let req = build(
            &endpoint(),
            &creds(),
            &conn,
            &Query {
                sql: "SELECT {p1:String}, {p2:Nullable(UInt8)}, {db:String}",
                params: &params,
                named: &[("db", "x y")],
                query_id: Some("q-1"),
                session: Some(&session),
                settings: &[("mutations_sync", "2")],
            },
        )
        .unwrap();
        assert_eq!(req.method, Method::Post);
        assert!(req.url.starts_with("http://ch.example:8123/?readonly=2&"));
        assert!(!req.url.contains("alice") && !req.url.contains("p%40ss"));
        assert_eq!(req.header(header::USER), Some("alice"));
        assert_eq!(req.header(header::KEY), Some("p@ss&word"));
        assert_eq!(req.header(header::FORMAT), Some(OUTPUT_FORMAT));
        let pairs = req.query_pairs();
        let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "readonly",
                "database",
                "query_id",
                "session_id",
                "session_timeout",
                "default_format",
                "role",
                "max_threads",
                "output_format_json_quote_64bit_integers",
                "output_format_json_quote_decimals",
                "output_format_json_quote_denormals",
                "output_format_json_validate_utf8",
                "cancel_http_readonly_queries_on_client_close",
                "mutations_sync",
                "param_p1",
                "param_p2",
                "param_db",
            ]
        );
        assert_eq!(req.param("database").as_deref(), Some("my db"));
        assert_eq!(req.param("param_p1").as_deref(), Some("a&b=c"));
        assert_eq!(req.param("param_p2").as_deref(), Some("\\N"));
        assert_eq!(req.param("param_db").as_deref(), Some("x y"));
        // The profile's own value of an output setting is dropped, not sent twice.
        assert_eq!(
            req.param("output_format_json_quote_decimals").as_deref(),
            Some("1")
        );
        assert_eq!(
            req.sql(),
            "SELECT {p1:String}, {p2:Nullable(UInt8)}, {db:String}"
        );
    }

    #[test]
    fn certificates_and_profiles_that_change_nothing() {
        let conn = ConnSettings {
            output_settings: false,
            ..ConnSettings::default()
        };
        let req = build(
            &Endpoint {
                tls: true,
                host: "::1".into(),
                port: 8443,
            },
            &Credentials::Certificate {
                user: String::new(),
            },
            &conn,
            &Query {
                sql: "SELECT 1",
                ..Query::default()
            },
        )
        .unwrap();
        assert!(req.url.starts_with("https://[::1]:8443/?default_format="));
        assert_eq!(req.header(header::USER), Some("default"));
        assert_eq!(req.header(header::CERT_AUTH), Some("on"));
        assert_eq!(req.header(header::KEY), None);
        assert!(!req.url.contains("output_format") && !req.url.contains("readonly"));
        assert_eq!(ping(&endpoint()).url, "http://ch.example:8123/ping");
    }

    #[test]
    fn profile_settings_cannot_replace_the_drivers() {
        for bad in [
            "readonly",
            "READONLY",
            "database",
            "param_p1",
            "compress",
            "a-b",
            "",
            "x y",
            // Another language than the classifier reads (polyglot, trino, kusto, prql…).
            "dialect",
            "Dialect",
            "stream_like_engine_allow_direct_select",
            "implicit_select",
            "implicit_table_at_top_level",
            "output_format",
            "framing_output_format",
            "compression",
            // What the driver's own queries return (red team, round 3).
            "select",
            "Filter",
            "order",
            "sort",
            "limit",
            "offset",
            "page",
            "format",
            "input_format",
            "additional_result_filter",
            "additional_table_filters",
            "parallel_replicas_custom_key",
            "format_schema_source",
            "format_schema",
            "format_template_row",
            "Output_Format_Schema",
            "input_format_record_errors_file_path",
            "http_response_headers",
            "use_query_cache",
            "read_overflow_mode",
            "timeout_overflow_mode_leaf",
            "GROUP_BY_OVERFLOW_MODE",
            "final",
            "parallel_replicas_count",
            "parallel_replica_offset",
            "url_base",
            "S3_BASE",
        ] {
            let conn = ConnSettings {
                settings: vec![(bad.into(), "0".into())],
                ..ConnSettings::default()
            };
            assert!(
                build(&endpoint(), &creds(), &conn, &Query::default()).is_err(),
                "{bad}"
            );
        }
        for good in ["max_execution_time", "max_rows_to_read", "overflow_mode_x"] {
            assert!(
                validate_settings(&[(good.into(), "30".into())]).is_ok(),
                "{good}"
            );
        }
        assert!(
            validate_settings(&[("dialect".into(), "polyglot".into())])
                .unwrap_err()
                .to_string()
                .contains("changes how statements are read")
        );
    }

    #[test]
    fn a_missing_parameter_is_an_error() {
        let err = build(
            &endpoint(),
            &creds(),
            &ConnSettings::default(),
            &Query {
                sql: "SELECT {p2:String}",
                params: &[Value::Int(1)],
                ..Query::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("parameter 2"), "{err}");
    }
}
