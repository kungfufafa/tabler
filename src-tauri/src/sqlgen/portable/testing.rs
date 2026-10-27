//! Test specs of the portable builders, and their goldens.
//!
//! `ANSI_TEST` is a plain SQL dialect (double-quoted names, `?` placeholders, LIMIT/OFFSET,
//! dollar-quoted bodies, two readings of backslashes); `TSQL_LIKE_TEST` reads and writes like
//! T-SQL (brackets, `N'…'`, `@P1`, OFFSET/FETCH, TOP, OUTPUT INSERTED, `GO` batches, optional
//! terminators). Tests of shared code run them in place of a lane's spec through
//! `dialect::testing::override_spec`. Their lexer profiles and classifier rules are exported to
//! `bindings/lex/{ansi_test,tsql_like_test}.json`, and their classifier and splitter cases live in
//! `tests/fixtures/portable/*.cases.json`, which the renderer's portable scanner runs too.
//!
//! The goldens in `tests/fixtures/sqlgen-portable/*.txt` record what the builders make of both
//! specs and of variants that cover every `Paging`, `LimitOne`, `PlaceholderStyle`, `NullsSort`
//! and `CiLike`. `TABLER_BLESS=1 cargo test --lib sqlgen::portable` rewrites them (review the
//! diff).

use std::collections::BTreeMap;

use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, PLAIN, reading};
use crate::sqlgen::dialect::spec::{
    CiLike, DialectSpec, LimitOne, NullSafeEq, NullsSort, Paging, PlaceholderStyle, RedactRule,
    Returning, SqlHooks, SqlKnobs, bracketed, double_quoted, hex_0x, national_quoted, ph_at_p,
    ph_question, single_quoted,
};
use crate::sqlgen::portable::classify;
use crate::types::schema::{ColumnMeta, ValueKind};
use crate::types::value::Value;

pub static ANSI_LEX: LexProfile = LexProfile {
    ident_quotes: &[('"', '"')],
    string_quotes: &['\''],
    readings: &[PLAIN, reading(true, false)],
    e_strings: false,
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
    begin_end_bodies: true,
};

pub static ANSI_RULES: ClassifyRules = ClassifyRules {
    queries: &["SELECT", "WITH", "VALUES", "TABLE"],
    inspections: &["SHOW", "DESCRIBE", "DESC", "EXPLAIN", "USE"],
    writes: &[
        "INTO", "INSERT", "UPDATE", "DELETE", "MERGE", "CREATE", "DROP", "ALTER", "TRUNCATE",
        "CALL", "EXEC", "EXECUTE", "GRANT", "REVOKE", "COPY",
    ],
    write_calls: &["READ_*", "LOAD_FILE"],
    table_functions: Some(&["GENERATE_SERIES", "UNNEST", "SYSTEM_*"]),
};

fn ansi_classify(sql: &str) -> StatementClass {
    classify::with(&ANSI_LEX, &ANSI_RULES, sql)
}

fn json_contains(column: &str, ph: &str) -> String {
    format!("JSON_CONTAINS({column}, {ph})")
}

