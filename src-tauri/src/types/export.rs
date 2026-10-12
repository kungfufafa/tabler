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

/// How CSV values are quoted (TablePlus: `"`, `'`, "Quote if needed").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CsvQuote {
    /// Every value in `"…"`.
    Double,
    /// Every value in `'…'`.
    Single,
    /// `"…"` only around values that contain the delimiter, a quote or a line break.
    #[default]
    IfNeeded,
}

fn default_line_break() -> String {
    "\n".into()
}

fn default_decimal() -> String {
    ".".into()
}

fn yes() -> bool {
    true
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
    /// RFC 4180-style CSV.
    Csv {
        /// One ASCII character, e.g. `,`, `;` or a tab.
        delimiter: String,
        /// Write the column names as the first record ("Put field names in the first row").
        header: bool,
        #[serde(default)]
        quote: CsvQuote,
        /// Text written for SQL NULL: empty ("Convert NULL to EMPTY") or `NULL`.
        null_as: String,
        /// Record terminator: `\n`, `\r\n` or `\r`.
        #[serde(default = "default_line_break")]
        line_break: String,
        /// Decimal separator of numbers: `.` or `,`.
        #[serde(default = "default_decimal")]
        decimal: String,
        /// "Convert line break to space": line breaks inside values become one space.
        #[serde(default)]
        line_break_to_space: bool,
    },
    /// A JSON array of objects, or one object per line (NDJSON). `pretty` indents the array
    /// form; NDJSON is always one compact object per line.
    Json { ndjson: bool, pretty: bool },
    /// A SQL script: multi-row `INSERT` statements in the connection's dialect, `batch_size`
    /// rows each, optionally after `DROP TABLE IF EXISTS` and the table's `CREATE TABLE`.
    SqlInsert {
        /// Target table name (one identifier, quoted on output). Empty = the source table's
        /// name when exporting a table.
        table_name: String,
        batch_size: u32,
        /// "Drop table if exists" before the structure / content (table sources only).
        #[serde(default)]
        drop_table: bool,
        /// "Include table structure": the table's `CREATE TABLE` (table sources only).
        #[serde(default)]
        structure: bool,
        /// "Include table content": the `INSERT` statements.
        #[serde(default = "yes")]
        content: bool,
        /// "Compress the file using Gzip" (`.sql.gz`).
        #[serde(default)]
        gzip: bool,
    },
    /// An Excel workbook (`.xlsx`): one sheet, or one per table for a multi-table workbook.
    Xlsx {
        /// Column names as the first (bold, frozen) row.
        header: bool,
    },
}

/// Where a multi-table export writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ExportLayout {
    /// `<table>.<ext>` per table into the folder `target`.
    FilePerTable,
    /// Every table into the one file `target`: one SQL script, or one workbook with a sheet per
    /// table (SQL and Excel only).
    SingleFile,
}

/// Several tables at once (File ▸ Export… on a table selection).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportTablesRequest {
    /// Chosen by the renderer; `cancel_export` takes the same id.
    pub export_id: String,
    pub tables: Vec<TableRef>,
    /// `SqlInsert`'s `table_name` is ignored: every table's INSERTs name that table.
    pub format: ExportFormat,
    pub layout: ExportLayout,
    /// Absolute path of the folder (`FilePerTable`) or of the file (`SingleFile`).
    pub target: String,
}

/// Sent through the progress channel while a multi-table export runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportTablesProgress {
    /// 0-based index of the table being written.
    pub table_index: u32,
    pub table_count: u32,
    /// Name of the table being written.
    pub table: String,
    /// Rows written of that table so far.
    pub table_rows: u64,
    /// Rows written over every table so far.
    pub rows: u64,
    /// Bytes written over every file so far.
    pub bytes: u64,
}

/// One table of a finished multi-table export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportedTable {
    pub table: TableRef,
    /// The file it went into (the same file for every table of a `SingleFile` export).
    pub path: String,
    pub rows: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportTablesResult {
    pub tables: Vec<ExportedTable>,
    /// Every file written, in order.
    pub paths: Vec<String>,
    pub rows: u64,
    pub bytes: u64,
    pub duration_ms: u64,
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
            "format": {"format": "csv", "delimiter": ";", "header": true, "nullAs": ""},
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
                quote: CsvQuote::IfNeeded,
                null_as: String::new(),
                line_break: "\n".into(),
                decimal: ".".into(),
                line_break_to_space: false,
            }
        );

        let fmt = serde_json::to_value(ExportFormat::SqlInsert {
            table_name: "t".into(),
            batch_size: 100,
            drop_table: false,
            structure: true,
            content: true,
            gzip: false,
        })
        .unwrap();
        assert_eq!(
            fmt,
            serde_json::json!({
                "format": "sqlInsert", "tableName": "t", "batchSize": 100,
                "dropTable": false, "structure": true, "content": true, "gzip": false
            })
        );
        // Older requests without the SQL options keep their meaning: content only.
        let old: ExportFormat = serde_json::from_value(
            serde_json::json!({"format": "sqlInsert", "tableName": "", "batchSize": 10}),
        )
        .unwrap();
        assert_eq!(
            old,
            ExportFormat::SqlInsert {
                table_name: String::new(),
                batch_size: 10,
                drop_table: false,
                structure: false,
                content: true,
                gzip: false,
            }
        );
        let csv: ExportFormat = serde_json::from_value(serde_json::json!({
            "format": "csv", "delimiter": ",", "header": false, "quote": "single",
            "nullAs": "NULL", "lineBreak": "\r\n", "decimal": ",", "lineBreakToSpace": true
        }))
        .unwrap();
        assert!(matches!(
            csv,
            ExportFormat::Csv {
                quote: CsvQuote::Single,
                line_break_to_space: true,
                ..
            }
        ));
        let tables: ExportTablesRequest = serde_json::from_value(serde_json::json!({
            "exportId": "m1",
            "tables": [{"schema": "public", "name": "a"}, {"schema": null, "name": "b"}],
            "format": {"format": "xlsx", "header": true},
            "layout": "singleFile",
            "target": "/tmp/all.xlsx"
        }))
        .unwrap();
        assert_eq!(tables.layout, ExportLayout::SingleFile);
        assert_eq!(tables.format, ExportFormat::Xlsx { header: true });
        assert_eq!(tables.tables.len(), 2);
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
