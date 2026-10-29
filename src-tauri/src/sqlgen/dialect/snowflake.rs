//! Snowflake SQL (the snowflake lane). Built with `drv-snowflake` only.
//!
//! Double-quoted names (unquoted ones fold to upper case, so the driver always quotes with the
//! case `SHOW` returns); single-quoted strings that always read backslash escapes (there is no
//! setting that turns them off); untagged `$$…$$` bodies (`$name` is a session variable, so `$a$`
//! is the variable `a$`, never a tag); `--`, `//` and flat `/* */` comments; Snowflake Scripting
//! `BEGIN … END` blocks; `?` binds. Every DDL statement commits an open transaction.
//!
//! Read-only is ClassifierOnly (Snowflake has no read-only session), so the rules are an
//! allow-list and default-deny. Beyond the statement words, a read turns into a write when it
//! calls something that changes the account or the session, reads staged files or reaches
//! another service (`write_calls`, swept from the whole function reference, see the brief), or
//! reads from a table function outside `table_functions` (a stored procedure can be selected
//! from, `SELECT * FROM TABLE(proc())`, and so can `INFER_SCHEMA` over staged files).
//!
//! The flow operator `->>` chains statements (`SHOW TABLES ->> DELETE FROM t`). The shared
//! classifier judges each statement it chains on its own, so `… ->> GET …` and `… ->> START
//! TRANSACTION` write although `GET` (also a function) and `START` (`CONNECT BY … START WITH`)
//! cannot be in `writes`, and the whole text as one statement too: `SHOW`, `DESCRIBE`, `DESC`
//! and `EXPLAIN` are queries here, not inspections, so a write word anywhere in them makes them
//! writes, and every statement word that writes is in `writes` (also `COMMENT`, `SET`, `USE`,
//! `BEGIN`). An `@` in code names a stage, and every use of one reads staged files (where a
//! table goes, as an argument, in `EXECUTE IMMEDIATE FROM`), so `@` is in `writes` too: a
//! comment or quote opener inside an unquoted stage path (`@s//x`) cannot hide a statement.
//!
//! The statements the driver sends for itself (session setup, `SHOW` introspection, `GET_DDL`,
//! cancels) are built here too.

use std::sync::LazyLock;

use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, reading};
use crate::sqlgen::dialect::spec::{
    CiLike, DialectSpec, ExplainForm, PlaceholderStyle, RedactCall, RedactRule, SqlHooks, SqlKnobs,
    double_quoted, ph_question, single_quoted_backslash,
};
use crate::sqlgen::mutate::cells;
use crate::sqlgen::portable::classify;
use crate::sqlgen::portable::mutate::{build_delete, build_insert, build_update};
use crate::sqlgen::select::BuiltSql;
use crate::sqlgen::split::leading_keywords;
use crate::sqlgen::sql::TableTarget;
use crate::types::mutate::StagedChange;
use crate::types::query::ExplainMode;
use crate::types::schema::{ColumnMeta, ValueKind};

pub static LEX: LexProfile = LexProfile {
    ident_quotes: &[('"', '"')],
    string_quotes: &['\''],
    // Snowflake always reads backslash escapes in single-quoted strings (never in `$$` bodies or
    // quoted names).
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
    // Block comments do not nest; one holding `/*` is a write either way (`Scan::ambiguous`).
    nested_block_comments: false,
    dash_comment_needs_space: false,
    batch_separator: None,
    terminator_optional: false,
    begin_end_bodies: true,
    attribute_calls: false,
};

/// What a `SELECT` may call that changes the account or the session, reads staged files, runs
/// SQL it builds, or reaches another service. Every `SYSTEM$` function counts (the brief's sweep
/// lists the families), and so do the file and stage functions, the Cortex calls that read files,
/// run agents or train models, and the few other functions the reference marks as control.
const WRITE_CALLS: &[&str] = &[
    // System functions: cancels, aborts, waits, settings, privatelink, replication, logging, tags,
    // the task and pipe controls (SYSTEM$CANCEL_ALL_QUERIES, SYSTEM$ABORT_SESSION, SYSTEM$WAIT…).
    "SYSTEM$*",
    // A function or table named by a string, variable or bind: `SELECT IDENTIFIER('f')()` may
    // call any function, a SYSTEM$ one too.
    "IDENTIFIER",
    // Runs the SQL text it is given.
    "TO_QUERY",
    // Staged files: URLs that grant access to them, their paths and locations, FILE values and
    // the FL_ readers of their metadata, schema inference and COPY validation over them, and
    // directory tables.
    "BUILD_SCOPED_FILE_URL",
    "BUILD_STAGE_FILE_URL",
    "GET_ABSOLUTE_PATH",
    "GET_PRESIGNED_URL",
    "GET_RELATIVE_PATH",
    "GET_STAGE_LOCATION",
    "TO_FILE",
    "TRY_TO_FILE",
    "FL_*",
    // `SNOWFLAKE.CORE.GET_PYTHON_PROFILER_OUTPUT(query_id)` returns the report file the profiler
    // wrote to the stage of PYTHON_PROFILER_TARGET_STAGE.
    "GET_PYTHON_PROFILER_OUTPUT",
    "INFER_SCHEMA",
    "VALIDATE",
    "DIRECTORY",
    // Cortex: the AI_ functions take staged files (images, documents, audio), COMPLETE and
    // TRY_COMPLETE too; PARSE_DOCUMENT reads one; FINETUNE creates a model; AGENT_RUN and
    // DATA_AGENT_RUN run an agent whose tools may run SQL or procedures; EXECUTE_AI_EVALUATION
    // starts an evaluation run (no EXECUTE_ function reads).
    "AI_*",
    "COMPLETE",
    "TRY_COMPLETE",
    "PARSE_DOCUMENT",
    "FINETUNE",
    "AGENT_RUN",
    "DATA_AGENT_RUN",
    "EXECUTE_*",
    // Sets a context value of the session.
    "SET_SYS_CONTEXT",
    // Mints a credential.
    "GENERATE_POSTGRES_ACCESS_TOKEN_FOR_USER",
    // Cancels a Snowpark Container Services job (`svc!SPCS_CANCEL_JOB()`).
    "SPCS_CANCEL_JOB",
    // Sets tags on the columns it classifies.
    "ASSOCIATE_SEMANTIC_CATEGORY_TAGS",
    // Resolve packages against the Anaconda channel.
    "GET_ANACONDA_PACKAGES_REPODATA",
    "SHOW_PYTHON_PACKAGES_DEPENDENCIES",
];

