//! Error replies as `AppError`s. Elasticsearch and OpenSearch answer
//! `{"error": {"type", "reason", "caused_by", "root_cause"}, "status"}` (very old servers a
//! string `error`); Amazon's gateways answer `{"message": …}`. The HTTP status goes into
//! `vendor_code`, the error type into `detail`:
//!
//! - 401, and an AWS signature or token refusal → `Auth`;
//! - a cancelled task → `Cancelled`; 408 / 504 → `Timeout`;
//! - `index_not_found_exception` → `NotFound`;
//! - 502 / 503 without an Elasticsearch error body (a proxy, a node that is down) → `Connect`;
//! - everything else → `Db`, with a hint for version conflicts, privileges and rejections.

use crate::error::AppError;

use super::json::{self, Json};

/// The type and reason of an `error` value, with its `caused_by` chain.
pub fn reason_text(error: &Json) -> String {
    match error {
        Json::Str(s) => s.clone(),
        Json::Obj(_) => {
            let mut parts = Vec::new();
            let mut cur = Some(error);
            let mut depth = 0;
            while let Some(e) = cur {
                if let Some(r) = e.get("reason").and_then(Json::as_str) {
                    parts.push(r.to_string());
                } else if let Some(t) = e.get("type").and_then(Json::as_str) {
                    parts.push(t.to_string());
                }
                cur = e.get("caused_by");
                depth += 1;
                if depth > 8 {
                    break;
                }
            }
            parts.dedup();
            if parts.is_empty() {
                error.to_text()
            } else {
                parts.join(": caused by: ")
            }
        }
        other => other.to_text(),
    }
}

fn error_type(error: &Json) -> Option<&str> {
    error.get("type").and_then(Json::as_str)
}

/// Whether a reply body is an error reply (it has `error`, or an AWS `message` / `Message`).
pub fn is_error_body(body: &Json) -> bool {
    body.get("error").is_some_and(|e| !e.is_null())
        || (body.get("message").is_some() && body.as_object().is_some_and(|o| o.len() <= 2))
        || body.get("Message").is_some()
}

/// The error of a reply with HTTP `status` and `body` (raw bytes), or `None` when it is not one
/// (a 404 `{"found": false}` of a document read is a reply, not an error).
pub fn reply_error(engine: &str, status: u16, body: &[u8]) -> Option<AppError> {
    if status < 400 {
        return None;
    }
    match json::parse_bytes(body).ok() {
        Some(b) if is_error_body(&b) => Some(from_error_body(engine, status, &b)),
        Some(_) if status == 404 => None,
        Some(b) => Some(status_error(engine, status, &b.to_text(), None)),
        None => Some(status_error(
            engine,
            status,
            &crate::drivers::common::http::body_excerpt(body, None),
            None,
        )),
    }
}

/// The error of an error reply body.
pub fn from_error_body(engine: &str, status: u16, body: &Json) -> AppError {
    let error = body
        .get("error")
        .or_else(|| body.get("message"))
        .or_else(|| body.get("Message"))
        .cloned()
        .unwrap_or(Json::Null);
    let ty = error_type(&error).map(str::to_string);
    let message = reason_text(&error);
    let lower = message.to_ascii_lowercase();
    if status == 401
        || (status == 403
            && (lower.contains("signature")
                || lower.contains("security token")
                || lower.contains("missing authentication")))
    {
        return AppError::Auth(format!("{engine} refused the credentials: {message}"));
    }
    match ty.as_deref() {
        Some("task_cancelled_exception") => return AppError::Cancelled,
        Some("index_not_found_exception") => return AppError::NotFound(message),
        Some("timeout_exception" | "receive_timeout_transport_exception") if status == 504 => {
            return AppError::Timeout;
        }
        _ => {}
    }
    if ty.is_none() && matches!(status, 502 | 503) {
        return AppError::Connect(format!(
            "{engine} is unavailable (HTTP {status}): {message}"
        ));
    }
    if ty.is_none() && matches!(status, 408 | 504) {
        return AppError::Timeout;
    }
    let hint = match (status, ty.as_deref()) {
        (409, _) | (_, Some("version_conflict_engine_exception")) => Some(
            "The document changed since it was read (or was deleted). Reload and edit again."
                .to_string(),
        ),
        (403, _) => Some("The user lacks a privilege this request needs.".to_string()),
        (429, _) | (_, Some("es_rejected_execution_exception" | "circuit_breaking_exception")) => {
            Some("The cluster is rejecting requests for now; retry in a moment.".to_string())
        }
        (_, Some("cluster_block_exception")) => {
            Some("The index or cluster is blocked for writes (read-only, or disk full).".into())
        }
        _ => None,
    };
    AppError::Db {
        message,
        detail: ty,
        hint,
        sqlstate: None,
        vendor_code: Some(i64::from(status)),
        position: None,
        statement_index: None,
    }
}

