// Mock handlers for Plugins ▸ Manage Plugins…: plugins_list, plugins_install, plugins_remove,
// plugins_read_script, plugins_reveal_folder, plugins_set_menu. Mirrors `src-tauri/src/plugins.rs`
// with an in-memory Plugins folder holding two sample plugins and one broken folder.
import type { InstalledPlugin } from "../bindings/InstalledPlugin";
import type { PluginList } from "../bindings/PluginList";
import type { PluginManifest } from "../bindings/PluginManifest";
import type { PluginMenuEntry } from "../bindings/PluginMenuEntry";
import { mockError, registerMock } from "./transport";

const DIR = "/Users/me/Library/Application Support/com.mekayastudio.tabler/Plugins";

interface MockPlugin {
  manifest: PluginManifest;
  scripts: Record<string, string>;
}

const ROW_COUNTER: MockPlugin = {
  manifest: {
    name: "Row Counter",
    identifier: "com.example.row-counter",
    version: "1.2",
    author: "Example Co.",
    description: "Counts a table's rows and copies rows as JSON.",
    commands: [
      {
        name: "Count Rows",
        script: "count.js",
        key: "Mod+Shift+U",
        contexts: ["sidebar", "global"],
      },
      { name: "Copy Row as JSON", script: "json.js", contexts: ["grid"], handler: "copyJson" },
    ],
  },
  scripts: {
    "count.js": `const item = context.currentItem();
if (!item) {
  await context.alert("Row Counter", "Select or open a table first.");
  return;
}
const q = context.driver() === "MySQL" ? "\`" : '"';
const name = (item.schema ? q + item.schema + q + "." : "") + q + item.name + q;
const res = await context.execute("SELECT COUNT(*) AS n FROM " + name);
await context.alert(item.name, res.rows[0].raw("n") + " rows");`,
    "json.js": `function copyJson(context) {
  const rows = context.selectedRows();
  const data = rows.map((r) => r.values);
  SystemService.insertToClipboard(JSON.stringify(rows.length === 1 ? data[0] : data, null, 2));
  SystemService.notify("Copied as JSON", rows.length + " row(s)");
}`,
  },
};

const SANDBOX_DEMO: MockPlugin = {
  manifest: {
    name: "Sandbox Demo",
    identifier: "com.example.sandbox-demo",
    version: "0.1",
    author: "Example Co.",
    description: "Shows what the sandbox refuses (network access) and opens the Tabler docs.",
    commands: [
      { name: "Try the Network", script: "net.js", contexts: ["global"] },
      { name: "Open Docs", script: "docs.js", contexts: ["global", "editor"] },
    ],
  },
  scripts: {
    "net.js": `await fetch("https://example.com");`,
    "docs.js": `await SystemService.openURL("https://docs.tableplus.com/utilities/plugin");`,
  },
};

let plugins = new Map<string, MockPlugin>();

function initial(): Map<string, MockPlugin> {
  return new Map([
    [ROW_COUNTER.manifest.identifier, ROW_COUNTER],
    [SANDBOX_DEMO.manifest.identifier, SANDBOX_DEMO],
  ]);
}
plugins = initial();

/** The native Plugins menu as the renderer last sent it (the browser has no menu bar). */
export let mockPluginMenu: PluginMenuEntry[] = [];

/** Test helper: back to the sample plugins. */
export function resetMockPlugins(): void {
  plugins = initial();
  mockPluginMenu = [];
}

const installed = (p: MockPlugin): InstalledPlugin => ({
  manifest: p.manifest,
  folder: `${DIR}/${p.manifest.identifier}`,
});

registerMock("plugins_list", (): PluginList => ({
  dir: DIR,
  plugins: [...plugins.values()]
    .map(installed)
    .sort((a, b) => a.manifest.name.localeCompare(b.manifest.name)),
  invalid: [
    {
      folder: `${DIR}/broken-plugin`,
      error: '"commands" is empty: a plugin needs at least one command',
    },
  ],
}));

registerMock("plugins_install", ({ path }) => {
  const folder = String(path ?? "").replace(/\/+$/, "");
  const base = folder.split("/").pop() ?? "";
  if (!/^[A-Za-z0-9][\w.-]*$/.test(base))
    mockError("INVALID_INPUT", "no manifest.json in this folder");
  const name = base.replace(/[-_.]+/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
  const plugin: MockPlugin = {
    manifest: {
      name,
      identifier: `com.example.${base.toLowerCase()}`,
      version: "1",
      author: "",
      description: `Installed from ${folder}.`,
      commands: [{ name: "Say Hello", script: "main.js", contexts: ["global", "grid", "sidebar"] }],
    },
    scripts: {
      "main.js": `const item = context.currentItem();
await context.alert("Hello", "Hello from ${name}" + (item ? " on " + item.name : "") + ".");`,
    },
  };
  plugins.set(plugin.manifest.identifier, plugin);
  return installed(plugin);
});

registerMock("plugins_remove", ({ identifier }) => {
  if (!plugins.delete(String(identifier)))
    mockError("NOT_FOUND", `not found: plugin ${String(identifier)}`);
  return null;
});

registerMock("plugins_read_script", ({ identifier, script }) => {
  const plugin = plugins.get(String(identifier));
  if (!plugin) mockError("NOT_FOUND", `not found: plugin ${String(identifier)}`);
  const code = plugin.scripts[String(script)];
  if (code === undefined)
    mockError("INVALID_INPUT", `${String(script)} is not a script of ${String(identifier)}`);
  return code;
});

registerMock("plugins_reveal_folder", () => null);

registerMock("plugins_set_menu", ({ entries }) => {
  mockPluginMenu = (entries ?? []) as PluginMenuEntry[];
  return null;
});
