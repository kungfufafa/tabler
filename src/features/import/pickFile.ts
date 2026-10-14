import { isMock } from "@/lib/ipc";
import type { ImportKind } from "./store";

/** What the browser mock offers in its prompt (see src/lib/ipc/mock/import.ts). */
export const MOCK_IMPORT_FILES: Record<ImportKind, string> = {
  csv: "/Users/me/Downloads/users.csv",
  json: "/Users/me/Downloads/users.json",
  sql: "/Users/me/Downloads/tabler_dev.sql",
};

const TITLES: Record<ImportKind, string> = {
  csv: "Import From CSV",
  json: "Import From JSON",
  sql: "Import From SQL Dump",
};

const FILTERS: Record<ImportKind, Array<{ name: string; extensions: string[] }>> = {
  csv: [{ name: "CSV", extensions: ["csv", "tsv", "txt"] }],
  json: [{ name: "JSON", extensions: ["json", "ndjson", "jsonl"] }],
  sql: [{ name: "SQL dump", extensions: ["sql", "gz"] }],
};

/**
 * Asks for the file to import: the native open panel in Tauri (CSV: .csv / .tsv / .txt; JSON:
 * .json / .ndjson / .jsonl; SQL: .sql or gzip-compressed .sql.gz), `window.prompt` in the browser
 * mock. Null = cancelled.
 */
export async function pickImportFile(kind: ImportKind): Promise<string | null> {
  if (isMock) {
    const answer = window.prompt(
      kind === "csv" ? "Import CSV file" : kind === "json" ? "Import JSON file" : "Import SQL dump",
      MOCK_IMPORT_FILES[kind],
    );
    const path = answer?.trim();
    return path ? path : null;
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    title: TITLES[kind],
    multiple: false,
    directory: false,
    filters: FILTERS[kind],
  });
  return typeof picked === "string" ? picked : null;
}
