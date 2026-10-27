//! The live-SQL API of shared code, for every `SqlDialect`. A core dialect goes to the core
//! builders it always used (`select.rs`, `mutate.rs`, `split.rs`, `classify.rs`, `explain.rs`,
//! `drivers/redact.rs`), with the same arguments, so its SQL is byte-identical (the
//! `sqlgen::golden` corpus); an extension dialect goes to the `sqlgen::portable` builders of its
//! spec, or is refused when this build lacks it.
//!
//! `column_form` and `fetch_form` tell the decoders how the rows of `select` and `fetch_cell`
//! arrive (as text, as hex…), keyed on the dialect the SQL was built for rather than the driver.

use crate::drivers::redact::{Redaction, redact_secrets};
use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::{DialectRef, SqlDialect, ensure_built};
use crate::sqlgen::mutate::TableSpec;
use crate::sqlgen::select::{BuiltSql, SelectSpec};
use crate::sqlgen::split::SplitStatement;
use crate::sqlgen::{Dialect, classify, explain, mutate, portable, select, split, transfer};
use crate::types::browse::{FilterGroup, RowIdentity, RowKey, SortSpec};
use crate::types::mutate::StagedChange;
use crate::types::query::ExplainMode;
use crate::types::schema::{ColumnMeta, TableRef, ValueKind};
use crate::types::value::Value;

/// A browse query: `select::SelectSpec` without its dialect.
#[derive(Debug, Clone, Copy)]
pub struct SelectQuery<'a> {
    pub table: &'a TableRef,
    /// All columns of the table (for types and validation).
    pub columns: &'a [ColumnMeta],
    /// Subset to select; `None` = all.
    pub selected: Option<&'a [String]>,
    pub filter: Option<&'a FilterGroup>,
    pub sort: &'a [SortSpec],
    pub limit: u32,
    pub offset: u64,
    pub pk: &'a [String],
    pub identity: RowIdentity,
}

impl<'a> SelectQuery<'a> {
    fn spec(&self, dialect: Dialect) -> SelectSpec<'a> {
        SelectSpec {
            dialect,
            table: self.table,
            columns: self.columns,
            selected: self.selected,
            filter: self.filter,
            sort: self.sort,
            limit: self.limit,
            offset: self.offset,
            pk: self.pk,
            identity: self.identity,
        }
    }
}

/// A table and its columns: `mutate::TableSpec` without its dialect.
#[derive(Debug, Clone, Copy)]
pub struct TableTarget<'a> {
    pub table: &'a TableRef,
    pub columns: &'a [ColumnMeta],
}

impl<'a> TableTarget<'a> {
    fn spec(&self, dialect: Dialect) -> TableSpec<'a> {
        TableSpec {
            dialect,
            table: self.table,
            columns: self.columns,
        }
    }
}

/// The paged SELECT of a table browse.
pub fn select(d: impl Into<SqlDialect>, q: &SelectQuery<'_>) -> Result<BuiltSql, AppError> {
    match d.into().resolve() {
        DialectRef::Core(dialect) => select::build_select(&q.spec(dialect)),
        DialectRef::Ext(spec) => portable::select::build_select(ensure_built(spec)?, q),
    }
}

/// `SELECT count(*) FROM table WHERE …`.
pub fn count(
    d: impl Into<SqlDialect>,
    table: &TableRef,
    columns: &[ColumnMeta],
    filter: Option<&FilterGroup>,
) -> Result<BuiltSql, AppError> {
    match d.into().resolve() {
        DialectRef::Core(dialect) => select::build_count(dialect, table, columns, filter),
        DialectRef::Ext(spec) => {
            portable::select::build_count(ensure_built(spec)?, table, columns, filter)
        }
    }
}

