//! EXPLAIN wrapping per dialect. Every form produces a textual plan the editor can show as-is.

use super::Dialect;
use crate::types::query::ExplainMode;

/// Wraps one statement in the dialect's EXPLAIN. A trailing `;` (and whitespace) is dropped.
/// `Analyze` executes the statement on PostgreSQL / MySQL / MariaDB.
pub fn wrap_explain(dialect: Dialect, sql: &str, mode: ExplainMode, mariadb: bool) -> String {
    format!(
        "{} {}",
        explain_prefix(dialect, mode, mariadb),
        strip_terminator(sql)
    )
}

/// The text `wrap_explain` puts in front of the statement (without the separating space).
pub fn explain_prefix(dialect: Dialect, mode: ExplainMode, mariadb: bool) -> &'static str {
    match (dialect, mode) {
        (Dialect::Postgres, ExplainMode::Plan) => "EXPLAIN (VERBOSE, COSTS, FORMAT TEXT)",
        (Dialect::Postgres, ExplainMode::Analyze) => {
            "EXPLAIN (ANALYZE, BUFFERS, VERBOSE, FORMAT TEXT)"
        }
        (Dialect::Mysql, ExplainMode::Plan) if mariadb => "EXPLAIN FORMAT=JSON",
        (Dialect::Mysql, ExplainMode::Analyze) if mariadb => "ANALYZE FORMAT=JSON",
        (Dialect::Mysql, ExplainMode::Plan) => "EXPLAIN FORMAT=TREE",
        (Dialect::Mysql, ExplainMode::Analyze) => "EXPLAIN ANALYZE",
        // SQLite has no executing EXPLAIN that reports a plan; both modes show the query plan.
        (Dialect::Sqlite, _) => "EXPLAIN QUERY PLAN",
    }
}

/// Number of characters `wrap_explain` adds before the (unchanged) statement text, so error
/// positions reported for the wrapped SQL can be mapped back onto the statement.
pub fn explain_offset(dialect: Dialect, mode: ExplainMode, mariadb: bool) -> usize {
    explain_prefix(dialect, mode, mariadb).chars().count() + 1
}

fn strip_terminator(sql: &str) -> &str {
    let mut s = sql.trim_end();
    while let Some(rest) = s.strip_suffix(';') {
        s = rest.trim_end();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_per_dialect() {
        let sql = "select * from users where id = 1;";
        assert_eq!(
            wrap_explain(Dialect::Postgres, sql, ExplainMode::Plan, false),
            "EXPLAIN (VERBOSE, COSTS, FORMAT TEXT) select * from users where id = 1"
        );
        assert_eq!(
            wrap_explain(Dialect::Postgres, sql, ExplainMode::Analyze, false),
            "EXPLAIN (ANALYZE, BUFFERS, VERBOSE, FORMAT TEXT) select * from users where id = 1"
        );
        assert_eq!(
            wrap_explain(Dialect::Mysql, sql, ExplainMode::Plan, false),
            "EXPLAIN FORMAT=TREE select * from users where id = 1"
        );
        assert_eq!(
            wrap_explain(Dialect::Mysql, sql, ExplainMode::Analyze, false),
            "EXPLAIN ANALYZE select * from users where id = 1"
        );
        assert_eq!(
            wrap_explain(Dialect::Mysql, sql, ExplainMode::Plan, true),
            "EXPLAIN FORMAT=JSON select * from users where id = 1"
        );
        assert_eq!(
            wrap_explain(Dialect::Mysql, sql, ExplainMode::Analyze, true),
            "ANALYZE FORMAT=JSON select * from users where id = 1"
        );
        assert_eq!(
            wrap_explain(Dialect::Sqlite, sql, ExplainMode::Plan, false),
            "EXPLAIN QUERY PLAN select * from users where id = 1"
        );
        assert_eq!(
            wrap_explain(Dialect::Sqlite, sql, ExplainMode::Analyze, false),
            "EXPLAIN QUERY PLAN select * from users where id = 1"
        );
    }

    #[test]
    fn strips_terminators_and_whitespace() {
        assert_eq!(
            wrap_explain(Dialect::Sqlite, "select 1 ;  ;\n", ExplainMode::Plan, false),
            "EXPLAIN QUERY PLAN select 1"
        );
        assert_eq!(
            wrap_explain(Dialect::Postgres, "select ';'", ExplainMode::Plan, false),
            "EXPLAIN (VERBOSE, COSTS, FORMAT TEXT) select ';'"
        );
    }

    #[test]
    fn offset_points_at_the_statement() {
        for (d, m, maria) in [
            (Dialect::Postgres, ExplainMode::Plan, false),
            (Dialect::Postgres, ExplainMode::Analyze, false),
            (Dialect::Mysql, ExplainMode::Plan, false),
            (Dialect::Mysql, ExplainMode::Analyze, true),
            (Dialect::Sqlite, ExplainMode::Plan, false),
        ] {
            let wrapped = wrap_explain(d, "select x", m, maria);
            let off = explain_offset(d, m, maria);
            assert_eq!(wrapped.chars().skip(off).collect::<String>(), "select x");
        }
    }
}
