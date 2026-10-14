//! Command lines of the dump / restore binaries. Connection settings come from the profile; the
//! password goes in the environment only (`PGPASSWORD` / `MYSQL_PWD`), never in argv, so it never
//! shows in `ps`, the log or the command line the window displays.

use std::path::{Path, PathBuf};

use crate::error::AppError;
use crate::types::backup::{
    DumpTool, ParsedVersion, RestoreFileInfo, RestoreFileKind, VersionFlavor,
};
use crate::types::profile::{Driver, SslMode};

/// Environment variables of the user's shell that would change what the binary connects to or
/// how; removed before ours are set. `PGPASSFILE` / `~/.pgpass` and option files stay usable.
pub const CLEARED_ENV: &[&str] = &[
    "PGPASSWORD",
    "PGHOST",
    "PGHOSTADDR",
    "PGPORT",
    "PGUSER",
    "PGDATABASE",
    "PGSERVICE",
    "PGSERVICEFILE",
    "PGOPTIONS",
    "PGSSLMODE",
    "PGSSLROOTCERT",
    "PGSSLCERT",
    "PGSSLKEY",
    "PGREQUIRESSL",
    "PGCONNECT_TIMEOUT",
    "PGAPPNAME",
    "PGTARGETSESSIONATTRS",
    "MYSQL_PWD",
    "MYSQL_HOST",
    "MYSQL_TCP_PORT",
    "MYSQL_UNIX_PORT",
];

/// Environment variables that hold the password.
pub const PASSWORD_ENV: &[&str] = &["PGPASSWORD", "MYSQL_PWD"];

/// Where the binary connects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    /// The address is the local end of an SSH tunnel (127.0.0.1:<local port>).
    pub via_tunnel: bool,
}

/// What the binary needs to know about the connection.
#[derive(Debug, Clone)]
pub struct ConnParams {
    pub driver: Driver,
    pub endpoint: Endpoint,
    /// The profile's own host: the name a server certificate is checked against.
    pub tls_host: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub database: String,
    pub ssl: SslMode,
    pub ssl_ca: Option<String>,
    pub connect_timeout_secs: u32,
    /// The system CA bundle (`/etc/ssl/cert.pem`), used to verify server certificates when the
    /// profile names no CA file.
    pub system_ca: Option<PathBuf>,
    /// MySQL "Use socket": the socket file to connect through (the endpoint is then unused).
    pub socket: Option<String>,
    /// MySQL: the password may be sent in clear text (an IAM authentication token over TLS or a
    /// socket, see `drivers::mysql::cleartext_password_allowed`).
    pub cleartext_password: bool,
}

/// The binary to run: its path, tool and version (`--version`).
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub path: PathBuf,
    pub tool: DumpTool,
    pub version: Option<ParsedVersion>,
}

impl ToolInfo {
    fn mariadb(&self) -> bool {
        self.version
            .as_ref()
            .is_some_and(|v| v.flavor == VersionFlavor::Mariadb)
    }

