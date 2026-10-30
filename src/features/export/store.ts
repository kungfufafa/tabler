import { toast } from "sonner";
import { create } from "zustand";
import { allowFeature } from "@/lib/caps";
import {
  ipc,
  toIpcError,
  type ExportRequest,
  type ExportResult,
  type ExportResultsRequest,
  type ExportResultsResult,
  type ExportTablesRequest,
  type ExportTablesResult,
  type ResultColumn,
  type TableRef,
  type Value,
} from "@/lib/ipc";
import { announceFileDone } from "@/features/notify/done";
import { notifyInBackground } from "@/features/notify/notify";
import { useConnectionsStore } from "@/stores/connections";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore } from "@/stores/tabs";
import {
  basename,
  formatBytes,
  formatCount,
  formatDuration,
  type ExportOptions,
  type ResultsScope,
  type RowScope,
} from "./options";

/**
 * What to export: a table (with the tab's filter / sort / page when `tabId` is given), SQL, or
 * rows the renderer already holds (a result shown in the SQL editor).
 */
export type ExportSourceSpec =
  | {
      kind: "table";
      table: TableRef;
      /** Table tab whose filter, sort, visible columns and page apply; null = the whole table. */
      tabId?: string | null;
    }
  | { kind: "sql"; sql: string }
  | {
      kind: "rows";
      columns: ResultColumn[];
      rows: Value[][];
      /** Shown as the dialog's title, e.g. "Result 2". */
      label: string;
      /** Default INSERT target (the result's source table, when the server reported one). */
      table: string | null;
      /**
       * The statement that produced the rows, to export all of them by running it again (when
       * the rows held are not the whole result, or a binary value was cut to its preview).
       */
      sql: string | null;
      /** The row cap cut the result: more rows exist than the ones held. */
      truncated: boolean;
      /**
       * The statement did not finish: it was cancelled, or failed after some rows arrived. The
       * rows held are only those that arrived before it stopped. Null when it completed.
       */
      stopped: "cancelled" | "error" | null;
    }
  | {
      /**
       * Several tables in full, chosen in the dialog from every database of the connection (the
       * current one open, with the tables of `schema` checked).
       */
      kind: "tables";
      schema: string | null;
      /** Checked when the dialog opens; with `all`, every table of the schema is. */
      tables: TableRef[];
      all?: boolean;
    };

export interface OpenExportArgs {
  connectionId: string;
  source: ExportSourceSpec;
}

export interface ExportRun {
  exportId: string;
  connectionId: string;
  path: string;
  startedAt: number;
  /** Rows the export is expected to write (for the progress bar); null = unknown. */
  expectedRows: number | null;
  rows: number;
  bytes: number;
  cancelling: boolean;
  /** Multi-table exports: the table being written (`database` when several are exported). */
  table?: { index: number; count: number; name: string; rows: number; database?: string };
}

/** The dialog's choices, kept for the session so the form reopens as it was (e.g. mid-export). */
export interface ExportDraft {
  options: ExportOptions;
  scope: RowScope;
  visibleOnly: boolean;
  tableName: string;
  batchText: string;
  /** A cut-off editor result: export all its rows by running its statement again. */
  allRows: boolean;
  /** A result of a run holding several: this one, or every result. */
  resultsScope?: ResultsScope;
  /**
   * The user picked the file encoding in the form: it then no longer follows the connection's
   * View Using Encoding. Absent in a draft written from outside the form (a pre-set dialog such
   * as Export Current Page…), which still follows it.
   */
  encodingTouched?: boolean;
  /** The remembered encoding while the form shows the connection's one instead (`options.encoding`). */
  rememberedEncoding?: ExportOptions["encoding"];
}

interface ExportState {
  open: boolean;
  /** Kept while the dialog closes so its content does not change during the exit animation. */
  target: OpenExportArgs | null;
  /** Bumped on every open so the form starts from the remembered options again. */
  session: number;
  /** The form's state in this session; null until the form first renders. */
  draft: ExportDraft | null;
  run: ExportRun | null;
  /** Failure of the last run, shown inline in the dialog. */
  error: string | null;
}

export const useExportStore = create<ExportState>()(() => ({
  open: false,
  target: null,
  session: 0,
  draft: null,
  run: null,
  error: null,
}));

/**
 * Opens the export dialog. Public entry point, e.g. for the SQL editor:
 * `openExport({ connectionId, source: { kind: "sql", sql } })`. While an export runs, the dialog
 * reopens on its progress instead.
 */
