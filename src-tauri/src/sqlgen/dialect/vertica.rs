//! Vertica SQL (the vertica lane). Built with `drv-vertica` only.
//!
//! PostgreSQL-like text: double-quoted names, `''` in strings, which read backslashes as escapes
//! only with `StandardConformingStrings` off (a session setting, so the classifier reads both
//! ways), `E'…'` and `$$…$$` strings, `?` parameters. No RETURNING and no row id, so only tables
//! with a primary key are editable, and DDL commits on its own.
//!
//! Vertica's meta-functions change the cluster from a plain `SELECT` (`SELECT CLOSE_SESSION(…)`,
//! `SELECT DROP_PARTITIONS(…)`), and so do its machine-learning trainers (`SELECT
//! LINEAR_REG('model', …)` creates a model). `write_calls` lists them by name and family, so an
//! unknown function stays a read. An allow-list of calls is no way out: a meta-function is called
//! like any other function, so every query calling a built-in it does not name, or a function of
//! the user's, would be a write (docs/drivers/vertica.md, Limits). `SAFE_META_FUNCTIONS` lists
//! the read-only meta-functions, tested to stay reads.
//!
//! The statements the driver sends for itself (session setup, EXPORT_OBJECTS, kills) are built
//! here too.

use std::borrow::Cow;

use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, PLAIN, TokenKind, reading, scan};
use crate::sqlgen::dialect::spec::{
    CiLike, DialectSpec, NullSafeEq, NullsSort, PlaceholderStyle, RedactCall, RedactRule, SqlHooks,
    SqlKnobs, double_quoted, ph_question, single_quoted,
};
use crate::sqlgen::portable::classify;
use crate::sqlgen::split::leading_keywords;
use crate::types::schema::{ColumnMeta, TableRef, ValueKind};

pub static LEX: LexProfile = LexProfile {
    ident_quotes: &[('"', '"')],
    string_quotes: &['\''],
    // `StandardConformingStrings` on (the default): backslashes are plain; off: they escape.
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
    // Block comments nest, as in the SQL standard (the SQL reference's Comments page).
    nested_block_comments: true,
    dash_comment_needs_space: false,
    batch_separator: None,
    terminator_optional: false,
    begin_end_bodies: false,
    attribute_calls: false,
};

