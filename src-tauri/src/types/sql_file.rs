//! The SQL editor's file commands (`commands::sql_files`): what is known about a file before it
//! is read, and the first part of a file.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What `sql_file_info` knows about a file without reading its content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SqlFileInfo {
    /// Size on disk (the compressed size of a gzip file).
    pub bytes: u64,
    /// The file is gzip-compressed (recognised by its content, not its name).
    pub compressed: bool,
}

/// The text of an SQL file, or of its first part (`read_sql_file_head`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SqlFileHead {
    /// The text, decoded: UTF-8 as it is, anything else as the encoding named in `encoding`. A
    /// truncated head ends at a line boundary (a whole line is never cut in two).
    pub text: String,
    /// Size on disk (the compressed size of a gzip file).
    pub total_bytes: u64,
    /// The file goes on past `text` (the head was cut at the requested size).
    pub truncated: bool,
    pub compressed: bool,
    /// Set when the file was not UTF-8: the encoding it was read as ("Windows-1252", "UTF-16 LE",
    /// "UTF-16 BE"). Saving the buffer writes UTF-8.
    pub encoding: Option<String>,
}
