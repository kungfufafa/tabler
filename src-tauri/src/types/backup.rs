//! Backup / restore through the native dump binaries (pg_dump, pg_restore, psql, mysqldump,
//! mysql): the binaries found on this Mac, the target server, restore files, jobs and their events.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use super::profile::Driver;

/// A command-line tool the backup window runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum DumpTool {
    #[serde(rename = "pg_dump")]
    PgDump,
    #[serde(rename = "pg_restore")]
    PgRestore,
    #[serde(rename = "psql")]
    Psql,
    /// `mysqldump`, or MariaDB's `mariadb-dump`.
    #[serde(rename = "mysqldump")]
    Mysqldump,
    /// The `mysql` client, or MariaDB's `mariadb`.
    #[serde(rename = "mysql")]
    Mysql,
}

impl DumpTool {
    pub fn driver(self) -> Driver {
        match self {
            DumpTool::PgDump | DumpTool::PgRestore | DumpTool::Psql => Driver::Postgres,
            DumpTool::Mysqldump | DumpTool::Mysql => Driver::Mysql,
        }
    }

    /// The tool's usual name (MariaDB's binaries may be named differently).
    pub fn name(self) -> &'static str {
        match self {
            DumpTool::PgDump => "pg_dump",
            DumpTool::PgRestore => "pg_restore",
            DumpTool::Psql => "psql",
            DumpTool::Mysqldump => "mysqldump",
            DumpTool::Mysql => "mysql",
        }
    }
}

/// Which server family a binary (or a server) belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum VersionFlavor {
    Postgres,
    Mysql,
    Mariadb,
}

/// A version number read from `--version` output or from the server's version string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ParsedVersion {
    pub flavor: VersionFlavor,
    /// As printed, e.g. `17.6`, `9.6.24`, `8.0.36`, `11.8.9`.
    pub text: String,
    pub major: u32,
    pub minor: u32,
}

/// One dump / restore binary found on this Mac.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DumpBinary {
    pub tool: DumpTool,
    /// Absolute path, as found (not resolved through symlinks).
    pub path: String,
    /// Parsed `--version`; None when the output was not recognised.
    pub version: Option<ParsedVersion>,
    /// First line of the `--version` output.
    pub version_output: String,
}

/// What the backup window shows for a saved connection: its server and databases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BackupTarget {
    pub profile_id: Uuid,
    pub driver: Driver,
    /// The server's version string, e.g. `PostgreSQL 17.6 (Homebrew) on …` or `MySQL 9.6.0`.
    pub server_version: String,
    pub server: Option<ParsedVersion>,
    pub databases: Vec<String>,
    /// The database the session connected to (the profile's default).
    pub current_database: Option<String>,
    /// Read through an open connection (true) or a temporary one closed afterwards (false).
    pub live: bool,
}

/// What a restore file holds, from its first bytes (not its extension).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RestoreFileKind {
    /// Plain SQL text (psql / mysql).
    Sql,
    /// A pg_dump custom-format archive (`PGDMP` magic): pg_restore.
    PgCustom,
    /// A pg_dump tar-format archive: pg_restore.
    PgTar,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RestoreFileInfo {
    pub path: String,
    /// File name without its folder.
    pub name: String,
    /// Size on disk (compressed size for a gzip file).
    pub size: u64,
    pub kind: RestoreFileKind,
    /// The file is gzip-compressed (it is decompressed into the binary's stdin).
    pub gzip: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum BackupJobKind {
    Backup,
    Restore,
}

/// Starts a backup (dump `database` into `path`) or a restore (`path` into `database`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BackupJobRequest {
    /// Chosen by the renderer; `cancel_backup_job` takes the same id.
    pub job_id: String,
    pub kind: BackupJobKind,
    pub profile_id: Uuid,
    pub database: String,
    /// One of the binaries `list_dump_binaries` returned.
    pub binary_path: String,
    /// Extra flags, one chip each (`--no-owner`, `-v ON_ERROR_STOP=1`); a chip may hold a flag
    /// and its value separated by spaces, quoted like a shell word when the value has spaces.
    pub flags: Vec<String>,
    /// Backup: gzip the (plain SQL) output.
    pub gzip: bool,
    /// Backup: absolute path of the file to write (a free name is picked when it exists; missing
    /// folders are created). Restore: the file to read.
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum BackupLogStream {
    Stdout,
    Stderr,
    /// A note from Tabler itself (the command line, a TLS downgrade, truncated output…).
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BackupLogLine {
    pub stream: BackupLogStream,
    pub text: String,
}

/// Streamed while a job runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "event",
    content = "data",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export, rename_all_fields = "camelCase")]
pub enum BackupEvent {
    /// The binary is running. `command` is its command line (it never holds the password, which
    /// only goes in the environment); `path` the file written (backup) or read (restore).
    Started { command: String, path: String },
    /// Output lines, batched.
    Log { lines: Vec<BackupLogLine> },
    /// Backup: bytes written to the file. Restore: bytes of the file read so far, out of `total`
    /// (None when the binary reads the file itself).
    Progress { bytes: u64, total: Option<u64> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BackupJobResult {
    /// The file written (backup) or read (restore).
    pub path: String,
    pub bytes: u64,
    pub duration_ms: u64,
    /// Output lines that reported an error although the binary succeeded (psql goes on after a
    /// failed statement unless `ON_ERROR_STOP` is set).
    pub errors: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_shapes() {
        assert_eq!(
            serde_json::to_value(DumpTool::PgRestore).unwrap(),
            serde_json::json!("pg_restore")
        );
        let event = BackupEvent::Progress {
            bytes: 10,
            total: Some(20),
        };
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({"event": "progress", "data": {"bytes": 10, "total": 20}})
        );
        let req: BackupJobRequest = serde_json::from_value(serde_json::json!({
            "jobId": "j1",
            "kind": "restore",
            "profileId": "7b0e0b2c-6f43-4d2a-9b7a-4a6a1f0b8c11",
            "database": "app",
            "binaryPath": "/opt/homebrew/bin/psql",
            "flags": ["-v ON_ERROR_STOP=1"],
            "gzip": false,
            "path": "/tmp/app.sql"
        }))
        .unwrap();
        assert_eq!(req.kind, BackupJobKind::Restore);
        assert_eq!(
            serde_json::to_value(RestoreFileKind::PgCustom).unwrap(),
            serde_json::json!("pgCustom")
        );
    }
}
