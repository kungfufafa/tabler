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
import { isReadOnlyStatement } from "@/lib/sql/statement";
import { useConnectionsStore } from "@/stores/connections";
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
  hasCappedBytes,
  loadExportOptions,
  loadLastDir,
  saveExportOptions,
  saveLastDir,
  type ExportInput,
  type ExportOptions,
  type ExportProblem,
  type FormatKind,
  type RowScope,
} from "./options";
import {
  cancelExport,
  closeExport,
  releaseExportTarget,
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

/** A result shown in the SQL editor, as the dialog's source. */
type HeldRows = Extract<OpenExportArgs["source"], { kind: "rows" }>;

/**
 * Hint under a shown result's "Rows" choice: what the choice writes, or why the rows shown are
 * not the whole result (or can't be written).
 */
function heldRowsHint(
  held: HeldRows,
  rerun: boolean,
  canRerun: boolean,
  capped: boolean,
  problem: ExportProblem | null,
): ReactNode {
  if (rerun)
    return capped
      ? "Runs the statement again; the file gets every row it returns, binary values in full."
      : "Runs the statement again; the file gets every row it returns.";
  if (problem?.field === "rows")
    return (
      <span className="text-warning">
        {problem.message}{" "}
        {canRerun ? "Export all rows to read it in full." : "Export its table instead."}
      </span>
    );
  if (held.stopped === "cancelled")
    return "The statement was cancelled: only the rows that arrived before it stopped are written.";
  if (held.stopped === "error")
    return "The statement failed partway: only the rows that arrived before the error are written.";
  return "The result stopped at the row limit; only the rows shown are written.";
}

function ExportForm({ target }: { target: OpenExportArgs }) {
  const run = useExportStore((s) => s.run);
  const error = useExportStore((s) => s.error);
  const driver = useConnectionsStore((s) => s.byId[target.connectionId]?.info.driver);
  const isSql = target.source.kind === "sql";
  const held = target.source.kind === "rows" ? target.source : null;
  const capped = useMemo(() => (held ? hasCappedBytes(held.rows) : false), [held]);
  // A shown result can also be exported by running its statement again: when the rows held are
  // not the whole result (cut at the row limit, or the statement stopped early), and when a
  // binary value was cut to its preview (a PostgreSQL query export reads bytea in full). Only a
  // statement that just reads: running `DELETE … RETURNING` or `CALL` again would write again.
  const rerunSql =
    held?.sql &&
    isReadOnlyStatement(held.sql) &&
    (held.truncated || held.stopped !== null || (capped && driver === "postgres"))
      ? held.sql
      : null;
  // Reopening the dialog in the same session (e.g. on a running export) restores the form as it was.
  const [draft] = useState(() => useExportStore.getState().draft);
  const [options, setOptions] = useState<ExportOptions>(
    () => draft?.options ?? loadExportOptions(),
  );
  const [scope, setScope] = useState<RowScope>(draft?.scope ?? "all");
  const [visibleOnly, setVisibleOnly] = useState(draft?.visibleOnly ?? true);
  const [tableName, setTableName] = useState(draft?.tableName ?? "");
  // The rows shown by default, unless they can't be written as they are.
  const [allRows, setAllRows] = useState(draft?.allRows ?? (capped && rerunSql !== null));
  const [batchText, setBatchText] = useState(
    () => draft?.batchText ?? String(options.sql.batchSize),
  );
  const [picking, setPicking] = useState(false);
  const src = useExportSource(target, scope, visibleOnly);
  const rerun = rerunSql !== null && allRows;
  const input = useMemo<ExportInput | null>(
    () => (rerun && rerunSql !== null ? { kind: "sql", sql: rerunSql } : src.input),
    [rerun, rerunSql, src.input],
  );
  const statementText = target.source.kind === "sql" ? target.source.sql : (held?.sql ?? null);
  useEffect(
    () => setExportDraft({ options, scope, visibleOnly, tableName, batchText, allRows }),
    [options, scope, visibleOnly, tableName, batchText, allRows],
  );

  const fileName = useMemo(
    () => defaultFileName(src.fileBase, options, new Date()),
    [src.fileBase, options],
  );
  const set = <K extends keyof ExportOptions>(key: K, patch: Partial<ExportOptions[K]>) =>
    setOptions((o) => ({ ...o, [key]: { ...(o[key] as object), ...patch } }));

  const running = run !== null;
  // Table and result sources name their INSERT target themselves (when the result's table is
  // known); a query needs one.
  const needsTableName =
    options.format === "sqlInsert" && tableName.trim() === "" && !src.defaultTableName;
  const emptySql =
    isSql && (target.source.kind === "sql" ? target.source.sql.trim() === "" : false);
  const problem = useMemo(
    () => (input ? exportProblem(input, options.format) : null),
    [input, options.format],
  );
  const canSubmit =
    !running && !picking && input !== null && !needsTableName && !emptySql && !problem;

  const submit = async () => {
    if (!canSubmit || !input) return;
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
        input,
        options: finalOptions,
        // Running the statement again exports a query, which has no table of its own.
        sqlTableName: tableName.trim() || (rerun ? (src.defaultTableName ?? "") : ""),
      });
    } catch (e) {
      useExportStore.setState({ error: toIpcError(e).message });
      return;
    }
    const expectedRows =
      input.kind === "table" && input.scope === "page"
        ? (src.page?.rows ?? null)
        : rerun
          ? null
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
          <span
            className={cn("text-foreground truncate", !isSql && !held && "font-mono")}
            title={src.title}
          >
            {src.title}
          </span>
          {src.details.map((d) => (
            <span key={d} className="shrink-0">
              · {d}
            </span>
          ))}
        </DialogDescription>
        {statementText !== null && (
          <pre className="border-border bg-surface-2 text-muted-foreground mt-1 line-clamp-3 max-h-16 overflow-hidden rounded-md border px-2 py-1 font-mono text-[11px] leading-4 whitespace-pre-wrap">
            {statementText.trim() || "(empty query)"}
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
        {held &&
          (rerunSql !== null ||
            held.truncated ||
            held.stopped !== null ||
            problem?.field === "rows") && (
            <Row
              id="export-result-rows"
              label="Rows"
              hint={heldRowsHint(held, rerun, rerunSql !== null, capped, problem)}
            >
              {rerunSql !== null ? (
                <Segmented<"shown" | "all">
                  id="export-result-rows"
                  value={allRows ? "all" : "shown"}
                  onChange={(v) => setAllRows(v === "all")}
                  options={[
                    {
                      value: "shown",
                      label: `The ${formatCount(held.rows.length)} rows shown`,
                      title: "Write the rows the result holds",
                    },
                    {
                      value: "all",
                      label: "All rows",
                      title: "All rows — runs the statement again",
                    },
                  ]}
                />
              ) : (
                <span id="export-result-rows" className="pt-1.5 text-xs leading-4">
                  The {formatCount(held.rows.length)} rows shown
                </span>
              )}
            </Row>
          )}
        {!isSql && !held && src.columns.visible < src.columns.total && (
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
                  <span className="text-warning">
                    {held && !rerun
                      ? "The result's table is unknown: it needs a target table name"
                      : "A query export needs a target table name"}
                  </span>
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
      <DialogContent
        className="gap-0 p-0 sm:max-w-[480px]"
        showCloseButton
        // Closed and animated out: the source (a shown result's rows) is no longer needed.
        onCloseAutoFocus={releaseExportTarget}
      >
        {target && <ExportForm key={session} target={target} />}
      </DialogContent>
    </Dialog>
  );
}
