use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// SQL dialect used for quoting and statement generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Dialect {
    Postgres,
    Mysql,
    Sqlite,
}

impl Dialect {
    /// Quotes an identifier (table, column, schema). Always quotes, never trusts the input.
    pub fn quote_ident(self, ident: &str) -> String {
        match self {
            Dialect::Postgres | Dialect::Sqlite => format!("\"{}\"", ident.replace('"', "\"\"")),
            Dialect::Mysql => format!("`{}`", ident.replace('`', "``")),
        }
    }

    /// Quotes a string literal for *display only* (Preview SQL). Execution always binds parameters.
    pub fn quote_literal(self, s: &str) -> String {
        match self {
            Dialect::Postgres | Dialect::Sqlite => format!("'{}'", s.replace('\'', "''")),
            // MySQL treats backslash as an escape character inside string literals by default.
            Dialect::Mysql => format!(
                "'{}'",
                s.replace('\\', "\\\\")
                    .replace('\'', "\\'")
                    .replace('\0', "\\0")
            ),
        }
    }

    /// Bind-parameter placeholder for the n-th (1-based) parameter.
    pub fn placeholder(self, n: usize) -> String {
        match self {
            Dialect::Postgres => format!("${n}"),
            Dialect::Mysql => "?".to_string(),
            Dialect::Sqlite => format!("?{n}"),
        }
    }

    /// `"schema"."name"` / `` `db`.`name` `` / `"name"` when no schema is given.
    pub fn qualify(self, schema: Option<&str>, name: &str) -> String {
        match schema {
            Some(s) if !s.is_empty() => {
                format!("{}.{}", self.quote_ident(s), self.quote_ident(name))
            }
            _ => self.quote_ident(name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_identifiers_per_dialect() {
        assert_eq!(
            Dialect::Postgres.quote_ident("Order Items"),
            "\"Order Items\""
        );
        assert_eq!(Dialect::Sqlite.quote_ident("select"), "\"select\"");
        assert_eq!(Dialect::Mysql.quote_ident("from"), "`from`");
    }

    #[test]
    fn escapes_embedded_quote_characters() {
        assert_eq!(Dialect::Postgres.quote_ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(Dialect::Mysql.quote_ident("a`b"), "`a``b`");
        // An injection attempt stays inside the quotes.
        assert_eq!(
            Dialect::Postgres.quote_ident("x\"; DROP TABLE t; --"),
            "\"x\"\"; DROP TABLE t; --\""
        );
    }

    #[test]
    fn quotes_literals_for_preview() {
        assert_eq!(Dialect::Postgres.quote_literal("it's"), "'it''s'");
        assert_eq!(Dialect::Sqlite.quote_literal("a\\b"), "'a\\b'");
        assert_eq!(Dialect::Mysql.quote_literal("it's a\\b"), "'it\\'s a\\\\b'");
    }

    #[test]
    fn placeholders_and_qualification() {
        assert_eq!(Dialect::Postgres.placeholder(3), "$3");
        assert_eq!(Dialect::Mysql.placeholder(3), "?");
        assert_eq!(Dialect::Sqlite.placeholder(3), "?3");
        assert_eq!(
            Dialect::Postgres.qualify(Some("public"), "users"),
            "\"public\".\"users\""
        );
        assert_eq!(
            Dialect::Mysql.qualify(Some("app"), "users"),
            "`app`.`users`"
        );
        assert_eq!(Dialect::Sqlite.qualify(None, "users"), "\"users\"");
        assert_eq!(Dialect::Sqlite.qualify(Some(""), "users"), "\"users\"");
    }
}
