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
//! (`access_mode=READ_ONLY`) and unattended ones (`registry::unattended`, MCP) are opened
//! without a temp directory, with `SET enable_external_access = false`, the
//! local file system off where no database file takes writes, and `SET lock_configuration =
//! true` (so no statement can turn any of it back on), extension autoload and autoinstall off
//! (`drivers::duckdb::engine`), and report `safety.hardened` (and `ReadOnlyMode::Server` when
//! read-only). A plugin's run on a session that is not hardened is refused before it runs when
//! it reaches files, extensions, secrets or settings (`reads_files`, the session's
//! `admit_plugin_run`): lexical too, so a view or macro of the file over a scanner still reads.

use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{
    ClassifyRules, LexProfile, PLAIN, Scan, Token, TokenKind, listed,
};
use crate::sqlgen::dialect::spec::{
    CiLike, DialectSpec, ExplainForm, PlaceholderStyle, RedactCall, RedactRule, Returning,
    SqlHooks, SqlKnobs, double_quoted, ph_dollar, single_quoted,
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
    attribute_calls: false,
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
        // The aggregates, because `t PIVOT (sum(x) FOR …)` puts one where a table goes: a `%`
        // entry allows the unqualified name inside a pivot's `(… FOR …)` only, so `FROM sum(…)`
        // stays a write (a table macro of that name in the file would run its body there). Exact
        // names of DuckDB 1.5.6's built-in aggregates.
        "%SUM",
        "%COUNT",
        "%COUNT_IF",
        "%AVG",
        "%MEAN",
        "%MIN",
        "%MAX",
        "%FIRST",
        "%LAST",
        "%ANY_VALUE",
        "%ARBITRARY",
        "%LIST",
        "%ARRAY_AGG",
        "%STRING_AGG",
        "%GROUP_CONCAT",
        "%LISTAGG",
        "%MEDIAN",
        "%MODE",
        "%QUANTILE_CONT",
        "%QUANTILE_DISC",
        "%STDDEV",
        "%STDDEV_POP",
        "%STDDEV_SAMP",
        "%VARIANCE",
        "%VAR_POP",
        "%VAR_SAMP",
        "%BOOL_AND",
        "%BOOL_OR",
        "%PRODUCT",
        "%ARG_MAX",
        "%ARG_MIN",
        "%MAX_BY",
        "%MIN_BY",
        "%FSUM",
        "%KAHAN_SUM",
        "%BIT_AND",
        "%BIT_OR",
        "%BIT_XOR",
    ]),
};

fn classify_duckdb(sql: &str) -> StatementClass {
    classify::with(&LEX, &RULES, sql)
}

fn from_hex(hex: &str) -> String {
    format!("from_hex('{hex}')")
}

