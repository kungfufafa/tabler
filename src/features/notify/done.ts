import { toast } from "sonner";
import { create } from "zustand";
import { toIpcError } from "@/lib/ipc";
import { getPrefs } from "@/stores/prefs";
import { notifyInBackground, revealInFinder } from "./notify";

/** A finished export or backup, announced by `announceFileDone`. */
export interface DoneNotice {
  /** "Exported 1,234 rows", "Backed up tabler_dev". */
  title: string;
  /** "users_20261013-0930.csv · 12 KB · 0.4 s". */
  description: string;
  /** The file (or the first of the files) to show in Finder; null = nothing to show. */
  path: string | null;
  /** The popup's title, as TablePlus words it ("Export completed"). */
  popupTitle?: string;
}

interface DoneState {
  /** The "open containing folder" popup on screen; null when closed. */
  shown: DoneNotice | null;
  /** Kept while the popup animates out. */
  last: DoneNotice | null;
}

export const useDoneStore = create<DoneState>()(() => ({ shown: null, last: null }));

export function closeDonePopup(): void {
  useDoneStore.setState({ shown: null });
}

/** Shows `path` in Finder; a failure is a toast. */
export function revealFile(path: string): void {
  void revealInFinder(path).catch((e: unknown) =>
    toast.error("Could not show the file", { description: toIpcError(e).message }),
  );
}

/**
 * After an export or a backup wrote its file: the "open containing folder" popup when Settings ▸
 * "Show open containing folder popup after exporting data" is on (TablePlus's default), a toast
 * with "Show in Finder" otherwise. A native notification goes out too while Tabler is in the
 * background ("Show notifications").
 */
export function announceFileDone(notice: DoneNotice): void {
  void notifyInBackground({ title: notice.title, body: notice.description });
  const { path } = notice;
  if (path !== null && getPrefs().exportRevealPopup) {
    useDoneStore.setState({ shown: notice, last: notice });
    return;
  }
  toast.success(notice.title, {
    description: notice.description,
    action:
      path === null ? undefined : { label: "Show in Finder", onClick: () => revealFile(path) },
  });
}

/** Test helper. */
export function resetDoneStore(): void {
  useDoneStore.setState({ shown: null, last: null });
}
