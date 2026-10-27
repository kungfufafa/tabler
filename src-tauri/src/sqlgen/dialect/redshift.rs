//! Redshift SQL (the pg-flavors lane). A conservative first spec from the research notes; the
//! lane refines it. Built with `drv-pg-flavors` only.
//!
//! Lexes like PostgreSQL (`$n` parameters, `E'…'` and `$$` strings), but Redshift has no
//! RETURNING, no ON CONFLICT, no IS NOT DISTINCT FROM and no savepoints, and `::text` is
//! VARCHAR(256) there: rows come back through `::varchar(65535)` instead, SUPER through
//! JSON_SERIALIZE (which may cut it, so SUPER cells are not editable), and the types that cannot
//! be cast as they are.

use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, PLAIN, reading};
use crate::sqlgen::dialect::spec::{
    CiLike, DialectSpec, NullSafeEq, PlaceholderStyle, RedactRule, SqlHooks, SqlKnobs,
    double_quoted, ph_dollar, single_quoted,
};
use crate::sqlgen::portable::classify;
use crate::sqlgen::split::leading_keywords;
use crate::types::mutate::ServerExpr;
use crate::types::schema::{ColumnMeta, ValueKind};

pub static LEX: LexProfile = LexProfile {
    ident_quotes: &[('"', '"')],
    string_quotes: &['\''],
    readings: &[PLAIN, reading(true, false)],
    e_strings: true,
    dollar_quotes: true,
    dollar_tags: true,
    triple_quotes: false,
    raw_prefixes: false,
    byte_prefixes: false,
    hash_comments: false,
    slash_slash_comments: false,
    // The PostgreSQL 8.0 lexer Redshift's comes from nests them.
    nested_block_comments: true,
    dash_comment_needs_space: false,
    batch_separator: None,
    terminator_optional: false,
    begin_end_bodies: false,
};

pub static RULES: ClassifyRules = ClassifyRules {
    queries: &["SELECT", "WITH", "VALUES"],
    inspections: &["SHOW", "EXPLAIN"],
    writes: &[
        "INTO", "INSERT", "UPDATE", "DELETE", "MERGE", "CREATE", "DROP", "ALTER", "TRUNCATE",
        "COPY", "UNLOAD", "CALL", "GRANT", "REVOKE", "VACUUM",
    ],
    write_calls: &["PG_TERMINATE_BACKEND", "PG_CANCEL_BACKEND", "SET_CONFIG"],
    table_functions: None,
};

fn classify_redshift(sql: &str) -> StatementClass {
    classify::with(&LEX, &RULES, sql)
}

/// Quotes doubled, and backslashes too: Redshift reads a backslash in a plain string as an escape
/// (its LIKE escape is written `'\\'`).
fn quote_exec(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "''"))
}

fn from_hex(hex: &str) -> String {
    format!("FROM_HEX('{hex}')")
}

fn base_type(col: &ColumnMeta) -> String {
    let ty = col.cast_type().to_ascii_lowercase();
    ty.split('(').next().unwrap_or_default().trim().to_string()
}

/// The text a row carries a column in; None for the types a cast to VARCHAR refuses.
fn text(e: &str, col: &ColumnMeta) -> Option<String> {
    match base_type(col).as_str() {
        "geometry" | "geography" | "hllsketch" | "varbyte" | "binary varying" => None,
        "super" => Some(format!("JSON_SERIALIZE({e})")),
        _ => Some(format!("{e}::varchar(65535)")),
    }
}

fn lossy(col: &ColumnMeta) -> bool {
    base_type(col) == "super"
}

fn server_expr(expr: ServerExpr, _: ValueKind) -> Option<String> {
    match expr {
        ServerExpr::Now | ServerExpr::CurrentTimestamp => Some("GETDATE()".into()),
        ServerExpr::CurrentDate => Some("CURRENT_DATE".into()),
        _ => None,
    }
}

/// TRUNCATE commits the open transaction; VACUUM cannot run inside one.
fn implicit_commit(sql: &str) -> bool {
    matches!(
        leading_keywords(sql, 1).first().map(String::as_str),
        Some("TRUNCATE" | "VACUUM")
    )
}

pub static SPEC: DialectSpec = DialectSpec {
    dialect: SqlDialect::Redshift,
    label: "Redshift SQL",
    quote_ident: double_quoted,
    quote_literal: single_quoted,
    quote_literal_exec: quote_exec,
    placeholder: ph_dollar,
    placeholder_style: PlaceholderStyle::Dollar,
    placeholders_reusable: true,
    max_params: 32_000,
    max_name_parts: 3,
    lex: &LEX,
    rules: &RULES,
    classify: classify_redshift,
    knobs: SqlKnobs {
        bytes_literal: from_hex,
        text_projection: Some(text),
        lossy_text: lossy,
        text_expr: text,
        ci_like: CiLike::Ilike,
        null_safe_eq: NullSafeEq::OrForm,
        set_default: false,
        server_expr,
        implicit_commit,
        catalog_words: &[
            "PG_CATALOG",
            "INFORMATION_SCHEMA",
            "STL_",
            "STV_",
            "SVV_",
            "SVL_",
        ],
        side_effect_functions: &["PG_TERMINATE_BACKEND", "PG_CANCEL_BACKEND"],
        redact: &[
            RedactRule {
                keyword: "PASSWORD",
            },
            RedactRule {
                keyword: "CREDENTIALS",
            },
            RedactRule {
                keyword: "SECRET_ACCESS_KEY",
            },
            RedactRule {
                keyword: "SESSION_TOKEN",
            },
            RedactRule {
                keyword: "MASTER_SYMMETRIC_KEY",
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
    use crate::sqlgen::portable::testing::col;

    #[test]
    fn export_bindings_lex_redshift() {
        crate::sqlgen::dialect::lex::export_lex("redshift", &LEX, &RULES);
    }

    #[test]
    fn projects_rows_without_text_casts() {
        let name = col("name", "character varying(300)", ValueKind::Text);
        let doc = col("doc", "super", ValueKind::Json);
        let shape = col("shape", "geometry", ValueKind::Geometry);
        assert_eq!(
            text("\"name\"", &name).as_deref(),
            Some("\"name\"::varchar(65535)")
        );
        assert_eq!(
            text("\"doc\"", &doc).as_deref(),
            Some("JSON_SERIALIZE(\"doc\")")
        );
        assert_eq!(text("\"shape\"", &shape), None);
        assert!(lossy(&doc) && !lossy(&name));
        assert_eq!(
            classify_redshift("SELECT pg_terminate_backend(1)"),
            StatementClass::Write
        );
    }
}
