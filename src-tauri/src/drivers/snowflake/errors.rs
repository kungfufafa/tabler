//! The connector's errors as `AppError`s: a refused sign-in is `Auth`, a cancel `Cancelled`, a
//! lost or expired session `NotConnected` (the driver signs in again), a deadline `Timeout`, and
//! what Snowflake refuses `Db` with its code (`002003`) as `vendor_code` and the position of a
//! compilation error when the message gives one (`line 1 at position 7`).

use snowflake_connector_rs::{Error, ErrorKind};

use crate::error::{AppError, ErrorPos};

/// Snowflake's message when it sent one, the connector's otherwise.
fn message(e: &Error) -> String {
    e.snowflake_message()
        .map(str::to_string)
        .unwrap_or_else(|| e.to_string())
}

/// An error while signing in: a network failure is `Connect` (nothing was connected yet).
pub fn connect_error(e: Error) -> AppError {
    match e.kind() {
        ErrorKind::Network => AppError::Connect(format!("Snowflake: {e}")),
        ErrorKind::Config => AppError::InvalidInput(format!("Snowflake: {e}")),
        _ => map_error(e, None),
    }
}

/// An error of a statement (`sql`, when known, places a compilation error in it).
pub fn map_error(e: Error, sql: Option<&str>) -> AppError {
    match e.kind() {
        ErrorKind::Auth => AppError::Auth(message(&e)),
        ErrorKind::Cancelled => AppError::Cancelled,
        ErrorKind::SessionExpired | ErrorKind::Network => AppError::NotConnected,
        ErrorKind::Timeout => AppError::Timeout,
        ErrorKind::Server => {
            let text = message(&e);
            let position = sql.and_then(|s| error_position(&text, s));
            AppError::Db {
                vendor_code: e.snowflake_code().and_then(|c| c.trim().parse().ok()),
                message: text,
                detail: e.query_id().map(|id| format!("Query ID: {id}")),
                hint: None,
                sqlstate: None,
                position,
                statement_index: None,
            }
        }
        ErrorKind::BindEncode | ErrorKind::Config => {
            AppError::InvalidInput(format!("Snowflake: {e}"))
        }
        _ => AppError::Internal(format!("Snowflake: {e}")),
    }
}

/// `line L at position P` (1-based line, 0-based position in it) as a place in `sql`. Every
/// `line ` counts, since a name or word before the real one may hold it (`Pipeline`, `deadline`).
pub fn error_position(message: &str, sql: &str) -> Option<ErrorPos> {
    let (line, column) = message
        .match_indices("line ")
        .find_map(|(at, _)| line_and_position(&message[at + "line ".len()..]))?;
    if line == 0 {
        return None;
    }
    // The offset of the line's start, in characters, then the position within it.
    let mut offset = 0usize;
    for (i, l) in sql.split('\n').enumerate() {
        if i + 1 == line {
            if column > l.chars().count() {
                return None;
            }
            return Some(ErrorPos::from_offset(sql, offset + column));
        }
        offset += l.chars().count() + 1;
    }
    None
}

/// `L at position P…` (what follows a `line `) as its two numbers.
fn line_and_position(rest: &str) -> Option<(usize, usize)> {
    let line_end = rest.find(|c: char| !c.is_ascii_digit())?;
    let line: usize = rest[..line_end].parse().ok()?;
    let rest = rest[line_end..].strip_prefix(" at position ")?;
    let pos_end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let column: usize = rest[..pos_end].parse().ok()?;
    Some((line, column))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compilation_errors_point_into_the_statement() {
        let sql = "SELECT 1\nFORM t";
        let msg = "SQL compilation error:\nsyntax error line 2 at position 0 unexpected 'FORM'.";
        let pos = error_position(msg, sql).unwrap();
        assert_eq!((pos.line, pos.column, pos.offset), (2, 1, 9));
        assert!(error_position("SQL compilation error: Object 'T' does not exist", sql).is_none());
        assert!(error_position("line 9 at position 0", sql).is_none());
        assert!(error_position("line 1 at position 99", sql).is_none());
        assert!(error_position("line 0 at position 1", sql).is_none());
        assert!(error_position("line x at position 1", sql).is_none());
        // Words holding `line ` before the real one.
        for msg in [
            "SQL compilation error: Pipeline 'P' … syntax error line 2 at position 0 unexpected.",
            "deadline exceeded; online check: line 2 at position 0",
            "line 1 is long; line 2 at position 0",
        ] {
            let pos = error_position(msg, sql).unwrap();
            assert_eq!((pos.line, pos.column, pos.offset), (2, 1, 9), "{msg}");
        }
    }
}
