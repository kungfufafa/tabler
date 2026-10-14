// Mock handlers for Settings ▸ Connections ▸ MCP Server: mcp_info, mcp_install_cli,
// mcp_preview_config, mcp_write_config. Mirrors `src-tauri/src/mcp/install.rs`: Claude Code is
// "installed" (its CLI is found), Codex is not (the renderer copies the command), Cursor's
// mcp.json already lists another server and VS Code (and VS Code Insiders) has no mcp.json yet.
import type { McpClient } from "../bindings/McpClient";
import type { McpCliResult } from "../bindings/McpCliResult";
import type { McpConfigPreview } from "../bindings/McpConfigPreview";
import type { McpInfo } from "../bindings/McpInfo";
import { mockError, registerMock } from "./transport";

const EXE = "/Applications/Tabler.app/Contents/MacOS/tabler";
type ConfigClient = "cursor" | "vsCode" | "vsCodeInsiders";
const PATHS: Record<ConfigClient, string> = {
  cursor: "/Users/me/.cursor/mcp.json",
  vsCode: "/Users/me/Library/Application Support/Code/User/mcp.json",
  vsCodeInsiders: "/Users/me/Library/Application Support/Code - Insiders/User/mcp.json",
};
const SECTIONS: Record<ConfigClient, string> = {
  cursor: "mcpServers",
  vsCode: "servers",
  vsCodeInsiders: "servers",
};

const INITIAL_FILES: Record<ConfigClient, string | null> = {
  cursor:
    '{\n  "mcpServers": {\n    "github": {\n      "command": "github-mcp-server",\n      "args": ["stdio"]\n    }\n  }\n}\n',
  vsCode: null,
  vsCodeInsiders: null,
};

let files = { ...INITIAL_FILES };

/** Test helper: back to the initial config files. */
export function resetMockMcp(): void {
  files = { ...INITIAL_FILES };
}

function configClient(client: McpClient): ConfigClient {
  if (client !== "cursor" && client !== "vsCode" && client !== "vsCodeInsiders")
    mockError("INVALID_INPUT", "This client is registered from its command line");
  return client;
}

function entry(client: ConfigClient): Record<string, unknown> {
  const base = { command: EXE, args: ["--mcp"] };
  return client === "cursor" ? base : { type: "stdio", ...base };
}

function preview(client: McpClient): McpConfigPreview {
  const c = configClient(client);
  const before = files[c];
  const doc = (before ? JSON.parse(before) : {}) as Record<string, Record<string, unknown>>;
  const section = SECTIONS[c];
  const servers = doc[section] ?? {};
  const wanted = entry(c);
  const changed = JSON.stringify(servers.tabler) !== JSON.stringify(wanted);
  const after = changed
    ? `${JSON.stringify({ ...doc, [section]: { ...servers, tabler: wanted } }, null, 2)}\n`
    : (before ?? "");
  return { client, path: PATHS[c], before, after, changed };
}

registerMock("mcp_info", (): McpInfo => ({
  executable: EXE,
  args: ["--mcp"],
  commandLine: `${EXE} --mcp`,
  warning: null,
  clients: [
    {
      client: "claudeCode",
      label: "Claude Code",
      detected: true,
      detail: "/Users/me/.local/bin/claude",
    },
    { client: "codex", label: "Codex", detected: false, detail: null },
    { client: "cursor", label: "Cursor", detected: true, detail: PATHS.cursor },
    { client: "vsCode", label: "VS Code", detected: true, detail: PATHS.vsCode },
    {
      client: "vsCodeInsiders",
      label: "VS Code Insiders",
      detected: false,
      detail: PATHS.vsCodeInsiders,
    },
  ],
}));

registerMock("mcp_install_cli", (args): McpCliResult => {
  const client = args.client as McpClient;
  if (client === "claudeCode")
    return {
      ok: true,
      cliFound: true,
      command: `claude mcp add --scope user tabler -- ${EXE} --mcp`,
      message: `Added stdio MCP server tabler with command: ${EXE} --mcp to user config`,
    };
  if (client === "codex")
    return {
      ok: false,
      cliFound: false,
      command: `codex mcp add tabler -- ${EXE} --mcp`,
      message: "The codex command was not found.",
    };
  return mockError("INVALID_INPUT", "This client is set up through its mcp.json");
});

registerMock("mcp_preview_config", (args) => preview(args.client as McpClient));

registerMock("mcp_write_config", (args) => {
  const p = preview(args.client as McpClient);
  const expected = (args.expectedBefore as string | null | undefined) ?? null;
  if (p.before !== expected)
    mockError("INVALID_INPUT", `${p.path} changed since it was shown; review the change again`);
  files[configClient(args.client as McpClient)] = p.after;
  return p.path;
});
