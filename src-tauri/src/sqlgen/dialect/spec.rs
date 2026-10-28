//! `DialectSpec`: everything the portable builders (`sqlgen::portable`) need to know of an
//! extension dialect. Each lane writes one (`sqlgen/dialect/<d>.rs`, `pub static SPEC`); a
//! dialect this build lacks resolves to its `unavailable` spec, which refuses every builder.
//!
//! The fields that decide whether generated SQL is safe (quoting, literals, placeholders, the
//! parameter limit, the lexer and the classifier) are required: there is no base value to forget
//! to override. Only the optional behaviour starts from `SqlKnobs::ANSI`
//! (`knobs: SqlKnobs { …, ..SqlKnobs::ANSI }`).

use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::ddl::DdlPlanner;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile};
use crate::sqlgen::select::BuiltSql;
use crate::sqlgen::split::SplitStatement;
use crate::sqlgen::sql::{SelectQuery, TableTarget};
use crate::types::browse::{FilterGroup, RowKey};
use crate::types::mutate::{Clock, ServerExpr, StagedChange};
use crate::types::query::ExplainMode;
use crate::types::schema::{ColumnMeta, TableRef, ValueKind};
use crate::types::value::Value;

#[derive(Clone, Copy)]
pub struct DialectSpec {
    pub dialect: SqlDialect,
    /// The dialect's name in messages ("Transact-SQL").
    pub label: &'static str,
    /// Quotes an identifier; always quotes and escapes, never trusts the input.
    pub quote_ident: fn(&str) -> String,
    /// A string literal for display (Preview SQL); execution binds parameters.
    pub quote_literal: fn(&str) -> String,
    /// A string literal in SQL that runs without bind parameters, which the server reads back
    /// exactly.
    pub quote_literal_exec: fn(&str) -> String,
    /// The n-th (1-based) bind parameter, for a value of `column` when there is one (typed
    /// placeholders such as ClickHouse's `{p1:Int64}` need its type).
    pub placeholder: PlaceholderFn,
    pub placeholder_style: PlaceholderStyle,
    /// A placeholder may appear twice for one value (`$1`, `@P1`); a positional `?` cannot.
    pub placeholders_reusable: bool,
    /// Bind parameters one statement may carry (below the server's limit).
    pub max_params: u32,
    /// Parts of the longest qualified name (`db.schema.table` is 3).
    pub max_name_parts: u8,
    pub lex: &'static LexProfile,
    /// The portable classifier's rules. `classify` usually decides with them, and
    /// `is_single_statement` looks in them for the words that may start a second statement where
    /// terminators are optional.
    pub rules: &'static ClassifyRules,
    /// Safe Mode's READ / WRITE, default-deny (an unknown statement is a write). Usually
    /// `portable::classify::with(&LEX, &RULES, sql)`.
    pub classify: fn(&str) -> StatementClass,
    pub knobs: SqlKnobs,
    pub hooks: SqlHooks,
    /// The structure editor's planner; None: tables of this dialect cannot be altered.
    pub ddl: Option<&'static dyn DdlPlanner>,
}

impl std::fmt::Debug for DialectSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DialectSpec")
            .field("dialect", &self.dialect)
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

/// How a statement marks its bind parameters (for `inline_params`, which finds them again).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceholderStyle {
    /// `$1`
    Dollar,
    /// `?`, by position.
    Question,
    /// `?1`
    QuestionNumbered,
    /// `@P1` (T-SQL through `sp_executesql`).
    AtP,
    /// `@p1` (BigQuery named parameters).
    AtName,
    /// `:1`
    Colon,
    /// `{p1:Type}` (ClickHouse).
    Typed,
}

/// How a page of rows is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paging {
    /// `LIMIT n OFFSET m`
    LimitOffset,
    /// `ORDER BY … OFFSET m ROWS FETCH NEXT n ROWS ONLY` (an ORDER BY is required).
    OffsetFetch,
    /// `LIMIT n` and no offset: only the first page (later pages need a cursor).
    LimitOnly,
}

/// How a query is limited to one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitOne {
    /// `… LIMIT 1`
    Limit,
    /// `SELECT TOP (1) …`
    Top,
}

/// How a sort puts NULLs first or last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NullsSort {
    /// `NULLS FIRST` / `NULLS LAST`
    Native,
    /// A `CASE WHEN x IS NULL …` sort key in front of the column.
    NullFlag,
    /// The order of NULLs cannot be chosen.
    Unsupported,
}