/// The query of one whole cell (the inspector's "load full value"); `fetch_form` says how it
/// comes back.
pub fn fetch_cell(
    d: impl Into<SqlDialect>,
    t: &TableTarget<'_>,
    column: &ColumnMeta,
    key: &RowKey,
) -> Result<(String, Vec<Value>), AppError> {
    match d.into().resolve() {
        DialectRef::Core(dialect) => select::build_fetch_cell(&t.spec(dialect), column, key),
        DialectRef::Ext(spec) => {
            portable::select::build_fetch_cell(ensure_built(spec)?, t, column, key)
        }
    }
}

/// The statement of one staged change and how many rows it must affect.
pub fn change(
    d: impl Into<SqlDialect>,
    t: &TableTarget<'_>,
    change: &StagedChange,
    returning: bool,
) -> Result<(BuiltSql, Option<u64>), AppError> {
    match d.into().resolve() {
        DialectRef::Core(dialect) => mutate::build_change(&t.spec(dialect), change, returning),
        DialectRef::Ext(spec) => {
            portable::mutate::build_change(ensure_built(spec)?, t, change, returning)
        }
    }
}

/// A value as a SQL literal, for display only.
pub fn literal(d: impl Into<SqlDialect>, v: &Value) -> String {
    match d.into().resolve() {
        DialectRef::Core(dialect) => select::literal(dialect, v),
        DialectRef::Ext(spec) => portable::literal::literal(spec, v),
    }
}

/// `sql` with its placeholders replaced by literals, for display only.
pub fn inline_params(d: impl Into<SqlDialect>, sql: &str, params: &[Value]) -> String {
    match d.into().resolve() {
        DialectRef::Core(dialect) => select::inline_params(dialect, sql, params),
        DialectRef::Ext(spec) => portable::literal::inline_params(spec, sql, params),
    }
}

/// One statement in the dialect's EXPLAIN (`mariadb`: the MySQL dialect's MariaDB forms).
pub fn wrap_explain(
    d: impl Into<SqlDialect>,
    sql: &str,
    mode: ExplainMode,
    mariadb: bool,
) -> Result<String, AppError> {
    match d.into().resolve() {
        DialectRef::Core(dialect) => Ok(explain::wrap_explain(dialect, sql, mode, mariadb)),
        DialectRef::Ext(spec) => portable::explain::wrap_explain(ensure_built(spec)?, sql, mode),
    }
}

/// Characters `wrap_explain` adds before the statement.
pub fn explain_offset(d: impl Into<SqlDialect>, mode: ExplainMode, mariadb: bool) -> usize {
    match d.into().resolve() {
        DialectRef::Core(dialect) => explain::explain_offset(dialect, mode, mariadb),
        DialectRef::Ext(spec) => portable::explain::explain_offset(spec, mode),
    }
}

/// What an executing Explain Analyze runs in so that its changes are undone.
pub fn analyze_wrapper(
    d: impl Into<SqlDialect>,
    in_tx: Option<bool>,
    writes: bool,
) -> Result<Option<(String, Vec<String>)>, AppError> {
    match d.into().resolve() {
        DialectRef::Core(dialect) => explain::analyze_wrapper(dialect, in_tx, writes),
        DialectRef::Ext(spec) => {
            portable::explain::analyze_wrapper(ensure_built(spec)?, in_tx, writes)
        }
    }
}

/// Splits editor text into statements.
pub fn split_statements(d: impl Into<SqlDialect>, text: &str) -> Vec<SplitStatement> {
    match d.into().resolve() {
        DialectRef::Core(dialect) => split::split_statements(text, dialect),
        DialectRef::Ext(spec) => match spec.hooks.split {
            Some(split) => split(text),
            None => portable::split::split(spec.lex, text),
        },
    }
}

/// Whether `sql` holds at most one statement, however the server reads its backslashes.
pub fn is_single_statement(d: impl Into<SqlDialect>, sql: &str) -> bool {
    match d.into().resolve() {
        DialectRef::Core(dialect) => split::is_single_statement(sql, dialect),
        DialectRef::Ext(spec) => portable::split::is_single_statement(spec.lex, spec.rules, sql),
    }
}