/// The table functions a read may use where a table goes. Anything else there is a write: a
/// stored procedure (`FROM TABLE(proc())` runs it), a user table function (it may reach other
/// services), `INFER_SCHEMA`, `VALIDATE` and `DIRECTORY` over staged files. `TABLE` and `VALUES`
/// are here because `FROM TABLE(…)` and `FROM VALUES (…)` read like calls, and the aggregates
/// because `PIVOT(SUM(a) FOR …)` puts one where a table goes.
///
/// Exact names only, never a `PREFIX*`: Snowflake resolves a name no built-in has to the user's
/// procedure or table function in the current schema, so a pattern would let
/// `TABLE(QUERY_HISTORY_PURGE())` run one.
///
/// Only a built-in without a namespace keeps its name from user functions, and only unqualified:
/// a bare entry allows an unqualified call (`s.FLATTEN()` names a schema's object, so the
/// user's). The others reach a user's object of that name when called unqualified or through
/// another schema (`TABLE(myschema.QUERY_HISTORY())`), so they carry their namespace and match
/// the call's last parts: `INFORMATION_SCHEMA.X` with or without a database before it,
/// `SNOWFLAKE.CORE.X` that whole path, `!X` a service's method (`svc!X()`). Quoted, an entry
/// matches only its exact text, and the keywords `TABLE`, `VALUES` and `SEMANTIC_VIEW` never
/// (`portable::classify`).
const TABLE_FUNCTIONS: &[&str] = &[
    "TABLE",
    "VALUES",
    // `FROM SEMANTIC_VIEW(sv DIMENSIONS … METRICS …)` queries a semantic view.
    "SEMANTIC_VIEW",
    "FLATTEN",
    "GENERATOR",
    "SPLIT_TO_TABLE",
    "STRTOK_SPLIT_TO_TABLE",
    "RESULT_SCAN",
    "INFORMATION_SCHEMA.BIND_VALUES",
    "GET_QUERY_OPERATOR_STATS",
    "GET_OBJECT_REFERENCES",
    "SNOWFLAKE.CORE.GET_LINEAGE",
    "SNOWFLAKE.DATA_PRIVACY.CUMULATIVE_PRIVACY_LOSSES",
    "ESTIMATE_HYBRID_TABLE_STORAGE_USAGE",
    "SNOWFLAKE.DATA_PRIVACY.ESTIMATE_REMAINING_DP_AGGREGATES",
    "CORTEX_SEARCH_DATA_SCAN",
    "CORTEX_SEARCH_BATCH",
    "MODEL_MONITOR_DRIFT_METRIC",
    "MODEL_MONITOR_PERFORMANCE_METRIC",
    "MODEL_MONITOR_STAT_METRIC",
    // Methods of a service (`svc!SPCS_GET_LOGS()`).
    "!SPCS_GET_EVENTS",
    "!SPCS_GET_LOGS",
    "!SPCS_GET_METRICS",
    // SNOWFLAKE.LOCAL's AI observability functions.
    "SNOWFLAKE.LOCAL.GET_AI_EVALUATION_DATA",
    "SNOWFLAKE.LOCAL.GET_AI_OBSERVABILITY_EVENTS",
    "SNOWFLAKE.LOCAL.GET_AI_OBSERVABILITY_LOGS",
    "SNOWFLAKE.LOCAL.GET_AI_RECORD_TRACE",
    "SNOWFLAKE.CORE.GET_CONTACTS",
    "SNOWFLAKE.SPCS.GET_JOB_HISTORY",
    "INFORMATION_SCHEMA.GET_TABLE_VERSIONS",
    "INFORMATION_SCHEMA.GET_TABLE_VERSION_REFS",
    // INFORMATION_SCHEMA table functions (history and metadata, no file contents), and three of
    // the SNOWFLAKE database's in their alphabetical places.
    "INFORMATION_SCHEMA.ALERT_HISTORY",
    "INFORMATION_SCHEMA.APPLICATION_CALLBACK_HISTORY",
    "INFORMATION_SCHEMA.APPLICATION_CONFIGURATION_VALUE_HISTORY",
    "INFORMATION_SCHEMA.APPLICATION_SPECIFICATION_STATUS_HISTORY",
    "INFORMATION_SCHEMA.AUTO_REFRESH_REGISTRATION_HISTORY",
    "INFORMATION_SCHEMA.AUTOMATIC_CLUSTERING_HISTORY",
    "INFORMATION_SCHEMA.AVAILABLE_LISTING_REFRESH_HISTORY",
    "INFORMATION_SCHEMA.AVAILABLE_LISTINGS",
    "INFORMATION_SCHEMA.COMPLETE_TASK_GRAPHS",
    "INFORMATION_SCHEMA.COPY_HISTORY",
    "INFORMATION_SCHEMA.CORTEX_SEARCH_REFRESH_HISTORY",
    "INFORMATION_SCHEMA.CURRENT_TASK_GRAPHS",
    "INFORMATION_SCHEMA.DATA_METRIC_FUNCTION_EXPECTATIONS",
    "INFORMATION_SCHEMA.DATA_METRIC_FUNCTION_REFERENCES",
    "SNOWFLAKE.LOCAL.DATA_QUALITY_MONITORING_EXPECTATION_STATUS",
    "SNOWFLAKE.LOCAL.DATA_QUALITY_MONITORING_RESULTS",
    "INFORMATION_SCHEMA.DATA_TRANSFER_HISTORY",
    "INFORMATION_SCHEMA.DATABASE_REFRESH_HISTORY",
    "INFORMATION_SCHEMA.DATABASE_REFRESH_PROGRESS",
    "INFORMATION_SCHEMA.DATABASE_REFRESH_PROGRESS_BY_JOB",
    "INFORMATION_SCHEMA.DATABASE_REPLICATION_USAGE_HISTORY",
    "INFORMATION_SCHEMA.DATABASE_STORAGE_USAGE_HISTORY",
    "INFORMATION_SCHEMA.DBT_PROJECT_EXECUTION_HISTORY",
    "INFORMATION_SCHEMA.DYNAMIC_TABLE_GRAPH_HISTORY",
    "INFORMATION_SCHEMA.DYNAMIC_TABLE_REFRESH_HISTORY",
    "INFORMATION_SCHEMA.DYNAMIC_TABLES",
    "INFORMATION_SCHEMA.EXTERNAL_FUNCTIONS_HISTORY",
    "INFORMATION_SCHEMA.EXTERNAL_TABLE_FILE_REGISTRATION_HISTORY",
    "INFORMATION_SCHEMA.EXTERNAL_TABLE_FILES",
    "INFORMATION_SCHEMA.ICEBERG_TABLE_FILE_REGISTRATION_HISTORY",
    "INFORMATION_SCHEMA.ICEBERG_TABLE_FILES",
    "INFORMATION_SCHEMA.ICEBERG_TABLE_SNAPSHOT_REFRESH_HISTORY",
    "INFORMATION_SCHEMA.LISTING_REFRESH_HISTORY",
    "INFORMATION_SCHEMA.LOGIN_HISTORY",
    "INFORMATION_SCHEMA.LOGIN_HISTORY_BY_USER",
    "INFORMATION_SCHEMA.MATERIALIZED_VIEW_REFRESH_HISTORY",
    "INFORMATION_SCHEMA.NETWORK_RULE_REFERENCES",
    "INFORMATION_SCHEMA.NOTIFICATION_HISTORY",
    "INFORMATION_SCHEMA.ONLINE_FEATURE_TABLE_REFRESH_HISTORY",
    "INFORMATION_SCHEMA.PIPE_USAGE_HISTORY",
    "INFORMATION_SCHEMA.POLICY_REFERENCES",
    "INFORMATION_SCHEMA.QUERY_ACCELERATION_HISTORY",
    "INFORMATION_SCHEMA.QUERY_HISTORY",
    "INFORMATION_SCHEMA.QUERY_HISTORY_BY_SESSION",
    "INFORMATION_SCHEMA.QUERY_HISTORY_BY_USER",
    "INFORMATION_SCHEMA.QUERY_HISTORY_BY_WAREHOUSE",
    "INFORMATION_SCHEMA.REPLICATION_GROUP_DANGLING_REFERENCES",
    "INFORMATION_SCHEMA.REPLICATION_GROUP_LAG_HISTORY",
    "INFORMATION_SCHEMA.REPLICATION_GROUP_REFRESH_HISTORY",
    "INFORMATION_SCHEMA.REPLICATION_GROUP_REFRESH_HISTORY_ALL",
    "INFORMATION_SCHEMA.REPLICATION_GROUP_REFRESH_PROGRESS",
    "INFORMATION_SCHEMA.REPLICATION_GROUP_REFRESH_PROGRESS_ALL",
    "INFORMATION_SCHEMA.REPLICATION_GROUP_REFRESH_PROGRESS_BY_JOB",
    "INFORMATION_SCHEMA.REPLICATION_GROUP_USAGE_HISTORY",
    "INFORMATION_SCHEMA.REPLICATION_USAGE_HISTORY",
    "INFORMATION_SCHEMA.REST_EVENT_HISTORY",
    "INFORMATION_SCHEMA.SEARCH_OPTIMIZATION_HISTORY",
    "INFORMATION_SCHEMA.SEMANTIC_VIEW_MATERIALIZATION_REFRESH_HISTORY",
    "INFORMATION_SCHEMA.SERVERLESS_ALERT_HISTORY",
    "INFORMATION_SCHEMA.SERVERLESS_TASK_HISTORY",
    "INFORMATION_SCHEMA.STAGE_DIRECTORY_FILE_REGISTRATION_HISTORY",
    "INFORMATION_SCHEMA.STAGE_STORAGE_USAGE_HISTORY",
    "INFORMATION_SCHEMA.STORAGE_LIFECYCLE_POLICY_HISTORY",
    "INFORMATION_SCHEMA.TAG_REFERENCES",
    "INFORMATION_SCHEMA.TAG_REFERENCES_ALL_COLUMNS",
    "SNOWFLAKE.ACCOUNT_USAGE.TAG_REFERENCES_WITH_LINEAGE",
    "INFORMATION_SCHEMA.TASK_DEPENDENTS",
    "INFORMATION_SCHEMA.TASK_GRAPH_CHANGES",
    "INFORMATION_SCHEMA.TASK_HISTORY",
    "INFORMATION_SCHEMA.WAREHOUSE_LOAD_HISTORY",
    "INFORMATION_SCHEMA.WAREHOUSE_METERING_HISTORY",
    // PIVOT's aggregates.
    "AVG",
    "COUNT",
    "COUNT_IF",
    "MAX",
    "MIN",
    "SUM",
    "ANY_VALUE",
    "MEDIAN",
    "LISTAGG",
    "ARRAY_AGG",
    "STDDEV",
    "VARIANCE",
];

