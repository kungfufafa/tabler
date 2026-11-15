import { useEffect } from "react";
import { AppearanceHost } from "@/features/appearance";
import { LockHost } from "@/features/lock/LockHost";
import { actions } from "@/features/palette/actions";
import { UI_STORAGE_KEY } from "@/features/settings/crossWindow";
import { PrefsLoader } from "@/features/settings/SettingsPanel";
import { SettingsWindow } from "@/features/settings/SettingsWindow";
import { checkForUpdates } from "@/features/updater/store";
import { UpdateDialog } from "@/features/updater/UpdateDialog";
import { useKeymapVersion } from "@/lib/keys/keymap";
import { useUiStore } from "@/stores/ui";

/**
 * Settings ▸ General ▸ Check Now and the menu's Check for Updates… chosen here: the update dialog
 * shows in this window. Not the quiet check at launch (the app windows' `UpdaterHost` does it).
 */
function UpdateCheckHost() {
  useEffect(() => actions.register("app.check-updates", checkForUpdates), []);
  return <UpdateDialog />;
}

/**
 * Keeps this window's copy of the UI store (`tabler.ui`: the theme, the panels…) current when an
 * app window writes it, so the theme picked here saves the panels as they are now, not as they
 * were when Settings opened. (`installCrossWindowSync` only follows the theme: an app window keeps
 * its own panels.)
 */
function useUiStoreFollowsOtherWindows(): void {
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === UI_STORAGE_KEY) void useUiStore.persist.rehydrate();
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);
}

/**
 * The shell of a window that is not an app window: the Settings window (TablePlus's own
 * Settings window). Only what Settings needs: the preferences (loaded, saved, and followed across
 * windows), the theme and appearance (with the custom themes), the app lock (every window locks),
 * the update dialog, the toasts (`Providers`). No session, connections, tabs, window shape,
 * updater launch check, deep links or Open Recent: those belong to the app windows.
 */
export function AuxShell() {
  // A remapped shortcut re-renders the labels (Settings ▸ Shortcuts, Security's Lock Tabler).
  useKeymapVersion();
  useUiStoreFollowsOtherWindows();
  return (
    <>
      <PrefsLoader />
      <SettingsWindow />
      {/* Settings ▸ Appearance: theme, fonts and colours, applied to this window too. */}
      <AppearanceHost />
      <UpdateCheckHost />
      {/* The app lock (Settings ▸ Security ▸ Passcode): Lock Tabler locks this window too. */}
      <LockHost />
    </>
  );
}
