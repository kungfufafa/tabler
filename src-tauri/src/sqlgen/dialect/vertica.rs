//! Vertica SQL (the vertica lane). A conservative first spec from the research notes; the lane
//! refines it. Built with `drv-vertica` only.
//!
//! PostgreSQL-like text read both ways of backslashes (`standard_conforming_strings`), `E'…'` and
//! `$$…$$` strings, `?` parameters, no RETURNING and no row id, so only tables with a primary key
//! are editable. DDL commits on its own. Meta-functions called from a SELECT can change the
//! cluster.

use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, PLAIN, reading};
use crate::sqlgen::dialect::spec::{
    CiLike, DialectSpec, NullSafeEq, PlaceholderStyle, RedactRule, SqlHooks, SqlKnobs,
    double_quoted, ph_question, single_quoted,
};
use crate::sqlgen::portable::classify;
use crate::sqlgen::split::leading_keywords;
use crate::types::schema::ColumnMeta;

pub static LEX: LexProfile = LexProfile {
    ident_quotes: &[('"', '"')],
    string_quotes: &['\''],
    readings: &[PLAIN, reading(true, false)],
    e_strings: true,
    // `$$…$$` string literals. The reference shows no tags, but `$a$` is no other token, so
    // reading tags too only matters where the server takes them.
    dollar_quotes: true,
    dollar_tags: true,
    triple_quotes: false,
    raw_prefixes: false,
    byte_prefixes: false,
    hash_comments: false,
    slash_slash_comments: false,
    nested_block_comments: false,
    dash_comment_needs_space: false,
    batch_separator: None,
    terminator_optional: false,
    begin_end_bodies: false,
};

pub static RULES: ClassifyRules = ClassifyRules {
    queries: &["SELECT", "WITH"],
    inspections: &["SHOW", "EXPLAIN"],
    writes: &[
        "INTO", "INSERT", "UPDATE", "DELETE", "MERGE", "CREATE", "DROP", "ALTER", "TRUNCATE",
        "COPY", "EXPORT", "GRANT", "REVOKE",
    ],
    // The meta-functions that change the cluster, its data or its files. A deny-list (an
    // allow-list would have to name every function a query may call), so whole families go by
    // prefix; the lane checks it against the server's function reference.
    write_calls: &[
        "CLOSE_*",
        "INTERRUPT_STATEMENT",
        "CANCEL_*",
        "ANALYZE_STATISTICS*",
        "ANALYZE_EXTERNAL_ROW_COUNT",
        "PURGE*",
        "DROP_*",
        "MAKE_AHM_NOW",
        "ADVANCE_EPOCH",
        "SET_*",
        "CLEAR_*",
        "RESET_*",
        "DO_TM_TASK",
        "EXPORT_*",
        "IMPORT_*",
        "START_*",
        "STOP_*",
        "SHUTDOWN*",
        "COPY_*",
        "MOVE_*",
        "SWAP_*",
        "PARTITION_TABLE",
        "PARTITION_PROJECTION",
        "REBALANCE_*",
        "RESHARD_*",
        "REFRESH*",
        "ENABLE_*",
        "DISABLE_*",
        "REENABLE_*",
        "DESIGNER_*",
        "MARK_DESIGN_KSAFE",
        "RETIRE_*",
        "RESTORE_*",
        "ALTER_*",
        "ENFORCE_*",
        "MEASURE_LOCATION_PERFORMANCE",
        "FLUSH_*",
        "INSTALL_*",
        "PROMOTE_*",
        "DEMOTE_*",
        "SANDBOX_*",
        "UNSANDBOX_*",
        "MIGRATE_*",
        "SYNC_CATALOG",
        "CLEAN_*",
        "COMPACT_STORAGE",
        "FINISH_FETCHING_FILES",
        "BACKGROUND_DEPOT_WARMING",
        "RELOAD_*",
        "REALIGN_*",
        "RELEASE_*",
        "RESTRICT_*",
        "RESERVE_*",
        "CHANGE_*",
        "SAVE_*",
        "HURRY_SERVICE",
        "NOTIFY",
        "LDAP_LINK_*",
    ],
    table_functions: None,
};

fn classify_vertica(sql: &str) -> StatementClass {
    classify::with(&LEX, &RULES, sql)
}