    fn label(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.tool.name().to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdinSource {
    Null,
    /// The file is streamed into the binary's stdin (decompressed when `gzip`).
    File {
        path: PathBuf,
        gzip: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdoutSink {
    /// Written to the backup file (gzip-compressed when `gzip`).
    File { gzip: bool },
    /// Shown in the log.
    Log,
}

/// A ready-to-run command.
#[derive(Debug, Clone)]
pub struct Invocation {
    pub program: PathBuf,
    pub tool: DumpTool,
    /// The binary's file name (`pg_dump`, `mariadb-dump`…), for messages.
    pub label: String,
    pub args: Vec<String>,
    /// Set after [`CLEARED_ENV`] is removed; holds the password.
    pub env: Vec<(String, String)>,
    pub stdin: StdinSource,
    pub stdout: StdoutSink,
    /// Notes for the log (gzip skipped, TLS check relaxed through a tunnel…).
    pub notes: Vec<String>,
}

impl Invocation {
    /// The command line as the window shows it (shell-quoted; never holds the password).
    pub fn command_line(&self) -> String {
        std::iter::once(self.program.to_string_lossy().into_owned())
            .chain(self.args.iter().cloned())
            .map(|a| shell_quote(&a))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Quotes a word for display, POSIX shell style.
pub fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_=./,:@%+".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// Splits one option chip into argv words: whitespace separates words; single and double quotes
/// group (a backslash escapes the next character outside single quotes).
pub fn split_flag(chip: &str) -> Result<Vec<String>, AppError> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = chip.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('"'), '\\') | (None, '\\') => {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
                in_word = true;
            }
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err(AppError::InvalidInput(format!(
            "unbalanced quote in the option {chip}"
        )));
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

/// Options Tabler sets itself (connection, password, output): `(long, short)` names.
fn reserved(tool: DumpTool) -> &'static [(&'static str, Option<char>)] {
    match tool {
        DumpTool::PgDump | DumpTool::PgRestore => &[
            ("--host", Some('h')),
            ("--port", Some('p')),
            ("--username", Some('U')),
            ("--dbname", Some('d')),
            ("--password", Some('W')),
            ("--no-password", Some('w')),
            ("--file", Some('f')),
        ],
        DumpTool::Psql => &[
            ("--host", Some('h')),
            ("--port", Some('p')),
            ("--username", Some('U')),
            ("--dbname", Some('d')),
            ("--password", Some('W')),
            ("--no-password", Some('w')),
            ("--file", Some('f')),
            ("--command", Some('c')),
        ],
        DumpTool::Mysqldump => &[
            ("--host", Some('h')),
            ("--port", Some('P')),
            ("--user", Some('u')),
            ("--password", Some('p')),
            ("--result-file", Some('r')),
            ("--protocol", None),
            ("--socket", Some('S')),
            ("--pipe", Some('W')),
        ],
        // The restore is the file on stdin, not a statement.
        DumpTool::Mysql => &[
            ("--host", Some('h')),
            ("--port", Some('P')),
            ("--user", Some('u')),
            ("--password", Some('p')),
            ("--protocol", None),
            ("--socket", Some('S')),
            ("--pipe", Some('W')),
            ("--execute", Some('e')),
        ],
    }
}

/// The reserved option `word` stands for, if any (`--port=1`, `-p5432`, `--host`).
fn reserved_option(tool: DumpTool, word: &str) -> Option<&'static str> {
    reserved(tool).iter().find_map(|(long, short)| {
        let long_hit = word == *long
            || word
                .strip_prefix(long)
                .is_some_and(|rest| rest.starts_with('='));
        let short_hit = short.is_some_and(|s| {
            !word.starts_with("--") && word.strip_prefix('-').is_some_and(|r| r.starts_with(s))
        });
        (long_hit || short_hit).then_some(*long)
    })
}

/// Splits the chips into argv words and refuses the options Tabler sets itself.
pub fn user_args(tool: DumpTool, flags: &[String]) -> Result<Vec<String>, AppError> {
    let mut out = Vec::new();
    for chip in flags {
        let words = split_flag(chip)?;
        if let Some(first) = words.first()
            && !first.starts_with('-')
        {
            return Err(AppError::InvalidInput(format!(
                "\u{201c}{chip}\u{201d} is not an option: options start with - or --"
            )));
        }
        for w in &words {
            if let Some(long) = reserved_option(tool, w) {
                return Err(AppError::InvalidInput(format!(
                    "{long} is set by Tabler from the connection; remove \u{201c}{chip}\u{201d}"
                )));
            }
        }
        out.extend(words);
    }
    Ok(out)
}

/// pg_dump's output format (`--format` / `-F`, the last one wins).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PgFormat {
    Plain,
    Custom,
    Directory,
    Tar,
}

fn pg_format_value(v: &str) -> Option<PgFormat> {
    match v.to_ascii_lowercase().as_str() {
        "p" | "plain" => Some(PgFormat::Plain),
        "c" | "custom" => Some(PgFormat::Custom),
        "d" | "directory" => Some(PgFormat::Directory),
        "t" | "tar" => Some(PgFormat::Tar),
        _ => None,
    }
}

pub fn pg_output_format(args: &[String]) -> PgFormat {
    let mut format = PgFormat::Plain;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let value = if a == "-F" || a == "--format" {
            i += 1;
            args.get(i).map(String::as_str)
        } else {
            a.strip_prefix("--format=").or_else(|| a.strip_prefix("-F"))
        };
        if let Some(f) = value.and_then(pg_format_value) {
            format = f;
        }
        i += 1;
    }
    format
}

