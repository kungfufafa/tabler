//! CQL (the cassandra lane). A conservative first spec from the research notes; the lane refines
//! it. Built with `drv-cassandra` only.
//!
//! Double-quoted names, single-quoted strings, `$$` bodies, `//` comments, `?` binds. CQL filters
//! and sorts very little: there is no casting, no NULL ordering, no case-insensitive match and no
//! offset (later pages need the driver's paging state). Writes are upserts with no row count, so
//! the lane must prove them (lightweight transactions) before EditRows is on.

use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, PLAIN};
use crate::sqlgen::dialect::spec::{
    CiLike, DialectSpec, NullSafeEq, NullsSort, Paging, PlaceholderStyle, RedactRule, SqlHooks,
    SqlKnobs, double_quoted, explain_unsupported, hex_0x, ph_question, single_quoted,
};
use crate::sqlgen::portable::classify;
use crate::types::mutate::ServerExpr;
use crate::types::schema::{ColumnMeta, ValueKind};

pub static LEX: LexProfile = LexProfile {
    ident_quotes: &[('"', '"')],
    string_quotes: &['\''],
    readings: &[PLAIN],
    e_strings: false,
    dollar_quotes: true,
    // CQL has only `$$` bodies, and `$a$` is no token at all.
    dollar_tags: false,
    triple_quotes: false,
    raw_prefixes: false,
    byte_prefixes: false,
    hash_comments: false,
    slash_slash_comments: true,
    nested_block_comments: false,
    dash_comment_needs_space: false,
    batch_separator: None,
    terminator_optional: false,
    begin_end_bodies: false,
};

pub static RULES: ClassifyRules = ClassifyRules {
    queries: &["SELECT"],
    inspections: &["DESCRIBE", "DESC", "USE", "LIST"],
    writes: &[
        "INTO", "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "ALTER", "TRUNCATE", "BATCH",
        "APPLY", "BEGIN", "GRANT", "REVOKE",
    ],
    write_calls: &[],
    table_functions: None,
};

fn classify_cql(sql: &str) -> StatementClass {
    classify::with(&LEX, &RULES, sql)
}

fn no_text(_: &str, _: &ColumnMeta) -> Option<String> {
    None
}

fn no_server_expr(_: ServerExpr, _: ValueKind) -> Option<String> {
    None
}

pub static SPEC: DialectSpec = DialectSpec {
    dialect: SqlDialect::Cql,
    label: "CQL",
    quote_ident: double_quoted,
    quote_literal: single_quoted,
    quote_literal_exec: single_quoted,
    placeholder: ph_question,
    placeholder_style: PlaceholderStyle::Question,
    placeholders_reusable: false,
    max_params: 1000,
    max_name_parts: 2,
    lex: &LEX,
    rules: &RULES,
    classify: classify_cql,
    knobs: SqlKnobs {
        bool_literal: ["false", "true"],
        bytes_literal: hex_0x,
        text_expr: no_text,
        paging: Paging::LimitOnly,
        nulls_order: NullsSort::Unsupported,
        ci_like: CiLike::Unsupported,
        like_escape: None,
        null_safe_eq: NullSafeEq::OrForm,
        insert_all_defaults: None,
        set_default: false,
        server_expr: no_server_expr,
        explain: explain_unsupported,
        catalog_words: &["SYSTEM_SCHEMA", "SYSTEM."],
        redact: &[RedactRule {
            keyword: "PASSWORD",
        }],
        cancel_notice: "what it changed before it stopped stays changed (Cassandra has no \
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
    fn export_bindings_lex_cql() {
        crate::sqlgen::dialect::lex::export_lex("cql", &LEX, &RULES);
    }

    #[test]
    fn quotes_and_classifies() {
        assert_eq!((SPEC.quote_ident)("MixedCase\""), "\"MixedCase\"\"\"");
        assert_eq!(
            classify_cql("SELECT * FROM ks.t WHERE k = 1 ALLOW FILTERING"),
            StatementClass::Read
        );
        assert_eq!(
            classify_cql("BEGIN BATCH INSERT INTO t (k) VALUES (1); APPLY BATCH"),
            StatementClass::Write
        );
    }
}
