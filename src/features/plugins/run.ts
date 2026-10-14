import { toast } from "sonner";
import { ipc } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { PluginCancelled } from "./execute";
import { createHostServices } from "./host";
import { findCommand, type CommandRef } from "./registry";
import { runInWorker } from "./runner";
import { errorText } from "./sandbox/runtime";
import { toSnapshot } from "./snapshot";
import { pluginStates, usePluginsStore } from "./store";
import type { PluginInvocation, RunId } from "./types";

/** Runs one command: a built-in one in the app, an installed one in its sandboxed worker. */
export async function runCommandRef(command: CommandRef, inv: PluginInvocation): Promise<void> {
  const plugin = { identifier: command.plugin.identifier, name: command.plugin.name };
  if (command.builtin) {
    await command.builtin.run(inv, createHostServices({ plugin, invocation: inv }));
    return;
  }
  if (!command.script) return;
  const code = await ipc.plugins.readScript(plugin.identifier, command.script.file);
  const conn = inv.connectionId ? useConnectionsStore.getState().byId[inv.connectionId] : undefined;
  await runInWorker({
    plugin,
    code,
    handler: command.script.handler,
    snapshot: toSnapshot(inv, conn ? { name: conn.info.name, readOnly: conn.info.readOnly } : null),
    services: (hooks) => createHostServices({ plugin, invocation: inv, ...hooks }),
  });
}

/**
 * Runs the enabled command `runId` (menus, the palette, keys) and reports a failure as a toast
 * named after the plugin. Declined prompts (Safe Mode, a link) end it quietly.
 */
export async function runPluginCommand(runId: RunId, inv: PluginInvocation): Promise<void> {
  const command = findCommand(runId, usePluginsStore.getState().list, pluginStates());
  if (!command) {
    toast.error("This plugin command is not available", {
      description: "The plugin was disabled or removed (Plugins ▸ Manage Plugins…).",
    });
    return;
  }
  try {
    await runCommandRef(command, inv);
  } catch (e) {
    if (e instanceof PluginCancelled) return;
    toast.error(`${command.plugin.name}: ${command.name} failed`, {
      description: errorText(e).slice(0, 500),
    });
  }
}