export function openExport(args: OpenExportArgs): void {
  // Every export reads through the session's export (rows held by a result included).
  if (!useExportStore.getState().run && !allowFeature(args.connectionId, "exportTables")) return;
  useExportStore.setState((s) =>
    s.run
      ? { open: true }
      : { open: true, target: args, session: s.session + 1, draft: null, error: null },
  );
}

/** Closes the dialog; a running export keeps going and reports through a toast. */
export function closeExport(): void {
  useExportStore.setState({ open: false });
}

/**
 * Lets go of the closed dialog's source once nothing shows it any more (after the exit
 * animation, or when an export the closed dialog started ends): a shown result's rows are a
 * copy of up to the editor's row cap, which would otherwise stay reachable until the next export.
 */
export function releaseExportTarget(): void {
  useExportStore.setState((s) =>
    s.open || s.run || !s.target ? s : { target: null, draft: null },
  );
}

/**
 * Exports several tables at once (sidebar "Export…" on a selection, File ▸ Export… without a
 * table): the dialog lists the schema's tables with these checked.
 */
export function openExportTables(connectionId: string, tables: TableRef[]): void {
  const schema =
    tables[0]?.schema ?? useConnectionsStore.getState().byId[connectionId]?.currentSchema ?? null;
  openExport({ connectionId, source: { kind: "tables", schema, tables } });
}

/**
 * Opens the multi-table export with every table of the connection's current schema checked.
 * Returns false when no connection is active.
 */
function openExportCurrentSchema(): boolean {
  const { activeId: connectionId, byId } = useConnectionsStore.getState();
  if (connectionId && byId[connectionId]) {
    openExport({
      connectionId,
      source: {
        kind: "tables",
        schema: byId[connectionId].currentSchema ?? null,
        tables: [],
        all: true,
      },
    });
    return true;
  }
  toast.message("Nothing to export here", {
    description: "Connect to a database, or export a query from the SQL editor.",
  });
  return false;
}

/**
 * The table and structure tabs' Export buttons: exports the active table tab (its filter, sort
 * and page), or the table behind a structure tab. Anywhere else on a connection it opens the
 * multi-table export with every table of the current schema checked. Returns false when there
 * is nothing to export.
 */
export function openExportForActiveTab(): boolean {
  const { tabs, activeId } = useTabsStore.getState();
  const tab = tabs.find((t) => t.id === activeId);
  if (tab && (tab.kind === "table" || tab.kind === "structure")) {
    // The tab's filter, sort and page apply in both views, once its data view has been shown.
    const hasRows = useTableTabsStore.getState().slices[tab.id] !== undefined;
    openExport({
      connectionId: tab.connectionId,
      source: { kind: "table", table: tab.table, tabId: hasRows ? tab.id : null },
    });
    return true;
  }
  return openExportCurrentSchema();
}

/**
 * `export.open` (⌘⇧E, File ▸ Export…) outside the SQL editor: the multi-table export ("Export
 * tables"). With a table or structure tab active its table is checked and its schema selected;
 * anywhere else on a connection every table of the current schema is. The filtered export of one
 * table is the table tab's own Export button (`openExportForActiveTab`). Returns false when there
 * is nothing to export.
 */
export function openExportTablesForActiveTab(): boolean {
  const { tabs, activeId } = useTabsStore.getState();
  const tab = tabs.find((t) => t.id === activeId);
  if (tab && (tab.kind === "table" || tab.kind === "structure")) {
    openExport({
      connectionId: tab.connectionId,
      source: { kind: "tables", schema: tab.table.schema ?? null, tables: [tab.table] },
    });
    return true;
  }
  return openExportCurrentSchema();
}

function rowsLabel(n: number): string {
  return `${formatCount(n)} row${n === 1 ? "" : "s"}`;
}

/** " · 3 characters written as ?" when the encoding could not hold some. */
function replacedLabel(n: number | undefined): string {
  return n ? ` · ${formatCount(n)} character${n === 1 ? "" : "s"} written as ?` : "";
}

/** A failure: inline while the dialog is open, a toast otherwise; a notification in background. */
function reportFailure(message: string, open: boolean): void {
  void notifyInBackground({ title: "Export failed", body: message });
  if (open) useExportStore.setState({ error: message });
  else toast.error("Export failed", { description: message });
}

/** Saves the form's state for the current session (see `ExportDraft`). */
export function setExportDraft(draft: ExportDraft): void {
  useExportStore.setState({ draft });
}

/**
 * Streams `req` to its file, tracking progress in the store. Success closes the dialog and shows
 * the "open containing folder" popup (or a toast, see `announceFileDone`); a cancel is reported
 * neutrally; a failure shows inline while the dialog is open (a toast otherwise). Resolves with
 * the result, or null when it did not complete.
 */
