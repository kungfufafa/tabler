//! LibSQL errors as `AppError`s: a request's or step's Hrana error (SQLite's result code in
//! `vendor_code`, the server's own code in `detail`), and an HTTP status that ends the stream.

pub use crate::drivers::sqlite_http::codes::sqlite_code;
use crate::drivers::sqlite_http::transport::{HttpResponse, body_excerpt, status_error};
use crate::error::AppError;

use super::hrana::HranaError;

pub const ENGINE: &str = "LibSQL";

/// What a session sees when the server dropped its stream (idle for about 10 s, or after an
/// HTTP error): whatever the stream held, an open transaction included, is gone.
pub const STREAM_LOST_HINT: &str = "The LibSQL server closed this session (streams expire after \
     about 10 s idle, transactions after about 5 s); an open transaction was rolled back.";

/// A statement's error.
pub fn hrana_error(e: &HranaError) -> AppError {
    let code = e.code.as_deref().unwrap_or("").trim();
    let message = e
        .message
        .strip_prefix("SQLite error: ")
        .unwrap_or(&e.message)
        .to_string();
    let vendor_code = sqlite_code(code);
    let hint = match code {
        "TRANSACTION_TIMEOUT" => Some(
            "LibSQL rolls back a transaction that stays open longer than about 5 s.".to_string(),
        ),
        "STREAM_EXPIRED" | "STREAM_CLOSED" => Some(STREAM_LOST_HINT.to_string()),
        _ if matches!(
            vendor_code.map(|c| c & 0xff),
            Some(c) if c == i64::from(rusqlite::ffi::SQLITE_BUSY)
                || c == i64::from(rusqlite::ffi::SQLITE_LOCKED)
        ) =>
        {
            Some("The database is locked by another writer.".to_string())
        }
        _ => None,
    };
    AppError::Db {
        message: if message.is_empty() {
            format!("{ENGINE} error {code}")
        } else {
            message
        },
        detail: (!code.is_empty() && vendor_code.is_none())
            .then(|| format!("{ENGINE} code {code}")),
        hint,
        sqlstate: None,
        vendor_code,
        position: None,
        statement_index: None,
    }
}

/// A reply with an error status: the stream it was for is no longer valid (spec).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpFailure {
    pub status: u16,
    pub message: String,
    pub code: Option<String>,
    pub retry_after: Option<u64>,
}

impl HttpFailure {
    /// The body is a Hrana `Error`, `{"error": "…"}`, plain text or an HTML page.
    pub fn from_response(resp: &HttpResponse) -> HttpFailure {
        let json: Option<serde_json::Value> = serde_json::from_slice(&resp.body).ok();
        let field =
            |v: &serde_json::Value, k: &str| v.get(k).and_then(|m| m.as_str()).map(str::to_string);
        let (message, code) = match &json {
            Some(v) => {
                let inner = v.get("error").filter(|e| e.is_object()).unwrap_or(v);
                let message = field(inner, "message")
                    .or_else(|| field(v, "error"))
                    .unwrap_or_else(|| body_excerpt(&resp.body, resp.content_type.as_deref()));
                (message, field(inner, "code"))
            }
            None => (body_excerpt(&resp.body, resp.content_type.as_deref()), None),
        };
        HttpFailure {
            status: resp.status,
            message,
            code,
            retry_after: resp.retry_after,
        }
    }

    /// The server no longer knows the stream (expired, closed, or a baton it does not accept).
    pub fn stream_gone(&self) -> bool {
        let lower = self.message.to_ascii_lowercase();
        matches!(
            self.code.as_deref(),
            Some("STREAM_EXPIRED" | "STREAM_CLOSED" | "INVALID_BATON")
        ) || (self.status == 400
            && (lower.contains("baton") || (lower.contains("stream") && lower.contains("expired"))))
    }

