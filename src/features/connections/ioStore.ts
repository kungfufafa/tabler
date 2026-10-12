import { create } from "zustand";
import { isMock } from "@/lib/ipc";

/** "Export Connections ▸ …": what is exported. */
export interface ExportScope {
  /** Dialog title, e.g. "Export all connections" or "Export “Staging”". */
  title: string;
  profileIds: string[];
  /** Group names written to the file (empty groups included). */
  groups: string[];
  /** Suggested file name, without the extension. */
  fileName: string;
}

export type GroupDialogRequest =
  | { kind: "new"; /** Profiles to move into the new group once created. */ moveIds: string[] }
  | { kind: "rename"; group: string };

interface ConnectionsIoState {
  exportScope: ExportScope | null;
  /** The connections file being imported (picked before the dialog opens). */
  importPath: string | null;
  groupDialog: GroupDialogRequest | null;
}

/** Open state of the export / import / group-name dialogs (mounted by `ConnectionDialogHost`). */
export const useConnectionsIo = create<ConnectionsIoState>()(() => ({
  exportScope: null,
  importPath: null,
  groupDialog: null,
}));

export const CONNECTIONS_FILE_EXTENSION = "tablerconnections";
const FILTERS = [{ name: "Tabler connections", extensions: [CONNECTIONS_FILE_EXTENSION] }];

export function openExportConnections(scope: ExportScope): void {
  useConnectionsIo.setState({ exportScope: scope });
}

export function openGroupDialog(request: GroupDialogRequest): void {
  useConnectionsIo.setState({ groupDialog: request });
}

/** Asks where to save a connections file (save panel). */
export async function pickExportPath(defaultName: string): Promise<string | null> {
  const name = `${defaultName}.${CONNECTIONS_FILE_EXTENSION}`;
  if (isMock) return window.prompt("Export connections to", `/Users/me/Downloads/${name}`);
  const { save } = await import("@tauri-apps/plugin-dialog");
  const picked = await save({ title: "Export Connections", defaultPath: name, filters: FILTERS });
  return typeof picked === "string" ? picked : null;
}

/** Asks for a connections file to import (open panel). */
export async function pickImportPath(): Promise<string | null> {
  if (isMock)
    return window.prompt("Import connections from", "/Users/me/Downloads/team.tablerconnections");
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    title: "Import Connections",
    multiple: false,
    directory: false,
    filters: FILTERS,
  });
  return typeof picked === "string" ? picked : null;
}

/** "Import Connections…": picks the file, then opens the import dialog on it. */
export async function startImportConnections(): Promise<void> {
  const path = (await pickImportPath())?.trim();
  if (path) useConnectionsIo.setState({ importPath: path });
}
