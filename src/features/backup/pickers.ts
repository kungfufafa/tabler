import { isMock } from "@/lib/ipc";

/** Where the browser mock pretends backups land. */
export const MOCK_BACKUP_DIR = "/Users/me/Backups";

/**
 * Asks for the folder a backup is written to: the native open panel (folders only) in Tauri,
 * `window.prompt` in the browser mock. Null = cancelled.
 */
export async function pickBackupFolder(lastDir: string | null): Promise<string | null> {
  if (isMock) {
    const answer = window.prompt("Back up into folder", lastDir ?? MOCK_BACKUP_DIR);
    const dir = answer?.trim().replace(/\/+$/, "");
    return dir ? dir : null;
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    title: "Choose a folder for the backup",
    directory: true,
    multiple: false,
    canCreateDirectories: true,
    defaultPath: lastDir ?? undefined,
  });
  return typeof picked === "string" ? picked : null;
}

/** Asks for the dump file to restore. Null = cancelled. */
export async function pickRestoreFile(lastDir: string | null): Promise<string | null> {
  if (isMock) {
    const answer = window.prompt("Restore from file", `${lastDir ?? MOCK_BACKUP_DIR}/`);
    const path = answer?.trim();
    return path && !path.endsWith("/") ? path : null;
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    title: "Choose a dump to restore",
    multiple: false,
    directory: false,
    // No extension filter: dumps come as .sql, .sql.gz, .dump, .backup, .tar or without one, and
    // the file's first bytes tell what it is.
    defaultPath: lastDir ?? undefined,
  });
  return typeof picked === "string" ? picked : null;
}

/** Asks where to save the copy of a SQLite database file. Null = cancelled. */
export async function pickSqliteCopyPath(fileName: string): Promise<string | null> {
  if (isMock) {
    const answer = window.prompt(
      "Save a copy of the database as",
      `${MOCK_BACKUP_DIR}/${fileName}`,
    );
    const path = answer?.trim();
    return path ? path : null;
  }
  const { save } = await import("@tauri-apps/plugin-dialog");
  const picked = await save({
    title: "Copy Database File",
    defaultPath: fileName,
    canCreateDirectories: true,
    filters: [{ name: "SQLite database", extensions: ["sqlite", "db", "sqlite3"] }],
  });
  return picked ?? null;
}

/** Shows the file in Finder (no-op in the browser mock). */
export async function revealInFinder(path: string): Promise<void> {
  if (isMock) return;
  const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
  await revealItemInDir(path);
}