fn has_any(args: &[String], names: &[&str]) -> bool {
    args.iter().any(|a| {
        names
            .iter()
            .any(|n| a == n || a.strip_prefix(n).is_some_and(|r| r.starts_with('=')))
    })
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn check_tool(tool: &ToolInfo, expected: &[DumpTool], what: &str) -> Result<(), AppError> {
    if expected.contains(&tool.tool) {
        return Ok(());
    }
    let names: Vec<&str> = expected.iter().map(|t| t.name()).collect();
    Err(AppError::InvalidInput(format!(
        "{what} needs {}, not {}",
        names.join(" or "),
        tool.label()
    )))
}

/// libpq connection arguments and environment.
fn pg_connection(
    tool: &ToolInfo,
    conn: &ConnParams,
    args: &mut Vec<String>,
    env: &mut Vec<(String, String)>,
) {
    // verify-full checks the certificate against `host`; through a tunnel libpq dials
    // `hostaddr` (the tunnel) while still checking the server's own name.
    let tls_name = nonempty(&conn.tls_host)
        .filter(|_| conn.endpoint.via_tunnel && conn.ssl == SslMode::VerifyFull);
    match tls_name {
        Some(name) => {
            args.push(format!("--host={name}"));
            env.push(("PGHOSTADDR".into(), conn.endpoint.host.clone()));
        }
        None => args.push(format!("--host={}", conn.endpoint.host)),
    }
    args.push(format!("--port={}", conn.endpoint.port));
    if let Some(user) = nonempty(&conn.user) {
        args.push(format!("--username={user}"));
    }
    // Never prompt (there is no terminal to answer on); ~/.pgpass still applies.
    args.push("--no-password".into());
    if let Some(pw) = conn.password.as_deref().filter(|p| !p.is_empty()) {
        env.push(("PGPASSWORD".into(), pw.to_string()));
    }
    let mode = match conn.ssl {
        SslMode::Disable => "disable",
        SslMode::Prefer => "prefer",
        SslMode::Require => "require",
        SslMode::VerifyCa => "verify-ca",
        SslMode::VerifyFull => "verify-full",
    };
    env.push(("PGSSLMODE".into(), mode.into()));
    if conn.ssl != SslMode::Disable {
        let verify = matches!(conn.ssl, SslMode::VerifyCa | SslMode::VerifyFull);
        let libpq16 = tool.version.as_ref().is_some_and(|v| v.major >= 16);
        let root = match nonempty(&conn.ssl_ca) {
            Some(ca) => Some(ca.to_string()),
            None if verify => conn
                .system_ca
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .or_else(|| libpq16.then(|| "system".to_string())),
            None => None,
        };
        if let Some(root) = root {
            env.push(("PGSSLROOTCERT".into(), root));
        }
    }
    env.push((
        "PGCONNECT_TIMEOUT".into(),
        conn.connect_timeout_secs.max(2).to_string(),
    ));
    env.push(("PGAPPNAME".into(), "tabler".into()));
}

/// Options that MySQL / MariaDB clients only accept as the very first arguments.
fn is_defaults_option(word: &str) -> bool {
    [
        "--no-defaults",
        "--defaults-file",
        "--defaults-extra-file",
        "--defaults-group-suffix",
        "--login-path",
        "--print-defaults",
    ]
    .iter()
    .any(|n| word == *n || word.strip_prefix(n).is_some_and(|r| r.starts_with('=')))
}

/// MySQL / MariaDB connection arguments and environment; `user` are the user's options (the
/// `--defaults-*` ones are moved first, and an explicit `--ssl…` option replaces Tabler's).
fn mysql_connection(
    tool: &ToolInfo,
    conn: &ConnParams,
    user: Vec<String>,
    notes: &mut Vec<String>,
    env: &mut Vec<(String, String)>,
) -> Vec<String> {
    let (defaults, rest): (Vec<String>, Vec<String>) =
        user.into_iter().partition(|w| is_defaults_option(w));
    let mut args = defaults;
    let socket = nonempty(&conn.socket);
    match socket {
        // "Use socket", as the connection itself does.
        Some(path) => {
            args.push(format!("--socket={path}"));
            args.push("--protocol=SOCKET".into());
        }
        None => {
            args.push(format!("--host={}", conn.endpoint.host));
            args.push(format!("--port={}", conn.endpoint.port));
            // `localhost` would mean the Unix socket; the profile (and a tunnel) is TCP.
            args.push("--protocol=TCP".into());
        }
    }
    if let Some(u) = nonempty(&conn.user) {
        args.push(format!("--user={u}"));
    }
    if let Some(pw) = conn.password.as_deref().filter(|p| !p.is_empty()) {
        env.push(("MYSQL_PWD".into(), pw.to_string()));
    }
    // IAM authentication (`mysql_clear_password`): MySQL's clients refuse to send the token
    // unless asked to; MariaDB's send it on their own (and do not know the option).
    if conn.cleartext_password && !tool.mariadb() {
        args.push("--enable-cleartext-plugin".into());
    }
    let user_ssl = rest
        .iter()
        .any(|w| w.starts_with("--ssl") || w.starts_with("--skip-ssl"));
    // A local socket is not encrypted (as for the connection itself).
    if !user_ssl && socket.is_none() {
        let mut mode = conn.ssl;
        if mode == SslMode::VerifyFull && conn.endpoint.via_tunnel {
            mode = SslMode::VerifyCa;
            notes.push(
                "The server's host name cannot be checked through the SSH tunnel: the certificate is verified against the CA only.".into(),
            );
        }
        let verify = matches!(mode, SslMode::VerifyCa | SslMode::VerifyFull);
        let ca = nonempty(&conn.ssl_ca).map(str::to_string).or_else(|| {
            verify
                .then(|| {
                    conn.system_ca
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned())
                })
                .flatten()
        });
        if tool.mariadb() {
            match mode {
                SslMode::Disable => args.push("--skip-ssl".into()),
                SslMode::Prefer => args.push("--skip-ssl-verify-server-cert".into()),
                SslMode::Require => {
                    args.push("--ssl".into());
                    args.push("--skip-ssl-verify-server-cert".into());
                }
                SslMode::VerifyCa | SslMode::VerifyFull => {
                    args.push("--ssl".into());
                    args.push("--ssl-verify-server-cert".into());
                }
            }
        } else {
            let name = match mode {
                SslMode::Disable => "DISABLED",
                SslMode::Prefer => "PREFERRED",
                SslMode::Require => "REQUIRED",
                SslMode::VerifyCa => "VERIFY_CA",
                SslMode::VerifyFull => "VERIFY_IDENTITY",
            };
            args.push(format!("--ssl-mode={name}"));
        }
        if mode != SslMode::Disable
            && let Some(ca) = ca
        {
            args.push(format!("--ssl-ca={ca}"));
        }
    }
    args.extend(rest);
    args
}

