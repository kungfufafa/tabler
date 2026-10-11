import { CircleAlert, FileDown, Loader2 } from "lucide-react";
import { useEffect, useMemo, useState, type FormEvent, type ReactNode } from "react";
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
import { Input } from "@/components/ui/input";
import { Segmented } from "@/features/settings/controls";
import { toIpcError } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { pickExportPath } from "./destination";
import {
  BATCH_SIZE_RANGE,
  buildExportRequest,
  clampBatchSize,
  CSV_DELIMITERS,
  defaultFileName,
  dirname,
  exportProblem,
  fileFilter,
  formatBytes,
  formatCount,
  formatElapsed,
  loadExportOptions,
  loadLastDir,
  saveExportOptions,
  saveLastDir,
  type ExportOptions,
  type ExportProblem,
  type FormatKind,
  type RowScope,
} from "./options";
import {
  cancelExport,
  closeExport,
  runExport,
  setExportDraft,
  useExportStore,
  type ExportRun,
  type OpenExportArgs,
} from "./store";
import { useExportSource, type ResolvedExportSource } from "./useExportSource";

const FORMATS: ReadonlyArray<{ value: FormatKind; label: string; title: string }> = [
  { value: "csv", label: "CSV", title: "Comma-separated values" },
  { value: "json", label: "JSON", title: "JSON array or newline-delimited JSON" },
  { value: "sqlInsert", label: "SQL", title: "INSERT statements" },
];

/** Label column + control column; `id` labels a Segmented group (`${id}-label`). */
function Row({
  id,
  label,
  hint,
  children,
}: {
  id: string;
  label: string;
  hint?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="grid grid-cols-[84px_minmax(0,1fr)] items-start gap-x-3">
      <label
        id={`${id}-label`}
        htmlFor={id}
        className="text-muted-foreground pt-1.5 text-xs leading-4"
      >
        {label}
      </label>
      <div className="flex min-w-0 flex-col items-start gap-1">
        {children}
        {hint && <div className="text-muted-foreground text-[11px] leading-4">{hint}</div>}
      </div>
    </div>
  );
}

function Check({
  checked,
  onChange,
  disabled,
  children,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  children: ReactNode;
}) {
  return (
    <label
      className={cn(
        "flex h-7 cursor-default items-center gap-2 text-xs",
        disabled && "text-muted-foreground opacity-60",
      )}
    >
      <Checkbox
        checked={checked}
        disabled={disabled}
        onCheckedChange={(v) => onChange(v === true)}
      />
      {children}
    </label>
  );
}

const problemHint = (problem: ExportProblem) =>
  problem.severity === "fix" ? (
    <span className="text-warning">{problem.message}</span>
  ) : (
    problem.message
  );

function rowsHint(
  src: ResolvedExportSource,
  scope: RowScope,
  problem: ExportProblem | null,
): ReactNode {
  if (problem?.field === "rows") return problemHint(problem);
  if (scope === "page") {
    if (!src.pageExportable) return "Wait for the page to load to export it.";
    if (!src.page) return null;
    if (src.page.rows === 0) return "The current page is empty.";
    return `Rows ${formatCount(src.page.first)}–${formatCount(src.page.first + src.page.rows - 1)}`;
  }
  if (!src.count) return null;
  const n = `${src.count.exact ? "" : "~"}${formatCount(src.count.count)}`;
  const what = src.count.count === 1 && src.count.exact ? "row" : "rows";
  return src.filtered ? `${n} ${what} match the filter` : `${n} ${what}`;
}