export async function runExport(
  connectionId: string,
  req: ExportRequest,
  {
    expectedRows = null,
    now = Date.now,
  }: { expectedRows?: number | null; now?: () => number } = {},
): Promise<ExportResult | null> {
  if (useExportStore.getState().run) return null;
  useExportStore.setState({
    error: null,
    run: {
      exportId: req.exportId,
      connectionId,
      path: req.path,
      startedAt: now(),
      expectedRows,
      rows: 0,
      bytes: 0,
      cancelling: false,
    },
  });
  try {
    const result = await ipc.exports.run(connectionId, req, (p) =>
      useExportStore.setState((s) =>
        s.run?.exportId === req.exportId ? { run: { ...s.run, rows: p.rows, bytes: p.bytes } } : s,
      ),
    );
    const wasOpen = useExportStore.getState().open;
    useExportStore.setState({ run: null, open: false });
    // An open dialog lets go of its source once it has closed (`ExportDialog`); a closed one now.
    if (!wasOpen) releaseExportTarget();
    announceFileDone({
      title: `Exported ${rowsLabel(result.rows)}`,
      popupTitle: "Export completed",
      description: `${basename(result.path)} · ${formatBytes(result.bytes)} · ${formatDuration(result.durationMs)}${replacedLabel(result.replaced)}`,
      path: result.path,
    });
    return result;
  } catch (err) {
    const e = toIpcError(err);
    const open = useExportStore.getState().open;
    useExportStore.setState({ run: null });
    if (!open) releaseExportTarget();
    if (e.code === "CANCELLED") {
      toast.message("Export cancelled", { description: "No file was written." });
    } else {
      reportFailure(e.message, open);
    }
    return null;
  }
}

function tablesLabel(n: number): string {
  return `${formatCount(n)} table${n === 1 ? "" : "s"}`;
}

/** The database of a multi-database group (for the progress line and the result). */
export interface TablesGroupLabel {
  /** Shown as "database · table" while several databases are exported. */
  database: string | null;
}

/**
 * Exports several tables (see `runExport`): progress per table in the store; success closes the
 * dialog and announces the (first) file. Several requests (one per database of a
 * multi-database export) run one after the other as one export: one progress, one cancel; the
 * files of the databases done before a failure or a cancel stay.
 */
export async function runExportTables(
  connectionId: string,
  reqs: ExportTablesRequest | ExportTablesRequest[],
  { now = Date.now, labels = [] }: { now?: () => number; labels?: TablesGroupLabel[] } = {},
): Promise<ExportTablesResult | null> {
  const list = Array.isArray(reqs) ? reqs : [reqs];
  const first = list[0];
  if (useExportStore.getState().run || !first) return null;
  const count = list.reduce((n, r) => n + r.tables.length, 0);
  const several = list.length > 1;
  const databaseOf = (i: number) => (several ? (labels[i]?.database ?? undefined) : undefined);
  useExportStore.setState({
    error: null,
    run: {
      exportId: first.exportId,
      connectionId,
      path: first.target,
      startedAt: now(),
      expectedRows: null,
      rows: 0,
      bytes: 0,
      cancelling: false,
      table: {
        index: 0,
        count,
        name: first.tables[0]?.name ?? "",
        rows: 0,
        database: databaseOf(0),
      },
    },
  });
  const results: ExportTablesResult[] = [];
  let offset = 0;
  let rowsBefore = 0;
  let bytesBefore = 0;
  try {
    for (const [i, req] of list.entries()) {
      if (i > 0) {
        // A cancel asked for between two databases stops before the next one starts.
        if (useExportStore.getState().run?.cancelling)
          throw { code: "CANCELLED", message: "export cancelled" };
        useExportStore.setState((s) =>
          s.run ? { run: { ...s.run, exportId: req.exportId, path: req.target } } : s,
        );
      }
      const base = { offset, rows: rowsBefore, bytes: bytesBefore };
      const result = await ipc.exports.runTables(connectionId, req, (p) =>
        useExportStore.setState((s) =>
          s.run?.exportId === req.exportId
            ? {
                run: {
                  ...s.run,
                  rows: base.rows + p.rows,
                  bytes: base.bytes + p.bytes,
                  table: {
                    index: base.offset + p.tableIndex,
                    count,
                    name: p.table,
                    rows: p.tableRows,
                    database: databaseOf(i),
                  },
                },
              }
            : s,
        ),
      );
      results.push(result);
      offset += req.tables.length;
      rowsBefore += result.rows;
      bytesBefore += result.bytes;
    }
    const wasOpen = useExportStore.getState().open;
    useExportStore.setState({ run: null, open: false });
    if (!wasOpen) releaseExportTarget();
    const tables = results.reduce((n, r) => n + r.tables.length, 0);
    const paths = results.flatMap((r) => r.paths);
    const duration = results.reduce((n, r) => n + r.durationMs, 0);
    const replaced = results.reduce((n, r) => n + (r.replaced ?? 0), 0);
    const firstPath = paths[0] ?? first.target;
    const where = paths.length === 1 ? basename(firstPath) : basename(first.target);
    const databases = several ? ` from ${formatCount(list.length)} databases` : "";
    announceFileDone({
      title: `Exported ${tablesLabel(tables)}${databases}`,
      popupTitle: "Export completed",
      description: `${where} · ${rowsLabel(rowsBefore)} · ${formatBytes(bytesBefore)} · ${formatDuration(duration)}${replacedLabel(replaced)}`,
      path: firstPath,
    });
    return results.length === 1
      ? results[0]!
      : {
          tables: results.flatMap((r) => r.tables),
          paths,
          rows: rowsBefore,
          bytes: bytesBefore,
          durationMs: duration,
          ...(replaced > 0 ? { replaced } : {}),
        };
  } catch (err) {
    const e = toIpcError(err);
    const open = useExportStore.getState().open;
    useExportStore.setState({ run: null });
    if (!open) releaseExportTarget();
    const kept =
      results.length > 0
        ? ` The files of the ${formatCount(results.length)} database${results.length === 1 ? "" : "s"} exported before stay.`
        : "";
    if (e.code === "CANCELLED") {
      toast.message("Export cancelled", {
        description: kept ? kept.trim() : "No file was written.",
      });
    } else {
      reportFailure(`${e.message}${kept}`, open);
    }
    return null;
  }
}