/// The SQL-standard clocks, without a precision.
/// `current_date` lives in the ICU extension, which the bundled build lacks (a catalog error).
fn server_expr(expr: ServerExpr, kind: ValueKind) -> Option<String> {
    match expr.clock() {
        _ if expr == ServerExpr::CurrentDate => None,
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
        // A Parquet encryption key passed by position: `PRAGMA add_parquet_key('name', 'key')`.
        redact_calls: &[RedactCall {
            function: "ADD_PARQUET_KEY",
            argument: 2,
        }],
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

/// The names of DuckDB's logger switches: the `enable_logging()` table function and the
/// `enable_logging` / `logging_storage` settings.
const LOGGER_SWITCHES: [&str; 2] = ["ENABLE_LOGGING", "LOGGING_STORAGE"];

/// DuckDB's functions that run the SQL text they are given (`query('…')`, …).
const DYNAMIC_SQL: [&str; 3] = ["QUERY", "QUERY_TABLE", "JSON_EXECUTE_SERIALIZED_SQL"];

/// Whether `sql` names DuckDB's logger switches anywhere in its code (a word or a quoted name,
/// not a string or a comment), or calls dynamic SQL (`DYNAMIC_SQL`) with the letters `LOGGING`
/// anywhere, strings included (a concatenation may split the switch's name). File storage the
/// instance cannot write leaves a logger that fails every later statement, and closing a
/// connection then aborts the process (DuckDB 1.5.6 throws from a destructor), so the driver
/// refuses such a statement before it reaches the engine. A text the scanner may read otherwise
/// than DuckDB is judged by its letters alone. Lexical only: a stored view or macro, a variable
/// or a name built from smaller pieces still reaches the engine, and `engine` then keeps the
/// connections the logger broke from closing.
pub fn switches_logger(sql: &str) -> bool {
    let named = |text: &str| {
        let upper = text.to_ascii_uppercase();
        LOGGER_SWITCHES.iter().any(|w| upper.contains(w))
    };
    let logging = sql.to_ascii_uppercase().contains("LOGGING");
    if !logging {
        return false;
    }
    let s = crate::sqlgen::dialect::lex::scan(&LEX, PLAIN, sql);
    if s.ambiguous {
        return true;
    }
    let code: Vec<_> = s
        .tokens
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::Space | TokenKind::Comment))
        .collect();
    let dynamic = code.windows(2).any(|w| {
        let name = match w[0].kind {
            TokenKind::Word => s.upper(w[0]),
            TokenKind::Ident => s.ident_name(w[0]).to_ascii_uppercase(),
            _ => return false,
        };
        DYNAMIC_SQL.contains(&name.as_str()) && s.is_punct(w[1], '(')
    });
    if dynamic {
        return true;
    }
    code.iter().any(|t| match t.kind {
        TokenKind::Word => LOGGER_SWITCHES.contains(&s.upper(t).as_str()),
        TokenKind::Ident => named(&s.text(t)),
        _ => false,
    })
}

/// Words that name a file, an extension or a secret in any statement (`COPY`, `ATTACH`,
/// `INSTALL`, `LOAD`, `EXPORT` / `IMPORT DATABASE`, `PRAGMA`, `CREATE SECRET`, `UPDATE
/// EXTENSIONS`), wherever they stand.
const FILE_WORDS: [&str; 9] = [
    "COPY",
    "ATTACH",
    "INSTALL",
    "LOAD",
    "EXPORT",
    "IMPORT",
    "PRAGMA",
    "SECRET",
    "EXTENSIONS",
];

/// Statements that change the instance's settings or call a procedure (a hardened instance's
/// configuration is locked): `SET`, `RESET`, `CALL`, `FORCE INSTALL` / `FORCE CHECKPOINT`.
const SETTING_STATEMENTS: [&str; 4] = ["SET", "RESET", "CALL", "FORCE"];

/// Calls of `write_calls` that touch no file: the state of the session.
const SESSION_CALLS: [&str; 2] = ["NEXTVAL", "SETSEED"];

/// The last part of a name a replacement scan reads as a file (`FROM data.csv`, `FROM
/// logs.json.gz`): DuckDB's CSV, JSON and Parquet suffixes, their compressions, and the formats
/// of well-known extensions.
const FILE_SUFFIXES: [&str; 37] = [
    "CSV", "TSV", "TBL", "TXT", "DAT", "PSV", "PARQUET", "PARQ", "PQ", "JSON", "JSONL", "NDJSON",
    "GEOJSON", "GZ", "GZIP", "ZST", "ZSTD", "BZ2", "XZ", "LZ4", "ZIP", "XLSX", "XLS", "ARROW",
    "IPC", "FEATHER", "AVRO", "ORC", "SHP", "GPKG", "FGB", "SQLITE", "SQLITE3", "DB", "DUCKDB",
    "DDB", "VORTEX",
];

/// Words after which a table goes (a call there is a table function); `FROM` lists also go on
/// after a comma.
const TABLE_AT: [&str; 11] = [
    "FROM",
    "JOIN",
    "TABLE",
    "DESCRIBE",
    "DESC",
    "SUMMARIZE",
    "SHOW",
    "PIVOT",
    "UNPIVOT",
    "PIVOT_WIDER",
    "PIVOT_LONGER",
];