function Progress({ run }: { run: ExportRun }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 500);
    return () => clearInterval(t);
  }, []);
  const total = run.expectedRows;
  const pct = total && total > 0 ? Math.min(100, (run.rows / total) * 100) : null;
  return (
    <div role="status" aria-live="polite" className="flex min-w-0 flex-col gap-1">
      <div className="flex min-w-0 items-center gap-2 text-xs">
        <Loader2 className="text-brand size-3.5 shrink-0 animate-spin" />
        <span className="tabular-nums">
          {formatCount(run.rows)} row{run.rows === 1 ? "" : "s"}
        </span>
        <span className="text-muted-foreground truncate tabular-nums">
          {formatBytes(run.bytes)} · {formatElapsed(now - run.startedAt)}
          {run.cancelling ? " · cancelling…" : ""}
        </span>
      </div>
      <div className="bg-surface-2 h-1 w-44 overflow-hidden rounded-full">
        <div
          className={cn(
            "bg-brand h-full rounded-full transition-[width] duration-200",
            pct === null && "w-1/3 animate-pulse",
          )}
          style={pct === null ? undefined : { width: `${pct}%` }}
        />
      </div>
    </div>
  );
}

function ExportForm({ target }: { target: OpenExportArgs }) {
  const run = useExportStore((s) => s.run);
  const error = useExportStore((s) => s.error);
  // Reopening the dialog in the same session (e.g. on a running export) restores the form as it was.
  const [draft] = useState(() => useExportStore.getState().draft);
  const [options, setOptions] = useState<ExportOptions>(
    () => draft?.options ?? loadExportOptions(),
  );
  const [scope, setScope] = useState<RowScope>(draft?.scope ?? "all");
  const [visibleOnly, setVisibleOnly] = useState(draft?.visibleOnly ?? true);
  const [tableName, setTableName] = useState(draft?.tableName ?? "");
  const [batchText, setBatchText] = useState(
    () => draft?.batchText ?? String(options.sql.batchSize),
  );
  const [picking, setPicking] = useState(false);
  const src = useExportSource(target, scope, visibleOnly);
  const isSql = target.source.kind === "sql";
  useEffect(
    () => setExportDraft({ options, scope, visibleOnly, tableName, batchText }),
    [options, scope, visibleOnly, tableName, batchText],
  );

  const fileName = useMemo(
    () => defaultFileName(src.fileBase, options, new Date()),
    [src.fileBase, options],
  );
  const set = <K extends keyof ExportOptions>(key: K, patch: Partial<ExportOptions[K]>) =>
    setOptions((o) => ({ ...o, [key]: { ...(o[key] as object), ...patch } }));

  const running = run !== null;
  const needsTableName = options.format === "sqlInsert" && isSql && tableName.trim() === "";
  const emptySql =
    isSql && (target.source.kind === "sql" ? target.source.sql.trim() === "" : false);
  const problem = src.input ? exportProblem(src.input, options.format) : null;
  const canSubmit =
    !running && !picking && src.input !== null && !needsTableName && !emptySql && !problem;

  const submit = async () => {
    if (!canSubmit || !src.input) return;
    const finalOptions: ExportOptions = {
      ...options,
      sql: { batchSize: clampBatchSize(Number.parseInt(batchText, 10)) },
    };
    setPicking(true);
    let path: string | null;
    try {
      const name = defaultFileName(src.fileBase, finalOptions, new Date());
      path = await pickExportPath(name, fileFilter(finalOptions), loadLastDir());
    } catch (e) {
      toast.error("Could not open the save dialog", { description: toIpcError(e).message });
      return;
    } finally {
      setPicking(false);
    }
    if (!path) return;
    saveExportOptions(finalOptions);
    const dir = dirname(path);
    if (dir) saveLastDir(dir);
    let req;
    try {
      req = buildExportRequest({
        exportId: crypto.randomUUID(),
        path,
        input: src.input,
        options: finalOptions,
        sqlTableName: tableName,
      });
    } catch (e) {
      useExportStore.setState({ error: toIpcError(e).message });
      return;
    }
    const expectedRows =
      src.input.kind === "table" && src.input.scope === "page"
        ? (src.page?.rows ?? null)
        : (src.count?.count ?? null);
    void runExport(target.connectionId, req, { expectedRows });
  };

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    void submit();
  };

  return (
    <form
      onSubmit={onSubmit}
      onKeyDown={(e) => {
        if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
          e.preventDefault();
          void submit();
        }
      }}
    >
      <DialogHeader className="border-border gap-1 border-b px-4 py-3 pr-12">
        <DialogTitle className="flex items-center gap-2 text-sm">
          <FileDown className="text-brand size-4" />
          Export
        </DialogTitle>
        <DialogDescription className="flex min-w-0 items-center gap-1.5 text-xs">
          <span className={cn("text-foreground truncate", !isSql && "font-mono")} title={src.title}>
            {src.title}
          </span>
          {src.details.map((d) => (
            <span key={d} className="shrink-0">
              · {d}
            </span>
          ))}
        </DialogDescription>
        {target.source.kind === "sql" && (
          <pre className="border-border bg-surface-2 text-muted-foreground mt-1 line-clamp-3 max-h-16 overflow-hidden rounded-md border px-2 py-1 font-mono text-[11px] leading-4 whitespace-pre-wrap">
            {target.source.sql.trim() || "(empty query)"}
          </pre>
        )}
      </DialogHeader>

      <fieldset disabled={running} className="flex min-w-0 flex-col gap-3 px-4 py-4">
        {src.fromTab && (
          <Row id="export-rows" label="Rows" hint={rowsHint(src, scope, problem)}>
            <Segmented<RowScope>
              id="export-rows"
              value={scope}
              onChange={setScope}
              options={[
                { value: "all", label: "All rows", title: "Every row matching the filter" },
                {
                  value: "page",
                  label: "Current page",
                  title: src.pageExportable
                    ? "Only the rows on screen"
                    : "Load the page first to export it",
                },
              ]}
            />
          </Row>
        )}
        {!isSql && src.columns.visible < src.columns.total && (
          <Row
            id="export-columns"
            label="Columns"
            hint={problem?.field === "columns" ? problemHint(problem) : undefined}
          >
            <Check checked={visibleOnly} onChange={setVisibleOnly}>
              Only visible columns ({src.columns.visible} of {src.columns.total})
            </Check>
          </Row>
        )}
        <Row id="export-format" label="Format">
          <Segmented<FormatKind>
            id="export-format"
            value={options.format}
            onChange={(format) => setOptions((o) => ({ ...o, format }))}
            options={FORMATS}
          />
        </Row>

        {options.format === "csv" && (
          <>
            <Row id="export-delimiter" label="Delimiter">
              <Segmented
                id="export-delimiter"
                value={options.csv.delimiter}
                onChange={(delimiter) => set("csv", { delimiter })}
                options={CSV_DELIMITERS.map((d) => ({
                  ...d,
                  label: <span className={cn(d.label !== "Tab" && "font-mono")}>{d.label}</span>,
                }))}
              />
            </Row>
            <Row id="export-csv-options" label="Options">
              <div className="flex flex-wrap items-center gap-x-4">
                <Check checked={options.csv.header} onChange={(header) => set("csv", { header })}>
                  Header row
                </Check>
                <Check
                  checked={options.csv.quoteAll}
                  onChange={(quoteAll) => set("csv", { quoteAll })}
                >
                  Quote all fields
                </Check>
              </div>
            </Row>
            <Row id="export-null" label="NULL as" hint="Text written for SQL NULL values">
              <Input
                id="export-null"
                value={options.csv.nullAs}
                onChange={(e) => set("csv", { nullAs: e.target.value })}
                placeholder="(empty)"
                maxLength={32}
                spellCheck={false}
                className="h-7 w-36 font-mono text-xs"
              />
            </Row>
          </>
        )}

        {options.format === "json" && (
          <>
            <Row
              id="export-json-layout"
              label="Layout"
              hint={
                options.json.ndjson
                  ? "One JSON object per line (NDJSON)"
                  : "A single array of row objects"
              }
            >
              <Segmented<"array" | "ndjson">
                id="export-json-layout"
                value={options.json.ndjson ? "ndjson" : "array"}
                onChange={(v) => set("json", { ndjson: v === "ndjson" })}
                options={[
                  { value: "array", label: "Array" },
                  { value: "ndjson", label: "NDJSON" },
                ]}
              />
            </Row>
            <Row id="export-json-options" label="Options">
              <Check
                checked={options.json.pretty && !options.json.ndjson}
                disabled={options.json.ndjson}
                onChange={(pretty) => set("json", { pretty })}
              >
                Pretty print
              </Check>
            </Row>
          </>
        )}

        {options.format === "sqlInsert" && (
          <>
            <Row
              id="export-table-name"
              label="Table name"
              hint={
                needsTableName ? (
                  <span className="text-warning">A query export needs a target table name</span>
                ) : (
                  "Target of the INSERT statements"
                )
              }
            >
              <Input
                id="export-table-name"
                value={tableName}
                onChange={(e) => setTableName(e.target.value)}
                placeholder={src.defaultTableName ?? "table_name"}
                spellCheck={false}
                autoComplete="off"
                className="h-7 w-56 font-mono text-xs"
              />
            </Row>
            <Row id="export-batch" label="Batch size" hint="Rows per INSERT statement">
              <Input
                id="export-batch"
                type="number"
                inputMode="numeric"
                min={BATCH_SIZE_RANGE.min}
                max={BATCH_SIZE_RANGE.max}
                value={batchText}
                onChange={(e) => setBatchText(e.target.value)}
                onBlur={() => setBatchText(String(clampBatchSize(Number.parseInt(batchText, 10))))}
                className="h-7 w-24 font-mono text-xs tabular-nums"
              />
            </Row>
          </>
        )}
      </fieldset>

      <div className="border-border bg-surface-2/60 flex min-h-12 items-center gap-2 rounded-b-xl border-t px-4 py-2">
        <div className="min-w-0 flex-1">
          {run ? (
            <Progress run={run} />
          ) : error ? (
            <div role="alert" className="text-error flex min-w-0 items-start gap-1.5 text-xs">
              <CircleAlert className="mt-px size-3.5 shrink-0" />
              <span className="line-clamp-2" title={error}>
                {error}
              </span>
            </div>
          ) : src.error ? (
            <div role="alert" className="text-error flex min-w-0 items-center gap-1.5 text-xs">
              <CircleAlert className="size-3.5 shrink-0" />
              <span className="truncate">{src.error}</span>
            </div>
          ) : (
            <span
              className="text-muted-foreground block truncate font-mono text-[11px]"
              title={fileName}
            >
              {fileName}
            </span>
          )}
        </div>
        {run ? (
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => void cancelExport()}
            disabled={run.cancelling}
          >
            Cancel export
          </Button>
        ) : (
          <>
            <Button type="button" variant="ghost" size="sm" onClick={closeExport}>
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={!canSubmit}>
              {picking || src.loading ? <Loader2 className="animate-spin" /> : null}
              Export…
              <Kbd
                combo="Mod+Enter"
                className="border-primary-foreground/30 text-primary-foreground/80 bg-transparent"
              />
            </Button>
          </>
        )}
      </div>
    </form>
  );
}

/** The export dialog; opened through `openExport` (see store.ts). Rendered by `ExportHost`. */
export function ExportDialog() {
  const open = useExportStore((s) => s.open);
  const target = useExportStore((s) => s.target);
  const session = useExportStore((s) => s.session);
  return (
    <Dialog open={open} onOpenChange={(next) => !next && closeExport()}>
      <DialogContent className="gap-0 p-0 sm:max-w-[480px]" showCloseButton>
        {target && <ExportForm key={session} target={target} />}
      </DialogContent>
    </Dialog>
  );
}
