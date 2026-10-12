import { create } from "zustand";
import type { Driver } from "@/lib/ipc";
import { useConnectionsStore, type OpenConnection } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";

/**
 * The workspace `delta` steps away from `activeId` in rail order, wrapping around; null when there
 * is nothing else to go to.
 */
export function stepWorkspace(
  order: readonly string[],
  activeId: string | null,
  delta: number,
): string | null {
  if (order.length === 0) return null;
  const i = activeId === null ? -1 : order.indexOf(activeId);
  if (i < 0) return order[delta < 0 ? order.length - 1 : 0] ?? null;
  const next = order[(((i + delta) % order.length) + order.length) % order.length] ?? null;
  return next === activeId ? null : next;
}

/** Shows a workspace (its connection, last tab and sidebar), leaving the connection manager. */
export function showWorkspace(connectionId: string): void {
  const connections = useConnectionsStore.getState();
  if (!connections.byId[connectionId]) return;
  connections.setActive(connectionId);
  useUiStore.getState().setView("workspace");
}

/**
 * Select Next / Previous Workspace (TablePlus Keymap ▸ Navigate). From the connection manager the
 * active workspace comes back first.
 */
export function cycleWorkspace(delta: 1 | -1): void {
  const { order, activeId } = useConnectionsStore.getState();
  if (useUiStore.getState().view === "launcher" && activeId) {
    showWorkspace(activeId);
    return;
  }
  const target = stepWorkspace(order, activeId, delta);
  if (target) showWorkspace(target);
}

/** Open state of Show Workspaces (⇧⌘\). */
export const useWorkspaceSwitcher = create<{ open: boolean }>()(() => ({ open: false }));

export function openWorkspaceSwitcher(): void {
  useWorkspaceSwitcher.setState({ open: true });
}

export const toggleWorkspaceSwitcher = (): void =>
  useWorkspaceSwitcher.setState((s) => ({ open: !s.open }));

export interface WorkspaceEntry {
  connectionId: string;
  name: string;
  color: string | null;
  driver: Driver;
  database: string | null;
  tabCount: number;
}

/** The open workspaces in rail order, filtered by name / database (every term must match). */
export function workspaceEntries(
  order: readonly string[],
  byId: Readonly<Record<string, OpenConnection>>,
  tabCounts: ReadonlyMap<string, number>,
  query = "",
): WorkspaceEntry[] {
  const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
  return order.flatMap((id) => {
    const c = byId[id];
    if (!c) return [];
    const entry: WorkspaceEntry = {
      connectionId: id,
      name: c.info.name,
      color: c.info.color,
      driver: c.info.driver,
      database: c.info.currentDatabase,
      tabCount: tabCounts.get(id) ?? 0,
    };
    const text = `${entry.name} ${entry.database ?? ""}`.toLowerCase();
    return terms.every((t) => text.includes(t)) ? [entry] : [];
  });
}
