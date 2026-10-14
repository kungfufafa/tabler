// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `tabler --mcp`: the MCP server over stdio (Settings ▸ Connections ▸ MCP Server), no GUI.
    if tabler_lib::mcp::requested(std::env::args_os()) {
        std::process::exit(tabler_lib::mcp::main());
    }
    tabler_lib::run()
}
