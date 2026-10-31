import { CircleAlert, FileDown, Loader2 } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type FormEvent, type ReactNode } from "react";
import { toast } from "sonner";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/features/settings/controls";
import { connectionCaps, useCaps } from "@/lib/caps";
import { toIpcError } from "@/lib/ipc";
import { isReadOnlyStatement } from "@/lib/sql/statement";
import { useConnectionsStore } from "@/stores/connections";
import { cn } from "@/lib/utils";
import { allResultsFor, type RunResult } from "./allResults";
import { pickExportFolder, pickExportPath } from "./destination";
import { Check, FormatOptions, Row } from "./formParts";
import {
  BATCH_SIZE_RANGE,
  UNTITLED_TABLE,
  buildExportRequest,
  buildExportResultsRequest,
  clampBatchSize,
  csvProblem,
  defaultFileName,
  dirname,
  encodingForCharset,
  exportProblem,
  exportsQueries,
  fileExtension,
  fileFilter,
  formatBytes,
  formatCount,
  formatElapsed,
  formatsFor,
  hasCappedBytes,
  loadExportOptions,
  loadLastDir,
  resultsLayout,
  resultsProblem,
  saveExportOptions,
  saveLastDir,
  sqlWritesStructure,
  withOfferedFormat,
  type ExportInput,
  type ExportOptions,
  type ExportProblem,
  type FormatKind,
  type ResultsScope,
  type RowScope,
} from "./options";
import {
  cancelExport,
  closeExport,
  releaseExportTarget,
  runExport,
  runExportResults,
  setExportDraft,
  useExportStore,
  type ExportRun,
  type OpenExportArgs,
} from "./store";
import { TablesExportForm } from "./TablesExportForm";
import { useConnectionCharset } from "./useConnectionCharset";
import { useExportSource, type ResolvedExportSource } from "./useExportSource";

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
    if (!src.page) return null;
    // Not loaded yet (the next page is on its way): the export reads from the page's first row.
    if (src.page.rows === null)
      return `Up to ${formatCount(src.page.limit)} rows from row ${formatCount(src.page.first)}`;
    if (src.page.rows === 0) return "The current page is empty.";
    return `Rows ${formatCount(src.page.first)}–${formatCount(src.page.first + src.page.rows - 1)}`;
  }
  if (!src.count) return null;
  const n = `${src.count.exact ? "" : "~"}${formatCount(src.count.count)}`;
  const what = src.count.count === 1 && src.count.exact ? "row" : "rows";
  return src.filtered ? `${n} ${what} match the filter` : `${n} ${what}`;
}