/// How a case-insensitive pattern match is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiLike {
    /// `x ILIKE p`
    Ilike,
    /// `LOWER(x) LIKE LOWER(p)`
    LowerLike,
    Unsupported,
}

/// How a NULL-safe equality is written (a key or row match where NULL equals NULL).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NullSafeEq {
    /// `x IS NOT DISTINCT FROM p`
    IsNotDistinctFrom,
    /// `(x = p OR (x IS NULL AND p IS NULL))`
    OrForm,
}

/// How an UPDATE or INSERT returns the row it wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Returning {
    /// It cannot: the caller reads the row again.
    None,
    /// `… RETURNING cols`
    Returning,
    /// `… OUTPUT INSERTED.cols …` (before VALUES / WHERE).
    OutputInserted,
}

/// What a staged edit may address a row by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowLocator {
    /// Its primary key only.
    KeyOnly,
    /// Its primary key, or the `rowid` of a table without one.
    Rowid,
}

/// The EXPLAIN of a mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplainForm {
    /// The text put in front of the statement (without the separating space).
    Prefix(&'static str),
    Unsupported,
}

/// A password keyword of the trace's redaction: the first string literal after `keyword` is
/// masked, past `=`, `(`, `BY`, `AS`, `WITH`, `USING` and at most one other word
/// (`IDENTIFIED WITH sha256_password BY '…'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedactRule {
    /// Upper case.
    pub keyword: &'static str,
}

/// The optional behaviour of a dialect. Lanes start from `SqlKnobs::ANSI`.
#[derive(Clone, Copy)]
pub struct SqlKnobs {
    /// `[false, true]`
    pub bool_literal: [&'static str; 2],
    /// A bytes literal from its hex digits (Preview SQL).
    pub bytes_literal: fn(&str) -> String,
    /// The select-list expression of a column (its quoted name given) when the server should
    /// send it as text, or None to select it as it is. The rows of a column with a projection
    /// arrive as text (`sql::column_form`). Redshift: `{e}::varchar(65535)`.
    pub text_projection: Option<fn(&str, &ColumnMeta) -> Option<String>>,
    /// The projection of the column loses data (cuts or rounds it): the column cannot be
    /// edited, used as a key or loaded whole, since a later edit would write the cut value back.
    pub lossy_text: fn(&ColumnMeta) -> bool,
    /// A column (its quoted name given) as text, for pattern filters and "Any column"; None
    /// leaves the column out of them.
    pub text_expr: fn(&str, &ColumnMeta) -> Option<String>,
    /// Wraps a placeholder for a value of a column (PostgreSQL-like `$1::type`).
    pub param_cast: Option<fn(String, &ColumnMeta) -> String>,
    pub paging: Paging,
    pub limit_one: LimitOne,
    /// The count of a table count (`count(*)`, `COUNT_BIG(*)`, `count()`).
    pub count_expr: &'static str,
    pub nulls_order: NullsSort,
    pub ci_like: CiLike,
    /// The escape character of generated LIKE patterns (`ESCAPE '!'`); None: the dialect's LIKE
    /// takes a backslash and has no ESCAPE clause.
    pub like_escape: Option<char>,
    /// The characters LIKE reads as wildcards (T-SQL adds `[`).
    pub like_wildcards: &'static [char],
    pub null_safe_eq: NullSafeEq,
    /// A JSON containment test of a column and a placeholder; None: no "JSON contains".
    pub json_contains: Option<fn(&str, &str) -> String>,
    pub returning: Returning,
    /// What follows `INSERT INTO t` for a row of defaults only (`DEFAULT VALUES`); None: such
    /// an insert is refused.
    pub insert_all_defaults: Option<&'static str>,
    /// `SET c = DEFAULT` works.
    pub set_default: bool,
    /// The SQL of a server expression for a column of a kind; None: not available.
    /// `ServerExpr::Default` never reaches it.
    pub server_expr: fn(ServerExpr, ValueKind) -> Option<String>,
    pub row_locator: RowLocator,
    pub explain: fn(ExplainMode) -> ExplainForm,
    /// (open, close) around an executing Explain Analyze outside a transaction, so its changes
    /// are undone; None: such an Analyze runs only for a statement that does not write.
    pub analyze_tx: Option<(&'static str, &'static str)>,
    /// The statement commits an open transaction (`tx_effect`).
    pub implicit_commit: fn(&str) -> bool,
    /// The statement switches the session's database (the MCP gate refuses it).
    pub switch_db: fn(&str) -> bool,
    /// Catalog names that mark a read as introspection in the trace (upper case).
    pub catalog_words: &'static [&'static str],
    /// Functions a read calls that change the server or wait (the Metrics Board refuses them).
    /// An entry of several words is those words in a row (T-SQL `NEXT VALUE FOR`).
    pub side_effect_functions: &'static [&'static str],
    pub redact: &'static [RedactRule],
    /// Commands that carry a password after their first argument (cqlsh's `LOGIN user
    /// 'password'`), upper case: the string literal after the word, quoted name or string that
    /// follows one is masked, only where the command starts a statement, so a column of that name
    /// (`WHERE login = 'bob'`) keeps its literal.
    pub redact_commands: &'static [&'static str],
    /// The end of the row-cap cancel notice: what became of the statement's changes.
    pub cancel_notice: &'static str,
}

