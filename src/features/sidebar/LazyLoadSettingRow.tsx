import { Switch } from "@/components/ui/switch";
import { SettingGroup, SettingRow } from "@/features/settings/controls";
import { setPref, usePrefs } from "@/stores/prefs";

/**
 * Settings ▸ General ▸ Sidebar: "Lazy load schema items", the default of every PostgreSQL / MySQL
 * connection whose form leaves it unset (the sidebar's Settings ▸ toggle overrides it per
 * connection).
 */
export function LazyLoadSettingRow() {
  const lazy = usePrefs((p) => p.sidebarLazyLoad);
  return (
    <SettingGroup title="Sidebar">
      <SettingRow
        id="pref-sidebar-lazy-load"
        label="Lazy load schema items"
        description="List the schemas (PostgreSQL) or databases (MySQL) and load one's tables only when it is expanded. Open Anything then searches the loaded ones."
      >
        <Switch
          id="pref-sidebar-lazy-load"
          checked={lazy}
          onCheckedChange={(v) => setPref("sidebarLazyLoad", v)}
          aria-describedby="pref-sidebar-lazy-load-description"
        />
      </SettingRow>
    </SettingGroup>
  );
}
