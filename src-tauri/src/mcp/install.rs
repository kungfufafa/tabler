//! The command MCP clients run, and registering it with Claude Code, Codex, Cursor and VS Code.
//!
//! - Claude Code / Codex: their CLI (`claude mcp add --scope user tabler -- <exe> --mcp`, `codex
//!   mcp add tabler -- <exe> --mcp`), run through the user's login shell so its PATH applies.
//!   Without the CLI the renderer copies the command line instead.
//! - Cursor / VS Code: their `mcp.json` gets Tabler's entry ([`super::config::merge_server`]),
//!   after the user confirmed the change.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value as Json, json};

use crate::error::AppError;
use crate::secrets::login_shell;
use crate::types::mcp::{McpCliResult, McpClient, McpClientStatus, McpConfigPreview, McpInfo};

use super::config::merge_server;
use super::{MCP_FLAG, SERVER_NAME};

/// How long a CLI may take to register Tabler.
const CLI_TIMEOUT: Duration = Duration::from_secs(30);
/// How long looking a CLI up may take.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest CLI output kept for the message.
const MESSAGE_MAX_CHARS: usize = 600;

/// The executable to register: the running one, symlinks resolved.
pub fn executable(current_exe: &Path) -> PathBuf {
    std::fs::canonicalize(current_exe).unwrap_or_else(|_| current_exe.to_path_buf())
}

/// `s` as one POSIX shell word: unchanged when it only holds safe characters, else in single
/// quotes.
pub fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"/._-+:=@%,".contains(&c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// `<exe> --mcp`, quoted for a shell.
pub fn command_line(exe: &Path) -> String {
    format!("{} {MCP_FLAG}", shell_quote(&exe.to_string_lossy()))
}

/// Why a registered path may stop working.
pub fn path_warning(exe: &Path) -> Option<String> {
    let p = exe.to_string_lossy();
    if p.contains("/AppTranslocation/") {
        Some(
            "Tabler runs from a temporary copy macOS made (App Translocation). Move Tabler.app to \
             the Applications folder and open it from there, then install again."
                .into(),
        )
    } else if p.starts_with("/Volumes/") {
        Some(
            "Tabler runs from a disk image or an external volume. Copy it to the Applications \
             folder first, or MCP clients lose it when the volume is ejected."
                .into(),
        )
    } else if p.contains("/target/debug/") || p.contains("/target/release/") {
        Some("This is a development build: the command points at the build folder.".into())
    } else {
        None
    }
}

pub fn label(client: McpClient) -> &'static str {
    match client {
        McpClient::ClaudeCode => "Claude Code",
        McpClient::Codex => "Codex",
        McpClient::Cursor => "Cursor",
        McpClient::VsCode => "VS Code",
        McpClient::VsCodeInsiders => "VS Code Insiders",
    }
}

/// The CLI of a client registered from the command line.
fn cli_name(client: McpClient) -> Option<&'static str> {
    match client {
        McpClient::ClaudeCode => Some("claude"),
        McpClient::Codex => Some("codex"),
        McpClient::Cursor | McpClient::VsCode | McpClient::VsCodeInsiders => None,
    }
}

/// The arguments that register Tabler (after the CLI's name). Claude Code registers it for the
/// user (every project), not just for the folder the command runs in.
pub fn cli_add_args(client: McpClient, exe: &Path) -> Vec<String> {
    let exe = exe.to_string_lossy().into_owned();
    let mut args: Vec<String> = vec!["mcp".into(), "add".into()];
    if client == McpClient::ClaudeCode {
        args.extend(["--scope".into(), "user".into()]);
    }
    args.extend([SERVER_NAME.into(), "--".into(), exe, MCP_FLAG.into()]);
    args
}

fn cli_remove_args(client: McpClient) -> Vec<String> {
    let mut args: Vec<String> = vec!["mcp".into(), "remove".into()];
    if client == McpClient::ClaudeCode {
        args.extend(["--scope".into(), "user".into()]);
    }
    args.push(SERVER_NAME.into());
    args
}

