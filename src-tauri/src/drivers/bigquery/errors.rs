//! BigQuery's errors as `AppError`s: the HTTP error envelope of the REST API
//! (`{"error": {"code", "message", "status", "errors": [{"reason", "location", "message"}]}}`) and
//! the `ErrorProto`s of a job (`status.errorResult`, a query response's `errors`).
//!
//! Server errors are `Db` with the HTTP code (or 400 for a job error) in `vendor_code`, the
//! reason in `detail` and, for query errors, the position BigQuery names (`at [3:15]`); a
//! cancelled job is `Cancelled`, a refused token `Auth`, a missing API object `NotFound`.

use serde::Deserialize;

use super::transport::body_excerpt;
use crate::error::{AppError, ErrorPos};

/// One error of a job or a request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ErrorProto {
    pub reason: Option<String>,
    pub location: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Envelope {
    error: Option<EnvelopeBody>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct EnvelopeBody {
    message: Option<String>,
    status: Option<String>,
    errors: Vec<ErrorProto>,
}

/// Reasons that say the request may succeed later.
const RETRY_LATER: &[&str] = &[
    "rateLimitExceeded",
    "quotaExceeded",
    "backendError",
    "jobBackendError",
];

/// The error of a reply whose status is not a success. `sql`: the statement it ran, for the
/// position of a query error.
pub fn http_error(
    status: u16,
    body: &[u8],
    content_type: Option<&str>,
    sql: Option<&str>,
) -> AppError {
    let env: Envelope = serde_json::from_slice(body).unwrap_or_default();
    let (message, reason) = match env.error {
        Some(e) => {
            let first = e.errors.first();
            let reason = first.and_then(|p| p.reason.clone()).or(e.status.clone());
            let message = e
                .message
                .or_else(|| first.and_then(|p| p.message.clone()))
                .unwrap_or_default();
            (message, reason)
        }
        None => (body_excerpt(body, content_type), None),
    };
    let said = if message.is_empty() {
        String::new()
    } else {
        format!(": {message}")
    };
    match (status, reason.as_deref()) {
        // `jobs.getQueryResults` of a cancelled job.
        (_, Some("stopped")) => AppError::Cancelled,
        (401, _) => AppError::Auth(format!(
            "BigQuery refused the access token (HTTP 401){said}"
        )),
        (403, Some(r)) if RETRY_LATER.contains(&r) => retry_later(status, &message, r),
        (404, _) => AppError::NotFound(format!("BigQuery (HTTP 404){said}")),
        (408 | 504, _) => AppError::Timeout,
        (429, r) => retry_later(status, &message, r.unwrap_or("rateLimitExceeded")),
        (502 | 503, _) => {
            AppError::Connect(format!("BigQuery is unavailable (HTTP {status}){said}"))
        }
        (_, r) => db_error(
            if message.is_empty() {
                format!("BigQuery answered HTTP {status}")
            } else {
                message.clone()
            },
            r,
            i64::from(status),
            sql,
        ),
    }
}

/// The error a finished job or a query response reports: its first `ErrorProto` (BigQuery's
/// `errorResult`), with the others as detail.
pub fn job_error(first: &ErrorProto, all: &[ErrorProto], sql: Option<&str>) -> AppError {
    let reason = first.reason.as_deref();
    if reason == Some("stopped") {
        return AppError::Cancelled;
    }
    let message = first
        .message
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "the BigQuery job failed".to_string());
    let mut err = db_error(message, reason, 400, sql);
    let others: Vec<&str> = all
        .iter()
        .filter(|e| *e != first)
        .filter_map(|e| e.message.as_deref())
        .collect();
    if !others.is_empty()
        && let AppError::Db { detail, .. } = &mut err
    {
        let more = others.join("\n");
        *detail = Some(match detail.take() {
            Some(d) => format!("{d}\n{more}"),
            None => more,
        });
    }
    err
}

fn retry_later(status: u16, message: &str, reason: &str) -> AppError {
    AppError::Db {
        message: if message.is_empty() {
            format!("BigQuery is limiting requests (HTTP {status})")
        } else {
            message.to_string()
        },
        detail: Some(format!("reason: {reason}")),
        hint: Some("Wait a little before retrying.".into()),
        sqlstate: None,
        vendor_code: Some(i64::from(status)),
        position: None,
        statement_index: None,
    }
}

fn db_error(message: String, reason: Option<&str>, code: i64, sql: Option<&str>) -> AppError {
    let hint = match reason {
        Some("bytesBilledLimitExceeded") => Some(
            "The query would bill more bytes than the connection's Maximum bytes billed allows."
                .to_string(),
        ),
        Some("billingNotEnabled") => {
            Some("The sandbox does not run DML or DDL that needs billing.".to_string())
        }
        Some("accessDenied") => Some(
            "The account needs BigQuery Job User on the project and Data Viewer (or Editor) on \
             the data."
                .to_string(),
        ),
        _ => None,
    };
    let position = sql.and_then(|s| position_in(&message, s));
    AppError::Db {
        message,
        detail: reason.map(|r| format!("reason: {r}")),
        hint,
        sqlstate: None,
        vendor_code: Some(code),
        position,
        statement_index: None,
    }
}

