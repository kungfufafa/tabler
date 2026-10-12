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

    /// Quotes a string literal for SQL that runs without bind parameters (DDL, account
    /// statements), so that the server reads back exactly `s`. Single quotes are always doubled
    /// (`''` works in every mode). PostgreSQL text with a backslash uses the escape-string form
    /// `E'…'`, which means the same whatever `standard_conforming_strings` is. MySQL doubles
    /// backslashes (and spells NUL `\0`) only when the session reads them as escapes, i.e. its
    /// `sql_mode` lacks `NO_BACKSLASH_ESCAPES` (`mode`); the mode is ignored by other dialects.
    pub fn quote_literal_with(self, s: &str, mode: MysqlLiteralMode) -> String {
        let doubled = s.replace('\'', "''");
        match self {
            Dialect::Postgres if s.contains('\\') => {
                format!("E'{}'", doubled.replace('\\', "\\\\"))
            }
            Dialect::Postgres | Dialect::Sqlite => format!("'{doubled}'"),
            Dialect::Mysql => match mode {
                MysqlLiteralMode::BackslashEscapes => {
                    format!("'{}'", doubled.replace('\\', "\\\\").replace('\0', "\\0"))
                }
                MysqlLiteralMode::NoBackslashEscapes => format!("'{doubled}'"),
            },
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

/// How a MySQL session reads backslashes inside string literals: as escape characters (the
/// default) or as plain characters (`sql_mode` contains `NO_BACKSLASH_ESCAPES`, set by the
/// server's defaults or by the connection's startup SQL).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MysqlLiteralMode {
    #[default]
    BackslashEscapes,
    NoBackslashEscapes,
}

impl MysqlLiteralMode {
    /// Reads the session's mode back (one row, one column: the `sql_mode` text).
    pub const QUERY: &'static str = "SELECT @@SESSION.sql_mode";

    /// The mode of an `@@SESSION.sql_mode` value (`STRICT_TRANS_TABLES,NO_BACKSLASH_ESCAPES`).
    pub fn from_sql_mode(sql_mode: &str) -> MysqlLiteralMode {
        if sql_mode
            .split(',')
            .any(|m| m.trim().eq_ignore_ascii_case("NO_BACKSLASH_ESCAPES"))
        {
            MysqlLiteralMode::NoBackslashEscapes
        } else {
            MysqlLiteralMode::BackslashEscapes
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
    fn quotes_executed_literals_per_session_mode() {
        use MysqlLiteralMode::{BackslashEscapes as Esc, NoBackslashEscapes as NoEsc};
        // Quotes are doubled in both MySQL modes; backslashes only where they escape.
        assert_eq!(
            Dialect::Mysql.quote_literal_with(r"it's C:\temp", Esc),
            r"'it''s C:\\temp'"
        );
        assert_eq!(
            Dialect::Mysql.quote_literal_with(r"it's C:\temp", NoEsc),
            r"'it''s C:\temp'"
        );
        // A trailing backslash cannot swallow the closing quote in either mode.
        assert_eq!(Dialect::Mysql.quote_literal_with(r"a\", Esc), r"'a\\'");
        assert_eq!(Dialect::Mysql.quote_literal_with(r"a\'", NoEsc), r"'a\'''");
        assert_eq!(Dialect::Mysql.quote_literal_with("a\0b", Esc), r"'a\0b'");
        // PostgreSQL: the E'' form only when there is a backslash; SQLite never escapes.
        assert_eq!(Dialect::Postgres.quote_literal_with("it's", Esc), "'it''s'");
        assert_eq!(
            Dialect::Postgres.quote_literal_with(r"C:\it's", NoEsc),
            r"E'C:\\it''s'"
        );
        assert_eq!(Dialect::Sqlite.quote_literal_with(r"a\'b", Esc), r"'a\''b'");
    }

    #[test]
    fn reads_the_mysql_literal_mode_from_sql_mode() {
        assert_eq!(
            MysqlLiteralMode::from_sql_mode("STRICT_TRANS_TABLES,NO_ENGINE_SUBSTITUTION"),
            MysqlLiteralMode::BackslashEscapes
        );
        assert_eq!(
            MysqlLiteralMode::from_sql_mode("ONLY_FULL_GROUP_BY, no_backslash_escapes"),
            MysqlLiteralMode::NoBackslashEscapes
        );
        assert_eq!(
            MysqlLiteralMode::from_sql_mode(""),
            MysqlLiteralMode::BackslashEscapes
        );
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
