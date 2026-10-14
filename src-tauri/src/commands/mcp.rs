//! Settings ▸ Connections ▸ MCP Server: the command line of the running app's MCP server and the
//! one-click install into Claude Code, Codex, Cursor and VS Code (see `crate::mcp::install`).

use std::path::PathBuf;

use crate::error::AppError;
use crate::mcp::install;
use crate::types::mcp::{McpCliResult, McpClient, McpConfigPreview, McpInfo};

fn executable() -> Result<PathBuf, AppError> {
    let exe = std::env::current_exe().map_err(|e| AppError::Io(format!("the app's path: {e}")))?;
    Ok(install::executable(&exe))
}

fn home() -> Result<PathBuf, AppError> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Io("the home folder is unknown (HOME is not set)".into()))
}

#[tauri::command]
pub async fn mcp_info() -> Result<McpInfo, AppError> {
    Ok(install::info(&executable()?).await)
}

/// Claude Code / Codex: registers Tabler through the client's CLI.
#[tauri::command]
pub async fn mcp_install_cli(client: McpClient) -> Result<McpCliResult, AppError> {
    install::install_cli(client, &executable()?).await
}

/// Cursor / VS Code: the change to their `mcp.json`, for the user to confirm.
#[tauri::command]
pub async fn mcp_preview_config(client: McpClient) -> Result<McpConfigPreview, AppError> {
    let (exe, home) = (executable()?, home()?);
    tauri::async_runtime::spawn_blocking(move || install::preview_config(client, &exe, &home))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
}

/// Writes the confirmed change; `expected_before` is the file as the preview showed it. Returns
/// the file's path.
#[tauri::command]
pub async fn mcp_write_config(
    client: McpClient,
    expected_before: Option<String>,
) -> Result<String, AppError> {
    let (exe, home) = (executable()?, home()?);
    tauri::async_runtime::spawn_blocking(move || {
        install::write_config(client, &exe, &home, expected_before.as_deref())
            .map(|p| p.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
}
