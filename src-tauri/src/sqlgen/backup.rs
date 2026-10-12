//! SQL of the backup feature. The dump binaries generate their own SQL; only SQLite's file copy
//! runs a statement.

/// Writes a consistent copy of the main database (WAL content included) to the file named by the
/// bound parameter, which must not exist yet. Works on a read-only connection.
pub const SQLITE_VACUUM_INTO: &str = "VACUUM main INTO ?1";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vacuum_into_copies_a_database() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.sqlite");
        let dest = dir.path().join("dest.sqlite");
        let conn = rusqlite::Connection::open(&src).unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER); INSERT INTO t VALUES (1), (2);")
            .unwrap();
        conn.execute(SQLITE_VACUUM_INTO, [dest.to_string_lossy().as_ref()])
            .unwrap();
        let copy = rusqlite::Connection::open(&dest).unwrap();
        let n: i64 = copy
            .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }
}
