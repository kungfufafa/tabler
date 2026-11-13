import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { SettingRow } from "@/features/settings/controls";
import { buildLabel, useAppInfo } from "@/stores/appInfo";
import { setPref, usePrefs } from "@/stores/prefs";
import { checkForUpdates } from "./store";

/**
 * General ▸ Application: "Automatically check for updates" (the quiet check at launch), the
 * running build (version, commit and channel) and Check Now, which is the menu's Check for
 * Updates….
 */
export function UpdateSettingRow() {
  const auto = usePrefs((p) => p.autoCheckUpdates);
  const info = useAppInfo();
  const build = buildLabel(info);
  const channel = info?.channel === "nightly" ? "nightly" : "stable";
  return (
    <SettingRow
      id="pref-auto-check-updates"
      label="Automatically check for updates"
      description={
        <>
          {auto
            ? "Looks for a newer build when Tabler starts and says so when there is one."
            : "Only Check for Updates… looks for a newer build."}
          {build && (
            <>
              {" "}
              This is Tabler <span className="selectable font-mono">{build}</span>, on the {channel}{" "}
              channel.
            </>
          )}
        </>
      }
    >
      <Button variant="outline" size="xs" onClick={() => void checkForUpdates()}>
        Check Now
      </Button>
      <Switch
        id="pref-auto-check-updates"
        checked={auto}
        onCheckedChange={(v) => setPref("autoCheckUpdates", v)}
        aria-describedby="pref-auto-check-updates-description"
      />
    </SettingRow>
  );
}
