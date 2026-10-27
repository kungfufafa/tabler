//! The SQL dialects of every driver. `Dialect` (`quote.rs`) stays the closed set of the core
//! generators (PostgreSQL, MySQL, SQLite); `SqlDialect` adds the dialects of the other drivers.
//!
//! `SqlDialect::resolve` tells the two apart: a core dialect goes to the core generators
//! (unchanged), an extension dialect to the `DialectSpec` its lane writes in
//! `sqlgen/dialect/<d>.rs` (built with the lane's feature), or to its `unavailable` spec, which
//! refuses every builder, when this build lacks it. Shared code reaches either only through the
//! `sqlgen::sql` facade.

pub mod ddl;
pub mod lex;
pub mod spec;
mod unavailable;

// The lane dialects, each with its driver's feature.
#[cfg(feature = "drv-bigquery")]
pub mod bigquery;
#[cfg(feature = "drv-clickhouse")]
pub mod clickhouse;
#[cfg(feature = "drv-cassandra")]
pub mod cql;
#[cfg(feature = "drv-duckdb")]
pub mod duckdb;
#[cfg(feature = "drv-pg-flavors")]
pub mod redshift;
#[cfg(feature = "drv-snowflake")]
pub mod snowflake;
#[cfg(feature = "drv-sqlserver")]
pub mod tsql;
#[cfg(feature = "drv-vertica")]
pub mod vertica;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use ddl::DdlPlanner;
pub use lex::{ClassifyRules, LexProfile, Reading};
pub use spec::DialectSpec;

use crate::error::AppError;
use crate::sqlgen::Dialect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, rename = "SqlDialectId")]
pub enum SqlDialect {
    Postgres,
    Mysql,
    Sqlite,
    Tsql,
    Clickhouse,
    Snowflake,
    Bigquery,
    Duckdb,
    Vertica,
    Cql,
    Redshift,
}

/// What a `SqlDialect` is generated with: the core generators, or an extension spec.
#[derive(Debug, Clone, Copy)]
pub enum DialectRef {
    Core(Dialect),
    Ext(&'static DialectSpec),
}

impl SqlDialect {
    pub const ALL: [SqlDialect; 11] = [
        SqlDialect::Postgres,
        SqlDialect::Mysql,
        SqlDialect::Sqlite,
        SqlDialect::Tsql,
        SqlDialect::Clickhouse,
        SqlDialect::Snowflake,
        SqlDialect::Bigquery,
        SqlDialect::Duckdb,
        SqlDialect::Vertica,
        SqlDialect::Cql,
        SqlDialect::Redshift,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SqlDialect::Postgres => "PostgreSQL",
            SqlDialect::Mysql => "MySQL",
            SqlDialect::Sqlite => "SQLite",
            SqlDialect::Tsql => "Transact-SQL",
            SqlDialect::Clickhouse => "ClickHouse SQL",
            SqlDialect::Snowflake => "Snowflake SQL",
            SqlDialect::Bigquery => "GoogleSQL",
            SqlDialect::Duckdb => "DuckDB SQL",
            SqlDialect::Vertica => "Vertica SQL",
            SqlDialect::Cql => "CQL",
            SqlDialect::Redshift => "Redshift SQL",
        }
    }

    /// The core dialect, for the core three. Only `sqlgen::dialect`, the `sqlgen::sql` facade,
    /// `DbSession::core_dialect` and tests may ask: elsewhere an extension dialect would fall
    /// into a core generator's "else" branch.
    pub const fn core(self) -> Option<Dialect> {
        match self {
            SqlDialect::Postgres => Some(Dialect::Postgres),
            SqlDialect::Mysql => Some(Dialect::Mysql),
            SqlDialect::Sqlite => Some(Dialect::Sqlite),
            SqlDialect::Tsql
            | SqlDialect::Clickhouse
            | SqlDialect::Snowflake
            | SqlDialect::Bigquery
            | SqlDialect::Duckdb
            | SqlDialect::Vertica
            | SqlDialect::Cql
            | SqlDialect::Redshift => None,
        }
    }

