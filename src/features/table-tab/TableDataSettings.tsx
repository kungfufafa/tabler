import { Switch } from "@/components/ui/switch";
import { NumberField, SettingGroup, SettingRow } from "@/features/settings/controls";
import { ESTIMATE_COUNT_RANGE, setPref, usePrefs } from "@/stores/prefs";

/** Settings ▸ General ▸ Table Data (TablePlus's Preferences ▸ General ▸ Table Data rows). */
export function TableDataSettings() {
  const alternating = usePrefs((p) => p.gridAlternatingRows);
  const estimate = usePrefs((p) => p.gridEstimateCount);
  const threshold = usePrefs((p) => p.gridEstimateThreshold);
  const autoNull = usePrefs((p) => p.gridAutoNullEmpty);
  const rowNumbers = usePrefs((p) => p.gridRowNumbers);
  return (
    <SettingGroup title="Table Data">
      <SettingRow id="pref-grid-alternating" label="Alternating row background colors">
        <Switch
          id="pref-grid-alternating"
          checked={alternating}
          onCheckedChange={(v) => setPref("gridAlternatingRows", v)}
        />
      </SettingRow>
      <SettingRow
        id="pref-grid-estimate-threshold"
        label="Estimate count when table has more than"
        description={
          estimate
            ? `Unfiltered tables above ${threshold.toLocaleString()} rows show the planner's estimate (~); click it for an exact count.`
            : "Rows are always counted exactly."
        }
      >
        <div className="flex items-center gap-2">
          <NumberField
            id="pref-grid-estimate-threshold"
            value={threshold}
            min={ESTIMATE_COUNT_RANGE.min}
            max={ESTIMATE_COUNT_RANGE.max}
            suffix="rows"
            onCommit={(v) => setPref("gridEstimateThreshold", v)}
          />
          <Switch
            id="pref-grid-estimate"
            aria-label="Estimate large row counts"
            checked={estimate}
            onCheckedChange={(v) => setPref("gridEstimateCount", v)}
          />
        </div>
      </SettingRow>
      <SettingRow
        id="pref-grid-auto-null"
        label="Auto convert empty Numeric, UUID, JSON or Array to NULL"
        description='An emptied value in such a nullable column is staged as NULL; the text "NULL" stays text.'
      >
        <Switch
          id="pref-grid-auto-null"
          checked={autoNull}
          onCheckedChange={(v) => setPref("gridAutoNullEmpty", v)}
        />
      </SettingRow>
      <SettingRow id="pref-grid-row-numbers" label="Show row numbers">
        <Switch
          id="pref-grid-row-numbers"
          checked={rowNumbers}
          onCheckedChange={(v) => setPref("gridRowNumbers", v)}
        />
      </SettingRow>
    </SettingGroup>
  );
}
