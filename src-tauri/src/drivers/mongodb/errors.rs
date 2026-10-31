//! MongoDB errors as `AppError`s: server errors keep their code in `vendor_code` and their code
//! name in `detail`; a killed operation is `Cancelled`, `maxTimeMS` running out a `Timeout`, bad
//! credentials `Auth`, a lost or unreachable server `NotConnected` / `Connect`.

use ::mongodb::bson::Document;
use ::mongodb::error::{Error, ErrorKind, WriteFailure};

use crate::error::AppError;

/// `Interrupted`: the operation was killed (`killOp`, the console's cancel).
pub const INTERRUPTED: i32 = 11601;
/// `MaxTimeMSExpired`.
pub const MAX_TIME_EXPIRED: i32 = 50;
/// `AuthenticationFailed`.
pub const AUTHENTICATION_FAILED: i32 = 18;
/// `NamespaceNotFound` (`drop()` of a collection that does not exist).
pub const NAMESPACE_NOT_FOUND: i32 = 26;

/// A server error from its code, code name and message.
pub fn from_code(code: i32, code_name: &str, message: &str) -> AppError {
    match code {
        INTERRUPTED => AppError::Cancelled,
        MAX_TIME_EXPIRED => AppError::Timeout,
        AUTHENTICATION_FAILED => AppError::Auth(message.to_string()),
        _ => AppError::Db {
            message: if message.is_empty() {
                format!("MongoDB error {code}")
            } else {
                message.to_string()
            },
            detail: (!code_name.is_empty()).then(|| code_name.to_string()),
            hint: None,
            sqlstate: None,
            vendor_code: Some(i64::from(code)),
            position: None,
            statement_index: None,
        },
    }
}

/// The error a reply with `ok: 0` carries (`run_command` returns such replies as errors, a raw
/// reply may still hold one), or a write error of an `ok: 1` write reply.
pub fn from_reply(reply: &Document) -> Option<AppError> {
    let ok = match reply.get("ok") {
        Some(b) => {
            super::literal::int_of(b).is_some_and(|n| n == 1)
                || matches!(b, ::mongodb::bson::Bson::Boolean(true))
        }
        None => false,
    };
    let code = |d: &Document| {
        d.get("code")
            .and_then(super::literal::int_of)
            .and_then(|c| i32::try_from(c).ok())
            .unwrap_or(0)
    };
    if !ok {
        return Some(from_code(
            code(reply),
            reply.get_str("codeName").unwrap_or_default(),
            reply.get_str("errmsg").unwrap_or("the command failed"),
        ));
    }
    let first_write_error = reply
        .get_array("writeErrors")
        .ok()
        .and_then(|e| e.first())
        .and_then(|e| e.as_document());
    if let Some(e) = first_write_error {
        return Some(from_code(
            code(e),
            e.get_str("codeName").unwrap_or_default(),
            e.get_str("errmsg").unwrap_or("the write failed"),
        ));
    }
    reply.get_document("writeConcernError").ok().map(|e| {
        from_code(
            code(e),
            e.get_str("codeName").unwrap_or_default(),
            e.get_str("errmsg").unwrap_or("the write concern failed"),
        )
    })
}

/// An error of the driver. One Tabler raised inside the driver (the OIDC callback's password
/// command) comes back as it was.
pub fn from_driver(e: &Error) -> AppError {
    if let Some(app) = e.get_custom::<AppError>() {
        return app.clone();
    }
    match e.kind.as_ref() {
        ErrorKind::Authentication { message, .. } => AppError::Auth(message.clone()),
        ErrorKind::Command(c) => from_code(c.code, &c.code_name, &c.message),
        ErrorKind::Write(WriteFailure::WriteError(w)) => from_code(
            w.code,
            w.code_name.as_deref().unwrap_or_default(),
            &w.message,
        ),
        ErrorKind::Write(WriteFailure::WriteConcernError(w)) => {
            from_code(w.code, &w.code_name, &w.message)
        }
        ErrorKind::Io(_) | ErrorKind::ConnectionPoolCleared { .. } | ErrorKind::Shutdown => {
            AppError::NotConnected
        }
        ErrorKind::ServerSelection { message, .. }
        | ErrorKind::DnsResolve { message, .. }
        | ErrorKind::InvalidTlsConfig { message, .. } => AppError::Connect(message.clone()),
        ErrorKind::InvalidArgument { message, .. } => AppError::InvalidInput(message.clone()),
        _ => AppError::Db {
            message: e.to_string(),
            detail: None,
            hint: None,
            sqlstate: None,
            vendor_code: None,
            position: None,
            statement_index: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use ::mongodb::bson::doc;
    use ::mongodb::error::CommandError;

    use super::*;

    fn command_error(code: i32, name: &str, msg: &str) -> Error {
        let c: CommandError =
            ::mongodb::bson::from_document(doc! { "code": code, "codeName": name, "errmsg": msg })
                .unwrap();
        Error::from(ErrorKind::Command(c))
    }

    #[test]
    fn server_errors_keep_their_code() {
        match from_driver(&command_error(13, "Unauthorized", "not authorized on shop")) {
            AppError::Db {
                message,
                detail,
                vendor_code,
                sqlstate,
                ..
            } => {
                assert_eq!(message, "not authorized on shop");
                assert_eq!(detail.as_deref(), Some("Unauthorized"));
                assert_eq!(vendor_code, Some(13));
                assert_eq!(sqlstate, None);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            from_driver(&command_error(
                11601,
                "Interrupted",
                "operation was interrupted"
            )),
            AppError::Cancelled
        ));
        assert!(matches!(
            from_driver(&command_error(50, "MaxTimeMSExpired", "time limit")),
            AppError::Timeout
        ));
        assert!(matches!(
            from_driver(&command_error(18, "AuthenticationFailed", "bad auth")),
            AppError::Auth(m) if m == "bad auth"
        ));
    }

    #[test]
    fn transport_errors() {
        let io = Error::from(ErrorKind::Io(std::sync::Arc::new(std::io::Error::other(
            "reset",
        ))));
        assert!(matches!(from_driver(&io), AppError::NotConnected));
    }

    #[test]
    fn replies_with_errors() {
        assert!(from_reply(&doc! { "ok": 1, "n": 1 }).is_none());
        assert!(from_reply(&doc! { "ok": 1.0 }).is_none());
        let e = from_reply(
            &doc! { "ok": 0.0, "errmsg": "no such command", "code": 59, "codeName": "CommandNotFound" },
        );
        assert!(matches!(
            e,
            Some(AppError::Db {
                vendor_code: Some(59),
                ..
            })
        ));
        let e = from_reply(
            &doc! { "ok": 1, "n": 0, "writeErrors": [{ "index": 0, "code": 11000, "errmsg": "E11000 duplicate key" }] },
        );
        assert!(
            matches!(e, Some(AppError::Db { vendor_code: Some(11000), ref message, .. }) if message.starts_with("E11000"))
        );
        let e = from_reply(
            &doc! { "ok": 1, "writeConcernError": { "code": 64, "errmsg": "waiting for replication timed out" } },
        );
        assert!(matches!(
            e,
            Some(AppError::Db {
                vendor_code: Some(64),
                ..
            })
        ));
        assert!(from_reply(&doc! {}).is_some());
    }
}
