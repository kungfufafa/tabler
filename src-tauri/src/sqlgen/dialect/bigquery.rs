//! GoogleSQL, BigQuery's SQL (the bigquery lane). Built with `drv-bigquery` only.
//!
//! Backticked names and `'…'` / `"…"` strings, all with backslash escapes; triple-quoted, raw and
//! byte strings; `#`, `--` and (flat) block comments; scripts with `BEGIN … END`, `IF … END IF`
//! and the other blocks; named `@p1` parameters and `@@` system variables. There is no EXPLAIN
//! (a dry run estimates instead) and no RETURNING.
//!
//! Reads are `SELECT`, `WITH` and the pipe queries that start with `FROM`. Everything else
//! writes, and so does a query that runs another statement or reaches another service:
//! `EXECUTE IMMEDIATE`, `CALL`, scripting, `EXPORT DATA`, `LOAD DATA`, DCL, and the functions
//! of `WRITE_CALLS` (federated queries, remote models and Cloud AI APIs, Cloud KMS, Sensitive
//! Data Protection, Cloud Storage object functions, Earth Engine). The words `ML` and `AI` count
//! as writes wherever they are code: every BigQuery ML and AI function is called through them,
//! and whether a model is remote is not in the text (docs/drivers/bigquery.md, Classifier sweep).
//!
//! The statements the driver sends for itself (INFORMATION_SCHEMA reads) are built here too.

use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, TokenKind, reading, scan};
use crate::sqlgen::dialect::spec::{
    CiLike, DialectSpec, NullSafeEq, NullsSort, PlaceholderStyle, SqlHooks, SqlKnobs,
    backticked_backslash, explain_unsupported, ph_at_name, single_quoted_backslash,
};
use crate::sqlgen::portable::classify;
use crate::types::mutate::{Clock, ServerExpr};
use crate::types::schema::{ColumnMeta, ValueKind};

pub static LEX: LexProfile = LexProfile {
    ident_quotes: &[('`', '`')],
    string_quotes: &['\'', '"'],
    // Strings and backticked names both take backslash escapes, always.
    readings: &[reading(true, true)],
    e_strings: false,
    dollar_quotes: false,
    dollar_tags: false,
    triple_quotes: true,
    // GoogleSQL keeps a backslash and the character after it together in a raw string too ("both
    // characters are preserved"; a raw string cannot end in an odd number of backslashes), so
    // `r'\''` is one string. The shared scanner ends a raw string at the first quote, which
    // would read `r'\', ' ; DELETE FROM t; --'` as two strings around a comma where BigQuery
    // runs the DELETE. Without the flag the prefix is a word and the string after it reads
    // backslashes like any other, which is where BigQuery ends it.
    raw_prefixes: false,
    byte_prefixes: true,
    hash_comments: true,
    slash_slash_comments: false,
    // "Nested multiline comments aren't supported."
    nested_block_comments: false,
    dash_comment_needs_space: false,
    batch_separator: None,
    terminator_optional: false,
    begin_end_bodies: true,
};