/// `sql` with strings, quoted names and comments blanked, for keyword checks.
pub fn code_only(d: impl Into<SqlDialect>, sql: &str) -> String {
    match d.into().resolve() {
        DialectRef::Core(dialect) => split::code_only(sql, dialect),
        DialectRef::Ext(spec) => portable::classify::code_only(spec.lex, sql),
    }
}

/// Safe Mode's READ / WRITE of one statement or several.
pub fn classify(d: impl Into<SqlDialect>, sql: &str) -> StatementClass {
    match d.into().resolve() {
        DialectRef::Core(dialect) => classify::classify(sql, dialect),
        DialectRef::Ext(spec) => (spec.classify)(sql),
    }
}

/// `true` unless `sql` is a READ.
pub fn writes(d: impl Into<SqlDialect>, sql: &str) -> bool {
    classify(d, sql) == StatementClass::Write
}

/// Whether a read calls a function that changes the server or waits (the Metrics Board refuses
/// it).
pub fn calls_side_effect_function(d: impl Into<SqlDialect>, sql: &str) -> bool {
    match d.into().resolve() {
        DialectRef::Core(dialect) => classify::calls_side_effect_function(sql, dialect),
        DialectRef::Ext(spec) => portable::classify::calls_listed_function(
            spec.lex,
            spec.knobs.side_effect_functions,
            sql,
        ),
    }
}

/// `sql` with the passwords it carries masked, for the trace.
pub fn redact(d: impl Into<SqlDialect>, sql: &str) -> Redaction {
    match d.into().resolve() {
        DialectRef::Core(dialect) => redact_secrets(sql, dialect),
        DialectRef::Ext(spec) => portable::redact::redact(spec.lex, spec.knobs.redact, sql),
    }
}

/// Bind parameters one statement may carry.
pub fn max_params(d: impl Into<SqlDialect>) -> usize {
    match d.into().resolve() {
        DialectRef::Core(dialect) => transfer::max_params(dialect),
        DialectRef::Ext(spec) => spec.max_params as usize,
    }
}

/// How the rows of `select` carry a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnForm {
    /// In the column's own type, as the driver decodes it.
    Native,
    /// As text the server cast it to (PostgreSQL `::text`, a spec's projection).
    Text,
    /// As text that may be cut: the column cannot be edited, keyed on or loaded whole.
    LossyText,
}

/// How `select` returns `column` in dialect `d`.
pub fn column_form(d: impl Into<SqlDialect>, column: &ColumnMeta) -> ColumnForm {
    match d.into().resolve() {
        DialectRef::Core(Dialect::Postgres) => ColumnForm::Text,
        DialectRef::Core(Dialect::Mysql | Dialect::Sqlite) => ColumnForm::Native,
        DialectRef::Ext(spec) => {
            let id = (spec.quote_ident)(&column.name);
            match portable::select::projection(spec, &id, column) {
                None => ColumnForm::Native,
                Some(_) if portable::select::is_lossy(spec, column) => ColumnForm::LossyText,
                Some(_) => ColumnForm::Text,
            }
        }
    }
}

/// How the row of `fetch_cell` carries the cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchForm {
    /// One column in the column's own type.
    Native,
    /// One column of text.
    Text,
    /// One column of hex text (a bytes column; NULL stays NULL).
    Hex,
    /// Two columns: the value, then its hex when it is a blob (SQLite's dynamic types).
    ValueAndHex,
}

