import { create } from "zustand";
import { setSidebarPane, useSidebarPrefs, type SidebarPane } from "@/features/sidebar/sidebarPrefs";

/**
 * The sidebar's third pane, TablePlus's "Items | Queries | History". The items / history choice
 * stays in the sidebar preferences; whether Queries is showing on top of it is kept here.
 */
export type SidebarPaneId = SidebarPane | "queries";

export const QUERIES_PANE_KEY = "tabler.sidebar.queriesPane";

function readStored(): boolean {
  try {
    return globalThis.localStorage?.getItem(QUERIES_PANE_KEY) === "1";
  } catch {
    return false;
  }
}

function writeStored(open: boolean): void {
  try {
    if (open) globalThis.localStorage?.setItem(QUERIES_PANE_KEY, "1");
    else globalThis.localStorage?.removeItem(QUERIES_PANE_KEY);
  } catch {
    // Not persisted: the choice lasts until the window closes.
  }
}

const useQueriesPaneStore = create<{ open: boolean }>()(() => ({ open: readStored() }));

/** The pane the sidebar shows. */
export function useSidebarPaneId(): SidebarPaneId {
  const pane = useSidebarPrefs((s) => s.pane);
  const queries = useQueriesPaneStore((s) => s.open);
  return queries ? "queries" : pane;
}

export function showSidebarPane(pane: SidebarPaneId): void {
  const open = pane === "queries";
  if (useQueriesPaneStore.getState().open !== open) {
    useQueriesPaneStore.setState({ open });
    writeStored(open);
  }
  if (pane !== "queries") setSidebarPane(pane);
}

/** Test helper: reads the stored choice again. */
export function reloadQueriesPane(): void {
  useQueriesPaneStore.setState({ open: readStored() });
}