/// Words after which a table's name may go but a call is no table; after `USING` a call is one
/// except in a pivot (`PIVOT … USING sum(x)`) or an index (`CREATE INDEX … USING art (a)`).
const NAME_AT: [&str; 3] = ["USING", "INTO", "UPDATE"];

/// Words after which `TABLE` names the table a statement defines or drops, so that `(` after the
/// name opens its columns (`CREATE TEMP TABLE t (x INT)`), not a call.
const TABLE_DEFINED_AFTER: [&str; 8] = [
    "CREATE",
    "TEMP",
    "TEMPORARY",
    "REPLACE",
    "UNLOGGED",
    "ALTER",
    "DROP",
    "TRUNCATE",
];

/// Words that end a `FROM` list at its own depth: reserved words only, which no alias can be
/// (`FROM range(1) AS set, 'x.csv'` goes on with the list).
const FROM_LIST_ENDS: [&str; 13] = [
    "WHERE",
    "GROUP",
    "HAVING",
    "QUALIFY",
    "WINDOW",
    "ORDER",
    "LIMIT",
    "OFFSET",
    "UNION",
    "EXCEPT",
    "INTERSECT",
    "RETURNING",
    "SELECT",
];

/// Words that start a query in a table's place (`FROM (VALUES …)`, `FROM (FROM t)`): its own
/// tables are looked at where they stand. Every start of DuckDB's `select_with_parens`, which
/// takes `(DESC SELECT …)` and `(SHOW …)` too.
const QUERY_AT: [&str; 13] = [
    "SELECT",
    "VALUES",
    "FROM",
    "WITH",
    "TABLE",
    "PIVOT",
    "UNPIVOT",
    "PIVOT_WIDER",
    "PIVOT_LONGER",
    "SUMMARIZE",
    "DESCRIBE",
    "DESC",
    "SHOW",
];

/// Statements whose table follows without a word of `TABLE_AT` (`TRUNCATE "x.csv"`, `VACUUM
/// ANALYZE "x.csv"`), which their binder runs the replacement scan on all the same.
const MAINTENANCE_STATEMENTS: [&str; 4] = ["TRUNCATE", "VACUUM", "ANALYZE", "ANALYSE"];

/// Words that may stand between a `MAINTENANCE_STATEMENTS` word and its table (`TRUNCATE TABLE
/// ONLY t`, `VACUUM FULL FREEZE VERBOSE ANALYZE t`).
const MAINTENANCE_OPTIONS: [&str; 7] = [
    "TABLE", "ONLY", "FULL", "FREEZE", "VERBOSE", "ANALYZE", "ANALYSE",
];

/// Functions whose arguments may say `FROM` (`EXTRACT(YEAR FROM ts)`), which no table follows;
/// a group that opens with a query is a table's (`INSERT INTO substr (SELECT …)`), not theirs.
const FROM_IN_ARGUMENTS: [&str; 6] = [
    "EXTRACT",
    "SUBSTRING",
    "SUBSTR",
    "TRIM",
    "OVERLAY",
    "POSITION",
];

