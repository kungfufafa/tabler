//! Cloudflare's D1 REST API: the endpoints, request bodies and the response envelope, and its
//! errors as `AppError`s.
//!
//! `POST …/d1/database/{id}/raw` (rows as arrays, so `SELECT a.id, b.id` keeps both columns),
//! `GET …/d1/database` (the account's databases, for the switcher and name lookup) and
//! `GET …/d1/database/{id}` (version and size).

use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::drivers::sqlite_http::codes::split_code_suffix;
use crate::drivers::sqlite_http::transport::{HttpResponse, body_excerpt, status_error};
use crate::error::{AppError, ErrorPos};

pub const ENGINE: &str = "Cloudflare D1";
pub const DEFAULT_API_BASE: &str = "https://api.cloudflare.com/client/v4";
/// Bound parameters per statement.
pub const MAX_PARAMS: usize = 100;
/// Bytes per statement.
pub const MAX_STATEMENT_BYTES: usize = 100_000;

/// Which database of which account, through which API base (tests point it at a fake).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct D1Target {
    pub api_base: String,
    pub account_id: String,
    /// The database's UUID (a name is resolved to it first).
    pub database_id: String,
}

impl D1Target {
    pub fn new(
        api_base: Option<&str>,
        account_id: &str,
        database_id: &str,
    ) -> Result<D1Target, AppError> {
        validate_account_id(account_id)?;
        if database_id.trim().is_empty() {
            return Err(AppError::InvalidInput("choose a D1 database".into()));
        }
        Ok(D1Target {
            api_base: api_base
                .map(str::trim)
                .filter(|b| !b.is_empty())
                .unwrap_or(DEFAULT_API_BASE)
                .trim_end_matches('/')
                .to_string(),
            account_id: account_id.trim().to_string(),
            database_id: database_id.trim().to_string(),
        })
    }

    pub fn raw_url(&self) -> Result<String, AppError> {
        url(
            &self.api_base,
            &[
                "accounts",
                &self.account_id,
                "d1",
                "database",
                &self.database_id,
                "raw",
            ],
            &[],
        )
    }

    pub fn database_url(&self) -> Result<String, AppError> {
        url(
            &self.api_base,
            &[
                "accounts",
                &self.account_id,
                "d1",
                "database",
                &self.database_id,
            ],
            &[],
        )
    }

    /// The same account and API base, another database.
    pub fn with_database(&self, database_id: &str) -> D1Target {
        D1Target {
            database_id: database_id.to_string(),
            ..self.clone()
        }
    }
}

/// One page of the account's databases, those named `name` only when given.
pub fn list_url(
    api_base: &str,
    account_id: &str,
    name: Option<&str>,
    page: u32,
    per_page: u32,
) -> Result<String, AppError> {
    let page = page.to_string();
    let per_page = per_page.to_string();
    let mut query = vec![("page", page.as_str()), ("per_page", per_page.as_str())];
    if let Some(name) = name {
        query.push(("name", name));
    }
    url(
        api_base,
        &["accounts", account_id, "d1", "database"],
        &query,
    )
}

/// `base` with path segments (each percent-encoded) and query pairs appended.
fn url(base: &str, segments: &[&str], query: &[(&str, &str)]) -> Result<String, AppError> {
    let mut url = Url::parse(base)
        .map_err(|e| AppError::InvalidInput(format!("invalid D1 API base URL {base}: {e}")))?;
    url.path_segments_mut()
        .map_err(|_| AppError::InvalidInput(format!("invalid D1 API base URL {base}")))?
        .pop_if_empty()
        .extend(segments);
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    Ok(url.to_string())
}

/// An account ID: 32 hexadecimal characters.
pub fn validate_account_id(id: &str) -> Result<(), AppError> {
    let id = id.trim();
    if id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "the Cloudflare account ID is 32 hexadecimal characters (Workers & Pages \u{25b8} Overview)"
                .into(),
        ))
    }
}

/// A database UUID, as opposed to a name.
pub fn is_uuid(s: &str) -> bool {
    let groups: Vec<&str> = s.trim().split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// One statement of a `/raw` call.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RawQuery {
    pub sql: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<serde_json::Value>,
}

