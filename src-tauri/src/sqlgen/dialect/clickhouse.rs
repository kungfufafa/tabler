//! ClickHouse SQL (the clickhouse lane). Built with `drv-clickhouse` only.
//!
//! Backticks or double quotes and single-quoted strings, all with backslash escapes; `‘…’`
//! strings and `“…”` names (Unicode quotes, without escapes); typed `{p1:Type}` parameters (the
//! value travels as `param_p1`, never in the text); `--`, `#`, `//` and nesting `/* */`
//! comments; `$tag$` heredocs. There are no transactions and no RETURNING.
//!
//! **Safe Mode.** The classifier knows lexical forms only. A read is a SELECT, a WITH query, a
//! query that starts with FROM (with or without pipe operators `|>`), SHOW, DESCRIBE / DESC,
//! EXPLAIN or EXISTS that holds none of:
//!
//! - a table function outside the allow-list where a table goes (`url()`, `s3()`, `remote()`,
//!   `mysql()`, `file()`…: they reach other servers or the server's files);
//! - a call of a function that reaches other servers, reads the server's files, runs a program
//!   or changes server state, wherever it is (`write_calls`: `file()` is a scalar function
//!   too, `EXPLAIN TABLE OVERRIDE mysql(…)` names a table function where no table goes, the AI
//!   functions call a provider, `generateSerialID` counts in Keeper, `logTrace` writes the
//!   server log);
//! - `INTO` (`INTO OUTFILE`), `PARALLEL` (`… PARALLEL WITH DROP TABLE t`), `ANALYZE`
//!   (`EXPLAIN ANALYZE` runs its statement) or the setting
//!   `stream_like_engine_allow_direct_select` (a read of a Kafka, RabbitMQ, NATS, FileLog or
//!   S3Queue table then consumes its messages);
//! - a `SETTINGS` clause: settings whose values are SQL the server parses and runs (`select`,
//!   `order`, `filter`, `additional_result_filter`, `additional_table_filters`,
//!   `parallel_replicas_custom_key`, `implicit_table_at_top_level` beside a query-construction
//!   setting, `format_schema` once `format_schema_source = 'query'`) hide calls in strings, and
//!   the first three are keywords no word list can tell from the query's own.
//!
//! All of these starters are `queries`, so the words above count in every one of them. The
//! parser takes `PARALLEL WITH` after any statement, and the server runs the statements side by
//! side and refuses one with a result only once it has run, so `SHOW TABLES PARALLEL WITH DROP
//! TABLE t` and `EXPLAIN SELECT 1 PARALLEL WITH DROP TABLE t` drop the table. The price is that
//! every `EXPLAIN ANALYZE`, every EXPLAIN of an `INSERT … INTO` and every query with a SETTINGS
//! clause is a write too.
//!
//! `USE` is no read: it returns no result, so `USE db PARALLEL WITH DROP TABLE t` runs the
//! DROP. What a SELECT reaches through objects that already exist (a dictionary's source, a
//! table's engine, an executable UDF) is the server's to refuse: a read-only or unattended
//! session sends `readonly=2` with every request (`drivers::clickhouse::request`), and the
//! server's grants on sources (`FILE`, `URL`, `REMOTE`, `S3`…) decide what a read may reach.
//!
//! **Edits** address a row by its primary key only. ClickHouse keys are not unique and nothing
//! limits a mutation to one row, so the driver counts the rows a key matches right before the
//! edit (`guard_count`) and refuses unless it is exactly one: best effort, since a row inserted
//! between the count and the edit is not seen. Updates are `ALTER TABLE … UPDATE` mutations
//! (every MergeTree table of every supported version), deletes are lightweight
//! `DELETE FROM … WHERE`, inserts `INSERT INTO … SELECT` of typed parameters. Key columns,
//! MATERIALIZED and ALIAS columns cannot be updated.

use std::collections::BTreeMap;

use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, Scan, TokenKind, reading, scan};
use crate::sqlgen::dialect::spec::{
    CiLike, DialectSpec, NullSafeEq, PlaceholderStyle, RedactRule, SqlHooks, SqlKnobs,
    backticked_backslash, explain_plan_only, single_quoted_backslash,
};
use crate::sqlgen::mutate::{Cell, cells};
use crate::sqlgen::portable::classify;
use crate::sqlgen::portable::select::{
    Params, check_whole, find_column, ident, row_condition, table_name,
};
use crate::sqlgen::select::BuiltSql;
use crate::sqlgen::sql::TableTarget;
use crate::types::browse::RowKey;
use crate::types::mutate::{Clock, ServerExpr, StagedChange};
use crate::types::schema::{ColumnMeta, ValueKind};
use crate::types::value::Value;

pub static LEX: LexProfile = LexProfile {
    // The Lexer also takes Unicode quotes (copied from word processors): `‘…’` is a string and
    // `“…”` a quoted name, both closed by the first closing quote, without escapes. A `‘…’`
    // string read as a quoted name only adds writes (one followed by `(` is a callee).
    ident_quotes: &[
        ('`', '`'),
        ('"', '"'),
        ('\u{2018}', '\u{2019}'),
        ('\u{201C}', '\u{201D}'),
    ],
    string_quotes: &['\''],
    // Strings and the ASCII-quoted names take backslash escapes, the Unicode-quoted ones none: a
    // read must read under both (`“\”, file('/etc/passwd') --”` is a name, a call and a
    // comment). The price is a false write for a backquoted or double-quoted name holding a
    // backslash-escaped quote.
    readings: &[reading(true, true), reading(true, false)],
    e_strings: false,
    dollar_quotes: true,
    dollar_tags: true,
    triple_quotes: false,
    raw_prefixes: false,
    byte_prefixes: false,
    // `# ` and `#!`; a `#` before anything else is a syntax error to the server, so reading it
    // as a comment too only hides text the server refuses.
    hash_comments: true,
    slash_slash_comments: true,
    // ClickHouse's Lexer nests them.
    nested_block_comments: true,
    dash_comment_needs_space: false,
    batch_separator: None,
    terminator_optional: false,
    begin_end_bodies: false,
};

