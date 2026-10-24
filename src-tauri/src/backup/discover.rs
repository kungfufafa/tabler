//! Finds the dump / restore binaries installed on this Mac. Apps started from Finder get a minimal
//! `PATH`, so the usual Homebrew, Postgres.app and /usr/local locations are searched too.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::process::NoWindow as _;
use crate::types::backup::{DumpBinary, DumpTool};
use crate::types::profile::Driver;

use super::version::{parse_tool_version, version_line};

/// Searched after `PATH`, in this order. A `*` matches any part of one path segment.
pub const SEARCH_PATTERNS: &[&str] = &[
    "/opt/homebrew/bin",
    "/opt/homebrew/opt/postgresql@*/bin",
    "/opt/homebrew/opt/libpq/bin",
    "/opt/homebrew/opt/libpq@*/bin",
    // Covers mysql, mysql@8.4, mysql-client and mysql-client@8.0.
    "/opt/homebrew/opt/mysql*/bin",
    "/opt/homebrew/opt/mariadb*/bin",
    "/usr/local/bin",
    "/usr/local/opt/*/bin",
    "/Applications/Postgres.app/Contents/Versions/*/bin",
];

/// File names of each tool (MariaDB installs `mariadb-dump` / `mariadb`, plus deprecated
/// `mysqldump` / `mysql` links).
pub fn tool_names(driver: Driver) -> &'static [(&'static str, DumpTool)] {
    match driver {
        Driver::Postgres => &[
            ("pg_dump", DumpTool::PgDump),
            ("pg_restore", DumpTool::PgRestore),
            ("psql", DumpTool::Psql),
        ],
        Driver::Mysql => &[
            ("mysqldump", DumpTool::Mysqldump),
            ("mariadb-dump", DumpTool::Mysqldump),
            ("mysql", DumpTool::Mysql),
            ("mariadb", DumpTool::Mysql),
        ],
        // SQLite copies its file; the other drivers have no dump tool.
        _ => &[],
    }
}

/// A file name without the platform's executable suffix (`pg_dump.exe` on Windows, in any case);
/// elsewhere the suffix is empty and the name is unchanged.
fn without_exe_suffix(name: &str) -> &str {
    let suffix = std::env::consts::EXE_SUFFIX;
    let cut = name.len().saturating_sub(suffix.len());
    match name.get(cut..) {
        Some(end) if cut > 0 && end.eq_ignore_ascii_case(suffix) => &name[..cut],
        _ => name,
    }
}

/// The tool a file name stands for, among the driver's tools.
pub fn tool_for_file(driver: Driver, path: &Path) -> Option<DumpTool> {
    let name = without_exe_suffix(path.file_name()?.to_str()?);
    tool_names(driver)
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, t)| *t)
}

/// Expands one search pattern below `root` (`/` in the app; a temp dir in tests). Matches are
/// sorted by name, newest-looking versions first (`postgresql@17` before `postgresql@16`).
pub fn expand_pattern(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let mut current = vec![root.to_path_buf()];
    for segment in pattern.split('/').filter(|s| !s.is_empty()) {
        let mut next = Vec::new();
        for dir in &current {
            match segment.split_once('*') {
                None => next.push(dir.join(segment)),
                Some((prefix, suffix)) => {
                    let Ok(entries) = std::fs::read_dir(dir) else {
                        continue;
                    };
                    let mut names: Vec<String> = entries
                        .filter_map(Result::ok)
                        .filter_map(|e| e.file_name().into_string().ok())
                        .filter(|n| {
                            n.len() >= prefix.len() + suffix.len()
                                && n.starts_with(prefix)
                                && n.ends_with(suffix)
                        })
                        .collect();
                    names.sort_by_key(|n| std::cmp::Reverse(natural_key(n)));
                    next.extend(names.into_iter().map(|n| dir.join(n)));
                }
            }
        }
        current = next;
    }
    current.into_iter().filter(|p| p.is_dir()).collect()
}

/// Sort key that orders embedded numbers numerically (`@9` < `@17`).
fn natural_key(name: &str) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    let mut digits = String::new();
    let mut text = String::new();
    for ch in name.chars() {
        if ch.is_ascii_digit() {
            if !text.is_empty() {
                out.push((0, std::mem::take(&mut text)));
            }
            digits.push(ch);
        } else {
            if !digits.is_empty() {
                out.push((digits.parse().unwrap_or(u64::MAX), String::new()));
                digits.clear();
            }
            text.push(ch);
        }
    }
    if !digits.is_empty() {
        out.push((digits.parse().unwrap_or(u64::MAX), String::new()));
    }
    if !text.is_empty() {
        out.push((0, text));
    }
    out
}

