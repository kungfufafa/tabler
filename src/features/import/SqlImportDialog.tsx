import { useQuery, useQueryClient } from "@tanstack/react-query";
import { CircleAlert, FileUp, Loader2, TriangleAlert } from "lucide-react";
import { useRef, useState } from "react";
import { toast } from "sonner";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { formatBytes, formatCount } from "@/features/export/options";
import { guardExecution } from "@/features/safety/guard";
import { invalidateConnectionSchema, reportHeldTables } from "@/features/structure/invalidate";
import { ipc, toIpcError, type SqlImportProgress, type SqlImportReport } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { ENCODINGS } from "./mapping";
import { closeImport, type ImportRequest } from "./store";

const statements = (n: number) => `${formatCount(n)} statement${n === 1 ? "" : "s"}`;

/** The preview's warning about bytes that are not UTF-8, or null. */
function invalidUtf8Warning(count: number, firstLine: number | null): string | null {
  if (count === 0) return null;
  const sequences = `${formatCount(count)} byte sequence${count === 1 ? " is" : "s are"}`;
  const at = firstLine !== null ? ` (the first on line ${formatCount(firstLine)})` : "";
  return `${sequences} not UTF-8${at}: they will be read as Windows-1252.`;
}

interface RunState {
  importId: string;
  progress: SqlImportProgress | null;
  cancelling: boolean;
}

/**
 * Import ▸ From SQL Dump…: the first statements of the file and its size, then "Continue" runs
 * every statement on the connection, with progress, "Stop on error" and cancel. A run with
 * errors keeps the dialog open on its report.
 */