    pub fn to_error(&self) -> AppError {
        if self.stream_gone() {
            return AppError::Db {
                message: format!("{ENGINE} session expired: {}", self.message),
                detail: self.code.clone(),
                hint: Some(STREAM_LOST_HINT.to_string()),
                sqlstate: None,
                vendor_code: None,
                position: None,
                statement_index: None,
            };
        }
        status_error(ENGINE, self.status, self.retry_after, &self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(message: &str, code: Option<&str>) -> HranaError {
        HranaError {
            message: message.into(),
            code: code.map(str::to_string),
        }
    }

    #[test]
    fn statement_errors_keep_sqlites_codes() {
        match hrana_error(&err(
            "SQLite error: UNIQUE constraint failed: t.a",
            Some("SQLITE_CONSTRAINT_UNIQUE"),
        )) {
            AppError::Db {
                message,
                vendor_code,
                detail,
                ..
            } => {
                assert_eq!(message, "UNIQUE constraint failed: t.a");
                assert_eq!(vendor_code, Some(2067));
                assert_eq!(detail, None);
            }
            other => panic!("{other:?}"),
        }
        match hrana_error(&err("parse error", Some("SQL_PARSE_ERROR"))) {
            AppError::Db {
                vendor_code,
                detail,
                ..
            } => {
                assert_eq!(vendor_code, None);
                assert_eq!(detail.as_deref(), Some("LibSQL code SQL_PARSE_ERROR"));
            }
            other => panic!("{other:?}"),
        }
        match hrana_error(&err("database is locked", Some("SQLITE_BUSY"))) {
            AppError::Db { hint, .. } => assert!(hint.unwrap().contains("locked")),
            other => panic!("{other:?}"),
        }
        match hrana_error(&err("", Some("TRANSACTION_TIMEOUT"))) {
            AppError::Db { message, hint, .. } => {
                assert_eq!(message, "LibSQL error TRANSACTION_TIMEOUT");
                assert!(hint.unwrap().contains("5 s"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(sqlite_code("SQLITE_CONSTRAINT"), Some(19));
        assert_eq!(sqlite_code("CONSTRAINT"), None);
        assert_eq!(sqlite_code("SQLITE_WHATEVER"), None);
    }

    #[test]
    fn http_bodies_of_every_shape() {
        let json = HttpFailure::from_response(&HttpResponse::json(
            400,
            &serde_json::json!({"message": "The stream has expired due to inactivity", "code": "STREAM_EXPIRED"}),
        ));
        assert!(json.stream_gone());
        let e = json.to_error();
        assert!(e.to_string().contains("session expired"), "{e}");
        let old = HttpFailure::from_response(&HttpResponse::json(
            400,
            &serde_json::json!({"error": "Received an invalid baton"}),
        ));
        assert_eq!(old.message, "Received an invalid baton");
        assert!(old.stream_gone());
        let nested = HttpFailure::from_response(&HttpResponse::json(
            500,
            &serde_json::json!({"error": {"message": "boom", "code": "X"}}),
        ));
        assert_eq!(
            (nested.message.as_str(), nested.code.as_deref()),
            ("boom", Some("X"))
        );
        let html = HttpFailure::from_response(&HttpResponse {
            status: 502,
            retry_after: None,
            content_type: Some("text/html".into()),
            body: b"<html><h1>Bad Gateway</h1></html>".to_vec(),
        });
        assert_eq!(html.message, "Bad Gateway");
        assert!(matches!(html.to_error(), AppError::Connect(_)));
        let auth = HttpFailure::from_response(&HttpResponse::text(401, "Unauthorized"));
        assert!(!auth.stream_gone());
        assert!(matches!(auth.to_error(), AppError::Auth(_)));
        let missing = HttpFailure::from_response(&HttpResponse::text(404, "Namespace not found"));
        assert!(matches!(missing.to_error(), AppError::NotFound(_)));
        let limited = HttpFailure::from_response(&HttpResponse {
            retry_after: Some(3),
            ..HttpResponse::text(429, "slow down")
        });
        assert!(
            matches!(limited.to_error(), AppError::Db { hint: Some(h), .. } if h.contains("3 s"))
        );
    }
}