/// The command line a user pastes in a terminal to register Tabler.
pub fn cli_command(client: McpClient, exe: &Path) -> Option<String> {
    let cli = cli_name(client)?;
    let words: Vec<String> = std::iter::once(cli.to_string())
        .chain(cli_add_args(client, exe))
        .map(|w| shell_quote(&w))
        .collect();
    Some(words.join(" "))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// The config file of a client registered through its `mcp.json`, and the section holding the
/// servers.
pub fn config_target(client: McpClient, home: &Path) -> Option<(PathBuf, &'static str)> {
    match client {
        McpClient::Cursor => Some((home.join(".cursor").join("mcp.json"), "mcpServers")),
        McpClient::VsCode | McpClient::VsCodeInsiders => {
            let app = if client == McpClient::VsCodeInsiders {
                "Code - Insiders"
            } else {
                "Code"
            };
            let user = if cfg!(target_os = "macos") {
                home.join("Library/Application Support")
                    .join(app)
                    .join("User")
            } else if cfg!(windows) {
                std::env::var_os("APPDATA")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join("AppData/Roaming"))
                    .join(app)
                    .join("User")
            } else {
                std::env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".config"))
                    .join(app)
                    .join("User")
            };
            Some((user.join("mcp.json"), "servers"))
        }
        McpClient::ClaudeCode | McpClient::Codex => None,
    }
}

/// Tabler's entry in a client's `mcp.json`.
pub fn config_entry(client: McpClient, exe: &Path) -> Vec<(&'static str, Json)> {
    let command = json!(exe.to_string_lossy());
    let args = json!([MCP_FLAG]);
    match client {
        McpClient::VsCode | McpClient::VsCodeInsiders => vec![
            ("type", json!("stdio")),
            ("command", command),
            ("args", args),
        ],
        _ => vec![("command", command), ("args", args)],
    }
}

fn read_config(path: &Path) -> Result<Option<String>, AppError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AppError::Io(format!("{}: {e}", path.display()))),
    }
}

/// What installing into Cursor / VS Code would change.
pub fn preview_config(
    client: McpClient,
    exe: &Path,
    home: &Path,
) -> Result<McpConfigPreview, AppError> {
    let (path, section) = config_target(client, home).ok_or_else(|| {
        AppError::InvalidInput(format!(
            "{} is registered from its command line",
            label(client)
        ))
    })?;
    let before = read_config(&path)?;
    let merged = merge_server(
        before.as_deref().unwrap_or(""),
        section,
        SERVER_NAME,
        &config_entry(client, exe),
    )
    .map_err(|e| AppError::InvalidInput(format!("{} cannot be updated: {e}", path.display())))?;
    Ok(McpConfigPreview {
        client,
        path: path.to_string_lossy().into_owned(),
        before,
        after: merged.text,
        changed: merged.changed,
    })
}

/// Writes the change `preview_config` showed, provided the file still reads `expected_before`
/// (else the user reviews the new state first). A symlinked file is written where it points.
/// Returns the file written.
pub fn write_config(
    client: McpClient,
    exe: &Path,
    home: &Path,
    expected_before: Option<&str>,
) -> Result<PathBuf, AppError> {
    let preview = preview_config(client, exe, home)?;
    if preview.before.as_deref() != expected_before {
        return Err(AppError::InvalidInput(format!(
            "{} changed since it was shown; review the change again",
            preview.path
        )));
    }
    let path = PathBuf::from(&preview.path);
    if !preview.changed {
        return Ok(path);
    }
    let target = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let dir = target
        .parent()
        .ok_or_else(|| AppError::Io(format!("{}: no parent folder", target.display())))?;
    std::fs::create_dir_all(dir).map_err(|e| AppError::Io(format!("{}: {e}", dir.display())))?;
    let io = |e: std::io::Error| AppError::Io(format!("{}: {e}", target.display()));
    // The temp file is private from its first byte: it holds the whole merged text, other
    // servers' API keys included.
    let (tmp, mut file) = create_temp(dir, &target).map_err(io)?;
    let written = (|| {
        use std::io::Write;
        file.write_all(preview.after.as_bytes())?;
        file.flush()?;
        // Keep the file's permissions exactly (a config can be private); the temp file already
        // has no more than them, so this never opens it up before the rename.
        if let Ok(meta) = std::fs::metadata(&target) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        std::fs::rename(&tmp, &target)
    })();
    drop(file);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(io(e));
    }
    Ok(path)
}

