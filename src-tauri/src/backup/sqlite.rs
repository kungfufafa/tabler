//! SQLite has no dump binary: "Copy database file…" writes a consistent copy of the database file
//! (including what still sits in its WAL) with `VACUUM INTO` on a read-only connection.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::error::AppError;
use crate::sqlgen::backup::SQLITE_VACUUM_INTO;

/// Copies the SQLite database at `src` to `dest` (replacing it: the save panel already asked).
/// Returns the size of the copy.
pub fn copy_database(src: &Path, dest: &Path) -> Result<u64, AppError> {
    if !dest.is_absolute() {
        return Err(AppError::InvalidInput(
            "the destination path must be absolute".into(),
        ));
    }
    if !src.is_file() {
        return Err(AppError::NotFound(format!(
            "SQLite database file {}",
            src.display()
        )));
    }
    let same = match (std::fs::canonicalize(src), std::fs::canonicalize(dest)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if same {
        return Err(AppError::InvalidInput(
            "choose another file than the database itself".into(),
        ));
    }
    let dir = dest
        .parent()
        .ok_or_else(|| AppError::InvalidInput("the destination has no folder".into()))?;
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "copy.sqlite".into());
    // VACUUM INTO needs a file that does not exist yet.
    let tmp: PathBuf = dir.join(format!(
        ".{name}.{}.partial",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    ));
    let result = (|| {
        let conn = Connection::open_with_flags(
            src,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.execute(SQLITE_VACUUM_INTO, [tmp.to_string_lossy().as_ref()])?;
        drop(conn);
        std::fs::rename(&tmp, dest)
            .map_err(|e| AppError::Io(format!("cannot save {}: {e}", dest.display())))?;
        Ok(std::fs::metadata(dest)?.len())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("app.sqlite");
        let conn = Connection::open(&src).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; CREATE TABLE t (v TEXT); INSERT INTO t VALUES ('a');",
        )
        .unwrap();
        let dest = dir.path().join("copy.sqlite");
        std::fs::write(&dest, "old").unwrap();
        // The source connection stays open: its WAL content is copied too.
        let size = copy_database(&src, &dest).unwrap();
        assert_eq!(size, std::fs::metadata(&dest).unwrap().len());
        let copy = Connection::open(&dest).unwrap();
        let v: String = copy.query_row("SELECT v FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "a");
        let leftovers = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".partial")
            })
            .count();
        assert_eq!(leftovers, 0);
        assert!(copy_database(&src, &src).is_err());
        assert!(copy_database(&dir.path().join("missing.sqlite"), &dest).is_err());
        assert!(copy_database(&src, Path::new("relative.sqlite")).is_err());
    }
}