    /// The generators of the dialect: the core ones, the lane's spec, or the unavailable spec of
    /// a dialect this build lacks.
    pub fn resolve(self) -> DialectRef {
        let spec: fn() -> &'static DialectSpec = match self {
            SqlDialect::Postgres => return DialectRef::Core(Dialect::Postgres),
            SqlDialect::Mysql => return DialectRef::Core(Dialect::Mysql),
            SqlDialect::Sqlite => return DialectRef::Core(Dialect::Sqlite),
            SqlDialect::Tsql => tsql_spec,
            SqlDialect::Clickhouse => clickhouse_spec,
            SqlDialect::Snowflake => snowflake_spec,
            SqlDialect::Bigquery => bigquery_spec,
            SqlDialect::Duckdb => duckdb_spec,
            SqlDialect::Vertica => vertica_spec,
            SqlDialect::Cql => cql_spec,
            SqlDialect::Redshift => redshift_spec,
        };
        #[cfg(test)]
        if let Some(spec) = testing::overridden(self) {
            return DialectRef::Ext(spec);
        }
        DialectRef::Ext(spec())
    }

    /// Quotes an identifier (always; the input is never trusted).
    pub fn quote_ident(self, s: &str) -> String {
        match self.resolve() {
            DialectRef::Core(d) => d.quote_ident(s),
            DialectRef::Ext(spec) => (spec.quote_ident)(s),
        }
    }

    /// A string literal for display only (Preview SQL); execution binds parameters.
    pub fn quote_literal(self, s: &str) -> String {
        match self.resolve() {
            DialectRef::Core(d) => d.quote_literal(s),
            DialectRef::Ext(spec) => (spec.quote_literal)(s),
        }
    }

    /// The n-th (1-based) bind parameter.
    pub fn placeholder(self, n: usize) -> String {
        match self.resolve() {
            DialectRef::Core(d) => d.placeholder(n),
            DialectRef::Ext(spec) => (spec.placeholder)(n, None),
        }
    }

    /// `schema.name` quoted, or the name alone when no schema is given.
    pub fn qualify(self, schema: Option<&str>, name: &str) -> String {
        match self.resolve() {
            DialectRef::Core(d) => d.qualify(schema, name),
            DialectRef::Ext(spec) => qualify_with(spec, schema, name),
        }
    }
}

impl From<Dialect> for SqlDialect {
    fn from(d: Dialect) -> SqlDialect {
        match d {
            Dialect::Postgres => SqlDialect::Postgres,
            Dialect::Mysql => SqlDialect::Mysql,
            Dialect::Sqlite => SqlDialect::Sqlite,
        }
    }
}

/// `Dialect::qualify` with a spec's quoting.
pub fn qualify_with(spec: &DialectSpec, schema: Option<&str>, name: &str) -> String {
    match schema {
        Some(s) if !s.is_empty() => {
            format!("{}.{}", (spec.quote_ident)(s), (spec.quote_ident)(name))
        }
        _ => (spec.quote_ident)(name),
    }
}

/// Whether `spec` is the unavailable spec of a dialect this build lacks.
pub fn is_unavailable(spec: &DialectSpec) -> bool {
    unavailable::is(spec)
}

/// Refuses a spec this build lacks with "<dialect> is not built into this Tabler".
pub fn ensure_built(spec: &'static DialectSpec) -> Result<&'static DialectSpec, AppError> {
    if unavailable::is(spec) {
        Err(unavailable::not_built(spec))
    } else {
        Ok(spec)
    }
}

// One pair of shims per lane dialect: the lane's spec when its feature is on, the unavailable
// spec otherwise, so nothing else needs a `cfg`.

/// The unavailable spec of an extension dialect.
#[allow(dead_code)] // every lane feature on: no shim needs it
fn unavailable(d: SqlDialect) -> &'static DialectSpec {
    unavailable::of(d).expect("an extension dialect has an unavailable spec")
}

#[cfg(feature = "drv-sqlserver")]
fn tsql_spec() -> &'static DialectSpec {
    &tsql::SPEC
}
#[cfg(not(feature = "drv-sqlserver"))]
fn tsql_spec() -> &'static DialectSpec {
    unavailable(SqlDialect::Tsql)
}

#[cfg(feature = "drv-clickhouse")]
fn clickhouse_spec() -> &'static DialectSpec {
    &clickhouse::SPEC
}
#[cfg(not(feature = "drv-clickhouse"))]
fn clickhouse_spec() -> &'static DialectSpec {
    unavailable(SqlDialect::Clickhouse)
}

#[cfg(feature = "drv-snowflake")]
fn snowflake_spec() -> &'static DialectSpec {
    &snowflake::SPEC
}
#[cfg(not(feature = "drv-snowflake"))]
fn snowflake_spec() -> &'static DialectSpec {
    unavailable(SqlDialect::Snowflake)
}

#[cfg(feature = "drv-bigquery")]
fn bigquery_spec() -> &'static DialectSpec {
    &bigquery::SPEC
}
#[cfg(not(feature = "drv-bigquery"))]
fn bigquery_spec() -> &'static DialectSpec {
    unavailable(SqlDialect::Bigquery)
}