/// The functions a query may call that reach another service or change something, by the last
/// part of their name (`ML.PREDICT(` calls `PREDICT`), and whole when the name is quoted in one
/// piece (`` `ML.PREDICT`( ``, also with the `SAFE.` prefix). The plain built-ins `IF`,
/// `TRANSLATE`, `SEARCH` and `AGG` share a last part with `AI.IF`, `ML.TRANSLATE`,
/// `AI.TRANSLATE`, `AI.SEARCH` and `AI.AGG`, so those are caught by the words `ML` and `AI` in
/// `writes` instead.
const WRITE_CALLS: &[&str] = &[
    // Federated queries: the SQL runs on Cloud SQL, Spanner or AlloyDB.
    "EXTERNAL_QUERY",
    // Signed URLs for the objects of an object table (Cloud Storage).
    "EXTERNAL_OBJECT_TRANSFORM",
    // Embeds a text or ObjectRef query value through the base table's connection and endpoint.
    "VECTOR_SEARCH",
    // Raster statistics from Earth Engine.
    "ST_REGIONSTATS",
    // Cloud KMS: the wrapped-keyset functions unwrap or rewrap through a KMS key.
    "KEYSET_CHAIN",
    "NEW_WRAPPED_KEYSET",
    "REWRAP_KEYSET",
    "ROTATE_WRAPPED_KEYSET",
    "KEYS.KEYSET_CHAIN",
    "KEYS.NEW_WRAPPED_KEYSET",
    "KEYS.REWRAP_KEYSET",
    "KEYS.ROTATE_WRAPPED_KEYSET",
    // Sensitive Data Protection (DLP) with a KMS-wrapped key.
    "DLP_*",
    // ObjectRef functions: Cloud Storage metadata, listings and signed URLs through a connection.
    "FETCH_METADATA",
    "GET_ACCESS_URL",
    "GET_READ_URL",
    "LIST",
    "OBJ.FETCH_METADATA",
    "OBJ.GET_ACCESS_URL",
    "OBJ.GET_READ_URL",
    "OBJ.LIST",
    // BigQuery ML and AI (also behind the words `ML` and `AI`): remote models on Vertex AI
    // endpoints, Gemini and the Cloud AI APIs. These last parts are no plain built-in, so a
    // separately quoted qualifier (`` `ml`.predict( ``) still counts.
    "ML.*",
    "AI.*",
    "PREDICT",
    "EVALUATE",
    "FORECAST",
    "DETECT_ANOMALIES",
    "GENERATE",
    "GENERATE_BOOL",
    "GENERATE_DOUBLE",
    "GENERATE_INT",
    "GENERATE_TABLE",
    "GENERATE_TEXT",
    "GENERATE_EMBEDDING",
    "EMBED",
    "SIMILARITY",
    "SCORE",
    "CLASSIFY",
    "COUNT_TOKENS",
    "CHUNK_DOCUMENT",
    "KEY_DRIVERS",
    "CAUSAL_EFFECT",
    "UNDERSTAND_TEXT",
    "PROCESS_DOCUMENT",
    "TRANSCRIBE",
    "ANNOTATE_IMAGE",
    // The same functions with GoogleSQL's one call prefix, quoted in one piece
    // (`` `safe.ml.predict`( ``). Unquoted, `SAFE.X(` calls `X`, which the entries above catch;
    // a bare `SAFE.*` would not hold for that spelling.
    "SAFE.EXTERNAL_QUERY",
    "SAFE.EXTERNAL_OBJECT_TRANSFORM",
    "SAFE.VECTOR_SEARCH",
    "SAFE.ST_REGIONSTATS",
    "SAFE.KEYS.KEYSET_CHAIN",
    "SAFE.KEYS.NEW_WRAPPED_KEYSET",
    "SAFE.KEYS.REWRAP_KEYSET",
    "SAFE.KEYS.ROTATE_WRAPPED_KEYSET",
    "SAFE.DLP_*",
    "SAFE.OBJ.FETCH_METADATA",
    "SAFE.OBJ.GET_ACCESS_URL",
    "SAFE.OBJ.GET_READ_URL",
    "SAFE.OBJ.LIST",
    "SAFE.ML.*",
    "SAFE.AI.*",
];

pub static RULES: ClassifyRules = ClassifyRules {
    // Pipe queries may start with `FROM` (`FROM t |> WHERE …`).
    queries: &["SELECT", "WITH", "FROM"],
    inspections: &[],
    writes: &[
        // DML and DDL (`UNDROP SCHEMA` too), the statements that move data in or out, DCL.
        "INTO", "INSERT", "UPDATE", "DELETE", "MERGE", "CREATE", "DROP", "ALTER", "TRUNCATE",
        "UNDROP", "EXPORT", "LOAD", "GRANT", "REVOKE",
        // Statements that run others, scripting and transactions.
        "CALL", "EXECUTE", "DECLARE", "SET", "BEGIN", "COMMIT", "ROLLBACK",
        // The qualifiers of the BigQuery ML and AI functions (see `WRITE_CALLS`).
        "ML", "AI",
    ],
    write_calls: WRITE_CALLS,
    // Table-valued functions are the user's own (and APPENDS, CHANGES, GAP_FILL, UNNEST): there
    // is no fixed list to allow. The ones that reach other services are in `write_calls`.
    table_functions: None,
};