/// The folders to search: `PATH` first, then [`SEARCH_PATTERNS`] below `root`; no duplicates.
pub fn search_dirs(root: &Path, path_env: Option<&OsStr>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let from_path = path_env
        .map(|p| std::env::split_paths(p).collect::<Vec<_>>())
        .unwrap_or_default();
    let patterns = SEARCH_PATTERNS.iter().flat_map(|p| expand_pattern(root, p));
    for dir in from_path.into_iter().chain(patterns) {
        if dir.as_os_str().is_empty() || !dir.is_absolute() {
            continue;
        }
        if seen.insert(dir.clone()) {
            out.push(dir);
        }
    }
    out
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Every executable of the driver's tools in `dirs`, in search order. The same binary reached
/// through several links (`/opt/homebrew/bin/mysqldump` → `Cellar/…`) is listed once, at the
/// first place it was found.
pub fn find_binaries(dirs: &[PathBuf], driver: Driver) -> Vec<(DumpTool, PathBuf)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for dir in dirs {
        for (name, tool) in tool_names(driver) {
            let path = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
            if !is_executable(&path) {
                continue;
            }
            let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if seen.insert(key) {
                out.push((*tool, path));
            }
        }
    }
    out
}

/// Bound on one `--version` run (a hung binary must not hold up the window).
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(4);

/// Runs `<path> --version` and returns its combined output; None when it does not run.
pub async fn version_output(path: &Path) -> Option<String> {
    let run = tokio::process::Command::new(path)
        .no_window()
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(VERSION_TIMEOUT, run)
        .await
        .ok()?
        .ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some(text)
}

/// Builds the entry of one found binary from its `--version` output.
pub fn describe(tool: DumpTool, path: &Path, output: &str) -> DumpBinary {
    DumpBinary {
        tool,
        path: path.to_string_lossy().into_owned(),
        version: parse_tool_version(output),
        version_output: version_line(output),
    }
}

/// Sorts binaries by tool, then newest version first (unknown versions last), then path.
pub fn sort_binaries(list: &mut [DumpBinary]) {
    let rank = |t: DumpTool| t as u8;
    list.sort_by(|a, b| {
        rank(a.tool).cmp(&rank(b.tool)).then_with(|| {
            let key = |x: &DumpBinary| x.version.as_ref().map(|v| (v.major, v.minor));
            key(b)
                .cmp(&key(a))
                .then_with(|| {
                    b.version
                        .as_ref()
                        .map(|v| &v.text)
                        .cmp(&a.version.as_ref().map(|v| &v.text))
                })
                .then_with(|| a.path.cmp(&b.path))
        })
    });
}

