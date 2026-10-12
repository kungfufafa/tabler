import { toast } from "sonner";
import { create } from "zustand";
import {
  ipc,
  toIpcError,
  type ExportRequest,
  type ExportResult,
  type ResultColumn,
  type TableRef,
  type Value,
} from "@/lib/ipc";
import { useTabsStore } from "@/stores/tabs";
import { revealExport } from "./destination";
import {
  basename,
  formatBytes,
  formatCount,
  formatDuration,
  type ExportOptions,
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
 * `export.open` (⌘⇧E) outside the SQL editor: exports the active table tab (its filter, sort and
 * page), or the table behind a structure tab. Returns false when there is nothing to export.
 */
export function openExportForActiveTab(): boolean {
  const { tabs, activeId } = useTabsStore.getState();
  const tab = tabs.find((t) => t.id === activeId);
  if (tab && (tab.kind === "table" || tab.kind === "structure")) {
    openExport({
      connectionId: tab.connectionId,
      source: { kind: "table", table: tab.table, tabId: tab.kind === "table" ? tab.id : null },
    });
    return true;
  }
  toast.message("Nothing to export here", {
    description: "Open a table, or export a query from the SQL editor.",
  });
  return false;
}

function rowsLabel(n: number): string {
  return `${formatCount(n)} row${n === 1 ? "" : "s"}`;
}

/** Saves the form's state for the current session (see `ExportDraft`). */
export function setExportDraft(draft: ExportDraft): void {
  useExportStore.setState({ draft });
}

/**
 * Streams `req` to its file, tracking progress in the store. Success closes the dialog and offers
 * "Reveal in Finder"; a cancel is reported neutrally; a failure shows inline while the dialog is
 * open (a toast otherwise). Resolves with the result, or null when it did not complete.
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
    toast.success(`Exported ${rowsLabel(result.rows)}`, {
      description: `${basename(result.path)} · ${formatBytes(result.bytes)} · ${formatDuration(result.durationMs)}`,
      action: {
        label: "Reveal in Finder",
        onClick: () =>
          void revealExport(result.path).catch((e: unknown) =>
            toast.error("Could not reveal the file", { description: toIpcError(e).message }),
          ),
      },
    });
    return result;
  } catch (err) {
    const e = toIpcError(err);
    const open = useExportStore.getState().open;
    useExportStore.setState({ run: null });
    if (!open) releaseExportTarget();
    if (e.code === "CANCELLED") {
      toast.message("Export cancelled", { description: "No file was written." });
    } else if (open) {
      useExportStore.setState({ error: e.message });
    } else {
      toast.error("Export failed", { description: e.message });
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
