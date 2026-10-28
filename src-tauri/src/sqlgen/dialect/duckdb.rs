//! DuckDB SQL (the duckdb lane), as DuckDB 1.5 speaks it. Built with `drv-duckdb` only. (The
//! `duckdb` crate is `::duckdb` from here.)
//!
//! PostgreSQL-like text (libpg_query's lexer: double-quoted names, `E'…'` and `$tag$…$tag$`
//! strings, nesting block comments, backslashes plain in standard strings), `$1` parameters,
//! RETURNING, ILIKE, `EXPLAIN ANALYZE` that runs its statement, transactional DDL, no SAVEPOINT.
//! Queries may start with FROM, PIVOT / UNPIVOT (and PIVOT_WIDER / PIVOT_LONGER); SUMMARIZE,
//! DESCRIBE and SHOW read.
//!
//! DuckDB reads files and other databases from SQL, so the classifier calls these writes (the
//! sweep of DuckDB 1.5.6's `duckdb_functions()` with its built-in JSON and Parquet, and of the
//! documented core and well-known community extensions):
//!
//! - statements: COPY, ATTACH / DETACH, INSTALL / LOAD, EXPORT / IMPORT DATABASE, CHECKPOINT,
//!   VACUUM, CALL, PRAGMA, SET / RESET (settings and variables), and DML and DDL;
//! - calls of a file scanner (`read_*`: CSV, JSON, NDJSON, Parquet, text, blob, DuckDB files,
//!   Excel, Avro, Vortex; `glob`, `sniff_csv`, `parquet_*`, `st_read*`, `arrow_scan*`, which
//!   reads process memory), of a scanner of another server or format (`postgres_*`, `mysql_*`,
//!   `sqlite_*`, `odbc_*`, `adbc_*`, `iceberg_*`, `delta_*`, `ducklake_*`, `lance_*`,
//!   `vortex_*`, MotherDuck's `md_*`, `bigquery_*`, `snowflake_*`, `mongo_*`, `redis_*`,
//!   `airport_*`, `http_*`, `open_prompt`), of dynamic SQL (`query`, `query_table`,
//!   `json_execute_serialized_sql`, and `json_serialize_plan`, which binds the SQL in its string
//!   and so runs the binders of the scanners it names), of state changes (`nextval`, `setseed`,
//!   `write_log`, `enable_*` / `disable_*` logging and profiling, `truncate_duckdb_logs`,
//!   `force_checkpoint`, `pg_clear_cache`, `load_aws_credentials`, `dbgen` / `dsdgen`, the UI
//!   server's `start_ui*` / `stop_ui_server`), and of `getenv`, `duckdb_extensions` (which list
//!   the server's environment and extension folder), `duckdb_secrets` / `which_secret` (which
//!   load the persistent secrets from the secret folder) and `duckdb_table_sample`;
//! - a table function outside `table_functions` (the catalog's `duckdb_*` / `pragma_*`,
//!   `range`, `generate_series`, `unnest`, `repeat`, the JSON tree functions, the test types).
//!
//! It does not look for a file named where a table goes: a replacement scan (`FROM 'x.csv'`,
//! `FROM "C:/x.csv"`, `FROM x."/path"`, `FROM data.csv`, whose parts the binder joins into a
//! path when no table has that name) reads like a table to it, and so do views and macros
//! defined over a scanner. The engine refuses those reads: read-only sessions
//! (`access_mode=READ_ONLY`), unattended ones (`registry::unattended`, MCP) and plugin-origin
//! ones are opened without a temp directory, with `SET enable_external_access = false`, the
//! local file system off where no database file takes writes, and `SET lock_configuration =
//! true` (so no statement can turn any of it back on), extension autoload and autoinstall off
//! (`drivers::duckdb::engine`), and report `safety.hardened` (and `ReadOnlyMode::Server` when
//! read-only).