fn classify_bigquery(sql: &str) -> StatementClass {
    classify::with(&LEX, &RULES, sql)
}

fn from_hex(hex: &str) -> String {
    format!("FROM_HEX('{hex}')")
}

/// A column as text for filters and "Any column": a cast where GoogleSQL has one, JSON text for
/// the nested and JSON kinds (which no cast turns into a string), WKT for geography; bytes have
/// no text (a cast fails on invalid UTF-8).
fn text_expr(e: &str, col: &ColumnMeta) -> Option<String> {
    match col.kind {
        ValueKind::Bytes => None,
        ValueKind::Json | ValueKind::Array => Some(format!("TO_JSON_STRING({e})")),
        ValueKind::Geometry => Some(format!("ST_ASTEXT({e})")),
        ValueKind::Other if col.data_type.to_ascii_uppercase().starts_with("STRUCT") => {
            Some(format!("TO_JSON_STRING({e})"))
        }
        _ => Some(format!("CAST({e} AS STRING)")),
    }
}

/// GoogleSQL's clocks take no precision (TIMESTAMP and DATETIME hold microseconds).
fn server_expr(expr: ServerExpr, kind: ValueKind) -> Option<String> {
    if expr == ServerExpr::CurrentDate {
        return Some("CURRENT_DATE()".into());
    }
    let (clock, _) = expr.clock()?;
    Some(
        match clock {
            Clock::CurrentTime => "CURRENT_TIME()",
            Clock::LocalTimestamp => "CURRENT_DATETIME()",
            // A DATETIME column takes the wall-clock time, a TIMESTAMP the instant.
            _ if kind == ValueKind::Timestamp => "CURRENT_DATETIME()",
            _ => "CURRENT_TIMESTAMP()",
        }
        .into(),
    )
}

/// `SET @@dataset_project_id = …` and `SET @@dataset_id = …` change where unqualified names
/// resolve for the rest of the script or session.
fn switch_db(sql: &str) -> bool {
    let Some(&reading) = LEX.readings.first() else {
        return false;
    };
    let s = scan(&LEX, reading, sql);
    let code: Vec<String> = s
        .tokens
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::Space | TokenKind::Comment))
        .map(|t| s.text(t))
        .collect();
    code.windows(4).any(|w| {
        w[0].eq_ignore_ascii_case("SET")
            && w[1] == "@"
            && w[2] == "@"
            && (w[3].eq_ignore_ascii_case("DATASET_PROJECT_ID")
                || w[3].eq_ignore_ascii_case("DATASET_ID"))
    })
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
        text_expr,
        count_expr: "COUNT(*)",
        nulls_order: NullsSort::Native,
        // No ILIKE: `LOWER(x) LIKE LOWER(p)`.
        ci_like: CiLike::LowerLike,
        // LIKE takes `\` before `%`, `_` and `\` and has no ESCAPE clause.
        like_escape: None,
        null_safe_eq: NullSafeEq::IsNotDistinctFrom,
        insert_all_defaults: None,
        set_default: false,
        server_expr,
        explain: explain_unsupported,
        switch_db,
        catalog_words: &["INFORMATION_SCHEMA"],
        cancel_notice: "a statement that had finished keeps its changes, the cancelled one makes \
                        none",
        ..SqlKnobs::ANSI
    },
    hooks: SqlHooks::NONE,
    ddl: None,
};

// --- The driver's own statements -----------------------------------------------------------------

/// `` `project`.`dataset`.`table` `` (without the project, it resolves in the job's project).
pub fn table_path(project: Option<&str>, dataset: &str, table: &str) -> String {
    let mut parts: Vec<String> = project.into_iter().map(backticked_backslash).collect();
    parts.push(backticked_backslash(dataset));
    parts.push(backticked_backslash(table));
    parts.join(".")
}

