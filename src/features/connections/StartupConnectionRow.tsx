import { Checkbox } from "@/components/ui/checkbox";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useEffect, useState } from "react";
import { SettingRow } from "@/features/settings/controls";
import { ipc, type ConnectionProfile } from "@/lib/ipc";
import { setPref, usePrefs } from "@/stores/prefs";

/** The saved connections, read when the row shows (no query client needed in Settings). */
function useProfileList(): { list: readonly ConnectionProfile[]; loaded: boolean } {
  const [state, setState] = useState<{ list: readonly ConnectionProfile[]; loaded: boolean }>({
    list: [],
    loaded: false,
  });
  useEffect(() => {
    let live = true;
    ipc.profiles
      .list()
      .then((list) => live && setState({ list, loaded: true }))
      .catch(() => live && setState({ list: [], loaded: true }));
    return () => {
      live = false;
    };
  }, []);
  return state;
}

const WELCOME = "welcome";

/**
 * General ▸ Application ▸ "At launch" (TablePlus's "Connection startup"): show the welcome
 * screen, or open a connection, optionally with its form first ("Prompt the input form").
 */
export function StartupConnectionRow() {
  const action = usePrefs((p) => p.startupAction);
  const profileId = usePrefs((p) => p.startupProfileId);
  const promptForm = usePrefs((p) => p.startupPromptForm);
  const { list, loaded } = useProfileList();
  const chosen = action === "connection" ? list.find((p) => p.id === profileId) : undefined;
  const value = action === "connection" && profileId !== "" ? profileId : WELCOME;
  return (
    <SettingRow
      id="pref-startup-connection"
      label="At launch"
      description={
        action === "connection" && profileId !== "" && !chosen && loaded
          ? "The chosen connection was deleted: the welcome screen shows."
          : "Reopened workspaces come first; otherwise this opens."
      }
    >
      <div className="flex flex-col items-end gap-1.5">
        <Select
          value={value}
          onValueChange={(v) => {
            if (v === WELCOME) setPref("startupAction", "welcome");
            else {
              setPref("startupAction", "connection");
              setPref("startupProfileId", v);
            }
          }}
        >
          <SelectTrigger
            id="pref-startup-connection"
            size="sm"
            aria-describedby="pref-startup-connection-description"
            className="h-7 w-56 rounded-md text-xs"
          >
            <SelectValue />
          </SelectTrigger>
          {/* Portaled outside [role=dialog]: keep the listbox in the dialog's keyboard scope. */}
          <SelectContent data-scope="dialog">
            <SelectItem value={WELCOME} className="text-xs">
              Show the welcome screen
            </SelectItem>
            {list.length > 0 && <SelectSeparator />}
            {list.map((p) => (
              <SelectItem key={p.id} value={p.id} className="text-xs">
                Open connection ▸ {p.name}
              </SelectItem>
            ))}
            {value !== WELCOME && !chosen && (
              <SelectItem value={value} className="text-xs" disabled>
                Open connection ▸ (deleted)
              </SelectItem>
            )}
          </SelectContent>
        </Select>
        {value !== WELCOME && (
          <label className="text-muted-foreground flex items-center gap-1.5 text-xs">
            <Checkbox
              checked={promptForm}
              onCheckedChange={(v) => setPref("startupPromptForm", v === true)}
              aria-label="Prompt the input form"
            />
            Prompt the input form
          </label>
        )}
      </div>
    </SettingRow>
  );
}