pub static RULES: ClassifyRules = ClassifyRules {
    // `SHOW`, `DESCRIBE` / `DESC` and `EXPLAIN` read, but `->>` may chain any statement after
    // them, so they are judged like queries (see the module doc). `LIST` / `LS` (which read a
    // stage) and `USE` (which changes the session's role, warehouse or database) write, and so
    // does a stage itself, `@`.
    queries: &["SELECT", "WITH", "SHOW", "DESCRIBE", "DESC", "EXPLAIN"],
    inspections: &[],
    writes: &[
        "INTO", "INSERT", "UPDATE", "DELETE", "MERGE", "CREATE", "DROP", "ALTER", "TRUNCATE",
        "UNDROP", "CALL", "EXECUTE", "COPY", "PUT", "REMOVE", "RM", "LIST", "LS", "GRANT",
        "REVOKE", "USE", "SET", "UNSET", "BEGIN", "COMMIT", "ROLLBACK", "COMMENT", "DECLARE",
        "LET", "NEXTVAL", "@",
    ],
    write_calls: WRITE_CALLS,
    table_functions: Some(TABLE_FUNCTIONS),
};

fn classify_snowflake(sql: &str) -> StatementClass {
    classify::with(&LEX, &RULES, sql)
}

/// A column as text for pattern filters and "Any column"; binary columns have none.
fn text_expr(e: &str, col: &ColumnMeta) -> Option<String> {
    (col.kind != ValueKind::Bytes).then(|| format!("TO_VARCHAR({e})"))
}

/// `TO_BINARY('00ff', 'HEX')`
fn hex_to_binary(hex: &str) -> String {
    format!("TO_BINARY('{hex}', 'HEX')")
}

/// The `VECTOR(INT, 3)` / `VECTOR(FLOAT, 256)` type of a column, rebuilt from its parts so that
/// nothing but the element type and the count reaches the SQL.
pub fn vector_type(data_type: &str) -> Option<String> {
    let upper = data_type.trim().to_ascii_uppercase();
    let inner = upper.strip_prefix("VECTOR")?.trim_start();
    let inner = inner.strip_prefix('(')?.strip_suffix(')')?;
    let (elem, dim) = inner.split_once(',')?;
    let elem = match elem.trim() {
        "INT" | "INTEGER" => "INT",
        "FLOAT" | "FLOAT4" | "FLOAT8" | "DOUBLE" | "REAL" => "FLOAT",
        _ => return None,
    };
    let dim: u32 = dim.trim().parse().ok()?;
    Some(format!("VECTOR({elem}, {dim})"))
}

/// A bind for a column whose values travel as text but whose type text does not cast to:
/// semi-structured values are parsed as JSON, spatial ones as GeoJSON / WKT, vectors cast.
fn param_cast(ph: String, col: &ColumnMeta) -> String {
    let ty = col.data_type.trim().to_ascii_uppercase();
    if ty.starts_with("VECTOR") {
        return match vector_type(&ty) {
            Some(v) => format!("{ph}::{v}"),
            None => ph,
        };
    }
    match col.kind {
        ValueKind::Json => format!("PARSE_JSON({ph})"),
        ValueKind::Geometry if ty.starts_with("GEOMETRY") => format!("TO_GEOMETRY({ph})"),
        ValueKind::Geometry => format!("TO_GEOGRAPHY({ph})"),
        _ => ph,
    }
}

/// Whether a value of the column goes in through a function (`param_cast`): the VALUES clause
/// of an INSERT refuses those (`Invalid expression [PARSE_JSON(?)] in VALUES clause`).
fn needs_select_insert(col: &ColumnMeta) -> bool {
    param_cast("?".into(), col) != "?"
}