/// Discovers the driver's binaries below `root` (with `PATH`) and reads their versions.
pub async fn discover(root: &Path, path_env: Option<&OsStr>, driver: Driver) -> Vec<DumpBinary> {
    let dirs = search_dirs(root, path_env);
    let found = find_binaries(&dirs, driver);
    let probes = found.into_iter().map(|(tool, path)| async move {
        let output = version_output(&path).await?;
        Some(describe(tool, &path, &output))
    });
    let mut list: Vec<DumpBinary> = futures::future::join_all(probes)
        .await
        .into_iter()
        .flatten()
        .collect();
    sort_binaries(&mut list);
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    // The binary-probing tests run shell scripts, so they and their helpers are Unix-only.
    #[cfg(unix)]
    use crate::types::backup::{ParsedVersion, VersionFlavor};

    #[cfg(unix)]
    fn touch_exe(path: &Path, script: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn expands_patterns_newest_first() {
        let root = tempfile::tempdir().unwrap();
        for d in [
            "opt/homebrew/opt/postgresql@9.6/bin",
            "opt/homebrew/opt/postgresql@17/bin",
            "opt/homebrew/opt/postgresql@16/bin",
            "opt/homebrew/opt/postgresql@15", // no bin folder
            "opt/homebrew/opt/mysql-client@8.0/bin",
            "opt/homebrew/opt/mysql/bin",
            "opt/homebrew/opt/redis/bin",
        ] {
            std::fs::create_dir_all(root.path().join(d)).unwrap();
        }
        // Compared as paths: the matches use the platform's separator (`\` on Windows).
        let rel = |p: &PathBuf| p.strip_prefix(root.path()).unwrap().to_path_buf();
        let paths = |list: &[&str]| list.iter().map(PathBuf::from).collect::<Vec<_>>();
        let pg: Vec<PathBuf> = expand_pattern(root.path(), "/opt/homebrew/opt/postgresql@*/bin")
            .iter()
            .map(rel)
            .collect();
        assert_eq!(
            pg,
            paths(&[
                "opt/homebrew/opt/postgresql@17/bin",
                "opt/homebrew/opt/postgresql@16/bin",
                "opt/homebrew/opt/postgresql@9.6/bin",
            ])
        );
        let my: Vec<PathBuf> = expand_pattern(root.path(), "/opt/homebrew/opt/mysql*/bin")
            .iter()
            .map(rel)
            .collect();
        assert_eq!(
            my,
            paths(&[
                "opt/homebrew/opt/mysql-client@8.0/bin",
                "opt/homebrew/opt/mysql/bin"
            ])
        );
        assert!(expand_pattern(root.path(), "/nope/*/bin").is_empty());
    }

    #[test]
    fn search_dirs_put_path_first_without_duplicates() {
        let root = tempfile::tempdir().unwrap();
        let brew = root.path().join("opt/homebrew/bin");
        std::fs::create_dir_all(&brew).unwrap();
        // An absolute folder that does not exist (Windows ones start with a drive).
        let custom = Path::new(if cfg!(windows) {
            r"C:\custom\bin"
        } else {
            "/custom/bin"
        });
        let path_env =
            std::env::join_paths([custom, brew.as_path(), Path::new("relative/bin")]).unwrap();
        let dirs = search_dirs(root.path(), Some(&path_env));
        assert_eq!(dirs[0], custom);
        assert_eq!(dirs[1], brew);
        assert_eq!(dirs.iter().filter(|d| **d == brew).count(), 1);
        assert!(!dirs.iter().any(|d| d.ends_with("relative/bin")));
    }

    #[cfg(unix)]
    #[test]
    fn finds_each_binary_once() {
        let root = tempfile::tempdir().unwrap();
        let cellar = root.path().join("Cellar/mysql/9.6.0/bin");
        touch_exe(&cellar.join("mysqldump"), "#!/bin/sh\n");
        touch_exe(&cellar.join("mysql"), "#!/bin/sh\n");
        let bin = root.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(cellar.join("mysqldump"), bin.join("mysqldump")).unwrap();
        // Not executable: skipped.
        std::fs::write(bin.join("mysql"), "").unwrap();
        let maria = root.path().join("maria/bin");
        touch_exe(&maria.join("mariadb-dump"), "#!/bin/sh\n");
        let found = find_binaries(&[bin.clone(), cellar.clone(), maria.clone()], Driver::Mysql);
        assert_eq!(
            found,
            vec![
                (DumpTool::Mysqldump, bin.join("mysqldump")),
                (DumpTool::Mysql, cellar.join("mysql")),
                (DumpTool::Mysqldump, maria.join("mariadb-dump")),
            ]
        );
        assert!(find_binaries(&[bin], Driver::Postgres).is_empty());
        assert_eq!(
            tool_for_file(Driver::Mysql, &maria.join("mariadb-dump")),
            Some(DumpTool::Mysqldump)
        );
        assert_eq!(tool_for_file(Driver::Postgres, Path::new("/x/mysql")), None);
    }

    /// Windows installs the tools as `pg_dump.exe`, `mysqldump.exe`…
    #[cfg(windows)]
    #[test]
    fn finds_windows_binaries_by_their_exe_name() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("PostgreSQL/17/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("pg_dump.exe"), "").unwrap();
        // Without the suffix it is not a program Windows runs.
        std::fs::write(bin.join("psql"), "").unwrap();
        assert_eq!(
            find_binaries(std::slice::from_ref(&bin), Driver::Postgres),
            vec![(DumpTool::PgDump, bin.join("pg_dump.exe"))]
        );
        assert_eq!(
            tool_for_file(Driver::Mysql, Path::new(r"C:\MySQL\bin\mysqldump.exe")),
            Some(DumpTool::Mysqldump)
        );
        assert_eq!(
            tool_for_file(Driver::Postgres, Path::new(r"C:\pg\bin\pg_restore.EXE")),
            Some(DumpTool::PgRestore)
        );
        assert_eq!(
            tool_for_file(Driver::Postgres, Path::new(r"C:\x\.exe")),
            None
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn discovers_and_reads_versions() {
        let root = tempfile::tempdir().unwrap();
        let v16 = root.path().join("opt/homebrew/opt/postgresql@16/bin");
        let v17 = root.path().join("opt/homebrew/opt/postgresql@17/bin");
        touch_exe(
            &v16.join("pg_dump"),
            "#!/bin/sh\necho 'pg_dump (PostgreSQL) 16.4'\n",
        );
        touch_exe(
            &v17.join("pg_dump"),
            "#!/bin/sh\necho 'pg_dump (PostgreSQL) 17.6 (Homebrew)'\n",
        );
        touch_exe(
            &v17.join("psql"),
            "#!/bin/sh\necho 'psql (PostgreSQL) 17.6'\n",
        );
        touch_exe(&v17.join("pg_restore"), "#!/bin/sh\necho 'garbage'\n");
        let list = discover(root.path(), None, Driver::Postgres).await;
        let summary: Vec<(DumpTool, Option<String>)> = list
            .iter()
            .map(|b| (b.tool, b.version.as_ref().map(|v| v.text.clone())))
            .collect();
        assert_eq!(
            summary,
            vec![
                (DumpTool::PgDump, Some("17.6".into())),
                (DumpTool::PgDump, Some("16.4".into())),
                (DumpTool::PgRestore, None),
                (DumpTool::Psql, Some("17.6".into())),
            ]
        );
        assert_eq!(list[2].version_output, "garbage");
        assert_eq!(
            list[0].version,
            Some(ParsedVersion {
                flavor: VersionFlavor::Postgres,
                text: "17.6".into(),
                major: 17,
                minor: 6
            })
        );
    }
}
