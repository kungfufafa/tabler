//! Version numbers of the dump binaries (`--version` output) and of the servers.

use std::sync::LazyLock;

use regex::Regex;

use crate::types::backup::{ParsedVersion, VersionFlavor};

static PG_TOOL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\(PostgreSQL\)\s+((\d+)(?:\.(\d+))?(?:\.\d+)*(?:(?:alpha|beta|rc|devel)\d*)?)")
        .expect("valid regex")
});
static PG_SERVER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"PostgreSQL\s+((\d+)(?:\.(\d+))?(?:\.\d+)*(?:(?:alpha|beta|rc|devel)\d*)?)")
        .expect("valid regex")
});
/// `from 11.8.9-MariaDB`, `Distrib 10.6.12-MariaDB`, `10.11.6-MariaDB-0+deb12u1`.
static MARIADB: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"((\d+)\.(\d+)\.\d+)-MariaDB").expect("valid regex"));
/// MySQL 5.x clients: `mysqldump  Ver 10.13 Distrib 5.7.44, for osx10.19`.
static MYSQL_DISTRIB: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Distrib\s+((\d+)\.(\d+)\.\d+)").expect("valid regex"));
/// MySQL 8+ clients: `mysqldump  Ver 8.0.36 for macos14.2 on arm64`.
static MYSQL_VER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bVer\s+((\d+)\.(\d+)\.\d+)").expect("valid regex"));
/// Any `major.minor[.patch]` (server version strings such as `MySQL 9.6.0` or `8.0.36-log`).
static NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"((\d+)\.(\d+)(?:\.\d+)?)").expect("valid regex"));

fn from_caps(caps: &regex::Captures<'_>, flavor: VersionFlavor) -> Option<ParsedVersion> {
    let text = caps.get(1)?.as_str().to_string();
    let major = caps.get(2)?.as_str().parse().ok()?;
    let minor = caps
        .get(3)
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0);
    Some(ParsedVersion {
        flavor,
        text,
        major,
        minor,
    })
}

/// Parses the output of `pg_dump --version`, `mysqldump --version`, `mariadb --version`…
pub fn parse_tool_version(output: &str) -> Option<ParsedVersion> {
    if let Some(c) = PG_TOOL.captures(output) {
        return from_caps(&c, VersionFlavor::Postgres);
    }
    if let Some(c) = MARIADB.captures(output) {
        return from_caps(&c, VersionFlavor::Mariadb);
    }
    if let Some(c) = MYSQL_DISTRIB.captures(output) {
        return from_caps(&c, VersionFlavor::Mysql);
    }
    MYSQL_VER
        .captures(output)
        .and_then(|c| from_caps(&c, VersionFlavor::Mysql))
}

/// Parses the server's version string as the drivers report it: `PostgreSQL 17.6 (Homebrew) on
/// aarch64-apple-darwin…`, `MySQL 9.6.0`, `MariaDB 11.8.2`, or a raw `VERSION()` value.
pub fn parse_server_version(version: &str) -> Option<ParsedVersion> {
    if let Some(c) = PG_SERVER.captures(version) {
        return from_caps(&c, VersionFlavor::Postgres);
    }
    if let Some(c) = MARIADB.captures(version) {
        return from_caps(&c, VersionFlavor::Mariadb);
    }
    let flavor = if version.to_ascii_lowercase().contains("mariadb") {
        VersionFlavor::Mariadb
    } else {
        VersionFlavor::Mysql
    };
    NUMBER.captures(version).and_then(|c| from_caps(&c, flavor))
}

