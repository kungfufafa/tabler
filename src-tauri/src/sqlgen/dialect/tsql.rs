//! Transact-SQL (the sqlserver lane). A conservative first spec from the research notes; the lane
//! refines it. Built with `drv-sqlserver` only.
//!
//! `[name]` with `]` doubled, `N'…'` text, `@P1` parameters (sp_executesql, at most 2,100 per
//! request), OFFSET/FETCH paging and TOP, OUTPUT INSERTED for written rows, `0x…` bytes. Batches
//! end at `GO` lines and statements need no `;`, so the classifier treats a write keyword
//! anywhere as a write (`SELECT 1 DELETE FROM t`).

use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, PLAIN};
use crate::sqlgen::dialect::spec::{
    DialectSpec, LimitOne, NullSafeEq, NullsSort, Paging, PlaceholderStyle, RedactRule, Returning,
    SqlHooks, SqlKnobs, bracketed, explain_unsupported, hex_0x, national_quoted, ph_at_p,
};
use crate::sqlgen::portable::classify;
use crate::types::mutate::ServerExpr;
use crate::types::schema::{ColumnMeta, ValueKind};

pub static LEX: LexProfile = LexProfile {
    ident_quotes: &[('[', ']'), ('"', '"')],
    string_quotes: &['\''],
    readings: &[PLAIN],
    e_strings: false,
    dollar_quotes: false,
    dollar_tags: false,
    triple_quotes: false,
    raw_prefixes: false,
    byte_prefixes: false,
    hash_comments: false,
    slash_slash_comments: false,
    nested_block_comments: true,
    dash_comment_needs_space: false,
    batch_separator: Some("GO"),
    terminator_optional: true,
    begin_end_bodies: true,
};

pub static RULES: ClassifyRules = ClassifyRules {
    queries: &["SELECT", "WITH"],
    inspections: &["USE"],
    // Every statement keyword that is not a read: without terminators, any of them may start a
    // second statement. FETCH and END stay out, since reads use them too (OFFSET … FETCH NEXT,
    // CASE … END); CONVERSATION stands for END CONVERSATION, and the other statements END ends
    // open with BEGIN.
    writes: &[
        "INTO",
        "INSERT",
        "UPDATE",
        "DELETE",
        "MERGE",
        "CREATE",
        "DROP",
        "ALTER",
        "ADD",
        "TRUNCATE",
        "EXEC",
        "EXECUTE",
        "GRANT",
        "REVOKE",
        "DENY",
        "BACKUP",
        "RESTORE",
        "DBCC",
        "KILL",
        "SHUTDOWN",
        "BULK",
        "SET",
        "DECLARE",
        "BEGIN",
        "COMMIT",
        "ROLLBACK",
        "SAVE",
        "WAITFOR",
        "RECONFIGURE",
        "CHECKPOINT",
        "RAISERROR",
        "THROW",
        "PRINT",
        "OPEN",
        "CLOSE",
        "DEALLOCATE",
        "GOTO",
        "RETURN",
        "WHILE",
        "IF",
        "ENABLE",
        "DISABLE",
        "UPDATETEXT",
        "WRITETEXT",
        "READTEXT",
        "SETUSER",
        "REVERT",
        "BREAK",
        "CONTINUE",
        // Service Broker: RECEIVE dequeues (deletes) messages.
        "RECEIVE",
        "SEND",
        "MOVE",
        "GET",
        "CONVERSATION",
        // Azure Synapse RENAME OBJECT.
        "RENAME",
    ],
    // These run a statement hidden in a string, or reach other servers and the file system.
    write_calls: &["OPENQUERY", "OPENROWSET", "OPENDATASOURCE", "XP_*", "SP_*"],
    table_functions: None,
};

fn classify_tsql(sql: &str) -> StatementClass {
    classify::with(&LEX, &RULES, sql)
}

/// Text of a column. Binary values stay as they are (CONVERT would read their bytes as UTF-16);
/// spatial values cannot be converted, and their WKT drops the SRID.
fn text(e: &str, col: &ColumnMeta) -> Option<String> {
    match col.kind {
        ValueKind::Bytes => None,
        ValueKind::Geometry => Some(format!("{e}.ToString()")),
        _ => Some(format!("CONVERT(NVARCHAR(MAX), {e})")),
    }
}

fn lossy(col: &ColumnMeta) -> bool {
    col.kind == ValueKind::Geometry
}

fn server_expr(expr: ServerExpr, _: ValueKind) -> Option<String> {
    match expr {
        ServerExpr::Now | ServerExpr::CurrentTimestamp => Some("CURRENT_TIMESTAMP".into()),
        ServerExpr::CurrentDate => Some("CAST(GETDATE() AS date)".into()),
        _ => None,
    }
}