/// The portable builders, but an INSERT of a value that needs a function becomes `INSERT INTO t
/// (…) SELECT …`, which Snowflake takes where VALUES does not.
fn build_change(
    spec: &DialectSpec,
    t: &TableTarget<'_>,
    change: &StagedChange,
    returning: bool,
) -> Result<(BuiltSql, Option<u64>), AppError> {
    let built = match change {
        StagedChange::Update {
            key,
            changes,
            exprs,
            ..
        } => build_update(spec, t, key, &cells(changes, exprs.as_ref()), returning)?,
        StagedChange::Insert { values, exprs, .. } => {
            let cells = cells(values, exprs.as_ref());
            let mut built = build_insert(spec, t, &cells, returning)?;
            let casts = cells.keys().any(|name| {
                t.columns
                    .iter()
                    .any(|c| c.name == *name && needs_select_insert(c))
            });
            // The values are placeholders and clock expressions, none of which holds this text,
            // so the last one is the clause the builder wrote.
            if casts
                && built.sql.ends_with(')')
                && let Some(at) = built.sql.rfind(" VALUES (")
            {
                let values = &built.sql[at + " VALUES (".len()..built.sql.len() - 1];
                built.sql = format!("{} SELECT {values}", &built.sql[..at]);
            }
            built
        }
        StagedChange::Delete { key, .. } => build_delete(spec, t, key)?,
    };
    Ok((built, Some(1)))
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
        bytes_literal: hex_to_binary,
        text_expr,
        param_cast: Some(param_cast),
        ci_like: CiLike::Ilike,
        // INSERT takes no `DEFAULT VALUES`, UPDATE no `= DEFAULT`.
        insert_all_defaults: None,
        set_default: false,
        explain,
        implicit_commit,
        catalog_words: &[
            "INFORMATION_SCHEMA",
            "ACCOUNT_USAGE",
            "ORGANIZATION_USAGE",
            "READER_ACCOUNT_USAGE",
            "DATA_SHARING_USAGE",
        ],
        // Reads the classifier lets through that still wait or bill per call (Cortex).
        side_effect_functions: &[
            "SYSTEM$*",
            "SPCS_WAIT_FOR",
            "SENTIMENT",
            "SUMMARIZE",
            "EXTRACT_ANSWER",
            "CLASSIFY_TEXT",
            "EMBED_TEXT_*",
            "ENTITY_SENTIMENT",
            "SEARCH_PREVIEW",
            "SPLIT_TEXT_*",
            // Not the plain string function `TRANSLATE(s, 'abc', 'xyz')`.
            "CORTEX.TRANSLATE",
        ],
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
            RedactRule {
                keyword: "SECRET_STRING",
            },
            RedactRule {
                keyword: "OAUTH_CLIENT_SECRET",
            },
            RedactRule {
                keyword: "OAUTH_REFRESH_TOKEN",
            },
            RedactRule { keyword: "API_KEY" },
            // A rule matches its exact word, so PASSWORD does not cover these.
            RedactRule {
                keyword: "ADMIN_PASSWORD",
            },
            RedactRule {
                keyword: "BEARER_TOKEN",
            },
            RedactRule {
                keyword: "OIDC_CLIENT_SECRET",
            },
        ],
        // A credential passed by position: the token of a privatelink authorization, the
        // passphrase or key of the encryption functions.
        redact_calls: &[
            RedactCall {
                function: "SYSTEM$AUTHORIZE_PRIVATELINK",
                argument: 2,
            },
            RedactCall {
                function: "SYSTEM$REVOKE_PRIVATELINK",
                argument: 2,
            },
            RedactCall {
                function: "ENCRYPT",
                argument: 2,
            },
            RedactCall {
                function: "DECRYPT",
                argument: 2,
            },
            RedactCall {
                function: "TRY_DECRYPT",
                argument: 2,
            },
            RedactCall {
                function: "ENCRYPT_RAW",
                argument: 2,
            },
            RedactCall {
                function: "DECRYPT_RAW",
                argument: 2,
            },
            RedactCall {
                function: "TRY_DECRYPT_RAW",
                argument: 2,
            },
        ],
        ..SqlKnobs::ANSI
    },
    hooks: SqlHooks {
        build_change: Some(build_change),
        ..SqlHooks::NONE
    },
    ddl: None,
};

// --- The driver's own statements -----------------------------------------------------------------

/// The session's state, without a warehouse (`ping` must never resume one).
pub const SERVER_INFO: &str = "SELECT CURRENT_VERSION(), CURRENT_ACCOUNT(), CURRENT_REGION(), \
     CURRENT_USER(), CURRENT_ROLE(), CURRENT_WAREHOUSE(), CURRENT_DATABASE(), CURRENT_SCHEMA()";
/// Runs in the cloud services layer: no warehouse resumes for it.
pub const PING: &str = "SELECT 1";
pub const SHOW_DATABASES: &str = "SHOW DATABASES";
pub const SHOW_WAREHOUSES: &str = "SHOW WAREHOUSES";
pub const SHOW_ROLES: &str = "SHOW ROLES";

/// SHOW returns at most this many rows; later pages start after the last name (`LIMIT … FROM`).
pub const SHOW_PAGE: u32 = 10_000;

/// A session parameter's name (`TIMEZONE`, `QUERY_TAG`): a bare word, since parameters are no
/// quotable names.
pub fn parameter_name(name: &str) -> Result<String, AppError> {
    let upper = name.trim().to_ascii_uppercase();
    let ok = upper.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && upper.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok {
        Ok(upper)
    } else {
        Err(AppError::InvalidInput(format!(
            "\u{201c}{name}\u{201d} is no Snowflake session parameter name"
        )))
    }
}

/// A session parameter's value as SQL: text quoted, numbers and booleans as they are.
pub fn parameter_value(value: &serde_json::Value) -> Result<String, AppError> {
    match value {
        serde_json::Value::String(s) => Ok(single_quoted_backslash(s)),
        serde_json::Value::Bool(b) => Ok(if *b { "TRUE" } else { "FALSE" }.into()),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        other => Err(AppError::InvalidInput(format!(
            "a Snowflake session parameter takes text, a number or a boolean, not {other}"
        ))),
    }
}

/// `ALTER SESSION SET NAME = value`
pub fn alter_session_set(name: &str, value: &serde_json::Value) -> Result<String, AppError> {
    Ok(format!(
        "ALTER SESSION SET {} = {}",
        parameter_name(name)?,
        parameter_value(value)?
    ))
}

/// What `USE` switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseKind {
    Warehouse,
    Role,
    Database,
    Schema,
}

/// `USE WAREHOUSE "w"`, `USE ROLE "r"`, `USE DATABASE "d"`, `USE SCHEMA "d"."s"` (the database
/// first when given).
pub fn use_object(kind: UseKind, database: Option<&str>, name: &str) -> String {
    let what = match kind {
        UseKind::Warehouse => "WAREHOUSE",
        UseKind::Role => "ROLE",
        UseKind::Database => "DATABASE",
        UseKind::Schema => "SCHEMA",
    };
    match (kind, database) {
        (UseKind::Schema, Some(db)) => {
            format!("USE SCHEMA {}.{}", double_quoted(db), double_quoted(name))
        }
        _ => format!("USE {what} {}", double_quoted(name)),
    }
}

