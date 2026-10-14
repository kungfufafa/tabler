import { ipc, type PluginMenuEntry } from "@/lib/ipc";
import { subscribeKeymap } from "@/lib/keys/keymap";
import { usePrefsStore } from "@/stores/prefs";
import { effectiveKey, enabledCommands } from "./integration";
import { nativeMenuEntries } from "./menuModel";
import { usePluginsStore } from "./store";

/**
 * Keeps the native Plugins menu on the enabled plugins' commands: sent at start, and again when a
 * plugin is installed, removed, enabled or disabled, or a key is remapped. Returns the uninstall
 * function.
 */
export function installPluginMenuSync(
  send: (entries: PluginMenuEntry[]) => Promise<void> = ipc.plugins.setMenu,
): () => void {
  let sent: string | null = null;
  const sync = () => {
    const entries = nativeMenuEntries(enabledCommands(), effectiveKey);
    const json = JSON.stringify(entries);
    if (json === sent) return;
    sent = json;
    send(entries).catch((err: unknown) => {
      if (sent === json) sent = null;
      console.warn("[plugins] could not update the Plugins menu", err);
    });
  };
  const offList = usePluginsStore.subscribe((s, prev) => {
    if (s.list !== prev.list) sync();
  });
  const offPrefs = usePrefsStore.subscribe((s, prev) => {
    if (s.prefs.pluginStates !== prev.prefs.pluginStates) sync();
  });
  const offKeys = subscribeKeymap(sync);
  sync();
  return () => {
    offList();
    offPrefs();
    offKeys();
  };
}
