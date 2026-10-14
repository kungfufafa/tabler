import { Switch } from "@/components/ui/switch";
import { SettingRow } from "@/features/settings/controls";
import { setPref, usePrefs } from "@/stores/prefs";
import { AUTO_EXPAND_MAX_W } from "./autoExpand";

/**
 * General ▸ Table Data: TablePlus's two older behaviour switches. They sit with the other Table
 * Data rows (`TableDataSettings`); their logic is here, with the rest of the data table's look.
 */
export function GridBehaviourRows() {
  const noElastic = usePrefs((p) => p.gridNoElastic);
  const autoExpand = usePrefs((p) => p.gridAutoExpandText);
  return (
    <>
      <SettingRow
        id="pref-grid-no-elastic"
        label="Disable scroll elastically"
        description="The table stops at its edges instead of bouncing."
      >
        <Switch
          id="pref-grid-no-elastic"
          checked={noElastic}
          onCheckedChange={(v) => setPref("gridNoElastic", v)}
        />
      </SettingRow>
      <SettingRow
        id="pref-grid-auto-expand"
        label="Auto expand text column"
        description={`Text columns that open narrower than their content widen to fit it, up to ${AUTO_EXPAND_MAX_W} px, when a table first loads.`}
      >
        <Switch
          id="pref-grid-auto-expand"
          checked={autoExpand}
          onCheckedChange={(v) => setPref("gridAutoExpandText", v)}
        />
      </SettingRow>
    </>
  );
}
