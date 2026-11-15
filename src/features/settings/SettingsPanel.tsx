import {
  FolderOpen,
  Keyboard,
  Palette,
  Plug,
  Server,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  SquareCode,
  type LucideIcon,
} from "lucide-react";
import { Tabs as TabsPrimitive } from "radix-ui";
import { useEffect, useLayoutEffect, useRef, type ReactNode, type RefObject } from "react";
import { flushSync } from "react-dom";
import { AppearanceSettings } from "@/features/appearance";
import { AssistantSettingsGroup } from "@/features/assistant/AssistantSettings";
import { LocationsSettings } from "@/features/locations/LocationsSettings";
import { McpSettings } from "@/features/mcp/McpSettings";
import { Kbd } from "@/components/Kbd";
import { comboFromEvent, isMac } from "@/lib/keys/combo";
import { isCapturingShortcut } from "@/lib/keys/useShortcuts";
import { cn } from "@/lib/utils";
import { flushPrefs, loadPrefs, usePrefs } from "@/stores/prefs";
import { applyUiFontSize } from "./applyPrefs";
import { SectionShell } from "./controls";
import { installCrossWindowSync } from "./crossWindow";
import { ConnectionsSection, EditorSection, GeneralSection, SecuritySection } from "./sections";
import { ShortcutsSection } from "./ShortcutsSection";
import { SETTINGS_SECTIONS, useSettingsDialog, type SettingsSection } from "./store";

const SECTION_META: Record<SettingsSection, { label: string; icon: LucideIcon }> = {
  general: { label: "General", icon: SlidersHorizontal },
  editor: { label: "Editor", icon: SquareCode },
  connections: { label: "Connections", icon: Plug },
  locations: { label: "Locations", icon: FolderOpen },
  shortcuts: { label: "Shortcuts", icon: Keyboard },
  security: { label: "Security", icon: ShieldCheck },
  assistant: { label: "AI Assistant", icon: Sparkles },
  mcp: { label: "MCP Server", icon: Server },
  appearance: { label: "Appearance", icon: Palette },
};

/** Settings ▸ Locations: where the connections, settings and saved queries are kept. */
function LocationsSection() {
  return (
    <SectionShell title="Locations">
      <LocationsSettings />
    </SectionShell>
  );
}

/** Settings ▸ AI Assistant: provider, model, API key and what is sent. */
function AssistantSection() {
  return (
    <SectionShell title="AI Assistant">
      <AssistantSettingsGroup />
    </SectionShell>
  );
}

/** Settings ▸ MCP Server: the `tabler --mcp` server and its one-click install. */
function McpSection() {
  return (
    <SectionShell title="MCP Server">
      <McpSettings />
    </SectionShell>
  );
}

/**
 * Loads preferences once per session, applies the interface text size, flushes saves on exit and
 * follows the settings other windows change. Every window mounts one (the app shell through the
 * settings dialog, the Settings window itself).
 */
