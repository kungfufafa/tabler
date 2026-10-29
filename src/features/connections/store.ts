import { toast } from "sonner";
import { create } from "zustand";
import { persist } from "zustand/middleware";
import { isCoreDriver, kindOf, visibleKinds } from "@/features/drivers/catalog";
import type { ConnectionProfile } from "@/lib/ipc";
import { useAppInfoStore } from "@/stores/appInfo";
import { liftPath, normalizePath, rebasePath } from "./groups";
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
  // The form has the fields of the core drivers (their flavors too) and of the databases this
  // build has (a newer or fuller Tabler may have saved another).
  if (profile && !isCoreDriver(profile.driver) && !builtKind(profile)) {
    toast.error(`“${profile.name}” cannot be edited in this version of Tabler`);
    return;
  }
  useConnectionDialog.setState({ open: true, profile, prefill: profile ? null : prefill });
}

/** Whether this build has the database of `profile` (`AppInfo.kinds`). */
function builtKind(profile: ConnectionProfile): boolean {
  return visibleKinds(useAppInfoStore.getState().info).includes(kindOf(profile));
}

/**
 * "New ▸ Connection in this group…" on a group header: a new connection with its Group field
 * set to `group` (a path). It starts on the form (PostgreSQL), like any prefilled connection.
 */
export function openNewConnectionInGroup(group: string): void {
  openConnectionDialog(null, { values: { group: normalizePath(group) }, password: null });
}

/**
 * Opens "New connection from URL…": the welcome screen's action, and what "Import from URL…" in
 * the New connection sheet calls after closing the sheet (the URL dialog then reopens the form,
 * prefilled, through `openConnectionDialog`).
 */
export function openConnectionUrlDialog(): void {
  useConnectionDialog.setState({ urlOpen: true });
}

interface LauncherPrefsState {
  /**
   * Paths of the connection groups collapsed in the launcher list. Never "": the connections
   * without a group have no header and are always shown.
   */
  collapsedGroups: readonly string[];
  toggleGroup: (group: string) => void;
  /** "Sort by ▸" of the welcome screen (inside each group). */
  sortBy: ConnectionSort;
  setSortBy: (sort: ConnectionSort) => void;
  /**
   * Follows a renamed or moved group (`to`: its new path), subgroups included; for a deleted one
   * (`to` null) forgets it and moves its subgroups' state up one level.
   */
  renameCollapsed: (from: string, to: string | null) => void;
}

/** The collapsed paths after the group `from` became `to` (null: was deleted). */
export function remapCollapsed(
  collapsed: readonly string[],
  from: string,
  to: string | null,
): string[] {
  const out: string[] = [];
  for (const path of collapsed) {
    const next = to === null ? liftPath(path, from) : rebasePath(path, from, to);
    if (next && !out.includes(next)) out.push(next);
  }
  return out;
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
        set((s) => {
          const next = remapCollapsed(s.collapsedGroups, from, to);
          const changed =
            next.length !== s.collapsedGroups.length ||
            next.some((g, i) => g !== s.collapsedGroups[i]);
          return changed ? { collapsedGroups: next } : s;
        }),
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