use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, PLAIN};
use crate::sqlgen::dialect::spec::{
    CiLike, DialectSpec, ExplainForm, PlaceholderStyle, RedactRule, Returning, SqlHooks, SqlKnobs,
    double_quoted, ph_dollar, single_quoted,
};
use crate::sqlgen::portable::classify;
use crate::types::mutate::ServerExpr;
use crate::types::query::ExplainMode;
use crate::types::schema::{ColumnMeta, ValueKind};

pub static LEX: LexProfile = LexProfile {
    ident_quotes: &[('"', '"')],
    string_quotes: &['\''],
    readings: &[PLAIN],
    e_strings: true,
    dollar_quotes: true,
    dollar_tags: true,
    triple_quotes: false,
    raw_prefixes: false,
    byte_prefixes: false,
    hash_comments: false,
    slash_slash_comments: false,
    // libpg_query's scanner nests them, as PostgreSQL's does.
    nested_block_comments: true,
    dash_comment_needs_space: false,
    batch_separator: None,
    terminator_optional: false,
    begin_end_bodies: false,
};

pub static RULES: ClassifyRules = ClassifyRules {
    // DuckDB queries may start with FROM; PIVOT_WIDER and PIVOT_LONGER spell PIVOT and UNPIVOT.
    queries: &[
        "SELECT",
        "WITH",
        "VALUES",
        "TABLE",
        "FROM",
        "PIVOT",
        "UNPIVOT",
        "PIVOT_WIDER",
        "PIVOT_LONGER",
    ],
    inspections: &["SHOW", "DESCRIBE", "DESC", "SUMMARIZE", "EXPLAIN", "USE"],
    // No INTO: DuckDB has no `SELECT … INTO` (a parser error, tested against the engine), every
    // statement that writes INTO spells INSERT or MERGE too, and `UNPIVOT t ON a, b INTO NAME n
    // VALUE v` is a read.
    writes: &[
        "INSERT",
        "UPDATE",
        "DELETE",
        "MERGE",
        "CREATE",
        "DROP",
        "ALTER",
        "TRUNCATE",
        "COPY",
        "ATTACH",
        "DETACH",
        "INSTALL",
        "LOAD",
        "EXPORT",
        "IMPORT",
        "CHECKPOINT",
        "VACUUM",
        "CALL",
        "PRAGMA",
        "SET",
        "RESET",
    ],
    write_calls: &[
        // Files of the server (CSV, JSON, Parquet, text, blobs, DuckDB, Excel, Avro, Vortex…),
        // and the memory of the process.
        "READ_*",
        "GLOB",
        "SNIFF_CSV",
        "PARQUET_*",
        "ST_READ*",
        "ARROW_SCAN*",
        // Other servers and formats (core and community extensions).
        "POSTGRES_*",
        "PG_CLEAR_CACHE",
        "MYSQL_*",
        "SQLITE_*",
        "ODBC_*",
        "ADBC_*",
        "ICEBERG_*",
        "DELTA_*",
        "DUCKLAKE_*",
        "LANCE_*",
        "VORTEX_*",
        "MD_*",
        "BIGQUERY_*",
        "SNOWFLAKE_*",
        "MONGO_*",
        "REDIS_*",
        "AIRPORT_*",
        "HTTP_*",
        "OPEN_PROMPT",
        "LOAD_AWS_CREDENTIALS",
        // Dynamic SQL.
        "QUERY",
        "QUERY_TABLE",
        "JSON_EXECUTE_SERIALIZED_SQL",
        // Binds the SQL in its string, so the scanners it names open their files.
        "JSON_SERIALIZE_PLAN",
        // State of the session or the server.
        "NEXTVAL",
        "SETSEED",
        "WRITE_LOG",
        "ENABLE_*",
        "DISABLE_*",
        "TRUNCATE_DUCKDB_LOGS",
        "FORCE_CHECKPOINT",
        "DBGEN",
        "DSDGEN",
        "START_UI*",
        "STOP_UI_SERVER",
        // The server's environment, extension folder and persistent secrets (loaded from the
        // secret folder on first use), and a table sampled by its name.
        "GETENV",
        "DUCKDB_EXTENSIONS",
        "DUCKDB_SECRETS",
        "WHICH_SECRET",
        "DUCKDB_TABLE_SAMPLE",
    ],
    table_functions: Some(&[
        "RANGE",
        "GENERATE_SERIES",
        "UNNEST",
        "DUCKDB_*",
        "PRAGMA_*",
        "JSON_EACH",
        "JSON_TREE",
        "REPEAT",
        "REPEAT_ROW",
        "REGEXP_SPLIT_TO_TABLE",
        "TEST_ALL_TYPES",
        "TEST_VECTOR_TYPES",
    ]),
};

