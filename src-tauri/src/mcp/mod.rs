//! Tabler's built-in Model Context Protocol server (TablePlus b718 / b818): `tabler --mcp` (the
//! app's own executable, `/Applications/Tabler.app/Contents/MacOS/tabler --mcp`) serves MCP over
//! stdio, so AI tools such as Claude Code, Codex, Cursor or VS Code can list the saved
//! connections, read schemas and run queries through them.
//!
//! - The process opens the app database (connections, settings, history) and the Keychain like
//!   the app does; it never changes connections or settings, and only adds history rows.
//! - Settings ▸ Connections ▸ MCP Server turns it on (when off, `--mcp` exits with a message)
//!   and holds the safety switches ([`gate`]): read-only by default, production connections
//!   hidden by default.
//! - Database connections are opened on demand and closed after five idle minutes ([`pool`]).

pub mod config;
pub mod gate;
pub mod install;
pub mod output;
pub mod paths;
pub mod pool;
pub mod prefs;
pub mod server;
pub mod tools;

use std::sync::Arc;

use crate::appdb::AppDb;
use crate::secrets::Secrets;
use crate::state::AppState;

/// The command-line flag that runs the MCP server instead of the app.
pub const MCP_FLAG: &str = "--mcp";
/// The name Tabler registers itself under in MCP clients.
pub const SERVER_NAME: &str = "tabler";

/// Whether the process was started as the MCP server (`tabler --mcp`): its first argument.
pub fn requested<A: AsRef<std::ffi::OsStr>>(args: impl IntoIterator<Item = A>) -> bool {
    args.into_iter()
        .nth(1)
        .is_some_and(|a| a.as_ref() == std::ffi::OsStr::new(MCP_FLAG))
}

/// Log records go to stderr: stdout carries the protocol.
struct StderrLog;

impl log::Log for StderrLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("[tabler mcp] {} {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

fn init_log() {
    static LOGGER: StderrLog = StderrLog;
    let level = match std::env::var("TABLER_MCP_LOG").as_deref() {
        Ok("debug") => log::LevelFilter::Debug,
        Ok("info") => log::LevelFilter::Info,
        Ok("off") => log::LevelFilter::Off,
        _ => log::LevelFilter::Warn,
    };
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(level);
    }
}

/// Opens the app's database and secrets as the app does (Settings ▸ Locations followed).
pub fn open_state() -> Result<AppState, String> {
    let default_dir =
        paths::default_app_data_dir().ok_or("cannot find the app data folder (HOME is not set)")?;
    let file = paths::app_db_file(&default_dir);
    if !file.is_file() {
        return Err(format!(
            "Tabler's data was not found at {}: open Tabler once first.",
            file.display()
        ));
    }
    let appdb = AppDb::open(&file).map_err(|e| format!("cannot open {}: {e}", file.display()))?;
    let secrets = Secrets::from_env(appdb.clone());
    Ok(AppState::new(appdb, secrets))
}

/// The process's stdin and stdout as files, for tokio's file I/O (blocking reads run on its
/// blocking threads, as tokio's own stdin does).
fn std_streams() -> std::io::Result<(std::fs::File, std::fs::File)> {
    #[cfg(unix)]
    use std::os::fd::AsFd;
    #[cfg(windows)]
    use std::os::windows::io::AsHandle;
    #[cfg(unix)]
    let (input, output) = (
        std::io::stdin().as_fd().try_clone_to_owned()?,
        std::io::stdout().as_fd().try_clone_to_owned()?,
    );
    #[cfg(windows)]
    let (input, output) = (
        std::io::stdin().as_handle().try_clone_to_owned()?,
        std::io::stdout().as_handle().try_clone_to_owned()?,
    );
    Ok((std::fs::File::from(input), std::fs::File::from(output)))
}

/// `tabler --mcp`: serves MCP on stdin / stdout until the client closes stdin. Returns the
/// process exit code.
pub fn main() -> i32 {
    init_log();
    // Nobody watches what MCP clients run: drivers open their sessions hardened.
    crate::drivers::registry::set_unattended();
    // The same TLS provider as the app (see `run`).
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let state = match open_state() {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("tabler --mcp: {msg}");
            return 1;
        }
    };
    match prefs::McpPrefs::load(&state.appdb) {
        Ok(p) if p.enabled => {}
        Ok(_) => {
            eprintln!("tabler --mcp: {}", tools::DISABLED_MESSAGE);
            return 1;
        }
        Err(e) => {
            eprintln!("tabler --mcp: cannot read Tabler's settings: {e}");
            return 1;
        }
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("tabler --mcp: {e}");
            return 1;
        }
    };
    let (stdin, stdout) = match std_streams() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("tabler --mcp: cannot use stdin / stdout: {e}");
            return 1;
        }
    };
    runtime.block_on(async move {
        let ctx = Arc::new(tools::Ctx::new(Arc::new(state)));
        let server = Arc::new(server::Server::new(ctx));
        let stdin = tokio::io::BufReader::new(tokio::fs::File::from_std(stdin));
        server::serve(server, stdin, tokio::fs::File::from_std(stdout)).await;
    });
    // Blocking reads of stdin (tokio's stdin thread) must not keep the process alive.
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    0
}

#[cfg(test)]
mod tests;
