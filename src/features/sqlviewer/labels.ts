import { baseName, headTitle } from "@/features/editor/files";
import type { SqlDialect } from "@/features/editor/scanner";
import type { ViewerDoc } from "./store";

export const DIALECT_LABELS: Record<SqlDialect, string> = {
  postgres: "PostgreSQL",
  mysql: "MySQL",
  sqlite: "SQLite",
};

export const DIALECTS = Object.keys(DIALECT_LABELS) as SqlDialect[];

/** "report.sql", "dump-head.sql" for the head of a big file, or "Untitled" for a buffer without a file. */
export function viewerTitle(doc: Pick<ViewerDoc, "path" | "headOf"> | undefined): string {
  if (doc?.path) return baseName(doc.path);
  return doc?.headOf ? headTitle(doc.headOf) : "Untitled";
}

/** A viewer tab's title for its file (or an untitled viewer). */
export function viewerTabTitle(path: string | null): string {
  return path ? baseName(path) : "SQL Viewer";
}
