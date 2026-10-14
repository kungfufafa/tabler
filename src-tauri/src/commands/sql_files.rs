//! The SQL editor's files: like `read_text_file` / `write_text_file`, but `.sql.gz` files are
//! decompressed on open and compressed again on save, a file that is not UTF-8 opens (as
//! Windows-1252 / UTF-16, see `query_files::decode_sql_bytes`), and a big file is looked at
//! before it is read (`sql_file_info`) and can be opened as its first part
//! (`read_sql_file_head`) so that the editor never has to swallow a multi-hundred-MB dump.

use std::path::{Path, PathBuf};

use tauri::State;
use uuid::Uuid;

use crate::error::AppError;
use crate::locations::Locations;
use crate::query_files;
use crate::types::sql_file::{SqlFileHead, SqlFileInfo};

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?
}

/// The whole text of a file, for the ones that are known to be small. A file that is not UTF-8 is
/// decoded as Windows-1252 / UTF-16 (use `read_sql_file_head` to be told which).
#[tauri::command]
pub async fn read_sql_file(path: String) -> Result<String, AppError> {
    blocking(move || query_files::read_sql_text(Path::new(&path))).await
}

/// Size and compression of a file, without reading it.
#[tauri::command]
pub async fn sql_file_info(path: String) -> Result<SqlFileInfo, AppError> {
    blocking(move || query_files::sql_file_info(Path::new(&path))).await
}

/// The first `max_bytes` of a file as text (all of it when `max_bytes` is null), cut after a
/// complete line; a gzip file is streamed so only the head is decompressed.
#[tauri::command]
pub async fn read_sql_file_head(
    path: String,
    max_bytes: Option<u64>,
) -> Result<SqlFileHead, AppError> {
    blocking(move || query_files::read_sql_head(Path::new(&path), max_bytes)).await
}

#[tauri::command]
pub async fn write_sql_file(path: String, content: String) -> Result<(), AppError> {
    blocking(move || query_files::write_sql_text(Path::new(&path), &content)).await
}

/// The `.sql` file behind a saved query: the Queries pane's "Import…" hands it to the SQL import
/// wizard.
#[tauri::command]
pub async fn saved_query_path(
    locations: State<'_, Locations>,
    id: Uuid,
) -> Result<String, AppError> {
    let root = locations.queries_dir();
    let path: PathBuf = blocking(move || query_files::path_of(&root, id, false)).await?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A command is `async` only to be spawned: run it to completion.
    fn run<T>(f: impl std::future::Future<Output = T>) -> T {
        tauri::async_runtime::block_on(f)
    }

    #[test]
    fn the_commands_read_a_latin_1_file_as_windows_1252_with_a_note_and_a_head_at_a_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.sql");
        let mut bytes = b"-- caf\xe9\nselect 1;\n".to_vec();
        bytes.extend(b"select 'long line to be cut';\n".repeat(40));
        std::fs::write(&path, &bytes).unwrap();
        let p = path.to_string_lossy().into_owned();

        let info = run(sql_file_info(p.clone())).unwrap();
        assert_eq!(
            info,
            SqlFileInfo {
                bytes: bytes.len() as u64,
                compressed: false
            }
        );

        let whole = run(read_sql_file_head(p.clone(), None)).unwrap();
        assert!(!whole.truncated);
        assert_eq!(whole.encoding.as_deref(), Some("Windows-1252"));
        assert!(whole.text.starts_with("-- café\nselect 1;\n"));
        // The plain command opens it too, instead of failing with "not UTF-8".
        assert_eq!(run(read_sql_file(p.clone())).unwrap(), whole.text);

        let head = run(read_sql_file_head(p, Some(100))).unwrap();
        assert!(head.truncated);
        assert_eq!(head.total_bytes, bytes.len() as u64);
        assert_eq!(head.encoding.as_deref(), Some("Windows-1252"));
        assert!(head.text.ends_with('\n'), "{:?}", head.text);
        assert!(whole.text.starts_with(&head.text));
        assert!(head.text.len() < whole.text.len());
    }

    #[test]
    fn a_missing_file_or_a_folder_is_an_error_not_a_read() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.sql").to_string_lossy().into_owned();
        assert!(run(sql_file_info(missing)).is_err());
        let folder = dir.path().to_string_lossy().into_owned();
        let err = run(sql_file_info(folder)).unwrap_err();
        assert!(err.to_string().contains("not a file"), "{err}");
    }
}
