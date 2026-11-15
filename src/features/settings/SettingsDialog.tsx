import { useRef } from "react";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { useAction } from "@/features/palette/actions";
import { isCapturingShortcut } from "@/lib/keys/useShortcuts";
import { PrefsLoader, SettingsPanel, type SettingsKeys } from "./SettingsPanel";
import { closeSettings, openSettings, useSettingsDialog } from "./store";

const openFromAction = () => openSettings();

/**
 * Settings… (⌘,) in an app window. The app opens the Settings window (`openSettings`, like
 * TablePlus); this dialog shows the same panel where there is no other window to open (the
 * browser mock, the tests), or when the window could not be opened. Always mounted from AppShell,
 * which also gets its preferences loaded through it.
 */
export function SettingsDialog() {
  const open = useSettingsDialog((s) => s.open);
  const setOpen = useSettingsDialog((s) => s.setOpen);
  const query = useSettingsDialog((s) => s.query);
  const setQuery = useSettingsDialog((s) => s.setQuery);
  const searchRef = useRef<HTMLInputElement>(null);
  const keysRef = useRef<SettingsKeys>(null);

  useAction("settings.open", openFromAction);
  // ⌘W closes the settings dialog instead of the tab behind it (macOS convention).
  useAction("tab.close", open ? closeSettings : null);

  return (
    <>
      <PrefsLoader />
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent
          className="flex h-[min(40rem,calc(100dvh-4rem))] gap-0 overflow-hidden p-0 sm:max-w-[min(56rem,calc(100%-2rem))]"
          onKeyDown={(e) => keysRef.current?.(e.nativeEvent)}
          onEscapeKeyDown={(e) => {
            // Esc while a shortcut is being recorded only cancels the recording.
            if (isCapturingShortcut()) {
              e.preventDefault();
              return;
            }
            // Esc in a non-empty search clears it first, like a native search field.
            if (query && document.activeElement === searchRef.current) {
              e.preventDefault();
              setQuery("");
            }
          }}
        >
          <DialogDescription className="sr-only">Tabler preferences</DialogDescription>
          <SettingsPanel
            variant="dialog"
            keysRef={keysRef}
            searchRef={searchRef}
            title={
              <DialogTitle className="px-2 pt-1 pb-1.5 text-sm font-medium">Settings</DialogTitle>
            }
          />
        </DialogContent>
      </Dialog>
    </>
  );
}
