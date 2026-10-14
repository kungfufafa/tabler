import { Switch } from "@/components/ui/switch";
import { GridBehaviourRows } from "@/features/appearance/GridBehaviourRows";
import { NumberField, SettingGroup, SettingRow } from "@/features/settings/controls";
import { ESTIMATE_COUNT_RANGE, setPref, usePrefs } from "@/stores/prefs";

/** Settings ▸ General ▸ Table Data (TablePlus's Preferences ▸ General ▸ Table Data rows). */
export function TableDataSettings() {
  const alternating = usePrefs((p) => p.gridAlternatingRows);
  const estimate = usePrefs((p) => p.gridEstimateCount);
  const threshold = usePrefs((p) => p.gridEstimateThreshold);
  const autoNull = usePrefs((p) => p.gridAutoNullEmpty);
  const rowNumbers = usePrefs((p) => p.gridRowNumbers);
  const autoHide = usePrefs((p) => p.gridAutoHideScrollers);
  const tooltips = usePrefs((p) => p.gridExpansionTooltips);
  return (
    <SettingGroup title="Table Data">
      <SettingRow
        id="pref-grid-autohide-scrollers"
        label="Auto hide table scrollers"
        description="Thin scroll bars that show only while scrolling or under the pointer."
      >
        <Switch
          id="pref-grid-autohide-scrollers"
          checked={autoHide}
          onCheckedChange={(v) => setPref("gridAutoHideScrollers", v)}
        />
      </SettingRow>
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
      <SettingRow
        id="pref-grid-expansion-tooltips"
        label="Allows expansion Tool Tips"
        description="A cell cut off by its column shows its whole value when the pointer rests on it."
      >
        <Switch
          id="pref-grid-expansion-tooltips"
          checked={tooltips}
          onCheckedChange={(v) => setPref("gridExpansionTooltips", v)}
        />
      </SettingRow>
      <SettingRow id="pref-grid-row-numbers" label="Show row numbers">
        <Switch
          id="pref-grid-row-numbers"
          checked={rowNumbers}
          onCheckedChange={(v) => setPref("gridRowNumbers", v)}
        />
      </SettingRow>
      <GridBehaviourRows />
    </SettingGroup>
  );
}

/** Settings ▸ General ▸ Application ▸ Use loading wallpaper (TablePlus, on by default). */
export function LoadingWallpaperRow() {
  const on = usePrefs((p) => p.gridLoadingWallpaper);
  return (
    <SettingRow
      id="pref-loading-wallpaper"
      label="Use loading wallpaper"
      description="A quiet pattern in the grid while a table's rows load."
    >
      <Switch
        id="pref-loading-wallpaper"
        checked={on}
        onCheckedChange={(v) => setPref("gridLoadingWallpaper", v)}
      />
    </SettingRow>
  );
}

/** Settings ▸ General ▸ Data Decoder (TablePlus's "Automatically convert PostGIS geometry to WKT"). */
export function DataDecoderSettings() {
  const wkt = usePrefs((p) => p.geometryToWkt);
  return (
    <SettingGroup title="Data Decoder">
      <SettingRow
        id="pref-geometry-wkt"
        label="Automatically convert PostGIS geometry to WKT"
        description={
          wkt
            ? "PostGIS geometry / geography and MySQL spatial values show as WKT (SRID=4326;POINT(1 2)). Rows load in that form from the next refresh."
            : "Spatial values show as the hex the database sends."
        }
      >
        <Switch
          id="pref-geometry-wkt"
          checked={wkt}
          onCheckedChange={(v) => setPref("geometryToWkt", v)}
        />
      </SettingRow>
    </SettingGroup>
  );
}
