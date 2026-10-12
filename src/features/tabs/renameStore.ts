import { create } from "zustand";

/** The tab whose title is being edited in place (Rename Tab…, or a double-click on a query tab). */
export const useTabRenameStore = create<{ renamingId: string | null }>()(() => ({
  renamingId: null,
}));

export function startTabRename(tabId: string): void {
  useTabRenameStore.setState({ renamingId: tabId });
}

export function stopTabRename(): void {
  useTabRenameStore.setState((s) => (s.renamingId === null ? s : { renamingId: null }));
}
