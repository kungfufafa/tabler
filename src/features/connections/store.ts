import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { ConnectionProfile } from "@/lib/ipc";
import { CONNECTION_SORTS, type ConnectionSort } from "./list";
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
  /** "Sort by ▸" of the welcome screen (inside each group). */
  sortBy: ConnectionSort;
  setSortBy: (sort: ConnectionSort) => void;
  /** Follows a renamed group's collapsed state; forgets a deleted one. */
  renameCollapsed: (from: string, to: string | null) => void;
}

const isSort = (v: unknown): v is ConnectionSort =>
  (CONNECTION_SORTS as readonly unknown[]).includes(v);

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
      sortBy: "manual",
      setSortBy: (sortBy) => set({ sortBy }),
      renameCollapsed: (from, to) =>
        set((s) =>
          s.collapsedGroups.includes(from)
            ? {
                collapsedGroups: [
                  ...s.collapsedGroups.filter((g) => g !== from && g !== to),
                  ...(to ? [to] : []),
                ],
              }
            : s,
        ),
    }),
    {
      name: "tabler.launcher",
      version: 3,
      partialize: (s) => ({ collapsedGroups: s.collapsedGroups, sortBy: s.sortBy }),
      // v1 had an "Ungrouped" header that could collapse "": drop it, there is no header to undo it.
      // v3 added the sort order.
      migrate: (persisted) => {
        const state = persisted as { collapsedGroups?: unknown; sortBy?: unknown } | null;
        const groups = Array.isArray(state?.collapsedGroups) ? state.collapsedGroups : [];
        return {
          collapsedGroups: groups.filter((g): g is string => typeof g === "string" && g !== ""),
          sortBy: isSort(state?.sortBy) ? state.sortBy : "manual",
        };
      },
    },
  ),
);
