//! Small file helpers: the SQL editor's open/save .sql, the grid's Set Value ▸ Add a File….
//! Paths come from the native dialog.

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::error::AppError;
use crate::types::value::Value;

/// Largest file Set Value ▸ Add a File… loads into a cell.
pub const BINARY_FILE_MAX: u64 = 64 * 1024 * 1024;

#[tauri::command]
pub async fn read_text_file(path: String) -> Result<String, AppError> {
    tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| AppError::Io(format!("{path}: {e}")))
}

#[tauri::command]
pub async fn write_text_file(path: String, content: String) -> Result<(), AppError> {
    tokio::fs::write(&path, content)
        .await
        .map_err(|e| AppError::Io(format!("{path}: {e}")))
}

/// Reads a whole file as a bytes value (base64 over IPC), for staging into a blob cell.
#[tauri::command]
pub async fn read_binary_file(path: String) -> Result<Value, AppError> {
    read_binary_file_inner(&path, BINARY_FILE_MAX).await
}

/// Everything `reader` holds, or None when it holds more than `max` bytes (never reads past
/// `max + 1`).
async fn read_capped(reader: impl AsyncRead + Unpin, max: u64) -> std::io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    reader
        .take(max.saturating_add(1))
        .read_to_end(&mut bytes)
        .await?;
    Ok((bytes.len() as u64 <= max).then_some(bytes))
}

async fn read_binary_file_inner(path: &str, max: u64) -> Result<Value, AppError> {
    let io = |e: std::io::Error| AppError::Io(format!("{path}: {e}"));
    let too_large = |size: String| {
        AppError::InvalidInput(format!(
            "{path} is too large ({size}); files up to {} MB can be loaded into a cell",
            max / (1024 * 1024)
        ))
    };
    // Devices and pipes report no size (and a FIFO's open would wait for a writer): refused.
    let meta = tokio::fs::metadata(path).await.map_err(io)?;
    if !meta.is_file() {
        return Err(AppError::InvalidInput(format!(
            "{path} is not a regular file"
        )));
    }
    if meta.len() > max {
        return Err(too_large(format!("{} bytes", meta.len())));
    }
    let file = tokio::fs::File::open(path).await.map_err(io)?;
    // The size is checked again while reading: the file may grow in between.
    let bytes = read_capped(file, max)
        .await
        .map_err(io)?
        .ok_or_else(|| too_large(format!("over {max} bytes")))?;
    Ok(Value::from_bytes_uncapped(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_files_as_whole_bytes_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob.bin");
        let data: Vec<u8> = (0..70_000u32).map(|i| (i % 256) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        let path = path.to_string_lossy().into_owned();
        let v = read_binary_file_inner(&path, BINARY_FILE_MAX)
            .await
            .unwrap();
        assert_eq!(v, Value::from_bytes_uncapped(&data));
        assert!(matches!(v, Value::Bytes(b) if !b.truncated && b.len == 70_000));
        // Over the cap: refused before reading.
        assert!(matches!(
            read_binary_file_inner(&path, 1_000).await,
            Err(AppError::InvalidInput(_))
        ));
        assert!(matches!(
            read_binary_file_inner("/nonexistent/tabler-grid.bin", BINARY_FILE_MAX).await,
            Err(AppError::Io(_))
        ));
    }

    #[tokio::test]
    async fn refuses_devices_and_directories() {
        // /dev/zero reports a size of 0 and never ends.
        for path in ["/dev/zero", "/tmp"] {
            let err = read_binary_file_inner(path, BINARY_FILE_MAX)
                .await
                .unwrap_err();
            assert!(err.to_string().contains("not a regular file"), "{err}");
        }
    }

    #[tokio::test]
    async fn reads_at_most_one_byte_past_the_cap() {
        let data = [7u8; 10];
        assert_eq!(
            read_capped(&data[..], 10).await.unwrap(),
            Some(data.to_vec())
        );
        assert_eq!(read_capped(&data[..], 9).await.unwrap(), None);
        // An endless reader stops at the cap.
        assert_eq!(
            read_capped(tokio::io::repeat(1), 1_000).await.unwrap(),
            None
        );
        assert_eq!(read_capped(&[][..], 0).await.unwrap(), Some(Vec::new()));
    }
}
