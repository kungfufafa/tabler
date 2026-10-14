import { listenEvent, RENDERER_ID } from "@/lib/ipc/events";
import { applyPrefsFromOtherWindow } from "@/stores/prefs";
import { isThemePref, useUiStore, type ThemePref } from "@/stores/ui";

/** The UI store's localStorage key (its `persist` name). */
export const UI_STORAGE_KEY = "tabler.ui";

/** The theme another window wrote to the UI store's localStorage copy, if valid. */
export function themeFromStorage(raw: string | null): ThemePref | null {
  if (!raw) return null;
  try {
    const theme = (JSON.parse(raw) as { state?: { themePref?: unknown } }).state?.themePref;
    return isThemePref(theme) ? theme : null;
  } catch {
    return null;
  }
}

/**
 * Settings changed in one window reach the others right away: the preferences another window
 * saved (`prefs:changed`, keyboard shortcuts included) and the theme (kept in localStorage, whose
 * changes other windows see as `storage` events). Returns the uninstall function.
 */
export function installCrossWindowSync(): () => void {
  let gone = false;
  let unlisten: (() => void) | null = null;
  void listenEvent("prefs:changed", ({ from, changed }) => {
    if (from !== RENDERER_ID) applyPrefsFromOtherWindow(changed);
  }).then((u) => {
    if (gone) u();
    else unlisten = u;
  });
  const onStorage = (e: StorageEvent) => {
    if (e.key !== UI_STORAGE_KEY) return;
    const theme = themeFromStorage(e.newValue);
    if (theme && theme !== useUiStore.getState().themePref)
      useUiStore.getState().setThemePref(theme);
  };
  window.addEventListener("storage", onStorage);
  return () => {
    gone = true;
    unlisten?.();
    window.removeEventListener("storage", onStorage);
  };
}