/// A dataset's INFORMATION_SCHEMA view (`TABLES`, `COLUMNS`, `ROUTINES`): views of a dataset
/// must be qualified by it. Every read of one is billed (10 MB at least), so the driver reads
/// them only on demand.
fn information_schema(project: Option<&str>, dataset: &str, view: &str) -> String {
    let mut parts: Vec<String> = project.into_iter().map(backticked_backslash).collect();
    parts.push(backticked_backslash(dataset));
    format!("{}.INFORMATION_SCHEMA.{view}", parts.join("."))
}

/// The CREATE statement of one table or view (`@name`: its name). Show DDL, on demand.
pub fn table_ddl_query(project: Option<&str>, dataset: &str) -> String {
    format!(
        "SELECT ddl FROM {} WHERE table_name = @name",
        information_schema(project, dataset, "TABLES")
    )
}

/// The CREATE statement of one routine (`@name`: its name).
pub fn routine_ddl_query(project: Option<&str>, dataset: &str) -> String {
    format!(
        "SELECT ddl FROM {} WHERE routine_name = @name",
        information_schema(project, dataset, "ROUTINES")
    )
}

/// Every column of a dataset, for completion (one billed read per dataset, cached by the
/// session).
pub fn completion_columns_query(project: Option<&str>, dataset: &str) -> String {
    format!(
        "SELECT table_name, column_name FROM {} ORDER BY table_name, ordinal_position",
        information_schema(project, dataset, "COLUMNS")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlgen::dialect::lex::listed;
    use crate::sqlgen::portable::testing::col;

    #[test]
    fn export_bindings_lex_bigquery() {
        crate::sqlgen::dialect::lex::export_lex("bigquery", &LEX, &RULES);
    }

    #[test]
    fn quotes_names_and_strings() {
        assert_eq!((SPEC.quote_ident)("my-proj.ds`"), "`my-proj.ds\\``");
        assert_eq!((SPEC.quote_ident)("a\\b"), "`a\\\\b`");
        assert_eq!((SPEC.quote_literal)("a\nb'\\"), "'a\\nb\\'\\\\'");
        assert_eq!((SPEC.placeholder)(3, None), "@p3");
        assert_eq!(from_hex("00ff"), "FROM_HEX('00ff')");
        // A quoted value is one string to the classifier, whatever it holds.
        for v in [
            "x'; DELETE FROM t; --",
            "a\\",
            "\\'; DROP TABLE t; #",
            "'''",
        ] {
            let sql = format!("SELECT {}", (SPEC.quote_literal)(v));
            assert_eq!(classify_bigquery(&sql), StatementClass::Read, "{sql}");
            let sql = format!("SELECT 1 FROM {}", (SPEC.quote_ident)(v));
            assert_eq!(classify_bigquery(&sql), StatementClass::Read, "{sql}");
        }
    }

    #[test]
    fn raw_strings_end_where_bigquery_ends_them() {
        // `\'` stays in a raw string, so the DELETE is code to BigQuery.
        for sql in [
            "SELECT r'\\', ' ; DELETE FROM t; --'",
            "SELECT R\"\\\", \" ; DELETE FROM t; --\"",
            "SELECT rb'\\', ' ; DELETE FROM t; --'",
            "SELECT br'\\', ' ; DELETE FROM t; --'",
            "SELECT r'''\\''', ''' ; DELETE FROM t; --'''",
        ] {
            assert_eq!(classify_bigquery(sql), StatementClass::Write, "{sql}");
        }
        for sql in [
            "SELECT r'\\' ; DELETE FROM t; --'",
            "SELECT r'a\\\\' AS x",
            "SELECT r'''it's \\''' DELETE'''",
            "SELECT b'\\x00; DELETE'",
        ] {
            assert_eq!(classify_bigquery(sql), StatementClass::Read, "{sql}");
        }
    }

    #[test]
    fn scripts_and_calls_are_writes() {
        for sql in [
            "EXECUTE IMMEDIATE 'DELETE FROM t'",
            "EXECUTE IMMEDIATE \"SELECT 1\"",
            "CALL p()",
            "CALL BQ.ABORT_SESSION()",
            "BEGIN SELECT 1; END",
            "BEGIN TRANSACTION",
            "DECLARE x INT64 DEFAULT 1",
            "SET @@dataset_id = 'd'",
            "IF TRUE THEN SELECT 1; END IF",
            "FOR r IN (SELECT 1 AS x) DO SELECT r.x; END FOR",
            "EXPORT DATA OPTIONS (uri = 'gs://b/*.csv', format = 'CSV') AS SELECT * FROM t",
            "LOAD DATA INTO d.t FROM FILES (format = 'CSV', uris = ['gs://b/x.csv'])",
            "SELECT * FROM EXTERNAL_QUERY('us.conn', 'SELECT 1')",
            "SELECT * FROM ML.PREDICT(MODEL d.m, TABLE d.t)",
            "SELECT AI.IF(('is it?', x), connection_id => 'us.c') FROM t",
            "SELECT * FROM `ml.predict`(MODEL d.m, TABLE d.t)",
            "SELECT * FROM `ml`.predict(MODEL d.m, TABLE d.t)",
            "SELECT KEYS.KEYSET_CHAIN('gcp-kms://k', b'x')",
            "SELECT OBJ.FETCH_METADATA(ref) FROM t",
        ] {
            assert_eq!(classify_bigquery(sql), StatementClass::Write, "{sql}");
        }
        for sql in [
            "SELECT IF(a, 1, 2), TRANSLATE(s, 'ab', 'xy') FROM t",
            "SELECT SEARCH(t, 'x') FROM d.t",
            "FROM d.t |> WHERE a > 1 |> AGGREGATE COUNT(*) GROUP BY b",
            "SELECT KEYS.NEW_KEYSET('AEAD_AES_GCM_256')",
        ] {
            assert_eq!(classify_bigquery(sql), StatementClass::Read, "{sql}");
        }
    }

    /// The holes the rules cannot close, each under Limits in the brief with its change request
    /// to the shared classifier. They read today; when a change request lands and a case here
    /// turns into a write, move it to the suite's WRITES.
    #[test]
    fn documented_holes_read_until_their_change_requests() {
        for sql in [
            // A separately quoted qualifier hides the words `AI` and `ML`, and the last part is a
            // plain built-in (qualified callees).
            "SELECT `ai`.if(('spam?', body)) FROM d.mail",
            "SELECT `safe.ai`.if(('spam?', body)) FROM d.mail",
            "SELECT `ai`.translate(body, 'en') FROM d.docs",
            "SELECT * FROM `ml`.translate(MODEL d.tr, TABLE d.docs, STRUCT('en' AS x))",
            "SELECT * FROM `ai`.search(TABLE d.docs, 'body', 'cats')",
            "SELECT `ai`.agg(('sum up', body)) FROM d.docs",
            // A quoted name takes the escapes of a string (`\u0073` is `s`); the classifier
            // matches it as written (escaped quoted names).
            "SELECT `st_regionstat\\u0073`(geo, 'ee://x') FROM t",
            "SELECT * FROM `external_quer\\u0079`('c', 'q')",
            "SELECT * FROM `\\u006dl.predict`(MODEL d.m, TABLE d.t)",
            "SELECT `keys`.`keyset_chai\\u006e`('gcp-kms://k', b'x')",
        ] {
            assert_eq!(classify_bigquery(sql), StatementClass::Read, "{sql}");
        }
    }

    #[test]
    fn the_write_calls_are_no_read_starters_and_spell_upper_case() {
        for c in WRITE_CALLS {
            assert_eq!(*c, c.to_ascii_uppercase(), "{c}");
            assert!(!listed(RULES.queries, c), "{c}");
        }
        // The plain built-ins that share a last part with an AI or ML function stay reads.
        for f in [
            "IF",
            "TRANSLATE",
            "SEARCH",
            "AGG",
            "GENERATE_ARRAY",
            "GENERATE_UUID",
        ] {
            assert!(!listed(WRITE_CALLS, f), "{f}");
        }
    }

    #[test]
    fn text_forms_and_clocks() {
        let name = col("name", "STRING", ValueKind::Text);
        let blob = col("b", "BYTES", ValueKind::Bytes);
        let tags = col("tags", "ARRAY<STRING>", ValueKind::Array);
        let rec = col("r", "STRUCT<a INT64>", ValueKind::Other);
        let geo = col("g", "GEOGRAPHY", ValueKind::Geometry);
        assert_eq!(
            text_expr("`name`", &name).as_deref(),
            Some("CAST(`name` AS STRING)")
        );
        assert_eq!(text_expr("`b`", &blob), None);
        assert_eq!(
            text_expr("`tags`", &tags).as_deref(),
            Some("TO_JSON_STRING(`tags`)")
        );
        assert_eq!(
            text_expr("`r`", &rec).as_deref(),
            Some("TO_JSON_STRING(`r`)")
        );
        assert_eq!(text_expr("`g`", &geo).as_deref(), Some("ST_ASTEXT(`g`)"));
        let at = |e, k| server_expr(e, k);
        assert_eq!(
            at(ServerExpr::Now, ValueKind::TimestampTz).as_deref(),
            Some("CURRENT_TIMESTAMP()")
        );
        assert_eq!(
            at(ServerExpr::Now, ValueKind::Timestamp).as_deref(),
            Some("CURRENT_DATETIME()")
        );
        assert_eq!(
            at(
                ServerExpr::Fsp(Clock::CurrentTimestamp, 3),
                ValueKind::TimestampTz
            )
            .as_deref(),
            Some("CURRENT_TIMESTAMP()")
        );
        assert_eq!(
            at(ServerExpr::CurrentDate, ValueKind::Date).as_deref(),
            Some("CURRENT_DATE()")
        );
        assert_eq!(
            at(ServerExpr::CurrentTime, ValueKind::Time).as_deref(),
            Some("CURRENT_TIME()")
        );
        assert_eq!(at(ServerExpr::Default, ValueKind::Int), None);
    }

    #[test]
    fn switching_the_default_project_or_dataset() {
        assert!(switch_db("SET @@dataset_project_id = 'p'"));
        assert!(switch_db("set @@Dataset_Id='d'"));
        assert!(switch_db("SELECT 1; SET @@dataset_id = 'd'"));
        assert!(!switch_db("SET @@query_label = 'a:b'"));
        assert!(!switch_db("SELECT '@@dataset_id'"));
        assert!(!switch_db("SELECT 1 # SET @@dataset_id = 'd'"));
    }

    #[test]
    fn builds_the_drivers_statements() {
        assert_eq!(
            table_path(Some("my-proj"), "ds", "t`x"),
            "`my-proj`.`ds`.`t\\`x`"
        );
        assert_eq!(table_path(None, "ds", "t"), "`ds`.`t`");
        assert_eq!(
            table_ddl_query(Some("p"), "d"),
            "SELECT ddl FROM `p`.`d`.INFORMATION_SCHEMA.TABLES WHERE table_name = @name"
        );
        assert_eq!(
            routine_ddl_query(None, "d"),
            "SELECT ddl FROM `d`.INFORMATION_SCHEMA.ROUTINES WHERE routine_name = @name"
        );
        assert_eq!(
            completion_columns_query(Some("p"), "d"),
            "SELECT table_name, column_name FROM `p`.`d`.INFORMATION_SCHEMA.COLUMNS ORDER BY \
             table_name, ordinal_position"
        );
        // The driver's reads are reads.
        for sql in [
            table_ddl_query(Some("p"), "d"),
            routine_ddl_query(Some("p"), "d"),
            completion_columns_query(Some("delete"), "drop"),
        ] {
            assert_eq!(classify_bigquery(&sql), StatementClass::Read, "{sql}");
        }
    }
}
