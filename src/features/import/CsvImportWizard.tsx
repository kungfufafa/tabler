import { keepPreviousData, useQuery, useQueryClient } from "@tanstack/react-query";
import { FileUp } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { toast } from "sonner";
import { FormError } from "@/components/FormError";
import { KbdHint } from "@/components/KbdHint";
import { QuietStatus } from "@/components/QuietLoading";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { csvDefaults, importDelimiter, importQuote } from "@/features/datagrid/csv";
import { formatBytes, formatCount } from "@/features/export/options";
import { notifyInBackground } from "@/features/notify/notify";
import { guardExecution } from "@/features/safety/guard";
import { useCurrentSchema, useTables } from "@/features/sidebar/useSchemaTree";
import { invalidateSchema } from "@/features/structure/invalidate";
import { rowKeys } from "@/features/table-tab/useTableRows";
import { useDirtyDialogGuard } from "@/hooks/useDirtyDialogGuard";
import {
  ipc,
  toIpcError,
  type CsvFormat,
  type CsvImportFailure,
  type CsvImportProgress,
  type CsvImportReport,
  type ImportColumnDef,
  type ImportType,
  type InsertMode,
  type TableRef,
  type TextEncoding,
} from "@/lib/ipc";
import { useCaps } from "@/lib/caps";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import {
  applyOverrides,
  COLUMN_TYPES,
  DELIMITERS,
  ENCODINGS,
  fileNameOf,
  INSERT_MODES,
  insertModesFor,
  loadCsvPrefs,
  MATCH_STRATEGIES,
  matchColumns,
  matchedCount,
  QUOTES,
  saveCsvPrefs,
  statusLine,
  tableNameFromFile,
  type MatchStrategy,
} from "./mapping";
import { fromCsvPreview, fromJsonPreview, layoutLabel, type WizardPreview } from "./jsonPreview";
import { closeImport, type ImportRequest } from "./store";

/** "Do not import" in the per-column header menus (Radix Select needs a non-empty value). */
const SKIP = "\u0000skip";

const isTableKind = (kind: string) =>
  kind === "table" || kind === "partitioned" || kind === "foreignTable";

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 items-center gap-3">
      <span className="text-muted-foreground w-[5.5rem] shrink-0 text-right text-xs">{label}</span>
      <div className="flex min-w-0 flex-1 items-center gap-2">{children}</div>
    </div>
  );
}