fn classify_duckdb(sql: &str) -> StatementClass {
    classify::with(&LEX, &RULES, sql)
}

fn from_hex(hex: &str) -> String {
    format!("from_hex('{hex}')")
}

/// The SQL-standard clocks, without a precision.
fn server_expr(expr: ServerExpr, kind: ValueKind) -> Option<String> {
    match expr.clock() {
        Some((_, Some(_))) => None,
        _ => crate::sqlgen::dialect::spec::ansi_server_expr(expr, kind),
    }
}

/// Whether a type name holds a VARIANT, which the duckdb crate cannot decode.
fn has_variant(data_type: &str) -> bool {
    data_type
        .to_ascii_uppercase()
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|w| w == "VARIANT")
}

/// Columns the grid reads as text: a VARIANT anywhere in the type (the duckdb crate refuses to
/// decode it), and TIME WITH TIME ZONE, whose offset Arrow does not carry.
fn text_projection(e: &str, col: &ColumnMeta) -> Option<String> {
    let t = col.data_type.trim().to_ascii_uppercase();
    (has_variant(&t) || t == "TIME WITH TIME ZONE" || t == "TIMETZ")
        .then(|| format!("CAST({e} AS VARCHAR)"))
}

fn json_contains(column: &str, placeholder: &str) -> String {
    format!("json_contains({column}, {placeholder})")
}

/// `EXPLAIN` plans; `EXPLAIN ANALYZE` runs the statement (in a transaction it rolls back).
fn explain(mode: ExplainMode) -> ExplainForm {
    match mode {
        ExplainMode::Plan => ExplainForm::Prefix("EXPLAIN"),
        ExplainMode::Analyze => ExplainForm::Prefix("EXPLAIN ANALYZE"),
    }
}