#[cfg(feature = "drv-duckdb")]
fn duckdb_spec() -> &'static DialectSpec {
    &duckdb::SPEC
}
#[cfg(not(feature = "drv-duckdb"))]
fn duckdb_spec() -> &'static DialectSpec {
    unavailable(SqlDialect::Duckdb)
}

#[cfg(feature = "drv-vertica")]
fn vertica_spec() -> &'static DialectSpec {
    &vertica::SPEC
}
#[cfg(not(feature = "drv-vertica"))]
fn vertica_spec() -> &'static DialectSpec {
    unavailable(SqlDialect::Vertica)
}

#[cfg(feature = "drv-cassandra")]
fn cql_spec() -> &'static DialectSpec {
    &cql::SPEC
}
#[cfg(not(feature = "drv-cassandra"))]
fn cql_spec() -> &'static DialectSpec {
    unavailable(SqlDialect::Cql)
}

#[cfg(feature = "drv-pg-flavors")]
fn redshift_spec() -> &'static DialectSpec {
    &redshift::SPEC
}
#[cfg(not(feature = "drv-pg-flavors"))]
fn redshift_spec() -> &'static DialectSpec {
    unavailable(SqlDialect::Redshift)
}

/// Tests that run shared code with an extension dialect: `override_spec(d, &SPEC)` makes `d`
/// resolve to `SPEC` on this thread until the guard drops (a test spec such as
/// `portable::testing::TSQL_LIKE_TEST`, whatever this build has). The override is per thread:
/// use it in synchronous tests or on a current-thread runtime.
#[cfg(test)]
pub mod testing {
    use std::cell::RefCell;

    use super::{DialectSpec, SqlDialect};

