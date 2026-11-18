import { create } from "zustand";
import { isMock } from "@/lib/ipc";

/** "Export Connections ▸ …": what is exported. */
export interface ExportScope {
  /** Dialog title, e.g. "Export all connections" or "Export “Staging”". */
  title: string;
  profileIds: string[];
  /** Group paths written to the file (empty groups and subgroups included). */
  groups: string[];
  /** Suggested file name, without the extension. */
  fileName: string;
}

/** "New Group…" / "New Subgroup…", "Rename Group…" or "Change Icon…" (`group`: a path). */
export type GroupDialogRequest =
  | {
      kind: "new";
      /** The group the new one goes into ("" = top level). */
      parent: string;
      /** Profiles to move into the new group once created. */
      moveIds: string[];
    }
  | { kind: "rename"; group: string }
  | { kind: "icon"; group: string };

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
/** What TablePlus's "Export Connections ▸ …" writes (macOS and Windows alike). */
export const TABLEPLUS_FILE_EXTENSION = "tableplusconnection";
const TABLER_FILTER = { name: "Tabler connections", extensions: [CONNECTIONS_FILE_EXTENSION] };
const FILTERS = [TABLER_FILTER];
const TABLEPLUS_FILTER = { name: "TablePlus connections", extensions: [TABLEPLUS_FILE_EXTENSION] };
const ANY_IMPORT_FILTER = {
  name: "Connections (Tabler, TablePlus)",
  extensions: [CONNECTIONS_FILE_EXTENSION, TABLEPLUS_FILE_EXTENSION],
};

/** Where an import comes from: any connections file, or a TablePlus export in particular. */
export type ImportSource = "any" | "tableplus";

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

/**
 * Asks for a connections file to import (open panel): a Tabler or a TablePlus file, with the
 * TablePlus filter first for "Import from TablePlus…" (the backend tells the two apart by their
 * content, not by their name).
 */
export async function pickImportPath(source: ImportSource = "any"): Promise<string | null> {
  const tableplus = source === "tableplus";
  if (isMock)
    return window.prompt(
      tableplus ? "Import TablePlus connections from" : "Import connections from",
      tableplus ? DEMO_TABLEPLUS_PATH : "/Users/me/Downloads/team.tablerconnections",
    );
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    title: tableplus ? "Import from TablePlus" : "Import Connections",
    multiple: false,
    directory: false,
    filters: tableplus
      ? [TABLEPLUS_FILTER, ANY_IMPORT_FILTER]
      : [ANY_IMPORT_FILTER, TABLER_FILTER, TABLEPLUS_FILTER],
  });
  return typeof picked === "string" ? picked : null;
}

/** The mock's TablePlus export (`lib/ipc/mock/connections_io.ts`). */
const DEMO_TABLEPLUS_PATH = "/Users/me/Downloads/TablePlus.tableplusconnection";

/**
 * "Import Connections…" / "Import from TablePlus…": picks the file, then opens the import dialog
 * on it.
 */
export async function startImportConnections(source: ImportSource = "any"): Promise<void> {
  const path = (await pickImportPath(source))?.trim();
  if (path) useConnectionsIo.setState({ importPath: path });
}