pub static SPEC: DialectSpec = DialectSpec {
    dialect: SqlDialect::Duckdb,
    label: "DuckDB SQL",
    quote_ident: double_quoted,
    quote_literal: single_quoted,
    quote_literal_exec: single_quoted,
    placeholder: ph_dollar,
    placeholder_style: PlaceholderStyle::Dollar,
    placeholders_reusable: true,
    max_params: 10_000,
    max_name_parts: 3,
    lex: &LEX,
    rules: &RULES,
    classify: classify_duckdb,
    knobs: SqlKnobs {
        bytes_literal: from_hex,
        text_projection: Some(text_projection),
        ci_like: CiLike::Ilike,
        json_contains: Some(json_contains),
        returning: Returning::Returning,
        set_default: false,
        server_expr,
        explain,
        // No SAVEPOINT: inside a transaction an executing Explain Analyze of a write is refused.
        analyze_tx: Some(("BEGIN TRANSACTION", "ROLLBACK")),
        catalog_words: &["INFORMATION_SCHEMA", "DUCKDB_", "PRAGMA_", "PG_CATALOG"],
        side_effect_functions: &["NEXTVAL", "SETSEED", "SLEEP_MS", "PG_SLEEP", "WRITE_LOG"],
        // The secret options of CREATE SECRET (S3 / GCS / R2, HTTP, Azure, Hugging Face,
        // PostgreSQL, MySQL, Iceberg, Unity Catalog, Quack, MotherDuck), ATTACH's key, and the
        // settings that carry a credential (`SET s3_secret_access_key = '…'`), swept from the
        // secret providers and extension settings DuckDB 1.5.6 lists (extension_entries.hpp)
        // besides `duckdb_settings()`. A rule matches a whole word only.
        redact: &[
            RedactRule { keyword: "SECRET" },
            RedactRule {
                keyword: "PASSWORD",
            },
            RedactRule {
                keyword: "SESSION_TOKEN",
            },
            RedactRule { keyword: "TOKEN" },
            RedactRule {
                keyword: "BEARER_TOKEN",
            },
            RedactRule {
                keyword: "ENCRYPTION_KEY",
            },
            RedactRule {
                keyword: "CLIENT_SECRET",
            },
            RedactRule {
                keyword: "CONNECTION_STRING",
            },
            RedactRule {
                keyword: "PROXY_PASSWORD",
            },
            RedactRule {
                keyword: "HTTP_PROXY_PASSWORD",
            },
            RedactRule {
                keyword: "MOTHERDUCK_TOKEN",
            },
            RedactRule {
                keyword: "S3_SECRET_ACCESS_KEY",
            },
            RedactRule {
                keyword: "S3_SESSION_TOKEN",
            },
            RedactRule {
                keyword: "AZURE_STORAGE_CONNECTION_STRING",
            },
            RedactRule {
                keyword: "AZURE_PROXY_PASSWORD",
            },
            // The Azure `access_token` provider's token, and postgres_scanner's OAuth token
            // setting (`SET pg_oauth_token = '…'`).
            RedactRule {
                keyword: "ACCESS_TOKEN",
            },
            RedactRule {
                keyword: "PG_OAUTH_TOKEN",
            },
        ],
        ..SqlKnobs::ANSI
    },
    hooks: SqlHooks::NONE,
    ddl: None,
};

// --- Text the driver (`drivers::duckdb`) writes ---------------------------------------------

/// `ATTACH 'path' AS "name" (ENCRYPTION_KEY 'key', READ_ONLY)`: an encrypted database file,
/// read-only (DuckDB 1.5's built-in crypto only decrypts).
pub fn attach_encrypted(path: &str, name: &str, key: &str) -> String {
    format!(
        "ATTACH {} AS {} (ENCRYPTION_KEY {}, READ_ONLY)",
        (SPEC.quote_literal_exec)(path),
        (SPEC.quote_ident)(name),
        (SPEC.quote_literal_exec)(key)
    )
}

/// `USE "name"`: an attached database becomes the connection's current one.
pub fn use_database(name: &str) -> String {
    format!("USE {}", (SPEC.quote_ident)(name))
}

/// One overload of a macro, as `duckdb_functions()` lists it.
pub struct MacroOverload<'a> {
    pub parameters: &'a [String],
    pub table: bool,
    pub body: &'a str,
}

/// `CREATE MACRO "s"."m"("a", "b") AS body, ("n") AS TABLE body;`: a macro with every overload.
pub fn create_macro(schema: &str, name: &str, overloads: &[MacroOverload]) -> String {
    let quote = SPEC.quote_ident;
    let parts: Vec<String> = overloads
        .iter()
        .map(|o| {
            let params: Vec<String> = o.parameters.iter().map(|p| quote(p)).collect();
            format!(
                "({}) AS {}{}",
                params.join(", "),
                if o.table { "TABLE " } else { "" },
                o.body.trim()
            )
        })
        .collect();
    format!(
        "CREATE MACRO {}.{}{};",
        quote(schema),
        quote(name),
        parts.join(",\n    ")
    )
}

/// The keywords DuckDB 1.5.6's parser takes as no column or member name (`duckdb_keywords()`'s
/// `reserved` and `type_function` categories; a test keeps the list equal to the engine's),
/// separated by spaces.
pub const RESERVED_WORDS: &str = "all analyse analyze and anti any array as asc asof \
     asymmetric at authorization binary both by case cast check collate collation column columns \
     concurrently constraint create cross default deferrable desc describe distinct do else end \
     except false fetch for foreign freeze from full generated glob group having ilike in \
     initially inner intersect into is isnull join lambda lateral leading left like limit map \
     natural not notnull null offset on only or order outer overlaps pivot pivot_longer \
     pivot_wider placing positional primary qualify references returning right select semi show \
     similar some struct summarize symmetric table tablesample then to trailing true try_cast \
     union unique unpack unpivot using variadic verbose when where window with";