/// Quotes doubled. A backslash is itself under `StandardConformingStrings` (on by default) and
/// an escape without it, so a value holding one goes in an `E'…'` string, which reads escapes
/// either way.
fn quote_exec(s: &str) -> String {
    if s.contains('\\') {
        format!("E'{}'", s.replace('\\', "\\\\").replace('\'', "''"))
    } else {
        single_quoted(s)
    }
}

fn hex_to_binary(hex: &str) -> String {
    format!("HEX_TO_BINARY('{hex}')")
}

fn cast_varchar(e: &str, _: &ColumnMeta) -> Option<String> {
    Some(format!("CAST({e} AS VARCHAR(65000))"))
}

/// DDL commits on its own.
fn implicit_commit(sql: &str) -> bool {
    matches!(
        leading_keywords(sql, 1).first().map(String::as_str),
        Some("CREATE" | "ALTER" | "DROP" | "TRUNCATE" | "GRANT" | "REVOKE" | "COMMENT")
    )
}

pub static SPEC: DialectSpec = DialectSpec {
    dialect: SqlDialect::Vertica,
    label: "Vertica SQL",
    quote_ident: double_quoted,
    quote_literal: single_quoted,
    quote_literal_exec: quote_exec,
    placeholder: ph_question,
    placeholder_style: PlaceholderStyle::Question,
    placeholders_reusable: false,
    max_params: 1000,
    max_name_parts: 3,
    lex: &LEX,
    rules: &RULES,
    classify: classify_vertica,
    knobs: SqlKnobs {
        bytes_literal: hex_to_binary,
        text_expr: cast_varchar,
        ci_like: CiLike::Ilike,
        null_safe_eq: NullSafeEq::OrForm,
        set_default: false,
        implicit_commit,
        catalog_words: &["V_CATALOG", "V_MONITOR", "V_INTERNAL"],
        redact: &[
            RedactRule {
                keyword: "PASSWORD",
            },
            RedactRule {
                keyword: "IDENTIFIED",
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
    fn export_bindings_lex_vertica() {
        crate::sqlgen::dialect::lex::export_lex("vertica", &LEX, &RULES);
    }

    #[test]
    fn quotes_and_classifies() {
        assert_eq!(quote_exec("a\\'b"), "E'a\\\\''b'");
        assert_eq!(quote_exec("it's"), "'it''s'");
        for sql in [
            "SELECT CLOSE_ALL_SESSIONS()",
            "SELECT SHUTDOWN()",
            "SELECT SHUTDOWN_SUBCLUSTER('sc')",
            "SELECT COPY_TABLE('a','b')",
            "SELECT MOVE_PARTITIONS_TO_TABLE('a', '1', '2', 'b')",
            "SELECT SWAP_PARTITIONS_BETWEEN_TABLES('a', '1', '2', 'b')",
            "SELECT PARTITION_TABLE('t')",
            "SELECT REBALANCE_CLUSTER()",
            "SELECT REFRESH('t')",
            "SELECT ENABLE_PROFILING('ee')",
            "SELECT DESIGNER_RUN_POPULATE_DESIGN_AND_DEPLOY('d', 'q', 'o', 'p')",
            "SELECT RETIRE_LOCATION('/data', 'node')",
            "SELECT ALTER_LOCATION_USE('/data', '', 'TEMP')",
            "SELECT FLUSH_DATA_COLLECTOR()",
            "SELECT INSTALL_LICENSE('/x.key')",
            "SELECT PROMOTE_SUBCLUSTER_TO_PRIMARY('sc')",
            "SELECT RESHARD_DATABASE(24)",
        ] {
            assert_eq!(classify_vertica(sql), StatementClass::Write, "{sql}");
        }
        assert_eq!(
            classify_vertica("SELECT 'a\\'; DELETE FROM t --'"),
            StatementClass::Write
        );
        // Quotes inside a dollar-quoted string open nothing.
        assert_eq!(
            classify_vertica("SELECT $$'$$; DELETE FROM t; SELECT $$'$$"),
            StatementClass::Write
        );
        assert_eq!(classify_vertica("SELECT * FROM t"), StatementClass::Read);
        assert_eq!(
            classify_vertica("SELECT REGEXP_REPLACE(a, 'x', 'y'), COALESCE(b, 0) FROM t"),
            StatementClass::Read
        );
    }
}
