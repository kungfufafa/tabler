//! The spec of an extension dialect this build does not have (its lane's feature is off): every
//! builder refuses with "<dialect> is not built into this Tabler", every statement classifies as
//! a write and nothing can be bound. The quoting it still answers (ANSI double quotes) only ever
//! reaches display text.

use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, PLAIN};
use crate::sqlgen::dialect::spec::{
    DialectSpec, PlaceholderStyle, SqlHooks, SqlKnobs, double_quoted, explain_unsupported,
    ph_question, single_quoted,
};
use crate::sqlgen::select::BuiltSql;
use crate::sqlgen::sql::{SelectQuery, TableTarget};
use crate::types::browse::{FilterGroup, RowKey};
use crate::types::mutate::StagedChange;
use crate::types::schema::{ColumnMeta, TableRef};
use crate::types::value::Value;

const LEX: LexProfile = LexProfile {
    ident_quotes: &[('"', '"')],
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
    nested_block_comments: false,
    dash_comment_needs_space: false,
    batch_separator: None,
    terminator_optional: false,
    begin_end_bodies: false,
    attribute_calls: false,
};

/// Nothing reads (`classify` answers WRITE without them anyway).
const RULES: ClassifyRules = ClassifyRules {
    queries: &[],
    inspections: &[],
    writes: &[],
    write_calls: &[],
    table_functions: None,
};

const fn spec(dialect: SqlDialect, label: &'static str) -> DialectSpec {
    DialectSpec {
        dialect,
        label,
        quote_ident: double_quoted,
        quote_literal: single_quoted,
        quote_literal_exec: single_quoted,
        placeholder: ph_question,
        placeholder_style: PlaceholderStyle::Question,
        placeholders_reusable: false,
        max_params: 0,
        max_name_parts: 2,
        lex: &LEX,
        rules: &RULES,
        classify: write,
        knobs: SqlKnobs {
            explain: explain_unsupported,
            ..SqlKnobs::ANSI
        },
        hooks: SqlHooks {
            build_select: Some(refuse_select),
            build_count: Some(refuse_count),
            build_fetch_cell: Some(refuse_fetch_cell),
            build_change: Some(refuse_change),
            split: None,
        },
        ddl: None,
    }
}

static TSQL: DialectSpec = spec(SqlDialect::Tsql, "Transact-SQL");
static CLICKHOUSE: DialectSpec = spec(SqlDialect::Clickhouse, "ClickHouse SQL");
static SNOWFLAKE: DialectSpec = spec(SqlDialect::Snowflake, "Snowflake SQL");
static BIGQUERY: DialectSpec = spec(SqlDialect::Bigquery, "GoogleSQL");
static DUCKDB: DialectSpec = spec(SqlDialect::Duckdb, "DuckDB SQL");
static VERTICA: DialectSpec = spec(SqlDialect::Vertica, "Vertica SQL");
static CQL: DialectSpec = spec(SqlDialect::Cql, "CQL");
static REDSHIFT: DialectSpec = spec(SqlDialect::Redshift, "Redshift SQL");

/// The unavailable spec of an extension dialect; None for the core three, which are always
/// built.
pub fn of(d: SqlDialect) -> Option<&'static DialectSpec> {
    match d {
        SqlDialect::Postgres | SqlDialect::Mysql | SqlDialect::Sqlite => None,
        SqlDialect::Tsql => Some(&TSQL),
        SqlDialect::Clickhouse => Some(&CLICKHOUSE),
        SqlDialect::Snowflake => Some(&SNOWFLAKE),
        SqlDialect::Bigquery => Some(&BIGQUERY),
        SqlDialect::Duckdb => Some(&DUCKDB),
        SqlDialect::Vertica => Some(&VERTICA),
        SqlDialect::Cql => Some(&CQL),
        SqlDialect::Redshift => Some(&REDSHIFT),
    }
}

/// Whether `spec` is the unavailable spec of its dialect.
pub fn is(spec: &DialectSpec) -> bool {
    of(spec.dialect).is_some_and(|u| std::ptr::eq(u, spec))
}

pub fn not_built(spec: &DialectSpec) -> AppError {
    AppError::Unsupported(format!("{} is not built into this Tabler", spec.label))
}

fn write(_: &str) -> StatementClass {
    StatementClass::Write
}

fn refuse_select(spec: &DialectSpec, _: &SelectQuery<'_>) -> Result<BuiltSql, AppError> {
    Err(not_built(spec))
}

fn refuse_count(
    spec: &DialectSpec,
    _: &TableRef,
    _: &[ColumnMeta],
    _: Option<&FilterGroup>,
) -> Result<BuiltSql, AppError> {
    Err(not_built(spec))
}

fn refuse_fetch_cell(
    spec: &DialectSpec,
    _: &TableTarget<'_>,
    _: &ColumnMeta,
    _: &RowKey,
) -> Result<(String, Vec<Value>), AppError> {
    Err(not_built(spec))
}

fn refuse_change(
    spec: &DialectSpec,
    _: &TableTarget<'_>,
    _: &StagedChange,
    _: bool,
) -> Result<(BuiltSql, Option<u64>), AppError> {
    Err(not_built(spec))
}