/// A STRUCT or UNION member's name in a type (`STRUCT(a INTEGER, "B c" VARCHAR)`), so that the
/// type parses back: bare when it is a plain lower-case word and no reserved keyword,
/// double-quoted otherwise. (DuckDB's own text also quotes the other keywords, `"key"`, and
/// leaves capitals bare.)
pub fn type_member_name(name: &str) -> String {
    let plain = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !RESERVED_WORDS.split_ascii_whitespace().any(|w| w == name);
    if plain {
        name.to_string()
    } else {
        (SPEC.quote_ident)(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_bindings_lex_duckdb() {
        crate::sqlgen::dialect::lex::export_lex("duckdb", &LEX, &RULES);
    }

    #[test]
    fn classifies_scanners_and_external_access_as_writes() {
        assert_eq!(classify_duckdb("FROM t SELECT a"), StatementClass::Read);
        for sql in [
            "SELECT * FROM range(10)",
            "PIVOT t ON c USING sum(x)",
            "SHOW TABLES",
            "SELECT * FROM (SELECT * FROM t) b, u",
            "SELECT * FROM \"main\".\"t\" /* c */ -- d\r\n",
            "SELECT * FROM (t CROSS JOIN u)",
            "SELECT * FROM (SELECT * FROM t) b",
            "SELECT * FROM (VALUES (1), (2)) v",
            "SELECT * FROM (FROM t) b",
            "SELECT 1e5, 0x1F FROM t",
            // The clauses that end a FROM list, whose own lists hold calls.
            "SELECT * FROM t WHERE a = 1 ORDER BY a, b",
            "SELECT a, lower(b) FROM t GROUP BY a, lower(b)",
            "SELECT * FROM t UNION ALL SELECT a, lower(b) FROM u",
            // A comparison's FROM names no table.
            "SELECT a IS DISTINCT FROM lower(b) FROM t",
            "SELECT a IS NOT DISTINCT FROM lower(b) FROM t",
            "PIVOT_WIDER t ON c USING sum(x)",
            "SELECT * FROM (PIVOT_LONGER t ON COLUMNS(*))",
            // Qualified and quoted tables, whatever their names look like.
            "SELECT * FROM main.t",
            "SELECT * FROM s.json JOIN \"my.table\" ON true",
            "SELECT t.csv FROM t JOIN u ON t.csv = u.json",
        ] {
            assert_eq!(classify_duckdb(sql), StatementClass::Read, "{sql}");
        }
        for sql in [
            "SELECT content FROM read_text('x')",
            "SELECT * FROM glob('*')",
            "SELECT * FROM main.read_csv('x.csv')",
            "SELECT * FROM st_read('C:/x.shp')",
            "COPY t TO 'x.csv'",
            "ATTACH 'x.db'",
            "INSTALL httpfs",
            "LOAD httpfs",
            "EXPORT DATABASE 'dir'",
            "IMPORT DATABASE 'dir'",
            // A subquery in the FROM list ends, and the list goes on.
            "SELECT * FROM (SELECT * FROM t) b, st_read('x.shp')",
            "SELECT * FROM (SELECT * FROM t) b, evil_fn()",
            "SELECT * FROM t JOIN (SELECT * FROM u) b ON true, st_read('x.shp')",
            // A clause keyword where a name goes leaves the list open: SETTINGS, PREWHERE and
            // FORMAT are not reserved here (an alias, a column, `format()`), and any keyword may
            // follow a `.` or an `AS`.
            "SELECT * FROM t settings, st_read('x.shp')",
            "SELECT * FROM t AS settings, st_read('x.shp')",
            "SELECT * FROM t prewhere, st_read('x.shp')",
            "SELECT * FROM t format, st_read('x.shp')",
            "SELECT * FROM range(3) format, st_read('x.shp')",
            "SELECT * FROM t JOIN u ON t.a = u.order, st_read('x.shp')",
            "SELECT * FROM t JOIN u ON t.a = u.where, st_read('x.shp')",
            "SELECT * FROM t JOIN u ON format('{}', a) = b, st_read('x.shp')",
            "SELECT * FROM t JOIN u ON t.a = settings, st_read('x.shp')",
            // PIVOT, UNPIVOT, SHOW and DESCRIBE take a table, or a table function; so do the
            // DuckDB spellings of PIVOT and UNPIVOT, also inside a query.
            "PIVOT st_read('x.shp') ON column0",
            "UNPIVOT st_read('x.shp') ON COLUMNS(*)",
            "DESCRIBE st_read('x.shp')",
            "PIVOT_WIDER st_read('x.shp') ON a",
            "SELECT * FROM (PIVOT_WIDER st_read('x.shp') ON column0)",
            "WITH p AS (PIVOT_WIDER st_read('x.shp') ON a) SELECT * FROM p",
            "SELECT * FROM t, (PIVOT_LONGER st_read('x.shp') ON COLUMNS(*))",
            // A set operator may be followed by a FROM-first query.
            "SELECT 1 UNION DISTINCT FROM st_read('x.shp')",
            "SELECT 1 EXCEPT DISTINCT FROM read_csv('x.csv')",
            "SELECT * FROM t INTERSECT DISTINCT FROM st_read('C:/x.shp')",
            "SELECT 1 UNION ALL FROM st_read('x.shp')",
            // The first table of a parenthesized join.
            "SELECT * FROM (st_read('C:/x.shp') CROSS JOIN t)",
            "SELECT * FROM t JOIN (st_read('x.shp') CROSS JOIN u) ON true",
            "SELECT * FROM (FROM st_read('x.shp')) b",
            // A number ends before the keyword after it (libpg_query's lexer predates PG 15).
            "SELECT 1DELETE FROM t",
            "SELECT 1e0FROM st_read('x.shp')",
            "SELECT 1FROM t, st_read('x.shp')",
            "SELECT 1read_text('x')",
            // Also past a decimal point: it reads `1.e0` as one number.
            "SELECT 1.e0FROM st_read('x.shp')",
            "SELECT 1.e5DELETE FROM t",
            // Comments a reader may end elsewhere (libpg_query nests `/* */`).
            "/* /* */ SELECT ' */ DELETE FROM t --'",
            "/* /* */ SELECT 1 */ COMMENT ON TABLE t IS 'x'",
            "SELECT 1 -- x\r' \nDELETE FROM t --'",
        ] {
            assert_eq!(classify_duckdb(sql), StatementClass::Write, "{sql}");
        }
    }

    /// Replacement scans read like tables here: the engine settings of the session refuse them
    /// (see the module doc), so a change to this list is a change to that contract.
    #[test]
    fn leaves_files_named_as_tables_to_the_engine() {
        for sql in [
            "SELECT * FROM 'x.csv'",
            "FROM \"data.parquet\"",
            "SELECT * FROM x.\"/data.csv\"",
            "SELECT * FROM data.csv",
            "DESCRIBE 'data.parquet'",
        ] {
            assert_eq!(classify_duckdb(sql), StatementClass::Read, "{sql}");
        }
    }

    fn column(data_type: &str) -> ColumnMeta {
        serde_json::from_value(serde_json::json!({
            "name": "c", "ordinal": 1, "dataType": data_type, "kind": "other",
            "nullable": true, "isPk": false, "isAutoIncrement": false, "isGenerated": false,
            "isUnsigned": false
        }))
        .unwrap()
    }

    #[test]
    fn knobs() {
        let project = |t: &str| text_projection("\"c\"", &column(t));
        assert_eq!(
            project("VARIANT").as_deref(),
            Some("CAST(\"c\" AS VARCHAR)")
        );
        assert_eq!(
            project("STRUCT(a VARIANT)").as_deref(),
            Some("CAST(\"c\" AS VARCHAR)")
        );
        assert_eq!(
            project("TIME WITH TIME ZONE").as_deref(),
            Some("CAST(\"c\" AS VARCHAR)")
        );
        assert_eq!(project("INTEGER"), None);
        assert_eq!(project("STRUCT(variant_id INTEGER)"), None);
        assert_eq!(
            explain(ExplainMode::Analyze),
            ExplainForm::Prefix("EXPLAIN ANALYZE")
        );
        assert_eq!(from_hex("00ff"), "from_hex('00ff')");
        assert_eq!(json_contains("\"j\"", "$1"), "json_contains(\"j\", $1)");
        // The scanners and state changes the sweep found, as calls anywhere.
        for sql in [
            "SELECT nextval('s')",
            "SELECT write_log('x')",
            "SELECT * FROM t WHERE x = getenv('HOME')",
            "SELECT * FROM duckdb_extensions()",
            "CALL enable_logging()",
            "SELECT * FROM disable_profiling()",
            "SELECT * FROM arrow_scan_dumb(1, 2, 3)",
            "SELECT * FROM read_duckdb('other.duckdb')",
            "SELECT * FROM json_execute_serialized_sql('{}')",
            "SELECT http_get('https://example.com')",
            "SELECT * FROM postgres_query('db', 'DELETE FROM t')",
            "SELECT * FROM duckdb_table_sample('t')",
            "SELECT * FROM histogram('t', a)",
            // Binds its string: the binder of read_duckdb opens the other file.
            "SELECT json_serialize_plan('SELECT * FROM read_duckdb(''C:/x.duckdb'')')",
            "SELECT json_serialize_plan('FROM ''C:/x.csv''', optimize := true)",
            "SELECT * FROM duckdb_secrets()",
            "SELECT * FROM which_secret('s3://b/x.parquet', 's3')",
        ] {
            assert_eq!(classify_duckdb(sql), StatementClass::Write, "{sql}");
        }
        for sql in [
            "SELECT * FROM duckdb_tables()",
            "SELECT * FROM pragma_table_info('t')",
            "SELECT * FROM test_all_types()",
            "SELECT * FROM repeat('x', 3)",
            "UNPIVOT monthly ON jan, feb INTO NAME month VALUE amount",
            "PIVOT_LONGER monthly ON COLUMNS(* EXCLUDE (id)) INTO NAME month VALUE amount",
            "SELECT currval('s'), current_setting('threads'), getvariable('v')",
            "SELECT md5('x'), struct_insert({'a': 1}, b := 2)",
            // Parses only: nothing is bound.
            "SELECT json_serialize_sql('SELECT * FROM read_csv(''x.csv'')')",
            "SELECT json_deserialize_sql(json_serialize_sql('SELECT 1'))",
        ] {
            assert_eq!(classify_duckdb(sql), StatementClass::Read, "{sql}");
        }
    }

    #[test]
    fn redacts_secrets_and_credential_settings() {
        let redact =
            |sql: &str| crate::sqlgen::portable::redact::redact(&LEX, SPEC.knobs.redact, sql);
        let mask = crate::sqlgen::admin::PASSWORD_MASK;
        for (sql, secret) in [
            ("CREATE SECRET (TYPE http, BEARER_TOKEN 'tok')", "tok"),
            (
                "CREATE SECRET s (TYPE s3, KEY_ID 'AKIA', SECRET 'sk', SESSION_TOKEN 'st')",
                "sk",
            ),
            (
                "CREATE SECRET s (TYPE s3, KEY_ID 'AKIA', SECRET 'sk', SESSION_TOKEN 'st')",
                "st",
            ),
            ("CREATE SECRET (TYPE http, HTTP_PROXY_PASSWORD 'pp')", "pp"),
            (
                "CREATE SECRET (TYPE azure, PROVIDER service_principal, CLIENT_SECRET 'cs')",
                "cs",
            ),
            (
                "CREATE SECRET (TYPE azure, CONNECTION_STRING 'AccountKey=k')",
                "AccountKey=k",
            ),
            ("CREATE SECRET (TYPE azure, PROXY_PASSWORD 'pp')", "pp"),
            ("CREATE SECRET (TYPE huggingface, TOKEN 'hf')", "hf"),
            ("CREATE SECRET (TYPE postgres, PASSWORD 'pw')", "pw"),
            ("SET s3_secret_access_key = 'sk'", "sk"),
            ("SET s3_session_token = 'st'", "st"),
            ("SET azure_storage_connection_string = 'cs'", "cs"),
            ("SET azure_proxy_password TO 'pp'", "pp"),
            ("SET http_proxy_password = 'pp'", "pp"),
            ("SET GLOBAL motherduck_token = 'md'", "md"),
            ("SET password = 'pw'", "pw"),
            ("ATTACH 'e.duckdb' AS e (ENCRYPTION_KEY 'k3y')", "k3y"),
            (
                "CREATE SECRET (TYPE azure, PROVIDER access_token, ACCESS_TOKEN 'tok1', \
                 ACCOUNT_NAME 'a')",
                "tok1",
            ),
            ("SET pg_oauth_token = 'tok2'", "tok2"),
            ("SET GLOBAL pg_oauth_token TO 'tok3'", "tok3"),
            (
                "CREATE SECRET (TYPE iceberg, CLIENT_ID 'c', CLIENT_SECRET 'ics')",
                "ics",
            ),
            ("ATTACH 'quack:h' AS q (TYPE quack, TOKEN 'qt')", "qt"),
        ] {
            let r = redact(sql);
            assert!(
                !r.sql.contains(&format!("'{secret}'")),
                "{sql} -> {}",
                r.sql
            );
            assert!(r.sql.contains(mask), "{sql} -> {}", r.sql);
            assert!(r.secrets.iter().any(|s| s == secret), "{sql}");
        }
        // The key id and the rest of a secret stay readable.
        let r = redact("CREATE SECRET s (TYPE s3, KEY_ID 'AKIA', SECRET 'sk', REGION 'eu')");
        assert!(
            r.sql.contains("'AKIA'") && r.sql.contains("'eu'"),
            "{}",
            r.sql
        );
        let r = redact(
            "CREATE SECRET (TYPE azure, PROVIDER access_token, ACCESS_TOKEN 'tok1', \
             ACCOUNT_NAME 'acct')",
        );
        assert!(r.sql.contains("'acct'"), "{}", r.sql);
    }

    #[test]
    fn driver_text_quotes_names_and_literals() {
        assert_eq!(
            attach_encrypted("C:/d/it's.duckdb", "a\"b", "k'y"),
            "ATTACH 'C:/d/it''s.duckdb' AS \"a\"\"b\" (ENCRYPTION_KEY 'k''y', READ_ONLY)"
        );
        assert_eq!(use_database("sales"), "USE \"sales\"");
        let params = ["n".to_string(), "My Arg".to_string()];
        let overloads = [
            MacroOverload {
                parameters: &params[..1],
                table: true,
                body: " SELECT * FROM range(n) ",
            },
            MacroOverload {
                parameters: &params,
                table: false,
                body: "n + 1",
            },
        ];
        assert_eq!(
            create_macro("main", "my macro", &overloads),
            "CREATE MACRO \"main\".\"my macro\"(\"n\") AS TABLE SELECT * FROM range(n),\n    \
             (\"n\", \"My Arg\") AS n + 1;"
        );
        assert_eq!(type_member_name("a_1"), "a_1");
        assert_eq!(type_member_name("Ab"), "\"Ab\"");
        assert_eq!(type_member_name("a\"b"), "\"a\"\"b\"");
        assert_eq!(type_member_name(""), "\"\"");
        assert_eq!(type_member_name("order"), "\"order\"");
        assert_eq!(type_member_name("key"), "key");
    }
}
