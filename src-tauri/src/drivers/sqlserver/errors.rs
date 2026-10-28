//! tiberius errors as `AppError`s: server messages keep their number in `vendor_code` (SQL
//! Server sends no SQLSTATE), bad logins are `Auth`, a lost transport is `NotConnected`.

use ::tiberius::error::{Error, IoErrorKind, TokenError};

use crate::error::{AppError, ErrorPos};

/// Where an error happened: logging in, or running a statement on a live connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Connect,
    Run,
}

/// Login errors (`Login failed`, untrusted domain, locked, expired or must-change passwords).
const AUTH_CODES: &[u32] = &[18452, 18456, 18470, 18486, 18487, 18488];

/// Maps a tiberius error; `sql` is the statement it came from, for the position of the line the
/// server names.
pub fn map_error(e: Error, phase: Phase, sql: Option<&str>) -> AppError {
    match e {
        Error::Server(t) => server_error(&t, phase, sql),
        Error::Io { kind, message } => match kind {
            IoErrorKind::TimedOut => AppError::Timeout,
            _ if phase == Phase::Connect => AppError::Connect(format!("SQL Server: {message}")),
            _ => AppError::NotConnected,
        },
        Error::Tls(message) => AppError::Connect(format!(
            "SQL Server TLS: {message}. A server with its own self-signed certificate needs SSL \
             mode Prefer or Require (no certificate check), or its CA certificate with Verify."
        )),
        // A request whose write was cut short leaves the connection unusable.
        Error::Protocol(message) if phase == Phase::Run => {
            log::warn!("SQL Server connection dropped: {message}");
            AppError::NotConnected
        }
        Error::Protocol(message) => AppError::Connect(format!("SQL Server: {message}")),
        Error::Routing { host, port } => AppError::Connect(format!(
            "SQL Server redirected the connection to {host}:{port}"
        )),
        Error::BulkInput(message) => AppError::InvalidInput(message.into_owned()),
        other => AppError::Internal(format!("SQL Server: {other}")),
    }
}

/// A message the server raised.
pub fn server_error(t: &TokenError, phase: Phase, sql: Option<&str>) -> AppError {
    let message = t.message().to_string();
    if phase == Phase::Connect && AUTH_CODES.contains(&t.code()) {
        return AppError::Auth(message);
    }
    let mut detail = format!("Msg {}, Level {}, State {}", t.code(), t.class(), t.state());
    if !t.procedure().is_empty() {
        detail.push_str(&format!(", Procedure {}", t.procedure()));
    }
    if t.line() > 0 {
        detail.push_str(&format!(", Line {}", t.line()));
    }
    AppError::Db {
        message,
        detail: Some(detail),
        hint: None,
        sqlstate: None,
        vendor_code: Some(i64::from(t.code())),
        // A line of a procedure is not a line of the statement.
        position: sql
            .filter(|_| t.procedure().is_empty())
            .and_then(|sql| line_position(sql, t.line())),
        statement_index: None,
    }
}

/// The start of 1-based line `line` of `sql`, when it has that many lines.
pub fn line_position(sql: &str, line: u32) -> Option<ErrorPos> {
    if line == 0 {
        return None;
    }
    let mut offset = 0usize;
    let mut current = 1u32;
    for c in sql.chars() {
        if current == line {
            break;
        }
        if c == '\n' {
            current += 1;
        }
        offset += 1;
    }
    (current == line).then(|| ErrorPos::from_offset(sql, offset))
}

/// Deadlock victim (1205) and lock timeout (1222): worth trying again.
pub fn is_retryable(e: &AppError) -> bool {
    matches!(
        e,
        AppError::Db {
            vendor_code: Some(1205 | 1222),
            ..
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_and_tls_errors() {
        let io = |kind| Error::Io {
            kind,
            message: "boom".into(),
        };
        assert!(matches!(
            map_error(io(IoErrorKind::TimedOut), Phase::Run, None),
            AppError::Timeout
        ));
        assert!(matches!(
            map_error(io(IoErrorKind::ConnectionReset), Phase::Run, None),
            AppError::NotConnected
        ));
        assert!(matches!(
            map_error(io(IoErrorKind::ConnectionRefused), Phase::Connect, None),
            AppError::Connect(m) if m.contains("boom")
        ));
        assert!(matches!(
            map_error(Error::Tls("bad cert".into()), Phase::Connect, None),
            AppError::Connect(m) if m.contains("bad cert") && m.contains("Prefer")
        ));
        assert!(matches!(
            map_error(Error::Protocol("poisoned".into()), Phase::Run, None),
            AppError::NotConnected
        ));
    }

    #[test]
    fn lines_become_positions() {
        let sql = "SELECT 1\nSELECT x\r\nFROM t";
        let pos = line_position(sql, 2).unwrap();
        assert_eq!((pos.offset, pos.line, pos.column), (9, 2, 1));
        assert_eq!(line_position(sql, 3).unwrap().offset, 19);
        assert!(line_position(sql, 4).is_none() && line_position(sql, 0).is_none());
    }
}
