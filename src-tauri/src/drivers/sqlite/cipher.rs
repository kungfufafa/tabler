//! SQLCipher: encrypted SQLite databases (b100). The key is the SQLite profile's password (same
//! Keychain entry and storage modes); it is set with `PRAGMA key` right after a connection opens,
//! before any other statement.

use std::path::Path;

use rusqlite::{Connection, ErrorCode, OpenFlags};

use crate::error::AppError;
use crate::sqlgen::conn_options as sql;

/// What a wrong key (or an encrypted file that is not SQLCipher's) reports.
pub const WRONG_KEY: &str = "Wrong encryption key (or not a SQLCipher database)";
/// A key was given for a database that is not encrypted.
pub const NOT_ENCRYPTED: &str =
    "This database is not encrypted: remove the encryption key from the connection";
/// Oldest and newest `PRAGMA cipher_compatibility` values (SQLCipher 1–4).
pub use crate::conn_options::CIPHER_COMPATIBILITY as COMPATIBILITY_RANGE;

/// The key of an encrypted database and the SQLCipher major version whose format it uses.
#[derive(Clone, PartialEq, Eq)]
pub struct Cipher {
    pub key: String,
    /// `PRAGMA cipher_compatibility`; None: SQLCipher 4's defaults.
    pub compatibility: Option<u8>,
}

impl std::fmt::Debug for Cipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cipher")
            .field("key", &"…")
            .field("compatibility", &self.compatibility)
            .finish()
    }
}

impl Cipher {
    /// The cipher of a connect: `key` (the profile's password) when it is not empty.
    pub fn from_key(key: Option<&str>, compatibility: Option<u8>) -> Option<Cipher> {
        key.filter(|k| !k.is_empty()).map(|k| Cipher {
            key: k.to_string(),
            compatibility: compatibility.filter(|v| COMPATIBILITY_RANGE.contains(v)),
        })
    }
}

/// Keys a freshly opened connection, then reads the schema so a wrong key fails now with
/// [`WRONG_KEY`] instead of at the first query.
pub fn apply(conn: &Connection, cipher: &Cipher) -> Result<(), AppError> {
    conn.execute_batch(&sql::sqlite_key(&cipher.key))?;
    if let Some(version) = cipher.compatibility {
        conn.execute_batch(&sql::sqlite_cipher_compatibility(version))?;
    }
    conn.query_row(sql::SQLITE_KEY_CHECK, [], |_| Ok(()))
        .map_err(key_error)
}

/// `file is not a database` (what SQLCipher says for a wrong key) as [`WRONG_KEY`].
pub fn key_error(e: rusqlite::Error) -> AppError {
    match e.sqlite_error_code() {
        Some(ErrorCode::NotADatabase) => AppError::Connect(WRONG_KEY.into()),
        _ => e.into(),
    }
}