/// The functions a `SELECT` may call that change the cluster, its data, its files or its
/// sessions, read the server's files or reach other servers: the meta-functions by family
/// (`SET_*`, `DROP_*`…) and by name, the machine-learning functions that create models, views or
/// tables, the flex-table and geospatial-index builders, the exporters (shapefiles too), and the
/// AWS, Kafka and Voltage SecureData libraries.
const WRITE_CALLS: &[&str] = &[
    // Epochs, the tuple mover, statistics, logs (DO_TM_TASK runs the tuple mover,
    // DO_LOGROTATE_LOCAL deletes old logs; no DO_ function reads).
    "ADVANCE_EPOCH",
    "MAKE_AHM_NOW",
    "DO_*",
    "ANALYZE_*",
    "COMPACT_STORAGE",
    "PURGE*",
    "REFRESH*",
    // Sessions, statements, resources.
    "CLOSE_*",
    "INTERRUPT_STATEMENT",
    "CANCEL_*",
    "CHANGE_*",
    "MOVE_*",
    "RESERVE_*",
    "RELEASE_*",
    "RESET_*",
    "RESTRICT_*",
    "MEMORY_TRIM",
    // Stored procedures: EXECUTE_TRIGGER runs the procedure behind a trigger, which may hold any
    // DML or DDL (no EXECUTE_ function reads).
    "EXECUTE_*",
    // Settings and policies.
    "SET_*",
    "CLEAR_*",
    "ENABLE_*",
    "DISABLE_*",
    "REENABLE_*",
    "ENFORCE_*",
    "FLUSH_*",
    "RELOAD_*",
    "NOTIFY",
    "AZURE_TOKEN_CACHE_CLEAR",
    // Partitions, tables, projections, storage locations (ADD_LOCATION and MERGE_PARTITIONS on
    // old servers; by name, as ADD_MONTHS reads).
    "DROP_*",
    "COPY_*",
    "SWAP_*",
    "MERGE_PARTITIONS",
    "PARTITION_PROJECTION",
    "PARTITION_TABLE",
    "ADD_LOCATION",
    "REBALANCE_*",
    "RESHARD_*",
    "REALIGN_*",
    "ALTER_*",
    "RETIRE_*",
    "RESTORE_*",
    "MEASURE_LOCATION_PERFORMANCE",
    "RUN_INDEX_TOOL",
    // Cluster, subclusters, Eon depots and communal storage.
    "START_*",
    "STOP_*",
    "SHUTDOWN*",
    "PROMOTE_*",
    "DEMOTE_*",
    "SANDBOX_*",
    "UNSANDBOX_*",
    "MIGRATE_*",
    "SYNC_*",
    "CLEAN_*",
    "FINISH_FETCHING_FILES",
    "BACKGROUND_DEPOT_WARMING",
    "HURRY_SERVICE",
    "ROTATE_*",
    // Licenses, the catalog, designs, directed queries.
    "AUDIT*",
    "INSTALL_*",
    "MARK_DESIGN_KSAFE",
    "DESIGNER_*",
    "SAVE_*",
    "EXPORT_*",
    "IMPORT_*",
    "LDAP_LINK_*",
    // Server files and other servers. v_txtindex.DELETE_TOKENIZER_CONFIG_FILE deletes a file: the
    // DELETE keyword misses a one-word name, and no DELETE_ function reads.
    "DELETE_*",
    "GET_METADATA",
    "INFER_*",
    // Reads the statistics XML file at the server path the SQL names (superuser only).
    "VALIDATE_STATISTICS",
    "EXTERNAL_CONFIG_CHECK",
    "HADOOP_IMPERSONATION_CONFIG_CHECK",
    "HCATALOGCONNECTOR_CONFIG_CHECK",
    "HDFS_CLUSTER_CONFIG_CHECK",
    "KERBEROS_*",
    "SECURITY_CONFIG_CHECK",
    "VERIFY_HADOOP_CONF_DIR",
    // The AWS library: S3EXPORT writes to S3, AWS_SET_CONFIG sets the session's credentials.
    "S3EXPORT*",
    "AWS_SET_CONFIG",
    // The Kafka library is COPY sources, parsers and filters, and calls that reach the brokers
    // the SQL names (KafkaCheckBrokers, KafkaListTopics, KafkaListManyTopics,
    // KafkaTopicDetails, KafkaExport). KafkaOffsets only reads the last load's statistics, but
    // the family keeps a new broker call a write.
    "KAFKA*",
    // The Voltage SecureData library: VoltageSecureConfigureGlobal and VoltageSecureConfigure
    // save settings (a password too) to DFS files other users read, RefreshPolicy fetches the
    // policy, and Protect, Access and ProtectAllKeys call the SecureData appliance.
    "VOLTAGESECURE*",
    // Vertica Place writes shapefiles into the node's export directory, which the second sets,
    // and reads one from a node, a shared file system or an object store to describe its table.
    "STV_EXPORT2SHAPEFILE",
    "STV_SETEXPORTSHAPEFILEDIRECTORY",
    "STV_SHPCREATETABLE",
    // Flex tables and geospatial indexes.
    "BUILD_FLEXTABLE_VIEW",
    "COMPUTE_FLEXTABLE_KEYS*",
    "MATERIALIZE_FLEXTABLE_COLUMNS",
    "STV_CREATE_INDEX",
    "STV_DROP_INDEX",
    "STV_REFRESH_INDEX",
    "STV_RENAME_INDEX",
    // Machine learning: trainers create a model, the preparers a view or a table.
    "ARIMA",
    "AUTOREGRESSOR",
    "BISECTING_KMEANS",
    "IFOREST",
    "KMEANS",
    "KPROTOTYPES",
    "LINEAR_REG",
    "LOGISTIC_REG",
    "MOVING_AVERAGE",
    "NAIVE_BAYES",
    "PLS_REG",
    "POISSON_REG",
    "RF_CLASSIFIER",
    "RF_REGRESSOR",
    "SVM_CLASSIFIER",
    "SVM_REGRESSOR",
    "XGB_CLASSIFIER",
    "XGB_REGRESSOR",
    "PCA",
    "SVD",
    "NORMALIZE",
    "NORMALIZE_FIT",
    "ONE_HOT_ENCODER_FIT",
    "BALANCE",
    "DETECT_OUTLIERS",
    "IMPUTE",
    "REGISTER_MODEL",
    "UPGRADE_MODEL",
];

/// Meta-functions that only read: what Tabler itself and users call to look at the cluster. None
/// of them is in `write_calls` (tested), and each stays a read.
pub const SAFE_META_FUNCTIONS: &[&str] = &[
    "DATA_COLLECTOR_HELP",
    "DESCRIBE_LOAD_BALANCE_DECISION",
    "DISPLAY_LICENSE",
    "DUMP_CATALOG",
    "DUMP_LOCKTABLE",
    "DUMP_PARTITION_KEYS",
    "DUMP_PROJECTION_PARTITION_KEYS",
    "DUMP_TABLE_PARTITION_KEYS",
    "EVALUATE_DELETE_PERFORMANCE",
    "GET_AHM_EPOCH",
    "GET_AHM_TIME",
    "GET_AUDIT_TIME",
    "GET_CLIENT_LABEL",
    "GET_COMPLIANCE_STATUS",
    "GET_CONFIG_PARAMETER",
    "GET_CURRENT_EPOCH",
    "GET_DATA_COLLECTOR_NOTIFY_POLICY",
    "GET_DATA_COLLECTOR_POLICY",
    "GET_LAST_GOOD_EPOCH",
    "GET_NUM_ACCEPTED_ROWS",
    "GET_NUM_REJECTED_ROWS",
    "GET_PROJECTION_SORT_ORDER",
    "GET_PROJECTION_STATUS",
    "GET_PROJECTIONS",
    "GET_PRIVILEGES_DESCRIPTION",
    "HAS_ROLE",
    "ENABLED_ROLE",
    "SHOW_PROFILING_CONFIG",
    "GET_MODEL_ATTRIBUTE",
    "GET_MODEL_SUMMARY",
    "PREDICT_LINEAR_REG",
    "APPLY_NORMALIZE",
    "CONFUSION_MATRIX",
    "ERROR_RATE",
    "CURRENT_SESSION",
    "VERSION",
];

