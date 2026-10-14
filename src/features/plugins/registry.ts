import type { InstalledPlugin, PluginList } from "@/lib/ipc";
import { BUILTIN_PLUGINS } from "./builtin";
import { isPluginEnabled, type PluginStates } from "./pluginPrefs";
import {
  runIdOf,
  type BuiltinCommand,
  type BuiltinPlugin,
  type PluginContextKind,
  type PluginEntry,
  type RunId,
} from "./types";

/** One command of an enabled (or any) plugin, with what is needed to run it. */
export interface CommandRef {
  runId: RunId;
  plugin: PluginEntry;
  index: number;
  name: string;
  key: string | null;
  contexts: readonly PluginContextKind[];
  builtin?: BuiltinCommand;
  /** Installed plugins: the script and handler of the command. */
  script?: { file: string; handler: string | null };
}

function builtinEntry(p: BuiltinPlugin): PluginEntry {
  return {
    identifier: p.identifier,
    name: p.name,
    version: p.version,
    author: p.author,
    description: p.description,
    builtin: true,
    commands: p.commands.map((c) => ({ name: c.name, key: c.key ?? null, contexts: c.contexts })),
  };
}

function installedEntry(p: InstalledPlugin): PluginEntry {
  const m = p.manifest;
  return {
    identifier: m.identifier,
    name: m.name,
    version: m.version,
    author: m.author,
    description: m.description,
    builtin: false,
    commands: m.commands,
    folder: p.folder,
  };
}

/** Every plugin the manager lists: the built-in ones, then the Plugins folder's. */
export function pluginEntries(list: PluginList | null): PluginEntry[] {
  return [...BUILTIN_PLUGINS.map(builtinEntry), ...(list?.plugins ?? []).map(installedEntry)];
}

/** The commands of every plugin in `list` (built-ins included); `states` keeps the enabled ones. */
export function pluginCommands(list: PluginList | null, states?: PluginStates): CommandRef[] {
  const out: CommandRef[] = [];
  for (const p of BUILTIN_PLUGINS) {
    if (states && !isPluginEnabled(states, p.identifier, true)) continue;
    const plugin = builtinEntry(p);
    p.commands.forEach((c, index) =>
      out.push({
        runId: runIdOf(p.identifier, index),
        plugin,
        index,
        name: c.name,
        key: c.key ?? null,
        contexts: c.contexts,
        builtin: c,
      }),
    );
  }
  for (const p of list?.plugins ?? []) {
    if (states && !isPluginEnabled(states, p.manifest.identifier, false)) continue;
    const plugin = installedEntry(p);
    p.manifest.commands.forEach((c, index) =>
      out.push({
        runId: runIdOf(p.manifest.identifier, index),
        plugin,
        index,
        name: c.name,
        key: c.key ?? null,
        contexts: c.contexts,
        script: { file: c.script, handler: c.handler ?? null },
      }),
    );
  }
  return out;
}

/** The enabled command `runId`, or null. */
export function findCommand(
  runId: RunId,
  list: PluginList | null,
  states: PluginStates,
): CommandRef | null {
  return pluginCommands(list, states).find((c) => c.runId === runId) ?? null;
}

/** Commands offered in a context menu (the grid's cell menu, the sidebar's, the SQL editor's). */
export function commandsFor(
  commands: readonly CommandRef[],
  context: Extract<PluginContextKind, "grid" | "sidebar" | "editor">,
): CommandRef[] {
  return commands.filter((c) => c.contexts.includes(context));
}

/** Commands grouped by plugin, in order (one group per plugin). */
export function groupByPlugin(commands: readonly CommandRef[]): CommandRef[][] {
  const groups = new Map<string, CommandRef[]>();
  for (const c of commands) {
    const g = groups.get(c.plugin.identifier);
    if (g) g.push(c);
    else groups.set(c.plugin.identifier, [c]);
  }
  return [...groups.values()];
}

/** The palette's label of a command (TablePlus lists them as plugin commands). */
export function paletteLabel(c: CommandRef): string {
  return `Plugin: ${c.plugin.name}: ${c.name}`;
}
