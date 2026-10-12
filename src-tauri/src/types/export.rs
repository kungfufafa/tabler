//! Export request/progress/result types (M6): what to export, in which file format, where.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::browse::{FilterGroup, SortSpec};
use super::query::ResultColumn;
use super::schema::TableRef;
use super::value::Value;

/// Where the exported rows come from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export, rename_all_fields = "camelCase")]
pub enum ExportSource {
    /// A table or view, with the grid's filter and sort applied.
    Table {
        table: TableRef,
        filter: Option<FilterGroup>,
        #[serde(default)]
        sort: Vec<SortSpec>,
        /// Subset of columns (in this order); `None` = all.
        columns: Option<Vec<String>>,
    },
    /// One SQL statement that returns rows, run as written.
    Sql { sql: String },
    /// Rows the renderer already holds (a result shown in the SQL editor), written as they are
    /// without touching the database. `table` is the default INSERT target.
    Rows {
        columns: Vec<ResultColumn>,
        rows: Vec<Vec<Value>>,
        table: Option<String>,
    },
}

/// Output file format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "format",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export, rename_all_fields = "camelCase")]
pub enum ExportFormat {
    /// RFC 4180 CSV (CRLF line ends, fields quoted when needed or always).
    Csv {
        /// One ASCII character, e.g. `,`, `;` or a tab.
        delimiter: String,
        /// Write the column names as the first record.
        header: bool,
        /// Quote every field, not only the ones that need it.
        quote_all: bool,
        /// Text written for SQL NULL (often empty, `NULL` or `\N`).
        null_as: String,
    },
    /// A JSON array of objects, or one object per line (NDJSON). `pretty` indents the array
    /// form; NDJSON is always one compact object per line.
    Json { ndjson: bool, pretty: bool },
    /// Multi-row `INSERT` statements in the connection's dialect, `batch_size` rows each.
    SqlInsert {
        /// Target table name (one identifier, quoted on output). Empty = the source table's
        /// name when exporting a table.
        table_name: String,
        batch_size: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportRequest {
    /// Chosen by the renderer; `cancel_export` takes the same id.
    pub export_id: String,
    pub source: ExportSource,
    pub format: ExportFormat,
    /// Absolute path of the file to write (replaced atomically when the export succeeds).
    pub path: String,
    /// Stop after this many rows (`None` = every row).
    pub max_rows: Option<u64>,
}

/// Sent through the progress channel while an export runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportProgress {
    pub rows: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportResult {
    pub path: String,
    pub rows: u64,
    pub bytes: u64,
    pub duration_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_json_shape() {
        let req: ExportRequest = serde_json::from_value(serde_json::json!({
            "exportId": "e1",
            "source": {
                "kind": "table",
                "table": {"schema": "public", "name": "users"},
                "filter": null,
                "columns": ["id", "name"]
            },
            "format": {"format": "csv", "delimiter": ";", "header": true, "quoteAll": false, "nullAs": ""},
            "path": "/tmp/users.csv",
            "maxRows": 10
        }))
        .unwrap();
        assert_eq!(req.export_id, "e1");
        assert_eq!(req.max_rows, Some(10));
        match &req.source {
            ExportSource::Table {
                table,
                sort,
                columns,
                ..
            } => {
                assert_eq!(table.name, "users");
                assert!(sort.is_empty());
                assert_eq!(
                    columns.as_deref(),
                    Some(&["id".to_string(), "name".to_string()][..])
                );
            }
            other => panic!("unexpected source {other:?}"),
        }
        assert_eq!(
            req.format,
            ExportFormat::Csv {
                delimiter: ";".into(),
                header: true,
                quote_all: false,
                null_as: String::new()
            }
        );

        let fmt = serde_json::to_value(ExportFormat::SqlInsert {
            table_name: "t".into(),
            batch_size: 100,
        })
        .unwrap();
        assert_eq!(
            fmt,
            serde_json::json!({"format": "sqlInsert", "tableName": "t", "batchSize": 100})
        );
        let src = serde_json::to_value(ExportSource::Sql {
            sql: "select 1".into(),
        })
        .unwrap();
        assert_eq!(src, serde_json::json!({"kind": "sql", "sql": "select 1"}));
        let rows: ExportSource = serde_json::from_value(serde_json::json!({
            "kind": "rows",
            "columns": [{"name": "id", "dataType": "int4", "kind": "int", "nullable": null, "table": "users"}],
            "rows": [[{"t": "int", "v": 1}], [{"t": "null"}]],
            "table": "users"
        }))
        .unwrap();
        match &rows {
            ExportSource::Rows {
                columns,
                rows,
                table,
            } => {
                assert_eq!(columns[0].name, "id");
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[1][0], Value::Null);
                assert_eq!(table.as_deref(), Some("users"));
            }
            other => panic!("unexpected source {other:?}"),
        }
        let res = serde_json::to_value(ExportResult {
            path: "/x".into(),
            rows: 1,
            bytes: 2,
            duration_ms: 3,
        })
        .unwrap();
        assert_eq!(res["durationMs"], 3);
    }
}