pub static RULES: ClassifyRules = ClassifyRules {
    // `AT EPOCH LATEST SELECT …` and `AT TIME '…' SELECT …` read history.
    queries: &["SELECT", "WITH", "AT"],
    // EXPLAIN never runs the statement (PROFILE does, so it is no read).
    inspections: &["SHOW", "EXPLAIN"],
    writes: &[
        "INTO", "INSERT", "UPDATE", "DELETE", "MERGE", "CREATE", "DROP", "ALTER", "TRUNCATE",
        "COPY", "EXPORT", "GRANT", "REVOKE",
    ],
    write_calls: WRITE_CALLS,
    // Vertica reads no table functions where a table goes (transforms run in the select list,
    // `… OVER ()`).
    table_functions: None,
};

/// A write when either the text as sent or its folded spelling reads as one. The raw text keeps
/// the server's byte-exact dollar tags and the lexer's guard on non-ASCII tags (ambiguous, so a
/// write); the folded text catches names spelled with the KELVIN SIGN that the deny-lists miss.
fn classify_vertica(sql: &str) -> StatementClass {
    match classify::with(&LEX, &RULES, sql) {
        StatementClass::Write => StatementClass::Write,
        _ => classify::with(&LEX, &RULES, &fold_ascii_lookalikes(sql)),
    }
}

/// `sql` with each letter that case-folds to an ASCII letter written as that letter upper-cased.
/// Vertica compares names case-insensitively in every alphabet, and the KELVIN SIGN (U+212A)
/// folds to `k` but upper-cases to itself, so the shared deny-lists (which upper-case) would miss
/// `\u{212A}MEANS(…)`; LONG S (U+017F) folds to `s` and would hide `CLO\u{17F}E_SESSION(…)`.
/// Only the classifier reads the folded text, and only next to the raw text:
/// folding also rewrites dollar tags (`$\u{212A}$` becomes `$K$`), which the server keeps apart.
fn fold_ascii_lookalikes(sql: &str) -> Cow<'_, str> {
    // Lower case finds the KELVIN SIGN; upper case finds LATIN SMALL LONG S (U+017F, which
    // case-folds to `s` but lower-cases to itself) and the dotless i. Folding only adds writes.
    fn one_ascii(mut it: impl Iterator<Item = char>) -> Option<char> {
        match (it.next(), it.next()) {
            (Some(l), None) if l.is_ascii_alphabetic() => Some(l.to_ascii_uppercase()),
            _ => None,
        }
    }
    let folded = |c: char| {
        if c.is_ascii() {
            return None;
        }
        one_ascii(c.to_lowercase()).or_else(|| one_ascii(c.to_uppercase()))
    };
    if sql.chars().any(|c| folded(c).is_some()) {
        Cow::Owned(sql.chars().map(|c| folded(c).unwrap_or(c)).collect())
    } else {
        Cow::Borrowed(sql)
    }
}

/// Quotes doubled. A backslash is itself under `StandardConformingStrings` (on by default) and
/// an escape without it, so a value holding one goes in an `E'…'` string, which reads escapes
/// either way.
pub fn quote_exec(s: &str) -> String {
    if s.contains('\\') {
        format!("E'{}'", s.replace('\\', "\\\\").replace('\'', "''"))
    } else {
        single_quoted(s)
    }
}

fn hex_to_binary(hex: &str) -> String {
    format!("HEX_TO_BINARY('{hex}')")
}

/// A column as text for filters and "Any column": LONG VARCHAR keeps its length, binary columns
/// have no text.
fn text_expr(e: &str, col: &ColumnMeta) -> Option<String> {
    if col.kind == ValueKind::Bytes {
        return None;
    }
    let ty = col.data_type.trim().to_ascii_lowercase();
    if ty.starts_with("long varchar") {
        Some(format!("CAST({e} AS LONG VARCHAR(32000000))"))
    } else {
        Some(format!("CAST({e} AS VARCHAR(65000))"))
    }
}

