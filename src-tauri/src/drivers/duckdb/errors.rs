//! DuckDB errors as `AppError`s.
//!
//! DuckDB has no error numbers: a message starts with the kind of the exception (`Catalog Error:
//! Table with name x does not exist!`). The kind decides the mapping; the message stays as DuckDB
//! wrote it, with a hint where Tabler knows the way out (a file another process holds, a session
//! that may not read files, a write conflict between two connections).

use crate::error::AppError;

/// What to do about a file another DuckDB holds (DuckDB opens a file read-write in one process
/// at a time).
pub const FILE_LOCK_HINT: &str = "Another program (or another DuckDB in this one) has the file \
     open for writing. Close it there, close it in Tabler, or open the connection read-only.";

/// Why a hardened session refuses to read a file or reach another database.
pub const EXTERNAL_ACCESS_HINT: &str = "This session may not read files or reach other \
     databases: it is read-only, or it runs for MCP or a plugin.";

const CONFLICT_HINT: &str = "Another connection changed the same rows at the same time. Run it \
     again.";

const VERSION_HINT: &str = "The file was written by a newer DuckDB than the one in Tabler. Open \
     it with that DuckDB and EXPORT DATABASE, or wait for a Tabler with a newer DuckDB.";

/// The kind DuckDB names in front of a message (`Catalog` for `Catalog Error: …`).
pub fn error_type(message: &str) -> Option<&str> {
    let (head, _) = message.split_once(" Error: ")?;
    (!head.is_empty() && head.len() <= 32 && !head.contains('\n')).then_some(head)
}

/// Whether the message is DuckDB's refusal to lock a database file: the lock error on macOS and
/// Linux, and on Windows the sharing violation of the open, which DuckDB follows with the
/// processes that hold the file (in its own words, unlike the system's localized message).
pub fn is_lock_conflict(message: &str) -> bool {
    message.contains("Could not set lock on file")
        || message.contains("Conflicting lock is held")
        || message.contains("File is already open in")
}

fn hint_for(kind: Option<&str>, message: &str) -> Option<&'static str> {
    if is_lock_conflict(message) {
        return Some(FILE_LOCK_HINT);
    }
    let lower = message.to_ascii_lowercase();
    match kind {
        Some("Permission")
            if lower.contains("external access") || lower.contains("disabled by configuration") =>
        {
            Some(EXTERNAL_ACCESS_HINT)
        }
        Some("TransactionContext" | "Transaction") if lower.contains("conflict") => {
            Some(CONFLICT_HINT)
        }
        Some("Serialization" | "IO" | "Invalid Input")
            if lower.contains("storage version") || lower.contains("newer version") =>
        {
            Some(VERSION_HINT)
        }
        _ => None,
    }
}

/// A message from DuckDB as an `AppError`: an interrupt is `Cancelled`, everything else `Db`.
pub fn from_message(message: String) -> AppError {
    let kind = error_type(&message);
    if kind == Some("INTERRUPT") {
        return AppError::Cancelled;
    }
    AppError::Db {
        hint: hint_for(kind, &message).map(str::to_string),
        message,
        detail: None,
        sqlstate: None,
        vendor_code: None,
        position: None,
        statement_index: None,
    }
}

/// A duckdb-rs error as an `AppError`.
pub fn map(err: ::duckdb::Error) -> AppError {
    match err {
        ::duckdb::Error::DuckDBFailure(_, Some(message)) => from_message(message),
        other => AppError::db(other.to_string()),
    }
}

/// An error of opening a database: the reason, and the hint where one helps.
pub fn open_error(path: &str, err: ::duckdb::Error) -> AppError {
    let message = match err {
        ::duckdb::Error::DuckDBFailure(_, Some(message)) => message,
        other => other.to_string(),
    };
    let hint = hint_for(error_type(&message), &message);
    AppError::Connect(match hint {
        Some(hint) => format!("cannot open {path}: {message} {hint}"),
        None => format!("cannot open {path}: {message}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_hints() {
        assert_eq!(
            error_type("Catalog Error: Table with name x does not exist!"),
            Some("Catalog")
        );
        assert_eq!(error_type("no kind here"), None);
        assert!(matches!(
            from_message("INTERRUPT Error: Interrupted!".into()),
            AppError::Cancelled
        ));
        let AppError::Db { hint, message, .. } = from_message(
            "IO Error: Could not set lock on file \"C:\\x.duckdb\": Conflicting lock is held in \
             C:\\duckdb.exe (PID 1)"
                .into(),
        ) else {
            panic!("not a Db error");
        };
        assert_eq!(hint.as_deref(), Some(FILE_LOCK_HINT));
        assert!(message.starts_with("IO Error"));
        let AppError::Db { hint, .. } = from_message(
            "Permission Error: Cannot access file \"x.csv\" - file system operations are \
             disabled by configuration"
                .into(),
        ) else {
            panic!("not a Db error");
        };
        assert_eq!(hint.as_deref(), Some(EXTERNAL_ACCESS_HINT));
        let AppError::Db { hint, .. } = from_message(
            "TransactionContext Error: Failed to commit: write-write conflict on key".into(),
        ) else {
            panic!("not a Db error");
        };
        assert_eq!(hint.as_deref(), Some(CONFLICT_HINT));
        let AppError::Db { hint, .. } =
            from_message("Constraint Error: Duplicate key \"id: 1\"".into())
        else {
            panic!("not a Db error");
        };
        assert_eq!(hint, None);
    }

    #[test]
    fn open_errors_carry_the_lock_hint() {
        let err = open_error(
            "C:/x.duckdb",
            ::duckdb::Error::DuckDBFailure(
                ::duckdb::ffi::Error::new(1),
                Some("IO Error: Could not set lock on file \"C:/x.duckdb\"".into()),
            ),
        );
        let AppError::Connect(text) = err else {
            panic!("not a Connect error");
        };
        assert!(text.contains(FILE_LOCK_HINT), "{text}");
        // Windows: the open fails with a sharing violation and the holders.
        let AppError::Db { hint, .. } = from_message(
            "IO Error: Cannot open file \"C:\\x.duckdb\": The process cannot access the file \
             because it is being used by another process.\r\n\nFile is already open in \n\
             C:\\duckdb.exe (PID 7)"
                .into(),
        ) else {
            panic!("not a Db error");
        };
        assert_eq!(hint.as_deref(), Some(FILE_LOCK_HINT));
    }
}