/// Creates the temp file `write_config` fills next to `target`, which is renamed over it. It is
/// made with the mode `target` has (else `0600`: a config file may hold secrets, so a new one is
/// private too), so no other user can read it between its creation and the rename.
fn create_temp(dir: &Path, target: &Path) -> std::io::Result<(PathBuf, std::fs::File)> {
    let tmp = dir.join(format!(
        ".mcp.json.tabler-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mode = std::fs::metadata(target)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o600);
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = target;
    let file = options.open(&tmp)?;
    Ok((tmp, file))
}

struct Output {
    success: bool,
    text: String,
}

/// Runs `script` through the login shell (the one password commands use, cmd on Windows unless
/// `$SHELL` names a program), without stdin, bounded by `timeout`.
async fn run_shell(script: &str, timeout: Duration) -> Result<Output, AppError> {
    let shell = login_shell();
    let mut cmd = shell.command(script)?;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| AppError::Timeout)?
        .map_err(|e| AppError::Io(format!("cannot run {}: {e}", shell.program())))?;
    let mut text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !err.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&err);
    }
    Ok(Output {
        success: out.status.success(),
        text,
    })
}

/// Places a CLI is often installed to, for when the login shell's PATH misses it (set up in an
/// interactive-only rc file).
fn usual_locations(cli: &str, home: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![
        home.join(".local/bin"),
        home.join(".claude/local"),
        home.join(".npm-global/bin"),
        home.join(".bun/bin"),
        home.join(".volta/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    dirs.dedup();
    dirs.into_iter().map(|d| d.join(cli)).collect()
}

/// The CLI's path: as the login shell resolves it, else from a usual install location.
async fn find_cli(cli: &str) -> Option<String> {
    if let Ok(out) = run_shell(&format!("command -v {cli}"), LOOKUP_TIMEOUT).await
        && out.success
        && let Some(path) = out
            .text
            .lines()
            .map(str::trim)
            .rfind(|l| l.starts_with('/'))
    {
        return Some(path.to_string());
    }
    let home = home()?;
    usual_locations(cli, &home)
        .into_iter()
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
}

fn shorten(text: &str) -> String {
    let text = text.trim();
    match text.char_indices().nth(MESSAGE_MAX_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

/// Registers Tabler with Claude Code or Codex through its CLI. An existing `tabler` entry is
/// replaced (it may point at an older copy of the app).
pub async fn install_cli(client: McpClient, exe: &Path) -> Result<McpCliResult, AppError> {
    let cli = cli_name(client).ok_or_else(|| {
        AppError::InvalidInput(format!("{} is set up through its mcp.json", label(client)))
    })?;
    let command = cli_command(client, exe).unwrap_or_default();
    let Some(path) = find_cli(cli).await else {
        return Ok(McpCliResult {
            ok: false,
            cli_found: false,
            command,
            message: format!("The {cli} command was not found."),
        });
    };
    let script = |args: Vec<String>| {
        std::iter::once(shell_quote(&path))
            .chain(args.iter().map(|a| shell_quote(a)))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut out = run_shell(&script(cli_add_args(client, exe)), CLI_TIMEOUT).await?;
    if !out.success && out.text.to_lowercase().contains("already exists") {
        let removed = run_shell(&script(cli_remove_args(client)), CLI_TIMEOUT).await?;
        if removed.success {
            out = run_shell(&script(cli_add_args(client, exe)), CLI_TIMEOUT).await?;
        }
    }
    let message = if out.text.is_empty() {
        if out.success {
            format!("Tabler was added to {}.", label(client))
        } else {
            format!("{cli} failed without a message.")
        }
    } else {
        shorten(&out.text)
    };
    Ok(McpCliResult {
        ok: out.success,
        cli_found: true,
        command,
        message,
    })
}

/// The command line and the clients, as Settings shows them.
pub async fn info(exe: &Path) -> McpInfo {
    let home = home();
    let (claude, codex) = tokio::join!(find_cli("claude"), find_cli("codex"));
    let config_status = |client: McpClient| {
        let target = home.as_deref().and_then(|h| config_target(client, h));
        let detected = match (client, &home, &target) {
            (McpClient::Cursor, Some(h), _) => {
                h.join(".cursor").is_dir() || Path::new("/Applications/Cursor.app").exists()
            }
            (_, _, Some((path, _))) => path.parent().is_some_and(Path::is_dir),
            _ => false,
        };
        McpClientStatus {
            client,
            label: label(client).into(),
            detected,
            detail: target.map(|(p, _)| p.to_string_lossy().into_owned()),
        }
    };
    let cli_status = |client: McpClient, found: Option<String>| McpClientStatus {
        client,
        label: label(client).into(),
        detected: found.is_some(),
        detail: found,
    };
    McpInfo {
        executable: exe.to_string_lossy().into_owned(),
        args: vec![MCP_FLAG.into()],
        command_line: command_line(exe),
        warning: path_warning(exe),
        clients: vec![
            cli_status(McpClient::ClaudeCode, claude),
            cli_status(McpClient::Codex, codex),
            config_status(McpClient::Cursor),
            config_status(McpClient::VsCode),
            config_status(McpClient::VsCodeInsiders),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = "/Applications/Tabler.app/Contents/MacOS/tabler";

    #[test]
    fn command_line_of_the_bundled_app() {
        assert_eq!(command_line(Path::new(APP)), format!("{APP} --mcp"));
        assert_eq!(
            command_line(Path::new(
                "/Users/me/My Apps/Tabler.app/Contents/MacOS/tabler"
            )),
            "'/Users/me/My Apps/Tabler.app/Contents/MacOS/tabler' --mcp"
        );
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("--scope"), "--scope");
    }

    #[test]
    fn executable_resolves_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("tabler");
        std::fs::write(&real, b"").unwrap();
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert_eq!(executable(&link), std::fs::canonicalize(&real).unwrap());
        }
        // A path that does not exist (yet) is kept as it is.
        assert_eq!(
            executable(Path::new("/nope/tabler")),
            PathBuf::from("/nope/tabler")
        );
    }

    #[test]
    fn warns_about_paths_that_do_not_last() {
        assert_eq!(path_warning(Path::new(APP)), None);
        assert!(
            path_warning(Path::new(
                "/private/var/folders/x/AppTranslocation/1234/d/Tabler.app/Contents/MacOS/tabler"
            ))
            .unwrap()
            .contains("Applications folder")
        );
        assert!(
            path_warning(Path::new(
                "/Volumes/Tabler/Tabler.app/Contents/MacOS/tabler"
            ))
            .is_some()
        );
        assert!(
            path_warning(Path::new("/Users/me/tabler/src-tauri/target/debug/tabler"))
                .unwrap()
                .contains("development build")
        );
    }

    #[test]
    fn cli_commands() {
        let exe = Path::new(APP);
        assert_eq!(
            cli_command(McpClient::ClaudeCode, exe).unwrap(),
            format!("claude mcp add --scope user tabler -- {APP} --mcp")
        );
        assert_eq!(
            cli_command(McpClient::Codex, exe).unwrap(),
            format!("codex mcp add tabler -- {APP} --mcp")
        );
        assert_eq!(cli_command(McpClient::Cursor, exe), None);
        assert_eq!(
            cli_remove_args(McpClient::ClaudeCode),
            ["mcp", "remove", "--scope", "user", "tabler"]
        );
        let spaced = Path::new("/A B/tabler");
        assert!(
            cli_command(McpClient::Codex, spaced)
                .unwrap()
                .ends_with("-- '/A B/tabler' --mcp")
        );
    }

    #[test]
    fn config_files_and_entries() {
        let home = Path::new("/Users/me");
        let (cursor, section) = config_target(McpClient::Cursor, home).unwrap();
        assert_eq!(cursor, PathBuf::from("/Users/me/.cursor/mcp.json"));
        assert_eq!(section, "mcpServers");
        let (code, section) = config_target(McpClient::VsCode, home).unwrap();
        assert!(code.ends_with("Code/User/mcp.json"));
        #[cfg(target_os = "macos")]
        assert_eq!(
            code,
            PathBuf::from("/Users/me/Library/Application Support/Code/User/mcp.json")
        );
        assert_eq!(section, "servers");
        let (insiders, section) = config_target(McpClient::VsCodeInsiders, home).unwrap();
        assert!(insiders.ends_with("Code - Insiders/User/mcp.json"));
        #[cfg(target_os = "macos")]
        assert_eq!(
            insiders,
            PathBuf::from("/Users/me/Library/Application Support/Code - Insiders/User/mcp.json")
        );
        assert_eq!(section, "servers");
        assert_eq!(
            super::super::config::entry_json(&config_entry(
                McpClient::VsCodeInsiders,
                Path::new(APP)
            )),
            json!({"type": "stdio", "command": APP, "args": ["--mcp"]})
        );
        assert!(config_target(McpClient::ClaudeCode, home).is_none());
        let entry =
            super::super::config::entry_json(&config_entry(McpClient::VsCode, Path::new(APP)));
        assert_eq!(
            entry,
            json!({"type": "stdio", "command": APP, "args": ["--mcp"]})
        );
        let entry =
            super::super::config::entry_json(&config_entry(McpClient::Cursor, Path::new(APP)));
        assert_eq!(entry, json!({"command": APP, "args": ["--mcp"]}));
    }

    #[test]
    fn preview_and_write_a_config_file() {
        let home = tempfile::tempdir().unwrap();
        let exe = Path::new(APP);
        let p = preview_config(McpClient::Cursor, exe, home.path()).unwrap();
        assert!(p.before.is_none() && p.changed);
        // The file must still read as previewed.
        let err = write_config(McpClient::Cursor, exe, home.path(), Some("{}")).unwrap_err();
        assert!(err.to_string().contains("changed since"), "{err}");
        let path = write_config(McpClient::Cursor, exe, home.path(), None).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), p.after);

        // Another server added by hand is kept; writing again changes nothing.
        let mine = "{\n  \"mcpServers\": {\n    \"mine\": {\"command\": \"m\"}\n  }\n}\n";
        std::fs::write(&path, mine).unwrap();
        let p = preview_config(McpClient::Cursor, exe, home.path()).unwrap();
        assert_eq!(p.before.as_deref(), Some(mine));
        write_config(McpClient::Cursor, exe, home.path(), Some(mine)).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("\"mine\": {\"command\": \"m\"}"));
        let p = preview_config(McpClient::Cursor, exe, home.path()).unwrap();
        assert!(!p.changed);
        write_config(McpClient::Cursor, exe, home.path(), Some(&written)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), written);

        // A file Tabler cannot merge is left alone.
        std::fs::write(&path, "not json").unwrap();
        assert!(preview_config(McpClient::Cursor, exe, home.path()).is_err());
        assert!(write_config(McpClient::Cursor, exe, home.path(), Some("not json")).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json");
        assert!(preview_config(McpClient::Codex, exe, home.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn writes_through_a_symlinked_config() {
        let home = tempfile::tempdir().unwrap();
        let dotfiles = tempfile::tempdir().unwrap();
        let real = dotfiles.path().join("cursor-mcp.json");
        std::fs::write(&real, "{}").unwrap();
        std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
        let link = home.path().join(".cursor/mcp.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_config(McpClient::Cursor, Path::new(APP), home.path(), Some("{}")).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            std::fs::read_to_string(&real)
                .unwrap()
                .contains("\"tabler\"")
        );
    }

    /// The permission bits of `path`.
    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn the_temp_file_is_created_with_the_targets_mode() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("mcp.json");
        // A new file: private.
        let (tmp, mut file) = create_temp(dir.path(), &target).unwrap();
        assert_eq!(mode_of(&tmp), 0o600);
        file.write_all(b"secret").unwrap();
        // Whatever the umask: an existing file's mode is the temp file's, from the first byte.
        for mode in [0o600, 0o640, 0o644, 0o400] {
            std::fs::write(&target, "{}").unwrap();
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode)).unwrap();
            let (tmp, _file) = create_temp(dir.path(), &target).unwrap();
            assert_eq!(mode_of(&tmp), mode, "target mode {mode:o}");
            let _ = std::fs::remove_file(&tmp);
            // And never more than the target allows.
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        // Names are random: two temp files never collide.
        let (a, _fa) = create_temp(dir.path(), &target).unwrap();
        let (b, _fb) = create_temp(dir.path(), &target).unwrap();
        assert_ne!(a, b);
    }

    #[cfg(unix)]
    #[test]
    fn a_private_config_stays_private_and_a_new_one_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let exe = Path::new(APP);
        // A new file: 0600 (it may end up next to other servers' keys).
        let path = write_config(McpClient::Cursor, exe, home.path(), None).unwrap();
        assert_eq!(mode_of(&path), 0o600);

        // An existing 0600 file holding another server's key keeps its mode and its content.
        let mine = "{\n  \"mcpServers\": {\n    \"mine\": {\"command\": \"m\", \"env\": {\"KEY\": \"sk-secret\"}}\n  }\n}\n";
        std::fs::write(&path, mine).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_config(McpClient::Cursor, exe, home.path(), Some(mine)).unwrap();
        assert_eq!(mode_of(&path), 0o600);
        let merged = std::fs::read_to_string(&path).unwrap();
        assert!(merged.contains("sk-secret") && merged.contains("\"tabler\""));
        // No temp file is left behind.
        let left: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".mcp.json.tabler-")
            })
            .collect();
        assert!(left.is_empty());

        // 0644 stays 0644.
        std::fs::write(&path, mine).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_config(McpClient::Cursor, exe, home.path(), Some(mine)).unwrap();
        assert_eq!(mode_of(&path), 0o644);
    }

    /// While a config is being written, an observer polling the folder never sees a temp file
    /// that other users can read.
    #[cfg(unix)]
    #[test]
    fn no_temp_file_is_readable_by_others_while_a_private_config_is_written() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let home = tempfile::tempdir().unwrap();
        let exe = Path::new(APP);
        let path = write_config(McpClient::Cursor, exe, home.path(), None).unwrap();
        let dir = path.parent().unwrap().to_path_buf();
        let stop = Arc::new(AtomicBool::new(false));
        let open = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(AtomicUsize::new(0));
        let observer = {
            let (dir, stop, open, seen) = (dir.clone(), stop.clone(), open.clone(), seen.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    for e in std::fs::read_dir(&dir).unwrap().filter_map(Result::ok) {
                        if !e
                            .file_name()
                            .to_string_lossy()
                            .starts_with(".mcp.json.tabler-")
                        {
                            continue;
                        }
                        if let Ok(meta) = e.metadata() {
                            seen.fetch_add(1, Ordering::Relaxed);
                            if meta.permissions().mode() & 0o077 != 0 {
                                open.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
            })
        };
        // A different content each round, so every round really writes.
        for round in 0..300 {
            let marker = format!("{{\"mcpServers\": {{\"k{round}\": {{\"command\": \"x\"}}}}}}");
            std::fs::write(&path, &marker).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            write_config(McpClient::Cursor, exe, home.path(), Some(&marker)).unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        observer.join().unwrap();
        assert_eq!(
            open.load(Ordering::Relaxed),
            0,
            "a temp file readable by others was seen ({} temp files seen)",
            seen.load(Ordering::Relaxed)
        );
    }

    #[tokio::test]
    async fn the_shell_runner_reports_output_and_status() {
        let ok = run_shell("printf 'hi'", LOOKUP_TIMEOUT).await.unwrap();
        assert!(ok.success);
        assert_eq!(ok.text, "hi");
        let bad = run_shell("echo oops >&2; exit 3", LOOKUP_TIMEOUT)
            .await
            .unwrap();
        assert!(!bad.success);
        assert_eq!(bad.text, "oops");
        assert!(find_cli("tabler-no-such-cli-xyz").await.is_none());
    }
}
