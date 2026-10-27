//! ClickHouse SQL (the clickhouse lane). A conservative first spec from the research notes; the
//! lane refines it. Built with `drv-clickhouse` only.
//!
//! Backticks or double quotes and single-quoted strings, all with backslash escapes; typed
//! `{p1:Type}` parameters; `#` comments and `$tag$` heredocs. There are no transactions and no
//! RETURNING. Only the table functions that read nothing outside the server are reads: `url()`,
//! `s3()`, `remote()`, `file()` and the rest reach other servers or the server's files.
//!
//! The classifier knows only those plain spellings (a table function where a table goes, a
//! `file()` call, `INTO OUTFILE`). Whatever else a SELECT may reach (dictionaries, engines of
//! existing tables) is the server's to refuse, so the lane must send `readonly=2` with every
//! request of a read-only or unattended (`registry::unattended`, MCP) session, and report
//! `ReadOnlyMode::PerRequest` with `safety.hardened` only when it does.

use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, reading};
use crate::sqlgen::dialect::spec::{
    CiLike, DialectSpec, NullSafeEq, PlaceholderStyle, RedactRule, SqlHooks, SqlKnobs,
    backticked_backslash, explain_plan_only, single_quoted_backslash,
};
use crate::sqlgen::portable::classify;
use crate::types::mutate::{Clock, ServerExpr};
use crate::types::schema::{ColumnMeta, ValueKind};

pub static LEX: LexProfile = LexProfile {
    ident_quotes: &[('`', '`'), ('"', '"')],
    string_quotes: &['\''],
    readings: &[reading(true, true)],
    e_strings: false,
    dollar_quotes: true,
    dollar_tags: true,
    triple_quotes: false,
    raw_prefixes: false,
    byte_prefixes: false,
    hash_comments: true,
    slash_slash_comments: false,
    // ClickHouse's Lexer nests them.
    nested_block_comments: true,
    dash_comment_needs_space: false,
    batch_separator: None,
    terminator_optional: false,
    begin_end_bodies: false,
};

pub static RULES: ClassifyRules = ClassifyRules {
    queries: &["SELECT", "WITH"],
    inspections: &["SHOW", "DESCRIBE", "DESC", "EXPLAIN", "EXISTS", "USE"],
    // Statements end at `;`, so these are only looked for inside queries, where the statement
    // keywords (SYSTEM, ALTER, KILL…) can only be names (`FROM system.tables`). INTO is
    // `INTO OUTFILE`.
    writes: &["INTO"],
    // `file()` is a scalar function too, which reads a file of the server outside any table
    // position.
    write_calls: &["FILE"],
    table_functions: Some(&[
        "NUMBERS",
        "NUMBERS_MT",
        "ZEROS",
        "ZEROS_MT",
        "VALUES",
        "GENERATERANDOM",
        "VIEW",
        "MERGE",
        "NULL",
    ]),
};

fn classify_clickhouse(sql: &str) -> StatementClass {
    classify::with(&LEX, &RULES, sql)
}

/// `{pN:Type}`: the column's own type, or String for a value without a column.
fn placeholder(n: usize, col: Option<&ColumnMeta>) -> String {
    format!(
        "{{p{n}:{}}}",
        col.map_or("String", |c| c.data_type.as_str())
    )
}

fn unhex(hex: &str) -> String {
    format!("unhex('{hex}')")
}

fn to_string(e: &str, _: &ColumnMeta) -> Option<String> {
    Some(format!("toString({e})"))
}

fn server_expr(expr: ServerExpr, _: ValueKind) -> Option<String> {
    match expr.clock() {
        Some((Clock::Now | Clock::CurrentTimestamp, None)) => Some("now()".into()),
        Some((Clock::Now | Clock::CurrentTimestamp, Some(n))) => Some(format!("now64({n})")),
        None if expr == ServerExpr::CurrentDate => Some("today()".into()),
        _ => None,
    }
}

