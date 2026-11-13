import { useEffect, useRef } from "react";
import { actions } from "@/features/palette/actions";
import { usePrefs, usePrefsStore } from "@/stores/prefs";
import { checkAtLaunch, checkForUpdates } from "./store";
import { UpdateDialog } from "./UpdateDialog";

/** The quiet check waits this long after the window opened, out of the way of the startup. */
export const LAUNCH_CHECK_DELAY_MS = 3000;

/**
 * In-app updates: Check for Updates… (the app menu on macOS, Help elsewhere; Settings ▸ General),
 * the update dialog, and the quiet check at launch behind "Automatically check for updates"
 * (once the preferences are loaded; of several windows only the first one's check is made).
 * Mount once.
 */
export function UpdaterHost() {
  const loaded = usePrefsStore((s) => s.loaded);
  const auto = usePrefs((p) => p.autoCheckUpdates);
  const asked = useRef(false);

  useEffect(() => actions.register("app.check-updates", checkForUpdates), []);

  useEffect(() => {
    if (!loaded || !auto || asked.current) return;
    const timer = setTimeout(() => {
      asked.current = true;
      void checkAtLaunch();
    }, LAUNCH_CHECK_DELAY_MS);
    return () => clearTimeout(timer);
  }, [loaded, auto]);

  return <UpdateDialog />;
}
