import type { SidebarSettingKey, SidebarSettings } from "@/features/objects/menu";
import type { OpenConnection } from "@/stores/connections";
import { setLazyLoad, supportsLazyLoad, useLazyLoad, useProfileLazyOption } from "./lazyLoad";
import { setSidebarOption, useSidebarPrefs } from "./sidebarPrefs";

/**
 * The sidebar's view options (⚙, "+" ▸ Settings and the search menu's Settings ▸) of a connection,
 * and their setter: "Lazy Load Schema Items" belongs to the saved connection (`lazyLoad.ts`), the
 * others to the sidebar.
 */
export function useSidebarSettings(connection: OpenConnection): {
  settings: SidebarSettings;
  set: (key: SidebarSettingKey, value: boolean) => void;
} {
  const options = useSidebarPrefs((s) => s.options);
  const lazy = useLazyLoad(connection);
  const { profileId, driver } = connection.info;
  const profileOption = useProfileLazyOption(profileId);
  return {
    settings: {
      showRecent: options.showRecent,
      showFunctions: options.showFunctions,
      showSequences: options.showSequences,
      showSystemSchemas: options.showSystemSchemas,
      showRowCounts: options.showRowCounts,
      showColumns: options.showColumns,
      lazyLoad: supportsLazyLoad(driver) ? lazy : null,
    },
    set: (key, value) => {
      if (key === "lazyLoad") setLazyLoad(profileId, value, profileOption);
      else setSidebarOption(key, value);
    },
  };
}
