/** Focusable main surface of a tab: the data grid, or the SQL editor's content. */
const TAB_SURFACE = '[data-scope="grid"], .cm-content';
/** How long to wait for a newly activated tab's surface to mount (its metadata may be loading). */
const SURFACE_WAIT_MS = 3000;
const SURFACE_POLL_MS = 40;

function findTabSurface(exclude: Element | null): HTMLElement | null {
  for (const el of document.querySelectorAll<HTMLElement>(TAB_SURFACE)) {
    if (el === exclude) continue;
    if (!el.closest("[role='dialog'], [role='alertdialog'], [data-scope='sidebar']")) return el;
  }
  return null;
}

/**
 * Moves focus into the newly activated tab's grid/editor once it has mounted. Only the active
 * tab's content is mounted, so switching tabs unmounts whatever had focus and leaves focus on
 * <body>, where grid shortcuts and arrow keys do nothing. Call it right after the switch: the
 * element focused at that moment belongs to the outgoing tab and is never picked. Gives up as soon
 * as focus moves anywhere else, or after a few seconds.
 */
export function focusTabSurface(): void {
  const before = document.activeElement;
  const deadline = Date.now() + SURFACE_WAIT_MS;
  const tick = () => {
    const current = document.activeElement;
    // Focus moved on (a click, the command itself, the palette reopening): leave it there.
    if (current && current !== document.body && current !== before) return;
    const surface = findTabSurface(before);
    if (surface) surface.focus({ preventScroll: true });
    else if (Date.now() < deadline) window.setTimeout(tick, SURFACE_POLL_MS);
  };
  // The switch renders after the current task; look once it has.
  window.setTimeout(tick, 0);
}
