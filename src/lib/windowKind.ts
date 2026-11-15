import { isMock } from "@/lib/ipc/transport";

/** The Settings window's label (`SETTINGS_WINDOW` in src-tauri/src/windows.rs). */
export const SETTINGS_WINDOW_LABEL = "settings";

/** What a window shows: the app (workspace / Welcome) or the Settings window. */
export type WindowView = "app" | "settings";

/**
 * This window's label, synchronously (Tauri puts it in `__TAURI_INTERNALS__` before the page
 * runs; it is what `getCurrentWindow().label` reads). null outside Tauri.
 */
export function currentWindowLabel(): string | null {
  try {
    const internals = (globalThis as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ as
      { metadata?: { currentWindow?: { label?: unknown } } } | undefined;
    const label = internals?.metadata?.currentWindow?.label;
    return typeof label === "string" ? label : null;
  } catch {
    return null;
  }
}

/** `?view=` of the page's URL (the browser mock opens the Settings layout with `?view=settings`). */
function viewParam(): string | null {
  try {
    return new URLSearchParams(globalThis.location?.search ?? "").get("view");
  } catch {
    return null;
  }
}

/** What this window shows, decided before the first render: its label, else `?view=`. */
export function windowView(): WindowView {
  if (currentWindowLabel() === SETTINGS_WINDOW_LABEL) return "settings";
  return viewParam() === "settings" ? "settings" : "app";
}

/** Whether this window is the Settings window. */
export function isSettingsWindow(): boolean {
  return windowView() === "settings";
}

let ownWindowOverride: boolean | null = null;

/**
 * Whether Settings opens in a window of its own (the app) rather than as the in-window dialog
 * (the browser mock and the tests, where there is no other window to open).
 */
export function settingsHaveOwnWindow(): boolean {
  return ownWindowOverride ?? !isMock;
}

/** Test helper: pretend to be the app (true), the browser mock (false), or undo it (null). */
export function setSettingsOwnWindowForTests(on: boolean | null): void {
  ownWindowOverride = on;
}
