import { isMock } from "@/lib/ipc";

/** A file type the open panel offers ("SQLite": sqlite, db…). */
export interface FileFilter {
  name: string;
  extensions: string[];
}

/** Asks for an existing file (the open panel; a prompt in the browser mock). */
export async function pickFile(
  title: string,
  filters: readonly FileFilter[] = [],
): Promise<string | null> {
  if (isMock) return window.prompt(title, "");
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    multiple: false,
    directory: false,
    title,
    filters: filters.length > 0 ? [...filters] : undefined,
  });
  return typeof picked === "string" ? picked : null;
}

/** The last part of a path, as a file button shows it. */
export const baseName = (path: string) => path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
