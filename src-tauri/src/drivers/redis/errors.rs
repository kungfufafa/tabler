//! Redis errors as `AppError`s. A server error keeps redis-cli's text (`WRONGTYPE Operation …`);
//! Redis codes are words, so neither `sqlstate` nor the numeric `vendor_code` carries them.
//! Bad credentials are `Auth`, a lost connection `NotConnected`, a timeout `Timeout`.

use crate::error::AppError;

/// Codes and messages of a refused AUTH / HELLO … AUTH, over every server version.
fn is_auth_failure(code: &str, detail: &str) -> bool {
    let d = detail.to_ascii_lowercase();
    matches!(code, "NOAUTH" | "WRONGPASS")
        || (code == "ERR"
            && (d.starts_with("invalid password")
                || d.starts_with("invalid username-password pair")
                || d.contains("called without any password configured")
                || d.starts_with("client sent auth, but no password is set")))
}

fn hint(code: &str) -> Option<String> {
    Some(
        match code {
            "NOPERM" => "The connection's ACL user may not run this command or touch this key.",
            "READONLY" => "The server is a read-only replica: connect to the primary to write.",
            "BUSY" => "A script is running; SCRIPT KILL or FUNCTION KILL stops it.",
            "LOADING" => "The server is still loading its data set into memory.",
            "OOM" => "The server reached maxmemory and its eviction policy refuses the write.",
            "MOVED" | "ASK" | "CROSSSLOT" | "CLUSTERDOWN" => {
                "The server is a cluster node; Tabler connects to standalone servers only."
            }
            _ => return None,
        }
        .to_string(),
    )
}

/// An error from code and detail as a server sent it.
pub fn from_parts(code: &str, detail: Option<&str>) -> AppError {
    let detail = detail.unwrap_or("");
    if is_auth_failure(code, detail) {
        return AppError::Auth(format!("{code} {detail}").trim().to_string());
    }
    AppError::Db {
        message: format!("{code} {detail}").trim().to_string(),
        detail: None,
        hint: hint(code),
        sqlstate: None,
        vendor_code: None,
        position: None,
        statement_index: None,
    }
}

/// A server error reply.
pub fn server_error(e: &::redis::ServerError) -> AppError {
    from_parts(e.code(), e.details())
}

/// An error of the client: the server's reply, a lost connection, a timeout, or the client's own.
pub fn redis_error(e: &::redis::RedisError) -> AppError {
    use ::redis::ErrorKind;
    if e.is_timeout() {
        return AppError::Timeout;
    }
    if e.is_connection_refusal() {
        return AppError::Connect(e.to_string());
    }
    if e.is_connection_dropped() || e.is_io_error() {
        return AppError::NotConnected;
    }
    match e.kind() {
        ErrorKind::AuthenticationFailed => AppError::Auth(e.to_string()),
        ErrorKind::RESP3NotSupported => AppError::Connect(
            "the server does not support RESP3 (HELLO 3); connect with RESP2".into(),
        ),
        ErrorKind::Server(_) | ErrorKind::Extension => match e.code() {
            Some(code) => from_parts(code, e.detail()),
            None => AppError::db(e.to_string()),
        },
        _ => AppError::Internal(format!("Redis client: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::redis::{Value, parse_redis_value};

    fn server(text: &str) -> AppError {
        match parse_redis_value(text.as_bytes()).unwrap() {
            Value::ServerError(e) => server_error(&e),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn server_errors_keep_their_text() {
        let e = server("-WRONGTYPE Operation against a key holding the wrong kind of value\r\n");
        let AppError::Db {
            message,
            sqlstate,
            vendor_code,
            hint,
            ..
        } = e
        else {
            panic!()
        };
        assert_eq!(
            message,
            "WRONGTYPE Operation against a key holding the wrong kind of value"
        );
        assert_eq!((sqlstate, vendor_code, hint), (None, None, None));
        assert_eq!(
            server("-ERR unknown command 'FOO'\r\n").to_string(),
            "ERR unknown command 'FOO'"
        );
        let AppError::Db { hint, .. } = server("-NOPERM User x has no permissions\r\n") else {
            panic!()
        };
        assert!(hint.unwrap().contains("ACL"));
        assert!(matches!(
            server("-READONLY You can't write\r\n"),
            AppError::Db { hint: Some(_), .. }
        ));
        assert_eq!(from_parts("OK", None).to_string(), "OK");
    }

    #[test]
    fn credentials() {
        for text in [
            "-WRONGPASS invalid username-password pair or user is disabled.\r\n",
            "-NOAUTH Authentication required.\r\n",
            "-ERR invalid password\r\n",
            "-ERR invalid username-password pair\r\n",
            "-ERR AUTH <password> called without any password configured for the default user. Are you sure your configuration is correct?\r\n",
            "-ERR Client sent AUTH, but no password is set\r\n",
        ] {
            assert!(matches!(server(text), AppError::Auth(_)), "{text}");
        }
        assert!(matches!(
            server("-ERR wrong number of arguments\r\n"),
            AppError::Db { .. }
        ));
    }

    #[test]
    fn client_errors() {
        let io: ::redis::RedisError =
            std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset").into();
        assert!(matches!(redis_error(&io), AppError::NotConnected));
        let timeout: ::redis::RedisError =
            std::io::Error::new(std::io::ErrorKind::TimedOut, "slow").into();
        assert!(matches!(redis_error(&timeout), AppError::Timeout));
        let refused: ::redis::RedisError =
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "no").into();
        assert!(matches!(redis_error(&refused), AppError::Connect(_)));
        let Value::ServerError(se) = parse_redis_value(b"-WRONGPASS bad\r\n").unwrap() else {
            panic!()
        };
        let as_client: ::redis::RedisError = se.into();
        assert!(matches!(redis_error(&as_client), AppError::Auth(_)));
        let Value::ServerError(se) = parse_redis_value(b"-ERR syntax error\r\n").unwrap() else {
            panic!()
        };
        let as_client: ::redis::RedisError = se.into();
        assert_eq!(redis_error(&as_client).to_string(), "ERR syntax error");
        let ext = ::redis::make_extension_error("BUSY".into(), Some("script running".into()));
        let AppError::Db { message, hint, .. } = redis_error(&ext) else {
            panic!()
        };
        assert_eq!(message, "BUSY script running");
        assert!(hint.is_some());
    }
}
