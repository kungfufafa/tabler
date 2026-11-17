import { useEffect } from "react";
import { useAction } from "@/features/palette/actions";
import { ExportDialog } from "./ExportDialog";
import { createJobToast } from "./jobToast";
import { useSingleHost } from "./singleHost";
import {
  exportProgressToast,
  openExportTablesForActiveTab,
  reopenExport,
  useExportStore,
} from "./store";

/** Name of the single-host slot `ExportHost` registers. */
const EXPORT_HOST = "export-dialog";

/** Toast id of the progress of an export whose dialog was closed. */
const EXPORT_PROGRESS_TOAST = "export-progress";

const openFromShortcut = () => void openExportTablesForActiveTab();

/**
 * Mounts the export dialog and the global `export.open` (⌘⇧E, File ▸ Export…) handler, which opens
 * the multi-table export ("Export tables") with the active table tab's table checked. Mount once
 * near the app root; extra instances stay inert. Views with their own source (the SQL editor)
 * register a more specific `export.open` handler, or call `openExport` directly; the table tab's
 * Export buttons export that one table's rows (`openExportForActiveTab`). While an export runs
 * with the dialog closed, a toast shows its progress, with "Show" to bring the dialog back.
 */
export function ExportHost() {
  const owner = useSingleHost(EXPORT_HOST);
  useAction("export.open", owner ? openFromShortcut : null);
  useEffect(() => {
    if (!owner) return;
    const job = createJobToast({ id: EXPORT_PROGRESS_TOAST, onShow: reopenExport });
    job.sync(exportProgressToast(useExportStore.getState()));
    const off = useExportStore.subscribe((s) => job.sync(exportProgressToast(s)));
    return () => {
      off();
      job.sync(null);
    };
  }, [owner]);
  return owner ? <ExportDialog /> : null;
}
