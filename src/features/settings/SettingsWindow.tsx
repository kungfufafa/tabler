import { X } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type MouseEvent } from "react";
import { flushSync } from "react-dom";
import { flushThemes } from "@/features/appearance/themesStore";
import { useAction } from "@/features/palette/actions";
import { ipc, isTauri } from "@/lib/ipc";
import { listenEvent, listenWindowEvent } from "@/lib/ipc/events";
import { windowsChrome } from "@/lib/platform";
import { flushPrefs } from "@/stores/prefs";
import { SETTINGS_DRAG_ATTR, SettingsPanel, type SettingsKeys } from "./SettingsPanel";
import { isSettingsSection, useSettingsDialog } from "./store";

async function currentWindow() {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow();
}

/** The section list's button of the section on screen. */
function activeSectionTrigger(): HTMLElement | null {
  return document.querySelector<HTMLElement>(
    "[data-slot='settings-panel'] [role='tab'][data-state='active']",
  );
}

/** How long the window waits for the section it was opened on before it shows anyway. */
const SECTION_WAIT_MS = 400;

/** Writes what waits to be saved (preferences, custom themes): the window is going away. */
async function flushSettings(): Promise<void> {
  await Promise.allSettled([flushPrefs(), flushThemes()]);
}

/** ⌘W and the close button: closes the Settings window (its close request flushes first). */
function closeSettingsWindow(): void {
  if (!isTauri()) return;
  void currentWindow()
    .then((w) => w.close())
    .catch((e: unknown) => console.warn("[settings] could not close the window", e));
}

/** ⌘, in the Settings window: it is already there. */
const stayHere = () => undefined;

/**
 * Takes the section the window was opened on (parked by the backend) and follows the ones asked
 * for later (`settings:section`). Resolves `ready` once the first answer is in (or after a short
 * wait), so the window shows on the right section.
 */
function useRequestedSection(): boolean {
  const [ready, setReady] = useState(false);
  useEffect(() => {
    let gone = false;
    let unlisten: (() => void) | undefined;
    const show = (section: unknown) => {
      if (!isSettingsSection(section)) return;
      // Taken once, so applied even by a run already cleaned up (React StrictMode runs effects
      // twice: the first run's take empties the parked section): only the store changes then.
      if (gone) {
        useSettingsDialog.getState().setSection(section);
        return;
      }
      const active = document.activeElement;
      const onList = !active || active === document.body || active === activeSectionTrigger();
      flushSync(() => useSettingsDialog.getState().setSection(section));
      // The focus follows from the list (not from a field the user is typing in).
      if (onList) activeSectionTrigger()?.focus();
    };
    const timer = setTimeout(() => setReady(true), SECTION_WAIT_MS);
    void (async () => {
      try {
        const off = await listenWindowEvent("settings:section", show);
        if (gone) off();
        else unlisten = off;
      } catch (e) {
        console.warn("[settings] cannot follow the requested section", e);
      }
      try {
        show(await ipc.windows.takeSettingsSection());
      } catch (e) {
        console.warn("[settings] could not read the requested section", e);
      }
      if (!gone) {
        clearTimeout(timer);
        setReady(true);
      }
    })();
    return () => {
      gone = true;
      clearTimeout(timer);
      unlisten?.();
    };
  }, []);
  return ready;
}

/**
 * Shows the window (built hidden, so it never shows half laid out) once its section is on screen,
 * and puts the focus on the section list, so ⌘1…⌘9 and the arrow keys work at once.
 */
function useReveal(ready: boolean): void {
  useLayoutEffect(() => {
    if (!ready) return;
    activeSectionTrigger()?.focus();
    if (!isTauri()) return;
    const frame = requestAnimationFrame(() => {
      void currentWindow()
        .then(async (w) => {
          await w.show();
          await w.setFocus();
        })
        .catch((e: unknown) => console.warn("[settings] could not show the window", e));
    });
    return () => cancelAnimationFrame(frame);
  }, [ready]);
}

/**
 * Closing the window (its close button, ⌘W, Alt+F4, the title bar's) writes the preferences and
 * themes still waiting for their debounced save first: a change made just before closing is kept
 * and reaches the other windows. The listener holds the native close while it writes, then
 * destroys the window (`core:window:allow-destroy`): the Settings window has nothing to ask.
 */