/// A `/raw` body: one statement, or a batch D1 runs in one transaction.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum RawBody {
    Single(RawQuery),
    Batch { batch: Vec<RawQuery> },
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
pub struct ApiMessage {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
pub struct ResultInfo {
    pub page: Option<u64>,
    pub per_page: Option<u64>,
    pub count: Option<u64>,
    pub total_count: Option<u64>,
}

/// Every reply of the Cloudflare API.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Envelope<T> {
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub errors: Vec<ApiMessage>,
    #[serde(default)]
    pub messages: Vec<ApiMessage>,
    #[serde(default = "none")]
    pub result: Option<T>,
    #[serde(default)]
    pub result_info: Option<ResultInfo>,
}

fn none<T>() -> Option<T> {
    None
}

/// The rows of one statement: column names and rows of JSON cells.
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
pub struct RawRows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
pub struct Meta {
    #[serde(deserialize_with = "count")]
    pub changes: u64,
    #[serde(deserialize_with = "opt_int")]
    pub last_row_id: Option<i64>,
    pub changed_db: Option<bool>,
    pub duration: Option<f64>,
    #[serde(deserialize_with = "count")]
    pub rows_read: u64,
    #[serde(deserialize_with = "count")]
    pub rows_written: u64,
    pub size_after: Option<f64>,
    pub served_by_region: Option<String>,
    pub served_by_primary: Option<bool>,
}

/// One statement's result.
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
pub struct RawResult {
    pub results: Option<RawRows>,
    pub meta: Option<Meta>,
    pub success: Option<bool>,
}

impl RawResult {
    pub fn rows(&self) -> &[Vec<serde_json::Value>] {
        self.results.as_ref().map_or(&[], |r| r.rows.as_slice())
    }

    pub fn changes(&self) -> u64 {
        self.meta.as_ref().map_or(0, |m| m.changes)
    }
}

/// A database of the account.
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
pub struct DatabaseInfo {
    pub uuid: String,
    pub name: String,
    pub created_at: Option<String>,
    pub version: Option<String>,
    #[serde(deserialize_with = "opt_count")]
    pub file_size: Option<u64>,
    #[serde(deserialize_with = "opt_count")]
    pub num_tables: Option<u64>,
    pub jurisdiction: Option<String>,
}

/// A JavaScript number that counts something: whole and not negative (`null` is 0).
fn count<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    Ok(opt_count(d)?.unwrap_or(0))
}

fn opt_count<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    use serde::de::Error as _;
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(n) => n
            .as_u64()
            .or_else(|| {
                n.as_f64()
                    .filter(|f| *f >= 0.0 && f.fract() == 0.0)
                    .map(|f| f as u64)
            })
            .map(Some)
            .ok_or_else(|| D::Error::custom(format!("count {n}"))),
        other => Err(D::Error::custom(format!("count {other}"))),
    }
}

fn opt_int<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    use serde::de::Error as _;
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
            .map(Some)
            .ok_or_else(|| D::Error::custom(format!("integer {n}"))),
        serde_json::Value::String(s) => s
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| D::Error::custom(format!("integer {s:?}"))),
        other => Err(D::Error::custom(format!("integer {other}"))),
    }
}

// Cloudflare's error codes.
const AUTH_CODES: [i64; 4] = [10000, 9106, 9109, 7403];
const NOT_FOUND_CODES: [i64; 1] = [7404];
const RATE_LIMIT_CODES: [i64; 1] = [971];

/// The `result` of a successful reply, or its error. `sql` is the statement the reply is for,
/// when there is one, so a syntax error can point into it.
pub fn parse_reply<T: for<'de> Deserialize<'de>>(
    resp: &HttpResponse,
    sql: Option<&str>,
) -> Result<T, AppError> {
    let envelope: Option<Envelope<T>> = serde_json::from_slice(&resp.body).ok();
    match envelope {
        Some(Envelope {
            success: true,
            result: Some(result),
            ..
        }) if resp.is_success() => Ok(result),
        Some(envelope) => Err(api_error(
            resp.status,
            resp.retry_after,
            &envelope.errors,
            sql,
        )),
        None if resp.is_success() => Err(AppError::db(format!(
            "unexpected reply from {ENGINE}: {}",
            body_excerpt(&resp.body, resp.content_type.as_deref())
        ))),
        None => Err(status_error(
            ENGINE,
            resp.status,
            resp.retry_after,
            &body_excerpt(&resp.body, resp.content_type.as_deref()),
        )),
    }
}

