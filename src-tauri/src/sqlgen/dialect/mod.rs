//! The SQL dialects of every driver. `Dialect` (`quote.rs`) stays the closed set of the core
//! generators (PostgreSQL, MySQL, SQLite); `SqlDialect` adds the dialects of the other drivers.
//! For now it is data only (descriptors, the renderer's catalog); the builders come later.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

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
}
