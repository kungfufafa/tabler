import { isMock } from "@/lib/ipc";

const SQL_FILTERS = [
  { name: "SQL", extensions: ["sql"] },
  { name: "All files", extensions: ["*"] },
];

/** Last path segment ("/a/b/report.sql" → "report.sql"). */
export function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

/** Appends `.sql` when the chosen name has no extension. */
export function withSqlExtension(path: string): string {
  return /\.[^\\/.]+$/.test(baseName(path)) ? path : `${path}.sql`;
}

/**
 * Native "Open" panel for .sql files. The browser mock has no file system dialog, so it asks for a
 * path (files live in the mock's in-memory map).
 */
export async function pickOpenPath(): Promise<string | null> {
  if (isMock) {
    const path = window.prompt("Open .sql file — path", "/Users/me/queries/sample.sql");
    return path?.trim() || null;
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    title: "Open SQL File",
    multiple: false,
    directory: false,
    filters: SQL_FILTERS,
  });
  return typeof picked === "string" ? picked : null;
}

/** Native "Save" panel; `suggested` is the default file name. */
export async function pickSavePath(suggested: string): Promise<string | null> {
  if (isMock) {
    const path = window.prompt("Save query as — path", `/Users/me/queries/${suggested}`);
    return path?.trim() ? withSqlExtension(path.trim()) : null;
  }
  const { save } = await import("@tauri-apps/plugin-dialog");
  const picked = await save({ title: "Save Query", defaultPath: suggested, filters: SQL_FILTERS });
  return picked ? withSqlExtension(picked) : null;
}