pub static RULES: ClassifyRules = ClassifyRules {
    // FROM: a query may start with its FROM clause (`FROM t WHERE …`, `FROM t |> …`). SHOW,
    // DESCRIBE, EXPLAIN and EXISTS are queries rather than inspections, so `writes` count in
    // them: `PARALLEL WITH` may follow any of them, and the server runs the statements side by
    // side before it refuses one that returns a result (InterpreterParallelWithQuery).
    queries: &[
        "SELECT", "WITH", "FROM", "SHOW", "DESCRIBE", "DESC", "EXPLAIN", "EXISTS",
    ],
    inspections: &[],
    // Statements end at `;`, so these are only looked for inside queries, where the statement
    // keywords (SYSTEM, ALTER, KILL…) can only be names (`FROM system.tables`). ANALYZE: an
    // `EXPLAIN ANALYZE` runs its statement (26.x a SELECT only, but the parser takes CREATE,
    // INSERT and SYSTEM there too), and an EXPLAIN is a query here.
    writes: &[
        "INTO",
        "PARALLEL",
        "ANALYZE",
        "STREAM_LIKE_ENGINE_ALLOW_DIRECT_SELECT",
        // A statement's settings reach the server only through SETTINGS (EXPLAIN's own options
        // are not settings). Several take SQL the server parses with every function at hand,
        // which `readonly=2` does not stop: the 26.8 query-construction settings `select`,
        // `order` and `filter` (keywords, so only the clause itself can be listed; quoted
        // setting names too), and the ones below.
        "SETTINGS",
        // Filter expressions over the result or a table's rows.
        "ADDITIONAL_RESULT_FILTER",
        "ADDITIONAL_TABLE_FILTERS",
        // An expression evaluated on every row to split a read between replicas.
        "PARALLEL_REPLICAS_CUSTOM_KEY",
        // Next to a query-construction setting the server parses it as a FROM list, table
        // functions included.
        "IMPLICIT_TABLE_AT_TOP_LEVEL",
        // `'query'` makes `format_schema` a query the server runs for a Protobuf or CapnProto
        // result. A result in another format than the driver's (a FORMAT clause before 26.8,
        // `output_format` in SETTINGS) writes a schema file, or reads schema and template files,
        // on the server; a read that parses CSV or TSV (an existing File or URL table) writes
        // its errors to a file.
        "FORMAT_SCHEMA_SOURCE",
        "FORMAT_SCHEMA",
        "OUTPUT_FORMAT_SCHEMA",
        "FORMAT_TEMPLATE_RESULTSET",
        "FORMAT_TEMPLATE_ROW",
        "INPUT_FORMAT_RECORD_ERRORS_FILE_PATH",
        // They point the relative URLs of existing URL and S3 tables at another host.
        "URL_BASE",
        "S3_BASE",
    ],
    // The sweep of the function reference (docs/drivers/clickhouse.md): what reaches other
    // servers, reads the server's files, runs a program or changes server state. Table functions
    // are here too, so they count wherever they are, not only where a table goes.
    write_calls: &[
        // Files of the server.
        "FILE",
        "FILECLUSTER",
        "FILESYSTEM",
        "SQLITE",
        // Other servers and object stores.
        "URL",
        "URLCLUSTER",
        "S3",
        "S3CLUSTER",
        "COSN",
        "OSS",
        "GCS",
        "AZUREBLOBSTORAGE",
        "AZUREBLOBSTORAGECLUSTER",
        "HDFS",
        "HDFSCLUSTER",
        "REMOTE",
        "REMOTESECURE",
        "CLUSTER",
        "CLUSTERALLREPLICAS",
        "MYSQL",
        "POSTGRESQL",
        "MONGODB",
        "REDIS",
        "JDBC",
        "ODBC",
        "HIVE",
        "MEILISEARCH",
        "BIGQUERY",
        "ELASTICSEARCH",
        "YTSAURUS",
        "ARROWFLIGHT",
        // Not ICEBERG*: icebergBucket, icebergTruncate… are pure functions.
        "ICEBERG",
        "ICEBERGS3",
        "ICEBERGAZURE",
        "ICEBERGHDFS",
        "ICEBERGLOCAL",
        "ICEBERGCLUSTER",
        "ICEBERGS3CLUSTER",
        "ICEBERGAZURECLUSTER",
        "ICEBERGHDFSCLUSTER",
        // Reads the files of every node of a cluster.
        "ICEBERGLOCALCLUSTER",
        "DELTALAKE*",
        "HUDI*",
        "PAIMON*",
        // Programs, and SQL hidden in a string.
        "EXECUTABLE",
        "EVAL",
        // Large language model providers (aiGenerate, aiEmbed…).
        "AI*",
        // Server state and files.
        "GENERATESERIALID",
        "LOGTRACE",
        "SHOWCERTIFICATE",
        "ADDRESSTOLINE",
        "ADDRESSTOLINEWITHINLINES",
        "ADDRESSTOSYMBOL",
        "CATBOOSTEVALUATE",
        "MODELEVALUATE",
        // Older servers take a host, a user and a password and ask another server.
        "HASCOLUMNINTABLE",
        // With its `cluster` argument it reads the span log of every replica.
        "TRACEVIEW",
    ],
    table_functions: Some(&[
        "NUMBERS",
        "NUMBERS_MT",
        "ZEROS",
        "ZEROS_MT",
        "VALUES",
        "GENERATERANDOM",
        "GENERATE_SERIES",
        "GENERATESERIES",
        "VIEW",
        "MERGE",
        "NULL",
    ]),
};