/// The error of a status whose body is not an error object.
pub fn status_error(engine: &str, status: u16, said: &str, ty: Option<String>) -> AppError {
    let said = if said.is_empty() {
        String::new()
    } else {
        format!(": {said}")
    };
    match status {
        401 => AppError::Auth(format!("{engine} refused the credentials (HTTP 401){said}")),
        408 | 504 => AppError::Timeout,
        502 | 503 => AppError::Connect(format!("{engine} is unavailable (HTTP {status}){said}")),
        _ => AppError::Db {
            message: format!("{engine} answered HTTP {status}{said}"),
            detail: ty,
            hint: None,
            sqlstate: None,
            vendor_code: Some(i64::from(status)),
            position: None,
            statement_index: None,
        },
    }
}

/// The error of one failed `_bulk` item.
pub fn item_error(engine: &str, status: u16, error: &Json) -> AppError {
    from_error_body(engine, status, &Json::obj([("error", error.clone())]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const E: &str = "Elasticsearch";

    fn err(status: u16, body: &str) -> Option<AppError> {
        reply_error(E, status, body.as_bytes())
    }

    #[test]
    fn auth_failures() {
        let e = err(
            401,
            r#"{"error":{"root_cause":[{"type":"security_exception","reason":"unable to authenticate user [elastic] for REST request [/]"}],"type":"security_exception","reason":"unable to authenticate user [elastic] for REST request [/]"},"status":401}"#,
        );
        assert!(matches!(e, Some(AppError::Auth(m)) if m.contains("unable to authenticate")));
        let aws = err(
            403,
            r#"{"message":"The request signature we calculated does not match the signature you provided."}"#,
        );
        assert!(matches!(aws, Some(AppError::Auth(_))));
        let denied = err(
            403,
            r#"{"error":{"type":"security_exception","reason":"action [indices:data/write/bulk] is unauthorized for user [ro]"},"status":403}"#,
        );
        match denied {
            Some(AppError::Db {
                vendor_code, hint, ..
            }) => {
                assert_eq!(vendor_code, Some(403));
                assert!(hint.unwrap().contains("privilege"));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(err(401, ""), Some(AppError::Auth(_))));
    }

    #[test]
    fn server_errors_keep_type_and_status() {
        match err(
            400,
            r#"{"error":{"root_cause":[],"type":"search_phase_execution_exception","reason":"all shards failed","caused_by":{"type":"query_shard_exception","reason":"failed to create query"}},"status":400}"#,
        ) {
            Some(AppError::Db {
                message,
                detail,
                vendor_code,
                ..
            }) => {
                assert_eq!(
                    message,
                    "all shards failed: caused by: failed to create query"
                );
                assert_eq!(detail.as_deref(), Some("search_phase_execution_exception"));
                assert_eq!(vendor_code, Some(400));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            err(
                404,
                r#"{"error":{"type":"index_not_found_exception","reason":"no such index [x]"},"status":404}"#
            ),
            Some(AppError::NotFound(_))
        ));
        assert!(matches!(
            err(
                400,
                r#"{"error":{"type":"task_cancelled_exception","reason":"cancelled"},"status":400}"#
            ),
            Some(AppError::Cancelled)
        ));
        match err(
            409,
            r#"{"error":{"type":"version_conflict_engine_exception","reason":"[1]: version conflict, required seqNo [5]"},"status":409}"#,
        ) {
            Some(AppError::Db { hint, .. }) => assert!(hint.unwrap().contains("changed")),
            other => panic!("{other:?}"),
        }
        assert!(
            matches!(err(400, r#"{"error":"old style"}"#), Some(AppError::Db { message, .. }) if message == "old style")
        );
    }

    #[test]
    fn not_every_status_is_an_error() {
        assert!(err(404, r#"{"_index":"a","_id":"1","found":false}"#).is_none());
        assert!(err(200, r#"{"acknowledged":true}"#).is_none());
        assert!(err(200, "plain text").is_none());
        assert!(matches!(
            err(503, "<html>Bad gateway</html>"),
            Some(AppError::Connect(_))
        ));
        assert!(matches!(err(504, ""), Some(AppError::Timeout)));
        assert!(matches!(
            err(
                503,
                r#"{"error":{"type":"search_phase_execution_exception","reason":"all shards failed"},"status":503}"#
            ),
            Some(AppError::Db { .. })
        ));
        assert!(matches!(err(500, "[1]"), Some(AppError::Db { .. })));
    }
}
