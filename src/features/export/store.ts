import { toast } from "sonner";
import { create } from "zustand";
import { ipc, toIpcError, type ExportRequest, type ExportResult, type TableRef } from "@/lib/ipc";
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

/** What to export: a table (with the tab's filter / sort / page when `tabId` is given) or SQL. */
export type ExportSourceSpec =
  | {
      kind: "table";
      table: TableRef;
      /** Table tab whose filter, sort, visible columns and page apply; null = the whole table. */
      tabId?: string | null;
    }
  | { kind: "sql"; sql: string };

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
    useExportStore.setState({ run: null, open: false });
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
