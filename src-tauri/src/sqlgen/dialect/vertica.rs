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

use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, PLAIN, reading};
use crate::sqlgen::dialect::spec::{
    CiLike, DialectSpec, NullSafeEq, NullsSort, PlaceholderStyle, RedactRule, SqlHooks, SqlKnobs,
    double_quoted, ph_question, single_quoted,
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

fn classify_vertica(sql: &str) -> StatementClass {
    classify::with(&LEX, &RULES, sql)
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
            RedactRule { keyword: "GCSAUTH" },
            RedactRule {
                keyword: "AZURESTORAGECREDENTIALS",
            },
            RedactRule {
                keyword: "CLIENT_SECRET",
            },
        ],
        ..SqlKnobs::ANSI
    },
    hooks: SqlHooks::NONE,
    ddl: None,
};

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