export function SqlImportDialog({ request, path }: { request: ImportRequest; path: string }) {
  const { connectionId } = request;
  const qc = useQueryClient();
  const [stopOnError, setStopOnError] = useState(true);
  const [run, setRun] = useState<RunState | null>(null);
  const [report, setReport] = useState<SqlImportReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Set before the safety guard is awaited: a second ⌘↩ or click meanwhile does not start twice.
  const starting = useRef(false);
  const [guarding, setGuarding] = useState(false);
  const preview = useQuery({
    queryKey: ["sqlDumpPreview", connectionId, path],
    queryFn: () => ipc.imports.previewSql(connectionId, path),
    staleTime: Infinity,
    gcTime: 60_000,
    retry: false,
  });
  const data = preview.data;
  const running = run !== null || guarding;
  const fileName = data?.fileName ?? path.slice(path.lastIndexOf("/") + 1);

  const close = () => {
    if (!running) closeImport();
  };

  const start = async () => {
    if (running || !data || starting.current) return;
    starting.current = true;
    try {
      await runImport();
    } finally {
      starting.current = false;
    }
  };

  const runImport = async () => {
    setGuarding(true);
    let allowed: boolean;
    try {
      allowed = await guardExecution(connectionId, {
        kind: "operation",
        label: "Import SQL dump",
        write: true,
      });
    } catch (e) {
      setError(toIpcError(e).message);
      return;
    } finally {
      setGuarding(false);
    }
    if (!allowed) return;
    const importId = crypto.randomUUID();
    setError(null);
    setReport(null);
    setRun({ importId, progress: null, cancelling: false });
    try {
      const result = await ipc.imports.sql(connectionId, { importId, path, stopOnError }, (p) =>
        setRun((r) => (r?.importId === importId ? { ...r, progress: p } : r)),
      );
      // The dump may have changed any table; press ⌘R in a tab to see its new rows.
      if (result.executed > 0) reportHeldTables(invalidateConnectionSchema(qc, connectionId));
      if (result.cancelled) {
        toast.message("Import cancelled", {
          description: [
            `${statements(result.executed)} ran before it stopped.`,
            ...result.notes,
          ].join("\n"),
        });
        closeImport();
      } else if (result.notes.length > 0) {
        // Rolled back, decoded as Windows-1252…: stay on the report.
        setReport(result);
      } else if (result.failed === 0) {
        toast.success("Imported SQL dump", {
          description: `${fileName} · ${statements(result.executed)}${result.skipped > 0 ? ` · ${formatCount(result.skipped)} psql commands skipped` : ""}`,
        });
        closeImport();
      } else {
        setReport(result);
      }
    } catch (e) {
      setError(toIpcError(e).message);
    } finally {
      setRun(null);
    }
  };

  const cancel = async () => {
    if (!run || run.cancelling) return;
    setRun({ ...run, cancelling: true });
    try {
      await ipc.imports.cancel(run.importId);
    } catch (e) {
      toast.error("Could not cancel the import", { description: toIpcError(e).message });
    }
  };

  const progress = run?.progress;
  const pct =
    progress && progress.totalBytes > 0
      ? Math.min(100, (progress.bytes / progress.totalBytes) * 100)
      : null;
  const encoding = ENCODINGS.find((e) => e.value === data?.encoding)?.label;

  return (
    <Dialog open onOpenChange={(open) => !open && close()}>
      <DialogContent
        className="flex max-h-[min(640px,calc(100vh-2rem))] flex-col gap-0 p-0 sm:max-w-[640px]"
        showCloseButton={!running}
        onInteractOutside={(e) => running && e.preventDefault()}
        onEscapeKeyDown={(e) => running && e.preventDefault()}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !report) {
            e.preventDefault();
            void start();
          }
        }}
      >
        <DialogHeader className="border-border gap-1 border-b px-4 py-3 pr-12">
          <DialogTitle className="flex items-center gap-2 text-sm">
            <FileUp className="text-brand size-4" />
            Import SQL Dump
          </DialogTitle>
          <DialogDescription className="flex min-w-0 items-center gap-1.5 text-xs">
            <span className="text-foreground truncate font-mono" title={path}>
              {fileName}
            </span>
            {data && (
              <span className="shrink-0">
                · {formatBytes(data.size)}
                {data.compressed ? " (gzip)" : ""}
                {encoding ? ` · ${encoding}` : ""}
              </span>
            )}
          </DialogDescription>
        </DialogHeader>

        <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-hidden px-4 py-3">
          {report ? (
            <div className="flex min-h-0 flex-col gap-2">
              <p className="text-xs">
                {statements(report.executed)} ran
                {report.failed > 0 && (
                  <>
                    , <span className="text-error">{formatCount(report.failed)} failed</span>
                  </>
                )}
                {report.stopped ? " — the import stopped at the first error." : "."}
                {report.skipped > 0 && ` ${formatCount(report.skipped)} psql commands skipped.`}
              </p>
              {report.notes.length > 0 && (
                <ul aria-label="Notes" className="flex flex-col gap-1">
                  {report.notes.map((n) => (
                    <li key={n} className="flex items-start gap-1.5 text-xs">
                      <TriangleAlert className="text-warning mt-px size-3.5 shrink-0" />
                      <span>{n}</span>
                    </li>
                  ))}
                </ul>
              )}
              <ul
                aria-label="Errors"
                hidden={report.errors.length === 0}
                className="border-border min-h-0 flex-1 divide-y overflow-y-auto rounded-md border"
              >
                {report.errors.map((e) => (
                  <li key={e.index} className="divide-border flex flex-col gap-1 px-2 py-1.5">
                    <span className="text-error text-xs">
                      Line {formatCount(e.line)}: {e.message}
                    </span>
                    <pre className="text-muted-foreground line-clamp-3 font-mono text-[11px] leading-4 whitespace-pre-wrap">
                      {e.sql}
                    </pre>
                  </li>
                ))}
              </ul>
            </div>
          ) : preview.isError ? (
            <div role="alert" className="text-error flex items-start gap-1.5 text-xs">
              <CircleAlert className="mt-px size-3.5 shrink-0" />
              {preview.error.message}
            </div>
          ) : !data ? (
            <div className="text-muted-foreground flex items-center gap-2 text-xs">
              <Loader2 className="size-3.5 animate-spin" /> Reading the file…
            </div>
          ) : (
            <>
              <div className="text-muted-foreground text-xs">
                {data.statements.length === 0
                  ? "The file holds no SQL statements."
                  : `The first ${statements(data.statements.length)}${data.more ? " of many" : ""}:`}
              </div>
              {invalidUtf8Warning(data.invalidUtf8, data.firstInvalidLine) && (
                <div role="note" className="flex items-start gap-1.5 text-xs">
                  <TriangleAlert className="text-warning mt-px size-3.5 shrink-0" />
                  {invalidUtf8Warning(data.invalidUtf8, data.firstInvalidLine)}
                </div>
              )}
              <pre
                data-testid="sql-preview"
                className="border-border bg-surface-2 min-h-24 flex-1 overflow-auto rounded-md border px-2 py-1.5 font-mono text-[11px] leading-4 whitespace-pre-wrap"
              >
                {data.statements.map((s) => `${s};`).join("\n\n")}
                {data.more ? "\n\n…" : ""}
              </pre>
            </>
          )}
          {!report && (
            <div className="flex flex-col gap-1">
              <label className="flex h-7 cursor-default items-center gap-2 text-xs">
                <Checkbox
                  checked={stopOnError}
                  disabled={running}
                  onCheckedChange={(v) => setStopOnError(v === true)}
                />
                Stop on error
              </label>
              <p className="text-muted-foreground text-[11px] leading-4">
                The statements run one by one on this connection. For large or complex dumps use
                File ▸ Restore…, which runs the database’s own tools.
              </p>
            </div>
          )}
        </div>

        <div className="border-border bg-surface-2/60 flex min-h-12 shrink-0 items-center gap-2 rounded-b-xl border-t px-4 py-2">
          <div className="min-w-0 flex-1">
            {run ? (
              <div role="status" aria-live="polite" className="flex min-w-0 flex-col gap-1">
                <div className="flex items-center gap-2 text-xs">
                  <Loader2 className="text-brand size-3.5 shrink-0 animate-spin" />
                  <span className="tabular-nums">
                    {statements(progress?.statements ?? 0)}
                    {progress && progress.failed > 0
                      ? ` · ${formatCount(progress.failed)} failed`
                      : ""}
                    {progress
                      ? ` · ${formatBytes(progress.bytes)} of ${formatBytes(progress.totalBytes)}`
                      : ""}
                    {run.cancelling ? " · cancelling…" : ""}
                  </span>
                </div>
                <div className="bg-surface-2 h-1 w-56 overflow-hidden rounded-full">
                  <div
                    className={cn(
                      "bg-brand h-full rounded-full transition-[width] duration-200",
                      pct === null && "w-1/3 animate-pulse",
                    )}
                    style={pct === null ? undefined : { width: `${pct}%` }}
                  />
                </div>
              </div>
            ) : error ? (
              <div role="alert" className="text-error flex min-w-0 items-start gap-1.5 text-xs">
                <CircleAlert className="mt-px size-3.5 shrink-0" />
                <span className="line-clamp-2" title={error}>
                  {error}
                </span>
              </div>
            ) : null}
          </div>
          {run ? (
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => void cancel()}
              disabled={run.cancelling}
            >
              Cancel import
            </Button>
          ) : report ? (
            <Button type="button" size="sm" onClick={() => closeImport()}>
              Done
            </Button>
          ) : (
            <>
              <Button type="button" variant="ghost" size="sm" onClick={close}>
                Cancel
              </Button>
              <Button
                type="button"
                size="sm"
                disabled={!data || data.statements.length === 0}
                onClick={() => void start()}
              >
                Continue
                <Kbd
                  combo="Mod+Enter"
                  className="border-primary-foreground/30 text-primary-foreground/80 bg-transparent"
                />
              </Button>
            </>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
