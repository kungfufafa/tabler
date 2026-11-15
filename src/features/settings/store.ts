import { create } from "zustand";
import { ipc } from "@/lib/ipc";
import { isSettingsWindow, settingsHaveOwnWindow } from "@/lib/windowKind";

export const SETTINGS_SECTIONS = [
  "general",
  "editor",
  "connections",
  "locations",
  "shortcuts",
  "security",
  "assistant",
  "mcp",
  "appearance",
] as const;
export type SettingsSection = (typeof SETTINGS_SECTIONS)[number];

export function isSettingsSection(value: unknown): value is SettingsSection {
  return typeof value === "string" && (SETTINGS_SECTIONS as readonly string[]).includes(value);
}

interface SettingsDialogState {
  /** The in-window dialog is open (the browser mock; the app opens the Settings window). */
  open: boolean;
  /** Last viewed section; kept across opens like a macOS settings window. */
  section: SettingsSection;
  /** Shortcuts search text; cleared when the dialog closes. */
  query: string;
  setOpen: (open: boolean) => void;
  setSection: (section: SettingsSection) => void;
  setQuery: (query: string) => void;
}

export const useSettingsDialog = create<SettingsDialogState>()((set) => ({
  open: false,
  section: "general",
  query: "",
  setOpen: (open) => set(open ? { open } : { open, query: "" }),
  setSection: (section) => set({ section }),
  setQuery: (query) => set({ query }),
}));

/**
 * Opens Settings, optionally on a given section (e.g. "shortcuts"). Like TablePlus, Settings is a
 * window of its own: the app opens it (or brings it to the front) through the backend; inside it,
 * this just switches the section. The browser mock has no other window, so there Settings stays
 * the in-window dialog.
 */
export function openSettings(section?: SettingsSection): void {
  if (isSettingsWindow()) {
    if (section) useSettingsDialog.getState().setSection(section);
    return;
  }
  if (settingsHaveOwnWindow()) {
    ipc.windows.openSettings(section ?? null).catch((err: unknown) => {
      console.warn("[settings] could not open the Settings window; using the dialog", err);
      showDialog(section);
    });
    return;
  }
  showDialog(section);
}

function showDialog(section?: SettingsSection): void {
  useSettingsDialog.setState((s) => ({ open: true, section: section ?? s.section }));
}

export function closeSettings(): void {
  useSettingsDialog.getState().setOpen(false);
}
