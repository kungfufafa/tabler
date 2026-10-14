import { useMemo } from "react";
import { create } from "zustand";
import type { Driver } from "@/lib/ipc";
import { useConnectionsStore, type OpenConnection } from "@/stores/connections";
import { workspaceKey } from "@/stores/session";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";

/**
 * A workspace is one open connection: a profile on one database (TablePlus opens another database
 * of the server in a workspace of its own). Its key (profile + database) outlives the connection
 * id, so the rail's order survives reconnects and restarts.
 */
export function connectionWorkspaceKey(conn: OpenConnection): string {
  return workspaceKey({ profileId: conn.info.profileId, database: conn.info.currentDatabase });
}

/**
 * The open connections in rail order: those whose workspace key is in `saved` (the order the user
 * dragged) by its position there, then the others in the order they were opened.
 */
export function sortWorkspaces(
  order: readonly string[],
  byId: Readonly<Record<string, OpenConnection>>,
  saved: readonly string[],
): string[] {
  const rank = new Map(saved.map((key, i) => [key, i]));
  const at = (id: string) => {
    const conn = byId[id];
    return conn ? (rank.get(connectionWorkspaceKey(conn)) ?? Infinity) : Infinity;
  };
  return order
    .map((id, i) => ({ id, i, r: at(id) }))
    .sort((a, b) => (a.r === b.r ? a.i - b.i : a.r - b.r))
    .map((e) => e.id);
}

/**
 * The saved rail order after dragging the workspace at `from` to `to` (positions in `visible`, the
 * rail as shown): the visible workspaces' keys in their new order, then the saved keys of
 * workspaces that are not open (kept for when they come back).
 */
export function reorderWorkspaces(
  visible: readonly string[],
  byId: Readonly<Record<string, OpenConnection>>,
  saved: readonly string[],
  from: number,
  to: number,
): string[] {
  const ids = [...visible];
  if (from < 0 || to < 0 || from >= ids.length || to >= ids.length) return [...saved];
  const [moved] = ids.splice(from, 1);
  ids.splice(to, 0, moved!);
  const keys: string[] = [];
  for (const id of ids) {
    const conn = byId[id];
    if (!conn) continue;
    const key = connectionWorkspaceKey(conn);
    if (!keys.includes(key)) keys.push(key);
  }
  return [...keys, ...saved.filter((k) => !keys.includes(k))];
}

/** The open connections in rail order, now. */
export function railOrder(): string[] {
  const { order, byId } = useConnectionsStore.getState();
  return sortWorkspaces(order, byId, useUiStore.getState().railOrder);
}

/** The open connections in rail order (reactive). */
export function useRailOrder(): string[] {
  const order = useConnectionsStore((s) => s.order);
  const byId = useConnectionsStore((s) => s.byId);
  const saved = useUiStore((s) => s.railOrder);
  return useMemo(() => sortWorkspaces(order, byId, saved), [order, byId, saved]);
}

/** Drags a rail entry (positions in the rail as shown) and keeps the new order. */
export function moveWorkspace(from: number, to: number): void {
  const { byId } = useConnectionsStore.getState();
  const ui = useUiStore.getState();
  ui.setRailOrder(reorderWorkspaces(railOrder(), byId, ui.railOrder, from, to));
}

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
  const { activeId } = useConnectionsStore.getState();
  const order = railOrder();
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

/**
 * The two letters of a workspace's icon in the rail (TablePlus's database icons): the initials of
 * the first two words of the database name (`tabler_dev` → TD, `ShopOrders` → SO), else its first
 * two letters (`shop` → SH); a SQLite file shows its name without the extension.
 */
export function workspaceInitials(name: string): string {
  const base = name.split(/[\\/]/).pop() ?? name;
  const stem = base.replace(/\.(sqlite3?|db3?|s3db|sl3)$/i, "");
  const words = stem
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .split(/[^\p{L}\p{N}]+/u)
    .filter(Boolean);
  if (words.length === 0) return "?";
  const letters =
    words.length >= 2
      ? `${[...words[0]!][0]}${[...words[1]!][0]}`
      : [...words[0]!].slice(0, 2).join("");
  return letters.toUpperCase();
}

/** Whether the workspace on screen is split (Split Right). */
export function useActiveSplit(): boolean {
  const connectionId = useConnectionsStore((s) => s.activeId);
  return useTabsStore((s) => (connectionId === null ? false : !!s.splits[connectionId]));
}
