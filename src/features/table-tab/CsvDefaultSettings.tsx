import { DecimalChoice } from "@/features/export/DecimalChoice";
import { CSV_DELIMITERS, CSV_LINE_BREAKS, CSV_QUOTES } from "@/features/export/options";
import { Segmented, SettingGroup, SettingRow } from "@/features/settings/controls";
import { setPref, usePrefs } from "@/stores/prefs";

/**
 * Settings ▸ General ▸ CSV default options (TablePlus's Preferences ▸ General ▸ CSV): what Copy
 * Rows As ▸ CSV writes, what the export dialog starts from and what the CSV import falls back to.
 */
export function CsvDefaultSettings() {
  const delimiter = usePrefs((p) => p.csvDelimiter);
  const quote = usePrefs((p) => p.csvQuote);
  const lineBreak = usePrefs((p) => p.csvLineBreak);
  const decimal = usePrefs((p) => p.csvDecimal);
  return (
    <SettingGroup title="CSV default options">
      <SettingRow
        id="pref-csv-delimiter"
        label="Delimiter"
        description="Copy Rows As ▸ CSV, new exports and CSV imports start from these."
      >
        <Segmented
          id="pref-csv-delimiter"
          value={delimiter}
          options={CSV_DELIMITERS}
          onChange={(v) => setPref("csvDelimiter", v)}
        />
      </SettingRow>
      <SettingRow id="pref-csv-quote" label="Quote">
        <Segmented
          id="pref-csv-quote"
          value={quote}
          options={CSV_QUOTES}
          onChange={(v) => setPref("csvQuote", v)}
        />
      </SettingRow>
      <SettingRow id="pref-csv-line-break" label="Line break">
        <Segmented
          id="pref-csv-line-break"
          value={lineBreak}
          options={CSV_LINE_BREAKS}
          onChange={(v) => setPref("csvLineBreak", v)}
        />
      </SettingRow>
      <SettingRow
        id="pref-csv-decimal"
        label="Decimal"
        description="The preset separators, or one character of your own (not a digit, a quote or the delimiter)."
      >
        <DecimalChoice
          id="pref-csv-decimal"
          value={decimal}
          delimiter={delimiter}
          onChange={(v) => setPref("csvDecimal", v)}
        />
      </SettingRow>
    </SettingGroup>
  );
}