/// How `fetch_cell` returns `column` in dialect `d`.
pub fn fetch_form(d: impl Into<SqlDialect>, column: &ColumnMeta) -> FetchForm {
    let bytes = column.kind == ValueKind::Bytes;
    match d.into().resolve() {
        DialectRef::Core(Dialect::Postgres | Dialect::Mysql) if bytes => FetchForm::Hex,
        DialectRef::Core(Dialect::Postgres) => FetchForm::Text,
        DialectRef::Core(Dialect::Mysql) => FetchForm::Native,
        DialectRef::Core(Dialect::Sqlite) => FetchForm::ValueAndHex,
        DialectRef::Ext(spec) => {
            let id = (spec.quote_ident)(&column.name);
            match portable::select::projection(spec, &id, column) {
                Some(_) => FetchForm::Text,
                None => FetchForm::Native,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fmt::Debug;

    use super::*;
    use crate::sqlgen::dialect::testing::override_spec;
    use crate::sqlgen::portable::testing::{TSQL_LIKE_TEST, columns, map, text};
    use crate::types::browse::{Conjunction, FilterNode, FilterOp, NullsOrder, SortDir};
    use crate::types::mutate::ServerExpr;

    const CORE: [Dialect; 3] = [Dialect::Postgres, Dialect::Mysql, Dialect::Sqlite];

    /// The facade's result and the core builder's are the same.
    fn same<T: Debug>(facade: T, core: T) {
        assert_eq!(format!("{facade:?}"), format!("{core:?}"));
    }

    fn filter() -> FilterGroup {
        FilterGroup {
            conjunction: Conjunction::And,
            items: vec![
                FilterNode::Condition {
                    column: "age".into(),
                    op: FilterOp::Between,
                    values: vec![Value::Int(1), Value::Int(9)],
                },
                FilterNode::Condition {
                    column: "name".into(),
                    op: FilterOp::ContainsCase,
                    values: vec![text("50%")],
                },
                FilterNode::AnyColumn {
                    op: FilterOp::In,
                    values: vec![text("a"), text("b")],
                },
            ],
        }
    }

    fn keys() -> Vec<RowKey> {
        vec![
            RowKey::Pk(map([("id", Value::Int(7))])),
            RowKey::Pk(map([("id", Value::Null)])),
            RowKey::Rowid(42),
            RowKey::AllColumns(map([("name", text("x")), ("age", Value::Null)])),
            RowKey::Pk(BTreeMap::new()),
        ]
    }

    fn changes() -> Vec<StagedChange> {
        let mut out: Vec<StagedChange> = keys()
            .into_iter()
            .map(|key| StagedChange::Update {
                row_id: "r".into(),
                key,
                changes: map([("name", text("it's")), ("age", Value::Default)]),
                exprs: Some(BTreeMap::from([("created".to_string(), ServerExpr::Now)])),
            })
            .collect();
        out.push(StagedChange::Insert {
            row_id: "n".into(),
            values: map([("name", text("a")), ("data", Value::from_bytes(&[1, 2]))]),
            exprs: None,
        });
        out.push(StagedChange::Insert {
            row_id: "n".into(),
            values: BTreeMap::new(),
            exprs: None,
        });
        out.extend(keys().into_iter().map(|key| StagedChange::Delete {
            row_id: "r".into(),
            key,
        }));
        out
    }

    const STATEMENTS: &[&str] = &[
        "SELECT 1; DELETE FROM t",
        "select * from t where a = 'x;y' -- c",
        "CREATE USER u IDENTIFIED BY 'p\\'w'",
        "ALTER ROLE r PASSWORD 'secret'",
        "SELECT pg_sleep(1), sleep(1)",
        "WITH a AS (DELETE FROM t RETURNING *) SELECT * FROM a",
        "PRAGMA key = 'k'",
        "USE other",
        "",
    ];

    #[test]
    fn core_dialects_take_the_core_builders() {
        let t = TableRef::new(Some("app"), "users");
        let cols = columns();
        let pk = ["id".to_string()];
        let f = filter();
        let sort = [SortSpec {
            column: "name".into(),
            dir: SortDir::Desc,
            nulls: Some(NullsOrder::First),
        }];
        for d in CORE {
            for identity in [RowIdentity::Pk, RowIdentity::Rowid, RowIdentity::None] {
                for (sort, pk) in [(&sort[..], &pk[..]), (&[][..], &[][..])] {
                    let q = SelectQuery {
                        table: &t,
                        columns: &cols,
                        selected: None,
                        filter: Some(&f),
                        sort,
                        limit: 50,
                        offset: 100,
                        pk,
                        identity,
                    };
                    same(select(d, &q), select::build_select(&q.spec(d)));
                    same(
                        select(SqlDialect::from(d), &q),
                        select::build_select(&q.spec(d)),
                    );
                }
            }
            same(
                count(d, &t, &cols, Some(&f)),
                select::build_count(d, &t, &cols, Some(&f)),
            );
            same(
                count(d, &t, &cols, None),
                select::build_count(d, &t, &cols, None),
            );
            let target = TableTarget {
                table: &t,
                columns: &cols,
            };
            for key in keys() {
                for column in &cols {
                    same(
                        fetch_cell(d, &target, column, &key),
                        select::build_fetch_cell(&target.spec(d), column, &key),
                    );
                    assert_eq!(
                        column_form(d, column),
                        column_form(SqlDialect::from(d), column)
                    );
                }
            }
            for change in changes() {
                for returning in [false, true] {
                    same(
                        super::change(d, &target, &change, returning),
                        mutate::build_change(&target.spec(d), &change, returning),
                    );
                }
            }
            let built = select::build_count(d, &t, &cols, Some(&f)).unwrap();
            same(
                inline_params(d, &built.sql, &built.params),
                select::inline_params(d, &built.sql, &built.params),
            );
            for v in [
                Value::Null,
                Value::Bool(true),
                text("it's \\"),
                Value::from_bytes(&[0, 255]),
                Value::Decimal("1.50".into()),
            ] {
                same(literal(d, &v), select::literal(d, &v));
            }
            for mode in [ExplainMode::Plan, ExplainMode::Analyze] {
                for mariadb in [false, true] {
                    same(
                        wrap_explain(d, "SELECT 1;", mode, mariadb),
                        Ok::<_, AppError>(explain::wrap_explain(d, "SELECT 1;", mode, mariadb)),
                    );
                    same(
                        explain_offset(d, mode, mariadb),
                        explain::explain_offset(d, mode, mariadb),
                    );
                }
            }
            for in_tx in [Some(false), Some(true), None] {
                for writes in [false, true] {
                    same(
                        analyze_wrapper(d, in_tx, writes),
                        explain::analyze_wrapper(d, in_tx, writes),
                    );
                }
            }
            for sql in STATEMENTS {
                same(split_statements(d, sql), split::split_statements(sql, d));
                same(
                    is_single_statement(d, sql),
                    split::is_single_statement(sql, d),
                );
                same(code_only(d, sql), split::code_only(sql, d));
                same(classify(d, sql), classify::classify(sql, d));
                same(writes(d, sql), classify::writes(sql, d));
                same(
                    calls_side_effect_function(d, sql),
                    classify::calls_side_effect_function(sql, d),
                );
                same(redact(d, sql), redact_secrets(sql, d));
            }
            same(max_params(d), transfer::max_params(d));
        }
    }

    #[test]
    fn core_cell_forms() {
        let cols = columns();
        let by = |n: &str| cols.iter().find(|c| c.name == n).expect("a column");
        assert_eq!(column_form(Dialect::Postgres, by("data")), ColumnForm::Text);
        assert_eq!(column_form(Dialect::Mysql, by("name")), ColumnForm::Native);
        assert_eq!(column_form(Dialect::Sqlite, by("name")), ColumnForm::Native);
        assert_eq!(fetch_form(Dialect::Postgres, by("data")), FetchForm::Hex);
        assert_eq!(fetch_form(Dialect::Postgres, by("name")), FetchForm::Text);
        assert_eq!(fetch_form(Dialect::Mysql, by("data")), FetchForm::Hex);
        assert_eq!(fetch_form(Dialect::Mysql, by("name")), FetchForm::Native);
        assert_eq!(
            fetch_form(Dialect::Sqlite, by("data")),
            FetchForm::ValueAndHex
        );
    }

    #[test]
    fn extension_dialects_take_their_spec() {
        let _tsql = override_spec(SqlDialect::Tsql, &TSQL_LIKE_TEST);
        let d = SqlDialect::Tsql;
        let spec = &TSQL_LIKE_TEST;
        let t = TableRef::new(Some("dbo"), "users");
        let cols = columns();
        let pk = ["id".to_string()];
        let f = filter();
        let q = SelectQuery {
            table: &t,
            columns: &cols,
            selected: None,
            filter: Some(&f),
            sort: &[],
            limit: 50,
            offset: 100,
            pk: &pk,
            identity: RowIdentity::Pk,
        };
        let built = select(d, &q).unwrap();
        same(
            Ok::<_, AppError>(built.clone()),
            portable::select::build_select(spec, &q),
        );
        assert!(built.sql.starts_with(
            "SELECT CONVERT(NVARCHAR(MAX), [id]) AS [id], CONVERT(NVARCHAR(MAX), [name]) AS [name]"
        ));
        assert!(built.sql.contains("[data], [geom].STAsText() AS [geom]"));
        assert!(
            built
                .sql
                .ends_with(" ORDER BY [id] ASC\nOFFSET 100 ROWS FETCH NEXT 50 ROWS ONLY")
        );
        same(
            count(d, &t, &cols, Some(&f)),
            portable::select::build_count(spec, &t, &cols, Some(&f)),
        );
        same(
            inline_params(d, &built.sql, &built.params),
            portable::literal::inline_params(spec, &built.sql, &built.params),
        );
        assert_eq!(literal(d, &Value::Bool(true)), "1");
        assert_eq!(literal(d, &text("it's")), "N'it''s'");
        assert_eq!(max_params(d), 2098);
        assert_eq!(classify(d, "SELECT 1"), StatementClass::Read);
        assert!(writes(d, "SELECT 1 DELETE FROM t"));
        assert!(is_single_statement(d, "SELECT * FROM t WITH (NOLOCK)"));
        assert!(!is_single_statement(d, "SELECT 1; DELETE FROM t"));
        assert!(!is_single_statement(d, "SELECT 1 DELETE FROM t"));
        assert_eq!(
            split_statements(d, "SELECT 1\nGO\nSELECT 2")
                .into_iter()
                .map(|s| s.sql)
                .collect::<Vec<_>>(),
            ["SELECT 1", "SELECT 2"]
        );
        assert!(matches!(
            wrap_explain(d, "SELECT 1", ExplainMode::Analyze, false),
            Err(AppError::Unsupported(_))
        ));

        // Rows arrive as the projection says; a shortened column is never written or keyed on.
        let by = |n: &str| cols.iter().find(|c| c.name == n).expect("a column");
        assert_eq!(column_form(d, by("name")), ColumnForm::Text);
        assert_eq!(column_form(d, by("data")), ColumnForm::Native);
        assert_eq!(column_form(d, by("geom")), ColumnForm::LossyText);
        assert_eq!(fetch_form(d, by("name")), FetchForm::Text);
        assert_eq!(fetch_form(d, by("data")), FetchForm::Native);
        let target = TableTarget {
            table: &t,
            columns: &cols,
        };
        let key = RowKey::Pk(map([("id", Value::Int(7))]));
        assert!(matches!(
            fetch_cell(d, &target, by("geom"), &key),
            Err(AppError::Unsupported(_))
        ));
        let edit = StagedChange::Update {
            row_id: "r".into(),
            key: key.clone(),
            changes: map([("geom", text("POINT (1 2)"))]),
            exprs: None,
        };
        assert!(matches!(
            change(d, &target, &edit, false),
            Err(AppError::Unsupported(_))
        ));
        // Only key identity: a row without a primary key cannot be edited.
        let keyless = StagedChange::Delete {
            row_id: "r".into(),
            key: RowKey::AllColumns(map([("name", text("x"))])),
        };
        assert!(matches!(
            change(d, &target, &keyless, false),
            Err(AppError::Unsupported(_))
        ));
        let (sql, params) = fetch_cell(d, &target, by("name"), &key).unwrap();
        assert_eq!(
            sql,
            "SELECT TOP (1) CONVERT(NVARCHAR(MAX), [name]) FROM [dbo].[users] WHERE [id] = @P1"
        );
        assert_eq!(params, [Value::Int(7)]);
    }
}
