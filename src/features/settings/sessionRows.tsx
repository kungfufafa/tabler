import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { AUTO_SAVE_QUERY_TABS, setPref, usePrefs, type AutoSaveQueryTabs } from "@/stores/prefs";
import { SettingRow } from "./controls";

/** TablePlus's wording (SQL editor ⚙ ▸ Auto Save SQL Query Tab). */
const AUTO_SAVE_LABELS: Record<AutoSaveQueryTabs, string> = {
  none: "Do not save the SQL query",
  first: "Only save the first SQL Query tab",
  all: "Save all SQL Query tabs",
};

/** General ▸ Application: "Reopen closed workspaces at startup". */
export function ReopenWorkspacesRow() {
  const reopen = usePrefs((p) => p.reopenWorkspaces);
  return (
    <SettingRow
      id="pref-reopen-workspaces"
      label="Reopen closed workspaces at startup"
      description="Reconnects the connections that were open, with their database and tabs."
    >
      <Switch
        id="pref-reopen-workspaces"
        checked={reopen}
        onCheckedChange={(v) => setPref("reopenWorkspaces", v)}
        aria-describedby="pref-reopen-workspaces-description"
      />
    </SettingRow>
  );
}

/** Editor ▸ SQL editor: "Auto Save SQL Query Tab". */
export function AutoSaveQueryTabsRow() {
  const mode = usePrefs((p) => p.autoSaveQueryTabs);
  return (
    <SettingRow
      id="pref-auto-save-query-tabs"
      label="Auto Save SQL Query Tab"
      description={
        mode === "none"
          ? "Query tabs open empty after a restart; .sql files reopen from disk."
          : "Query text is kept as you type and comes back after a restart."
      }
    >
      <Select
        value={mode}
        onValueChange={(v) => setPref("autoSaveQueryTabs", v as AutoSaveQueryTabs)}
      >
        <SelectTrigger
          id="pref-auto-save-query-tabs"
          size="sm"
          aria-describedby="pref-auto-save-query-tabs-description"
          className="h-7 w-56 rounded-md text-xs"
        >
          <SelectValue />
        </SelectTrigger>
        {/* Portaled outside [role=dialog]: keep the listbox in the dialog's keyboard scope. */}
        <SelectContent data-scope="dialog">
          {AUTO_SAVE_QUERY_TABS.map((m) => (
            <SelectItem key={m} value={m} className="text-xs">
              {AUTO_SAVE_LABELS[m]}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </SettingRow>
  );
}