fn classify_clickhouse(sql: &str) -> StatementClass {
    classify::with(&LEX, &RULES, sql)
}

/// `{pN:Type}`: the column's own type, or String for a value without a column.
fn placeholder(n: usize, col: Option<&ColumnMeta>) -> String {
    format!(
        "{{p{n}:{}}}",
        col.map_or("String", |c| c.data_type.as_str())
    )
}

fn unhex(hex: &str) -> String {
    format!("unhex('{hex}')")
}

fn to_string(e: &str, _: &ColumnMeta) -> Option<String> {
    Some(format!("toString({e})"))
}

/// An `AggregateFunction` column holds a binary state: the grid gets its final value as text.
fn is_aggregate_state(col: &ColumnMeta) -> bool {
    col.data_type.trim_start().starts_with("AggregateFunction(")
}

fn aggregate_text(e: &str, col: &ColumnMeta) -> Option<String> {
    is_aggregate_state(col).then(|| format!("toString(finalizeAggregation({e}))"))
}

fn server_expr(expr: ServerExpr, _: ValueKind) -> Option<String> {
    match expr.clock() {
        Some((Clock::Now | Clock::CurrentTimestamp, None)) => Some("now()".into()),
        Some((Clock::Now | Clock::CurrentTimestamp, Some(n))) => Some(format!("now64({n})")),
        None if expr == ServerExpr::CurrentDate => Some("today()".into()),
        _ => None,
    }
}

pub static SPEC: DialectSpec = DialectSpec {
    dialect: SqlDialect::Clickhouse,
    label: "ClickHouse SQL",
    quote_ident: backticked_backslash,
    quote_literal: single_quoted_backslash,
    quote_literal_exec: single_quoted_backslash,
    placeholder,
    placeholder_style: PlaceholderStyle::Typed,
    placeholders_reusable: true,
    // Values travel in the URL (1 MiB by default, `http_max_uri_size`).
    max_params: 1000,
    max_name_parts: 2,
    lex: &LEX,
    rules: &RULES,
    classify: classify_clickhouse,
    knobs: SqlKnobs {
        bool_literal: ["false", "true"],
        bytes_literal: unhex,
        text_projection: Some(aggregate_text),
        lossy_text: is_aggregate_state,
        text_expr: to_string,
        count_expr: "count()",
        ci_like: CiLike::Ilike,
        // LIKE takes `\%` and `\_`; there is no ESCAPE clause.
        like_escape: None,
        null_safe_eq: NullSafeEq::OrForm,
        insert_all_defaults: None,
        set_default: false,
        server_expr,
        // EXPLAIN ANALYZE runs its statement and exists only on recent servers: the session
        // offers it (`explain_form`) when the server has it, and the classifier calls every
        // EXPLAIN ANALYZE a write (`RULES.writes`).
        explain: explain_plan_only,
        catalog_words: &["SYSTEM.", "INFORMATION_SCHEMA"],
        side_effect_functions: &[
            "SLEEP",
            "SLEEPEACHROW",
            "GENERATESERIALID",
            "LOGTRACE",
            "AI*",
        ],
        redact: &[
            // `IDENTIFIED [WITH sha256_password] BY '…'`
            RedactRule {
                keyword: "IDENTIFIED",
            },
            RedactRule {
                keyword: "PASSWORD",
            },
            // Named collections and AI credentials (`api_key = '…'`), S3 and Azure keys.
            RedactRule { keyword: "API_KEY" },
            RedactRule {
                keyword: "SECRET_ACCESS_KEY",
            },
            RedactRule {
                keyword: "ACCOUNT_KEY",
            },
        ],
        cancel_notice: "what it changed before it stopped stays changed (ClickHouse has no \
                        transactions)",
        ..SqlKnobs::ANSI
    },
    hooks: SqlHooks {
        build_change: Some(build_change),
        ..SqlHooks::NONE
    },
    ddl: None,
};

// --- Statements the driver sends -----------------------------------------------------------

/// Stops a query by id; `{id:String}` is bound. Users may always stop their own queries.
pub const KILL_QUERY: &str = "KILL QUERY WHERE query_id = {id:String} ASYNC";

/// Words a catalog column's `extra` holds for each part of the table's key it is used in
/// (`drivers::clickhouse::catalog`). ClickHouse refuses to update such a column.
pub const KEY_ROLES: [&str; 4] = [
    "primary key",
    "sorting key",
    "partition key",
    "sampling key",
];

fn in_key(col: &ColumnMeta) -> bool {
    col.is_pk
        || col
            .extra
            .as_deref()
            .is_some_and(|e| KEY_ROLES.iter().any(|r| e.contains(r)))
}

/// What an edit statement is, for the settings it is sent with and how its count is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// `INSERT`: `X-ClickHouse-Summary` counts the rows written.
    Insert,
    /// `ALTER TABLE … UPDATE | DELETE`: asynchronous unless `mutations_sync`; no count.
    Mutation,
    /// `DELETE FROM`: no count.
    LightweightDelete,
    /// `UPDATE … SET` (25.7+): no count.
    LightweightUpdate,
    Other,
}

impl EditKind {
    /// The server does not say how many rows the statement changed.
    pub fn uncounted(self) -> bool {
        matches!(
            self,
            EditKind::Mutation | EditKind::LightweightDelete | EditKind::LightweightUpdate
        )
    }
}

