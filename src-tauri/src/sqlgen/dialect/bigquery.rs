//! GoogleSQL (the bigquery lane). A conservative first spec from the research notes; the lane
//! refines it. Built with `drv-bigquery` only.
//!
//! Backticked names and `'…'` / `"…"` strings, all with backslash escapes; triple-quoted, raw and
//! byte strings; `#` comments; scripts with `BEGIN … END` blocks; named `@p1` parameters. There is
//! no EXPLAIN (a dry run estimates instead) and no RETURNING. `EXECUTE IMMEDIATE` and `CALL` run
//! other statements, and `EXTERNAL_QUERY` runs one on another database.

use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, reading};
use crate::sqlgen::dialect::spec::{
    DialectSpec, PlaceholderStyle, SqlHooks, SqlKnobs, backticked_backslash, explain_unsupported,
    ph_at_name, single_quoted_backslash,
};
use crate::sqlgen::portable::classify;
use crate::types::mutate::ServerExpr;
use crate::types::schema::{ColumnMeta, ValueKind};

pub static LEX: LexProfile = LexProfile {
    ident_quotes: &[('`', '`')],
    string_quotes: &['\'', '"'],
    readings: &[reading(true, true)],
    e_strings: false,
    dollar_quotes: false,
    dollar_tags: false,
    triple_quotes: true,
    raw_prefixes: true,
    byte_prefixes: true,
    hash_comments: true,
    slash_slash_comments: false,
    nested_block_comments: false,
    dash_comment_needs_space: false,
    batch_separator: None,
    terminator_optional: false,
    begin_end_bodies: true,
};

pub static RULES: ClassifyRules = ClassifyRules {
    queries: &["SELECT", "WITH"],
    inspections: &[],
    writes: &[
        "INTO", "INSERT", "UPDATE", "DELETE", "MERGE", "CREATE", "DROP", "ALTER", "TRUNCATE",
        "CALL", "EXECUTE", "EXPORT", "LOAD", "GRANT", "REVOKE", "DECLARE", "SET", "BEGIN",
    ],
    write_calls: &["EXTERNAL_QUERY"],
    table_functions: None,
};

fn classify_bigquery(sql: &str) -> StatementClass {
    classify::with(&LEX, &RULES, sql)
}

fn from_hex(hex: &str) -> String {
    format!("FROM_HEX('{hex}')")
}

fn cast_string(e: &str, _: &ColumnMeta) -> Option<String> {
    Some(format!("CAST({e} AS STRING)"))
}

fn server_expr(expr: ServerExpr, _: ValueKind) -> Option<String> {
    match expr {
        ServerExpr::Now | ServerExpr::CurrentTimestamp => Some("CURRENT_TIMESTAMP()".into()),
        ServerExpr::CurrentDate => Some("CURRENT_DATE()".into()),
        _ => None,
    }
}

pub static SPEC: DialectSpec = DialectSpec {
    dialect: SqlDialect::Bigquery,
    label: "GoogleSQL",
    quote_ident: backticked_backslash,
    quote_literal: single_quoted_backslash,
    quote_literal_exec: single_quoted_backslash,
    placeholder: ph_at_name,
    placeholder_style: PlaceholderStyle::AtName,
    placeholders_reusable: true,
    max_params: 10_000,
    max_name_parts: 3,
    lex: &LEX,
    rules: &RULES,
    classify: classify_bigquery,
    knobs: SqlKnobs {
        bytes_literal: from_hex,
        text_expr: cast_string,
        like_escape: None,
        insert_all_defaults: None,
        set_default: false,
        server_expr,
        explain: explain_unsupported,
        catalog_words: &["INFORMATION_SCHEMA"],
        ..SqlKnobs::ANSI
    },
    hooks: SqlHooks::NONE,
    ddl: None,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_bindings_lex_bigquery() {
        crate::sqlgen::dialect::lex::export_lex("bigquery", &LEX, &RULES);
    }

    #[test]
    fn quotes_and_classifies() {
        assert_eq!((SPEC.quote_ident)("my-proj.ds`"), "`my-proj.ds\\``");
        assert_eq!((SPEC.quote_literal)("a\nb'"), "'a\\nb\\''");
        assert_eq!(
            classify_bigquery("SELECT '''x; DELETE''' # y\n"),
            StatementClass::Read
        );
        for sql in [
            "EXECUTE IMMEDIATE 'DELETE FROM t'",
            "CALL p()",
            "BEGIN SELECT 1; END",
        ] {
            assert_eq!(classify_bigquery(sql), StatementClass::Write, "{sql}");
        }
    }
}