function Toggle({
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
        "flex h-7 cursor-default items-center gap-2 text-xs whitespace-nowrap",
        disabled && "opacity-50",
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

function Choice<T extends string>({
  label,
  value,
  options,
  onChange,
  disabled,
  className,
}: {
  label: string;
  value: T;
  options: ReadonlyArray<{ value: T; label: string; title?: string }>;
  onChange: (v: T) => void;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <Select value={value} onValueChange={(v) => onChange(v as T)} disabled={disabled}>
      <SelectTrigger size="sm" aria-label={label} className={cn("bg-surface text-xs", className)}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {options.map((o) => (
          <SelectItem key={o.value} value={o.value} title={o.title} className="text-xs">
            {o.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

function failureText(f: CsvImportFailure): string {
  if (f.firstRow === null) return f.message;
  const rows =
    f.firstRow === f.lastRow || f.lastRow === null
      ? `row ${formatCount(f.firstRow)}`
      : `rows ${formatCount(f.firstRow)}–${formatCount(f.lastRow)}`;
  return `${f.message} (${rows}${f.line !== null ? `, line ${formatCount(f.line)}` : ""})`;
}

/** Rows "Replace" / "Update on duplicate" left out, and the import's notes, for the toast. */
function skippedText(report: CsvImportReport): string {
  const shown = report.skipped
    .slice(0, 3)
    .map((s) => `Row ${formatCount(s.row)} (line ${formatCount(s.line)}): ${s.reason}`);
  const more = report.rowsSkipped - shown.length;
  if (more > 0) shown.push(`…and ${formatCount(more)} more`);
  return [...shown, ...report.notes].join("\n");
}

interface RunState {
  importId: string;
  progress: CsvImportProgress | null;
  cancelling: boolean;
}

/**
 * The Import CSV wizard (File ▸ Import ▸ From CSV…): target table or "Create new table", a
 * preview of the first 100 rows with a column mapping per CSV column, the reading options and
 * the insert option. Mounted by `ImportHost` once a file was picked. A JSON file (From JSON…:
 * an array of objects or NDJSON) goes through the same wizard: its keys are the columns, matched
 * by name; null and missing keys are NULL; only the encoding is a reading option.
 */
export function CsvImportWizard({ request, path }: { request: ImportRequest; path: string }) {
  const { connectionId } = request;
  const json = request.kind === "json";
  const kindLabel = json ? "JSON" : "CSV";
  const qc = useQueryClient();
  const driver = useConnectionsStore((s) => s.byId[connectionId]?.info.driver);
  // The backend refuses an upsert where the server has none: the wizard does not offer it.
  const upsert = useCaps(connectionId)?.data.upsert ?? true;
  const current = useCurrentSchema(connectionId);
  const schema = request.table?.schema ?? current;
  const tables = useTables(connectionId, schema);
  const [prefs] = useState(loadCsvPrefs);
  // Settings ▸ General ▸ CSV default options: the starting quote, and the delimiter when the
  // detection is not sure.
  const [csvPrefs] = useState(() => csvDefaults());
  const [format, setFormat] = useState<CsvFormat>({
    delimiter: null,
    quote: importQuote(prefs.quote, csvPrefs.quote),
    encoding: null,
    header: prefs.header,
  });
  const [emptyAsNull, setEmptyAsNull] = useState(prefs.emptyAsNull);
  const [strategy, setStrategy] = useState<MatchStrategy>(prefs.strategy);
  const [creating, setCreating] = useState(false);
  const [tableName, setTableName] = useState<string | null>(request.table?.name ?? null);
  const [newName, setNewName] = useState(() => tableNameFromFile(fileNameOf(path)));
  const [mode, setMode] = useState<InsertMode>("stopOnError");
  const [overrides, setOverrides] = useState<Record<number, string | null>>({});
  const [defs, setDefs] = useState<ImportColumnDef[] | null>(null);
  const [skipped, setSkipped] = useState<Set<number>>(new Set());
  const [run, setRun] = useState<RunState | null>(null);
  const [failure, setFailure] = useState<{ text: string; imported: number } | null>(null);
  const [startError, setStartError] = useState<string | null>(null);
  // Set before the safety guard is awaited: a second ⌘↩ or click meanwhile does not start twice.
  const starting = useRef(false);
  const [guarding, setGuarding] = useState(false);
  // Set by the user's own choices only (not by detection or the first table picked), so a quick
  // look still closes with one Escape while hand-made mappings are not thrown away by accident.
  const [edited, setEdited] = useState(false);
  const edit =
    <A extends unknown[]>(fn: (...args: A) => void) =>
    (...args: A) => {
      setEdited(true);
      fn(...args);
    };
  const nameInput = useRef<HTMLInputElement>(null);
  // "Create new table" swaps the table menu for the name field: the user types the name next.
  const focusName = useRef(false);
  useEffect(() => {
    if (!creating || !focusName.current) return;
    focusName.current = false;
    nameInput.current?.focus();
    nameInput.current?.select();
  }, [creating]);

  const preview = useQuery({
    queryKey: json ? ["jsonPreview", path, format.encoding] : ["csvPreview", path, format],
    queryFn: async (): Promise<WizardPreview> =>
      json
        ? fromJsonPreview(await ipc.imports.previewJson({ path, encoding: format.encoding }))
        : fromCsvPreview(await ipc.imports.previewCsv({ path, format })),
    placeholderData: keepPreviousData,
    staleTime: Infinity,
    gcTime: 60_000,
    retry: false,
  });
  const data: WizardPreview | undefined = preview.data;
  // JSON keys are names: they match like a CSV header.
  const header = json || format.header;

  // The first preview detects the delimiter and encoding: show them as chosen (and reuse the
  // preview for the resolved options instead of reading the file again). A detection that found
  // a single column is not sure of the delimiter: the CSV default option's one is tried instead.
  useEffect(() => {
    if (json) {
      if (!data || format.encoding !== null) return;
      qc.setQueryData(["jsonPreview", path, data.encoding], data);
      setFormat((f) => ({ ...f, encoding: data.encoding }));
      return;
    }
    if (!data || (format.delimiter !== null && format.encoding !== null)) return;
    const resolved: CsvFormat = {
      ...format,
      delimiter:
        format.delimiter ??
        importDelimiter(data.delimiter, data.columns.length, csvPrefs.delimiter),
      encoding: format.encoding ?? data.encoding,
    };
    if (resolved.delimiter === data.delimiter)
      qc.setQueryData(["csvPreview", path, resolved], data);
    setFormat(resolved);
  }, [data, format, path, qc, csvPrefs, json]);

  // Table list: tables only (not views); the request's table first selected.
  const tableList = useMemo(
    () => (tables.data ?? []).filter((t) => isTableKind(t.kind)),
    [tables.data],
  );
  useEffect(() => {
    if (tableName === null && !creating && tables.isSuccess) {
      if (tableList.length === 0) setCreating(true);
      else setTableName(tableList[0]!.name);
    }
  }, [tableName, creating, tables.isSuccess, tableList]);

  const target: TableRef | null =
    !creating && tableName !== null
      ? { schema: request.table?.schema ?? schema, name: tableName }
      : null;
  const meta = useQuery({
    queryKey: target ? rowKeys.meta(connectionId, target) : ["import", "no-table"],
    queryFn: () => ipc.schema.table(connectionId, target!),
    enabled: target !== null,
    staleTime: 60_000,
  });
  const tableColumns = useMemo(
    () => (meta.data?.columns ?? []).filter((c) => !c.isGenerated).map((c) => c.name),
    [meta.data],
  );

  const csvNames = useMemo(() => (data?.columns ?? []).map((c) => c.name), [data]);
  // A new preview (other delimiter, header…) resets the per-column choices.
  const signature = `${csvNames.join("\u0001")}|${header}`;
  const lastSignature = useRef<string | null>(null);
  useEffect(() => {
    if (!data || lastSignature.current === signature) return;
    lastSignature.current = signature;
    setOverrides({});
    setSkipped(new Set());
    setDefs(data.columns.map((c) => ({ ...c.detected, name: c.name })));
  }, [data, signature]);

  const matched = useMemo(
    () => matchColumns(csvNames, tableColumns, strategy, header),
    [csvNames, tableColumns, strategy, header],
  );
  const mapping = useMemo(() => applyOverrides(matched, overrides), [matched, overrides]);
  const createDefs = defs ?? [];
  const createMapping = createDefs.map((d, i) => (skipped.has(i) ? null : d.name.trim() || null));
  const effective = creating ? createMapping : mapping;
  const matchCount = matchedCount(effective);
  const modes = insertModesFor(driver, creating, upsert);
  const effectiveMode = modes.includes(mode) ? mode : "stopOnError";

  const running = run !== null || guarding;
  const targetLabel = creating ? newName.trim() : (tableName ?? "");
  const canImport =
    !running &&
    data !== undefined &&
    !preview.isFetching &&
    matchCount > 0 &&
    targetLabel !== "" &&
    (creating || meta.isSuccess);

  const close = () => {
    if (!running) closeImport();
  };
  // Escape, a click outside and ✕ ask first once settings were changed; the Cancel button is an
  // explicit choice and still closes at once.
  const guard = useDirtyDialogGuard({
    dirty: edited && !running,
    onClose: close,
    what: "the import settings",
  });

  const start = async () => {
    if (!canImport || !data || starting.current) return;
    starting.current = true;
    try {
      await runImport(data);
    } finally {
      starting.current = false;
    }
  };

  const runImport = async (data: WizardPreview) => {
    setFailure(null);
    setStartError(null);
    const resolved: CsvFormat = {
      ...format,
      delimiter: format.delimiter ?? data.delimiter,
      encoding: format.encoding ?? data.encoding,
    };
    const table: TableRef = creating
      ? { schema, name: newName.trim() }
      : (meta.data?.table ?? target!);
    const columns = createDefs
      .map((d) => ({ ...d, name: d.name.trim() }))
      .filter((_, i) => !skipped.has(i));
    setGuarding(true);
    let allowed: boolean;
    try {
      allowed = await guardExecution(connectionId, {
        kind: "operation",
        label: `Import ${kindLabel} into ${table.name}`,
        write: true,
      });
    } catch (e) {
      setStartError(toIpcError(e).message);
      return;
    } finally {
      setGuarding(false);
    }
    if (!allowed) return;
    if (!json) saveCsvPrefs({ header: format.header, quote: format.quote, emptyAsNull, strategy });
    const importId = crypto.randomUUID();
    setRun({ importId, progress: null, cancelling: false });
    const importTarget = creating
      ? { kind: "create" as const, table, columns }
      : { kind: "existing" as const, table };
    const onProgress = (progress: CsvImportProgress) =>
      setRun((r) => (r?.importId === importId ? { ...r, progress } : r));
    try {
      const report = json
        ? await ipc.imports.json(
            connectionId,
            {
              importId,
              path,
              encoding: resolved.encoding,
              keys: data.keys ?? [],
              target: importTarget,
              mapping: effective,
              mode: effectiveMode,
            },
            onProgress,
          )
        : await ipc.imports.csv(
            connectionId,
            {
              importId,
              path,
              format: resolved,
              target: importTarget,
              mapping: effective,
              emptyAsNull,
              mode: effectiveMode,
            },
            onProgress,
          );
      if (report.rowsImported > 0 || report.createdTable) invalidateSchema(qc, connectionId, table);
      const rows = `${formatCount(report.rowsImported)} row${report.rowsImported === 1 ? "" : "s"}`;
      if (!report.cancelled)
        void notifyInBackground(
          report.failure
            ? { title: `Could not import the ${kindLabel} file`, body: report.failure.message }
            : { title: `Imported ${rows}`, body: `${data.fileName} → ${table.name}` },
        );
      if (report.failure) {
        const notes = report.notes.length > 0 ? `. ${report.notes.join(" ")}` : "";
        setFailure({ text: failureText(report.failure) + notes, imported: report.rowsImported });
      } else if (report.cancelled) {
        toast.message("Import canceled", {
          description:
            report.rowsImported > 0
              ? `${rows} were already imported into ${table.name}.`
              : "Nothing was imported.",
        });
        closeImport();
      } else if (report.rowsSkipped > 0 || report.notes.length > 0) {
        const skippedRows =
          report.rowsSkipped > 0
            ? `, skipped ${formatCount(report.rowsSkipped)} row${report.rowsSkipped === 1 ? "" : "s"}`
            : "";
        toast.warning(`Imported ${rows}${skippedRows}`, {
          description: skippedText(report),
          duration: 15_000,
        });
        closeImport();
      } else {
        toast.success(`Imported ${rows}`, {
          description: `${data.fileName} → ${table.name}${report.createdTable ? " (new table)" : ""}`,
        });
        closeImport();
      }
    } catch (e) {
      const message = toIpcError(e).message;
      void notifyInBackground({ title: `Could not import the ${kindLabel} file`, body: message });
      setStartError(message);
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
  const width = Math.max(csvNames.length, ...(data?.rows ?? []).map((r) => r.length));

  return (
    <Dialog open onOpenChange={guard.onOpenChange}>
      <DialogContent
        className="flex max-h-[min(760px,calc(100dvh-4rem))] flex-col gap-0 p-0 sm:max-w-[min(980px,calc(100%-2rem))]"
        showCloseButton={!running}
        onInteractOutside={(e) => running && e.preventDefault()}
        onEscapeKeyDown={(e) => {
          if (running) e.preventDefault();
          else guard.contentProps.onEscapeKeyDown(e);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            void start();
          }
        }}
      >
        <DialogHeader className="border-border gap-1 border-b px-4 py-3 pr-12">
          <DialogTitle className="flex items-center gap-2 text-sm">
            <FileUp aria-hidden className="text-brand size-4 shrink-0" />
            Import {kindLabel}
          </DialogTitle>
          <DialogDescription className="flex min-w-0 items-center gap-1.5 text-xs">
            <span className="text-foreground truncate font-mono" title={path}>
              {data?.fileName ?? fileNameOf(path)}
            </span>
            {data && <span className="shrink-0">· {formatBytes(data.size)}</span>}
            {json && layoutLabel(data?.layout ?? null) && (
              <span className="shrink-0">· {layoutLabel(data?.layout ?? null)}</span>
            )}
            {data && (
              <span className="shrink-0">
                · {data.more ? "first " : ""}
                {formatCount(data.rows.length)} rows shown
              </span>
            )}
          </DialogDescription>
        </DialogHeader>

        <fieldset
          disabled={running}
          className="border-border grid shrink-0 grid-cols-1 gap-x-6 gap-y-2 border-b px-4 py-3 md:grid-cols-2"
        >
          <div className="flex min-w-0 flex-col gap-2">
            <Field label="Table">
              {creating ? (
                <Input
                  ref={nameInput}
                  aria-label="New table name"
                  value={newName}
                  onChange={(e) => {
                    setEdited(true);
                    setNewName(e.target.value);
                  }}
                  spellCheck={false}
                  autoComplete="off"
                  className="h-7 min-w-0 flex-1 rounded-md font-mono text-xs md:text-xs"
                />
              ) : (
                <Select
                  value={tableName ?? undefined}
                  onValueChange={(v) => {
                    setEdited(true);
                    setTableName(v);
                    setOverrides({});
                  }}
                >
                  <SelectTrigger
                    size="sm"
                    aria-label="Table"
                    className="bg-surface min-w-0 flex-1 font-mono text-xs"
                  >
                    <SelectValue placeholder="Choose a table" />
                  </SelectTrigger>
                  <SelectContent>
                    {tableList.map((t) => (
                      <SelectItem key={t.name} value={t.name} className="font-mono text-xs">
                        {t.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
              <Toggle
                checked={creating}
                onChange={edit((on: boolean) => {
                  focusName.current = on;
                  setCreating(on);
                })}
              >
                Create new table
              </Toggle>
            </Field>
            <Field label="Matching">
              <Choice
                label="Matching strategy"
                value={strategy}
                options={MATCH_STRATEGIES}
                onChange={edit((v: MatchStrategy) => {
                  setStrategy(v);
                  setOverrides({});
                })}
                disabled={creating}
                className="min-w-0 flex-1"
              />
            </Field>
            <Field label="Insert option">
              <Choice
                label="Insert option"
                value={effectiveMode}
                options={INSERT_MODES.filter((m) => modes.includes(m.value))}
                onChange={edit(setMode)}
                disabled={modes.length === 1}
                className="w-52"
              />
            </Field>
          </div>
          <div className="flex min-w-0 flex-col gap-2">
            {json ? (
              <p className="text-muted-foreground flex h-7 items-center text-xs">
                Each key is a column; null and missing keys are NULL.
              </p>
            ) : (
              <div className="flex flex-wrap items-center gap-x-4">
                <Toggle
                  checked={format.header}
                  onChange={edit((header: boolean) => setFormat((f) => ({ ...f, header })))}
                >
                  First line is header
                </Toggle>
                <Toggle checked={emptyAsNull} onChange={edit(setEmptyAsNull)}>
                  Empty unquoted field as NULL
                </Toggle>
              </div>
            )}
            {!json && (
              <Field label="Delimiter">
                <Choice
                  label="Delimiter"
                  value={format.delimiter ?? data?.delimiter ?? ","}
                  options={DELIMITERS}
                  onChange={edit((delimiter: string) => setFormat((f) => ({ ...f, delimiter })))}
                  className="w-36"
                />
                <span className="text-muted-foreground text-xs">Quote</span>
                <Choice
                  label="Quote"
                  value={format.quote}
                  options={QUOTES}
                  onChange={edit((quote: string) => setFormat((f) => ({ ...f, quote })))}
                  className="w-40"
                />
              </Field>
            )}
            <Field label="Encoding">
              <Choice<TextEncoding>
                label="Encoding"
                value={format.encoding ?? data?.encoding ?? "utf8"}
                options={ENCODINGS}
                onChange={edit((encoding: TextEncoding) => setFormat((f) => ({ ...f, encoding })))}
                className="w-48"
              />
            </Field>
          </div>
        </fieldset>

        <div className="min-h-40 flex-1 overflow-auto" data-testid="csv-preview">
          {preview.isError ? (
            <FormError className="p-4">{preview.error.message}</FormError>
          ) : !data ? (
            <QuietStatus delay={400}>Reading the file</QuietStatus>
          ) : (
            <table className="w-max min-w-full border-separate border-spacing-0 text-xs">
              <thead className="bg-surface sticky top-0 z-10">
                <tr>
                  <th className="border-gridline bg-surface-2 sticky left-0 w-10 border-r border-b" />
                  {Array.from({ length: width }, (_, i) => (
                    <th
                      key={i}
                      className="border-gridline min-w-36 border-r border-b px-1.5 py-1 text-left align-top font-normal"
                    >
                      {creating ? (
                        <div className="flex flex-col gap-1">
                          <div className="flex items-center gap-1">
                            <Checkbox
                              aria-label={`Import column ${i + 1}`}
                              checked={!skipped.has(i)}
                              disabled={running || defs?.[i] === undefined}
                              onCheckedChange={edit((v: boolean | "indeterminate") =>
                                setSkipped((s) => {
                                  const next = new Set(s);
                                  if (v === true) next.delete(i);
                                  else next.add(i);
                                  return next;
                                }),
                              )}
                            />
                            <Input
                              aria-label={`Name of column ${i + 1}`}
                              value={defs?.[i]?.name ?? ""}
                              disabled={running || skipped.has(i) || defs?.[i] === undefined}
                              onChange={(e) => {
                                setEdited(true);
                                setDefs((d) =>
                                  d
                                    ? d.map((c, j) =>
                                        j === i ? { ...c, name: e.target.value } : c,
                                      )
                                    : d,
                                );
                              }}
                              spellCheck={false}
                              className="h-6 min-w-0 flex-1 rounded-md font-mono text-xs md:text-xs"
                            />
                          </div>
                          <Choice<ImportType>
                            label={`Type of column ${i + 1}`}
                            value={defs?.[i]?.ty ?? "text"}
                            options={COLUMN_TYPES}
                            disabled={running || skipped.has(i) || defs?.[i] === undefined}
                            onChange={edit((ty: ImportType) =>
                              setDefs((d) =>
                                d ? d.map((c, j) => (j === i ? { ...c, ty } : c)) : d,
                              ),
                            )}
                            className="h-6 w-full font-mono"
                          />
                        </div>
                      ) : (
                        <Select
                          value={mapping[i] ?? SKIP}
                          disabled={running || i >= csvNames.length}
                          onValueChange={edit((v: string) =>
                            setOverrides((o) => ({ ...o, [i]: v === SKIP ? null : v })),
                          )}
                        >
                          <SelectTrigger
                            size="sm"
                            aria-label={`Target of column ${i + 1}`}
                            className={cn(
                              "bg-surface h-6 w-full font-mono text-xs",
                              mapping[i] == null && "text-null italic",
                            )}
                          >
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem value={SKIP} className="text-xs italic">
                              Do not import
                            </SelectItem>
                            <SelectSeparator />
                            {tableColumns.map((c) => (
                              <SelectItem key={c} value={c} className="font-mono text-xs">
                                {c}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      )}
                      {header && (
                        <div
                          className="text-muted-foreground mt-1 truncate px-0.5 font-mono"
                          title={csvNames[i]}
                        >
                          {csvNames[i] ?? ""}
                        </div>
                      )}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {data.rows.map((row, r) => (
                  <tr key={r} className="hover:bg-row-hover">
                    <td className="border-gridline bg-surface-2 text-muted-foreground sticky left-0 border-r border-b px-1.5 text-right tabular-nums">
                      {r + 1}
                    </td>
                    {Array.from({ length: width }, (_, i) => {
                      const v = row[i];
                      const skippedCol = effective[i] == null;
                      return (
                        <td
                          key={i}
                          className={cn(
                            "border-gridline max-w-56 truncate border-r border-b px-1.5 py-0.5 font-mono",
                            skippedCol && "text-muted-foreground",
                          )}
                          title={v ?? undefined}
                        >
                          {v === null && (json || emptyAsNull) ? (
                            <span className="text-null italic">NULL</span>
                          ) : (
                            (v ?? "")
                          )}
                        </td>
                      );
                    })}
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>

        <DialogFooter className="m-0 shrink-0 flex-row items-center">
          <div className="min-w-0 flex-1">
            {run ? (
              <div role="status" aria-live="polite" className="flex min-w-0 flex-col gap-1">
                <div className="flex items-center gap-2 text-xs">
                  <span className="tabular-nums">
                    {progress?.phase === "writing" ? "Writing" : "Reading"} ·{" "}
                    {formatCount(progress?.rowsRead ?? 0)} rows read
                    {progress && progress.rowsImported > 0
                      ? ` · ${formatCount(progress.rowsImported)} imported`
                      : ""}
                    {run.cancelling ? " · canceling…" : ""}
                  </span>
                </div>
                <div className="bg-surface-2 h-1 w-56 overflow-hidden rounded-full">
                  {/* Before the first progress event the size read is not known yet: a pulsing
                      partial fill says the import is under way rather than an empty, stuck bar. */}
                  <div
                    className={cn(
                      "bg-brand h-full rounded-full",
                      pct === null && "w-1/3 animate-pulse",
                    )}
                    style={pct === null ? undefined : { width: `${pct}%` }}
                  />
                </div>
              </div>
            ) : failure || startError ? (
              <FormError clamp title={failure?.text ?? startError ?? ""}>
                {failure ? (
                  <>
                    {failure.text}.{" "}
                    {failure.imported > 0
                      ? `${formatCount(failure.imported)} rows of earlier chunks stay imported.`
                      : "Nothing was imported."}
                  </>
                ) : (
                  startError
                )}
              </FormError>
            ) : (
              <span
                className="text-muted-foreground block truncate text-xs"
                data-testid="csv-status"
              >
                {statusLine(
                  csvNames.length,
                  creating ? createDefs.length : tableColumns.length,
                  matchCount,
                  kindLabel,
                )}
              </span>
            )}
          </div>
          {run ? (
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => void cancel()}
              disabled={run.cancelling}
            >
              Cancel Import
            </Button>
          ) : (
            <>
              <Button type="button" variant="outline" size="sm" onClick={close}>
                Cancel
              </Button>
              <Button type="button" size="sm" disabled={!canImport} onClick={() => void start()}>
                Import
                <KbdHint combo="Mod+Enter" />
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
      {guard.confirm}
    </Dialog>
  );
}