export function PrefsLoader() {
  const uiFontSize = usePrefs((p) => p.uiFontSize);
  useLayoutEffect(() => applyUiFontSize(uiFontSize), [uiFontSize]);
  useEffect(() => installCrossWindowSync(), []);
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

/** Element attribute of the areas that drag the Settings window (Windows, no title bar). */
export const SETTINGS_DRAG_ATTR = "data-settings-drag";

const CONTENT_CLASS = "flex min-h-0 flex-1 flex-col outline-none";

/** The panel's keyboard handler, for the container that hears the keys (see `SettingsPanel`). */
export type SettingsKeys = (e: globalThis.KeyboardEvent) => void;

/**
 * The Settings body: the section list (⌘1…⌘9) and the sections, General / Editor / Connections /
 * Locations / Shortcuts / Security / AI Assistant / MCP Server / Appearance; ⌘F searches the
 * shortcuts. Shown in the Settings window (the app) and in the settings dialog (the browser mock).
 * `title` is the heading over the list (the dialog passes its `DialogTitle`). The keys are heard
 * by the container (`keysRef`): the whole dialog, or the whole window.
 */
export function SettingsPanel({
  title,
  variant,
  keysRef,
  searchRef: outerSearchRef,
  className,
}: {
  title: ReactNode;
  variant: "dialog" | "window";
  keysRef: RefObject<SettingsKeys | null>;
  /** The Shortcuts search field (the dialog clears it on Esc itself). */
  searchRef?: RefObject<HTMLInputElement | null>;
  className?: string;
}) {
  const section = useSettingsDialog((s) => s.section);
  const setSection = useSettingsDialog((s) => s.setSection);
  const query = useSettingsDialog((s) => s.query);
  const setQuery = useSettingsDialog((s) => s.setQuery);
  const ownSearchRef = useRef<HTMLInputElement>(null);
  const searchRef = outerSearchRef ?? ownSearchRef;
  const triggerRefs = useRef<Partial<Record<SettingsSection, HTMLButtonElement | null>>>({});

  /**
   * Shows a section and focuses an element in it. The render is flushed first so the new section is
   * in the DOM and whatever the old one owned is gone, e.g. an open Select whose focus trap would
   * otherwise pull focus straight back into its (now unmounted) listbox.
   */
  const goToSection = (next: SettingsSection, target: () => HTMLElement | null | undefined) => {
    flushSync(() => setSection(next));
    target()?.focus();
  };

  const onKeyDown = (e: globalThis.KeyboardEvent) => {
    const combo = comboFromEvent(e);
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
      return;
    }
    // The window has no Esc to close it (TablePlus's neither); Esc in a non-empty search clears
    // it, like a native search field. The dialog does this in its own Esc handler.
    if (
      variant === "window" &&
      e.key === "Escape" &&
      !isCapturingShortcut() &&
      query &&
      document.activeElement === searchRef.current
    ) {
      e.preventDefault();
      setQuery("");
    }
  };
  // The latest handler for the container that listens (the dialog's content, the window).
  useLayoutEffect(() => {
    keysRef.current = onKeyDown;
  });

  return (
    <TabsPrimitive.Root
      orientation="vertical"
      value={section}
      onValueChange={(v) => setSection(v as SettingsSection)}
      data-slot="settings-panel"
      className={cn("flex min-h-0 min-w-0 flex-1", className)}
    >
      <div className="border-border bg-sidebar flex w-44 shrink-0 flex-col gap-1 border-r p-2">
        <div {...{ [SETTINGS_DRAG_ATTR]: "" }} className={cn(variant === "window" && "pt-1")}>
          {title}
        </div>
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
                className="group text-foreground hover:bg-row-hover focus-visible:ring-ring data-[state=active]:bg-brand-soft flex h-7 items-center gap-2 rounded-md px-2 text-left text-sm transition-colors outline-none focus-visible:ring-1 focus-visible:ring-inset"
              >
                <Icon
                  aria-hidden
                  className="text-muted-foreground group-data-[state=active]:text-brand size-3.5 shrink-0"
                />
                <span className="flex-1 truncate">{label}</span>
                <Kbd combo={combo} className="hidden group-hover:inline-flex" />
              </TabsPrimitive.Trigger>
            );
          })}
        </TabsPrimitive.List>
        {/* The empty rest of the list drags the window too. */}
        <div {...{ [SETTINGS_DRAG_ATTR]: "" }} className="min-h-0 flex-1" />
      </div>
      {/* Panels get tabIndex -1: each section starts with a focusable control, so Radix's
          default tab stop on the panel itself would be an invisible extra Tab press. */}
      <div className="bg-background flex min-w-0 flex-1 flex-col">
        <TabsPrimitive.Content value="general" tabIndex={-1} className={CONTENT_CLASS}>
          <GeneralSection />
        </TabsPrimitive.Content>
        <TabsPrimitive.Content value="editor" tabIndex={-1} className={CONTENT_CLASS}>
          <EditorSection />
        </TabsPrimitive.Content>
        <TabsPrimitive.Content value="connections" tabIndex={-1} className={CONTENT_CLASS}>
          <ConnectionsSection />
        </TabsPrimitive.Content>
        <TabsPrimitive.Content value="locations" tabIndex={-1} className={CONTENT_CLASS}>
          <LocationsSection />
        </TabsPrimitive.Content>
        <TabsPrimitive.Content value="shortcuts" tabIndex={-1} className={CONTENT_CLASS}>
          <ShortcutsSection query={query} onQueryChange={setQuery} searchRef={searchRef} />
        </TabsPrimitive.Content>
        <TabsPrimitive.Content value="security" tabIndex={-1} className={CONTENT_CLASS}>
          <SecuritySection />
        </TabsPrimitive.Content>
        <TabsPrimitive.Content value="assistant" tabIndex={-1} className={CONTENT_CLASS}>
          <AssistantSection />
        </TabsPrimitive.Content>
        <TabsPrimitive.Content value="mcp" tabIndex={-1} className={CONTENT_CLASS}>
          <McpSection />
        </TabsPrimitive.Content>
        <TabsPrimitive.Content value="appearance" tabIndex={-1} className={CONTENT_CLASS}>
          <AppearanceSettings />
        </TabsPrimitive.Content>
      </div>
    </TabsPrimitive.Root>
  );
}
