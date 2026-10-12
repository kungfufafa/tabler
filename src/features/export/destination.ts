import { isMock } from "@/lib/ipc";

/** Where the browser mock pretends exports land. */
export const MOCK_EXPORT_DIR = "/Users/me/Downloads";

/**
 * Asks where to save the export: the native save panel in Tauri, `window.prompt` in the browser
 * mock. `lastDir` (the folder of the previous export) seeds the panel. Null = cancelled.
 */
export async function pickExportPath(
  fileName: string,
  filter: { name: string; extensions: string[] },
  lastDir: string | null,
): Promise<string | null> {
  if (isMock) {
    const answer = window.prompt("Save export as", `${lastDir ?? MOCK_EXPORT_DIR}/${fileName}`);
    const path = answer?.trim();
    return path ? path : null;
  }
  const { save } = await import("@tauri-apps/plugin-dialog");
  const picked = await save({
    title: "Export",
    defaultPath: lastDir ? `${lastDir}/${fileName}` : fileName,
    filters: [filter],
    canCreateDirectories: true,
  });
  return picked ?? null;
}

/**
 * Asks for the folder a multi-table export writes its files into: the native folder panel in
 * Tauri, `window.prompt` in the browser mock. Null = cancelled.
 */
export async function pickExportFolder(lastDir: string | null): Promise<string | null> {
  if (isMock) {
    const answer = window.prompt("Export the tables into folder", lastDir ?? MOCK_EXPORT_DIR);
    const path = answer?.trim().replace(/\/+$/, "");
    return path ? path : null;
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    title: "Export Tables To",
    directory: true,
    multiple: false,
    canCreateDirectories: true,
    defaultPath: lastDir ?? undefined,
  });
  return typeof picked === "string" ? picked : null;
}

/** Shows the exported file in Finder (no-op in the browser mock). */
export async function revealExport(path: string): Promise<void> {
  if (isMock) return;
  const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
  await revealItemInDir(path);
}