/// The first non-empty line of `--version` output that names a version (MariaDB's deprecated
/// `mysqldump` name prints a warning line first).
pub fn version_line(output: &str) -> String {
    let lines = || output.lines().map(str::trim).filter(|l| !l.is_empty());
    lines()
        .find(|l| parse_tool_version(l).is_some())
        .or_else(|| lines().next())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(flavor: VersionFlavor, text: &str, major: u32, minor: u32) -> Option<ParsedVersion> {
        Some(ParsedVersion {
            flavor,
            text: text.into(),
            major,
            minor,
        })
    }

    #[test]
    fn parses_postgres_tools() {
        use VersionFlavor::Postgres;
        assert_eq!(
            parse_tool_version("pg_dump (PostgreSQL) 17.6 (Homebrew)\n"),
            v(Postgres, "17.6", 17, 6)
        );
        assert_eq!(
            parse_tool_version("pg_restore (PostgreSQL) 18.2"),
            v(Postgres, "18.2", 18, 2)
        );
        assert_eq!(
            parse_tool_version("psql (PostgreSQL) 9.6.24"),
            v(Postgres, "9.6.24", 9, 6)
        );
        assert_eq!(
            parse_tool_version("pg_dump (PostgreSQL) 18beta1"),
            v(Postgres, "18beta1", 18, 0)
        );
    }

    #[test]
    fn parses_mysql_and_mariadb_tools() {
        use VersionFlavor::{Mariadb, Mysql};
        assert_eq!(
            parse_tool_version("mysqldump  Ver 9.6.0 for macos26.4 on arm64 (Homebrew)"),
            v(Mysql, "9.6.0", 9, 6)
        );
        // `mysql --version` prints argv[0], here a full path.
        assert_eq!(
            parse_tool_version("/opt/homebrew/bin/mysql  Ver 8.0.36 for macos14.2 on arm64"),
            v(Mysql, "8.0.36", 8, 0)
        );
        assert_eq!(
            parse_tool_version("mysqldump  Ver 10.13 Distrib 5.7.44, for osx10.19 (x86_64)"),
            v(Mysql, "5.7.44", 5, 7)
        );
        assert_eq!(
            parse_tool_version(
                "/opt/homebrew/opt/mariadb@11.8/bin/mariadb-dump from 11.8.9-MariaDB, client 10.20 for osx10.21 (arm64)"
            ),
            v(Mariadb, "11.8.9", 11, 8)
        );
        assert_eq!(
            parse_tool_version("mysql  Ver 15.1 Distrib 10.6.12-MariaDB, for osx10.17 (arm64)"),
            v(Mariadb, "10.6.12", 10, 6)
        );
        let deprecated = "mysqldump: Deprecated program name. It will be removed in a future release, use 'mariadb-dump' instead\nmysqldump from 11.8.9-MariaDB, client 10.20 for osx10.21 (arm64)\n";
        assert_eq!(parse_tool_version(deprecated), v(Mariadb, "11.8.9", 11, 8));
        assert_eq!(
            version_line(deprecated),
            "mysqldump from 11.8.9-MariaDB, client 10.20 for osx10.21 (arm64)"
        );
        assert_eq!(parse_tool_version("something else"), None);
        assert_eq!(version_line("\n  odd output \n"), "odd output");
    }

    #[test]
    fn parses_server_versions() {
        use VersionFlavor::{Mariadb, Mysql, Postgres};
        assert_eq!(
            parse_server_version(
                "PostgreSQL 17.6 (Homebrew) on aarch64-apple-darwin24.4.0, compiled by Apple clang"
            ),
            v(Postgres, "17.6", 17, 6)
        );
        assert_eq!(
            parse_server_version("PostgreSQL 9.6.24 on x86_64-pc-linux-gnu"),
            v(Postgres, "9.6.24", 9, 6)
        );
        assert_eq!(parse_server_version("MySQL 9.6.0"), v(Mysql, "9.6.0", 9, 6));
        assert_eq!(
            parse_server_version("MariaDB 11.8.2"),
            v(Mariadb, "11.8.2", 11, 8)
        );
        assert_eq!(
            parse_server_version("10.11.6-MariaDB-0+deb12u1"),
            v(Mariadb, "10.11.6", 10, 11)
        );
        assert_eq!(parse_server_version("8.0.36-log"), v(Mysql, "8.0.36", 8, 0));
        assert_eq!(parse_server_version("unknown"), None);
    }
}