/// What a session that is not hardened refuses of a plugin's run (`admit_plugin_run`): what a
/// hardened one's engine refuses, so a plugin reads no file through a read-write profile. A text
/// reaches files when, in its code (never a string or a comment):
///
/// - it says a word of `FILE_WORDS` anywhere (`COPY`, `ATTACH`, `INSTALL`, `LOAD`, `EXPORT`,
///   `IMPORT`, `PRAGMA`, `SECRET`, `EXTENSIONS`), or a statement starts with `SET`, `RESET`,
///   `CALL` or `FORCE`;
/// - a statement that binds a table without a word of `TABLE_AT` names a file there:
///   `TRUNCATE`, `VACUUM` and `ANALYZE` past their options, and `ON` in `CREATE [UNIQUE] INDEX`,
///   also behind `EXPLAIN` with its options or `PREPARE … AS` (`statement_verb`);
/// - it calls a function of `write_calls` other than `nextval` and `setseed` (the scanners,
///   other servers, dynamic SQL, the environment), spelled any way the classifier knows;
/// - where a table goes it names a string (`FROM 'x.csv'`, `DESCRIBE 'x.csv'`), a quoted name
///   holding a path's characters (`FROM "C:/x.csv"`, `FROM x."/path"`), a name of several parts
///   whose last one is a file's suffix (`FROM data.csv`, which the binder joins into a path), a
///   Unicode-escaped name, or a call outside `table_functions` (a table macro of the file may
///   scan a file; an aggregate only right inside `PIVOT (`), looking past a prefix alias
///   (`FROM f: 'x.csv'`) and taking an unreserved word before `.` as a name (`FROM values.csv`);
/// - the scanner may read it otherwise than DuckDB (`Scan::ambiguous`, or a non-ASCII
///   character in code that it splits a word at, `splits_names`), or a number runs into
///   letters (`1FROM`, which DuckDB reads as `1 FROM`).
///
/// Lexical, like the classifier: a view or a macro of the file that scans a file still reads it
/// when queried by its plain name (brief, Limits).
pub fn reads_files(sql: &str) -> bool {
    let s = crate::sqlgen::dialect::lex::scan(&LEX, PLAIN, sql);
    if s.ambiguous || splits_names(&s) {
        return true;
    }
    let calls: Vec<&str> = RULES
        .write_calls
        .iter()
        .copied()
        .filter(|c| !SESSION_CALLS.contains(c))
        .collect();
    if classify::calls_listed_function(&LEX, &calls, sql) {
        return true;
    }
    let code: Vec<&Token> = s
        .tokens
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::Space | TokenKind::Comment))
        .collect();
    let word = |i: usize| {
        code.get(i)
            .filter(|t| t.kind == TokenKind::Word)
            .map(|t| s.upper(t))
    };
    let punct = |i: usize, c: char| code.get(i).is_some_and(|t| s.is_punct(t, c));
    // Per open parenthesis: whether it holds the arguments of `FROM_IN_ARGUMENTS`.
    let mut parens: Vec<bool> = Vec::new();
    // The depths of the open FROM lists, innermost last.
    let mut froms: Vec<usize> = Vec::new();
    // The depths of the PIVOT words, where `USING` takes aggregates.
    let mut pivots: Vec<usize> = Vec::new();
    // The statement's first words from its verb on, for `CREATE [UNIQUE] INDEX … USING art (a)`.
    let mut head: Vec<String> = Vec::new();
    // Where the statement's verb stands; the words up to it lead the statement.
    let mut verb = statement_verb(&s, &code, 0);
    for i in 0..code.len() {
        let w = word(i);
        if w.as_deref().is_some_and(glued_number) {
            return true;
        }
        let leading = i <= verb;
        if let Some(w) = w
            .as_deref()
            .filter(|_| i >= verb && head.len() < 3 && parens.is_empty())
        {
            head.push(w.to_string());
        }
        let index = matches!(
            head.iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .as_slice(),
            ["CREATE", "INDEX", ..] | ["CREATE", "UNIQUE", "INDEX"]
        );
        if leading
            && w.as_deref()
                .is_some_and(|w| MAINTENANCE_STATEMENTS.contains(&w))
            && maintenance_reads_files(&s, &code, i + 1)
        {
            return true;
        }
        if leading
            && w.as_deref()
                .is_some_and(|w| SETTING_STATEMENTS.contains(&w))
        {
            return true;
        }
        // `U&"…"` may spell any name.
        if w.as_deref() == Some("U") && punct(i + 1, '&') {
            return true;
        }
        let in_from = froms.last() == Some(&parens.len());
        let reaches = if punct(i, ';') {
            parens.clear();
            froms.clear();
            pivots.clear();
            head.clear();
            verb = statement_verb(&s, &code, i + 1);
            false
        } else if punct(i, '(') {
            let args = i.checked_sub(1).and_then(word);
            let query =
                punct(i + 1, '(') || word(i + 1).is_some_and(|n| QUERY_AT.contains(&n.as_str()));
            parens.push(!query && args.is_some_and(|p| FROM_IN_ARGUMENTS.contains(&p.as_str())));
            false
        } else if punct(i, ')') {
            parens.pop();
            while froms.last().is_some_and(|&d| d > parens.len()) {
                froms.pop();
            }
            while pivots.last().is_some_and(|&d| d > parens.len()) {
                pivots.pop();
            }
            false
        } else if punct(i, ',') {
            in_from && table_reads_files(&s, &code, i + 1, true)
        } else if let Some(w) = w.as_deref() {
            // `a IS [NOT] DISTINCT FROM b` compares.
            let distinct = i.checked_sub(1).and_then(word).as_deref() == Some("DISTINCT")
                && matches!(
                    i.checked_sub(2).and_then(word).as_deref(),
                    Some("IS" | "NOT")
                );
            if FILE_WORDS.contains(&w) {
                true
            } else if w == "FROM" {
                if distinct || parens.last().copied().unwrap_or(false) {
                    false
                } else {
                    if !in_from {
                        froms.push(parens.len());
                    }
                    table_reads_files(&s, &code, i + 1, true)
                }
            } else if w == "TABLE" {
                let defined = i
                    .checked_sub(1)
                    .and_then(word)
                    .is_some_and(|p| TABLE_DEFINED_AFTER.contains(&p.as_str()));
                table_reads_files(&s, &code, i + 1, !defined)
            } else if TABLE_AT.contains(&w) {
                if w.contains("PIVOT") {
                    pivots.push(parens.len());
                }
                table_reads_files(&s, &code, i + 1, true)
            } else if w == "ON" && index && parens.is_empty() {
                // `CREATE INDEX i ON "x.csv" (a)` binds its table as a query would.
                table_reads_files(&s, &code, i + 1, false)
            } else if w == "USING" {
                // A table macro after `MERGE … USING` or `DELETE … USING` may scan a file.
                let aggregates = index || pivots.last() == Some(&parens.len());
                table_reads_files(&s, &code, i + 1, !aggregates)
            } else if NAME_AT.contains(&w) {
                table_reads_files(&s, &code, i + 1, false)
            } else {
                if in_from && FROM_LIST_ENDS.contains(&w) {
                    froms.pop();
                }
                false
            }
        } else {
            false
        };
        if reaches {
            return true;
        }
    }
    false
}