/// `"DB"."S"` or `"DB"."S"."T"`.
pub fn qualified(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|p| double_quoted(p))
        .collect::<Vec<_>>()
        .join(".")
}

/// ` LIMIT 10000 FROM 'last'` after the first page of a SHOW.
fn page(after: Option<&str>) -> String {
    match after {
        Some(last) => format!(" LIMIT {SHOW_PAGE} FROM {}", single_quoted_backslash(last)),
        None => String::new(),
    }
}

/// What a SHOW in a schema lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaObjects {
    Tables,
    Views,
    MaterializedViews,
    ExternalTables,
    DynamicTables,
    UserFunctions,
    Procedures,
    Sequences,
    Stages,
    Pipes,
    Streams,
    Tasks,
    /// Every column of the schema (completion).
    Columns,
}

impl SchemaObjects {
    fn words(self) -> &'static str {
        match self {
            SchemaObjects::Tables => "TABLES",
            SchemaObjects::Views => "VIEWS",
            SchemaObjects::MaterializedViews => "MATERIALIZED VIEWS",
            SchemaObjects::ExternalTables => "EXTERNAL TABLES",
            SchemaObjects::DynamicTables => "DYNAMIC TABLES",
            SchemaObjects::UserFunctions => "USER FUNCTIONS",
            SchemaObjects::Procedures => "PROCEDURES",
            SchemaObjects::Sequences => "SEQUENCES",
            SchemaObjects::Stages => "STAGES",
            SchemaObjects::Pipes => "PIPES",
            SchemaObjects::Streams => "STREAMS",
            SchemaObjects::Tasks => "TASKS",
            SchemaObjects::Columns => "COLUMNS",
        }
    }
}

/// `SHOW SCHEMAS IN DATABASE "DB"` (a later page starts after `after`).
pub fn show_schemas(database: &str, after: Option<&str>) -> String {
    format!(
        "SHOW SCHEMAS IN DATABASE {}{}",
        double_quoted(database),
        page(after)
    )
}

/// `SHOW TABLES IN SCHEMA "DB"."S"` and the other object lists of a schema.
pub fn show_in_schema(
    what: SchemaObjects,
    database: &str,
    schema: &str,
    after: Option<&str>,
) -> String {
    format!(
        "SHOW {} IN SCHEMA {}{}",
        what.words(),
        qualified(&[database, schema]),
        page(after)
    )
}

/// What a SHOW on one table lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableObjects {
    Columns,
    PrimaryKeys,
    UniqueKeys,
    /// The table's foreign keys (the keys it imports).
    ImportedKeys,
    /// Indexes of a hybrid table.
    Indexes,
}

/// `SHOW COLUMNS IN TABLE "DB"."S"."T"`, its keys and (hybrid tables) indexes.
pub fn show_in_table(what: TableObjects, database: &str, schema: &str, table: &str) -> String {
    let words = match what {
        TableObjects::Columns => "COLUMNS",
        TableObjects::PrimaryKeys => "PRIMARY KEYS",
        TableObjects::UniqueKeys => "UNIQUE KEYS",
        TableObjects::ImportedKeys => "IMPORTED KEYS",
        TableObjects::Indexes => "INDEXES",
    };
    format!(
        "SHOW {words} IN TABLE {}",
        qualified(&[database, schema, table])
    )
}

/// The object kinds `GET_DDL` takes that the driver asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DdlKind {
    Table,
    View,
    Sequence,
    Function,
    Procedure,
    Schema,
    Database,
}

/// `SELECT GET_DDL('TABLE', '"DB"."S"."T"')`: the CREATE text of an object (a function or
/// procedure needs its argument types after the name, `'"DB"."S"."F"(NUMBER)'`).
pub fn get_ddl(kind: DdlKind, name: &str) -> String {
    let kind = match kind {
        DdlKind::Table => "TABLE",
        DdlKind::View => "VIEW",
        DdlKind::Sequence => "SEQUENCE",
        DdlKind::Function => "FUNCTION",
        DdlKind::Procedure => "PROCEDURE",
        DdlKind::Schema => "SCHEMA",
        DdlKind::Database => "DATABASE",
    };
    format!(
        "SELECT GET_DDL('{kind}', {})",
        single_quoted_backslash(name)
    )
}

/// Stops another query of the user (Kill Query of the process list).
pub fn cancel_query(query_id: &str) -> String {
    format!(
        "SELECT SYSTEM$CANCEL_QUERY({})",
        single_quoted_backslash(query_id)
    )
}

/// The running and queued queries of the user (the process list; needs a warehouse).
pub const RUNNING_QUERIES: &str = "SELECT query_id, session_id, user_name, warehouse_name, \
     execution_status, start_time, query_text FROM TABLE(INFORMATION_SCHEMA.QUERY_HISTORY_BY_USER(\
     RESULT_LIMIT => 1000)) WHERE execution_status IN ('RUNNING', 'QUEUED', 'BLOCKED', \
     'RESUMING_WAREHOUSE') ORDER BY start_time";

