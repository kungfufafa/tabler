//! Import request / progress / report types: the CSV wizard and SQL dump import (File ▸ Import).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::schema::TableRef;

/// Text encoding of an imported file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TextEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Windows1252,
    Latin1,
}

/// How the CSV file is read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CsvFormat {
    /// One ASCII character; `None` = detect (`,` `;` tab `|`).
    pub delimiter: Option<String>,
    /// `"` or `'`.
    pub quote: String,
    /// `None` = detect (byte order mark, else UTF-8 when the start is valid UTF-8, else
    /// Windows-1252).
    pub encoding: Option<TextEncoding>,
    /// "First line is header".
    pub header: bool,
}

/// Column type of a table the CSV import creates (detected, editable in the wizard).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ImportType {
    Integer,
    Bigint,
    Numeric,
    Boolean,
    Date,
    Timestamp,
    /// A timestamp with a UTC offset (`…Z`, `…+07:00`): PostgreSQL `timestamptz`; MySQL
    /// `DATETIME` holding the times converted to UTC (it keeps no offset); SQLite text.
    TimestampTz,
    Text,
}

/// A column of the table "Create new table" makes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportColumnDef {
    pub name: String,
    pub ty: ImportType,
    /// Numeric: significant digits seen (sizes MySQL's DECIMAL).
    pub precision: Option<u32>,
    /// Numeric: digits after the point (exponents applied: `1.5e-3` has 4); timestamp:
    /// fractional-second digits.
    pub scale: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CsvPreviewRequest {
    pub path: String,
    pub format: CsvFormat,
}

/// A CSV column as the preview sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CsvColumn {
    /// The header's name (`column_<n>` without a header row, or when it is empty).
    pub name: String,
    /// Type detected from the sampled rows (the new table's default).
    pub detected: ImportColumnDef,
}

/// The first rows of a CSV file, read with the resolved options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CsvPreview {
    pub path: String,
    pub file_name: String,
    pub size: u64,
    /// The delimiter used (detected when the request left it open).
    pub delimiter: String,
    pub encoding: TextEncoding,
    pub columns: Vec<CsvColumn>,
    /// Up to 100 data rows; `None` = an empty unquoted field (NULL with "Empty unquoted field
    /// as NULL"). Rows may be shorter or longer than `columns`.
    pub rows: Vec<Vec<Option<String>>>,
    /// Data rows the types were detected from.
    pub sampled_rows: u32,
    /// The file has more data rows than `rows`.
    pub more: bool,
}

/// The wizard's insert option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum InsertMode {
    /// Plain INSERTs; the first failure stops the import and rolls back.
    StopOnError,
    /// MySQL `REPLACE INTO`, SQLite `INSERT OR REPLACE` (not on PostgreSQL).
    Replace,
    /// MySQL `ON DUPLICATE KEY UPDATE`, PostgreSQL / SQLite `ON CONFLICT (pk) DO UPDATE`.
    UpdateOnDuplicate,
}

