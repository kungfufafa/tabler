import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { ConnectionProfile } from "@/lib/ipc";
import type { ParsedConnectionUrl } from "./url";

interface ConnectionDialogState {
  open: boolean;
  /** Profile being edited, or null for a new connection. */
  profile: ConnectionProfile | null;
  /** A new connection's starting values ("New connection from URL…"); null for the defaults. */
  prefill: ParsedConnectionUrl | null;
  /** The "New connection from URL…" dialog. */
  urlOpen: boolean;
  /** The profile the form saved last (the welcome list selects it). */
  savedId: string | null;
  setOpen: (open: boolean) => void;
  setUrlOpen: (open: boolean) => void;
}

/**
 * Open state of the New/Edit connection dialog. The dialog is mounted app-wide
 * (`ConnectionDialogHost`), so ⌘N, the palette or the toolbar's lock open it over the workspace.
 */
export const useConnectionDialog = create<ConnectionDialogState>()((set) => ({
  open: false,
  profile: null,
  prefill: null,
  urlOpen: false,
  savedId: null,
  // Keep `profile` while closing so the dialog's content doesn't change during the exit animation.
  setOpen: (open) => set({ open }),
  setUrlOpen: (urlOpen) => set({ urlOpen }),
}));

/**
 * Opens the connection form: a new connection by default (starting with the driver picker), a new
 * one prefilled from a connection URL, or `profile` to edit it.
 */
export function openConnectionDialog(
  profile: ConnectionProfile | null = null,
  prefill: ParsedConnectionUrl | null = null,
): void {
  useConnectionDialog.setState({ open: true, profile, prefill: profile ? null : prefill });
}

/** Opens "New connection from URL…". */
export function openConnectionUrlDialog(): void {
  useConnectionDialog.setState({ urlOpen: true });
}

interface LauncherPrefsState {
  /**
   * Connection groups collapsed in the launcher list. Never "": the connections without a group
   * have no header and are always shown.
   */
  collapsedGroups: readonly string[];
  toggleGroup: (group: string) => void;
}

/** Launcher list preferences, remembered across launches. */
export const useLauncherPrefs = create<LauncherPrefsState>()(
  persist(
    (set) => ({
      collapsedGroups: [],
      toggleGroup: (group) => {
        if (group === "") return;
        set((s) => ({
          collapsedGroups: s.collapsedGroups.includes(group)
            ? s.collapsedGroups.filter((g) => g !== group)
            : [...s.collapsedGroups, group],
        }));
      },
    }),
    {
      name: "tabler.launcher",
      version: 2,
      partialize: (s) => ({ collapsedGroups: s.collapsedGroups }),
      // v1 had an "Ungrouped" header that could collapse "": drop it, there is no header to undo it.
      migrate: (persisted) => {
        const stored = (persisted as { collapsedGroups?: unknown } | null)?.collapsedGroups;
        const groups = Array.isArray(stored) ? stored : [];
        return {
          collapsedGroups: groups.filter((g): g is string => typeof g === "string" && g !== ""),
        };
      },
    },
  ),
);
