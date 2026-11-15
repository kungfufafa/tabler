import { flushThemes } from "@/features/appearance/themesStore";
import { flushFileAutoSaves } from "@/features/editor/fileAutoSave";
import { refreshSavedQueries } from "@/features/queries/store";
import { listenEvent, RENDERER_ID } from "@/lib/ipc/events";
import { flushPrefs } from "@/stores/prefs";
import { setLocations } from "./store";

/**
 * Follows Settings ▸ Locations used in another window (the Settings window): a moved folder is
 * read again here (and the saved queries with the queries folder), and before a restart this
 * window writes what waits to be saved. Returns the uninstall function.
 */
export function installLocationsSync(): () => void {
  let gone = false;
  const offs: (() => void)[] = [];
  const keep = (u: () => void) => {
    if (gone) u();
    else offs.push(u);
  };
  void listenEvent("locations:changed", ({ from, info, queries }) => {
    if (from === RENDERER_ID) return;
    setLocations(info);
    if (queries) void refreshSavedQueries();
  }).then(keep);
  void listenEvent("app:flush-saves", ({ from }) => {
    if (from === RENDERER_ID) return;
    flushFileAutoSaves();
    void flushPrefs();
    void flushThemes();
  }).then(keep);
  return () => {
    gone = true;
    for (const off of offs.splice(0)) off();
  };
}
