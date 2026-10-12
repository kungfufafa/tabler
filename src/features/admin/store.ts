import { create } from "zustand";

/** Server-administration surfaces of a live connection. */
export type AdminPanel =
  | "processList"
  | "users"
  | "createDatabase"
  | "createSchema"
  | "renameDatabase"
  | "dropDatabase"
  | "dropSchema";

export interface AdminRequest {
  panel: AdminPanel;
  /** Live connection id. */
  connectionId: string;
  /**
   * Database (rename / drop) or schema (drop) the dialog acts on. Without one, the drop dialogs
   * let the user pick it.
   */
  target?: string;
}

interface AdminState {
  request: AdminRequest | null;
}

export const useAdminStore = create<AdminState>()(() => ({ request: null }));

export function openAdmin(request: AdminRequest): void {
  useAdminStore.setState({ request });
}

export function closeAdmin(): void {
  useAdminStore.setState({ request: null });
}

/** Connection ▸ Process List… */
export const openProcessList = (connectionId: string) =>
  openAdmin({ panel: "processList", connectionId });
/** Connection ▸ User Management… */
export const openUserManagement = (connectionId: string) =>
  openAdmin({ panel: "users", connectionId });
/** ⌘K database list "+" / New Database… */
export const openCreateDatabase = (connectionId: string) =>
  openAdmin({ panel: "createDatabase", connectionId });
/** Sidebar "+" ▸ New Schema… (PostgreSQL) */
export const openCreateSchema = (connectionId: string) =>
  openAdmin({ panel: "createSchema", connectionId });
/** ⌘K database list ▸ right-click ▸ Rename… (PostgreSQL) */
export const openRenameDatabase = (connectionId: string, database: string) =>
  openAdmin({ panel: "renameDatabase", connectionId, target: database });
/** ⌘K database list ▸ right-click ▸ Drop… / "−"; without a database, the dialog asks which. */
export const openDropDatabase = (connectionId: string, database?: string) =>
  openAdmin({ panel: "dropDatabase", connectionId, target: database });
/** Drop Schema… (PostgreSQL); without a schema, the dialog asks which. */
export const openDropSchema = (connectionId: string, schema?: string) =>
  openAdmin({ panel: "dropSchema", connectionId, target: schema });
