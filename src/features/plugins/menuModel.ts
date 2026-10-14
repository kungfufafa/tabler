import type { PluginMenuEntry } from "@/lib/ipc";
import { comboToAccelerator, isValidCombo } from "@/lib/keys/overrides";
import { groupByPlugin, type CommandRef } from "./registry";

/** A command's shortcut: its keymap action's effective binding, else the manifest's key. */
export type KeyOf = (c: CommandRef) => string | null;

export const manifestKey: KeyOf = (c) => c.key;

/**
 * The native Plugins menu's commands: a plugin with one command shows it directly (Plugins ▸ SQL
 * Format, TablePlus), one with several gets a submenu; the built-in plugins come first.
 */
export function nativeMenuEntries(
  commands: readonly CommandRef[],
  keyOf: KeyOf = manifestKey,
): PluginMenuEntry[] {
  const item = (c: CommandRef, label = c.name): PluginMenuEntry => {
    const key = keyOf(c);
    return {
      kind: "command",
      run: c.runId,
      label,
      accelerator: key && isValidCombo(key) ? comboToAccelerator(key) : null,
    };
  };
  const groups = groupByPlugin(commands);
  const builtin = groups.filter((g) => g[0]!.plugin.builtin);
  const installed = groups.filter((g) => !g[0]!.plugin.builtin);
  const entries = (list: CommandRef[][]): PluginMenuEntry[] =>
    list.map((g) =>
      g.length === 1
        ? item(g[0]!)
        : { kind: "submenu", label: g[0]!.plugin.name, items: g.map((c) => item(c)) },
    );
  const out = entries(builtin);
  if (builtin.length > 0 && installed.length > 0) out.push({ kind: "separator" });
  out.push(...entries(installed));
  return out;
}
