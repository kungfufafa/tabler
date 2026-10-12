//! What a restore file holds, from its first bytes: plain SQL, a pg_dump custom archive (`PGDMP`)
//! or a pg_dump tar archive, each optionally gzip-compressed.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use flate2::read::MultiGzDecoder;

use crate::error::AppError;
use crate::types::backup::{RestoreFileInfo, RestoreFileKind};

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
const PG_CUSTOM_MAGIC: &[u8] = b"PGDMP";
/// `ustar` at offset 257 of the first tar header.
const TAR_MAGIC_OFFSET: usize = 257;
const TAR_MAGIC: &[u8] = b"ustar";
/// Enough of the (decompressed) head to see every magic.
const HEAD_LEN: usize = 512;

/// The kind of an uncompressed head.
pub fn kind_of(head: &[u8]) -> RestoreFileKind {
    if head.starts_with(PG_CUSTOM_MAGIC) {
        RestoreFileKind::PgCustom
    } else if head.len() >= TAR_MAGIC_OFFSET + TAR_MAGIC.len()
        && &head[TAR_MAGIC_OFFSET..TAR_MAGIC_OFFSET + TAR_MAGIC.len()] == TAR_MAGIC
    {
        RestoreFileKind::PgTar
    } else {
        RestoreFileKind::Sql
    }
}

pub fn is_gzip(head: &[u8]) -> bool {
    head.starts_with(&GZIP_MAGIC)
}

/// Reads up to `len` bytes (fewer only at the end of the stream).
fn read_head(mut r: impl Read, len: usize) -> io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(len);
    r.by_ref().take(len as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Detects the kind of any stream: `(kind, gzip)`.
pub fn detect_stream(mut r: impl Read) -> io::Result<(RestoreFileKind, bool)> {
    let head = read_head(&mut r, HEAD_LEN)?;
    if is_gzip(&head) {
        let rest = io::Cursor::new(head).chain(r);
        let inner = read_head(MultiGzDecoder::new(rest), HEAD_LEN)?;
        return Ok((kind_of(&inner), true));
    }
    Ok((kind_of(&head), false))
}

/// Inspects a file the user picked for a restore.
pub fn inspect_file(path: &Path) -> Result<RestoreFileInfo, AppError> {
    if !path.is_absolute() {
        return Err(AppError::InvalidInput(
            "the restore file path must be absolute".into(),
        ));
    }
    let meta = std::fs::metadata(path)
        .map_err(|e| AppError::Io(format!("cannot read {}: {e}", path.display())))?;
    if !meta.is_file() {
        return Err(AppError::InvalidInput(format!(
            "{} is not a file",
            path.display()
        )));
    }
    let file = File::open(path)
        .map_err(|e| AppError::Io(format!("cannot open {}: {e}", path.display())))?;
    let (kind, gzip) = detect_stream(file).map_err(|e| {
        AppError::InvalidInput(format!(
            "{} could not be read as a dump: {e}",
            path.display()
        ))
    })?;
    Ok(RestoreFileInfo {
        path: path.to_string_lossy().into_owned(),
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        size: meta.len(),
        kind,
        gzip,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::GzEncoder;

    use super::*;

    fn gz(data: &[u8]) -> Vec<u8> {
        let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    fn tar_head() -> Vec<u8> {
        let mut head = vec![0u8; 512];
        head[..9].copy_from_slice(b"toc.dat\0\0");
        head[257..263].copy_from_slice(b"ustar\0");
        head
    }

    #[test]
    fn detects_kinds_by_magic() {
        let sql = b"--\n-- PostgreSQL database dump\n--\nSET statement_timeout = 0;\n";
        assert_eq!(
            detect_stream(&sql[..]).unwrap(),
            (RestoreFileKind::Sql, false)
        );
        let custom = b"PGDMP\x01\x10\x00\x04\x08\x08\x01\x01";
        assert_eq!(
            detect_stream(&custom[..]).unwrap(),
            (RestoreFileKind::PgCustom, false)
        );
        assert_eq!(
            detect_stream(&tar_head()[..]).unwrap(),
            (RestoreFileKind::PgTar, false)
        );
        assert_eq!(
            detect_stream(&gz(sql)[..]).unwrap(),
            (RestoreFileKind::Sql, true)
        );
        assert_eq!(
            detect_stream(&gz(custom)[..]).unwrap(),
            (RestoreFileKind::PgCustom, true)
        );
        // Empty and tiny files are plain SQL.
        assert_eq!(
            detect_stream(&b""[..]).unwrap(),
            (RestoreFileKind::Sql, false)
        );
        assert_eq!(
            detect_stream(&b"PG"[..]).unwrap(),
            (RestoreFileKind::Sql, false)
        );
        // "PGDMP" further in is not a custom archive.
        assert_eq!(
            detect_stream(&b"-- PGDMP"[..]).unwrap(),
            (RestoreFileKind::Sql, false)
        );
    }

    #[test]
    fn a_broken_gzip_file_is_an_error() {
        // A gzip header cut short.
        assert!(detect_stream(&[0x1f, 0x8b, 0x08, 0x00, 0xff][..]).is_err());
    }

    #[test]
    fn inspects_files_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.dump");
        std::fs::write(&path, b"PGDMP\x01\x0e\x00").unwrap();
        let info = inspect_file(&path).unwrap();
        assert_eq!(info.kind, RestoreFileKind::PgCustom);
        assert!(!info.gzip);
        assert_eq!(info.name, "app.dump");
        assert_eq!(info.size, 8);
        let gz_path = dir.path().join("app.sql.gz");
        std::fs::write(&gz_path, gz(b"CREATE TABLE t (id int);\n")).unwrap();
        let info = inspect_file(&gz_path).unwrap();
        assert_eq!((info.kind, info.gzip), (RestoreFileKind::Sql, true));
        assert!(matches!(
            inspect_file(Path::new("relative.sql")),
            Err(AppError::InvalidInput(_))
        ));
        assert!(matches!(
            inspect_file(&dir.path().join("missing.sql")),
            Err(AppError::Io(_))
        ));
        assert!(matches!(
            inspect_file(dir.path()),
            Err(AppError::InvalidInput(_))
        ));
    }
}