/// Writes a consistent encrypted copy of the database at `src` to `dest` (which must not exist):
/// SQLCipher's `sqlcipher_export` into a copy attached with the same key, so the copy opens with
/// the same key and compatibility setting. Used by Backup ▸ "Copy Database File…" for an
/// encrypted database (`VACUUM INTO` cannot read one without its key).
pub fn copy_encrypted(src: &Path, dest: &Path, cipher: &Cipher) -> Result<(), AppError> {
    // The source opens read-only (`mode=ro`), the connection read-write so that the copy can
    // be attached (created).
    let conn = Connection::open_with_flags(
        read_only_uri(src),
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    // The database may be open elsewhere (the driver's connection, a writer): wait for it.
    conn.busy_timeout(std::time::Duration::from_millis(5000))?;
    apply(&conn, cipher)?;
    let dest = dest.to_string_lossy().into_owned();
    conn.execute(
        sql::SQLITE_ATTACH_ENCRYPTED_COPY,
        rusqlite::params![dest, cipher.key],
    )?;
    if let Some(version) = cipher.compatibility {
        conn.execute_batch(&sql::sqlite_copy_cipher_compatibility(version))?;
    }
    conn.query_row(sql::SQLITE_EXPORT_TO_COPY, [], |_| Ok(()))?;
    conn.execute_batch(sql::SQLITE_DETACH_COPY)?;
    Ok(())
}

/// Whether `dest` is the file `src` (by path, symbolic links and `..` resolved): a destination
/// that does not exist yet is its resolved folder plus its name.
fn is_same_file(src: &Path, dest: &Path) -> bool {
    let Ok(src) = std::fs::canonicalize(src) else {
        return false;
    };
    let dest = std::fs::canonicalize(dest).or_else(|_| {
        let name = dest
            .file_name()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let dir = dest.parent().unwrap_or(Path::new(""));
        std::fs::canonicalize(dir).map(|dir| dir.join(name))
    });
    dest.is_ok_and(|dest| dest == src)
}

/// Backup ▸ "Copy Database File…" of an encrypted database: [`copy_encrypted`] into a temporary
/// file next to `dest`, which then replaces `dest` (the save panel already asked). Returns the
/// size of the copy. As with a plain database, `dest` must be an absolute path and not the
/// database itself: the replace would swap the file the open connections use for a new one.
pub fn copy_encrypted_to(src: &Path, dest: &Path, cipher: &Cipher) -> Result<u64, AppError> {
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
    if is_same_file(src, dest) {
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
    let tmp = dir.join(format!(
        ".{name}.{}.partial",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    ));
    let result = copy_encrypted(src, &tmp, cipher).and_then(|()| {
        std::fs::rename(&tmp, dest)
            .map_err(|e| AppError::Io(format!("cannot save {}: {e}", dest.display())))?;
        Ok(std::fs::metadata(dest)?.len())
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// `file:<path>?mode=ro`, the path's `%`, `?` and `#` escaped.
fn read_only_uri(path: &Path) -> String {
    let mut uri = String::from("file:");
    for c in path.to_string_lossy().chars() {
        match c {
            '%' => uri.push_str("%25"),
            '?' => uri.push_str("%3f"),
            '#' => uri.push_str("%23"),
            c => uri.push(c),
        }
    }
    uri.push_str("?mode=ro");
    uri
}

/// Opens (creating it) the database at `path` with `cipher` and writes its first page, so a new
/// database file is encrypted from the start.
pub fn create_encrypted(path: &str, cipher: &Cipher) -> Result<(), AppError> {
    let cannot = |e: AppError| AppError::Io(format!("cannot create {path}: {e}"));
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| cannot(e.into()))?;
    apply(&conn, cipher).map_err(cannot)?;
    // Writing the (unchanged) user version writes the first, encrypted, page.
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| cannot(e.into()))?;
    conn.execute_batch(&format!("PRAGMA user_version = {version};"))
        .map_err(|e| cannot(e.into()))?;
    conn.close().map_err(|(_, e)| cannot(e.into()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher() -> Cipher {
        Cipher {
            key: "s3cret".into(),
            compatibility: None,
        }
    }

    /// An encrypted database with a table `t` of three rows.
    fn encrypted_db(path: &Path) {
        create_encrypted(&path.to_string_lossy(), &cipher()).unwrap();
        let conn = Connection::open(path).unwrap();
        apply(&conn, &cipher()).unwrap();
        conn.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1), (2), (3);")
            .unwrap();
    }

    fn rows(path: &Path) -> i64 {
        let conn = Connection::open(path).unwrap();
        apply(&conn, &cipher()).unwrap();
        conn.query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap()
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_database_itself_is_not_a_destination() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("vault.sqlite");
        encrypted_db(&src);
        let before = std::fs::read(&src).unwrap();
        let ino = std::fs::metadata(&src).unwrap();

        let err = copy_encrypted_to(&src, &src, &cipher()).unwrap_err();
        assert!(
            matches!(&err, AppError::InvalidInput(m) if m.contains("another file")),
            "{err:?}"
        );
        // Other spellings of the same path are the same file.
        let dotted = dir.path().join(".").join("vault.sqlite");
        assert!(copy_encrypted_to(&src, &dotted, &cipher()).is_err());
        let around = dir.path().join("sub").join("..").join("vault.sqlite");
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        assert!(copy_encrypted_to(&src, &around, &cipher()).is_err());
        #[cfg(unix)]
        {
            let link = dir.path().join("link.sqlite");
            std::os::unix::fs::symlink(&src, &link).unwrap();
            assert!(copy_encrypted_to(&src, &link, &cipher()).is_err());
        }

        // The live file was not replaced: same bytes, same file, still opens with the key, and
        // no partial copy is left behind.
        assert_eq!(std::fs::read(&src).unwrap(), before);
        let after = std::fs::metadata(&src).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(after.ino(), ino.ino());
        }
        assert_eq!(after.len(), ino.len());
        assert_eq!(rows(&src), 3);
        let mut expected = vec!["vault.sqlite".to_string(), "sub".to_string()];
        #[cfg(unix)]
        expected.push("link.sqlite".into());
        expected.sort();
        assert_eq!(files_in(dir.path()), expected);
    }

    #[test]
    fn the_destination_must_be_absolute_and_the_source_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("vault.sqlite");
        encrypted_db(&src);
        let relative = Path::new("tabler-relative-copy.sqlite");
        let err = copy_encrypted_to(&src, relative, &cipher()).unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)), "{err:?}");
        assert!(!relative.exists());
        let err = copy_encrypted_to(
            &dir.path().join("none.sqlite"),
            &dir.path().join("c.sqlite"),
            &cipher(),
        )
        .unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)), "{err:?}");
        assert!(!dir.path().join("c.sqlite").exists());
        let err =
            copy_encrypted_to(dir.path(), &dir.path().join("c.sqlite"), &cipher()).unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)), "{err:?}");
    }

    #[test]
    fn a_copy_to_another_path_still_works() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("vault.sqlite");
        encrypted_db(&src);
        // A new file, and one that exists (the save panel asked to replace it).
        let fresh = dir.path().join("copy.sqlite");
        let size = copy_encrypted_to(&src, &fresh, &cipher()).unwrap();
        assert_eq!(size, std::fs::metadata(&fresh).unwrap().len());
        assert_eq!(rows(&fresh), 3);
        let old = dir.path().join("old.sqlite");
        std::fs::write(&old, "old").unwrap();
        copy_encrypted_to(&src, &old, &cipher()).unwrap();
        assert_eq!(rows(&old), 3);
        assert_eq!(rows(&src), 3);
        assert_eq!(
            files_in(dir.path()),
            ["copy.sqlite", "old.sqlite", "vault.sqlite"]
        );
    }

    #[test]
    fn a_copy_waits_for_a_database_that_is_busy() {
        // The driver's connection may hold the whole file for a moment (a commit): the copy waits
        // for it instead of failing with "database is locked".
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("vault.sqlite");
        encrypted_db(&src);
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let path = src.clone();
        let holder = std::thread::spawn(move || {
            let writer = Connection::open(&path).unwrap();
            apply(&writer, &cipher()).unwrap();
            writer
                .execute_batch("BEGIN EXCLUSIVE; INSERT INTO t VALUES (4);")
                .unwrap();
            locked_tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(700));
            writer.execute_batch("COMMIT").unwrap();
        });
        locked_rx.recv().unwrap();
        let dest = dir.path().join("copy.sqlite");
        copy_encrypted(&src, &dest, &cipher()).unwrap();
        holder.join().unwrap();
        // What was committed when the copy got its turn.
        assert_eq!(rows(&dest), 4);
    }
}
