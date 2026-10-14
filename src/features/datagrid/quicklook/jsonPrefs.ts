/**
 * The JSON viewers' "Sort keys" toggle, remembered per viewer (Quick Look, the Cell view, the
 * Details fields) in localStorage. Display only: nothing is staged by it. Storage can be missing or
 * throw (private window, blocked site data): the toggle then lasts for the session only.
 */
export type JsonViewer = "quicklook" | "cell" | "details";

/** localStorage key of a viewer's "Sort keys" toggle. */
export const sortKeysStorageKey = (viewer: JsonViewer) => `tabler.json.sortKeys.${viewer}`;

export function readSortKeys(viewer: JsonViewer): boolean {
  try {
    return localStorage.getItem(sortKeysStorageKey(viewer)) === "1";
  } catch {
    return false;
  }
}

export function writeSortKeys(viewer: JsonViewer, on: boolean): void {
  try {
    if (on) localStorage.setItem(sortKeysStorageKey(viewer), "1");
    else localStorage.removeItem(sortKeysStorageKey(viewer));
  } catch {
    // Not remembered; the toggle still works for this session.
  }
}
