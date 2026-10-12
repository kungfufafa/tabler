import { isMock } from "@/lib/ipc";
import type { ImportKind } from "./store";

/** What the browser mock offers in its prompt (see src/lib/ipc/mock/import.ts). */
export const MOCK_IMPORT_FILES: Record<ImportKind, string> = {
  csv: "/Users/me/Downloads/users.csv",
  sql: "/Users/me/Downloads/tabler_dev.sql",
};

/**
 * Asks for the file to import: the native open panel in Tauri (CSV: .csv / .tsv / .txt; SQL:
 * .sql or gzip-compressed .sql.gz), `window.prompt` in the browser mock. Null = cancelled.
 */
export async function pickImportFile(kind: ImportKind): Promise<string | null> {
  if (isMock) {
    const answer = window.prompt(
      kind === "csv" ? "Import CSV file" : "Import SQL dump",
      MOCK_IMPORT_FILES[kind],
    );
    const path = answer?.trim();
    return path ? path : null;
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    title: kind === "csv" ? "Import From CSV" : "Import From SQL Dump",
    multiple: false,
    directory: false,
    filters:
      kind === "csv"
        ? [{ name: "CSV", extensions: ["csv", "tsv", "txt"] }]
        : [{ name: "SQL dump", extensions: ["sql", "gz"] }],
  });
  return typeof picked === "string" ? picked : null;
}