fn invocation(tool: &ToolInfo, args: Vec<String>, env: Vec<(String, String)>) -> Invocation {
    Invocation {
        program: tool.path.clone(),
        tool: tool.tool,
        label: tool.label(),
        args,
        env,
        stdin: StdinSource::Null,
        stdout: StdoutSink::Log,
        notes: Vec::new(),
    }
}

fn require_database(conn: &ConnParams) -> Result<&str, AppError> {
    let db = conn.database.trim();
    if db.is_empty() {
        return Err(AppError::InvalidInput("choose a database".into()));
    }
    Ok(db)
}

/// pg_dump / mysqldump writing the dump of `conn.database` to stdout.
pub fn build_backup(
    tool: &ToolInfo,
    conn: &ConnParams,
    flags: &[String],
    gzip: bool,
) -> Result<Invocation, AppError> {
    let user = user_args(tool.tool, flags)?;
    let mut env = Vec::new();
    let mut notes = Vec::new();
    match conn.driver {
        Driver::Postgres => {
            check_tool(tool, &[DumpTool::PgDump], "A PostgreSQL backup")?;
            let db = require_database(conn)?;
            let format = pg_output_format(&user);
            if format == PgFormat::Directory {
                return Err(AppError::InvalidInput(
                    "the directory format writes a folder, not a file: use --format=custom or --format=plain".into(),
                ));
            }
            let mut args = Vec::new();
            pg_connection(tool, conn, &mut args, &mut env);
            args.extend(user);
            args.push(format!("--dbname={db}"));
            let gzip_output = gzip && format == PgFormat::Plain;
            if gzip && !gzip_output {
                notes.push(match format {
                    PgFormat::Custom => {
                        "Gzip skipped: the custom-format archive is already compressed.".into()
                    }
                    _ => "Gzip skipped: it applies to plain SQL output only.".into(),
                });
            }
            let mut inv = invocation(tool, args, env);
            inv.stdout = StdoutSink::File { gzip: gzip_output };
            inv.notes = notes;
            Ok(inv)
        }
        Driver::Mysql => {
            check_tool(tool, &[DumpTool::Mysqldump], "A MySQL backup")?;
            let all = has_any(&user, &["--all-databases"]) || user.iter().any(|w| w == "-A");
            let db = if all {
                None
            } else {
                Some(require_database(conn)?.to_string())
            };
            let mut args = mysql_connection(tool, conn, user, &mut notes, &mut env);
            args.extend(db);
            let mut inv = invocation(tool, args, env);
            inv.stdout = StdoutSink::File { gzip };
            inv.notes = notes;
            Ok(inv)
        }
        Driver::Sqlite => Err(AppError::Unsupported(
            "SQLite databases are backed up by copying the database file".into(),
        )),
    }
}

/// psql / pg_restore / mysql loading `file` into `conn.database`.
pub fn build_restore(
    tool: &ToolInfo,
    conn: &ConnParams,
    flags: &[String],
    file: &RestoreFileInfo,
) -> Result<Invocation, AppError> {
    let user = user_args(tool.tool, flags)?;
    let db = require_database(conn)?.to_string();
    let path = PathBuf::from(&file.path);
    let mut env = Vec::new();
    let mut notes = Vec::new();
    match conn.driver {
        Driver::Postgres => {
            let archive = file.kind != RestoreFileKind::Sql;
            if archive {
                check_tool(tool, &[DumpTool::PgRestore], "A pg_dump archive")?;
            } else {
                check_tool(tool, &[DumpTool::Psql], "A plain SQL file")?;
            }
            let mut args = Vec::new();
            pg_connection(tool, conn, &mut args, &mut env);
            if tool.tool == DumpTool::Psql {
                // A ~/.psqlrc could turn autocommit off or print timings into the restore.
                args.push("--no-psqlrc".into());
            }
            args.extend(user);
            args.push(format!("--dbname={db}"));
            let mut inv;
            if archive && !file.gzip {
                // pg_restore reads the archive itself (it can seek in a file).
                args.push(path.to_string_lossy().into_owned());
                inv = invocation(tool, args, env);
            } else {
                inv = invocation(tool, args, env);
                inv.stdin = StdinSource::File {
                    path,
                    gzip: file.gzip,
                };
            }
            inv.notes = notes;
            Ok(inv)
        }
        Driver::Mysql => {
            if file.kind != RestoreFileKind::Sql {
                return Err(AppError::InvalidInput(
                    "this file is a PostgreSQL pg_dump archive, not a MySQL dump".into(),
                ));
            }
            check_tool(tool, &[DumpTool::Mysql], "A MySQL restore")?;
            let mut args = mysql_connection(tool, conn, user, &mut notes, &mut env);
            args.push(db);
            let mut inv = invocation(tool, args, env);
            inv.stdin = StdinSource::File {
                path,
                gzip: file.gzip,
            };
            inv.notes = notes;
            Ok(inv)
        }
        Driver::Sqlite => Err(AppError::Unsupported(
            "SQLite databases are restored by replacing the database file".into(),
        )),
    }
}