/// DDL commits the open transaction (and TRUNCATE, which is DDL in Vertica).
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
    // The server takes 32,767; batches stay well below.
    max_params: 1000,
    max_name_parts: 3,
    lex: &LEX,
    rules: &RULES,
    classify: classify_vertica,
    knobs: SqlKnobs {
        bytes_literal: hex_to_binary,
        text_expr,
        // NULLS FIRST / LAST belong to analytic ORDER BYs: a sort key does it everywhere.
        nulls_order: NullsSort::NullFlag,
        ci_like: CiLike::Ilike,
        null_safe_eq: NullSafeEq::OrForm,
        set_default: false,
        implicit_commit,
        catalog_words: &["V_CATALOG", "V_MONITOR", "V_INTERNAL", "V_FUNC"],
        // Reads the classifier lets through that still change something or wait.
        side_effect_functions: &["NEXTVAL", "SLEEP"],
        redact: &[
            RedactRule {
                keyword: "PASSWORD",
            },
            RedactRule {
                keyword: "IDENTIFIED",
            },
            // `ALTER USER u IDENTIFIED BY 'new' REPLACE 'old'`.
            RedactRule { keyword: "REPLACE" },
            RedactRule { keyword: "AWSAUTH" },
            RedactRule {
                keyword: "AWS_SECRET_ACCESS_KEY",
            },
            RedactRule {
                keyword: "AWS_SESSION_TOKEN",
            },
            // The session parameter's own name: `ALTER SESSION SET AWSSessionToken = '…'`.
            RedactRule {
                keyword: "AWSSESSIONTOKEN",
            },
            RedactRule { keyword: "GCSAUTH" },
            // The per-bucket form of AWSAuth, which takes precedence over it.
            RedactRule {
                keyword: "S3BUCKETCREDENTIALS",
            },
            RedactRule {
                keyword: "AZURESTORAGECREDENTIALS",
            },
            // LDAP Link's bind password, and an LDAP authentication's search-and-bind account
            // (`ALTER AUTHENTICATION a SET bind_password='…'`).
            RedactRule {
                keyword: "LDAPLINKBINDPSWD",
            },
            RedactRule {
                keyword: "BIND_PASSWORD",
            },
            RedactRule {
                keyword: "CLIENT_SECRET",
            },
            // UDx parameters: Voltage's `USING PARAMETERS shared_secret='…'`, the Kafka
            // functions' `kafka_conf_secret='…'`.
            RedactRule {
                keyword: "SHARED_SECRET",
            },
            RedactRule {
                keyword: "KAFKA_CONF_SECRET",
            },
            // librdkafka settings, which may carry `sasl.password=…`.
            RedactRule {
                keyword: "KAFKA_CONF",
            },
            // A notifier's adapter settings (`PARAMETERS 'sasl.password=…'`). This also masks
            // the first value of a `USING PARAMETERS` list, where Vertica's UDxs take their
            // credentials.
            RedactRule {
                keyword: "PARAMETERS",
            },
        ],
        // A configuration value set by name, whatever the name: `AWS_SET_CONFIG(
        // 'aws_secret_access_key', '…')`, `SET_CONFIG_PARAMETER('AWSAuth', 'id:secret')`.
        redact_calls: &[
            RedactCall {
                function: "AWS_SET_CONFIG",
                argument: 2,
            },
            RedactCall {
                function: "SET_CONFIG_PARAMETER",
                argument: 2,
            },
            // LDAP Link's dry runs take the bind password third: `(url, dn, password, …)`.
            RedactCall {
                function: "LDAP_LINK_DRYRUN_CONNECT",
                argument: 3,
            },
            RedactCall {
                function: "LDAP_LINK_DRYRUN_SEARCH",
                argument: 3,
            },
            RedactCall {
                function: "LDAP_LINK_DRYRUN_SYNC",
                argument: 3,
            },
        ],
        ..SqlKnobs::ANSI
    },
    hooks: SqlHooks::NONE,
    ddl: None,
};

/// Whether `sql` names a sequence's next value the Vertica way, `seq.NEXTVAL` (or
/// `"seq"."NEXTVAL"`): a qualified name, not a call, so `side_effect_functions` (which matches
/// `NEXTVAL(`) misses it. A text the scanner cannot read for sure counts as one, and so does a
/// Unicode-escaped name after a dot (`s.U&"\004EEXTVAL"`): its escapes may spell any name, the
/// same reading the shared classifier gives `U&"…"`.
pub fn advances_sequence(sql: &str) -> bool {
    // The raw text keeps the server's dollar tags and the lexer's non-ASCII guard; the folded
    // one spells `NEXTVAL` written with lookalike letters in ASCII.
    let folded = fold_ascii_lookalikes(sql);
    names_next_value(sql) || (matches!(folded, Cow::Owned(_)) && names_next_value(&folded))
}

fn names_next_value(sql: &str) -> bool {
    LEX.readings.iter().any(|&r| {
        let scan = scan(&LEX, r, sql);
        let code: Vec<_> = scan
            .tokens
            .iter()
            .filter(|t| !matches!(t.kind, TokenKind::Space | TokenKind::Comment))
            .collect();
        scan.ambiguous
            || code.windows(2).any(|w| {
                let next = match w[1].kind {
                    TokenKind::Word => scan.upper(w[1]),
                    TokenKind::Ident => scan.ident_name(w[1]).to_ascii_uppercase(),
                    _ => return false,
                };
                scan.is_punct(w[0], '.') && next == "NEXTVAL"
            })
            || code.windows(4).any(|w| {
                scan.is_punct(w[0], '.')
                    && w[1].kind == TokenKind::Word
                    && scan.upper(w[1]) == "U"
                    && scan.is_punct(w[2], '&')
                    && w[3].kind == TokenKind::Ident
            })
    })
}

// --- The driver's own statements -----------------------------------------------------------------

/// What a read-only session runs after login: the server then refuses writes.
pub const READ_ONLY_SESSION: &str = "SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY";
pub const AUTOCOMMIT_ON: &str = "SET SESSION AUTOCOMMIT TO ON";
pub const AUTOCOMMIT_OFF: &str = "SET SESSION AUTOCOMMIT TO OFF";
pub const SEARCH_PATH_DEFAULT: &str = "SET SEARCH_PATH TO DEFAULT";

/// The session's time zone (`America/New_York`, `UTC`, `+07`).
pub fn set_time_zone(zone: &str) -> String {
    format!("SET TIME ZONE TO {}", quote_exec(zone))
}

/// The schemas unqualified names resolve in, first match wins.
pub fn set_search_path(schemas: &[&str]) -> String {
    let list: Vec<String> = schemas.iter().map(|s| double_quoted(s)).collect();
    format!("SET SEARCH_PATH TO {}", list.join(", "))
}

