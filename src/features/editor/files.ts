import { toast } from "sonner";
import { openImport } from "@/features/import/store";
import { ipc, isMock } from "@/lib/ipc";
import { askLargeFile, HEAD_BYTES, LARGE_FILE_BYTES, LARGE_GZ_BYTES, sizeLabel } from "./largeFile";

const SQL_FILTERS = [
  { name: "SQL", extensions: ["sql"] },
  { name: "All files", extensions: ["*"] },
];
/** Open also offers gzipped scripts (`.sql.gz`, opened decompressed). */
const OPEN_FILTERS = [
  { name: "SQL", extensions: ["sql", "gz"] },
  { name: "All files", extensions: ["*"] },
];

/** Last path segment ("/a/b/report.sql" → "report.sql"). */
export function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

/** Appends `.sql` when the chosen name has no extension (`x.sql.gz` stays gzipped). */
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
    filters: OPEN_FILTERS,
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

/** A file read for an editor (`readSqlFileGuarded`). */
export interface SqlFileRead {
  text: string;
  /**
   * Only the start of a big file was read. The buffer must NOT be linked to the file (no
   * `filePath`, no auto-save): saving it would truncate the original.
   */
  readOnlyHead: boolean;
  /** For `readOnlyHead`: "Showing the first 5 MB of dump.sql; saving here will not overwrite the file". */
  headNotice: string | null;
  /** The encoding a file that was not UTF-8 was read as. */
  encoding: string | null;
}

/**
 * Reads an SQL file for an editor without freezing the app. A file above `LARGE_FILE_BYTES` (2 MB
 * when gzipped) is not loaded blindly: the user chooses its first 5 MB (read-only, never linked
 * to the file), the whole file, or - with a `connectionId` - the SQL import wizard, which streams
 * it (then nothing is loaded here and the result is null, as for Cancel). Anything smaller reads
 * at once, as it always did. A file that is not UTF-8 opens as Windows-1252 / UTF-16 and a toast
 * says so. Rejects when the file cannot be read.
 */
export async function readSqlFileGuarded(
  path: string,
  opts: { connectionId?: string | null } = {},
): Promise<SqlFileRead | null> {
  const info = await ipc.sqlFiles.info(path);
  let maxBytes: number | null = null;
  if (info.bytes > (info.compressed ? LARGE_GZ_BYTES : LARGE_FILE_BYTES)) {
    const choice = await askLargeFile({
      name: baseName(path),
      bytes: info.bytes,
      compressed: info.compressed,
      canImport: Boolean(opts.connectionId),
    });
    if (choice === "cancel") return null;
    if (choice === "import") {
      if (opts.connectionId)
        openImport({ connectionId: opts.connectionId, kind: "sql", table: null }, path);
      return null;
    }
    if (choice === "head") maxBytes = HEAD_BYTES;
  }
  const read = await ipc.sqlFiles.readHead(path, maxBytes);
  if (read.encoding)
    toast.message(`Opened as ${read.encoding}`, {
      description: `${baseName(path)} is not UTF-8. Saving writes it as UTF-8.`,
    });
  const readOnlyHead = maxBytes !== null && read.truncated;
  return {
    text: read.text,
    readOnlyHead,
    headNotice: readOnlyHead
      ? `Showing the first ${sizeLabel(HEAD_BYTES)} of ${baseName(path)}; saving here will not overwrite the file`
      : null,
    encoding: read.encoding,
  };
}

/** The title of a tab that holds the head of a file: "dump-head.sql" (not the file's own name). */
export function headTitle(path: string): string {
  return `${baseName(path).replace(/(\.sql)?(\.gz)?$/i, "")}-head.sql`;
}