/// The system CA bundle, when this Mac has one.
pub fn system_ca_bundle() -> Option<PathBuf> {
    let p = Path::new("/etc/ssl/cert.pem");
    p.is_file().then(|| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "S3cr3t pass!'\"";

    fn pg_tool(tool: DumpTool, major: u32) -> ToolInfo {
        ToolInfo {
            path: PathBuf::from(format!(
                "/opt/homebrew/opt/postgresql@{major}/bin/{}",
                tool.name()
            )),
            tool,
            version: Some(ParsedVersion {
                flavor: VersionFlavor::Postgres,
                text: format!("{major}.1"),
                major,
                minor: 1,
            }),
        }
    }

    fn my_tool(tool: DumpTool, flavor: VersionFlavor) -> ToolInfo {
        ToolInfo {
            path: PathBuf::from(format!("/opt/homebrew/bin/{}", tool.name())),
            tool,
            version: Some(ParsedVersion {
                flavor,
                text: "9.6.0".into(),
                major: 9,
                minor: 6,
            }),
        }
    }

    fn conn(driver: Driver) -> ConnParams {
        ConnParams {
            driver,
            endpoint: Endpoint {
                host: "db.example.com".into(),
                port: if driver == Driver::Postgres {
                    5432
                } else {
                    3306
                },
                via_tunnel: false,
            },
            tls_host: Some("db.example.com".into()),
            user: Some("app".into()),
            password: Some(SECRET.into()),
            database: "app_db".into(),
            ssl: SslMode::Prefer,
            ssl_ca: None,
            connect_timeout_secs: 15,
            system_ca: None,
            socket: None,
            cleartext_password: false,
        }
    }

    fn env_of<'a>(inv: &'a Invocation, key: &str) -> Option<&'a str> {
        inv.env
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn assert_no_password(inv: &Invocation) {
        for a in &inv.args {
            assert!(!a.contains("S3cr3t"), "password in argv: {a}");
        }
        assert!(!inv.command_line().contains("S3cr3t"));
        assert!(!inv.program.to_string_lossy().contains("S3cr3t"));
        let in_env: Vec<&str> = inv
            .env
            .iter()
            .filter(|(_, v)| v.contains("S3cr3t"))
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(in_env.len(), 1);
        assert!(PASSWORD_ENV.contains(&in_env[0]));
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn splits_chips_like_shell_words() {
        assert_eq!(split_flag("--no-owner").unwrap(), s(&["--no-owner"]));
        assert_eq!(
            split_flag("-v ON_ERROR_STOP=1").unwrap(),
            s(&["-v", "ON_ERROR_STOP=1"])
        );
        assert_eq!(
            split_flag(r#"--exclude-table="my table" -n 'a b'"#).unwrap(),
            s(&["--exclude-table=my table", "-n", "a b"])
        );
        assert_eq!(split_flag(r"--x=a\ b").unwrap(), s(&["--x=a b"]));
        assert_eq!(split_flag("  ").unwrap(), Vec::<String>::new());
        assert_eq!(split_flag("''").unwrap(), s(&[""]));
        assert!(split_flag("--x='open").is_err());
    }

    #[test]
    fn refuses_options_tabler_sets() {
        for bad in [
            "--host=x",
            "-hx",
            "--port",
            "-p5432",
            "--file=/tmp/x",
            "-W",
            "--dbname=x",
        ] {
            assert!(
                user_args(DumpTool::PgDump, &s(&[bad])).is_err(),
                "{bad} should be refused"
            );
        }
        for bad in [
            "--password=x",
            "-px",
            "-P3306",
            "--user=root",
            "--protocol=SOCKET",
            "-r/tmp/x",
        ] {
            assert!(
                user_args(DumpTool::Mysqldump, &s(&[bad])).is_err(),
                "{bad} should be refused"
            );
        }
        assert!(user_args(DumpTool::Psql, &s(&["-c select 1"])).is_err());
        assert!(user_args(DumpTool::PgDump, &s(&["no-dash"])).is_err());
        // Look-alikes that are fine.
        assert_eq!(
            user_args(
                DumpTool::PgDump,
                &s(&["--no-owner", "--if-exists", "-Fc", "--hostile-table-name"])
            )
            .unwrap()
            .len(),
            4
        );
        assert!(user_args(DumpTool::Mysqldump, &s(&["--port-forwarding"])).is_ok());
        assert!(user_args(DumpTool::Mysqldump, &s(&["--single-transaction", "-P"])).is_err());
    }

    #[test]
    fn reads_the_pg_output_format() {
        assert_eq!(pg_output_format(&[]), PgFormat::Plain);
        assert_eq!(pg_output_format(&s(&["--format=custom"])), PgFormat::Custom);
        assert_eq!(pg_output_format(&s(&["-Fc"])), PgFormat::Custom);
        assert_eq!(pg_output_format(&s(&["-F", "t"])), PgFormat::Tar);
        assert_eq!(
            pg_output_format(&s(&["--format", "d"])),
            PgFormat::Directory
        );
        assert_eq!(
            pg_output_format(&s(&["--format=custom", "--format=plain"])),
            PgFormat::Plain
        );
        assert_eq!(pg_output_format(&s(&["--format=bogus"])), PgFormat::Plain);
    }

    #[test]
    fn pg_dump_command_line() {
        let tool = pg_tool(DumpTool::PgDump, 17);
        let inv = build_backup(
            &tool,
            &conn(Driver::Postgres),
            &s(&["--no-owner", "--format=custom"]),
            true,
        )
        .unwrap();
        assert_eq!(
            inv.args,
            s(&[
                "--host=db.example.com",
                "--port=5432",
                "--username=app",
                "--no-password",
                "--no-owner",
                "--format=custom",
                "--dbname=app_db",
            ])
        );
        assert_eq!(env_of(&inv, "PGPASSWORD"), Some(SECRET));
        assert_eq!(env_of(&inv, "PGSSLMODE"), Some("prefer"));
        assert_eq!(env_of(&inv, "PGAPPNAME"), Some("tabler"));
        // Custom archives are compressed already.
        assert_eq!(inv.stdout, StdoutSink::File { gzip: false });
        assert_eq!(inv.notes.len(), 1);
        assert_no_password(&inv);
        assert_eq!(
            inv.command_line(),
            "/opt/homebrew/opt/postgresql@17/bin/pg_dump --host=db.example.com --port=5432 --username=app --no-password --no-owner --format=custom --dbname=app_db"
        );

        let plain = build_backup(&tool, &conn(Driver::Postgres), &[], true).unwrap();
        assert_eq!(plain.stdout, StdoutSink::File { gzip: true });
        assert!(plain.notes.is_empty());
        assert!(build_backup(&tool, &conn(Driver::Postgres), &s(&["-Fd"]), false).is_err());
        assert!(
            build_backup(
                &pg_tool(DumpTool::Psql, 17),
                &conn(Driver::Postgres),
                &[],
                false
            )
            .is_err()
        );
    }

    #[test]
    fn no_password_means_no_password_variable() {
        let mut c = conn(Driver::Postgres);
        c.password = None;
        c.user = None;
        let inv = build_backup(&pg_tool(DumpTool::PgDump, 17), &c, &[], false).unwrap();
        assert_eq!(env_of(&inv, "PGPASSWORD"), None);
        assert!(!inv.args.iter().any(|a| a.starts_with("--username")));
        c.password = Some(String::new());
        let inv = build_backup(&pg_tool(DumpTool::PgDump, 17), &c, &[], false).unwrap();
        assert_eq!(env_of(&inv, "PGPASSWORD"), None);
    }

    #[test]
    fn pg_tls_and_tunnels() {
        let tool = pg_tool(DumpTool::PgDump, 17);
        let mut c = conn(Driver::Postgres);
        c.endpoint = Endpoint {
            host: "127.0.0.1".into(),
            port: 50123,
            via_tunnel: true,
        };
        let inv = build_backup(&tool, &c, &[], false).unwrap();
        assert_eq!(inv.args[0], "--host=127.0.0.1");
        assert_eq!(inv.args[1], "--port=50123");
        assert_eq!(env_of(&inv, "PGHOSTADDR"), None);

        // verify-full through a tunnel: dial the tunnel, check the server's own name.
        c.ssl = SslMode::VerifyFull;
        let inv = build_backup(&tool, &c, &[], false).unwrap();
        assert_eq!(inv.args[0], "--host=db.example.com");
        assert_eq!(env_of(&inv, "PGHOSTADDR"), Some("127.0.0.1"));
        assert_eq!(env_of(&inv, "PGSSLMODE"), Some("verify-full"));
        // No CA file: libpq 16+ uses the system store.
        assert_eq!(env_of(&inv, "PGSSLROOTCERT"), Some("system"));
        let old = build_backup(&pg_tool(DumpTool::PgDump, 15), &c, &[], false).unwrap();
        assert_eq!(env_of(&old, "PGSSLROOTCERT"), None);
        c.system_ca = Some(PathBuf::from("/etc/ssl/cert.pem"));
        let old = build_backup(&pg_tool(DumpTool::PgDump, 15), &c, &[], false).unwrap();
        assert_eq!(env_of(&old, "PGSSLROOTCERT"), Some("/etc/ssl/cert.pem"));
        c.ssl_ca = Some("/certs/ca.pem".into());
        let inv = build_backup(&tool, &c, &[], false).unwrap();
        assert_eq!(env_of(&inv, "PGSSLROOTCERT"), Some("/certs/ca.pem"));
        c.ssl = SslMode::Disable;
        let inv = build_backup(&tool, &c, &[], false).unwrap();
        assert_eq!(env_of(&inv, "PGSSLMODE"), Some("disable"));
        assert_eq!(env_of(&inv, "PGSSLROOTCERT"), None);
    }

    #[test]
    fn mysqldump_command_line() {
        let tool = my_tool(DumpTool::Mysqldump, VersionFlavor::Mysql);
        let flags = s(&[
            "--single-transaction",
            "--routines",
            "--defaults-extra-file=/x.cnf",
        ]);
        let inv = build_backup(&tool, &conn(Driver::Mysql), &flags, true).unwrap();
        assert_eq!(
            inv.args,
            s(&[
                "--defaults-extra-file=/x.cnf",
                "--host=db.example.com",
                "--port=3306",
                "--protocol=TCP",
                "--user=app",
                "--ssl-mode=PREFERRED",
                "--single-transaction",
                "--routines",
                "app_db",
            ])
        );
        assert_eq!(env_of(&inv, "MYSQL_PWD"), Some(SECRET));
        assert_eq!(inv.stdout, StdoutSink::File { gzip: true });
        assert_no_password(&inv);

        let all =
            build_backup(&tool, &conn(Driver::Mysql), &s(&["--all-databases"]), false).unwrap();
        assert_eq!(all.args.last().map(String::as_str), Some("--all-databases"));
        // An explicit --ssl option replaces Tabler's.
        let own = build_backup(
            &tool,
            &conn(Driver::Mysql),
            &s(&["--ssl-mode=DISABLED"]),
            false,
        )
        .unwrap();
        assert_eq!(
            own.args.iter().filter(|a| a.starts_with("--ssl")).count(),
            1
        );
    }

    #[test]
    fn mysql_tls_modes() {
        let tool = my_tool(DumpTool::Mysqldump, VersionFlavor::Mysql);
        let mut c = conn(Driver::Mysql);
        c.ssl = SslMode::VerifyFull;
        c.system_ca = Some(PathBuf::from("/etc/ssl/cert.pem"));
        let inv = build_backup(&tool, &c, &[], false).unwrap();
        assert!(inv.args.contains(&"--ssl-mode=VERIFY_IDENTITY".to_string()));
        assert!(inv.args.contains(&"--ssl-ca=/etc/ssl/cert.pem".to_string()));
        c.endpoint.via_tunnel = true;
        let inv = build_backup(&tool, &c, &[], false).unwrap();
        assert!(inv.args.contains(&"--ssl-mode=VERIFY_CA".to_string()));
        assert_eq!(inv.notes.len(), 1);

        let maria = my_tool(DumpTool::Mysqldump, VersionFlavor::Mariadb);
        let mut c = conn(Driver::Mysql);
        c.ssl = SslMode::Disable;
        let inv = build_backup(&maria, &c, &[], false).unwrap();
        assert!(inv.args.contains(&"--skip-ssl".to_string()));
        assert!(!inv.args.iter().any(|a| a.starts_with("--ssl-mode")));
        c.ssl = SslMode::Require;
        let inv = build_backup(&maria, &c, &[], false).unwrap();
        assert!(inv.args.contains(&"--ssl".to_string()));
        assert!(
            inv.args
                .contains(&"--skip-ssl-verify-server-cert".to_string())
        );
    }

    /// Regression: "Use socket" profiles were backed up and restored over TCP.
    #[test]
    fn mysql_socket_profiles_use_their_socket() {
        let mut c = conn(Driver::Mysql);
        c.socket = Some("/tmp/mysql.sock".into());
        for inv in [
            build_backup(
                &my_tool(DumpTool::Mysqldump, VersionFlavor::Mysql),
                &c,
                &[],
                false,
            )
            .unwrap(),
            build_restore(
                &my_tool(DumpTool::Mysql, VersionFlavor::Mysql),
                &c,
                &[],
                &file(RestoreFileKind::Sql, false),
            )
            .unwrap(),
        ] {
            assert_eq!(inv.args[0], "--socket=/tmp/mysql.sock");
            assert_eq!(inv.args[1], "--protocol=SOCKET");
            assert!(
                !inv.args
                    .iter()
                    .any(|a| a.starts_with("--host") || a.starts_with("--port")),
                "{:?}",
                inv.args
            );
            assert!(!inv.args.contains(&"--protocol=TCP".to_string()));
            assert!(!inv.args.iter().any(|a| a.starts_with("--ssl")));
            assert_no_password(&inv);
        }
        // The socket stays Tabler's to set.
        assert!(user_args(DumpTool::Mysqldump, &s(&["--socket=/x.sock"])).is_err());
    }

    /// Regression: IAM authentication tokens (`mysql_clear_password`) were refused by MySQL's
    /// clients, so backups of "Command Line (IAM authentication)" profiles could not log in.
    #[test]
    fn iam_tokens_enable_the_cleartext_plugin() {
        let mut c = conn(Driver::Mysql);
        let flag = "--enable-cleartext-plugin".to_string();
        let mysql = my_tool(DumpTool::Mysqldump, VersionFlavor::Mysql);
        assert!(
            !build_backup(&mysql, &c, &[], false)
                .unwrap()
                .args
                .contains(&flag)
        );
        c.cleartext_password = true;
        let inv = build_backup(&mysql, &c, &[], false).unwrap();
        assert!(inv.args.contains(&flag));
        assert_no_password(&inv);
        let restore = build_restore(
            &my_tool(DumpTool::Mysql, VersionFlavor::Mysql),
            &c,
            &[],
            &file(RestoreFileKind::Sql, false),
        )
        .unwrap();
        assert!(restore.args.contains(&flag));
        // MariaDB's clients send it without being asked (and do not know the option).
        let maria = my_tool(DumpTool::Mysqldump, VersionFlavor::Mariadb);
        assert!(
            !build_backup(&maria, &c, &[], false)
                .unwrap()
                .args
                .contains(&flag)
        );
    }

    fn file(kind: RestoreFileKind, gzip: bool) -> RestoreFileInfo {
        RestoreFileInfo {
            path: "/Users/me/app.dump".into(),
            name: "app.dump".into(),
            size: 100,
            kind,
            gzip,
        }
    }

    #[test]
    fn restore_command_lines() {
        let c = conn(Driver::Postgres);
        let restore = pg_tool(DumpTool::PgRestore, 17);
        let inv = build_restore(
            &restore,
            &c,
            &s(&["--clean", "--if-exists"]),
            &file(RestoreFileKind::PgCustom, false),
        )
        .unwrap();
        assert_eq!(
            inv.args.last().map(String::as_str),
            Some("/Users/me/app.dump")
        );
        assert!(inv.args.contains(&"--dbname=app_db".to_string()));
        assert_eq!(inv.stdin, StdinSource::Null);
        assert_no_password(&inv);
        // A gzipped archive is decompressed into stdin.
        let inv = build_restore(&restore, &c, &[], &file(RestoreFileKind::PgCustom, true)).unwrap();
        assert_eq!(
            inv.stdin,
            StdinSource::File {
                path: "/Users/me/app.dump".into(),
                gzip: true
            }
        );
        assert_eq!(inv.args.last().map(String::as_str), Some("--dbname=app_db"));

        let psql = pg_tool(DumpTool::Psql, 17);
        let inv = build_restore(
            &psql,
            &c,
            &s(&["-v ON_ERROR_STOP=1", "--single-transaction"]),
            &file(RestoreFileKind::Sql, false),
        )
        .unwrap();
        assert!(inv.args.contains(&"--no-psqlrc".to_string()));
        assert!(inv.args.windows(2).any(|w| w == ["-v", "ON_ERROR_STOP=1"]));
        assert!(matches!(inv.stdin, StdinSource::File { gzip: false, .. }));
        assert_no_password(&inv);
        // The tool has to match the file.
        assert!(build_restore(&psql, &c, &[], &file(RestoreFileKind::PgCustom, false)).is_err());
        assert!(build_restore(&restore, &c, &[], &file(RestoreFileKind::Sql, false)).is_err());

        let mysql = my_tool(DumpTool::Mysql, VersionFlavor::Mysql);
        let mc = conn(Driver::Mysql);
        let inv = build_restore(
            &mysql,
            &mc,
            &s(&["--force"]),
            &file(RestoreFileKind::Sql, true),
        )
        .unwrap();
        assert_eq!(inv.args.last().map(String::as_str), Some("app_db"));
        assert!(inv.args.contains(&"--force".to_string()));
        assert!(matches!(inv.stdin, StdinSource::File { gzip: true, .. }));
        assert_eq!(env_of(&inv, "MYSQL_PWD"), Some(SECRET));
        assert_no_password(&inv);
        assert!(build_restore(&mysql, &mc, &[], &file(RestoreFileKind::PgCustom, false)).is_err());
        let mut empty = mc.clone();
        empty.database = " ".into();
        assert!(build_restore(&mysql, &empty, &[], &file(RestoreFileKind::Sql, false)).is_err());
    }

    #[test]
    fn quotes_for_display() {
        assert_eq!(shell_quote("--dbname=app"), "--dbname=app");
        assert_eq!(shell_quote("my db"), "'my db'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
    }
}