/// Where the CSV rows go.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export, rename_all_fields = "camelCase")]
pub enum ImportTarget {
    /// An existing table.
    Existing { table: TableRef },
    /// "Create new table" with these columns (created in the import's first transaction).
    Create {
        table: TableRef,
        columns: Vec<ImportColumnDef>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CsvImportRequest {
    /// Chosen by the renderer; `cancel_import` takes the same id.
    pub import_id: String,
    pub path: String,
    /// Delimiter and encoding resolved (as the preview showed them).
    pub format: CsvFormat,
    pub target: ImportTarget,
    /// For each CSV column by position: the table column it goes into, or `None` ("Do not
    /// import"). CSV columns past the end are not imported.
    pub mapping: Vec<Option<String>>,
    /// "Empty unquoted field as NULL" (a quoted `""` is always an empty string).
    pub empty_as_null: bool,
    pub mode: InsertMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ImportPhase {
    Reading,
    Writing,
}

/// Sent through the progress channel while a CSV import runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CsvImportProgress {
    pub phase: ImportPhase,
    /// Data rows read from the file.
    pub rows_read: u64,
    /// Rows committed to the table.
    pub rows_imported: u64,
    pub bytes: u64,
    pub total_bytes: u64,
}

/// Why a CSV import stopped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CsvImportFailure {
    pub message: String,
    pub detail: Option<String>,
    /// Data rows (1-based, header excluded) of the failing statement or value; `None` when the
    /// failure is not tied to rows.
    pub first_row: Option<u64>,
    pub last_row: Option<u64>,
    /// File line the first of those rows starts on.
    pub line: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CsvImportReport {
    /// Rows committed to the table.
    pub rows_imported: u64,
    pub rows_read: u64,
    /// The rows went in as one transaction (all or nothing); a large file is committed in
    /// chunks instead, and rows of chunks before a failure stay.
    pub single_transaction: bool,
    /// The new table exists afterwards ("Create new table").
    pub created_table: bool,
    pub cancelled: bool,
    pub failure: Option<CsvImportFailure>,
    /// Rows left out by "Replace" / "Update on duplicate" (STOP ON ERROR stops at the first one
    /// instead): a different number of fields than the header, or a value the new table's column
    /// would change.
    pub rows_skipped: u64,
    /// The first skipped rows (at most 100).
    pub skipped: Vec<CsvSkippedRow>,
    /// What the import did to values on the way (e.g. times converted to UTC).
    pub notes: Vec<String>,
    pub duration_ms: u64,
}

/// A CSV row the import left out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CsvSkippedRow {
    /// Data row (1-based, header excluded).
    pub row: u64,
    /// File line the row starts on.
    pub line: u64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SqlDumpPreviewRequest {
    pub path: String,
}

/// The start of a SQL dump, before "Continue".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SqlDumpPreview {
    pub path: String,
    pub file_name: String,
    /// Size of the file (compressed size for `.gz`).
    pub size: u64,
    pub compressed: bool,
    pub encoding: TextEncoding,
    /// The first statements, each cut to a few lines.
    pub statements: Vec<String>,
    /// The file has more statements than `statements`.
    pub more: bool,
    /// Byte sequences of the previewed text that are not UTF-8 (decoded as Windows-1252).
    pub invalid_utf8: u64,
    /// Line of the first of them.
    pub first_invalid_line: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SqlImportRequest {
    pub import_id: String,
    pub path: String,
    /// "Stop on error": the first failing statement ends the import.
    pub stop_on_error: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SqlImportProgress {
    /// Statements run so far (failed ones included).
    pub statements: u64,
    pub failed: u64,
    /// Bytes read from the file (compressed bytes for `.gz`).
    pub bytes: u64,
    pub total_bytes: u64,
}

/// A statement of the dump that failed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SqlImportError {
    /// 0-based statement number in the file.
    pub index: u64,
    /// 1-based line the statement starts on.
    pub line: u64,
    /// The statement, cut to its first lines.
    pub sql: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SqlImportReport {
    /// Statements that ran without error.
    pub executed: u64,
    pub failed: u64,
    /// psql meta-commands (`\connect`, `\restrict`…), which only psql understands, skipped.
    pub skipped: u64,
    /// The first errors (at most 100).
    pub errors: Vec<SqlImportError>,
    /// Stopped at an error ("Stop on error", or a statement after which nothing can go on).
    pub stopped: bool,
    pub cancelled: bool,
    /// UTF-8, or Windows-1252 when some byte sequences were not UTF-8 (only those are decoded as
    /// Windows-1252; MySQL `_binary '…'` values keep their bytes).
    pub encoding: TextEncoding,
    /// Byte sequences that are not UTF-8.
    pub invalid_utf8: u64,
    /// Line of the first of them.
    pub first_invalid_line: Option<u64>,
    /// The import ended inside the dump's own transaction (`BEGIN` … without its `COMMIT`, or a
    /// `COMMIT` of a transaction PostgreSQL had aborted): it was rolled back, undoing this many
    /// statements that had run.
    pub rolled_back: Option<u64>,
    /// What the user should know about the run (rollbacks, decoding, aborted transactions).
    pub notes: Vec<String>,
    pub duration_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_shapes() {
        let req: CsvImportRequest = serde_json::from_value(serde_json::json!({
            "importId": "i1",
            "path": "/tmp/a.csv",
            "format": {"delimiter": ";", "quote": "\"", "encoding": "windows1252", "header": true},
            "target": {"kind": "create", "table": {"schema": "public", "name": "t"},
                "columns": [{"name": "a", "ty": "integer", "precision": null, "scale": null}]},
            "mapping": ["a", null],
            "emptyAsNull": true,
            "mode": "updateOnDuplicate"
        }))
        .unwrap();
        assert_eq!(req.format.encoding, Some(TextEncoding::Windows1252));
        assert_eq!(req.mapping, vec![Some("a".to_string()), None]);
        assert_eq!(req.mode, InsertMode::UpdateOnDuplicate);
        assert!(
            matches!(req.target, ImportTarget::Create { ref columns, .. } if columns[0].ty == ImportType::Integer)
        );
        let existing: ImportTarget = serde_json::from_value(
            serde_json::json!({"kind": "existing", "table": {"schema": null, "name": "x"}}),
        )
        .unwrap();
        assert!(matches!(existing, ImportTarget::Existing { .. }));
        let p = serde_json::to_value(CsvImportProgress {
            phase: ImportPhase::Writing,
            rows_read: 1,
            rows_imported: 0,
            bytes: 2,
            total_bytes: 3,
        })
        .unwrap();
        assert_eq!(p["phase"], "writing");
        assert_eq!(p["rowsImported"], 0);
        assert_eq!(
            serde_json::to_value(TextEncoding::Utf16Le).unwrap(),
            "utf16Le"
        );
    }
}
