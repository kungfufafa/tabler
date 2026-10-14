import { useCallback, useEffect, useState } from "react";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { LargeFileDialog } from "@/features/editor/LargeFileDialog";
import { actions, useAction } from "@/features/palette/actions";
import { useConnectionsStore } from "@/stores/connections";
import { registerUnsavedSource, registerWindowWork, useTabsStore } from "@/stores/tabs";
import { focusViewer } from "./buffers";
import { loadViewerFile, pickViewerFile, saveViewer } from "./files";
import { viewerTitle } from "./labels";
import {
  closeStandalone,
  dropClosedViewers,
  standaloneUnsavedWork,
  viewerUnsavedCount,
} from "./lifecycle";
import { openStandaloneViewer } from "./open";
import { SqlViewer, ViewerNotice } from "./SqlViewer";
import "./SqlViewerTab";
import { STANDALONE, getDoc, patchDoc, useSqlViewerStore } from "./store";

/**
 * The SQL Viewer with no connection open: the window's own viewer, over the welcome screen. ⌘W
 * and ⎋ close it, asking first when it holds unsaved changes.
 */
function StandaloneViewer() {
  const [confirmClose, setConfirmClose] = useState(false);
  const doc = useSqlViewerStore((s) => s.docs[STANDALONE]);
  const requestClose = useCallback(() => {
    if (getDoc(STANDALONE)?.dirty) setConfirmClose(true);
    else closeStandalone();
  }, []);
  // ⌘W (allowed in dialogs) closes the viewer rather than a tab behind it.
  useAction("tab.close", requestClose);

  const saveAndClose = async () => {
    if (await saveViewer(STANDALONE)) closeStandalone();
  };
  const notice = confirmClose ? (
    <ViewerNotice
      message={`Save the changes to ${viewerTitle(doc)} before closing?`}
      actions={[
        { label: "Cancel", run: () => (setConfirmClose(false), focusViewer(STANDALONE)) },
        { label: "Don't Save", run: closeStandalone },
        { label: "Save", run: () => void saveAndClose(), primary: true },
      ]}
    />
  ) : null;

  return (
    <Dialog open onOpenChange={(open) => !open && requestClose()}>
      <DialogContent
        aria-label="SQL Viewer"
        className="flex h-[86vh] max-h-[86vh] w-[92vw] max-w-[92vw] flex-col gap-0 overflow-hidden p-0 sm:max-w-[92vw]"
        showCloseButton={false}
        // The editor takes the focus itself once it mounts.
        onOpenAutoFocus={(e) => e.preventDefault()}
      >
        <DialogTitle className="sr-only">SQL Viewer</DialogTitle>
        <DialogDescription className="sr-only">
          A SQL file viewer that is not connected to a database.
        </DialogDescription>
        {doc && <SqlViewer viewerKey={STANDALONE} active notice={notice} />}
      </DialogContent>
    </Dialog>
  );
}

/**
 * The SQL Viewer's app-wide part (mount once): the viewer tab kind, the standalone viewer, the
 * large-file question (`LargeFileDialog`, asked by every way to open an .sql file), the
 * unsaved-changes count of viewer tabs and of the standalone viewer (the window's close / quit
 * guard asks about both), and File ▸ Open SQL File… with no connection open (the file opens in
 * the viewer rather than nowhere).
 */
export function SqlViewerHost() {
  const open = useSqlViewerStore((s) => s.standaloneOpen);
  const connected = useConnectionsStore((s) => s.activeId !== null);

  useEffect(() => registerUnsavedSource(viewerUnsavedCount), []);
  useEffect(() => registerWindowWork(standaloneUnsavedWork, useSqlViewerStore.subscribe), []);
  useEffect(
    () =>
      useTabsStore.subscribe((s, prev) => {
        if (s.tabs === prev.tabs) return;
        dropClosedViewers(new Set(s.tabs.map((t) => t.id)));
      }),
    [],
  );
  useEffect(() => {
    if (connected) return;
    return actions.registerFallback("query.open", async () => {
      const path = await pickViewerFile();
      if (!path) return;
      openStandaloneViewer(null);
      const doc = getDoc(STANDALONE);
      if (doc?.dirty) patchDoc(STANDALONE, { pendingPath: path });
      else await loadViewerFile(STANDALONE, path);
    });
  }, [connected]);

  return (
    <>
      {/* The "this file is big" question of every way to open an .sql file (editor, viewer, links). */}
      <LargeFileDialog />
      {open ? <StandaloneViewer /> : null}
    </>
  );
}