/// The error a reply reports: refused tokens, missing databases and rate limits by their codes
/// or statuses, SQL errors as `Db` with SQLite's code from the end of the message.
pub fn api_error(
    status: u16,
    retry_after: Option<u64>,
    errors: &[ApiMessage],
    sql: Option<&str>,
) -> AppError {
    let first = errors.first().cloned().unwrap_or_default();
    let message = first
        .message
        .strip_prefix("D1_ERROR: ")
        .unwrap_or(&first.message)
        .trim();
    if AUTH_CODES.contains(&first.code) {
        return status_error(ENGINE, 401, None, message);
    }
    if NOT_FOUND_CODES.contains(&first.code) {
        return status_error(ENGINE, 404, None, message);
    }
    if RATE_LIMIT_CODES.contains(&first.code) {
        return status_error(ENGINE, 429, retry_after, message);
    }
    if matches!(status, 401 | 403 | 404 | 408 | 429 | 502 | 503 | 504) || message.is_empty() {
        return status_error(ENGINE, status, retry_after, message);
    }
    let (message, vendor_code) = split_code_suffix(message);
    let position = sql.and_then(|sql| offset_in(message, sql));
    AppError::Db {
        message: message.to_string(),
        detail: (first.code != 0).then(|| format!("{ENGINE} error {}", first.code)),
        hint: None,
        sqlstate: None,
        vendor_code,
        position,
        statement_index: None,
    }
}