/// Whether the scanner splits a word where DuckDB does not: libpg_query's lexer reads every
/// byte of 0x80 or above as part of a name, so `r€where` is one alias to DuckDB, while the
/// scanner ends the word at `€` (punctuation) or at a Unicode space and sees `WHERE`.
fn splits_names(s: &Scan) -> bool {
    s.tokens.iter().any(|t| {
        matches!(t.kind, TokenKind::Punct | TokenKind::Space)
            && s.chars
                .get(t.start..t.end)
                .is_some_and(|cs| cs.iter().any(|c| !c.is_ascii()))
    })
}

/// Whether a word that starts with a digit runs into letters other than a number's (`1FROM`,
/// which DuckDB reads as `1 FROM`): `1e5`, `1e` before `-5`, `0x1F` and `0b101` are numbers.
fn glued_number(w: &str) -> bool {
    let Some(rest) = w.strip_prefix(|c: char| c.is_ascii_digit()) else {
        return false;
    };
    let all = |t: &str, ok: fn(char) -> bool| t.chars().all(|c| ok(c) || c == '_');
    let zero = w.starts_with('0');
    let number = match rest.strip_prefix('X').or_else(|| rest.strip_prefix('B')) {
        Some(n) if zero && rest.starts_with('X') => all(n, |c| c.is_ascii_hexdigit()),
        Some(n) if zero => all(n, |c| c == '0' || c == '1'),
        _ => match rest.split_once('E') {
            Some((m, e)) => all(m, |c| c.is_ascii_digit()) && all(e, |c| c.is_ascii_digit()),
            None => all(rest, |c| c.is_ascii_digit()),
        },
    };
    !number
}