    thread_local! {
        static OVERRIDES: RefCell<Vec<(SqlDialect, &'static DialectSpec)>> =
            const { RefCell::new(Vec::new()) };
    }

    #[must_use = "the override ends when the guard drops"]
    pub struct Override(SqlDialect);

    impl Drop for Override {
        fn drop(&mut self) {
            OVERRIDES.with(|o| {
                let mut o = o.borrow_mut();
                if let Some(at) = o.iter().rposition(|(d, _)| *d == self.0) {
                    o.remove(at);
                }
            });
        }
    }

    /// Makes `d` (an extension dialect) resolve to `spec` on this thread while the guard lives.
    pub fn override_spec(d: SqlDialect, spec: &'static DialectSpec) -> Override {
        assert!(d.core().is_none(), "the core dialects cannot be overridden");
        OVERRIDES.with(|o| o.borrow_mut().push((d, spec)));
        Override(d)
    }

    pub(super) fn overridden(d: SqlDialect) -> Option<&'static DialectSpec> {
        OVERRIDES.with(|o| {
            o.borrow()
                .iter()
                .rev()
                .find(|(x, _)| *x == d)
                .map(|(_, s)| *s)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_dialects_keep_their_ids() {
        for d in [Dialect::Postgres, Dialect::Mysql, Dialect::Sqlite] {
            assert_eq!(
                serde_json::to_value(SqlDialect::from(d)).unwrap(),
                serde_json::to_value(d).unwrap()
            );
        }
        let ids: Vec<String> = SqlDialect::ALL
            .iter()
            .map(|d| {
                serde_json::to_value(d)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(
            ids,
            [
                "postgres",
                "mysql",
                "sqlite",
                "tsql",
                "clickhouse",
                "snowflake",
                "bigquery",
                "duckdb",
                "vertica",
                "cql",
                "redshift"
            ]
        );
    }

    /// The lane feature that builds each extension dialect.
    fn built(d: SqlDialect) -> bool {
        match d {
            SqlDialect::Postgres | SqlDialect::Mysql | SqlDialect::Sqlite => true,
            SqlDialect::Tsql => cfg!(feature = "drv-sqlserver"),
            SqlDialect::Clickhouse => cfg!(feature = "drv-clickhouse"),
            SqlDialect::Snowflake => cfg!(feature = "drv-snowflake"),
            SqlDialect::Bigquery => cfg!(feature = "drv-bigquery"),
            SqlDialect::Duckdb => cfg!(feature = "drv-duckdb"),
            SqlDialect::Vertica => cfg!(feature = "drv-vertica"),
            SqlDialect::Cql => cfg!(feature = "drv-cassandra"),
            SqlDialect::Redshift => cfg!(feature = "drv-pg-flavors"),
        }
    }

    #[test]
    fn every_dialect_resolves() {
        for d in SqlDialect::ALL {
            match d.resolve() {
                DialectRef::Core(core) => {
                    assert_eq!(d.core(), Some(core));
                    assert_eq!(SqlDialect::from(core), d);
                }
                DialectRef::Ext(spec) => {
                    assert_eq!(d.core(), None, "{d:?}");
                    assert_eq!(spec.dialect, d);
                    assert_eq!(spec.label, d.label());
                    assert_eq!(is_unavailable(spec), !built(d), "{d:?}");
                    assert_eq!(ensure_built(spec).is_ok(), built(d), "{d:?}");
                }
            }
        }
    }

    #[test]
    fn quoting_follows_the_resolved_dialect() {
        assert_eq!(SqlDialect::Postgres.quote_ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(SqlDialect::Mysql.qualify(Some("app"), "t"), "`app`.`t`");
        assert_eq!(SqlDialect::Sqlite.placeholder(2), "?2");
        assert_eq!(SqlDialect::Mysql.quote_literal("it's"), "'it\\'s'");
        let _tsql = testing::override_spec(
            SqlDialect::Tsql,
            &crate::sqlgen::portable::testing::TSQL_LIKE_TEST,
        );
        assert_eq!(SqlDialect::Tsql.quote_ident("a]b"), "[a]]b]");
        assert_eq!(SqlDialect::Tsql.qualify(Some("dbo"), "t"), "[dbo].[t]");
        assert_eq!(SqlDialect::Tsql.placeholder(3), "@P3");
        assert_eq!(SqlDialect::Tsql.quote_literal("it's"), "N'it''s'");
    }

    #[test]
    fn unavailable_specs_refuse_every_builder() {
        use crate::sqlgen::classify::StatementClass;
        use crate::sqlgen::portable;
        use crate::sqlgen::portable::testing::{columns, map};
        use crate::sqlgen::sql::{self, SelectQuery, TableTarget};
        use crate::types::browse::{RowIdentity, RowKey};
        use crate::types::mutate::StagedChange;
        use crate::types::query::ExplainMode;
        use crate::types::schema::TableRef;
        use crate::types::value::Value;

        fn refused<T: std::fmt::Debug>(d: SqlDialect, r: Result<T, AppError>) {
            let want = format!("{} is not built into this Tabler", d.label());
            assert!(
                matches!(&r, Err(AppError::Unsupported(m)) if *m == want),
                "{d:?}: {r:?}"
            );
        }

        let t = TableRef::new(None, "t");
        let cols = columns();
        let q = SelectQuery {
            table: &t,
            columns: &cols,
            selected: None,
            filter: None,
            sort: &[],
            limit: 10,
            offset: 0,
            pk: &[],
            identity: RowIdentity::None,
        };
        let target = TableTarget {
            table: &t,
            columns: &cols,
        };
        let key = RowKey::Pk(map([("id", Value::Int(1))]));
        let delete = StagedChange::Delete {
            row_id: "r".into(),
            key: key.clone(),
        };
        for d in SqlDialect::ALL {
            let Some(spec) = unavailable::of(d) else {
                continue;
            };
            assert!(is_unavailable(spec));
            // The facade refuses it, whatever this build has.
            let _g = testing::override_spec(d, spec);
            refused(d, sql::select(d, &q));
            refused(d, sql::count(d, &t, &cols, None));
            refused(d, sql::fetch_cell(d, &target, &cols[0], &key));
            refused(d, sql::change(d, &target, &delete, false));
            for mode in [ExplainMode::Plan, ExplainMode::Analyze] {
                refused(d, sql::wrap_explain(d, "SELECT 1", mode, false));
            }
            refused(d, sql::analyze_wrapper(d, Some(false), false));
            assert_eq!(sql::classify(d, "SELECT 1"), StatementClass::Write);
            assert_eq!(sql::max_params(d), 0);
            // So do its builders, called directly.
            refused(d, portable::select::build_select(spec, &q));
            refused(d, portable::select::build_count(spec, &t, &cols, None));
            refused(
                d,
                portable::select::build_fetch_cell(spec, &target, &cols[0], &key),
            );
            refused(
                d,
                portable::mutate::build_change(spec, &target, &delete, true),
            );
        }
    }

    #[test]
    fn an_override_ends_with_its_guard() {
        {
            let _g = testing::override_spec(
                SqlDialect::Vertica,
                &crate::sqlgen::portable::testing::ANSI_TEST,
            );
            assert!(matches!(
                SqlDialect::Vertica.resolve(),
                DialectRef::Ext(s) if std::ptr::eq(s, &crate::sqlgen::portable::testing::ANSI_TEST)
            ));
        }
        let DialectRef::Ext(spec) = SqlDialect::Vertica.resolve() else {
            panic!("Vertica is an extension dialect");
        };
        assert!(!std::ptr::eq(
            spec,
            &crate::sqlgen::portable::testing::ANSI_TEST
        ));
    }
}
