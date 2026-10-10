import { create } from "zustand";
import type { ConnStatus, ConnectionInfo } from "@/lib/ipc";

export interface OpenConnection {
  info: ConnectionInfo;
  status: ConnStatus;
  message: string | null;
  /** Schema selected in the sidebar for this connection (PG schema / MySQL database / SQLite db). */
  currentSchema: string | null;
}

interface ConnectionsState {
  byId: Record<string, OpenConnection>;
  order: string[];
  activeId: string | null;
  add: (info: ConnectionInfo) => void;
  remove: (connectionId: string) => void;
  setActive: (connectionId: string | null) => void;
  setStatus: (connectionId: string, status: ConnStatus, message?: string | null) => void;
  setInfo: (info: ConnectionInfo) => void;
  setCurrentSchema: (connectionId: string, schema: string | null) => void;
}

export const useConnectionsStore = create<ConnectionsState>()((set) => ({
  byId: {},
  order: [],
  activeId: null,
  add: (info) =>
    set((s) => ({
      byId: {
        ...s.byId,
        [info.connectionId]: { info, status: "connected", message: null, currentSchema: null },
      },
      order: s.order.includes(info.connectionId) ? s.order : [...s.order, info.connectionId],
      activeId: info.connectionId,
    })),
  remove: (connectionId) =>
    set((s) => {
      const byId = { ...s.byId };
      delete byId[connectionId];
      const order = s.order.filter((id) => id !== connectionId);
      const activeId = s.activeId === connectionId ? (order[order.length - 1] ?? null) : s.activeId;
      return { byId, order, activeId };
    }),
  setActive: (activeId) => set({ activeId }),
  setStatus: (connectionId, status, message = null) =>
    set((s) => {
      const c = s.byId[connectionId];
      if (!c) return s;
      return { byId: { ...s.byId, [connectionId]: { ...c, status, message } } };
    }),
  setInfo: (info) =>
    set((s) => {
      const c = s.byId[info.connectionId];
      if (!c) return s;
      return { byId: { ...s.byId, [info.connectionId]: { ...c, info } } };
    }),
  setCurrentSchema: (connectionId, currentSchema) =>
    set((s) => {
      const c = s.byId[connectionId];
      if (!c) return s;
      return { byId: { ...s.byId, [connectionId]: { ...c, currentSchema } } };
    }),
}));

export function useActiveConnection(): OpenConnection | null {
  return useConnectionsStore((s) => (s.activeId ? (s.byId[s.activeId] ?? null) : null));
}
