import { create } from "zustand";
import { allowFeature } from "@/lib/caps";
import type { Feature, TableRef } from "@/lib/ipc";

/** CSV and JSON go through the import wizard; SQL dumps through their preview. */
export type ImportKind = "csv" | "json" | "sql";

export interface ImportRequest {
  connectionId: string;
  kind: ImportKind;
  /** Target table for a CSV / JSON import; null = chosen (or created) in the wizard. */
  table: TableRef | null;
}

/** What the session must offer for each kind of import. */
const IMPORT_FEATURE: Record<ImportKind, Feature> = {
  csv: "importCsv",
  json: "importJson",
  sql: "importSqlDump",
};

interface ImportState {
  request: ImportRequest | null;
  /** The file picked for `request`; null while the file panel is up. */
  path: string | null;
  /** Bumped on every open, so a wizard starts afresh. */
  session: number;
}

export const useImportStore = create<ImportState>()(() => ({
  request: null,
  path: null,
  session: 0,
}));

/**
 * Opens the import wizard (File ▸ Import, sidebar "Import ▸", grid menu): asks for the file,
 * then shows the CSV wizard or the SQL dump preview. Pass `path` to skip the file panel.
 */
export function openImport(request: ImportRequest, path: string | null = null): void {
  if (!allowFeature(request.connectionId, IMPORT_FEATURE[request.kind])) return;
  useImportStore.setState((s) => ({ request, path, session: s.session + 1 }));
}

export function closeImport(): void {
  useImportStore.setState({ request: null, path: null });
}

/** The file chosen for the open request. */
export function setImportPath(path: string): void {
  useImportStore.setState({ path });
}
