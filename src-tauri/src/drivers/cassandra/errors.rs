//! The scylla driver's errors as `AppError`s: bad credentials are `Auth`, a lost connection
//! `NotConnected`, the driver's own request timeout `Timeout`, and what the server refused `Db`
//! with its CQL error code in `vendor_code` (CQL has no SQLSTATE).

use ::scylla::errors::{
    ConnectionError, ConnectionPoolError, ConnectionSetupRequestErrorKind, DbError, ExecutionError,
    MetadataError, NewSessionError, PrepareError, RequestAttemptError, UseKeyspaceError,
};
use ::scylla::frame::protocol_features::ProtocolFeatures;

use crate::error::AppError;

/// An error the server answered with.
pub fn db_error(err: &DbError, message: &str) -> AppError {
    match err {
        DbError::AuthenticationError => AppError::Auth(message.to_string()),
        err => {
            let code = err.code(&ProtocolFeatures::default());
            AppError::Db {
                message: if message.is_empty() {
                    err.to_string()
                } else {
                    message.to_string()
                },
                detail: None,
                hint: None,
                sqlstate: None,
                vendor_code: Some(i64::from(code)),
                position: None,
                statement_index: None,
            }
        }
    }
}

pub fn attempt(e: &RequestAttemptError) -> AppError {
    match e {
        RequestAttemptError::DbError(err, message) => db_error(err, message),
        RequestAttemptError::BrokenConnectionError(_) => AppError::NotConnected,
        RequestAttemptError::SerializationError(e) => {
            AppError::InvalidInput(format!("Cassandra: a bound value does not fit: {e}"))
        }
        other => AppError::db(format!("Cassandra: {other}")),
    }
}

pub fn execution(e: ExecutionError) -> AppError {
    match e {
        ExecutionError::RequestTimeout(_) => AppError::Timeout,
        ExecutionError::LastAttemptError(e) => attempt(&e),
        ExecutionError::ConnectionPoolError(e) => pool(&e),
        ExecutionError::PrepareError(e) => prepare(e),
        ExecutionError::UseKeyspaceError(e) => use_keyspace(e),
        ExecutionError::BadQuery(e) => AppError::InvalidInput(format!("Cassandra: {e}")),
        other => AppError::db(format!("Cassandra: {other}")),
    }
}

pub fn prepare(e: PrepareError) -> AppError {
    match e {
        PrepareError::ConnectionPoolError(e) => pool(&e),
        PrepareError::AllAttemptsFailed { first_attempt } => attempt(&first_attempt),
        other => AppError::db(format!("Cassandra: {other}")),
    }
}

pub fn use_keyspace(e: UseKeyspaceError) -> AppError {
    match e {
        UseKeyspaceError::RequestError(e) => attempt(&e),
        UseKeyspaceError::BadKeyspaceName(e) => AppError::InvalidInput(format!("Cassandra: {e}")),
        UseKeyspaceError::RequestTimeout(_) => AppError::Timeout,
        other => AppError::db(format!("Cassandra: {other}")),
    }
}

fn pool(e: &ConnectionPoolError) -> AppError {
    match e {
        ConnectionPoolError::Broken {
            last_connection_error,
        } => match connection(last_connection_error) {
            AppError::Connect(_) => AppError::NotConnected,
            other => other,
        },
        _ => AppError::NotConnected,
    }
}

/// Why a connection could not be opened (or broke).
fn connection(e: &ConnectionError) -> AppError {
    match e {
        ConnectionError::ConnectionSetupRequestError(setup) => match &setup.error {
            ConnectionSetupRequestErrorKind::DbError(err, message) => db_error(err, message),
            _ if is_auth(&setup.to_string()) => AppError::Auth(setup.to_string()),
            _ => AppError::Connect(format!("Cassandra: {setup}")),
        },
        ConnectionError::UseKeyspaceError(e) => use_keyspace(e.clone()),
        other => AppError::Connect(format!("Cassandra: {other}")),
    }
}

/// Our authenticator's own failures (a Command password that did not run) come back as text.
fn is_auth(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("authenticat") || lower.contains("password")
}

/// Why a session could not be opened.
pub fn new_session(e: NewSessionError) -> AppError {
    match e {
        NewSessionError::MetadataError(MetadataError::ConnectionPoolError(
            ConnectionPoolError::Broken {
                last_connection_error,
            },
        )) => connection(&last_connection_error),
        NewSessionError::UseKeyspaceError(e) => use_keyspace(e),
        NewSessionError::FailedToResolveAnyHostname(hosts) => {
            AppError::Connect(format!("Cassandra: cannot resolve {}", hosts.join(", ")))
        }
        other => {
            let text = other.to_string();
            if is_auth(&text) {
                AppError::Auth(text)
            } else {
                AppError::Connect(format!("Cassandra: {text}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_errors_keep_their_cql_code() {
        let e = db_error(&DbError::Invalid, "unconfigured table nope");
        assert!(matches!(
            e,
            AppError::Db { ref message, vendor_code: Some(0x2200), sqlstate: None, .. }
                if message == "unconfigured table nope"
        ));
        let syntax = attempt(&RequestAttemptError::DbError(
            DbError::SyntaxError,
            "line 1:0 no viable alternative".into(),
        ));
        assert!(matches!(
            syntax,
            AppError::Db {
                vendor_code: Some(0x2000),
                ..
            }
        ));
        assert!(matches!(
            db_error(
                &DbError::AuthenticationError,
                "Provided username and/or password are incorrect"
            ),
            AppError::Auth(_)
        ));
        assert!(matches!(
            execution(ExecutionError::RequestTimeout(
                std::time::Duration::from_secs(1)
            )),
            AppError::Timeout
        ));
        assert!(matches!(
            execution(ExecutionError::LastAttemptError(
                RequestAttemptError::DbError(DbError::Unauthorized, "no SELECT permission".into())
            )),
            AppError::Db {
                vendor_code: Some(0x2100),
                ..
            }
        ));
        assert!(matches!(
            execution(ExecutionError::ConnectionPoolError(
                ConnectionPoolError::Initializing
            )),
            AppError::NotConnected
        ));
        assert!(matches!(
            new_session(NewSessionError::FailedToResolveAnyHostname(vec!["nope:9042".into()])),
            AppError::Connect(m) if m.contains("nope:9042")
        ));
        assert!(matches!(
            new_session(NewSessionError::MetadataError(
                MetadataError::ConnectionPoolError(ConnectionPoolError::Broken {
                    last_connection_error: ConnectionError::ConnectTimeout,
                },)
            )),
            AppError::Connect(_)
        ));
    }
}
