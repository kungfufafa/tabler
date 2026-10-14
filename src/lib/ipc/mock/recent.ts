import type { RecentEntry } from "../bindings/RecentEntry";
import type { RecentItems } from "../bindings/RecentItems";
import { mockError, registerMock } from "./transport";

/**
 * File ▸ Open Recent in the browser mock: the same list the backend keeps (ten of each, newest
 * first). The browser has no menu bar, so `recent_refresh_menu` only counts its calls.
 */
const MAX_RECENT = 10;
let items: RecentItems = { connections: [], files: [] };
let menuRebuilds = 0;

const copy = (r: RecentItems): RecentItems => ({
  connections: [...r.connections],
  files: [...r.files],
});

function front(list: string[], value: string): string[] {
  return [value, ...list.filter((v) => v !== value)].slice(0, MAX_RECENT);
}

registerMock("recent_list", () => copy(items));

registerMock("recent_add", ({ entry }) => {
  const e = entry as RecentEntry | undefined;
  if (e?.kind === "connection" && typeof e.profileId === "string" && e.profileId !== "") {
    items = { ...items, connections: front(items.connections, e.profileId) };
  } else if (e?.kind === "file" && typeof e.path === "string" && e.path.trim().startsWith("/")) {
    items = { ...items, files: front(items.files, e.path.trim()) };
  } else {
    mockError("INVALID_INPUT", "not a recent entry");
  }
  menuRebuilds++;
  return copy(items);
});

registerMock("recent_clear", () => {
  items = { connections: [], files: [] };
  menuRebuilds++;
  return null;
});

registerMock("recent_refresh_menu", () => {
  menuRebuilds++;
  return null;
});

/** Test helpers. */
export function mockRecentItems(): RecentItems {
  return copy(items);
}

export function mockRecentMenuRebuilds(): number {
  return menuRebuilds;
}

export function resetMockRecent(): void {
  items = { connections: [], files: [] };
  menuRebuilds = 0;
}
