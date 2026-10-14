/**
 * Navigate ▸ Select Next / Previous Pane (⇧⌘] / ⇧⌘[, TablePlus's Keymap ▸ Navigate): moves the
 * keyboard focus around the workspace's panes in reading order, skipping the hidden ones.
 * The panes are AppShell's resizable panels (`[data-panel]` with these ids); the content is
 * several stops when it is split: the two tab groups of a split workspace (`[data-tab-group]`)
 * and the two editors of a split SQL tab (`.cm-content` in `[data-pane]`).
 */
export const PANE_ORDER = ["sidebar", "content", "inspector", "console"] as const;
export type PaneId = (typeof PANE_ORDER)[number];

const panel = (root: ParentNode, id: PaneId) =>
  root.querySelector<HTMLElement>(`[data-panel][id="${id}"]`);

/** The panes on screen, in `PANE_ORDER`. */
export function visiblePanes(root: ParentNode = document): Array<{ id: PaneId; el: HTMLElement }> {
  return PANE_ORDER.flatMap((id) => {
    const el = panel(root, id);
    return el ? [{ id, el }] : [];
  });
}

/** The pane holding `el` (null outside the workspace's panes: toolbar, tab bar, dialogs…). */
export function paneOf(el: Element | null, root: ParentNode = document): PaneId | null {
  if (!el) return null;
  return visiblePanes(root).find((p) => p.el.contains(el))?.id ?? null;
}

const TABBABLE = [
  "input:not([disabled]):not([type='hidden'])",
  "textarea:not([disabled])",
  "select:not([disabled])",
  "button:not([disabled])",
  "a[href]",
  "[contenteditable='true']",
  "[tabindex]:not([tabindex='-1'])",
].join(", ");

/**
 * What takes the focus in a pane: the SQL editor, a data grid, the sidebar's selected item, else
 * its first focusable control; the pane itself when it has none.
 */
export function focusTarget(pane: HTMLElement): HTMLElement {
  return (
    pane.querySelector<HTMLElement>(".cm-content") ??
    pane.querySelector<HTMLElement>("[data-scope='grid']") ??
    pane.querySelector<HTMLElement>("[data-row-index][data-selected]") ??
    pane.querySelector<HTMLElement>("[data-row-index]") ??
    pane.querySelector<HTMLElement>(TABBABLE) ??
    pane
  );
}

/**
 * A place the pane cycle stops at. `zone` is what the stop belongs to: a pane, or one tab group of
 * a split workspace; `region` the part of it that counts as "being in" the stop; `target` what
 * takes the focus.
 */
export interface PaneStop {
  pane: PaneId;
  zone: HTMLElement;
  region: HTMLElement;
  target: HTMLElement;
}

/**
 * The elements of `list` that are not inside another one. A split workspace marks `data-tab-group`
 * on the group pane, on the tab bar inside it and on that bar's tab list: only the pane (the
 * outermost) is a place to stop at, the bar and the tabs are part of it.
 */
function outermost(list: Iterable<HTMLElement>): HTMLElement[] {
  const all = [...list];
  return all.filter((e) => !all.some((o) => o !== e && o.contains(e)));
}

/**
 * The stops of the panes on screen: the sidebar, then the content (a split workspace: its left
 * group, then its right one; inside a tab that shows several SQL editors, one stop per editor, in
 * the order they sit in the page: the main one, then the split one), then the row detail and the
 * console.
 */
export function paneStops(root: ParentNode = document): PaneStop[] {
  return visiblePanes(root).flatMap(({ id, el }): PaneStop[] => {
    if (id !== "content") return [{ pane: id, zone: el, region: el, target: focusTarget(el) }];
    const groups = outermost(el.querySelectorAll<HTMLElement>("[data-tab-group]"));
    return (groups.length > 0 ? groups : [el]).flatMap((zone): PaneStop[] => {
      const editors = [...zone.querySelectorAll<HTMLElement>(".cm-content")];
      if (editors.length < 2) return [{ pane: id, zone, region: zone, target: focusTarget(zone) }];
      return editors.map((target) => ({
        pane: id,
        zone,
        // a split editor pane is its own place: the editor and what sits in its pane
        region:
          target.closest<HTMLElement>("[data-pane]") ??
          target.closest<HTMLElement>(".cm-editor") ??
          target,
        target,
      }));
    });
  });
}

/**
 * The stop the focus is in: of the stops of the zone holding `el`, the one whose region does; else
 * (the results grid below the editors, a bar above them) the last one that starts before `el`,
 * else the first. -1 outside every zone.
 */
function stopOf(stops: readonly PaneStop[], el: Element | null): number {
  if (!el) return -1;
  const mine = stops.flatMap((s, i) => (s.zone.contains(el) ? [i] : []));
  if (mine.length === 0) return -1;
  const inside = mine.filter((i) => stops[i]!.region.contains(el));
  if (inside.length > 0) return inside[inside.length - 1]!;
  const before = mine.filter(
    (i) => stops[i]!.target.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING,
  );
  return before.length > 0 ? before[before.length - 1]! : mine[0]!;
}

/**
 * Focuses the next (`dir` 1) or previous (-1) stop, wrapping around: the panes in reading order,
 * the groups of a split workspace and the editors of a split SQL tab among them. From outside
 * every pane, "next" starts at the first one and "previous" at the last. Returns the pane focused.
 */
export function cyclePane(dir: 1 | -1, root: ParentNode = document): PaneId | null {
  const stops = paneStops(root);
  if (stops.length === 0) return null;
  const at = stopOf(stops, document.activeElement);
  const nextIndex =
    at < 0 ? (dir > 0 ? 0 : stops.length - 1) : (at + dir + stops.length) % stops.length;
  const next = stops[nextIndex]!;
  if (next.target === next.zone && !next.zone.hasAttribute("tabindex")) next.zone.tabIndex = -1;
  next.target.focus({ preventScroll: true });
  return next.pane;
}