export function Progress({ run }: { run: ExportRun }) {
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

/** Under the "All results" choice: where the results go, and which of them are not whole. */
function allResultsHint(format: FormatKind, ext: string, results: readonly RunResult[]): ReactNode {
  const cut = results.filter((r) => r.truncated && r.stopped === null).length;
  const stopped = results.filter((r) => r.stopped !== null).length;
  const plural = (n: number) => `${n} result${n === 1 ? "" : "s"}`;
  const where =
    format === "xlsx"
      ? "One workbook with a sheet per result"
      : format === "html"
        ? "One page with a heading per result"
        : format === "sqlInsert"
          ? "One script; each result's INSERTs name its table, or untitled"
          : `One .${ext} file per result in the folder you choose`;
  return (
    <>
      <div>{where}</div>
      {cut > 0 && (
        <div>{plural(cut)} stopped at the row limit: only the rows shown are written.</div>
      )}
      {stopped > 0 && (
        <div>
          {plural(stopped)} cancelled or failed partway: only the rows that arrived are written.
        </div>
      )}
    </>
  );
}

function ExportForm({ target }: { target: OpenExportArgs }) {
  const run = useExportStore((s) => s.run);
  const error = useExportStore((s) => s.error);
  const driver = useConnectionsStore((s) => s.byId[target.connectionId]?.info.driver);
  const isSql = target.source.kind === "sql";
  const held = target.source.kind === "rows" ? target.source : null;
  const capped = useMemo(() => (held ? hasCappedBytes(held.rows) : false), [held]);
  const caps = useCaps(target.connectionId);
  // A shown result can also be exported by running its statement again: when the rows held are
  // not the whole result (cut at the row limit, or the statement stopped early), and when a
  // binary value was cut to its preview (a PostgreSQL query export reads bytea in full). Only a
  // statement that just reads: running `DELETE … RETURNING` or `CALL` again would write again.
  // And only on a session that exports queries (a core one).
  const rerunSql =
    held?.sql &&
    exportsQueries(caps) &&
    isReadOnlyStatement(held.sql) &&
    (held.truncated || held.stopped !== null || (capped && driver === "postgres"))
      ? held.sql
      : null;
  // Reopening the dialog in the same session (e.g. on a running export) restores the form as it was.
  const [draft] = useState(() => useExportStore.getState().draft);
  const [options, setOptions] = useState<ExportOptions>(() =>
    withOfferedFormat(
      draft?.options ?? loadExportOptions(),
      formatsFor(connectionCaps(target.connectionId)),
    ),
  );
  // Only the formats the session writes (no SQL script without SQL export).
  const formats = useMemo(() => formatsFor(caps), [caps]);
  useEffect(() => setOptions((o) => withOfferedFormat(o, formats)), [formats]);
  // Connection ▸ View Using Encoding: a connection reading latin1 (cp1252, UTF-16) writes its files
  // in it, until the encoding is chosen here. The remembered choice is not overwritten by it.
  const rememberedEncoding = useRef(draft?.rememberedEncoding ?? options.encoding);
  const charset = useConnectionCharset(target.connectionId);
  const followed = encodingForCharset(charset);
  // A draft written by the form remembers whether the select was changed; one written from outside
  // (Export Current Page…) has not been touched, so the connection's encoding still applies.
  const [encodingTouched, setEncodingTouched] = useState(draft?.encodingTouched ?? false);
  useEffect(() => {
    if (followed && !encodingTouched)
      setOptions((o) => (o.encoding === followed ? o : { ...o, encoding: followed }));
  }, [followed, encodingTouched]);
  const following = followed !== null && !encodingTouched && options.encoding === followed;
  const storedOptions = (o: ExportOptions): ExportOptions =>
    following ? { ...o, encoding: rememberedEncoding.current } : o;
  const [scope, setScope] = useState<RowScope>(draft?.scope ?? "all");
  // A shown result of a run holding several: this one, or all of them.
  const runResults = useMemo(
    () => (held ? allResultsFor(target.connectionId, held) : null),
    [target.connectionId, held],
  );
  const [resultsScope, setResultsScope] = useState<ResultsScope>(draft?.resultsScope ?? "this");
  const allResults = runResults !== null && resultsScope === "all" ? runResults : null;
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
    () =>
      setExportDraft({
        options,
        scope,
        visibleOnly,
        tableName,
        batchText,
        allRows,
        resultsScope,
        encodingTouched,
        rememberedEncoding: rememberedEncoding.current,
      }),
    [options, scope, visibleOnly, tableName, batchText, allRows, resultsScope, encodingTouched],
  );

  const resultsFiles = allResults !== null && resultsLayout(options.format) === "filePerTable";
  const fileName = useMemo(
    () =>
      allResults
        ? resultsFiles
          ? `${allResults.length} .${fileExtension(options)} files into a folder`
          : defaultFileName("results", options, new Date())
        : defaultFileName(src.fileBase, options, new Date()),
    [src.fileBase, options, allResults, resultsFiles],
  );

  const running = run !== null;
  const tableSource = input?.kind === "table";
  // The script creates the source table: its INSERTs go there.
  const scriptsTable = tableSource && sqlWritesStructure(options);
  const emptySql =
    isSql && (target.source.kind === "sql" ? target.source.sql.trim() === "" : false);
  const singleProblem = useMemo(
    () => (input ? exportProblem(input, options.format, options.sql) : null),
    [input, options.format, options.sql],
  );
  const allProblem = useMemo(
    () => (allResults ? resultsProblem(allResults, options.format, options.sql) : null),
    [allResults, options.format, options.sql],
  );
  const problem = allResults ? allProblem : singleProblem;
  const csvIssue = options.format === "csv" ? csvProblem(options.csv) : null;
  const canSubmit =
    !running && !picking && input !== null && !emptySql && !problem && csvIssue === null;

  /** "All results": one file (Excel, HTML, SQL) or a folder with a file per result (CSV, JSON). */
  const submitAllResults = async (results: readonly RunResult[], finalOptions: ExportOptions) => {
    const oneFile = resultsLayout(finalOptions.format) === "singleFile";
    let destination: string | null;
    try {
      destination = oneFile
        ? await pickExportPath(
            defaultFileName("results", finalOptions, new Date()),
            fileFilter(finalOptions),
            loadLastDir(),
          )
        : await pickExportFolder(loadLastDir());
    } catch (e) {
      toast.error("Could not open the dialog", { description: toIpcError(e).message });
      return;
    } finally {
      setPicking(false);
    }
    if (!destination) return;
    saveExportOptions(storedOptions(finalOptions));
    const dir = oneFile ? dirname(destination) : destination;
    if (dir) saveLastDir(dir);
    let req;
    try {
      req = buildExportResultsRequest({
        exportId: crypto.randomUUID(),
        items: results,
        options: finalOptions,
        target: destination,
        name: "results",
      });
    } catch (e) {
      useExportStore.setState({ error: toIpcError(e).message });
      return;
    }
    void runExportResults(target.connectionId, req);
  };

  const submit = async () => {
    if (!canSubmit || !input) return;
    const finalOptions: ExportOptions = {
      ...options,
      sql: { ...options.sql, batchSize: clampBatchSize(Number.parseInt(batchText, 10)) },
    };
    setPicking(true);
    if (allResults) {
      await submitAllResults(allResults, finalOptions);
      return;
    }
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
    saveExportOptions(storedOptions(finalOptions));
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
        sqlTableName: scriptsTable
          ? ""
          : tableName.trim() || (rerun ? (src.defaultTableName ?? "") : ""),
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
        {runResults && held && (
          <Row
            id="export-results"
            label="Results"
            hint={
              allResults ? (
                <>
                  {allResultsHint(options.format, fileExtension(options), allResults)}
                  {problem?.field === "rows" && problemHint(problem)}
                </>
              ) : undefined
            }
          >
            <Segmented<ResultsScope>
              id="export-results"
              value={resultsScope}
              onChange={setResultsScope}
              options={[
                {
                  value: "this",
                  label: "This result",
                  title: `Only ${held.label}, as shown`,
                },
                {
                  value: "all",
                  label: `All results (${runResults.length})`,
                  title: "Every finished result of the run",
                },
              ]}
            />
          </Row>
        )}
        {held &&
          !allResults &&
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
        {!isSql && !held && (src.columns.hidden || src.columns.reordered) && (
          <Row
            id="export-columns"
            label="Columns"
            hint={problem?.field === "columns" ? problemHint(problem) : undefined}
          >
            <Check checked={visibleOnly} onChange={setVisibleOnly}>
              {src.columns.hidden
                ? `Only visible columns${src.columns.reordered ? ", in grid order" : ""} (${src.columns.visible} of ${src.columns.total})`
                : "Columns in grid order"}
            </Check>
          </Row>
        )}
        <Row id="export-format" label="Format">
          <Segmented<FormatKind>
            id="export-format"
            value={options.format}
            onChange={(format) => setOptions((o) => ({ ...o, format }))}
            options={formats}
          />
        </Row>

        <FormatOptions
          options={options}
          setOptions={setOptions}
          tableSource={tableSource}
          encodingFollows={following ? `Follows View Using Encoding (${charset})` : undefined}
          onEncodingChange={() => setEncodingTouched(true)}
          sqlExtra={
            options.sql.content && (
              <>
                {!allResults && (
                  <Row
                    id="export-table-name"
                    label="Table name"
                    hint={
                      scriptsTable
                        ? "The script creates and fills the source table"
                        : src.defaultTableName
                          ? "Target of the INSERT statements"
                          : "Target of the INSERT statements; without a name they go into untitled"
                    }
                  >
                    <Input
                      id="export-table-name"
                      value={scriptsTable ? "" : tableName}
                      disabled={scriptsTable}
                      onChange={(e) => setTableName(e.target.value)}
                      placeholder={src.defaultTableName ?? UNTITLED_TABLE}
                      spellCheck={false}
                      autoComplete="off"
                      className="h-7 w-56 font-mono text-xs"
                    />
                  </Row>
                )}
                <Row id="export-batch" label="Batch size" hint="Rows per INSERT statement">
                  <Input
                    id="export-batch"
                    type="number"
                    inputMode="numeric"
                    min={BATCH_SIZE_RANGE.min}
                    max={BATCH_SIZE_RANGE.max}
                    value={batchText}
                    onChange={(e) => setBatchText(e.target.value)}
                    onBlur={() =>
                      setBatchText(String(clampBatchSize(Number.parseInt(batchText, 10))))
                    }
                    className="h-7 w-24 font-mono text-xs tabular-nums"
                  />
                </Row>
              </>
            )
          }
        />
        {problem?.field === "options" && (
          <div className="text-warning text-[11px] leading-4">{problem.message}</div>
        )}
      </fieldset>

      <div className="border-border bg-surface-2/60 flex min-h-12 items-center gap-2 rounded-b-xl border-t px-4 py-2">
        <div className="min-w-0 flex-1">
          {run ? (
            <div className="flex min-w-0 flex-col gap-0.5">
              {run.table && (
                <span className="text-muted-foreground truncate text-[11px]">
                  Result {run.table.index + 1} of {run.table.count}:{" "}
                  <span className="text-foreground">{run.table.name}</span>
                </span>
              )}
              <Progress run={run} />
            </div>
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
        {target?.source.kind === "tables" ? (
          <TablesExportForm key={session} target={target} />
        ) : (
          target && <ExportForm key={session} target={target} />
        )}
      </DialogContent>
    </Dialog>
  );
}