/// Where the verb of the statement that starts at `at` stands, past what may come in front of it
/// and still bind its table: `(`, `EXPLAIN [( options )] [ANALYZE] [VERBOSE]` and `PREPARE name
/// [( types )] AS` (`EXPLAIN (FORMAT JSON) TRUNCATE "x.csv"`, `PREPARE p AS TRUNCATE "x.csv"`).
fn statement_verb(s: &Scan, code: &[&Token], mut at: usize) -> usize {
    let is_punct = |i: usize, c: char| code.get(i).is_some_and(|t| s.is_punct(t, c));
    let upper = |i: usize| {
        code.get(i)
            .filter(|t| t.kind == TokenKind::Word)
            .map(|t| s.upper(t))
    };
    // Past the group that opens at `at`, or `at` when none does.
    let past_group = |mut at: usize| {
        if !is_punct(at, '(') {
            return at;
        }
        let mut depth = 0usize;
        while code.get(at).is_some() {
            if is_punct(at, '(') {
                depth += 1;
            } else if is_punct(at, ')') {
                depth = depth.saturating_sub(1);
            }
            at += 1;
            if depth == 0 {
                break;
            }
        }
        at
    };
    loop {
        if is_punct(at, '(') {
            at += 1;
            continue;
        }
        match upper(at).as_deref() {
            Some("EXPLAIN") => {
                at = past_group(at + 1);
                while upper(at)
                    .is_some_and(|w| matches!(w.as_str(), "ANALYZE" | "ANALYSE" | "VERBOSE"))
                {
                    at += 1;
                }
            }
            Some("PREPARE") => {
                at = past_group(at + 2);
                if upper(at).as_deref() == Some("AS") {
                    at += 1;
                }
            }
            _ => return at,
        }
    }
}

/// Whether the table of a `MAINTENANCE_STATEMENTS` statement, after its options from `at` on
/// (`VACUUM (ANALYZE) t`), reaches a file; every word on the way is looked at as a table too.
fn maintenance_reads_files(s: &Scan, code: &[&Token], mut at: usize) -> bool {
    let is_punct = |i: usize, c: char| code.get(i).is_some_and(|t| s.is_punct(t, c));
    loop {
        if table_reads_files(s, code, at, false) {
            return true;
        }
        let option = code
            .get(at)
            .filter(|t| t.kind == TokenKind::Word)
            .is_some_and(|t| MAINTENANCE_OPTIONS.contains(&s.upper(t).as_str()));
        if option && !is_punct(at + 1, '.') && !is_punct(at + 1, ':') {
            at += 1;
        } else if is_punct(at, '(') {
            let mut depth = 0usize;
            loop {
                if is_punct(at, '(') {
                    depth += 1;
                } else if is_punct(at, ')') {
                    depth = depth.saturating_sub(1);
                } else if code.get(at).is_none() {
                    return false;
                }
                at += 1;
                if depth == 0 {
                    break;
                }
            }
        } else {
            return false;
        }
    }
}

/// Whether the group opened at `open` holds `FOR` at its own depth: a pivot's `(aggregate …
/// FOR …)`, not `PIVOT (t) ON …`, whose parenthesis holds a table.
fn pivot_for(s: &Scan, code: &[&Token], open: usize) -> bool {
    let mut depth = 0usize;
    for t in code.iter().skip(open) {
        if s.is_punct(t, '(') {
            depth += 1;
        } else if s.is_punct(t, ')') {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return false;
            }
        } else if depth == 1 && t.kind == TokenKind::Word && s.upper(t) == "FOR" {
            return true;
        }
    }
    false
}