function useFlushOnClose(): void {
  useEffect(() => {
    if (!isTauri()) return;
    let gone = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      const win = await currentWindow();
      const off = await win.onCloseRequested(async (event) => {
        event.preventDefault();
        await flushSettings();
        await win.destroy();
      });
      if (gone) off();
      else unlisten = off;
    })().catch((e: unknown) => console.warn("[settings] cannot guard the close", e));
    return () => {
      gone = true;
      unlisten?.();
    };
  }, []);
}

/**
 * A quit (the menu's Quit, ⌘Q, the Dock, an exit request) asks every window: Settings has nothing
 * unsaved, so it writes its pending saves and agrees at once. The quit does not wait for its
 * answer, but the backend closes it before exiting (`exit_after_settings`), and that close writes
 * the pending saves (`useFlushOnClose`); when it is the only window left its answer is what quits.
 */
function useAnswerQuit(): void {
  useEffect(() => {
    let gone = false;
    let unlisten: (() => void) | undefined;
    void listenEvent("app:quit-requested", () => {
      void (async () => {
        await flushSettings();
        await ipc.app.setUnsaved(false).catch(() => undefined);
        await ipc.app.quit().catch(() => undefined);
      })();
    }).then((u) => {
      if (gone) u();
      else unlisten = u;
    });
    return () => {
      gone = true;
      unlisten?.();
    };
  }, []);
}

/** Areas that start a window drag must not swallow clicks on the controls inside them. */
const INTERACTIVE =
  "button, input, select, textarea, a, [role='button'], [role='combobox'], [role='switch'], [role='tab'], [role='radio'], [role='menuitem']";

/**
 * Windows: the section list's free space and the section headers drag the window (it has no
 * title bar, like TablePlus's), but not the controls in them.
 */
function startDragFrom(e: MouseEvent<HTMLDivElement>): void {
  if (e.button !== 0 || e.detail > 1) return;
  const target = e.target instanceof Element ? e.target : null;
  if (!target?.closest(`[${SETTINGS_DRAG_ATTR}]`) || target.closest(INTERACTIVE)) return;
  e.preventDefault();
  void currentWindow()
    .then((w) => w.startDragging())
    .catch(() => undefined);
}

/**
 * The Settings window (TablePlus: "The Settings window holds the settings"): the settings panel
 * filling a window of its own, opened by Settings… (⌘,) in any window. One at most: asking again
 * brings it to the front on the section asked for. ⌘W and its close button close it; Esc does not.
 * Windows: no native title bar, so it draws its close button and drags by its headers.
 */
export function SettingsWindow() {
  const ready = useRequestedSection();
  const keysRef = useRef<SettingsKeys>(null);
  useReveal(ready);
  useFlushOnClose();
  useAnswerQuit();
  useAction("settings.open", stayHere);
  useAction("tab.close", closeSettingsWindow);
  useEffect(() => {
    document.title = "Settings";
    // Keys with nothing focused (after a click on empty space) still reach the panel: ⌘1…⌘9.
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.target === document.body || e.target === document.documentElement) keysRef.current?.(e);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
  const ownTitleBar = windowsChrome && isTauri();

  return (
    <div
      className="bg-background text-foreground relative flex h-screen w-screen overflow-hidden"
      onMouseDown={ownTitleBar ? startDragFrom : undefined}
      onKeyDown={(e) => keysRef.current?.(e.nativeEvent)}
    >
      <SettingsPanel
        variant="window"
        keysRef={keysRef}
        title={<h1 className="px-2 pt-1 pb-1.5 text-sm font-medium">Settings</h1>}
      />
      {ownTitleBar && (
        <button
          type="button"
          aria-label="Close"
          onClick={closeSettingsWindow}
          className="text-muted-foreground focus-visible:ring-ring/50 absolute top-0 right-0 flex h-11 w-11 items-center justify-center transition-colors outline-none hover:bg-[#c42b1c] hover:text-white focus-visible:ring-2 focus-visible:ring-inset"
        >
          <X aria-hidden className="size-4" strokeWidth={1.25} />
        </button>
      )}
    </div>
  );
}