/// The `[line:column]` BigQuery puts at the end of a query error (`… at [3:15]`), 1-based, as a
/// position in `sql`.
pub fn position_in(message: &str, sql: &str) -> Option<ErrorPos> {
    let at = message.rfind(" at [")?;
    let inside = &message[at + 5..];
    let end = inside.find(']')?;
    let (line, column) = inside[..end].split_once(':')?;
    let (line, column): (usize, usize) = (line.trim().parse().ok()?, column.trim().parse().ok()?);
    if line == 0 || column == 0 {
        return None;
    }
    // The character offset of that line and column.
    let mut offset = 0usize;
    for (i, text) in sql.split('\n').enumerate() {
        if i + 1 == line {
            if column > text.chars().count() + 1 {
                return None;
            }
            return Some(ErrorPos::from_offset(sql, offset + column - 1));
        }
        offset += text.chars().count() + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(code: u16, reason: &str, message: &str) -> Vec<u8> {
        serde_json::json!({"error": {"code": code, "message": message, "status": "X",
            "errors": [{"reason": reason, "location": "q", "message": message}]}})
        .to_string()
        .into_bytes()
    }

    #[test]
    fn http_statuses_map_to_error_kinds() {
        let sql = "SELECT 1\nFROM nope";
        let e = http_error(
            400,
            &envelope(400, "invalidQuery", "Unrecognized name: nope at [2:6]"),
            None,
            Some(sql),
        );
        match e {
            AppError::Db {
                message,
                detail,
                vendor_code,
                position,
                ..
            } => {
                assert_eq!(message, "Unrecognized name: nope at [2:6]");
                assert_eq!(detail.as_deref(), Some("reason: invalidQuery"));
                assert_eq!(vendor_code, Some(400));
                let p = position.expect("a position");
                assert_eq!((p.line, p.column, p.offset), (2, 6, 14));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            http_error(401, &envelope(401, "authError", "Invalid Credentials"), None, None),
            AppError::Auth(m) if m.contains("Invalid Credentials")
        ));
        assert!(matches!(
            http_error(
                404,
                &envelope(404, "notFound", "Not found: Dataset p:d"),
                None,
                None
            ),
            AppError::NotFound(_)
        ));
        match http_error(
            403,
            &envelope(403, "rateLimitExceeded", "slow down"),
            None,
            None,
        ) {
            AppError::Db { hint, .. } => assert!(hint.is_some()),
            other => panic!("{other:?}"),
        }
        match http_error(
            403,
            &envelope(403, "accessDenied", "Access Denied: Table p:d.t"),
            None,
            None,
        ) {
            AppError::Db { hint, detail, .. } => {
                assert!(hint.unwrap().contains("Job User"));
                assert_eq!(detail.as_deref(), Some("reason: accessDenied"));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            http_error(504, b"", None, None),
            AppError::Timeout
        ));
        // The results of a cancelled job.
        assert!(matches!(
            http_error(
                400,
                &envelope(
                    400,
                    "stopped",
                    "Job execution was cancelled: User requested cancellation"
                ),
                None,
                None
            ),
            AppError::Cancelled
        ));
        assert!(matches!(
            http_error(503, b"<html>Service Unavailable</html>", Some("text/html"), None),
            AppError::Connect(m) if m.contains("Service Unavailable")
        ));
        match http_error(500, b"oops", None, None) {
            AppError::Db { message, .. } => assert_eq!(message, "oops"),
            other => panic!("{other:?}"),
        }
        match http_error(400, b"", None, None) {
            AppError::Db { message, .. } => assert_eq!(message, "BigQuery answered HTTP 400"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn job_errors_cancel_and_carry_their_reason() {
        let stopped = ErrorProto {
            reason: Some("stopped".into()),
            message: Some("Job execution was cancelled: User requested cancellation".into()),
            ..Default::default()
        };
        assert!(matches!(
            job_error(&stopped, &[], None),
            AppError::Cancelled
        ));
        let billed = ErrorProto {
            reason: Some("bytesBilledLimitExceeded".into()),
            message: Some("Query exceeded limit for bytes billed: 1000.".into()),
            ..Default::default()
        };
        let other = ErrorProto {
            message: Some("second".into()),
            ..Default::default()
        };
        match job_error(&billed, &[billed.clone(), other], None) {
            AppError::Db { hint, detail, .. } => {
                assert!(hint.unwrap().contains("Maximum bytes billed"));
                assert_eq!(
                    detail.as_deref(),
                    Some("reason: bytesBilledLimitExceeded\nsecond")
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn positions_stay_inside_the_text() {
        assert_eq!(
            position_in("x at [1:1]", "SELECT").map(|p| p.offset),
            Some(0)
        );
        assert_eq!(
            position_in("x at [1:7]", "SELECT").map(|p| p.offset),
            Some(6)
        );
        assert!(position_in("x at [1:9]", "SELECT").is_none());
        assert!(position_in("x at [3:1]", "SELECT").is_none());
        assert!(position_in("x at [0:1]", "SELECT").is_none());
        assert!(position_in("x at [a:b]", "SELECT").is_none());
        assert!(position_in("no position", "SELECT").is_none());
    }
}