impl SqlKnobs {
    pub const ANSI: SqlKnobs = SqlKnobs {
        bool_literal: ["FALSE", "TRUE"],
        bytes_literal: hex_x,
        text_projection: None,
        lossy_text: never_lossy,
        text_expr: cast_varchar,
        param_cast: None,
        paging: Paging::LimitOffset,
        limit_one: LimitOne::Limit,
        count_expr: "count(*)",
        nulls_order: NullsSort::Native,
        ci_like: CiLike::LowerLike,
        like_escape: Some('!'),
        like_wildcards: &['%', '_'],
        null_safe_eq: NullSafeEq::IsNotDistinctFrom,
        json_contains: None,
        returning: Returning::None,
        insert_all_defaults: Some("DEFAULT VALUES"),
        set_default: true,
        server_expr: ansi_server_expr,
        row_locator: RowLocator::KeyOnly,
        explain: explain_plan_only,
        analyze_tx: None,
        implicit_commit: never,
        switch_db: starts_with_use,
        catalog_words: &[],
        side_effect_functions: &[],
        redact: &[],
        redact_commands: &[],
        cancel_notice: "any changes it made were rolled back",
    };
}

/// The n-th placeholder for a value of a column (`DialectSpec::placeholder`).
pub type PlaceholderFn = fn(usize, Option<&ColumnMeta>) -> String;
/// `portable::select::build_select`'s place.
pub type SelectHook = fn(&DialectSpec, &SelectQuery<'_>) -> Result<BuiltSql, AppError>;
/// `portable::select::build_count`'s place.
pub type CountHook =
    fn(&DialectSpec, &TableRef, &[ColumnMeta], Option<&FilterGroup>) -> Result<BuiltSql, AppError>;
/// `portable::select::build_fetch_cell`'s place.
pub type FetchCellHook = fn(
    &DialectSpec,
    &TableTarget<'_>,
    &ColumnMeta,
    &RowKey,
) -> Result<(String, Vec<Value>), AppError>;
/// `portable::mutate::build_change`'s place.
pub type ChangeHook = fn(
    &DialectSpec,
    &TableTarget<'_>,
    &StagedChange,
    bool,
) -> Result<(BuiltSql, Option<u64>), AppError>;

/// Whole-builder escape hatches of a dialect whose SQL the portable builders cannot shape.
#[derive(Clone, Copy)]
pub struct SqlHooks {
    pub build_select: Option<SelectHook>,
    pub build_count: Option<CountHook>,
    pub build_fetch_cell: Option<FetchCellHook>,
    pub build_change: Option<ChangeHook>,
    pub split: Option<fn(&str) -> Vec<SplitStatement>>,
}

impl SqlHooks {
    pub const NONE: SqlHooks = SqlHooks {
        build_select: None,
        build_count: None,
        build_fetch_cell: None,
        build_change: None,
        split: None,
    };
}

// Quoting, literal and placeholder forms the dialects share.

/// `"name"`, `"` doubled.
pub fn double_quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// `[name]`, `]` doubled.
pub fn bracketed(s: &str) -> String {
    format!("[{}]", s.replace(']', "]]"))
}

/// `` `name` ``, `` ` `` doubled.
pub fn backticked(s: &str) -> String {
    format!("`{}`", s.replace('`', "``"))
}

/// `` `name` `` where a backslash escapes (ClickHouse, BigQuery): backslash and backtick are
/// escaped with one.
pub fn backticked_backslash(s: &str) -> String {
    format!("`{}`", s.replace('\\', "\\\\").replace('`', "\\`"))
}

/// `'text'`, `'` doubled; backslashes are plain.
pub fn single_quoted(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// `N'text'` (T-SQL Unicode text), `'` doubled.
pub fn national_quoted(s: &str) -> String {
    format!("N'{}'", s.replace('\'', "''"))
}

/// `'text'` where a backslash escapes (ClickHouse, Snowflake, BigQuery): backslash, quote and
/// line breaks are written as escapes, which every one of them reads back.
pub fn single_quoted_backslash(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// `X'00ff'`
pub fn hex_x(hex: &str) -> String {
    format!("X'{hex}'")
}

/// `0x00ff`
pub fn hex_0x(hex: &str) -> String {
    format!("0x{hex}")
}

pub fn ph_dollar(n: usize, _: Option<&ColumnMeta>) -> String {
    format!("${n}")
}

pub fn ph_question(_: usize, _: Option<&ColumnMeta>) -> String {
    "?".into()
}

pub fn ph_question_numbered(n: usize, _: Option<&ColumnMeta>) -> String {
    format!("?{n}")
}

pub fn ph_at_p(n: usize, _: Option<&ColumnMeta>) -> String {
    format!("@P{n}")
}

pub fn ph_at_name(n: usize, _: Option<&ColumnMeta>) -> String {
    format!("@p{n}")
}

pub fn ph_colon(n: usize, _: Option<&ColumnMeta>) -> String {
    format!(":{n}")
}

// The `SqlKnobs::ANSI` defaults.

pub fn never_lossy(_: &ColumnMeta) -> bool {
    false
}

pub fn never(_: &str) -> bool {
    false
}

/// `CAST(e AS VARCHAR)`
pub fn cast_varchar(e: &str, _: &ColumnMeta) -> Option<String> {
    Some(format!("CAST({e} AS VARCHAR)"))
}

/// The SQL-standard clocks: `CURRENT_TIMESTAMP[(n)]` (also for "now"), `CURRENT_TIME[(n)]`,
/// `LOCALTIMESTAMP[(n)]`, `CURRENT_DATE`.
pub fn ansi_server_expr(expr: ServerExpr, _: ValueKind) -> Option<String> {
    if expr == ServerExpr::CurrentDate {
        return Some("CURRENT_DATE".into());
    }
    let (clock, fsp) = expr.clock()?;
    let keyword = match clock {
        Clock::Now => Clock::CurrentTimestamp.keyword(),
        c => c.keyword(),
    };
    Some(match fsp {
        Some(n) => format!("{keyword}({n})"),
        None => keyword.into(),
    })
}

/// `EXPLAIN` plans; nothing executes a statement to explain it.
pub fn explain_plan_only(mode: ExplainMode) -> ExplainForm {
    match mode {
        ExplainMode::Plan => ExplainForm::Prefix("EXPLAIN"),
        ExplainMode::Analyze => ExplainForm::Unsupported,
    }
}

pub fn explain_unsupported(_: ExplainMode) -> ExplainForm {
    ExplainForm::Unsupported
}

/// `USE …` switches the database.
pub fn starts_with_use(sql: &str) -> bool {
    crate::sqlgen::split::leading_keywords(sql, 1)
        .first()
        .is_some_and(|w| w == "USE")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_keeps_injections_inside_the_quotes() {
        assert_eq!(double_quoted("a\"b"), "\"a\"\"b\"");
        assert_eq!(bracketed("a]b"), "[a]]b]");
        assert_eq!(backticked("a`b"), "`a``b`");
        assert_eq!(backticked_backslash("a`b\\"), "`a\\`b\\\\`");
        assert_eq!(single_quoted("it's \\"), "'it''s \\'");
        assert_eq!(national_quoted("it's"), "N'it''s'");
        assert_eq!(
            single_quoted_backslash("it's C:\\x\n"),
            "'it\\'s C:\\\\x\\n'"
        );
    }

    #[test]
    fn ansi_clocks() {
        let at = |e| ansi_server_expr(e, ValueKind::Timestamp);
        assert_eq!(at(ServerExpr::Now).as_deref(), Some("CURRENT_TIMESTAMP"));
        assert_eq!(
            at(ServerExpr::Fsp(Clock::Now, 3)).as_deref(),
            Some("CURRENT_TIMESTAMP(3)")
        );
        assert_eq!(at(ServerExpr::CurrentDate).as_deref(), Some("CURRENT_DATE"));
        assert_eq!(
            at(ServerExpr::LocalTimestamp).as_deref(),
            Some("LOCALTIMESTAMP")
        );
        assert_eq!(at(ServerExpr::Default), None);
        assert!(starts_with_use("  use other") && !starts_with_use("SELECT 1"));
    }
}