pub static SPEC: DialectSpec = DialectSpec {
    dialect: SqlDialect::Clickhouse,
    label: "ClickHouse SQL",
    quote_ident: backticked_backslash,
    quote_literal: single_quoted_backslash,
    quote_literal_exec: single_quoted_backslash,
    placeholder,
    placeholder_style: PlaceholderStyle::Typed,
    placeholders_reusable: true,
    max_params: 1000,
    max_name_parts: 2,
    lex: &LEX,
    rules: &RULES,
    classify: classify_clickhouse,
    knobs: SqlKnobs {
        bool_literal: ["false", "true"],
        bytes_literal: unhex,
        text_expr: to_string,
        count_expr: "count()",
        ci_like: CiLike::Ilike,
        like_escape: None,
        null_safe_eq: NullSafeEq::OrForm,
        insert_all_defaults: None,
        set_default: false,
        server_expr,
        explain: explain_plan_only,
        catalog_words: &["SYSTEM.", "INFORMATION_SCHEMA"],
        side_effect_functions: &["SLEEP", "SLEEPEACHROW"],
        redact: &[
            RedactRule {
                keyword: "IDENTIFIED",
            },
            RedactRule {
                keyword: "PASSWORD",
            },
        ],
        cancel_notice: "what it changed before it stopped stays changed (ClickHouse has no \
                        transactions)",
        ..SqlKnobs::ANSI
    },
    hooks: SqlHooks::NONE,
    ddl: None,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_bindings_lex_clickhouse() {
        crate::sqlgen::dialect::lex::export_lex("clickhouse", &LEX, &RULES);
    }

    #[test]
    fn quotes_and_classifies() {
        assert_eq!((SPEC.quote_ident)("a`b\\"), "`a\\`b\\\\`");
        assert_eq!((SPEC.quote_literal)("it's \\"), "'it\\'s \\\\'");
        assert_eq!(
            classify_clickhouse("SELECT * FROM numbers(10)"),
            StatementClass::Read
        );
        for sql in [
            "SELECT * FROM system.tables",
            "SELECT name FROM system.columns WHERE database = 'db' AND table = 'kill'",
            "SHOW CREATE TABLE t",
            // Only bare setting names follow the commas of SETTINGS, and the clauses that end
            // a FROM list do so before their own lists.
            "SELECT * FROM t SETTINGS max_threads = 1, max_block_size = 2",
            "SELECT * FROM t WHERE a = 1 ORDER BY a, b",
            "SELECT count() FROM t GROUP BY a, lower(b)",
            "SELECT * FROM t LIMIT 1 BY a, lower(b)",
        ] {
            assert_eq!(classify_clickhouse(sql), StatementClass::Read, "{sql}");
        }
        for sql in [
            "SELECT * FROM t INTO OUTFILE 'x.csv'",
            "SYSTEM DROP DNS CACHE",
            "KILL QUERY WHERE query_id = 'x'",
            "OPTIMIZE TABLE t FINAL",
            "ALTER TABLE t DELETE WHERE 1",
            "SELECT * FROM url('http://x', CSV)",
            "SELECT * FROM t, urlCluster('c', 'http://x')",
            "DESCRIBE TABLE s3('https://x')",
            "SELECT * FROM t JOIN remote('h', db.t) USING (id)",
            // A subquery in the FROM list ends, and the list goes on.
            "SELECT * FROM (SELECT * FROM t) AS b, url('http://x')",
            "SELECT * FROM t JOIN (SELECT * FROM u) b ON true, url('http://x')",
            // Keywords are names after `AS` and `.` and as operands, so they leave the list
            // open.
            "SELECT * FROM numbers(1) AS settings, url('http://evil/x', CSV, 'a String')",
            "SELECT * FROM numbers(1) AS format, url('http://evil/x', CSV, 'a String')",
            "SELECT * FROM numbers(1) AS where, url('http://evil/x', CSV, 'a String')",
            "SELECT * FROM t fetch, url('http://x')",
            "SELECT * FROM t JOIN u ON t.a = u.order, url('http://x')",
            "SELECT * FROM t JOIN u ON t.a = limit, url('http://x')",
            "SELECT * FROM t JOIN u ON limit = t.a, url('http://x')",
            "SELECT * FROM limit, url('http://x')",
            "SELECT file('secret.txt')",
            "SELECT * FROM file('secret.txt', 'CSV')",
            // Comments a reader may end elsewhere: the Lexer nests `/* */` and ends `--` only
            // at an LF.
            "/* /* */ SELECT ' */ SELECT * FROM url('http://evil/x', CSV) --'",
            "/* /* */ SELECT 1 */ ALTER TABLE t DELETE WHERE 1",
            "SELECT 1 -- x\r'\nFROM url('http://evil') --'",
            "SELECT 1 # x\r'\nFROM url('http://evil') --'",
        ] {
            assert_eq!(classify_clickhouse(sql), StatementClass::Write, "{sql}");
        }
    }
}
