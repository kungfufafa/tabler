//! Snowflake SQL (the snowflake lane). A conservative first spec from the research notes; the
//! lane refines it. Built with `drv-snowflake` only.
//!
//! Double-quoted names; single-quoted strings with backslash escapes; `$$…$$` bodies (untagged:
//! `$name` is a session variable), `//` comments and Snowflake Scripting `BEGIN … END` blocks; `?`
//! binds. Every DDL statement commits an open transaction. `SYSTEM$…` functions act on the
//! account, so calling one is a write.

use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, reading};
use crate::sqlgen::dialect::spec::{
    CiLike, DialectSpec, ExplainForm, PlaceholderStyle, RedactRule, SqlHooks, SqlKnobs,
    double_quoted, ph_question, single_quoted_backslash,
};
use crate::sqlgen::portable::classify;
use crate::sqlgen::split::leading_keywords;
use crate::types::query::ExplainMode;
use crate::types::schema::ColumnMeta;

pub static LEX: LexProfile = LexProfile {
    ident_quotes: &[('"', '"')],
    string_quotes: &['\''],
    readings: &[reading(true, false)],
    e_strings: false,
    dollar_quotes: true,
    // Only `$$` opens a body: `$name` is a session variable, so `$a$` is the variable `a$`, and a
    // scanner that took it for a tag would read code between two of them as a string.
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
    begin_end_bodies: true,
};

pub static RULES: ClassifyRules = ClassifyRules {
    queries: &["SELECT", "WITH"],
    inspections: &["SHOW", "DESCRIBE", "DESC", "EXPLAIN", "LIST", "LS", "USE"],
    writes: &[
        "INTO", "INSERT", "UPDATE", "DELETE", "MERGE", "CREATE", "DROP", "ALTER", "TRUNCATE",
        "UNDROP", "CALL", "EXECUTE",
    ],
    write_calls: &["SYSTEM$*"],
    table_functions: None,
};

fn classify_snowflake(sql: &str) -> StatementClass {
    classify::with(&LEX, &RULES, sql)
}

fn to_varchar(e: &str, _: &ColumnMeta) -> Option<String> {
    Some(format!("TO_VARCHAR({e})"))
}

fn explain(mode: ExplainMode) -> ExplainForm {
    match mode {
        ExplainMode::Plan => ExplainForm::Prefix("EXPLAIN USING TEXT"),
        ExplainMode::Analyze => ExplainForm::Unsupported,
    }
}

/// DDL commits the open transaction.
fn implicit_commit(sql: &str) -> bool {
    matches!(
        leading_keywords(sql, 1).first().map(String::as_str),
        Some("CREATE" | "ALTER" | "DROP" | "UNDROP" | "TRUNCATE" | "GRANT" | "REVOKE" | "COMMENT")
    )
}

pub static SPEC: DialectSpec = DialectSpec {
    dialect: SqlDialect::Snowflake,
    label: "Snowflake SQL",
    quote_ident: double_quoted,
    quote_literal: single_quoted_backslash,
    quote_literal_exec: single_quoted_backslash,
    placeholder: ph_question,
    placeholder_style: PlaceholderStyle::Question,
    placeholders_reusable: false,
    max_params: 1000,
    max_name_parts: 3,
    lex: &LEX,
    rules: &RULES,
    classify: classify_snowflake,
    knobs: SqlKnobs {
        text_expr: to_varchar,
        ci_like: CiLike::Ilike,
        insert_all_defaults: None,
        set_default: false,
        explain,
        implicit_commit,
        catalog_words: &["INFORMATION_SCHEMA", "SNOWFLAKE.ACCOUNT_USAGE"],
        side_effect_functions: &["SYSTEM$*"],
        redact: &[
            RedactRule {
                keyword: "PASSWORD",
            },
            RedactRule {
                keyword: "AWS_SECRET_KEY",
            },
            RedactRule {
                keyword: "AWS_TOKEN",
            },
            RedactRule {
                keyword: "AZURE_SAS_TOKEN",
            },
            RedactRule {
                keyword: "MASTER_KEY",
            },
        ],
        ..SqlKnobs::ANSI
    },
    hooks: SqlHooks::NONE,
    ddl: None,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_bindings_lex_snowflake() {
        crate::sqlgen::dialect::lex::export_lex("snowflake", &LEX, &RULES);
    }

    #[test]
    fn quotes_and_classifies() {
        assert_eq!((SPEC.quote_ident)("a\"b"), "\"a\"\"b\"");
        assert_eq!((SPEC.quote_literal)("a\\'"), "'a\\\\\\''");
        assert_eq!(
            classify_snowflake("SELECT SYSTEM$CANCEL_ALL_QUERIES(1)"),
            StatementClass::Write
        );
        assert_eq!(
            classify_snowflake("EXECUTE IMMEDIATE 'x'"),
            StatementClass::Write
        );
        assert_eq!(
            classify_snowflake("SELECT 1 // DELETE"),
            StatementClass::Read
        );
        // `$a$` is a session variable, not a tag; `$$` still opens a body.
        assert_eq!(
            classify_snowflake("SELECT $a$, SYSTEM$CANCEL_ALL_QUERIES(1), $a$"),
            StatementClass::Write
        );
        assert_eq!(
            classify_snowflake("SELECT $$; DELETE FROM t; $$"),
            StatementClass::Read
        );
        assert!(implicit_commit("create table t (a int)"));
    }
}