/// `… at offset 12` (a byte offset into the statement) as a position.
fn offset_in(message: &str, sql: &str) -> Option<ErrorPos> {
    let rest = &message[message.find("at offset ")? + "at offset ".len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let byte: usize = digits.parse().ok()?;
    let chars = sql.get(..byte.min(sql.len()))?.chars().count();
    Some(ErrorPos::from_offset(sql, chars))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ACCOUNT: &str = "0123456789abcdef0123456789ABCDEF";

    #[test]
    fn urls_encode_their_parts() {
        let t = D1Target::new(None, ACCOUNT, "db/1").unwrap();
        assert_eq!(
            t.raw_url().unwrap(),
            format!(
                "https://api.cloudflare.com/client/v4/accounts/{ACCOUNT}/d1/database/db%2F1/raw"
            )
        );
        let t = D1Target::new(Some("http://127.0.0.1:9/client/v4/"), ACCOUNT, "u").unwrap();
        assert_eq!(
            t.database_url().unwrap(),
            format!("http://127.0.0.1:9/client/v4/accounts/{ACCOUNT}/d1/database/u")
        );
        assert_eq!(
            list_url(&t.api_base, ACCOUNT, Some("my db&x"), 2, 50).unwrap(),
            format!(
                "http://127.0.0.1:9/client/v4/accounts/{ACCOUNT}/d1/database?page=2&per_page=50&name=my+db%26x"
            )
        );
        assert_eq!(t.with_database("v").database_id, "v");
        assert!(D1Target::new(None, "nope", "db").is_err());
        assert!(D1Target::new(None, ACCOUNT, " ").is_err());
        assert!(
            D1Target::new(Some("not a url"), ACCOUNT, "x")
                .unwrap()
                .raw_url()
                .is_err()
        );
    }

    #[test]
    fn uuids_and_names() {
        assert!(is_uuid("6a1c5e0e-1d2f-4c3b-9a8e-0123456789ab"));
        assert!(!is_uuid("my-database"));
        assert!(!is_uuid("6a1c5e0e-1d2f-4c3b-9a8e-0123456789"));
    }

    #[test]
    fn bodies_serialize_as_the_api_documents_them() {
        let single = RawBody::Single(RawQuery {
            sql: "SELECT ?1".into(),
            params: vec![json!(1)],
        });
        assert_eq!(
            serde_json::to_value(&single).unwrap(),
            json!({"sql": "SELECT ?1", "params": [1]})
        );
        let batch = RawBody::Batch {
            batch: vec![RawQuery {
                sql: "SELECT 1".into(),
                params: vec![],
            }],
        };
        assert_eq!(
            serde_json::to_value(&batch).unwrap(),
            json!({"batch": [{"sql": "SELECT 1"}]})
        );
    }

    #[test]
    fn replies_parse_and_errors_map() {
        let ok = HttpResponse::json(
            200,
            &json!({"success": true, "errors": [], "messages": [], "result": [{
                "results": {"columns": ["a", "a"], "rows": [[1, 2]]},
                "meta": {"changes": 0, "last_row_id": 0, "duration": 0.2, "rows_read": 1,
                         "rows_written": 0, "changed_db": false, "size_after": 8192.0,
                         "served_by_region": "WEUR"},
                "success": true}]}),
        );
        let results: Vec<RawResult> = parse_reply(&ok, None).unwrap();
        assert_eq!(results[0].rows()[0], [json!(1), json!(2)]);
        assert_eq!(results[0].results.as_ref().unwrap().columns, ["a", "a"]);
        assert_eq!(results[0].meta.as_ref().unwrap().last_row_id, Some(0));

        let sql_error = HttpResponse::json(
            400,
            &json!({"success": false, "result": null, "messages": [],
                    "errors": [{"code": 7500, "message": "near \"SELEC\": syntax error at offset 0: SQLITE_ERROR"}]}),
        );
        match parse_reply::<Vec<RawResult>>(&sql_error, Some("SELEC 1")).unwrap_err() {
            AppError::Db {
                message,
                vendor_code,
                detail,
                position,
                ..
            } => {
                assert_eq!(message, "near \"SELEC\": syntax error at offset 0");
                assert_eq!(vendor_code, Some(1));
                assert_eq!(detail.as_deref(), Some("Cloudflare D1 error 7500"));
                assert_eq!(
                    position.map(|p| (p.offset, p.line, p.column)),
                    Some((0, 1, 1))
                );
            }
            other => panic!("{other:?}"),
        }
        let auth = HttpResponse::json(
            400,
            &json!({"success": false, "errors": [{"code": 10000, "message": "Authentication error"}]}),
        );
        assert!(matches!(
            parse_reply::<Vec<RawResult>>(&auth, None),
            Err(AppError::Auth(_))
        ));
        let missing = HttpResponse::json(
            404,
            &json!({"success": false, "errors": [{"code": 7404, "message": "The database x could not be found"}]}),
        );
        assert!(matches!(
            parse_reply::<DatabaseInfo>(&missing, None),
            Err(AppError::NotFound(_))
        ));
        let limited = HttpResponse {
            retry_after: Some(30),
            ..HttpResponse::json(
                429,
                &json!({"success": false, "errors": [{"code": 971, "message": "Please wait"}]}),
            )
        };
        match parse_reply::<DatabaseInfo>(&limited, None).unwrap_err() {
            AppError::Db { hint, .. } => assert_eq!(hint.as_deref(), Some("Retry in 30 s.")),
            other => panic!("{other:?}"),
        }
        let html = HttpResponse {
            status: 502,
            retry_after: None,
            content_type: Some("text/html".into()),
            body: b"<html>Bad gateway</html>".to_vec(),
        };
        assert!(matches!(
            parse_reply::<DatabaseInfo>(&html, None),
            Err(AppError::Connect(_))
        ));
        let garbage = HttpResponse::text(200, "not json");
        assert!(parse_reply::<DatabaseInfo>(&garbage, None).is_err());
        let refused = HttpResponse::json(200, &json!({"success": false, "errors": []}));
        assert!(parse_reply::<DatabaseInfo>(&refused, None).is_err());
        let prefixed = api_error(
            400,
            None,
            &[ApiMessage {
                code: 7500,
                message: "D1_ERROR: UNIQUE constraint failed: t.v: SQLITE_CONSTRAINT".into(),
            }],
            None,
        );
        assert!(
            matches!(prefixed, AppError::Db { vendor_code: Some(19), ref message, .. } if message == "UNIQUE constraint failed: t.v")
        );
    }

    #[test]
    fn meta_numbers_are_lenient() {
        let m: Meta =
            serde_json::from_value(json!({"changes": 2.0, "last_row_id": "7", "rows_read": null}))
                .unwrap();
        assert_eq!((m.changes, m.last_row_id, m.rows_read), (2, Some(7), 0));
        assert!(serde_json::from_value::<Meta>(json!({"changes": -1})).is_err());
        assert!(serde_json::from_value::<Meta>(json!({"changes": "x"})).is_err());
        let db: DatabaseInfo = serde_json::from_value(json!({
            "uuid": "u", "name": "n", "file_size": 12288, "version": "production",
            "created_at": "2026-01-01T00:00:00Z", "num_tables": 3, "jurisdiction": null}))
        .unwrap();
        assert_eq!((db.file_size, db.num_tables), (Some(12288), Some(3)));
    }
}