pub static ANSI_TEST: DialectSpec = DialectSpec {
    dialect: SqlDialect::Vertica,
    label: "ANSI test",
    quote_ident: double_quoted,
    quote_literal: single_quoted,
    quote_literal_exec: single_quoted,
    placeholder: ph_question,
    placeholder_style: PlaceholderStyle::Question,
    placeholders_reusable: false,
    max_params: 1000,
    max_name_parts: 2,
    lex: &ANSI_LEX,
    rules: &ANSI_RULES,
    classify: ansi_classify,
    knobs: SqlKnobs {
        json_contains: Some(json_contains),
        returning: Returning::Returning,
        side_effect_functions: &["SLEEP"],
        catalog_words: &["INFORMATION_SCHEMA"],
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

pub static TSQL_LEX: LexProfile = LexProfile {
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

pub static TSQL_RULES: ClassifyRules = ClassifyRules {
    queries: &["SELECT", "WITH"],
    inspections: &["USE"],
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
        "RECEIVE",
        "SEND",
        "MOVE",
        "GET",
        "CONVERSATION",
        "RENAME",
    ],
    write_calls: &["OPENQUERY", "OPENROWSET", "OPENDATASOURCE", "XP_*", "SP_*"],
    table_functions: None,
};

fn tsql_classify(sql: &str) -> StatementClass {
    classify::with(&TSQL_LEX, &TSQL_RULES, sql)
}

/// Text of every column but binary ones; spatial values as WKT, which drops their SRID (lossy).
fn tsql_text(e: &str, col: &ColumnMeta) -> Option<String> {
    match col.kind {
        ValueKind::Bytes => None,
        ValueKind::Geometry => Some(format!("{e}.STAsText()")),
        _ => Some(format!("CONVERT(NVARCHAR(MAX), {e})")),
    }
}

fn tsql_lossy(col: &ColumnMeta) -> bool {
    col.kind == ValueKind::Geometry
}

pub static TSQL_LIKE_TEST: DialectSpec = DialectSpec {
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
    lex: &TSQL_LEX,
    rules: &TSQL_RULES,
    classify: tsql_classify,
    knobs: SqlKnobs {
        bool_literal: ["0", "1"],
        bytes_literal: hex_0x,
        text_projection: Some(tsql_text),
        lossy_text: tsql_lossy,
        text_expr: tsql_text,
        paging: Paging::OffsetFetch,
        limit_one: LimitOne::Top,
        count_expr: "COUNT_BIG(*)",
        nulls_order: NullsSort::NullFlag,
        like_wildcards: &['%', '_', '['],
        null_safe_eq: NullSafeEq::OrForm,
        returning: Returning::OutputInserted,
        ..SqlKnobs::ANSI
    },
    hooks: SqlHooks::NONE,
    ddl: None,
};

/// A column of the test table.
pub fn col(name: &str, data_type: &str, kind: ValueKind) -> ColumnMeta {
    serde_json::from_value(serde_json::json!({
        "name": name, "ordinal": 1, "dataType": data_type, "kind": kind, "nullable": true,
        "isPk": false, "isAutoIncrement": false, "isGenerated": false, "isUnsigned": false,
    }))
    .expect("a column")
}

/// A table with a column of each shape the portable builders treat differently.
pub fn columns() -> Vec<ColumnMeta> {
    let mut id = col("id", "bigint", ValueKind::BigInt);
    id.is_pk = true;
    id.pk_ordinal = Some(1);
    id.nullable = false;
    vec![
        id,
        col("name", "nvarchar(50)", ValueKind::Text),
        col("age", "int", ValueKind::Int),
        col("price", "decimal(10,2)", ValueKind::Decimal),
        col("payload", "json", ValueKind::Json),
        col("data", "varbinary(max)", ValueKind::Bytes),
        col("geom", "geography", ValueKind::Geometry),
        col("Odd \"Name\" ]", "nvarchar(10)", ValueKind::Text),
        col("created", "datetime2(3)", ValueKind::Timestamp),
    ]
}

pub fn text(s: &str) -> Value {
    Value::Text(s.into())
}

pub fn map<const N: usize>(pairs: [(&str, Value); N]) -> BTreeMap<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// `ANSI_TEST` with another placeholder style.
pub fn with_placeholders(style: PlaceholderStyle) -> DialectSpec {
    use crate::sqlgen::dialect::spec::{
        PlaceholderFn, ph_at_name, ph_colon, ph_dollar, ph_question_numbered,
    };
    fn typed(n: usize, col: Option<&ColumnMeta>) -> String {
        format!(
            "{{p{n}:{}}}",
            col.map_or("String", |c| c.data_type.as_str())
        )
    }
    let (placeholder, reusable): (PlaceholderFn, bool) = match style {
        PlaceholderStyle::Dollar => (ph_dollar, true),
        PlaceholderStyle::Question => (ph_question, false),
        PlaceholderStyle::QuestionNumbered => (ph_question_numbered, true),
        PlaceholderStyle::AtP => (ph_at_p, true),
        PlaceholderStyle::AtName => (ph_at_name, true),
        PlaceholderStyle::Colon => (ph_colon, true),
        PlaceholderStyle::Typed => (typed, true),
    };
    DialectSpec {
        placeholder,
        placeholder_style: style,
        placeholders_reusable: reusable,
        ..ANSI_TEST
    }
}

/// `ANSI_TEST` with other knobs.
pub fn ansi_with(knobs: impl FnOnce(&mut SqlKnobs)) -> DialectSpec {
    let mut spec = ANSI_TEST;
    knobs(&mut spec.knobs);
    spec
}

pub const PLACEHOLDER_STYLES: [PlaceholderStyle; 7] = [
    PlaceholderStyle::Dollar,
    PlaceholderStyle::Question,
    PlaceholderStyle::QuestionNumbered,
    PlaceholderStyle::AtP,
    PlaceholderStyle::AtName,
    PlaceholderStyle::Colon,
    PlaceholderStyle::Typed,
];

pub const CI_LIKES: [CiLike; 3] = [CiLike::Ilike, CiLike::LowerLike, CiLike::Unsupported];

#[test]
fn export_bindings_lex_test_specs() {
    use crate::sqlgen::dialect::lex::export_lex;
    export_lex("ansi_test", &ANSI_LEX, &ANSI_RULES);
    export_lex("tsql_like_test", &TSQL_LEX, &TSQL_RULES);
}

/// The classifier and splitter cases both sides run (`tests/fixtures/portable`).
mod cases {
    use serde::Deserialize;

    use super::*;
    use crate::sqlgen::portable::split::split;

    #[derive(Deserialize)]
    struct ClassifyCase {
        sql: String,
        class: String,
    }

    #[derive(Deserialize)]
    struct SplitCase {
        sql: String,
        statements: Vec<String>,
    }

    #[derive(Deserialize)]
    struct Cases {
        classify: Vec<ClassifyCase>,
        split: Vec<SplitCase>,
    }

    fn run(json: &str, lex: &LexProfile, rules: &ClassifyRules) {
        let cases: Cases = serde_json::from_str(json).expect("cases parse");
        let mut failures = Vec::new();
        for c in &cases.classify {
            let got = match classify::with(lex, rules, &c.sql) {
                StatementClass::Read => "read",
                StatementClass::Write => "write",
            };
            if got != c.class {
                failures.push(format!("classify {:?}: {got}, want {}", c.sql, c.class));
            }
        }
        for c in &cases.split {
            let got: Vec<String> = split(lex, &c.sql).into_iter().map(|s| s.sql).collect();
            if got != c.statements {
                failures.push(format!(
                    "split {:?}: {got:?}, want {:?}",
                    c.sql, c.statements
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn ansi_cases() {
        run(
            include_str!("../../../tests/fixtures/portable/ansi_test.cases.json"),
            &ANSI_LEX,
            &ANSI_RULES,
        );
    }

    #[test]
    fn tsql_like_cases() {
        run(
            include_str!("../../../tests/fixtures/portable/tsql_like_test.cases.json"),
            &TSQL_LEX,
            &TSQL_RULES,
        );
    }

    #[test]
    fn statement_offsets_are_characters() {
        let got = split(&ANSI_LEX, "SELECT 'é';\n  -- c\n  SELECT 2;");
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].start, got[0].end, got[0].line), (0, 10, 1));
        assert_eq!((got[1].start, got[1].end, got[1].line), (21, 29, 3));
        assert_eq!(got[1].sql, "SELECT 2");
    }

    #[test]
    fn helpers_read_code_only() {
        use crate::sqlgen::portable::classify::{calls_listed_function, code_only};
        assert_eq!(
            code_only(&TSQL_LEX, "SELECT [x] -- c\nFROM t WHERE a = N'DELETE'"),
            "SELECT    \nFROM t WHERE a = N "
        );
        assert!(calls_listed_function(
            &ANSI_LEX,
            &["SLEEP"],
            "SELECT sleep (1)"
        ));
        assert!(!calls_listed_function(
            &ANSI_LEX,
            &["SLEEP"],
            "SELECT 'sleep(1)'"
        ));
        // To a server that ends `--` only at the LF, the call is code.
        assert!(calls_listed_function(
            &ANSI_LEX,
            &["SLEEP"],
            "SELECT 1 -- x\r'\n, sleep(1) --'"
        ));
    }

    #[test]
    fn a_single_statement_is_one_for_the_server() {
        use crate::sqlgen::portable::split::is_single_statement;
        let ansi = |sql| is_single_statement(&ANSI_LEX, &ANSI_RULES, sql);
        let tsql = |sql| is_single_statement(&TSQL_LEX, &TSQL_RULES, sql);
        assert!(ansi(""));
        assert!(ansi("SELECT 1;"));
        assert!(!ansi("SELECT 1; SELECT 2"));
        // Under the backslash reading the string ends later: two statements under one reading.
        assert!(!ansi(r"SELECT 'a\'; SELECT 2 --'"));
        // The statements of a block count too.
        assert!(!ansi("CREATE PROCEDURE p() BEGIN SELECT 1; SELECT 2; END"));
        for sql in [
            "SELECT 1;",
            "SELECT * FROM t WHERE a = 'x; DELETE' -- ; SELECT 2",
            "SELECT TOP (5) WITH TIES * FROM t WITH (NOLOCK) ORDER BY a",
            "WITH a AS (SELECT 1 AS x) SELECT x FROM a UNION ALL SELECT 2",
            "(SELECT 1) UNION (SELECT 2)",
            "SELECT * FROM t WHERE id IN (SELECT id FROM u)",
            "DELETE FROM t WHERE id = 1",
            "SELECT 1e5, 0x1F FROM t",
            "SELECT 1.5, 1.e5, t.a FROM t",
        ] {
            assert!(tsql(sql), "{sql}");
        }
        // A batch holds several statements, and none needs a `;` before the next.
        for sql in [
            "SELECT 1; SELECT 2",
            "SELECT 1; DELETE FROM t",
            "SELECT 1\nGO\nSELECT 2",
            "SELECT 1 DELETE FROM t",
            "SELECT 1 SELECT 2",
            "SELECT * FROM t WHERE a IN (1, 2) SELECT 2",
            "WITH a AS (SELECT 1) SELECT * FROM a SELECT 2",
            "SELECT 1 WITH a AS (SELECT 2) SELECT * FROM a",
            "SELECT 1 USE master",
            "SELECT 1 ADD SIGNATURE TO p BY CERTIFICATE c",
            "SELECT 1 RECEIVE * FROM dbo.q",
            "SELECT 1 MOVE CONVERSATION 'a' TO 'b'",
            "SELECT 1 SETUSER 'dbo'",
            "SELECT 1 END CONVERSATION @h",
            // A number ends before the keyword after it, wherever the server ends it.
            "SELECT 1DELETE FROM t",
            "SELECT 1e0DELETE FROM t",
            "SELECT $1DELETE FROM t",
            "SELECT 0xINTO t2 FROM t",
            "SELECT 1EXEC sp_who",
            "SELECT 1SELECT 2",
            "SELECT 1.e5DELETE FROM t",
            "SELECT 1.eSELECT 2",
            // A write's own words could start another statement: unsure, so not one.
            "INSERT INTO t VALUES (1)",
        ] {
            assert!(!tsql(sql), "{sql}");
        }
        // A comment a server may end elsewhere may hide a second statement.
        for sql in [
            "SELECT 1 -- x\r' \nDELETE FROM t --'",
            "/* /* */ SELECT ' */ DELETE FROM t --'",
            "/* /* */ DELETE FROM t --*/ SELECT 1",
        ] {
            assert!(!ansi(sql), "{sql:?}");
            assert!(!tsql(sql), "{sql:?}");
        }
        assert!(ansi("-- c\r\nSELECT 1 /* d */\r\n"));
    }
}
