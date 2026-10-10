use serde::Serialize;
use ts_rs::TS;

/// Position of an error inside a statement (0-based character offset plus 1-based line/column).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ErrorPos {
    pub offset: u32,
    pub line: u32,
    pub column: u32,
}

impl ErrorPos {
    /// Builds a position from a 0-based char offset into `sql`.
    pub fn from_offset(sql: &str, offset: usize) -> ErrorPos {
        let mut line = 1u32;
        let mut column = 1u32;
        for (i, ch) in sql.chars().enumerate() {
            if i >= offset {
                break;
            }
            if ch == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        ErrorPos {
            offset: offset as u32,
            line,
            column,
        }
    }
}

/// What the renderer receives for every failed command.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
    pub hint: Option<String>,
    pub sqlstate: Option<String>,
    pub vendor_code: Option<i64>,
    pub position: Option<ErrorPos>,
    pub statement_index: Option<usize>,
    #[ts(type = "unknown")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum AppError {
    #[error("{message}")]
    Db {
        message: String,
        detail: Option<String>,
        hint: Option<String>,
        sqlstate: Option<String>,
        vendor_code: Option<i64>,
        position: Option<ErrorPos>,
        statement_index: Option<usize>,
    },
    #[error("query cancelled")]
    Cancelled,
    #[error("{0}")]
    Connect(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("operation timed out")]
    Timeout,
    #[error("ssh tunnel: {0}")]
    Tunnel(String),
    #[error("unknown host key for {host}:{port}")]
    HostKey {
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
        changed: bool,
    },
    #[error("not connected")]
    NotConnected,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("keychain: {0}")]
    Secrets(String),
    #[error("app database: {0}")]
    AppDb(String),
    #[error("io: {0}")]
    Io(String),
    #[error("{0}")]
    InvalidInput(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("{0}")]
    Internal(String),
}

impl AppError {
    pub fn code(&self) -> &'static str {
        match self {
            AppError::Db { .. } => "DB_ERROR",
            AppError::Cancelled => "CANCELLED",
            AppError::Connect(_) => "CONNECT_FAILED",
            AppError::Auth(_) => "AUTH_FAILED",
            AppError::Timeout => "TIMEOUT",
            AppError::Tunnel(_) => "SSH_TUNNEL",
            AppError::HostKey { changed: false, .. } => "SSH_HOST_KEY_UNKNOWN",
            AppError::HostKey { changed: true, .. } => "SSH_HOST_KEY_CHANGED",
            AppError::NotConnected => "NOT_CONNECTED",
            AppError::NotFound(_) => "NOT_FOUND",
            AppError::Secrets(_) => "SECRETS",
            AppError::AppDb(_) => "APP_DB",
            AppError::Io(_) => "IO",
            AppError::InvalidInput(_) => "INVALID_INPUT",
            AppError::Unsupported(_) => "UNSUPPORTED",
            AppError::Internal(_) => "INTERNAL",
        }
    }

    pub fn db(message: impl Into<String>) -> AppError {
        AppError::Db {
            message: message.into(),
            detail: None,
            hint: None,
            sqlstate: None,
            vendor_code: None,
            position: None,
            statement_index: None,
        }
    }

    pub fn internal(message: impl Into<String>) -> AppError {
        AppError::Internal(message.into())
    }

    pub fn with_statement_index(mut self, index: usize) -> AppError {
        if let AppError::Db {
            statement_index, ..
        } = &mut self
        {
            *statement_index = Some(index);
        }
        self
    }

    pub fn to_payload(&self) -> ErrorPayload {
        let mut payload = ErrorPayload {
            code: self.code().to_string(),
            message: self.to_string(),
            detail: None,
            hint: None,
            sqlstate: None,
            vendor_code: None,
            position: None,
            statement_index: None,
            data: None,
        };
        match self {
            AppError::Db {
                detail,
                hint,
                sqlstate,
                vendor_code,
                position,
                statement_index,
                ..
            } => {
                payload.detail = detail.clone();
                payload.hint = hint.clone();
                payload.sqlstate = sqlstate.clone();
                payload.vendor_code = *vendor_code;
                payload.position = position.clone();
                payload.statement_index = *statement_index;
            }
            AppError::HostKey {
                host,
                port,
                key_type,
                fingerprint,
                changed,
            } => {
                payload.data = Some(serde_json::json!({
                    "host": host, "port": port, "keyType": key_type,
                    "fingerprint": fingerprint, "changed": changed
                }));
            }
            _ => {}
        }
        payload
    }
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_payload().serialize(serializer)
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Internal(format!("json: {e}"))
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        use rusqlite::Error as E;
        match e {
            E::SqliteFailure(ffi, msg) => {
                let code = ffi.extended_code;
                if ffi.code == rusqlite::ErrorCode::OperationInterrupted {
                    return AppError::Cancelled;
                }
                let hint = if ffi.code == rusqlite::ErrorCode::DatabaseBusy
                    || ffi.code == rusqlite::ErrorCode::DatabaseLocked
                {
                    Some("The database is locked by another writer.".to_string())
                } else {
                    None
                };
                AppError::Db {
                    message: msg.unwrap_or_else(|| ffi.to_string()),
                    detail: None,
                    hint,
                    sqlstate: None,
                    vendor_code: Some(code as i64),
                    position: None,
                    statement_index: None,
                }
            }
            E::SqlInputError {
                error, msg, offset, ..
            } => AppError::Db {
                message: msg,
                detail: None,
                hint: None,
                sqlstate: None,
                vendor_code: Some(error.extended_code as i64),
                // rusqlite reports a byte offset; convert at the call site when the SQL is known.
                position: Some(ErrorPos {
                    offset: offset as u32,
                    line: 0,
                    column: 0,
                }),
                statement_index: None,
            },
            other => AppError::db(other.to_string()),
        }
    }
}