/// The code tokens of `sql` (no spaces or comments), with their parenthesis depth; None when a
/// comment may end elsewhere for the server.
fn code_tokens(sql: &str) -> Option<(Scan, Vec<(usize, usize)>)> {
    let s = scan(&LEX, LEX.readings[0], sql);
    if s.ambiguous {
        return None;
    }
    let mut depth = 0usize;
    let mut out = Vec::new();
    for (i, t) in s.tokens.iter().enumerate() {
        if matches!(t.kind, TokenKind::Space | TokenKind::Comment) {
            continue;
        }
        if s.is_punct(t, ')') {
            depth = depth.saturating_sub(1);
        }
        out.push((i, depth));
        if s.is_punct(t, '(') {
            depth += 1;
        }
    }
    Some((s, out))
}

/// The kind of an edit statement, by its first keywords.
pub fn edit_kind(sql: &str) -> EditKind {
    let Some((s, code)) = code_tokens(sql) else {
        return EditKind::Other;
    };
    let word = |k: usize| {
        code.get(k)
            .map(|&(i, _)| &s.tokens[i])
            .filter(|t| t.kind == TokenKind::Word)
            .map(|t| s.upper(t))
    };
    match word(0).as_deref() {
        Some("INSERT") => EditKind::Insert,
        Some("DELETE") => EditKind::LightweightDelete,
        Some("UPDATE") => EditKind::LightweightUpdate,
        Some("ALTER") if word(1).as_deref() == Some("TABLE") => {
            match table_end(&s, &code, 2).and_then(word).as_deref() {
                Some("UPDATE" | "DELETE") => EditKind::Mutation,
                _ => EditKind::Other,
            }
        }
        _ => EditKind::Other,
    }
}

/// The server expressions an insert of `build_insert` may call.
const INSERT_CALLS: &[&str] = &["NOW", "NOW64", "TODAY"];

/// Whether `sql` is an insert of `build_insert`: `INSERT INTO t (…) SELECT` of placeholders and
/// clocks, with no FROM, set operation, join or other call, so it writes exactly one row to its
/// table. Its summary's `written_rows` may still be larger: rows the table's materialized views
/// wrote count too.
pub fn inserts_one_row(sql: &str) -> bool {
    let Some((s, code)) = code_tokens(sql) else {
        return false;
    };
    if edit_kind(sql) != EditKind::Insert {
        return false;
    }
    // The words outside placeholders (whose types look like calls: `{p1:Nullable(String)}`),
    // and whether each is called.
    let mut words: Vec<(String, bool)> = Vec::new();
    let mut braces = 0usize;
    for (k, &(i, _)) in code.iter().enumerate() {
        let t = &s.tokens[i];
        if s.is_punct(t, '{') {
            // Only a placeholder, never a map literal (`{'k': arrayJoin(…)}`).
            let placeholder = braces == 0
                && code.get(k + 1).is_some_and(|&(j, _)| {
                    let w = s.text(&s.tokens[j]);
                    s.tokens[j].kind == TokenKind::Word
                        && w.strip_prefix('p')
                            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                })
                && code
                    .get(k + 2)
                    .is_some_and(|&(j, _)| s.is_punct(&s.tokens[j], ':'));
            if !placeholder {
                return false;
            }
            braces += 1;
        } else if s.is_punct(t, '}') {
            braces = braces.saturating_sub(1);
        } else if braces == 0 && t.kind == TokenKind::Word {
            let called = code
                .get(k + 1)
                .is_some_and(|&(j, _)| s.is_punct(&s.tokens[j], '('));
            words.push((s.upper(t), called));
        }
    }
    let selects = words.iter().filter(|(w, _)| w == "SELECT").count();
    let Some(select) = words.iter().position(|(w, _)| w == "SELECT") else {
        return false;
    };
    // `INSERT INTO t (…)` before it (a column list looks like a call of the table's name), and
    // no `INSERT INTO FUNCTION`.
    let (head, tail) = words.split_at(select + 1);
    selects == 1
        && head
            .iter()
            .map(|(w, _)| w.as_str())
            .take(2)
            .eq(["INSERT", "INTO"])
        && !head.iter().any(|(w, _)| w == "FUNCTION")
        && !tail.iter().any(|(w, called)| {
            matches!(
                w.as_str(),
                "FROM" | "UNION" | "INTERSECT" | "EXCEPT" | "JOIN" | "ARRAY" | "VALUES" | "FORMAT"
            ) || (*called && !INSERT_CALLS.contains(&w.as_str()))
        })
}

/// The index (in `code`) after the possibly qualified name that starts at `from`.
fn table_end(s: &Scan, code: &[(usize, usize)], from: usize) -> Option<usize> {
    let is_name = |k: usize| {
        code.get(k)
            .is_some_and(|&(i, _)| matches!(s.tokens[i].kind, TokenKind::Word | TokenKind::Ident))
    };
    let is_dot = |k: usize| {
        code.get(k)
            .is_some_and(|&(i, _)| s.is_punct(&s.tokens[i], '.'))
    };
    if !is_name(from) {
        return None;
    }
    let mut k = from + 1;
    while is_dot(k) && is_name(k + 1) {
        k += 2;
    }
    Some(k)
}

