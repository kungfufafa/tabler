import { toast } from "sonner";
import { writeClipboard } from "@/features/datagrid/clipboard";
import { writeToEditor } from "./editorText";
import { executeSql } from "./execute";
import { loadPluginFile } from "./loadFile";
import { showPluginAlert } from "./store";
import type { HostServices, PluginInvocation, RunHooks } from "./types";
import { openPluginUrl } from "./urls";

/**
 * Where a command runs. The `RunHooks` come from `runInWorker` for sandboxed plugins (its limits:
 * the time-out pauses while a dialog is open, the number of dialogs and the run's dialogs go with
 * it); built-in plugins run without them.
 */
export interface HostEnv extends Partial<RunHooks> {
  plugin: { identifier: string; name: string };
  invocation: PluginInvocation;
}

/** Longest text a plugin may put into the editor, the clipboard or a dialog. */
export const MAX_PLUGIN_TEXT = 5_000_000;

function bounded(text: string): string {
  if (text.length > MAX_PLUGIN_TEXT) throw new Error("The text is too long");
  return text;
}

/** The services a command uses (built-in plugins directly, sandboxed ones through the bridge). */
export function createHostServices(env: HostEnv): HostServices {
  const wait = env.userWait ?? (<T>(p: Promise<T>) => p);
  const dialog: RunHooks["dialog"] = env.dialog ?? ((show) => wait(show()));
  const { invocation: inv } = env;
  let fileRequested = false;
  return {
    execute: (sql) =>
      executeSql(inv.connectionId, bounded(sql), {
        userWait: wait,
        isStopped: env.stopped,
        onStart: (queryId) => inv.connectionId && env.onQuery?.(inv.connectionId, queryId),
      }),
    alert: (title, message) => {
      const t = bounded(title);
      const m = bounded(message);
      return dialog(() => showPluginAlert(env.plugin.name, t, m, env.runId ?? null));
    },
    insertToEditor: async (text) => writeToEditor(inv.editorTabId, bounded(text), "insert"),
    replaceSelection: async (text) => writeToEditor(inv.editorTabId, bounded(text), "replace"),
    insertToClipboard: (text) => writeClipboard(bounded(text)),
    openURL: (url) => openPluginUrl(env.plugin, url, { dialog, runId: env.runId }),
    notify: (title, message) => {
      toast(title || env.plugin.name, { description: message || undefined });
    },
    loadFile: async () => {
      // The user picks the file, once per run: a plugin cannot keep the file dialog coming.
      if (fileRequested) throw new Error("context.loadFile() can be called once per command run");
      fileRequested = true;
      return dialog(() => loadPluginFile());
    },
  };
}
