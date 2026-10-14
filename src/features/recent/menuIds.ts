/**
 * Menu item ids of File ▸ Open Recent's entries (src-tauri/src/recent.rs): the backend sends the
 * chosen one to the focused window as a `menu:action` payload.
 */
export const RECENT_CONNECTION_PREFIX = "recent.connection:";
export const RECENT_FILE_PREFIX = "recent.file:";

export type RecentPick = { kind: "connection"; profileId: string } | { kind: "file"; path: string };

/** The entry an Open Recent menu item id names; null for any other id. */
export function parseRecentMenuId(id: unknown): RecentPick | null {
  if (typeof id !== "string") return null;
  if (id.startsWith(RECENT_CONNECTION_PREFIX)) {
    const profileId = id.slice(RECENT_CONNECTION_PREFIX.length);
    return profileId ? { kind: "connection", profileId } : null;
  }
  if (id.startsWith(RECENT_FILE_PREFIX)) {
    const path = id.slice(RECENT_FILE_PREFIX.length);
    return path ? { kind: "file", path } : null;
  }
  return null;
}
