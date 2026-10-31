//! SQLite result-code names, as both engines report them (`SQLITE_CONSTRAINT_UNIQUE` in a
//! Hrana error's `code`, `…: SQLITE_CONSTRAINT` at the end of a D1 message), and the numbers
//! the local driver puts in `vendor_code`.

use rusqlite::ffi as f;

const CODES: [(&str, i32); 32] = [
    ("ERROR", f::SQLITE_ERROR),
    ("INTERNAL", f::SQLITE_INTERNAL),
    ("PERM", f::SQLITE_PERM),
    ("ABORT", f::SQLITE_ABORT),
    ("BUSY", f::SQLITE_BUSY),
    ("LOCKED", f::SQLITE_LOCKED),
    ("NOMEM", f::SQLITE_NOMEM),
    ("READONLY", f::SQLITE_READONLY),
    ("INTERRUPT", f::SQLITE_INTERRUPT),
    ("IOERR", f::SQLITE_IOERR),
    ("CORRUPT", f::SQLITE_CORRUPT),
    ("NOTFOUND", f::SQLITE_NOTFOUND),
    ("FULL", f::SQLITE_FULL),
    ("CANTOPEN", f::SQLITE_CANTOPEN),
    ("PROTOCOL", f::SQLITE_PROTOCOL),
    ("SCHEMA", f::SQLITE_SCHEMA),
    ("TOOBIG", f::SQLITE_TOOBIG),
    ("CONSTRAINT", f::SQLITE_CONSTRAINT),
    ("MISMATCH", f::SQLITE_MISMATCH),
    ("MISUSE", f::SQLITE_MISUSE),
    ("AUTH", f::SQLITE_AUTH),
    ("RANGE", f::SQLITE_RANGE),
    ("NOTADB", f::SQLITE_NOTADB),
    ("CONSTRAINT_CHECK", f::SQLITE_CONSTRAINT_CHECK),
    ("CONSTRAINT_FOREIGNKEY", f::SQLITE_CONSTRAINT_FOREIGNKEY),
    ("CONSTRAINT_NOTNULL", f::SQLITE_CONSTRAINT_NOTNULL),
    ("CONSTRAINT_PRIMARYKEY", f::SQLITE_CONSTRAINT_PRIMARYKEY),
    ("CONSTRAINT_TRIGGER", f::SQLITE_CONSTRAINT_TRIGGER),
    ("CONSTRAINT_UNIQUE", f::SQLITE_CONSTRAINT_UNIQUE),
    ("CONSTRAINT_ROWID", f::SQLITE_CONSTRAINT_ROWID),
    ("CONSTRAINT_DATATYPE", f::SQLITE_CONSTRAINT_DATATYPE),
    ("READONLY_DBMOVED", f::SQLITE_READONLY_DBMOVED),
];

/// The result code a name stands for (`SQLITE_CONSTRAINT_UNIQUE` → 2067).
pub fn sqlite_code(name: &str) -> Option<i64> {
    let short = name.trim().strip_prefix("SQLITE_")?;
    CODES
        .iter()
        .find(|(n, _)| *n == short)
        .map(|(_, c)| i64::from(*c))
}

/// The name of a result code; an extended code nobody named goes by its primary code. The fake
/// servers word their errors with it.
#[cfg(test)]
pub fn code_name(code: i32) -> String {
    let named = |c: i32| CODES.iter().find(|(_, n)| *n == c).map(|(name, _)| *name);
    match named(code).or_else(|| named(code & 0xff)) {
        Some(name) => format!("SQLITE_{name}"),
        None => "SQLITE_ERROR".to_string(),
    }
}

/// A message that ends with a code name (`no such table: t: SQLITE_ERROR`): the message
/// without it, and the code.
pub fn split_code_suffix(message: &str) -> (&str, Option<i64>) {
    if let Some(at) = message.rfind(": SQLITE_") {
        let name = message[at + 2..].trim();
        if let Some(code) = sqlite_code(name) {
            return (message[..at].trim_end(), Some(code));
        }
    }
    (message, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_codes() {
        assert_eq!(sqlite_code("SQLITE_CONSTRAINT_UNIQUE"), Some(2067));
        assert_eq!(sqlite_code("SQLITE_CONSTRAINT"), Some(19));
        assert_eq!(sqlite_code("CONSTRAINT"), None);
        assert_eq!(sqlite_code("SQLITE_WHATEVER"), None);
        assert_eq!(code_name(2067), "SQLITE_CONSTRAINT_UNIQUE");
        assert_eq!(code_name(8), "SQLITE_READONLY");
        // SQLITE_IOERR_READ, which the table does not name.
        assert_eq!(code_name(266), "SQLITE_IOERR");
        assert_eq!(code_name(999), "SQLITE_ERROR");
    }

    #[test]
    fn suffixes() {
        assert_eq!(
            split_code_suffix("UNIQUE constraint failed: t.v: SQLITE_CONSTRAINT"),
            ("UNIQUE constraint failed: t.v", Some(19))
        );
        assert_eq!(
            split_code_suffix("no such table: t"),
            ("no such table: t", None)
        );
        assert_eq!(
            split_code_suffix("x: SQLITE_NOPE"),
            ("x: SQLITE_NOPE", None)
        );
    }
}
