import { useAction } from "@/features/palette/actions";
import { ExportDialog } from "./ExportDialog";
import { useSingleHost } from "./singleHost";
import { openExportTablesForActiveTab } from "./store";

/** Name of the single-host slot `ExportHost` registers. */
const EXPORT_HOST = "export-dialog";

const openFromShortcut = () => void openExportTablesForActiveTab();

/**
 * Mounts the export dialog and the global `export.open` (⌘⇧E, File ▸ Export…) handler, which opens
 * the multi-table export ("Export tables") with the active table tab's table checked. Mount once
 * near the app root; extra instances stay inert. Views with their own source (the SQL editor)
 * register a more specific `export.open` handler, or call `openExport` directly; the table tab's
 * Export buttons export that one table's rows (`openExportForActiveTab`).
 */
export function ExportHost() {
  const owner = useSingleHost(EXPORT_HOST);
  useAction("export.open", owner ? openFromShortcut : null);
  return owner ? <ExportDialog /> : null;
}
