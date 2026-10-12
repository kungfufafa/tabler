import { MENU_ACTIONS } from "@/hooks/useNativeMenu";
import { ipc } from "@/lib/ipc";
import { getPrefs } from "@/stores/prefs";
import { DEFAULT_KEYMAP, KEYMAP, subscribeKeymap } from "./keymap";
import { menuAccelerators } from "./overrides";

/** Menu items with a keymap binding: their accelerator follows the user's keymap. */
const SYNCED_IDS: readonly string[] = MENU_ACTIONS.filter((id) =>
  DEFAULT_KEYMAP.some((d) => d.id === id),
);

/**
 * Keeps the native menu's accelerators on the effective keymap (the menu is built with the
 * defaults, so nothing is sent until a binding differs). A remapped item shows its new keys, and
 * its old combo stops reaching the menu (WKWebView offers keys the page leaves alone to the menu).
 * Returns the uninstall function.
 */
export function installMenuAcceleratorSync(
  send: (accelerators: Record<string, string | null>) => Promise<void> = ipc.menu.setAccelerators,
): () => void {
  let sent: string | null = null;
  const sync = () => {
    const accelerators = menuAccelerators(KEYMAP, SYNCED_IDS);
    const json = JSON.stringify(accelerators);
    if (sent === null && Object.keys(getPrefs().keymapOverrides).length === 0) sent = json;
    if (json === sent) return;
    sent = json;
    send(accelerators).catch((err: unknown) => {
      if (sent === json) sent = null;
      console.warn("[keymap] could not update the menu accelerators", err);
    });
  };
  sync();
  return subscribeKeymap(sync);
}