/// Whether what stands at the table position `at` reaches a file (`reads_files`); `calls`:
/// whether a call there is a table function (after `USING`, `sum(x)` is a pivot's aggregate).
fn table_reads_files(s: &Scan, code: &[&Token], mut at: usize, calls: bool) -> bool {
    let is_punct = |i: usize, c: char| code.get(i).is_some_and(|t| s.is_punct(t, c));
    let upper = |i: usize| {
        code.get(i)
            .filter(|t| t.kind == TokenKind::Word)
            .map(|t| s.upper(t))
    };
    // Right inside a pivot's `(… FOR …)`, an aggregate may be called (`PIVOT (sum(x) FOR …)`).
    let pivot = is_punct(at, '(')
        && at
            .checked_sub(1)
            .and_then(upper)
            .is_some_and(|w| w.contains("PIVOT"))
        && pivot_for(s, code, at);
    while is_punct(at, '(') || matches!(upper(at).as_deref(), Some("LATERAL" | "ONLY")) {
        at += 1;
    }
    let Some(first) = code.get(at) else {
        return false;
    };
    match first.kind {
        TokenKind::Str => return true,
        TokenKind::Word if query_at(s, code, at) => return false,
        TokenKind::Word | TokenKind::Ident => {}
        _ => return false,
    }
    // The name's parts: a word in upper case, a quoted name as written.
    let mut parts: Vec<(TokenKind, String)> = Vec::new();
    let mut i = at;
    loop {
        match code.get(i) {
            Some(t) if t.kind == TokenKind::Word => parts.push((t.kind, s.upper(t))),
            Some(t) if t.kind == TokenKind::Ident => parts.push((t.kind, s.ident_name(t))),
            _ => break,
        }
        if is_punct(i + 1, '.') && code.get(i + 2).is_some() {
            i += 2;
        } else {
            break;
        }
    }
    let path_like = |n: &str| n.contains(['.', '/', '\\', ':', '*', '?', '~', '[', '{', '%']);
    if parts
        .iter()
        .any(|(k, n)| *k == TokenKind::Ident && path_like(n))
    {
        return true;
    }
    let suffix = parts
        .last()
        .is_some_and(|(_, n)| FILE_SUFFIXES.contains(&n.to_ascii_uppercase().as_str()));
    if parts.len() > 1 && suffix {
        return true;
    }
    // A prefix alias (`FROM f: 'x.csv'`): the table follows the colon.
    if is_punct(i + 1, ':') {
        return table_reads_files(s, code, i + 2, calls);
    }
    if !(calls && is_punct(i + 1, '(')) {
        return false;
    }
    // A call: only unqualified and unquoted, on the list (an aggregate right inside `PIVOT (`).
    let allowed = RULES.table_functions.unwrap_or(&[]);
    match parts.as_slice() {
        [(TokenKind::Word, name)] => !allowed.iter().any(|entry| match entry.strip_prefix('%') {
            Some(aggregate) => pivot && aggregate == name.as_str(),
            None => listed(&[*entry], name),
        }),
        _ => true,
    }
}

