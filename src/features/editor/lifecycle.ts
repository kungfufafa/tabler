import { ipc } from "@/lib/ipc";
import { getQueryTab, updateQueryTab, useQueryTabsStore } from "@/stores/queryTabs";
import { dropResults } from "@/stores/resultData";
import { useTabsStore } from "@/stores/tabs";
import { dropEditor, editorTabIds } from "./editorStates";
import { dropRememberedParams } from "./paramsPrompt";
import { baseTabId, dropSplit } from "./split";
import { dropSubmissions } from "./submissions";

/** Stops whatever the tab is executing (its tab went away). */
function stopRunning(tabId: string, connectionId: string): void {
  const s = getQueryTab(tabId);
  const id =
    s.run?.status === "running" ? s.run.id : s.plan?.status === "running" ? s.plan.id : null;
  if (id) void ipc.query.cancel(connectionId, id).catch(() => undefined);
}

/** Frees a tab's result buffers (and forgets the run that owned them). */
function releaseResults(tabId: string): void {
  const run = getQueryTab(tabId).run;
  if (!run) return;
  dropResults((id) => id.startsWith(`${run.id}:`));
  updateQueryTab(tabId, { run: null, plan: null, pane: "messages" });
}

let installed = false;

/**
 * Keeps per-tab editor resources in step with the tab strip: a closed query tab stops its query
 * and frees its result rows; its document is kept while the tab can still be reopened with ⇧⌘T, and
 * dropped once it leaves the closed-tabs list.
 */
export function installQueryTabLifecycle(): void {
  if (installed) return;
  installed = true;
  useTabsStore.subscribe((s, prev) => {
    if (s.tabs === prev.tabs && s.closed === prev.closed) return;
    const open = new Set(s.tabs.map((t) => t.id));
    for (const t of prev.tabs) {
      if (t.kind !== "query" || open.has(t.id)) continue;
      stopRunning(t.id, t.connectionId);
      releaseResults(t.id);
    }
    const keep = new Set<string>(open);
    for (const t of s.closed) if (t.kind === "query") keep.add(t.id);
    for (const id of Object.keys(useQueryTabsStore.getState().slices))
      if (!keep.has(id)) useQueryTabsStore.getState().remove(id);
    // A split pane (`tabId::split`) lives as long as its tab.
    for (const key of editorTabIds()) {
      const id = baseTabId(key);
      if (keep.has(id)) continue;
      dropEditor(key);
      if (key === id) {
        dropRememberedParams(id);
        dropSplit(id);
        dropSubmissions(id);
      }
    }
  });
}
