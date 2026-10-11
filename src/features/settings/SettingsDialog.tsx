import { Keyboard, Plug, SlidersHorizontal, SquareCode, type LucideIcon } from "lucide-react";
import { Tabs as TabsPrimitive } from "radix-ui";
import { useEffect, useLayoutEffect, useRef, type KeyboardEvent } from "react";
import { flushSync } from "react-dom";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { useAction } from "@/features/palette/actions";
import { comboFromEvent, formatCombo, isMac } from "@/lib/keys/combo";
import { flushPrefs, loadPrefs, usePrefs } from "@/stores/prefs";
import { applyUiFontSize } from "./applyPrefs";
import { ConnectionsSection, EditorSection, GeneralSection } from "./sections";
import { ShortcutsSection } from "./ShortcutsSection";
import {
  closeSettings,
  openSettings,
  SETTINGS_SECTIONS,
  useSettingsDialog,
  type SettingsSection,
} from "./store";

const SECTION_META: Record<SettingsSection, { label: string; icon: LucideIcon }> = {
  general: { label: "General", icon: SlidersHorizontal },
  editor: { label: "Editor", icon: SquareCode },
  connections: { label: "Connections", icon: Plug },
  shortcuts: { label: "Shortcuts", icon: Keyboard },
};

/** Loads preferences once per session, applies the interface text size and flushes saves on exit. */
function PrefsLoader() {
  const uiFontSize = usePrefs((p) => p.uiFontSize);
  useLayoutEffect(() => applyUiFontSize(uiFontSize), [uiFontSize]);
  useEffect(() => {
    void loadPrefs();
    const flush = () => void flushPrefs();
    window.addEventListener("pagehide", flush);
    window.addEventListener("beforeunload", flush);
    return () => {
      window.removeEventListener("pagehide", flush);
      window.removeEventListener("beforeunload", flush);
    };
  }, []);
  return null;
}

const openFromAction = () => openSettings();

/** Settings dialog (⌘,): General / Editor / Connections / Shortcuts. Always mounted from AppShell. */
export function SettingsDialog() {
  const open = useSettingsDialog((s) => s.open);
  const section = useSettingsDialog((s) => s.section);
  const setOpen = useSettingsDialog((s) => s.setOpen);
  const setSection = useSettingsDialog((s) => s.setSection);
  const query = useSettingsDialog((s) => s.query);
  const setQuery = useSettingsDialog((s) => s.setQuery);
  const searchRef = useRef<HTMLInputElement>(null);
  const triggerRefs = useRef<Partial<Record<SettingsSection, HTMLButtonElement | null>>>({});

  useAction("settings.open", openFromAction);
  // ⌘W closes the settings window instead of the tab behind it (macOS convention).
  useAction("tab.close", open ? closeSettings : null);

  /**
   * Shows a section and focuses an element in it. The render is flushed first so the new section is
   * in the DOM and whatever the old one owned is gone, e.g. an open Select whose focus trap would
   * otherwise pull focus straight back into its (now unmounted) listbox.
   */
  const goToSection = (next: SettingsSection, target: () => HTMLElement | null | undefined) => {
    flushSync(() => setSection(next));
    target()?.focus();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const combo = comboFromEvent(e.nativeEvent);
    const digit = /^Mod\+([1-9])$/.exec(combo)?.[1];
    if (digit) {
      const next = SETTINGS_SECTIONS[Number(digit) - 1];
      if (!next) return;
      e.preventDefault();
      goToSection(next, () => triggerRefs.current[next]);
      return;
    }
    if (combo === "Mod+F") {
      e.preventDefault();
      goToSection("shortcuts", () => searchRef.current);
    }
  };

  return (
    <>
      <PrefsLoader />
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent
          className="flex h-[520px] max-h-[calc(100vh-4rem)] w-[680px] max-w-[calc(100%-2rem)] gap-0 overflow-hidden p-0 sm:max-w-[680px]"
          onKeyDown={onKeyDown}
          onEscapeKeyDown={(e) => {
            // Esc in a non-empty search clears it first, like a native search field.
            if (query && document.activeElement === searchRef.current) {
              e.preventDefault();
              setQuery("");
            }
          }}
        >
          <DialogDescription className="sr-only">Tabler preferences</DialogDescription>
          <TabsPrimitive.Root
            orientation="vertical"
            value={section}
            onValueChange={(v) => setSection(v as SettingsSection)}
            className="flex min-h-0 min-w-0 flex-1"
          >
            <div className="border-border bg-sidebar flex w-44 shrink-0 flex-col gap-1 border-r p-2">
              <DialogTitle className="px-2 pt-1 pb-1.5 text-[13px] font-semibold">
                Settings
              </DialogTitle>
              <TabsPrimitive.List aria-label="Settings sections" className="flex flex-col gap-px">
                {SETTINGS_SECTIONS.map((id, i) => {
                  const { label, icon: Icon } = SECTION_META[id];
                  const combo = `Mod+${i + 1}`;
                  return (
                    <TabsPrimitive.Trigger
                      key={id}
                      value={id}
                      ref={(el) => {
                        triggerRefs.current[id] = el;
                      }}
                      aria-keyshortcuts={`${isMac ? "Meta" : "Control"}+${i + 1}`}
                      className="group text-foreground/80 hover:bg-accent/70 focus-visible:ring-ring/60 data-[state=active]:bg-brand-soft data-[state=active]:text-foreground flex h-7 items-center gap-2 rounded-md px-2 text-left text-[13px] outline-none focus-visible:ring-1"
                    >
                      <Icon className="text-muted-foreground group-data-[state=active]:text-brand size-3.5 shrink-0" />
                      <span className="flex-1 truncate">{label}</span>
                      <span className="text-muted-foreground/70 hidden text-[10px] tabular-nums group-hover:inline">
                        {formatCombo(combo)}
                      </span>
                    </TabsPrimitive.Trigger>
                  );
                })}
              </TabsPrimitive.List>
            </div>
            {/* Panels get tabIndex -1: each section starts with a focusable control, so Radix's
                default tab stop on the panel itself would be an invisible extra Tab press. */}
            <div className="bg-background flex min-w-0 flex-1 flex-col">
              <TabsPrimitive.Content
                value="general"
                tabIndex={-1}
                className="flex min-h-0 flex-1 flex-col outline-none"
              >
                <GeneralSection />
              </TabsPrimitive.Content>
              <TabsPrimitive.Content
                value="editor"
                tabIndex={-1}
                className="flex min-h-0 flex-1 flex-col outline-none"
              >
                <EditorSection />
              </TabsPrimitive.Content>
              <TabsPrimitive.Content
                value="connections"
                tabIndex={-1}
                className="flex min-h-0 flex-1 flex-col outline-none"
              >
                <ConnectionsSection />
              </TabsPrimitive.Content>
              <TabsPrimitive.Content
                value="shortcuts"
                tabIndex={-1}
                className="flex min-h-0 flex-1 flex-col outline-none"
              >
                <ShortcutsSection query={query} onQueryChange={setQuery} searchRef={searchRef} />
              </TabsPrimitive.Content>
            </div>
          </TabsPrimitive.Root>
        </DialogContent>
      </Dialog>
    </>
  );
}