/// Whether the word at the table position `at` starts a query rather than a name: words such
/// as `VALUES` and `SUMMARIZE` are unreserved, so `FROM values.csv` names a file and
/// `FROM summarize: 'x.csv'` an alias; `VALUES` lists rows only before `(`.
fn query_at(s: &Scan, code: &[&Token], at: usize) -> bool {
    let next = |c: char| code.get(at + 1).is_some_and(|t| s.is_punct(t, c));
    match code.get(at).map(|t| s.upper(t)).as_deref() {
        Some("VALUES") => next('('),
        Some(w) => QUERY_AT.contains(&w) && !next('.') && !next(':'),
        None => false,
    }
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
    fn logger_switches_are_found_in_code_only() {
        for sql in [
            "CALL enable_logging(storage := 'file', storage_path := 'C:/x')",
            "SELECT * FROM Enable_Logging()",
            "SELECT * FROM main.\"enable_logging\"()",
            "SET logging_storage = 'file'",
            "SET GLOBAL enable_logging = true",
            "SELECT 1; CALL enable_logging()",
            // The scanner may end the comment elsewhere than DuckDB: judged by its letters.
            "SELECT 1 /* /* */ CALL enable_logging() */",
            // Dynamic SQL names them in a string, whole or in pieces.
            "SELECT * FROM query('SELECT * FROM enable_logging(storage := ''file'', \
             storage_path := ''Z:/nope'')')",
            "SELECT * FROM query('SELECT * FROM enable_' || 'logging()')",
            "FROM Query_Table('enable_' || 'logging')",
            "SELECT * FROM main.\"query\" ('CALL enable_logging()')",
            "SELECT json_execute_serialized_sql(json_serialize_sql('SELECT * FROM \
             enable_logging()'))",
            "SELECT * FROM query('SET logging_' || 'storage = ''file''')",
        ] {
            assert!(switches_logger(sql), "{sql}");
        }
        for sql in [
            "SELECT 'enable_logging'",
            "SELECT 1 -- enable_logging",
            "SELECT * FROM duckdb_logs",
            "CALL disable_logging()",
            "SELECT enable_logging_x FROM t",
            "SELECT * FROM query('SELECT 42')",
            "SELECT query FROM t WHERE note = 'logging'",
        ] {
            assert!(!switches_logger(sql), "{sql}");
        }
    }

    #[test]
    fn pivot_aggregates_read_where_a_table_goes() {
        for sql in [
            "SELECT * FROM t PIVOT (sum(amount) FOR month IN ('jan', 'feb'))",
            "FROM t PIVOT (count(*) FOR m IN (1, 2) GROUP BY region)",
            "SELECT * FROM t PIVOT (avg(x) AS a, max(x) AS m FOR y IN (1))",
        ] {
            assert_eq!(classify_duckdb(sql), StatementClass::Read, "{sql}");
        }
        for sql in [
            "SELECT * FROM t PIVOT (my_agg(amount) FOR month IN ('jan'))",
            "SELECT * FROM main.sum(1)",
            "SELECT * FROM t PIVOT (sum(x) FOR y IN (1)), read_csv('x.csv')",
            // Where a table goes, outside a pivot's `(… FOR …)`, the name is a table macro.
            "SELECT * FROM sum('x.csv')",
            "FROM first(1)",
            "SELECT * FROM t, list(1)",
        ] {
            assert_eq!(classify_duckdb(sql), StatementClass::Write, "{sql}");
        }
    }

    /// `reads_files` on the cases the mock's `readsFiles` runs too (`plugin-files.json`).
    #[test]
    fn plugin_runs_reach_no_files_on_the_shared_cases() {
        let cases: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/duckdb/plugin-files.json"
        ))
        .expect("the plugin cases");
        let list = |key: &str| -> Vec<String> {
            cases[key]
                .as_array()
                .expect("a list")
                .iter()
                .map(|v| v.as_str().expect("a text").to_string())
                .collect()
        };
        let (refused, admitted) = (list("refused"), list("admitted"));
        assert!(refused.len() > 50 && admitted.len() > 25);
        for sql in &refused {
            assert!(reads_files(sql), "refused: {sql}");
        }
        for sql in &admitted {
            assert!(!reads_files(sql), "admitted: {sql}");
        }
    }

    /// A word that ends a FROM list must be one no alias can be, or `FROM t AS <word>, 'x.csv'`
    /// would close the list before the file.
    #[test]
    fn from_lists_end_at_reserved_words_only() {
        for w in FROM_LIST_ENDS {
            let lower = w.to_ascii_lowercase();
            assert!(
                RESERVED_WORDS.split_ascii_whitespace().any(|r| r == lower),
                "{w}"
            );
        }
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
        let redact = |sql: &str| crate::sqlgen::portable::redact::redact_spec(&SPEC, sql);
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
            // A named argument, and a key passed by position.
            (
                "SELECT * FROM quack_serve('quack:h', token := 'qt2')",
                "qt2",
            ),
            ("CALL quack_serve('quack:h', token => 'qt3')", "qt3"),
            (
                "PRAGMA add_parquet_key('k1', 'AAAAAAAAAAAAAAAA')",
                "AAAAAAAAAAAAAAAA",
            ),
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
