import { useEffect } from "react";
import { toast } from "sonner";
import { useSingleHost } from "@/features/export/singleHost";
import { useAction } from "@/features/palette/actions";
import { toIpcError, type TableRef } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { CsvImportWizard } from "./CsvImportWizard";
import { pickImportFile } from "./pickFile";
import { SqlImportDialog } from "./SqlImportDialog";
import { closeImport, openImport, setImportPath, useImportStore, type ImportKind } from "./store";

/** Name of the single-host slot `ImportHost` registers. */
const IMPORT_HOST = "import-wizard";

/**
 * The connection File ▸ Import works on (the active one) and, for a CSV import, the table of the
 * active table tab. Null (with a message) when there is nothing to import into.
 */
function importTarget(): { connectionId: string; table: TableRef | null } | null {
  const { activeId, byId } = useConnectionsStore.getState();
  const conn = activeId ? byId[activeId] : undefined;
  if (!activeId || !conn) {
    toast.message("Nothing to import into", { description: "Connect to a database first." });
    return null;
  }
  if (conn.info.readOnly) {
    toast.error("This connection is read-only", {
      description: "Imports write to the database.",
    });
    return null;
  }
  const { tabs, activeId: tabId } = useTabsStore.getState();
  const tab = tabs.find((t) => t.id === tabId);
  const table =
    tab && (tab.kind === "table" || tab.kind === "structure") && tab.connectionId === activeId
      ? tab.table
      : null;
  return { connectionId: activeId, table };
}

function importHere(kind: ImportKind): void {
  const target = importTarget();
  if (!target) return;
  openImport({
    connectionId: target.connectionId,
    kind,
    table: kind === "sql" ? null : target.table,
  });
}

const importCsv = () => importHere("csv");
const importJson = () => importHere("json");
const importSql = () => importHere("sql");

/** Sessions whose file panel is up (so a re-mounted picker does not open a second one). */
const picking = new Set<number>();

/** Asks for the file of the open request, then hands it to the store (or closes the request). */
function FilePicker({ kind, session }: { kind: ImportKind; session: number }) {
  useEffect(() => {
    if (picking.has(session)) return;
    picking.add(session);
    void pickImportFile(kind)
      .then((path) => {
        if (useImportStore.getState().session !== session) return;
        if (path) setImportPath(path);
        else closeImport();
      })
      .catch((e: unknown) => {
        toast.error("Could not open the file panel", { description: toIpcError(e).message });
        closeImport();
      })
      .finally(() => picking.delete(session));
  }, [kind, session]);
  return null;
}

/**
 * Mounts the import flows (CSV / JSON wizard, SQL dump) opened through `openImport`, and the
 * `import.csv` / `import.json` / `import.sql` actions of File ▸ Import. Mount once near the app
 * root.
 */
export function ImportHost() {
  const owner = useSingleHost(IMPORT_HOST);
  useAction("import.csv", owner ? importCsv : null);
  useAction("import.json", owner ? importJson : null);
  useAction("import.sql", owner ? importSql : null);
  const request = useImportStore((s) => s.request);
  const path = useImportStore((s) => s.path);
  const session = useImportStore((s) => s.session);
  if (!owner || !request) return null;
  if (path === null) return <FilePicker key={session} kind={request.kind} session={session} />;
  return request.kind !== "sql" ? (
    <CsvImportWizard key={session} request={request} path={path} />
  ) : (
    <SqlImportDialog key={session} request={request} path={path} />
  );
}
