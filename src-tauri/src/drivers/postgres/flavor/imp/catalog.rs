//! What the engines' catalog code shares: rows read as text, optional queries, the column lists
//! of constraint definitions, and kill outcomes. Relation kinds, type modifiers and the table's
//! schema are `introspect`'s (`kind_of_relkind`, `typmod_info`, `schema_of`).

use tokio_postgres::Row;
use tokio_postgres::error::SqlState;

use crate::error::AppError;
use crate::types::admin::KillOutcome;
use crate::types::value::Value;

/// Each row's columns (all text in the query) as `Value::Text` or `Value::Null`, the shape
/// `admin::pg_process_rows` reads.
pub(super) fn text_rows(rows: &[Row]) -> Result<Vec<Vec<Value>>, AppError> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut values = Vec::with_capacity(row.len());
        for i in 0..row.len() {
            values.push(match row.try_get::<_, Option<String>>(i)? {
                Some(s) => Value::Text(s),
                None => Value::Null,
            });
        }
        out.push(values);
    }
    Ok(out)
}

/// A catalog relation, function, setting or privilege the server does not have or does not show
/// this user: what an optional query (row estimates, sizes, an older version's view or setting)
/// may meet.
pub(super) fn missing(e: &tokio_postgres::Error) -> bool {
    matches!(
        e.code(),
        Some(c) if *c == SqlState::UNDEFINED_TABLE
            || *c == SqlState::UNDEFINED_FUNCTION
            || *c == SqlState::UNDEFINED_COLUMN
            || *c == SqlState::UNDEFINED_OBJECT
            || *c == SqlState::INSUFFICIENT_PRIVILEGE
    )
}

/// The rows of an optional query: none when the server lacks what it reads (logged); other
/// errors stand.
pub(super) fn optional(
    rows: Result<Vec<Row>, tokio_postgres::Error>,
    what: &str,
) -> Result<Vec<Row>, AppError> {
    match rows {
        Ok(rows) => Ok(rows),
        Err(e) if missing(&e) => {
            log::info!("{what}: not available on this server: {e}");
            Ok(Vec::new())
        }
        Err(e) => Err(e.into()),
    }
}

/// The identifier lists in parentheses of a constraint definition, in order:
/// `FOREIGN KEY (a, "b c") REFERENCES t(x, y)` → `[[a, b c], [x, y]]`. Quoted names keep their
/// case and doubled quotes; bare names are as the server wrote them.
pub(super) fn paren_lists(def: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut chars = def.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                // A quoted name outside a list (a table): skip it whole.
                while let Some(q) = chars.next() {
                    if q == '"' {
                        if chars.peek() == Some(&'"') {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
            }
            '(' => {
                let mut names = Vec::new();
                let mut name = String::new();
                let mut quoted = false;
                while let Some(c) = chars.next() {
                    match c {
                        '"' if quoted && chars.peek() == Some(&'"') => {
                            chars.next();
                            name.push('"');
                        }
                        '"' => quoted = !quoted,
                        ',' if !quoted => names.push(std::mem::take(&mut name)),
                        ')' if !quoted => break,
                        c if quoted || !c.is_whitespace() => name.push(c),
                        _ => {}
                    }
                }
                names.push(name);
                out.push(names.into_iter().filter(|n| !n.is_empty()).collect());
            }
            _ => {}
        }
    }
    out
}

/// A kill that worked, did not find its process, or failed.
pub(super) fn kill_outcome(id: &str, result: Result<bool, AppError>) -> KillOutcome {
    match result {
        Ok(true) => KillOutcome {
            id: id.to_string(),
            ok: true,
            message: None,
        },
        Ok(false) => KillOutcome {
            id: id.to_string(),
            ok: false,
            message: Some("No such process: it may have ended already".into()),
        },
        Err(e) => KillOutcome {
            id: id.to_string(),
            ok: false,
            message: Some(e.to_string()),
        },
    }
}

/// `pg_cancel_backend` / `pg_terminate_backend` answer a boolean (PostgreSQL, Greenplum) or an
/// integer (Redshift: 1 when it was sent).
pub(super) fn signalled(row: Option<&Row>) -> bool {
    let Some(row) = row else {
        return false;
    };
    if let Ok(Some(b)) = row.try_get::<_, Option<bool>>(0) {
        return b;
    }
    matches!(row.try_get::<_, Option<i32>>(0), Ok(Some(n)) if n != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_lists_of_constraint_definitions() {
        assert_eq!(
            paren_lists("PRIMARY KEY (id, \"Order \"\"No\"\"\")"),
            [vec!["id".to_string(), "Order \"No\"".to_string()]]
        );
        assert_eq!(
            paren_lists("FOREIGN KEY (a, b) REFERENCES \"s(x)\".t(x, y)"),
            [
                vec!["a".to_string(), "b".to_string()],
                vec!["x".to_string(), "y".to_string()]
            ]
        );
        assert_eq!(paren_lists("UNIQUE (\"a,b\")"), [vec!["a,b".to_string()]]);
        assert!(paren_lists("CHECK").is_empty());
    }

    #[test]
    fn maps_kill_outcomes() {
        let failed = kill_outcome("7", Err(AppError::NotConnected));
        assert!(!failed.ok && failed.message.is_some());
        assert!(kill_outcome("7", Ok(true)).ok);
        assert!(!kill_outcome("7", Ok(false)).ok);
    }
}