/// How many rows a table holds, from its metadata (no warehouse): the `rows` column of `SHOW
/// TABLES LIKE 'name' IN SCHEMA …`. LIKE takes a pattern, so `_` and `%` are escaped.
pub fn show_table_like(database: &str, schema: &str, table: &str) -> String {
    let pattern: String = table
        .chars()
        .flat_map(|c| match c {
            '\\' | '_' | '%' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    format!(
        "SHOW TABLES LIKE {} IN SCHEMA {}",
        single_quoted_backslash(&pattern),
        qualified(&[database, schema])
    )
}

/// Every statement above, for the tests that check how they classify.
pub static DRIVER_READS: LazyLock<Vec<String>> = LazyLock::new(|| {
    vec![
        SERVER_INFO.to_string(),
        PING.to_string(),
        SHOW_DATABASES.to_string(),
        SHOW_WAREHOUSES.to_string(),
        SHOW_ROLES.to_string(),
        show_schemas("DB", Some("LAST")),
        show_in_schema(SchemaObjects::Tables, "DB", "S", None),
        show_in_table(TableObjects::ImportedKeys, "DB", "S", "T"),
        get_ddl(DdlKind::Table, &qualified(&["DB", "S", "T"])),
        RUNNING_QUERIES.to_string(),
        show_table_like("DB", "S", "T_1"),
    ]
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlgen::portable::testing::col;
    use crate::types::browse::RowKey;
    use crate::types::schema::TableRef;
    use crate::types::value::Value;
    use std::collections::BTreeMap;

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
        assert!(!implicit_commit("insert into t values (1)"));
    }

    #[test]
    fn the_flow_operator_cannot_hide_a_write() {
        for sql in [
            "SHOW TABLES ->> DELETE FROM t",
            "SHOW TABLES ->> SELECT * FROM $1 ->> UPDATE t SET a = 1",
            "DESCRIBE TABLE t ->> INSERT INTO u SELECT * FROM $1",
            "DESC TABLE t ->> DROP TABLE t",
            "EXPLAIN SELECT 1 ->> CALL p()",
            "SELECT 1 ->> GRANT ROLE r TO USER u",
            "SELECT 1 ->> COMMENT ON TABLE t IS 'x'",
            "SELECT 1 ->> USE ROLE ACCOUNTADMIN",
            "SELECT 1 ->> SET x = 1",
            "SELECT 1 ->> UNSET x",
            "SELECT 1 ->> COMMIT",
            "SELECT 1 ->> BEGIN",
            "SELECT 1 ->> PUT file:///tmp/x @s",
            "SELECT 1 ->> REMOVE @s",
            "SELECT 1 ->> LIST @s",
            "SELECT 1 ->> UNDROP TABLE t",
            "SHOW TABLES ->> TRUNCATE TABLE t",
            "SHOW TABLES ->> EXECUTE TASK t",
        ] {
            assert_eq!(classify_snowflake(sql), StatementClass::Write, "{sql}");
        }
        assert_eq!(
            classify_snowflake("SHOW TABLES ->> SELECT \"name\" FROM $1 WHERE \"rows\" > 0"),
            StatementClass::Read
        );
    }

    #[test]
    fn table_functions_outside_the_allow_list_write() {
        for sql in [
            "SELECT * FROM TABLE(my_proc(1))",
            "SELECT * FROM TABLE(INFER_SCHEMA(LOCATION => '@s', FILE_FORMAT => 'f'))",
            "SELECT * FROM DIRECTORY(@s)",
            "SELECT * FROM TABLE(TO_QUERY('SELECT 1'))",
            "SELECT * FROM TABLE(db.s.udtf(1)) x",
            "SELECT * FROM t, TABLE(udtf(t.a))",
            "SELECT * FROM t JOIN TABLE(VALIDATE(t, JOB_ID => '_last')) v ON true",
            "SELECT * FROM IDENTIFIER('t')",
            "SELECT IDENTIFIER('SYSTEM$CANCEL_ALL_QUERIES')(1)",
            // No built-in has these names, so they are the user's.
            "SELECT * FROM TABLE(QUERY_HISTORY_PURGE())",
            "SELECT * FROM TABLE(INFORMATION_SCHEMA.REPLICATION_GROUP_X())",
            "SELECT * FROM TABLE(THREAD_MESSAGES(1))",
            // LIMIT as an alias after AS, or before a comma, keeps the FROM list open.
            "SELECT * FROM t1 AS limit (a, b), my_udtf(t1.a)",
            "SELECT * FROM t1 limit, my_udtf(t1.a)",
            // PUBLIC's call is checked, named with its schema or through `db..`.
            "SELECT * FROM TABLE(db.public.my_proc())",
            "SELECT * FROM t PIVOT (my_agg(amount) FOR month IN ('JAN')) AS p",
        ] {
            assert_eq!(classify_snowflake(sql), StatementClass::Write, "{sql}");
        }
        // A pattern admits names no built-in has (see TABLE_FUNCTIONS).
        assert!(TABLE_FUNCTIONS.iter().all(|f| !f.contains('*')));
        for sql in [
            "SELECT * FROM TABLE(FLATTEN(input => PARSE_JSON('[1]')))",
            "SELECT f.value FROM t, LATERAL FLATTEN(input => t.v) f",
            "SELECT * FROM TABLE(RESULT_SCAN(LAST_QUERY_ID()))",
            "SELECT * FROM TABLE(INFORMATION_SCHEMA.QUERY_HISTORY_BY_SESSION())",
            "SELECT * FROM TABLE(db.INFORMATION_SCHEMA.TASK_HISTORY(RESULT_LIMIT => 10))",
            "SELECT SEQ4() FROM TABLE(GENERATOR(ROWCOUNT => 10))",
            "SELECT * FROM VALUES (1, 'a'), (2, 'b') AS v (n, s)",
            "SELECT * FROM t PIVOT (SUM(amount) FOR month IN ('JAN', 'FEB')) AS p",
            "SELECT * FROM t UNPIVOT (sales FOR month IN (jan, feb))",
            "SELECT * FROM TABLE(SPLIT_TO_TABLE('a,b', ','))",
        ] {
            assert_eq!(classify_snowflake(sql), StatementClass::Read, "{sql}");
        }
    }

    /// The hole the rules cannot close, under Limits in the brief: it reads today; when it closes
    /// and the case turns into a write, move it to the suite's WRITES. To verify on a real
    /// account: whether a quoted NEXTVAL advances the sequence.
    #[test]
    fn documented_holes_read_until_their_change_requests() {
        let sql = "SELECT seq.\"NEXTVAL\"";
        assert_eq!(
            classify_snowflake(sql),
            StatementClass::Read,
            "{sql} writes now: move it to the suite's WRITES and update the brief's Limits"
        );
    }

    /// Every secret parameter of the CREATE / ALTER syntax keeps its literal out of the trace.
    #[test]
    fn redacts_every_secret_parameter() {
        let redact = |sql: &str| crate::sqlgen::portable::redact::redact_spec(&SPEC, sql);
        let mask = crate::sqlgen::admin::PASSWORD_MASK;
        for (sql, secret) in [
            ("ALTER USER u SET PASSWORD = 'pw1'", "pw1"),
            (
                "CREATE USER u PASSWORD = 'pw2' MUST_CHANGE_PASSWORD = TRUE",
                "pw2",
            ),
            (
                "CREATE SECRET s TYPE = PASSWORD USERNAME = 'u' PASSWORD = 'pw3'",
                "pw3",
            ),
            (
                "CREATE ACCOUNT a ADMIN_NAME = admin ADMIN_PASSWORD = 'apw1' \
                 EMAIL = 'a@b.c' EDITION = ENTERPRISE",
                "apw1",
            ),
            (
                "CREATE ORGANIZATION ACCOUNT oa ADMIN_NAME = admin ADMIN_PASSWORD = 'apw2' \
                 EMAIL = 'a@b.c' EDITION = ENTERPRISE",
                "apw2",
            ),
            (
                "CREATE MANAGED ACCOUNT r ADMIN_NAME = u, ADMIN_PASSWORD = 'apw3', TYPE = READER",
                "apw3",
            ),
            (
                "CREATE CATALOG INTEGRATION c CATALOG_SOURCE = ICEBERG_REST \
                 TABLE_FORMAT = ICEBERG REST_CONFIG = (CATALOG_URI = 'https://x') \
                 REST_AUTHENTICATION = (TYPE = BEARER BEARER_TOKEN = 'tok1') ENABLED = TRUE",
                "tok1",
            ),
            (
                "ALTER CATALOG INTEGRATION c SET REST_AUTHENTICATION = (BEARER_TOKEN = 'tok2')",
                "tok2",
            ),
            (
                "CREATE CATALOG INTEGRATION d CATALOG_SOURCE = DELTA_SHARING \
                 REST_AUTHENTICATION = (TYPE = BEARER BEARER_TOKEN = 'tok3')",
                "tok3",
            ),
            (
                "CREATE SECURITY INTEGRATION o TYPE = OIDC OIDC_CLIENT_ID = 'id' \
                 OIDC_CLIENT_SECRET = 'cs1' ENABLED = TRUE",
                "cs1",
            ),
            (
                "ALTER SECURITY INTEGRATION o SET OIDC_CLIENT_SECRET = 'cs2'",
                "cs2",
            ),
            (
                "CREATE CATALOG INTEGRATION c REST_AUTHENTICATION = (TYPE = OAUTH \
                 OAUTH_CLIENT_ID = 'id' OAUTH_CLIENT_SECRET = 'cs3' OAUTH_ALLOWED_SCOPES = ('a'))",
                "cs3",
            ),
            (
                "CREATE SECRET s TYPE = OAUTH2 OAUTH_REFRESH_TOKEN = 'rt' \
                 OAUTH_REFRESH_TOKEN_EXPIRY_TIME = '2030-01-01' API_AUTHENTICATION = i",
                "rt",
            ),
            (
                "CREATE SECRET s TYPE = GENERIC_STRING SECRET_STRING = 'gs'",
                "gs",
            ),
            (
                "CREATE API INTEGRATION a API_PROVIDER = git_https_api API_KEY = 'ak'",
                "ak",
            ),
            (
                "CREATE STAGE st URL = 's3://b/' CREDENTIALS = (AWS_KEY_ID = 'id' \
                 AWS_SECRET_KEY = 'sk' AWS_TOKEN = 'st') \
                 ENCRYPTION = (TYPE = 'AWS_CSE' MASTER_KEY = 'mk')",
                "sk",
            ),
            (
                "COPY INTO t FROM 's3://b/' CREDENTIALS = (AWS_KEY_ID = 'id' \
                 AWS_SECRET_KEY = 'sk2' AWS_TOKEN = 'st2')",
                "st2",
            ),
            (
                "CREATE STAGE st URL = 'azure://a/c' CREDENTIALS = (AZURE_SAS_TOKEN = 'sas') \
                 ENCRYPTION = (TYPE = 'AZURE_CSE' MASTER_KEY = 'mk2')",
                "sas",
            ),
            (
                "CREATE STAGE st URL = 'azure://a/c' ENCRYPTION = (TYPE = 'AZURE_CSE' \
                 MASTER_KEY = 'mk2')",
                "mk2",
            ),
            // Lower case, a backslash escape and a `$$` body are masked whole.
            (
                "create account a admin_name = x admin_password = 'a\\'b' email = 'e' \
                 edition = standard",
                "a\\'b",
            ),
            ("ALTER USER u SET PASSWORD = $$p'w$$", "p'w"),
        ] {
            let r = redact(sql);
            assert!(!r.sql.contains(secret), "{sql} -> {}", r.sql);
            assert!(r.sql.contains(mask), "{sql} -> {}", r.sql);
            assert!(!r.secrets.is_empty(), "{sql}");
        }
        assert_eq!(
            redact("CREATE SECURITY INTEGRATION o TYPE = OIDC OIDC_CLIENT_SECRET = 'x'").sql,
            format!("CREATE SECURITY INTEGRATION o TYPE = OIDC OIDC_CLIENT_SECRET = {mask}")
        );
        assert_eq!(
            redact("create account a admin_name = x admin_password = 'a\\'b' email = 'e'").sql,
            format!("create account a admin_name = x admin_password = {mask} email = 'e'")
        );
        // Settings and identifiers next to them stay readable.
        for sql in [
            "ALTER USER u SET MUST_CHANGE_PASSWORD = TRUE",
            "CREATE PASSWORD POLICY p PASSWORD_MIN_LENGTH = 14",
            "CREATE SECURITY INTEGRATION s TYPE = SCIM SCIM_CLIENT = 'OKTA' SYNC_PASSWORD = TRUE",
            "ALTER USER u SET RSA_PUBLIC_KEY = 'MIIBIjANBg'",
            "SELECT * FROM t WHERE token = 'x'",
            "SELECT ENCRYPT(v, k), TRY_DECRYPT('ciphertext')",
        ] {
            assert_eq!(redact(sql).sql, sql, "{sql}");
        }
    }

    /// A credential passed to a function by position is masked too, and only that argument.
    #[test]
    fn redacts_the_secret_arguments_of_functions() {
        let redact = |sql: &str| crate::sqlgen::portable::redact::redact_spec(&SPEC, sql);
        let mask = crate::sqlgen::admin::PASSWORD_MASK;
        for (sql, masked) in [
            (
                "SELECT SYSTEM$AUTHORIZE_PRIVATELINK('185...', 'eyJr0k3n')",
                format!("SELECT SYSTEM$AUTHORIZE_PRIVATELINK('185...', {mask})"),
            ),
            (
                "select system$revoke_privatelink('sub-id', 'tok')",
                format!("select system$revoke_privatelink('sub-id', {mask})"),
            ),
            (
                "SELECT ENCRYPT('value', 'pass\\'phrase'), DECRYPT(c, 'pp') FROM t",
                format!("SELECT ENCRYPT('value', {mask}), DECRYPT(c, {mask}) FROM t"),
            ),
            (
                "SELECT TRY_DECRYPT(c, 'pp', 'aad', 'AES-GCM')",
                format!("SELECT TRY_DECRYPT(c, {mask}, 'aad', 'AES-GCM')"),
            ),
            (
                "SELECT ENCRYPT_RAW(v, TO_BINARY('k', 'HEX'), iv), DECRYPT_RAW(v, 'k2', iv), \
                 TRY_DECRYPT_RAW(v, 'k3', iv)",
                format!(
                    "SELECT ENCRYPT_RAW(v, TO_BINARY({mask}, {mask}), iv), DECRYPT_RAW(v, {mask}, \
                     iv), TRY_DECRYPT_RAW(v, {mask}, iv)"
                ),
            ),
        ] {
            assert_eq!(redact(sql).sql, masked, "{sql}");
        }
    }

    /// The Metrics Board refuses Cortex's TRANSLATE, which bills per call, but not the plain
    /// string function of that name.
    #[test]
    fn cortex_translate_is_a_side_effect_by_its_schema() {
        use crate::sqlgen::portable::classify::calls_listed_function;
        let side = |sql| calls_listed_function(&LEX, SPEC.knobs.side_effect_functions, sql);
        assert!(side(
            "SELECT SNOWFLAKE.CORTEX.TRANSLATE(body, 'de', 'en') FROM docs"
        ));
        assert!(side("SELECT cortex.translate(body, '', 'en') FROM docs"));
        assert!(!side("SELECT TRANSLATE(s, 'abc', 'xyz') FROM t"));
    }

    #[test]
    fn builds_the_drivers_statements() {
        assert_eq!(
            alter_session_set("timezone", &serde_json::json!("Asia/Jakarta")).unwrap(),
            "ALTER SESSION SET TIMEZONE = 'Asia/Jakarta'"
        );
        assert_eq!(
            alter_session_set("QUERY_TAG", &serde_json::json!("it's \\ x")).unwrap(),
            "ALTER SESSION SET QUERY_TAG = 'it\\'s \\\\ x'"
        );
        assert_eq!(
            alter_session_set("STATEMENT_TIMEOUT_IN_SECONDS", &serde_json::json!(60)).unwrap(),
            "ALTER SESSION SET STATEMENT_TIMEOUT_IN_SECONDS = 60"
        );
        assert_eq!(
            alter_session_set("ABORT_DETACHED_QUERY", &serde_json::json!(true)).unwrap(),
            "ALTER SESSION SET ABORT_DETACHED_QUERY = TRUE"
        );
        for bad in ["TIMEZONE = 'x'; DROP", "a b", "", "1x", "x-y"] {
            assert!(parameter_name(bad).is_err(), "{bad}");
        }
        assert!(alter_session_set("X", &serde_json::json!([1])).is_err());
        assert_eq!(
            use_object(UseKind::Warehouse, None, "compute_wh"),
            "USE WAREHOUSE \"compute_wh\""
        );
        assert_eq!(
            use_object(UseKind::Schema, Some("My DB"), "s\"x"),
            "USE SCHEMA \"My DB\".\"s\"\"x\""
        );
        assert_eq!(
            use_object(UseKind::Role, None, "ANALYST"),
            "USE ROLE \"ANALYST\""
        );
        assert_eq!(show_schemas("DB", None), "SHOW SCHEMAS IN DATABASE \"DB\"");
        assert_eq!(
            show_in_schema(SchemaObjects::Tables, "DB", "S", Some("it's")),
            "SHOW TABLES IN SCHEMA \"DB\".\"S\" LIMIT 10000 FROM 'it\\'s'"
        );
        assert_eq!(
            show_in_schema(SchemaObjects::MaterializedViews, "d", "s", None),
            "SHOW MATERIALIZED VIEWS IN SCHEMA \"d\".\"s\""
        );
        assert_eq!(
            show_in_table(TableObjects::ImportedKeys, "D", "S", "T\"1"),
            "SHOW IMPORTED KEYS IN TABLE \"D\".\"S\".\"T\"\"1\""
        );
        assert_eq!(
            get_ddl(DdlKind::Table, &qualified(&["D", "S", "it's"])),
            "SELECT GET_DDL('TABLE', '\"D\".\"S\".\"it\\'s\"')"
        );
        assert_eq!(
            cancel_query("01b2-x'"),
            "SELECT SYSTEM$CANCEL_QUERY('01b2-x\\'')"
        );
        assert_eq!(
            show_table_like("D", "S", "a_b%c\\"),
            "SHOW TABLES LIKE 'a\\\\_b\\\\%c\\\\\\\\' IN SCHEMA \"D\".\"S\""
        );
        // The driver's own reads are reads; its session changes and cancels are writes.
        for sql in DRIVER_READS.iter() {
            assert_eq!(classify_snowflake(sql), StatementClass::Read, "{sql}");
        }
        for sql in [
            use_object(UseKind::Database, None, "D"),
            alter_session_set("TIMEZONE", &serde_json::json!("UTC")).unwrap(),
            cancel_query("q"),
        ] {
            assert_eq!(classify_snowflake(&sql), StatementClass::Write, "{sql}");
        }
    }

    #[test]
    fn values_that_need_a_function_bind_through_it() {
        let v = col("v", "VARIANT", ValueKind::Json);
        let g = col("g", "GEOGRAPHY", ValueKind::Geometry);
        let m = col("m", "GEOMETRY", ValueKind::Geometry);
        let e = col("e", "VECTOR(FLOAT, 3)", ValueKind::Array);
        let bad = col("b", "VECTOR(INT, 3); DROP", ValueKind::Array);
        let n = col("n", "NUMBER(38,0)", ValueKind::Int);
        assert_eq!(param_cast("?".into(), &v), "PARSE_JSON(?)");
        assert_eq!(param_cast("?".into(), &g), "TO_GEOGRAPHY(?)");
        assert_eq!(param_cast("?".into(), &m), "TO_GEOMETRY(?)");
        assert_eq!(param_cast("?".into(), &e), "?::VECTOR(FLOAT, 3)");
        assert_eq!(param_cast("?".into(), &bad), "?");
        assert_eq!(param_cast("?".into(), &n), "?");
        assert_eq!(
            vector_type("vector(int,16)").as_deref(),
            Some("VECTOR(INT, 16)")
        );
        assert_eq!(vector_type("VECTOR(STRING, 2)"), None);
        assert_eq!(text_expr("\"n\"", &n).as_deref(), Some("TO_VARCHAR(\"n\")"));
        assert_eq!(
            text_expr("\"b\"", &col("b", "BINARY", ValueKind::Bytes)),
            None
        );
        assert_eq!(hex_to_binary("00ff"), "TO_BINARY('00ff', 'HEX')");
    }

    #[test]
    fn inserts_of_json_values_select_them() {
        let mut id = col("id", "NUMBER(38,0)", ValueKind::Int);
        id.is_pk = true;
        id.pk_ordinal = Some(1);
        let columns = vec![id, col("doc", "VARIANT", ValueKind::Json)];
        let table = TableRef::new(Some("S"), "T");
        let t = TableTarget {
            table: &table,
            columns: &columns,
        };
        let mut values = BTreeMap::new();
        values.insert("id".to_string(), Value::Int(1));
        values.insert("doc".to_string(), Value::Json("{\"a\":1}".into()));
        let insert = StagedChange::Insert {
            row_id: "n1".into(),
            values: values.clone(),
            exprs: None,
        };
        let (built, expect) = build_change(&SPEC, &t, &insert, false).unwrap();
        assert_eq!(
            built.sql,
            "INSERT INTO \"S\".\"T\" (\"doc\", \"id\") SELECT PARSE_JSON(?), ?"
        );
        assert_eq!(expect, Some(1));
        assert_eq!(built.params.len(), 2);
        // Without such a column the VALUES form stays.
        values.remove("doc");
        let plain = StagedChange::Insert {
            row_id: "n2".into(),
            values,
            exprs: None,
        };
        let (built, _) = build_change(&SPEC, &t, &plain, false).unwrap();
        assert_eq!(built.sql, "INSERT INTO \"S\".\"T\" (\"id\") VALUES (?)");
        let mut key = BTreeMap::new();
        key.insert("id".to_string(), Value::Int(1));
        let mut changes = BTreeMap::new();
        changes.insert("doc".to_string(), Value::Json("[]".into()));
        let update = StagedChange::Update {
            row_id: "r1".into(),
            key: RowKey::Pk(key.clone()),
            changes,
            exprs: None,
        };
        let (built, _) = build_change(&SPEC, &t, &update, false).unwrap();
        assert_eq!(
            built.sql,
            "UPDATE \"S\".\"T\" SET \"doc\" = PARSE_JSON(?) WHERE \"id\" = ?"
        );
        let delete = StagedChange::Delete {
            row_id: "r1".into(),
            key: RowKey::Pk(key),
        };
        let (built, _) = build_change(&SPEC, &t, &delete, false).unwrap();
        assert_eq!(built.sql, "DELETE FROM \"S\".\"T\" WHERE \"id\" = ?");
    }
}
