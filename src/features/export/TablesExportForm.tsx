import { CircleAlert, Eye, FileDown, Loader2, Search, Table2 } from "lucide-react";
import { useMemo, useState, type FormEvent } from "react";
import { toast } from "sonner";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { useCurrentSchema, useTables } from "@/features/sidebar/useSchemaTree";
import { Segmented } from "@/features/settings/controls";
import { toIpcError, type TableRef, type TableSummary } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { Progress } from "./ExportDialog";
import { pickExportFolder, pickExportPath } from "./destination";
import { FormatOptions, Row } from "./formParts";
import {
  BATCH_SIZE_RANGE,
  FORMATS,
  buildExportTablesRequest,
  clampBatchSize,
  dirname,
  fileExtension,
  fileFilter,
  filterTables,
  formatCount,
  loadExportOptions,
  loadLastDir,
  multiLayout,
  safeFileBase,
  saveExportOptions,
  saveLastDir,
  timestamp,
  type ExportOptions,
  type FormatKind,
} from "./options";
import {
  cancelExport,
  closeExport,
  runExportTables,
  useExportStore,
  type ExportSourceSpec,
  type OpenExportArgs,
} from "./store";

type TablesSource = Extract<ExportSourceSpec, { kind: "tables" }>;

const isView = (t: TableSummary) =>
  t.kind === "view" || t.kind === "materializedView" || t.kind === "systemView";

/**
 * The export dialog for several tables: the schema's tables with checkboxes (search, select
 * all), the format and its options, and where the files go (one file, or one per table).
 */