/// The count of the rows an edit built by `build_change` would change: `SELECT count() FROM t
/// WHERE <its condition>`, with the same placeholders (send it with the edit's parameters).
/// None for any other statement: the driver then refuses to apply it with an expected count.
pub fn guard_count(sql: &str) -> Option<String> {
    let (s, code) = code_tokens(sql)?;
    let upper = |k: usize| {
        code.get(k)
            .map(|&(i, _)| &s.tokens[i])
            .filter(|t| t.kind == TokenKind::Word)
            .map(|t| s.upper(t))
    };
    let name_start = match (upper(0).as_deref(), upper(1).as_deref()) {
        (Some("ALTER"), Some("TABLE")) | (Some("DELETE"), Some("FROM")) => 2,
        (Some("UPDATE"), _) => 1,
        _ => return None,
    };
    let name_end = table_end(&s, &code, name_start)?;
    let next = upper(name_end)?;
    let expected = match upper(0).as_deref() {
        Some("ALTER") => ["UPDATE", "DELETE"].contains(&next.as_str()),
        Some("DELETE") => next == "WHERE",
        _ => next == "SET",
    };
    if !expected {
        return None;
    }
    // The first WHERE outside parentheses after the name.
    let at =
        (name_end..code.len()).find(|&k| code[k].1 == 0 && upper(k).as_deref() == Some("WHERE"))?;
    let char_at = |k: usize| s.tokens[code[k].0].start;
    let table: String = s.chars[char_at(name_start)..s.tokens[code[name_end - 1].0].end]
        .iter()
        .collect();
    let cond_start = s.tokens[code[at].0].end;
    let condition: String = s.chars[cond_start..].iter().collect();
    let condition = condition.trim().trim_end_matches(';').trim_end();
    if condition.is_empty() {
        return None;
    }
    Some(format!("SELECT count() FROM {table} WHERE {condition}"))
}

fn is_default(cell: &Cell<'_>) -> bool {
    matches!(
        cell,
        Cell::Value(Value::Default) | Cell::Expr(ServerExpr::Default)
    )
}

fn expr_sql(spec: &DialectSpec, expr: ServerExpr, col: &ColumnMeta) -> Result<String, AppError> {
    if let ServerExpr::Fsp(_, n) = expr
        && n > ServerExpr::MAX_FSP
    {
        return Err(AppError::InvalidInput(format!(
            "a time precision is 0 to {} digits, not {n}",
            ServerExpr::MAX_FSP
        )));
    }
    (spec.knobs.server_expr)(expr, col.kind).ok_or_else(|| {
        AppError::Unsupported(format!(
            "{} has no {} for \"{}\"",
            spec.label,
            expr.wire(),
            col.name
        ))
    })
}

fn no_default(spec: &DialectSpec, col: &ColumnMeta) -> AppError {
    AppError::Unsupported(format!(
        "{} cannot set \"{}\" to DEFAULT; enter the value explicitly",
        spec.label, col.name
    ))
}

/// `ALTER TABLE t UPDATE c = {p1:T}, … WHERE <key>`.
pub fn build_update(
    spec: &DialectSpec,
    t: &TableTarget<'_>,
    key: &RowKey,
    changes: &BTreeMap<&str, Cell<'_>>,
) -> Result<BuiltSql, AppError> {
    if changes.is_empty() {
        return Err(AppError::InvalidInput(
            "update has no changed columns".into(),
        ));
    }
    let mut values = Vec::new();
    let mut params = Params::new(spec, &mut values);
    let mut sets = Vec::new();
    for (name, cell) in changes {
        let col = find_column(t.columns, name)?;
        check_whole(spec, col, "be edited")?;
        if in_key(col) {
            return Err(AppError::Unsupported(format!(
                "\"{name}\" is part of the table's key (ORDER BY, PARTITION BY, PRIMARY KEY or \
                 SAMPLE BY), which ClickHouse cannot update"
            )));
        }
        if col.is_generated {
            return Err(AppError::Unsupported(format!(
                "\"{name}\" is computed by the server (MATERIALIZED or ALIAS) and cannot be set"
            )));
        }
        if is_default(cell) {
            return Err(no_default(spec, col));
        }
        let id = ident(spec, name);
        sets.push(match cell {
            Cell::Expr(e) => format!("{id} = {}", expr_sql(spec, *e, col)?),
            Cell::Value(v) => format!("{id} = {}", params.push((*v).clone(), Some(col))),
        });
    }
    let condition = row_condition(spec, t.columns, key, &mut params, false)?;
    Ok(BuiltSql {
        sql: format!(
            "ALTER TABLE {} UPDATE {} WHERE {condition}",
            table_name(spec, t.table),
            sets.join(", ")
        ),
        params: values,
    })
}

/// `INSERT INTO t (cols) SELECT {p1:T}, …`: the server parses each value as its column's type.
/// Columns left out (or set to DEFAULT) take their default.
pub fn build_insert(
    spec: &DialectSpec,
    t: &TableTarget<'_>,
    cells: &BTreeMap<&str, Cell<'_>>,
) -> Result<BuiltSql, AppError> {
    let mut values = Vec::new();
    let mut params = Params::new(spec, &mut values);
    let mut names = Vec::new();
    let mut exprs = Vec::new();
    for (name, cell) in cells {
        let col = find_column(t.columns, name)?;
        check_whole(spec, col, "be edited")?;
        if is_default(cell) {
            continue;
        }
        if col.is_generated {
            return Err(AppError::Unsupported(format!(
                "\"{name}\" is computed by the server (MATERIALIZED or ALIAS) and cannot be set"
            )));
        }
        names.push(ident(spec, name));
        exprs.push(match cell {
            Cell::Expr(e) => expr_sql(spec, *e, col)?,
            Cell::Value(v) => params.push((*v).clone(), Some(col)),
        });
    }
    if names.is_empty() {
        return Err(AppError::Unsupported(format!(
            "{} cannot insert a row of defaults only; enter at least one value",
            spec.label
        )));
    }
    Ok(BuiltSql {
        sql: format!(
            "INSERT INTO {} ({}) SELECT {}",
            table_name(spec, t.table),
            names.join(", "),
            exprs.join(", ")
        ),
        params: values,
    })
}

