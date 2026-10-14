import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { toast } from "sonner";
import { HISTORY_KEY } from "@/features/editor/historyGroups";
import { openSettings } from "@/features/settings/store";
import { usePrefs, usePrefsStore } from "@/stores/prefs";
import { pruneHistory } from "./historyRetention";
import { loadLocations } from "./store";

/**
 * App-wide: shows the Locations problems found at startup once (a chosen folder missing, so the
 * default one is used; a move that failed), and prunes the query history as "Keep history for"
 * says, at startup (once the preferences are loaded) and when the preference changes.
 */
export function LocationsHost() {
  const qc = useQueryClient();
  const loaded = usePrefsStore((s) => s.loaded);
  const retention = usePrefs((p) => p.historyRetention);

  useEffect(() => {
    let cancelled = false;
    void loadLocations()
      .then((info) => {
        if (cancelled) return;
        for (const w of info.warnings)
          toast.warning(w, {
            id: `locations:${w}`,
            duration: 15_000,
            action: { label: "Locations…", onClick: () => openSettings("locations") },
          });
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!loaded || retention === "forever") return;
    void pruneHistory(retention)
      .then((n) => {
        if (n > 0) void qc.invalidateQueries({ queryKey: HISTORY_KEY });
      })
      .catch((e: unknown) => console.warn("[history] pruning failed", e));
  }, [loaded, retention, qc]);

  return null;
}
