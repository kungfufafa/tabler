import { Switch } from "@/components/ui/switch";
import { SettingGroup, SettingRow } from "@/features/settings/controls";
import { NOTIFICATION_NOUN, REVEAL_LABEL } from "@/lib/platform";
import { setPref, usePrefs } from "@/stores/prefs";

/**
 * Settings ▸ General ▸ Import and export: TablePlus's "Show open containing folder popup after
 * exporting data" (Table Data) and the native notifications of long jobs.
 */
export function NotifySettingRows() {
  const popup = usePrefs((p) => p.exportRevealPopup);
  const notifications = usePrefs((p) => p.showNotifications);
  return (
    <SettingGroup title="Import and export">
      <SettingRow
        id="pref-export-reveal-popup"
        label="Show open containing folder popup after exporting data"
        description={`After an export or a backup, a popup offers ${REVEAL_LABEL}; otherwise a notice says it finished.`}
      >
        <Switch
          id="pref-export-reveal-popup"
          checked={popup}
          onCheckedChange={(v) => setPref("exportRevealPopup", v)}
          aria-describedby="pref-export-reveal-popup-description"
        />
      </SettingRow>
      <SettingRow
        id="pref-show-notifications"
        label="Show notifications"
        description={`A ${NOTIFICATION_NOUN} when an export, import, backup or restore finishes (or fails) while Tabler is in the background.`}
      >
        <Switch
          id="pref-show-notifications"
          checked={notifications}
          onCheckedChange={(v) => setPref("showNotifications", v)}
          aria-describedby="pref-show-notifications-description"
        />
      </SettingRow>
    </SettingGroup>
  );
}