/// `DELETE FROM t WHERE <key>` (a lightweight delete).
pub fn build_delete(
    spec: &DialectSpec,
    t: &TableTarget<'_>,
    key: &RowKey,
) -> Result<BuiltSql, AppError> {
    let mut values = Vec::new();
    let condition = row_condition(
        spec,
        t.columns,
        key,
        &mut Params::new(spec, &mut values),
        false,
    )?;
    Ok(BuiltSql {
        sql: format!(
            "DELETE FROM {} WHERE {condition}",
            table_name(spec, t.table)
        ),
        params: values,
    })
}

/// The statement of one staged change; each must change exactly one row.
fn build_change(
    spec: &DialectSpec,
    t: &TableTarget<'_>,
    change: &StagedChange,
    _returning: bool,
) -> Result<(BuiltSql, Option<u64>), AppError> {
    let built = match change {
        StagedChange::Update {
            key,
            changes,
            exprs,
            ..
        } => build_update(spec, t, key, &cells(changes, exprs.as_ref()))?,
        StagedChange::Insert { values, exprs, .. } => {
            build_insert(spec, t, &cells(values, exprs.as_ref()))?
        }
        StagedChange::Delete { key, .. } => build_delete(spec, t, key)?,
    };
    Ok((built, Some(1)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlgen::portable::mutate;
    use crate::types::schema::TableRef;

    #[test]
    fn export_bindings_lex_clickhouse() {
        crate::sqlgen::dialect::lex::export_lex("clickhouse", &LEX, &RULES);
    }

    #[test]
    fn quotes_and_classifies() {
        assert_eq!((SPEC.quote_ident)("a`b\\"), "`a\\`b\\\\`");
        assert_eq!((SPEC.quote_literal)("it's \\"), "'it\\'s \\\\'");
        assert_eq!(
            classify_clickhouse("SELECT * FROM numbers(10)"),
            StatementClass::Read
        );
        for sql in [
            "SELECT * FROM system.tables",
            "SELECT name FROM system.columns WHERE database = 'db' AND table = 'kill'",
            "SHOW CREATE TABLE t",
            // A quoted name is no word of `writes`.
            "SELECT name, value FROM system.`settings` WHERE changed",
            // The clauses that end a FROM list do so before their own lists.
            "SELECT * FROM t WHERE a = 1 ORDER BY a, b",
            "SELECT count() FROM t GROUP BY a, lower(b)",
            "SELECT * FROM t LIMIT 1 BY a, lower(b)",
            "FROM t |> WHERE a > 1 |> AGGREGATE count() GROUP BY b",
            "SELECT dictGet('db.dict', 'name', toUInt64(1))",
            "SELECT icebergBucket(16, id) FROM t",
            "SELECT * FROM generate_series(1, 10)",
            // Unicode quotes: a string and names.
            "SELECT ‘a b’ AS s, “my col” FROM “my db”.“my table”",
        ] {
            assert_eq!(classify_clickhouse(sql), StatementClass::Read, "{sql}");
        }
        for sql in [
            // A quoted name matches the allow-list on its exact text only, and the list is
            // upper case: a known false write.
            "SELECT * FROM “numbers”(3)",
            "SELECT * FROM t INTO OUTFILE 'x.csv'",
            "SYSTEM DROP DNS CACHE",
            "KILL QUERY WHERE query_id = 'x'",
            "OPTIMIZE TABLE t FINAL",
            "ALTER TABLE t DELETE WHERE 1",
            "SELECT * FROM url('http://x', CSV)",
            "SELECT * FROM t, urlCluster('c', 'http://x')",
            "DESCRIBE TABLE s3('https://x')",
            "SELECT * FROM t JOIN remote('h', db.t) USING (id)",
            // A subquery in the FROM list ends, and the list goes on.
            "SELECT * FROM (SELECT * FROM t) AS b, url('http://x')",
            "SELECT * FROM t JOIN (SELECT * FROM u) b ON true, url('http://x')",
            // Keywords are names after `AS` and `.` and as operands, so they leave the list
            // open.
            "SELECT * FROM numbers(1) AS settings, url('http://evil/x', CSV, 'a String')",
            "SELECT * FROM numbers(1) AS format, url('http://evil/x', CSV, 'a String')",
            "SELECT * FROM numbers(1) AS where, url('http://evil/x', CSV, 'a String')",
            "SELECT * FROM t fetch, url('http://x')",
            "SELECT * FROM t JOIN u ON t.a = u.order, url('http://x')",
            "SELECT * FROM t JOIN u ON t.a = limit, url('http://x')",
            "SELECT * FROM t JOIN u ON limit = t.a, url('http://x')",
            "SELECT * FROM limit, url('http://x')",
            "SELECT file('secret.txt')",
            "SELECT * FROM file('secret.txt', 'CSV')",
            // Comments a reader may end elsewhere: the Lexer nests `/* */` and ends `--` only
            // at an LF.
            "/* /* */ SELECT ' */ SELECT * FROM url('http://evil/x', CSV) --'",
            "/* /* */ SELECT 1 */ ALTER TABLE t DELETE WHERE 1",
            "SELECT 1 -- x\r'\nFROM url('http://evil') --'",
            "SELECT 1 # x\r'\nFROM url('http://evil') --'",
            // `//` is a comment to ClickHouse: what follows it on the next line is code.
            "SELECT 1 // '\nFROM url('http://evil/x', CSV) --'",
            // The statements of PARALLEL WITH run side by side; one with a result is refused
            // only after it has run, so the others run too.
            "USE db PARALLEL WITH DROP TABLE t",
            "USE db",
            "SELECT 1 PARALLEL WITH DROP TABLE t",
            "SHOW TABLES PARALLEL WITH DROP TABLE t",
            "EXISTS t PARALLEL WITH DROP TABLE t",
            "DESCRIBE TABLE t PARALLEL WITH DROP TABLE t",
            "DESC t PARALLEL WITH TRUNCATE TABLE t",
            "EXPLAIN SELECT 1 PARALLEL WITH DROP TABLE t",
            "EXPLAIN AST SELECT 1 PARALLEL WITH DROP TABLE t",
            // EXPLAIN ANALYZE runs its statement.
            "EXPLAIN ANALYZE INSERT INTO t SELECT 1",
            "EXPLAIN ANALYZE SELECT 1",
            "SELECT 1 IN icebergLocalCluster('c', '/data/t')",
            // A table function where no table goes.
            "EXPLAIN TABLE OVERRIDE mysql('h:3306', 'db', 't', 'u', 'p') PARTITION BY a",
            "SELECT 1 IN remote('h', db.t)",
            "SELECT * FROM t WHERE x IN (SELECT 1 FROM s3('https://x'))",
            "SELECT * FROM kafka_t SETTINGS stream_like_engine_allow_direct_select = 1",
            // Settings whose values are SQL the server runs (red team, round 3), and with them
            // every SETTINGS clause (a known false write).
            "SELECT 1 SETTINGS select = 'file(''/etc/passwd'')'",
            "SELECT * FROM t SETTINGS filter = 'length(file(''/etc/passwd'')) > 0'",
            "SELECT * FROM t SETTINGS \"order\" = '(SELECT 1 FROM url(''http://x''))'",
            "SELECT * FROM t SETTINGS additional_result_filter = 'file(''/etc/passwd'') != '''''",
            "SELECT 1 SETTINGS implicit_table_at_top_level = 'url(''http://x'', CSV)', limit = 1",
            "SELECT * FROM t SETTINGS max_threads = 1, max_block_size = 2",
            "SELECT name FROM system.settings",
            "SHOW SETTINGS LIKE 'max_%'",
            "SELECT aiGenerate('hi')",
            "SELECT generateSerialID('orders')",
            "SELECT logTrace('x')",
            "SELECT * FROM eval('SELECT 1')",
            "FROM url('http://x') |> WHERE 1",
            "SELECT * FROM dictionary('d')",
            "SELECT * FROM executable('script.sh', TabSeparated, 'a String')",
            // A Unicode-quoted name is a callee, and the quotes, comments and heredocs inside
            // Unicode quotes open nothing (red team, round 2).
            "SELECT “file”('/etc/passwd')",
            "SELECT * FROM db.“s3”('https://b/x')",
            "SELECT ‘'’, file('/etc/passwd') --'",
            "SELECT “--” PARALLEL WITH DROP TABLE t",
            // No escapes in them: the name ends at the first closing quote.
            "SELECT “\\”, file('/etc/passwd') --”",
        ] {
            assert_eq!(classify_clickhouse(sql), StatementClass::Write, "{sql}");
        }
    }

    fn col(name: &str, ty: &str, pk: bool) -> ColumnMeta {
        let mut c: ColumnMeta = serde_json::from_value(serde_json::json!({
            "name": name, "ordinal": 1, "dataType": ty, "kind": "text",
            "nullable": ty.starts_with("Nullable"), "isPk": pk, "isAutoIncrement": false,
            "isGenerated": false, "isUnsigned": false
        }))
        .unwrap();
        if pk {
            c.extra = Some("primary key, sorting key".into());
        }
        c
    }

    fn columns() -> Vec<ColumnMeta> {
        let mut part = col("day", "Date", false);
        part.extra = Some("partition key".into());
        let mut mat = col("total", "UInt64", false);
        mat.is_generated = true;
        vec![
            col("id", "UInt64", true),
            col("name", "Nullable(String)", false),
            col("ts", "DateTime64(3, 'Asia/Jakarta')", false),
            part,
            mat,
            col("state", "AggregateFunction(uniq, UInt64)", false),
        ]
    }

    fn pk(id: i64) -> RowKey {
        RowKey::Pk(BTreeMap::from([("id".to_string(), Value::Int(id))]))
    }

    fn values(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn edits_are_mutations_lightweight_deletes_and_insert_selects() {
        let cols = columns();
        let table = TableRef::new(Some("my db"), "t`x");
        let t = TableTarget {
            table: &table,
            columns: &cols,
        };
        let update = StagedChange::Update {
            row_id: "r".into(),
            key: pk(7),
            changes: values(&[
                ("name", Value::Text("it's".into())),
                ("ts", Value::Timestamp("2024-01-01 00:00:00.000".into())),
            ]),
            exprs: None,
        };
        let (b, expect) = mutate::build_change(&SPEC, &t, &update, true).unwrap();
        assert_eq!(expect, Some(1));
        assert_eq!(
            b.sql,
            "ALTER TABLE `my db`.`t\\`x` UPDATE `name` = {p1:Nullable(String)}, `ts` = \
             {p2:DateTime64(3, 'Asia/Jakarta')} WHERE `id` = {p3:UInt64}"
        );
        assert_eq!(
            b.params,
            [
                Value::Text("it's".into()),
                Value::Timestamp("2024-01-01 00:00:00.000".into()),
                Value::Int(7)
            ]
        );
        assert_eq!(edit_kind(&b.sql), EditKind::Mutation);
        assert_eq!(
            guard_count(&b.sql).unwrap(),
            "SELECT count() FROM `my db`.`t\\`x` WHERE `id` = {p3:UInt64}"
        );

        let now = StagedChange::Update {
            row_id: "r".into(),
            key: pk(7),
            changes: BTreeMap::new(),
            exprs: Some(BTreeMap::from([(
                "ts".to_string(),
                ServerExpr::Fsp(Clock::Now, 3),
            )])),
        };
        let (b, _) = mutate::build_change(&SPEC, &t, &now, false).unwrap();
        assert!(b.sql.contains("`ts` = now64(3) WHERE"), "{}", b.sql);

        let insert = StagedChange::Insert {
            row_id: "n".into(),
            values: values(&[
                ("id", Value::Int(8)),
                ("name", Value::Null),
                ("day", Value::Default),
            ]),
            exprs: None,
        };
        let (b, _) = mutate::build_change(&SPEC, &t, &insert, true).unwrap();
        assert_eq!(
            b.sql,
            "INSERT INTO `my db`.`t\\`x` (`id`, `name`) SELECT {p1:UInt64}, {p2:Nullable(String)}"
        );
        assert_eq!(edit_kind(&b.sql), EditKind::Insert);
        assert_eq!(guard_count(&b.sql), None);
        assert!(inserts_one_row(&b.sql));
        assert!(inserts_one_row(
            "INSERT INTO t (`a`, `b`) SELECT {p1:UInt8}, now64(3)"
        ));
        for sql in [
            "INSERT INTO t SELECT * FROM s",
            "INSERT INTO t SELECT arrayJoin([1, 2])",
            "INSERT INTO t SELECT 1 UNION ALL SELECT 2",
            "INSERT INTO t VALUES (1), (2)",
            "INSERT INTO t (`a`) SELECT {p1:UInt8} -- x\n FROM numbers(2)",
            "INSERT INTO t (`m`) SELECT {'a': arrayJoin([1, 2])}",
            "INSERT INTO t (`m`) SELECT {p1:UInt8}, {p2:{p3:UInt8}}",
            "INSERT INTO FUNCTION url('http://x', CSV) (`a`) SELECT {p1:UInt8}",
            "ALTER TABLE t DELETE WHERE 1",
        ] {
            assert!(!inserts_one_row(sql), "{sql}");
        }

        let delete = StagedChange::Delete {
            row_id: "r".into(),
            key: pk(7),
        };
        let (b, _) = mutate::build_change(&SPEC, &t, &delete, false).unwrap();
        assert_eq!(
            b.sql,
            "DELETE FROM `my db`.`t\\`x` WHERE `id` = {p1:UInt64}"
        );
        assert_eq!(edit_kind(&b.sql), EditKind::LightweightDelete);
        assert_eq!(
            guard_count(&b.sql).unwrap(),
            "SELECT count() FROM `my db`.`t\\`x` WHERE `id` = {p1:UInt64}"
        );
    }

    #[test]
    fn edits_that_clickhouse_cannot_make_are_refused() {
        let cols = columns();
        let table = TableRef::new(None, "t");
        let t = TableTarget {
            table: &table,
            columns: &cols,
        };
        let update = |name: &str, v: Value| StagedChange::Update {
            row_id: "r".into(),
            key: pk(1),
            changes: values(&[(name, v)]),
            exprs: None,
        };
        for (change, why) in [
            (update("id", Value::Int(2)), "key"),
            (update("day", Value::Date("2024-01-01".into())), "key"),
            (update("total", Value::Int(2)), "computed"),
            (update("name", Value::Default), "DEFAULT"),
            (update("state", Value::Text("x".into())), "shortened"),
        ] {
            let e = mutate::build_change(&SPEC, &t, &change, false).unwrap_err();
            assert!(e.to_string().contains(why), "{why}: {e}");
        }
        // Without a primary key a row cannot be addressed.
        let keyless = StagedChange::Delete {
            row_id: "r".into(),
            key: RowKey::AllColumns(values(&[("name", Value::Text("x".into()))])),
        };
        assert!(mutate::build_change(&SPEC, &t, &keyless, false).is_err());
        let defaults = StagedChange::Insert {
            row_id: "n".into(),
            values: values(&[("name", Value::Default)]),
            exprs: None,
        };
        assert!(mutate::build_change(&SPEC, &t, &defaults, false).is_err());
    }

    #[test]
    fn guard_counts_only_of_edit_shapes() {
        assert_eq!(
            guard_count("UPDATE db.t SET a = {p1:UInt8} WHERE (id = {p2:UInt64}) AND x IN (SELECT 1 WHERE 1);").unwrap(),
            "SELECT count() FROM db.t WHERE (id = {p2:UInt64}) AND x IN (SELECT 1 WHERE 1)"
        );
        assert_eq!(
            edit_kind("UPDATE db.t SET a = 1 WHERE 1"),
            EditKind::LightweightUpdate
        );
        assert_eq!(
            guard_count("ALTER TABLE t DELETE WHERE `where` = 1").unwrap(),
            "SELECT count() FROM t WHERE `where` = 1"
        );
        for sql in [
            "SELECT 1",
            "ALTER TABLE t ADD COLUMN c UInt8",
            "ALTER TABLE t UPDATE a = 1",
            "DELETE FROM t",
            "DELETE FROM t WHERE ",
            "INSERT INTO t SELECT 1",
            "/* /* */ DELETE FROM t WHERE 1",
            "ALTER TABLE t ON CLUSTER c DELETE WHERE 1",
        ] {
            assert_eq!(guard_count(sql), None, "{sql}");
        }
        assert_eq!(
            edit_kind("ALTER TABLE t ADD COLUMN c UInt8"),
            EditKind::Other
        );
        assert_eq!(edit_kind("SELECT 1"), EditKind::Other);
        assert!(EditKind::Mutation.uncounted() && !EditKind::Insert.uncounted());
    }

    #[test]
    fn aggregate_states_reach_the_grid_as_text() {
        let cols = columns();
        let state = &cols[5];
        assert_eq!(
            (SPEC.knobs.text_projection.unwrap())("`state`", state).as_deref(),
            Some("toString(finalizeAggregation(`state`))")
        );
        assert!((SPEC.knobs.lossy_text)(state));
        assert_eq!(
            (SPEC.knobs.text_projection.unwrap())("`id`", &cols[0]),
            None
        );
    }
}