impl From<tokio_postgres::Error> for AppError {
    fn from(e: tokio_postgres::Error) -> Self {
        if let Some(db) = e.as_db_error() {
            let code = db.code().code().to_string();
            if code == "57014" {
                return AppError::Cancelled;
            }
            if code == "28P01" || code == "28000" {
                return AppError::Auth(db.message().to_string());
            }
            let position = match db.position() {
                Some(tokio_postgres::error::ErrorPosition::Original(p)) => Some(ErrorPos {
                    // PG positions are 1-based character offsets.
                    offset: p.saturating_sub(1),
                    line: 0,
                    column: 0,
                }),
                _ => None,
            };
            let mut detail = db.detail().map(str::to_string);
            if let Some(tokio_postgres::error::ErrorPosition::Internal { query, position }) =
                db.position()
            {
                detail = Some(format!(
                    "{}\nin internal query at position {position}: {query}",
                    detail.unwrap_or_default()
                ));
            }
            return AppError::Db {
                message: db.message().to_string(),
                detail,
                hint: db.hint().map(str::to_string),
                sqlstate: Some(code),
                vendor_code: None,
                position,
                statement_index: None,
            };
        }
        if e.is_closed() {
            return AppError::NotConnected;
        }
        let text = e.to_string();
        if text.contains("timed out") || text.contains("timeout") {
            return AppError::Timeout;
        }
        AppError::Connect(text)
    }
}

impl From<mysql_async::Error> for AppError {
    fn from(e: mysql_async::Error) -> Self {
        use mysql_async::Error as E;
        match e {
            E::Server(err) => {
                let code = err.code as i64;
                match code {
                    1317 => AppError::Cancelled,
                    1045 | 1698 => AppError::Auth(err.message.clone()),
                    _ => AppError::Db {
                        message: err.message.clone(),
                        detail: None,
                        hint: None,
                        sqlstate: Some(err.state.clone()),
                        vendor_code: Some(code),
                        position: None,
                        statement_index: None,
                    },
                }
            }
            E::Driver(mysql_async::DriverError::ConnectionClosed) => AppError::NotConnected,
            E::Io(io) => {
                let text = io.to_string();
                if text.contains("timed out") {
                    AppError::Timeout
                } else {
                    AppError::Connect(text)
                }
            }
            other => AppError::Connect(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_line_and_column_from_offset() {
        let sql = "select 1;\nselect oops\n  from x";
        let pos = ErrorPos::from_offset(sql, 17);
        assert_eq!((pos.line, pos.column), (2, 8));
        let pos = ErrorPos::from_offset(sql, 0);
        assert_eq!((pos.line, pos.column), (1, 1));
    }

    #[test]
    fn serializes_as_payload() {
        let err = AppError::db("boom").with_statement_index(2);
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["code"], "DB_ERROR");
        assert_eq!(json["message"], "boom");
        assert_eq!(json["statementIndex"], 2);
    }
}
