import { useAction } from "@/features/palette/actions";
import { ExportDialog } from "./ExportDialog";
import { useSingleHost } from "./singleHost";
import { openExportForActiveTab } from "./store";

/** Name of the single-host slot `ExportHost` registers. */
const EXPORT_HOST = "export-dialog";

const openFromShortcut = () => void openExportForActiveTab();

/**
 * Mounts the export dialog and the global `export.open` (⌘⇧E) handler, which exports the active
 * table tab. Mount once near the app root; extra instances stay inert. Views with their own source
 * (the SQL editor) register a more specific `export.open` handler, or call `openExport` directly.
 */
export function ExportHost() {
  const owner = useSingleHost(EXPORT_HOST);
  useAction("export.open", owner ? openFromShortcut : null);
  return owner ? <ExportDialog /> : null;
}
