/**
 * Navigate ▸ Select Next / Previous Pane (⇧⌘] / ⇧⌘[, TablePlus's Keymap ▸ Navigate): moves the
 * keyboard focus around the workspace's panes in reading order, skipping the hidden ones.
 * The panes are AppShell's resizable panels (`[data-panel]` with these ids).
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
 * Focuses the next (`dir` 1) or previous (-1) visible pane, wrapping around. From outside every
 * pane, "next" starts at the first one and "previous" at the last. Returns the pane focused.
 */
export function cyclePane(dir: 1 | -1, root: ParentNode = document): PaneId | null {
  const panes = visiblePanes(root);
  if (panes.length === 0) return null;
  const current = paneOf(document.activeElement, root);
  const at = current === null ? -1 : panes.findIndex((p) => p.id === current);
  const nextIndex =
    at < 0 ? (dir > 0 ? 0 : panes.length - 1) : (at + dir + panes.length) % panes.length;
  const next = panes[nextIndex]!;
  const target = focusTarget(next.el);
  if (target === next.el && !next.el.hasAttribute("tabindex")) next.el.tabIndex = -1;
  target.focus({ preventScroll: true });
  return next.id;
}