/// `"schema"."table"` (the table alone without a schema).
pub fn table_name(table: &TableRef) -> String {
    match &table.schema {
        Some(s) => format!("{}.{}", double_quoted(s), double_quoted(&table.name)),
        None => double_quoted(&table.name),
    }
}

/// The DDL of a table, view, sequence or schema, as `EXPORT_OBJECTS` writes it (an empty
/// destination returns it as text; `false`: without the K-safety statement).
pub fn export_objects(object: &TableRef) -> String {
    format!(
        "SELECT EXPORT_OBJECTS('', {}, false)",
        quote_exec(&table_name(object))
    )
}

/// Stops a session's running statement (Kill Query): the session id and statement id of
/// `v_monitor.sessions`. Meta-functions take constants, so the id is a literal.
pub fn interrupt_statement(session_id: &str, statement_id: i64) -> String {
    format!(
        "SELECT INTERRUPT_STATEMENT({}, {statement_id})",
        quote_exec(session_id)
    )
}

/// Ends a session (Kill Connection).
pub fn close_session(session_id: &str) -> String {
    format!("SELECT CLOSE_SESSION({})", quote_exec(session_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlgen::dialect::lex::listed;
    use crate::sqlgen::portable::testing::col;

    #[test]
    fn export_bindings_lex_vertica() {
        crate::sqlgen::dialect::lex::export_lex("vertica", &LEX, &RULES);
    }

    #[test]
    fn quotes_and_builds_the_drivers_statements() {
        assert_eq!(quote_exec("a\\'b"), "E'a\\\\''b'");
        assert_eq!(quote_exec("it's"), "'it''s'");
        assert_eq!(
            set_time_zone("Asia/Jakarta"),
            "SET TIME ZONE TO 'Asia/Jakarta'"
        );
        assert_eq!(set_time_zone("x'; DROP"), "SET TIME ZONE TO 'x''; DROP'");
        assert_eq!(
            set_search_path(&["My \"S\"", "public"]),
            "SET SEARCH_PATH TO \"My \"\"S\"\"\", \"public\""
        );
        let t = TableRef::new(Some("store"), "Sales \"2024\"");
        assert_eq!(
            export_objects(&t),
            "SELECT EXPORT_OBJECTS('', '\"store\".\"Sales \"\"2024\"\"\"', false)"
        );
        assert_eq!(table_name(&TableRef::new(None, "t")), "\"t\"");
        assert_eq!(
            interrupt_statement("v_vmart_node0001-31:0x1'", 3),
            "SELECT INTERRUPT_STATEMENT('v_vmart_node0001-31:0x1''', 3)"
        );
        assert_eq!(close_session("a\\b"), "SELECT CLOSE_SESSION(E'a\\\\b')");
        // The driver's own statements are what the classifier says they are.
        for sql in [READ_ONLY_SESSION, AUTOCOMMIT_ON, SEARCH_PATH_DEFAULT] {
            assert_eq!(classify_vertica(sql), StatementClass::Write, "{sql}");
        }
        assert_eq!(classify_vertica(&close_session("x")), StatementClass::Write);
        assert_eq!(classify_vertica(&export_objects(&t)), StatementClass::Write);
    }

    #[test]
    fn text_forms_and_commits() {
        let name = col("name", "varchar(80)", ValueKind::Text);
        let doc = col("doc", "long varchar(1048576)", ValueKind::Text);
        let blob = col("b", "varbinary(80)", ValueKind::Bytes);
        assert_eq!(
            text_expr("\"name\"", &name).as_deref(),
            Some("CAST(\"name\" AS VARCHAR(65000))")
        );
        assert_eq!(
            text_expr("\"doc\"", &doc).as_deref(),
            Some("CAST(\"doc\" AS LONG VARCHAR(32000000))")
        );
        assert_eq!(text_expr("\"b\"", &blob), None);
        assert_eq!(hex_to_binary("00ff"), "HEX_TO_BINARY('00ff')");
        assert!(
            implicit_commit("truncate table t") && implicit_commit("COMMENT ON TABLE t IS 'x'")
        );
        assert!(!implicit_commit("INSERT INTO t VALUES (1)"));
    }

    #[test]
    fn meta_functions_that_write_and_the_ones_that_read() {
        for sql in [
            "SELECT CLOSE_ALL_SESSIONS()",
            "SELECT SHUTDOWN()",
            "SELECT SHUTDOWN_SUBCLUSTER('sc')",
            "SELECT COPY_TABLE('a','b')",
            "SELECT MOVE_PARTITIONS_TO_TABLE('a', '1', '2', 'b')",
            "SELECT SWAP_PARTITIONS_BETWEEN_TABLES('a', '1', '2', 'b')",
            "SELECT DROP_PARTITIONS('t', '2020', '2021')",
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
            "SELECT SET_CONFIG_PARAMETER('MaxClientSessions', 100)",
            "SELECT ANALYZE_STATISTICS('public.t')",
            "SELECT PURGE_TABLE('t')",
            "SELECT DO_TM_TASK('mergeout', 't')",
            "SELECT DO_LOGROTATE_LOCAL('force;max_age=1 day')",
            "SELECT EXECUTE_TRIGGER('vmart.management.log_user_actions')",
            "SELECT v_txtindex.DELETE_TOKENIZER_CONFIG_FILE(USING PARAMETERS proc_oid='45035996274126984', confirm=TRUE)",
            "SELECT EXPORT_CATALOG('/tmp/cat.sql', 'DESIGN')",
            "SELECT EXPORT_OBJECTS('/tmp/x.sql', 'public')",
            "SELECT INTERRUPT_STATEMENT('s', 1)",
            "SELECT GET_METADATA('/data/f.parquet')",
            "SELECT INFER_TABLE_DDL('/data/*.parquet' USING PARAMETERS format = 'parquet', table_name = 't')",
            "SELECT LINEAR_REG('m', 'public.t', 'y', 'x')",
            "SELECT NORMALIZE('v', 't', 'x', 'minmax')",
            "SELECT COMPUTE_FLEXTABLE_KEYS_AND_BUILD_VIEW('raw')",
            "SELECT STV_Create_Index(gid, geom USING PARAMETERS index = 'i') OVER () FROM polys",
            "SELECT s3export(*) OVER () FROM t",
            "SELECT STV_Export2Shapefile(* USING PARAMETERS shapefile = 'x.shp') OVER() FROM t",
            "SELECT STV_SetExportShapefileDirectory(USING PARAMETERS path = '/tmp')",
            "SELECT ADD_LOCATION('/data', 'node', 'DATA')",
            "SELECT MERGE_PARTITIONS('t', '2020', '2021')",
            "SELECT AZURE_TOKEN_CACHE_CLEAR()",
            "SELECT STV_ShpCreateTable(USING PARAMETERS file = '/etc/x.shp') OVER()",
            "SELECT KafkaCheckBrokers(USING PARAMETERS brokers = 'h:9092') OVER ()",
            "SELECT KafkaListTopics(USING PARAMETERS brokers = 'h:9092') OVER ()",
            "SELECT KafkaListManyTopics('h:9092') OVER ()",
            "SELECT KafkaTopicDetails(USING PARAMETERS brokers = 'h:9092', topic = 't') OVER ()",
            "SELECT KafkaExport(p, k, v USING PARAMETERS brokers = 'h:9092') OVER () FROM t",
            "SELECT AWS_SET_CONFIG('aws_secret', 'x')",
            "SELECT VoltageSecureConfigureGlobal(USING PARAMETERS policy_url = 'u') OVER ()",
            "SELECT \"VoltageSecureConfigureGlobal\"(USING PARAMETERS policy_url='u') OVER ()",
            "SELECT VoltageSecureConfigure(USING PARAMETERS config_dfs_path = '/voltagesecure/conf', store_password = true) OVER ()",
            "SELECT VoltageSecureRefreshPolicy() OVER ()",
            "SELECT VoltageSecureProtect('123-45-6789' USING PARAMETERS format = 'ssn')",
            "SELECT VoltageSecureAccess(ssn USING PARAMETERS format = 'ssn') FROM t",
            "SELECT VoltageSecureProtectAllKeys('123' USING PARAMETERS format = 'efpe') OVER ()",
            "SELECT VALIDATE_STATISTICS('/home/dbadmin/stats.xml')",
            "SELECT \"close_session\"('x')",
            "SELECT v_catalog.CLOSE_SESSION('x')",
            "SELECT LDAP_LINK_SYNC_START()",
            "SELECT AUDIT('')",
            "SELECT 'a\\'; DELETE FROM t --'",
            "SELECT $$'$$; DELETE FROM t; SELECT $$'$$",
            "PROFILE SELECT * FROM t",
            "AT EPOCH LATEST DELETE FROM t",
        ] {
            assert_eq!(classify_vertica(sql), StatementClass::Write, "{sql}");
        }
        for f in SAFE_META_FUNCTIONS {
            assert!(!listed(WRITE_CALLS, f), "{f} is a safe meta-function");
            let sql = format!("SELECT {f}()");
            assert_eq!(classify_vertica(&sql), StatementClass::Read, "{sql}");
        }
        for sql in [
            "SELECT * FROM t",
            "SELECT REGEXP_REPLACE(a, 'x', 'y'), COALESCE(b, 0) FROM t",
            "SELECT ADD_MONTHS(d, 1) FROM t",
            "SELECT AWS_GET_CONFIG('aws_region')",
            "SELECT v_txtindex.READ_CONFIG_FILE(USING PARAMETERS proc_oid='1') OVER ()",
            "SELECT ACTIVE_SCHEDULER_NODE()",
            "AT EPOCH LATEST SELECT * FROM t",
            "AT TIME '2024-01-01 00:00:00' SELECT count(*) FROM t",
            "SELECT PREDICT_LINEAR_REG(x USING PARAMETERS model_name = 'm') FROM t",
            "SELECT NEXTVAL('s')",
            "EXPLAIN SELECT * FROM t",
            "EXPLAIN DELETE FROM t",
            "SHOW SEARCH_PATH",
        ] {
            assert_eq!(classify_vertica(sql), StatementClass::Read, "{sql}");
        }
    }

    /// The KELVIN SIGN folds to `k` under Unicode case rules, which Vertica's names follow: each
    /// listed write spelled with it stays a write, as with ASCII `K` and LATIN SMALL LONG S.
    #[test]
    fn a_kelvin_sign_hides_no_listed_write() {
        for sql in [
            "SELECT \u{212A}afkaListTopics('evil:9092')",
            "SELECT LDAP_LIN\u{212A}_SYNC_START()",
            "SELECT \u{212A}MEANS('m','t','*',2)",
            "SELECT \"\u{212A}AFKAEXPORT\"(1,2,3) OVER ()",
            "SELECT \u{212A}ERBEROS_CONFIG_CHECK()",
            "SELECT MAR\u{212A}_DESIGN_\u{212A}SAFE(1)",
            "SELECT v_txtindex.\u{212A}afkaExport(1) OVER ()",
            "SELECT BISECTING_\u{212A}MEANS('m','t','*',2)",
            "SELECT \u{212A}PROTOTYPES('m','t','*',2)",
            "SELECT CLO\u{17F}E_ALL_SESSIONS()",
            "SELECT CLO\u{17F}E_SESSION('x')",
            "SELECT \u{17F}HUTDOWN()",
        ] {
            assert_eq!(classify_vertica(sql), StatementClass::Write, "{sql}");
            let ascii = sql.replace('\u{212A}', "K").replace('\u{17F}', "S");
            assert_eq!(classify_vertica(&ascii), StatementClass::Write, "{ascii}");
        }
        for call in WRITE_CALLS.iter().filter(|c| c.contains('K')) {
            let name = call.trim_end_matches('*').replace('K', "\u{212A}");
            let sql = format!("SELECT {name}(1)");
            assert_eq!(classify_vertica(&sql), StatementClass::Write, "{sql}");
        }
        // A string or a name that only holds the sign stays a read; other letters stay as they are.
        assert_eq!(
            classify_vertica("SELECT '\u{212A}MEANS(1)', \u{212A}elvin FROM t"),
            StatementClass::Read
        );
        assert_eq!(fold_ascii_lookalikes("caf\u{e9} \u{212A}"), "caf\u{e9} K");
        assert_eq!(fold_ascii_lookalikes("\u{17F}\u{131}"), "SI");
        assert!(advances_sequence("SELECT s.NEXTVAL"));
        // Dollar tags are byte-exact on the server, so folding must not make `$\u{212A}$` and
        // `$K$` one tag: the lexer's guard on a non-ASCII tag (or a `$$` after a non-ASCII
        // letter) still makes each of these a write, and a sequence hidden the same way counts.
        for sql in [
            "SELECT $\u{212A}$ a $K$ b $\u{212A}$ AS x, KMEANS('m','t','*',2) -- $K$",
            "SELECT $\u{212A}$ a $K$ b $\u{212A}$; SELECT CLOSE_ALL_SESSIONS() -- $K$",
            "SELECT $\u{212A}$ x $\u{212A}$",
            "SELECT $\u{212A}$ x $K$ ' $\u{212A}$, DROP_PARTITIONS('t', 1, 2) -- '",
            "SELECT \u{212A}$$ '$$, DROP_PARTITIONS('t',1,2) -- '",
            "SELECT a\u{212A}$$ '$$ ; DELETE FROM t --'",
        ] {
            assert_eq!(classify_vertica(sql), StatementClass::Write, "{sql}");
            assert!(
                crate::sqlgen::sql::writes(SqlDialect::Vertica, sql),
                "{sql}"
            );
        }
        assert!(advances_sequence(
            "SELECT $\u{212A}$ x $K$ ' $\u{212A}$, s.NEXTVAL -- '"
        ));
    }

    #[test]
    fn a_sequence_s_next_value_is_found_in_either_form() {
        for sql in [
            "SELECT my_seq.NEXTVAL",
            "SELECT s.nextval FROM t",
            "SELECT \"my_seq\".\"NEXTVAL\"",
            "SELECT app.s . /* x */ NextVal",
            "INSERT INTO t SELECT s.NEXTVAL",
            r#"SELECT s.U&"NEXTVAL""#,
            r#"SELECT s.u&"nextval""#,
            r#"SELECT s.U&"\004EEXTVAL""#,
            r#"SELECT s.U&"!004EEXTVAL" UESCAPE '!'"#,
        ] {
            assert!(advances_sequence(sql), "{sql}");
        }
        for sql in [
            "SELECT NEXTVAL('s')",
            "SELECT s.CURRVAL",
            "SELECT 's.NEXTVAL', \"a.NEXTVAL\" FROM t -- s.NEXTVAL",
            "SELECT t.nextval_count FROM t",
            r#"SELECT U&"nextval" FROM t"#,
        ] {
            assert!(!advances_sequence(sql), "{sql}");
        }
    }

    /// The credentials Vertica statements carry are masked in the trace and history.
    #[test]
    fn redacts_passwords_and_cloud_credentials() {
        let redact =
            |sql: &str| crate::drivers::redact::redact_secrets(sql, SqlDialect::Vertica).sql;
        let mask = crate::sqlgen::admin::PASSWORD_MASK;
        for (sql, masked) in [
            (
                "ALTER USER u IDENTIFIED BY 'SECRETX' REPLACE 'OLDX'",
                format!("ALTER USER u IDENTIFIED BY {mask} REPLACE {mask}"),
            ),
            (
                "SELECT AWS_SET_CONFIG('aws_secret_access_key', 'SECRETX')",
                format!("SELECT AWS_SET_CONFIG('aws_secret_access_key', {mask})"),
            ),
            (
                "select aws_set_config('aws_session_token', 'SECRETX')",
                format!("select aws_set_config('aws_session_token', {mask})"),
            ),
            (
                "ALTER SESSION SET AWSSessionToken='SECRETX'",
                format!("ALTER SESSION SET AWSSessionToken={mask}"),
            ),
            (
                "ALTER SESSION SET UDPARAMETER FOR awslib aws_secret_access_key='SECRETX'",
                format!("ALTER SESSION SET UDPARAMETER FOR awslib aws_secret_access_key={mask}"),
            ),
            (
                "SELECT SET_CONFIG_PARAMETER('AWSAuth', 'id:SECRETX')",
                format!("SELECT SET_CONFIG_PARAMETER('AWSAuth', {mask})"),
            ),
            (
                "ALTER DATABASE DEFAULT SET AWSAuth = 'id:SECRETX'",
                format!("ALTER DATABASE DEFAULT SET AWSAuth = {mask}"),
            ),
            (
                "SELECT VoltageSecureConfigureGlobal(USING PARAMETERS server='v', shared_secret='SECRETX')",
                format!(
                    "SELECT VoltageSecureConfigureGlobal(USING PARAMETERS server={mask}, shared_secret={mask})"
                ),
            ),
            (
                "SELECT KafkaListTopics(USING PARAMETERS brokers='k', kafka_conf_secret='SECRETX')",
                format!(
                    "SELECT KafkaListTopics(USING PARAMETERS brokers={mask}, kafka_conf_secret={mask})"
                ),
            ),
            (
                r#"ALTER SESSION SET S3BucketCredentials='[{"bucket":"b","accessKey":"a","secretAccessKey":"SECRETX"}]'"#,
                format!("ALTER SESSION SET S3BucketCredentials={mask}"),
            ),
            (
                "ALTER DATABASE DEFAULT SET LDAPLinkBindPswd = 'SECRETX'",
                format!("ALTER DATABASE DEFAULT SET LDAPLinkBindPswd = {mask}"),
            ),
            (
                "ALTER DATABASE DEFAULT SET PARAMETER LDAPLinkBindPswd = 'SECRETX'",
                format!("ALTER DATABASE DEFAULT SET PARAMETER LDAPLinkBindPswd = {mask}"),
            ),
            (
                "ALTER AUTHENTICATION v_ldap SET host='ldap://h', binddn='cn=x', bind_password='SECRETX'",
                format!(
                    "ALTER AUTHENTICATION v_ldap SET host='ldap://h', binddn='cn=x', bind_password={mask}"
                ),
            ),
            (
                "SELECT LDAP_LINK_DRYRUN_CONNECT('ldap://h', 'cn=x', 'SECRETX')",
                format!("SELECT LDAP_LINK_DRYRUN_CONNECT('ldap://h', 'cn=x', {mask})"),
            ),
            (
                "SELECT ldap_link_dryrun_search('ldap://h', 'cn=x', 'SECRETX', 'dc=x', '')",
                format!("SELECT ldap_link_dryrun_search('ldap://h', 'cn=x', {mask}, 'dc=x', '')"),
            ),
            (
                "SELECT LDAP_LINK_DRYRUN_SYNC('ldap://h', 'cn=x', 'SECRETX', 'dc=x', '', 'dc=x', '')",
                format!(
                    "SELECT LDAP_LINK_DRYRUN_SYNC('ldap://h', 'cn=x', {mask}, 'dc=x', '', 'dc=x', '')"
                ),
            ),
            (
                "SELECT KafkaListTopics(USING PARAMETERS brokers='k', kafka_conf='sasl.password=SECRETX')",
                format!(
                    "SELECT KafkaListTopics(USING PARAMETERS brokers={mask}, kafka_conf={mask})"
                ),
            ),
            (
                "CREATE NOTIFIER n ACTION 'kafka://x' PARAMETERS 'sasl.password=SECRETX'",
                format!("CREATE NOTIFIER n ACTION 'kafka://x' PARAMETERS {mask}"),
            ),
        ] {
            let out = redact(sql);
            assert_eq!(out, masked, "{sql}");
            assert!(!out.contains("SECRETX"), "{sql}");
        }
        // Statements without a credential are left as written.
        for sql in [
            "SELECT 'x' AS a FROM t WHERE b = 'y'",
            "SELECT GET_CONFIG_PARAMETER('AWSRegion')",
        ] {
            assert_eq!(redact(sql), sql, "{sql}");
        }
    }

    #[test]
    fn block_comments_nest_when_splitting() {
        // A flat reader would send `SELECT 1 /* x /* y */`, an unterminated comment.
        let parts: Vec<String> =
            crate::sqlgen::portable::split::split(&LEX, "SELECT 1 /* x /* y */ ; z */ ; SELECT 2")
                .into_iter()
                .map(|s| s.sql)
                .collect();
        assert_eq!(parts, ["SELECT 1 /* x /* y */ ; z */", "SELECT 2"]);
        // The classifier does not rely on it: a block comment holding `/*` writes.
        assert_eq!(
            classify_vertica("SELECT 1 /* x /* y */ ; z */ ; SELECT 2"),
            StatementClass::Write
        );
    }
}