pub static SPEC: DialectSpec = DialectSpec {
    dialect: SqlDialect::Tsql,
    label: "Transact-SQL",
    quote_ident: bracketed,
    quote_literal: national_quoted,
    quote_literal_exec: national_quoted,
    placeholder: ph_at_p,
    placeholder_style: PlaceholderStyle::AtP,
    placeholders_reusable: true,
    max_params: 2098,
    max_name_parts: 3,
    lex: &LEX,
    rules: &RULES,
    classify: classify_tsql,
    knobs: SqlKnobs {
        bool_literal: ["0", "1"],
        bytes_literal: hex_0x,
        text_projection: Some(text),
        lossy_text: lossy,
        text_expr: text,
        paging: Paging::OffsetFetch,
        limit_one: LimitOne::Top,
        count_expr: "COUNT_BIG(*)",
        nulls_order: NullsSort::NullFlag,
        like_wildcards: &['%', '_', '['],
        null_safe_eq: NullSafeEq::OrForm,
        returning: Returning::OutputInserted,
        server_expr,
        explain: explain_unsupported,
        catalog_words: &["INFORMATION_SCHEMA", "SYS."],
        redact: &[
            RedactRule {
                keyword: "PASSWORD",
            },
            RedactRule { keyword: "SECRET" },
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
    fn export_bindings_lex_tsql() {
        crate::sqlgen::dialect::lex::export_lex("tsql", &LEX, &RULES);
    }

    /// The renderer's cases run against `TSQL_LIKE_TEST`, so its rules must be these.
    #[test]
    fn the_test_spec_classifies_alike() {
        use crate::sqlgen::portable::testing::TSQL_RULES;
        assert_eq!(RULES.queries, TSQL_RULES.queries);
        assert_eq!(RULES.inspections, TSQL_RULES.inspections);
        assert_eq!(RULES.writes, TSQL_RULES.writes);
        assert_eq!(RULES.write_calls, TSQL_RULES.write_calls);
    }

    #[test]
    fn quotes_and_classifies() {
        assert_eq!((SPEC.quote_ident)("a]b"), "[a]]b]");
        assert_eq!((SPEC.quote_literal)("it's"), "N'it''s'");
        assert_eq!(
            classify_tsql("SELECT 1 DELETE FROM t"),
            StatementClass::Write
        );
        assert_eq!(
            classify_tsql("SELECT * FROM OPENQUERY(s, 'DELETE FROM t')"),
            StatementClass::Write
        );
        assert_eq!(
            classify_tsql("SELECT 1 ADD SENSITIVITY CLASSIFICATION TO dbo.t.c WITH (LABEL = 'x')"),
            StatementClass::Write
        );
        assert_eq!(classify_tsql("SELECT [x] FROM t"), StatementClass::Read);
        assert_eq!(
            classify_tsql("SELECT 1e5, 0x1F FROM t"),
            StatementClass::Read
        );
        assert_eq!(
            classify_tsql("SELECT 1xp_cmdshell('dir')"),
            StatementClass::Write
        );
        for sql in [
            // Statements a terminator need not end.
            "SELECT 1 RECEIVE * FROM dbo.q",
            "SELECT 1 MOVE CONVERSATION 'a' TO 'b'",
            "SELECT 1 SETUSER 'dbo'",
            "SELECT 1 SEND ON CONVERSATION @h (N'x')",
            "SELECT 1 GET CONVERSATION GROUP @g FROM dbo.q",
            "SELECT 1 END CONVERSATION @h",
            "SELECT 1 RENAME OBJECT t TO u",
            // A number ends before the keyword after it.
            "SELECT 1DELETE FROM t",
            "SELECT 1e0DELETE FROM t",
            "SELECT $1DELETE FROM t",
            "SELECT 0xINTO t2 FROM t",
            "SELECT 1EXEC sp_who",
            // Also past a decimal point, which the scanner keeps apart (`1`, `.`, `E5DELETE`).
            "SELECT 1.e5DELETE FROM t",
            "SELECT 1.eDELETE FROM t",
            "SELECT 1.E5EXEC sp_who",
        ] {
            assert_eq!(classify_tsql(sql), StatementClass::Write, "{sql}");
            assert!(
                !crate::sqlgen::portable::split::is_single_statement(&LEX, &RULES, sql),
                "{sql}"
            );
        }
        // Comments a reader may end elsewhere: at the lone CR or only at the LF, at the first
        // `*/` or the one that closes the nesting.
        for sql in [
            "SELECT 1 -- x\r' \nDELETE FROM t --'",
            "SELECT 1 /* /* */ DELETE FROM t */",
            "/* /* */ DELETE FROM t --*/ SELECT 1",
        ] {
            assert_eq!(classify_tsql(sql), StatementClass::Write, "{sql:?}");
            assert!(
                !crate::sqlgen::portable::split::is_single_statement(&LEX, &RULES, sql),
                "{sql:?}"
            );
        }
    }
}
