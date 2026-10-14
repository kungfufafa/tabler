/**
 * The customisable toolbar (right-click the toolbar ▸ Customize Toolbar…, Safari style): which
 * items it shows, in which order, and with or without their labels. Pure data and list edits:
 * the toolbar, its customisation sheet and the `toolbarItems` preference share them.
 */

export const TOOLBAR_ITEMS = [
  "sidebar",
  "actions",
  "safeMode",
  "databases",
  "status",
  "refresh",
  "openAnything",
  "workspaces",
  "users",
  "metrics",
  "panels",
  "space",
  "flexibleSpace",
] as const;
export type ToolbarItemId = (typeof TOOLBAR_ITEMS)[number];

/** Items the toolbar may hold more than once (the others at most once). */
export const REPEATABLE_ITEMS: ReadonlySet<ToolbarItemId> = new Set(["space", "flexibleSpace"]);

/** The palette's names, TablePlus's / Safari's wording. */
export const TOOLBAR_ITEM_LABEL: Record<ToolbarItemId, string> = {
  sidebar: "Toggle Sidebar",
  actions: "Action Control",
  safeMode: "Safe Mode",
  databases: "Databases & SQL",
  status: "Status",
  refresh: "Refresh",
  openAnything: "Open Anything",
  workspaces: "Show Workspaces",
  users: "User Management",
  metrics: "Metrics Board",
  panels: "Toggle Panels",
  space: "Space",
  flexibleSpace: "Flexible Space",
};

/** Short labels shown under / beside the icons with "Icon and Text". */
export const TOOLBAR_ITEM_SHORT: Record<ToolbarItemId, string> = {
  sidebar: "Sidebar",
  actions: "Changes",
  safeMode: "Safe Mode",
  databases: "Databases",
  status: "Status",
  refresh: "Refresh",
  openAnything: "Search",
  workspaces: "Workspaces",
  users: "Users",
  metrics: "Metrics",
  panels: "Panels",
  space: "",
  flexibleSpace: "",
};

/** The default set, left → right (TablePlus's toolbar). */
export const DEFAULT_TOOLBAR_ITEMS: readonly ToolbarItemId[] = Object.freeze([
  "sidebar",
  "actions",
  "safeMode",
  "databases",
  "status",
  "refresh",
  "openAnything",
  "workspaces",
  "users",
  "metrics",
  "panels",
]);

export const TOOLBAR_DISPLAYS = ["icon", "iconAndText"] as const;
export type ToolbarDisplay = (typeof TOOLBAR_DISPLAYS)[number];
export const TOOLBAR_DISPLAY_LABEL: Record<ToolbarDisplay, string> = {
  icon: "Icon Only",
  iconAndText: "Icon and Text",
};

/** Upper bound on the stored list (spaces are repeatable). */
export const MAX_TOOLBAR_ITEMS = 40;

export function isToolbarItemId(v: unknown): v is ToolbarItemId {
  return typeof v === "string" && (TOOLBAR_ITEMS as readonly string[]).includes(v);
}

/** A valid stored layout: known ids, the non-repeatable ones at most once. */
export function isToolbarItems(v: unknown): v is ToolbarItemId[] {
  if (!Array.isArray(v) || v.length > MAX_TOOLBAR_ITEMS) return false;
  const seen = new Set<string>();
  for (const id of v) {
    if (!isToolbarItemId(id)) return false;
    if (!REPEATABLE_ITEMS.has(id)) {
      if (seen.has(id)) return false;
      seen.add(id);
    }
  }
  return true;
}

/**
 * `items` with `id` inserted at `index` (clamped). A non-repeatable item already in the toolbar
 * moves there instead of appearing twice.
 */
export function insertItem(
  items: readonly ToolbarItemId[],
  id: ToolbarItemId,
  index: number,
): ToolbarItemId[] {
  const at = Math.max(0, Math.min(index, items.length));
  if (!REPEATABLE_ITEMS.has(id)) {
    const from = items.indexOf(id);
    if (from >= 0) return moveItem(items, from, at > from ? at - 1 : at);
  }
  if (items.length >= MAX_TOOLBAR_ITEMS) return [...items];
  return [...items.slice(0, at), id, ...items.slice(at)];
}

/** `items` with the item at `from` moved to `to` (both clamped). */
export function moveItem(
  items: readonly ToolbarItemId[],
  from: number,
  to: number,
): ToolbarItemId[] {
  const out = [...items];
  if (from < 0 || from >= out.length) return out;
  const [moved] = out.splice(from, 1);
  out.splice(Math.max(0, Math.min(to, out.length)), 0, moved!);
  return out;
}

export function removeItemAt(items: readonly ToolbarItemId[], index: number): ToolbarItemId[] {
  return items.filter((_, i) => i !== index);
}

/** What the palette offers: every item not in the toolbar yet, and the repeatable ones always. */
export function paletteItems(items: readonly ToolbarItemId[]): ToolbarItemId[] {
  return TOOLBAR_ITEMS.filter((id) => REPEATABLE_ITEMS.has(id) || !items.includes(id));
}

export interface ToolbarRows {
  /** Items left of the status pill (all of them without one). */
  left: ToolbarItemId[];
  /** The status pill is shown, centred between `left` and `right`. */
  status: boolean;
  right: ToolbarItemId[];
}

/**
 * Splits a layout at the status pill: the two sides grow equally, so the pill stays centred
 * while there is room (flexible spaces then push items towards it or away from it).
 */
export function splitAtStatus(items: readonly ToolbarItemId[]): ToolbarRows {
  const at = items.indexOf("status");
  if (at < 0) return { left: [...items], status: false, right: [] };
  return { left: items.slice(0, at), status: true, right: items.slice(at + 1) };
}

/** The items actually rendered: the Metrics Board only once something handles it. */
export function visibleItems(
  items: readonly ToolbarItemId[],
  available: { metrics: boolean },
): ToolbarItemId[] {
  return items.filter((id) => id !== "metrics" || available.metrics);
}
