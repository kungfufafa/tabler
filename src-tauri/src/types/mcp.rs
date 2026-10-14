//! Settings ▸ MCP Server: the command MCP clients run (`tabler --mcp`) and the
//! one-click install into Claude Code, Codex, Cursor and VS Code.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// An MCP client Tabler can register itself with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum McpClient {
    /// `claude mcp add` (user scope).
    ClaudeCode,
    /// `codex mcp add`.
    Codex,
    /// `~/.cursor/mcp.json`.
    Cursor,
    /// The VS Code user `mcp.json`.
    VsCode,
    /// The VS Code Insiders user `mcp.json`.
    VsCodeInsiders,
}

/// What Settings shows about one client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct McpClientStatus {
    pub client: McpClient,
    /// "Claude Code", "Codex", "Cursor", "VS Code".
    pub label: String,
    /// The CLI was found (Claude Code / Codex), or the app's settings folder exists (Cursor /
    /// VS Code).
    pub detected: bool,
    /// The CLI's path, or the config file Tabler writes.
    pub detail: Option<String>,
}

/// The MCP server command of the running app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct McpInfo {
    /// The app's executable (`/Applications/Tabler.app/Contents/MacOS/tabler`).
    pub executable: String,
    /// Its arguments (`["--mcp"]`).
    pub args: Vec<String>,
    /// Both as one shell command line, quoted where needed.
    pub command_line: String,
    /// Why the path may not last (the app runs from a disk image, a quarantine copy or a build
    /// folder).
    pub warning: Option<String>,
    pub clients: Vec<McpClientStatus>,
}

/// The outcome of a CLI install (Claude Code / Codex).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct McpCliResult {
    /// Registered.
    pub ok: bool,
    /// The CLI was found (when not, the renderer copies `command` instead).
    pub cli_found: bool,
    /// The command line that registers Tabler (to paste in a terminal).
    pub command: String,
    /// What the CLI said, or why it failed.
    pub message: String,
}

/// A config file change, shown for confirmation before it is written (Cursor / VS Code).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct McpConfigPreview {
    pub client: McpClient,
    pub path: String,
    /// The file as it is now; `None` when it does not exist yet.
    pub before: Option<String>,
    pub after: String,
    /// `false`: Tabler is already registered exactly like this.
    pub changed: bool,
}