export function TablesExportForm({ target }: { target: OpenExportArgs }) {
  const source = target.source as TablesSource;
  const { connectionId } = target;
  const run = useExportStore((s) => s.run);
  const error = useExportStore((s) => s.error);
  const current = useCurrentSchema(connectionId);
  const schema = source.schema ?? current;
  const tables = useTables(connectionId, schema);
  const database = useConnectionsStore(
    (s) => s.byId[connectionId]?.info.currentDatabase ?? s.byId[connectionId]?.info.name ?? null,
  );
  const [options, setOptions] = useState<ExportOptions>(() => loadExportOptions());
  const [batchText, setBatchText] = useState(() => String(options.sql.batchSize));
  const [search, setSearch] = useState("");
  // Null until chosen: every table (`all`) or the ones passed.
  const [picked, setPicked] = useState<Set<string> | null>(() =>
    source.all ? null : new Set(source.tables.map((t) => t.name)),
  );
  const [picking, setPicking] = useState(false);

  const all = useMemo(() => tables.data ?? [], [tables.data]);
  const selected = useMemo(() => picked ?? new Set(all.map((t) => t.name)), [picked, all]);
  const shown = useMemo(() => filterTables(all, search), [all, search]);
  const chosen: TableRef[] = all
    .filter((t) => selected.has(t.name))
    .map((t) => ({ schema: t.schema ?? schema, name: t.name }));
  const shownChecked = shown.filter((t) => selected.has(t.name)).length;
  const allState: boolean | "indeterminate" =
    shown.length > 0 && shownChecked === shown.length
      ? true
      : shownChecked > 0
        ? "indeterminate"
        : false;

  const toggle = (name: string, on: boolean) =>
    setPicked(() => {
      const next = new Set(selected);
      if (on) next.add(name);
      else next.delete(name);
      return next;
    });
  const toggleShown = (on: boolean) =>
    setPicked(() => {
      const next = new Set(selected);
      for (const t of shown) {
        if (on) next.add(t.name);
        else next.delete(t.name);
      }
      return next;
    });

  const running = run !== null;
  const layout = multiLayout(options);
  const nothing =
    options.format === "sqlInsert" &&
    !options.sql.content &&
    !options.sql.structure &&
    !options.sql.dropTable;
  const canSubmit = !running && !picking && chosen.length > 0 && !nothing;
  const base = safeFileBase(schema && schema !== "public" ? schema : (database ?? "export"));

  const submit = async () => {
    if (!canSubmit) return;
    const finalOptions: ExportOptions = {
      ...options,
      sql: { ...options.sql, batchSize: clampBatchSize(Number.parseInt(batchText, 10)) },
    };
    setPicking(true);
    let destination: string | null;
    try {
      destination =
        multiLayout(finalOptions) === "singleFile"
          ? await pickExportPath(
              `${base}_${timestamp(new Date())}.${fileExtension(finalOptions)}`,
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
    saveExportOptions(finalOptions);
    const dir = multiLayout(finalOptions) === "singleFile" ? dirname(destination) : destination;
    if (dir) saveLastDir(dir);
    let req;
    try {
      req = buildExportTablesRequest({
        exportId: crypto.randomUUID(),
        tables: chosen,
        options: finalOptions,
        target: destination,
      });
    } catch (e) {
      useExportStore.setState({ error: toIpcError(e).message });
      return;
    }
    void runExportTables(connectionId, req);
  };

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    void submit();
  };

  const layoutRow = (options.format === "sqlInsert" || options.format === "xlsx") && (
    <Row
      id="export-layout"
      label="Files"
      hint={
        layout === "singleFile"
          ? options.format === "xlsx"
            ? "One workbook with a sheet per table"
            : "One script with every table, parents before the tables that reference them"
          : `One .${fileExtension(options)} file per table in the folder you choose`
      }
    >
      <Segmented<"one" | "each">
        id="export-layout"
        value={layout === "singleFile" ? "one" : "each"}
        onChange={(v) =>
          setOptions((o) => ({
            ...o,
            multi:
              o.format === "xlsx"
                ? { ...o.multi, xlsxSingleFile: v === "one" }
                : { ...o.multi, sqlSingleFile: v === "one" },
          }))
        }
        options={[
          { value: "one", label: "One file" },
          { value: "each", label: "One file per table" },
        ]}
      />
    </Row>
  );

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
          <span className="text-foreground truncate font-mono">{schema ?? "—"}</span>
          <span className="shrink-0">
            · {formatCount(chosen.length)} of {formatCount(all.length)} table
            {all.length === 1 ? "" : "s"}
          </span>
        </DialogDescription>
      </DialogHeader>

      <fieldset disabled={running} className="flex min-w-0 flex-col gap-3 px-4 py-4">
        <div className="border-border flex flex-col overflow-hidden rounded-md border">
          <div className="border-border bg-surface-2/60 flex items-center gap-2 border-b px-2 py-1">
            <Checkbox
              aria-label="Select all tables"
              checked={allState}
              onCheckedChange={(v) => toggleShown(v === true)}
            />
            <div className="relative min-w-0 flex-1">
              <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-1.5 size-3.5 -translate-y-1/2" />
              <Input
                aria-label="Search tables"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="Search tables"
                spellCheck={false}
                className="h-6 border-0 bg-transparent pl-6 text-xs shadow-none focus-visible:ring-0"
              />
            </div>
          </div>
          <ul className="max-h-48 min-h-16 overflow-y-auto py-0.5" aria-label="Tables">
            {tables.isPending && (
              <li className="text-muted-foreground flex items-center gap-2 px-2 py-1.5 text-xs">
                <Loader2 className="size-3.5 animate-spin" /> Loading tables…
              </li>
            )}
            {tables.isError && (
              <li className="text-error px-2 py-1.5 text-xs">{tables.error.message}</li>
            )}
            {tables.isSuccess && shown.length === 0 && (
              <li className="text-muted-foreground px-2 py-1.5 text-xs">
                {all.length === 0 ? "This schema has no tables." : "No table matches."}
              </li>
            )}
            {shown.map((t) => {
              const Icon = isView(t) ? Eye : Table2;
              return (
                <li key={t.name}>
                  <label className="hover:bg-accent/60 flex h-7 cursor-default items-center gap-2 px-2 text-xs">
                    <Checkbox
                      checked={selected.has(t.name)}
                      onCheckedChange={(v) => toggle(t.name, v === true)}
                    />
                    <Icon className="text-muted-foreground size-3.5 shrink-0" />
                    <span className="truncate font-mono">{t.name}</span>
                    {t.rowEstimate !== null && (
                      <span className="text-muted-foreground ml-auto shrink-0 tabular-nums">
                        ~{formatCount(t.rowEstimate)}
                      </span>
                    )}
                  </label>
                </li>
              );
            })}
          </ul>
        </div>

        <Row id="export-format" label="Format">
          <Segmented<FormatKind>
            id="export-format"
            value={options.format}
            onChange={(format) => setOptions((o) => ({ ...o, format }))}
            options={FORMATS}
          />
        </Row>
        {layoutRow}
        <FormatOptions
          options={options}
          setOptions={setOptions}
          tableSource
          sqlExtra={
            options.sql.content && (
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
            )
          }
        />
        {nothing && (
          <div className="text-warning text-[11px] leading-4">
            Nothing to export: include the table structure or its content.
          </div>
        )}
      </fieldset>

      <div className="border-border bg-surface-2/60 flex min-h-12 items-center gap-2 rounded-b-xl border-t px-4 py-2">
        <div className="min-w-0 flex-1">
          {run ? (
            <div className="flex min-w-0 flex-col gap-0.5">
              {run.table && (
                <span className="text-muted-foreground truncate text-[11px]">
                  Table {run.table.index + 1} of {run.table.count}:{" "}
                  <span className="text-foreground font-mono">{run.table.name}</span>
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
          ) : (
            <span className={cn("text-muted-foreground block truncate text-[11px]")}>
              {chosen.length === 0
                ? "Choose the tables to export"
                : layout === "singleFile"
                  ? `${formatCount(chosen.length)} tables into one .${fileExtension(options)} file`
                  : `${formatCount(chosen.length)} files into a folder`}
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
              {picking ? <Loader2 className="animate-spin" /> : null}
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
