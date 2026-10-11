import { create } from "zustand";

export const SETTINGS_SECTIONS = ["general", "editor", "connections", "shortcuts"] as const;
export type SettingsSection = (typeof SETTINGS_SECTIONS)[number];

interface SettingsDialogState {
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

/** Opens the settings dialog, optionally on a given section (e.g. "shortcuts"). */
export function openSettings(section?: SettingsSection): void {
  useSettingsDialog.setState((s) => ({ open: true, section: section ?? s.section }));
}

export function closeSettings(): void {
  useSettingsDialog.getState().setOpen(false);
}
