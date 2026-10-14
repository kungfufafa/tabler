import { applyPreviewedResultEdits, commitResultEdits } from "@/features/results/commit";
import { pendingForTab, useResultsStore } from "@/features/results/store";
import { getPrefs } from "@/stores/prefs";
import { reportTabCommit } from "./commitChain";

/**
 * The commit paths of a query tab's result edits, reporting their outcome to the commit chain
 * (`reportTabCommit`): ⌘S, and the Apply / dismissal of the result's Preview SQL sheet.
 */

/** ⌘S on staged result edits. With "Confirm before commit" it opens the sheet, which reports. */
export async function commitResultEditsReported(tabId: string): Promise<boolean> {
  const viaSheet = getPrefs().confirmCommit;
  const ok = await commitResultEdits(tabId);
  if (ok) reportTabCommit(tabId, "committed");
  else if (!viaSheet) reportTabCommit(tabId, "failed");
  return ok;
}

/**
 * The sheet's Apply (⌘↩): committed when the tab's staged edits are gone and the sheet closed;
 * a failure or the safe mode's refusal keeps the sheet open.
 */
export async function applyPreviewedResultEditsReported(tabId: string): Promise<void> {
  const before = useResultsStore.getState();
  const { preview } = before;
  // Nothing starts (already applying, or another tab's sheet): nothing to report.
  if (preview.tabId !== tabId || preview.applying || preview.loading)
    return applyPreviewedResultEdits();
  const had = pendingForTab(before, tabId);
  await applyPreviewedResultEdits();
  const after = useResultsStore.getState();
  const open = after.preview.open && after.preview.tabId === tabId;
  if (had > 0 && !open && pendingForTab(after, tabId) === 0) reportTabCommit(tabId, "committed");
  else reportTabCommit(tabId, open && !after.preview.error ? "cancelled" : "failed");
}
