import { isMock } from "@/lib/ipc";

/**
 * Native pickers of the Queries pane: Import… (`.sql` / `.sql.gz` files) and Add Folder to
 * Favorites… (a folder). The browser mock asks for paths instead.
 */

export async function pickQueryFiles(): Promise<string[]> {
  if (isMock) {
    const raw = window.prompt(
      "Import queries — file paths (comma-separated)",
      "/Users/me/queries/sample.sql, /Users/me/queries/nightly.sql.gz",
    );
    return (raw ?? "")
      .split(",")
      .map((p) => p.trim())
      .filter(Boolean);
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    title: "Import Queries",
    multiple: true,
    directory: false,
    filters: [
      { name: "SQL", extensions: ["sql", "gz"] },
      { name: "All files", extensions: ["*"] },
    ],
  });
  if (!picked) return [];
  return Array.isArray(picked) ? picked : [picked];
}

export async function pickFolder(title: string, suggested?: string): Promise<string | null> {
  if (isMock) {
    const path = window.prompt(`${title} — folder path`, suggested ?? "/Users/me/queries");
    return path?.trim() || null;
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    title,
    multiple: false,
    directory: true,
    defaultPath: suggested,
  });
  return typeof picked === "string" ? picked : null;
}