/**
 * Exports every result of a run (see `runExport`): progress per result in the store; success
 * closes the dialog and announces the (first) file.
 */
export async function runExportResults(
  connectionId: string,
  req: ExportResultsRequest,
  { now = Date.now }: { now?: () => number } = {},
): Promise<ExportResultsResult | null> {
  if (useExportStore.getState().run) return null;
  useExportStore.setState({
    error: null,
    run: {
      exportId: req.exportId,
      connectionId,
      path: req.target,
      startedAt: now(),
      expectedRows: req.results.reduce((n, r) => n + r.rows.length, 0),
      rows: 0,
      bytes: 0,
      cancelling: false,
      table: { index: 0, count: req.results.length, name: req.results[0]?.label ?? "", rows: 0 },
    },
  });
  try {
    const result = await ipc.exports.runResults(connectionId, req, (p) =>
      useExportStore.setState((s) =>
        s.run?.exportId === req.exportId
          ? {
              run: {
                ...s.run,
                rows: p.rows,
                bytes: p.bytes,
                table: {
                  index: p.tableIndex,
                  count: p.tableCount,
                  name: p.table,
                  rows: p.tableRows,
                },
              },
            }
          : s,
      ),
    );
    const wasOpen = useExportStore.getState().open;
    useExportStore.setState({ run: null, open: false });
    if (!wasOpen) releaseExportTarget();
    const firstPath = result.paths[0] ?? req.target;
    const where = result.paths.length === 1 ? basename(firstPath) : basename(req.target);
    announceFileDone({
      title: `Exported ${formatCount(result.results.length)} result${result.results.length === 1 ? "" : "s"}`,
      popupTitle: "Export completed",
      description: `${where} · ${rowsLabel(result.rows)} · ${formatBytes(result.bytes)} · ${formatDuration(result.durationMs)}${replacedLabel(result.replaced)}`,
      path: firstPath,
    });
    return result;
  } catch (err) {
    const e = toIpcError(err);
    const open = useExportStore.getState().open;
    useExportStore.setState({ run: null });
    if (!open) releaseExportTarget();
    if (e.code === "CANCELLED") {
      toast.message("Export cancelled", { description: "No file was written." });
    } else {
      reportFailure(e.message, open);
    }
    return null;
  }
}

/** Asks the backend to stop the running export (its partial file is removed). */
export async function cancelExport(): Promise<void> {
  const run = useExportStore.getState().run;
  if (!run || run.cancelling) return;
  useExportStore.setState({ run: { ...run, cancelling: true } });
  try {
    await ipc.exports.cancel(run.exportId);
  } catch (err) {
    useExportStore.setState((s) =>
      s.run?.exportId === run.exportId ? { run: { ...s.run, cancelling: false } } : s,
    );
    toast.error("Could not cancel the export", { description: toIpcError(err).message });
  }
}

/** Test helper. */
export function resetExportStore(): void {
  useExportStore.setState({
    open: false,
    target: null,
    session: 0,
    draft: null,
    run: null,
    error: null,
  });
}
