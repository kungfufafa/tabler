import { useQueries, type UseQueryResult } from "@tanstack/react-query";
import {
  ChevronRight,
  CircleAlert,
  Database,
  Eye,
  FileDown,
  Loader2,
  Search,
  Table2,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { toast } from "sonner";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { useCurrentSchema, useDatabases } from "@/features/sidebar/useSchemaTree";
import { Segmented } from "@/features/settings/controls";
import { ipc, toIpcError, type TableRef, type TableSummary } from "@/lib/ipc";
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
  csvProblem,
  dirname,
  encodingForCharset,
  fileExtension,
  fileFilter,
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
  openExport,
  runExportTables,
  useExportStore,
  type ExportSourceSpec,
  type OpenExportArgs,
} from "./store";
import { useConnectionCharset } from "./useConnectionCharset";
import {
  databaseFileBase,
  databaseState,
  groupChosen,
  hasSeveralSchemas,
  matchTables,
  tableLabel,
  treeTables,
  uniqueDatabaseBases,
  type TreeTable,
} from "./tablesTree";

type TablesSource = Extract<ExportSourceSpec, { kind: "tables" }>;
/** A database of the list; null = the connection has no database level (SQLite). */
type Db = string | null;

/** What to check in a database once its tables are loaded. */
type Pending =
  | { kind: "all" }
  /** Every table of `schema` (all of them when null). */
  | { kind: "schema"; schema: string | null }
  | { kind: "tables"; tables: TableRef[] };

const isView = (t: TableSummary) =>
  t.kind === "view" || t.kind === "materializedView" || t.kind === "systemView";

const NO_TABLES: TreeTable[] = [];

/** The part of each database's query the dialog uses (stable while nothing changes). */
function combineTables(results: ReadonlyArray<UseQueryResult<TableSummary[]>>) {
  return results.map((r) => ({
    data: r.data,
    isPending: r.isPending,
    isError: r.isError,
    isSuccess: r.isSuccess,
    error: r.error,
  }));
}

function pendingKeys(pending: Pending, tables: readonly TreeTable[]): string[] {
  switch (pending.kind) {
    case "all":
      return tables.map((t) => t.key);
    case "schema":
      return tables
        .filter((t) => pending.schema === null || t.table.schema === pending.schema)
        .map((t) => t.key);
    case "tables":
      return tables
        .filter((t) =>
          pending.tables.some(
            (r) => r.name === t.table.name && (r.schema === null || r.schema === t.table.schema),
          ),
        )
        .map((t) => t.key);
  }
}

/**
 * The export dialog for several tables: every database of the connection (PostgreSQL, MySQL)
 * with its tables (loaded when it is opened; the current one is open), checkboxes per table and
 * per database, a search, the format and its options, and where the files go. Tables of several
 * databases go into one folder, a file or folder per database. "Export Single Table…" switches to
 * the single-table export of the one table checked.
 */
export function TablesExportForm({ target }: { target: OpenExportArgs }) {
  const source = target.source as TablesSource;
  const { connectionId } = target;
  const run = useExportStore((s) => s.run);
  const error = useExportStore((s) => s.error);
  const driver = useConnectionsStore((s) => s.byId[connectionId]?.info.driver ?? null);
  const sessionDb = useConnectionsStore(
    (s) => s.byId[connectionId]?.info.currentDatabase ?? s.byId[connectionId]?.info.name ?? null,
  );
  const currentSchema = useCurrentSchema(connectionId);
  const schema = source.schema ?? currentSchema;
  const leveled = driver === "postgres" || driver === "mysql";
  // MySQL: a database is a schema; the current one is the sidebar's.
  const currentDb: Db = !leveled
    ? null
    : driver === "mysql"
      ? (source.schema ?? source.tables[0]?.schema ?? currentSchema ?? sessionDb)
      : sessionDb;
  const dbList = useDatabases(leveled ? connectionId : null);
  const databases: Db[] = useMemo(() => {
    if (!leveled) return [null];
    const names = (dbList.data ?? []).map((d) => d.name);
    const ordered =
      currentDb === null ? names : [currentDb, ...names.filter((n) => n !== currentDb)];
    return ordered;
  }, [leveled, dbList.data, currentDb]);

  const [options, setOptions] = useState<ExportOptions>(() => loadExportOptions());
  // Connection ▸ View Using Encoding: the files follow it until the encoding is chosen here (and
  // the remembered choice is not overwritten by it).
  const rememberedEncoding = useRef(options.encoding);
  const charset = useConnectionCharset(connectionId);
  const followed = encodingForCharset(charset);
  const [encodingTouched, setEncodingTouched] = useState(false);
  useEffect(() => {
    if (followed && !encodingTouched)
      setOptions((o) => (o.encoding === followed ? o : { ...o, encoding: followed }));
  }, [followed, encodingTouched]);
  const following = followed !== null && !encodingTouched && options.encoding === followed;
  const [batchText, setBatchText] = useState(() => String(options.sql.batchSize));
  const [search, setSearch] = useState("");
  const [expanded, setExpanded] = useState<Set<Db>>(() => new Set([currentDb]));
  // Every database ever opened: their tables stay loaded (and checked) when they are closed.
  const [opened, setOpened] = useState<Db[]>(() => [currentDb]);
  const [checked, setChecked] = useState<Set<string>>(() => new Set());
  const [pending, setPending] = useState<Map<Db, Pending>>(
    () =>
      new Map([
        [
          currentDb,
          source.all
            ? { kind: "schema", schema: driver === "postgres" ? schema : null }
            : { kind: "tables", tables: source.tables },
        ],
      ]),
  );
  const [picking, setPicking] = useState(false);

  const queries = useQueries({
    queries: opened.map((db) => ({
      queryKey: ["export", "databaseTables", connectionId, db] as const,
      queryFn: () => ipc.exports.databaseTables(connectionId, db),
      staleTime: 60_000,
      retry: false,
    })),
    combine: combineTables,
  });
  const loaded = useMemo(() => {
    const map = new Map<Db, TreeTable[]>();
    opened.forEach((db, i) => {
      const data = queries[i]?.data;
      if (data) map.set(db, treeTables(db, data));
    });
    return map;
  }, [opened, queries]);

  // Check what was asked for once a database's tables are in.
  useEffect(() => {
    if (pending.size === 0) return;
    const ready = [...pending].filter(([db]) => loaded.has(db));
    if (ready.length === 0) return;
    setChecked((c) => {
      const next = new Set(c);
      for (const [db, p] of ready)
        for (const k of pendingKeys(p, loaded.get(db) ?? [])) next.add(k);
      return next;
    });
    setPending((p) => {
      const next = new Map(p);
      for (const [db] of ready) next.delete(db);
      return next;
    });
  }, [pending, loaded]);

  const open = (db: Db) => {
    setExpanded((e) => new Set(e).add(db));
    setOpened((o) => (o.includes(db) ? o : [...o, db]));
  };
  const toggleOpen = (db: Db) =>
    expanded.has(db)
      ? setExpanded((e) => {
          const next = new Set(e);
          next.delete(db);
          return next;
        })
      : open(db);

  const searching = search.trim() !== "";
  const shownOf = (db: Db) => matchTables(loaded.get(db) ?? NO_TABLES, search);
  const shownDbs = databases.filter(
    (db) =>
      !searching ||
      (db ?? "").toLowerCase().includes(search.trim().toLowerCase()) ||
      shownOf(db).length > 0,
  );
  const shownTables = shownDbs.flatMap((db) =>
    expanded.has(db) || searching ? shownOf(db) : NO_TABLES,
  );
  const shownChecked = shownTables.filter((t) => checked.has(t.key)).length;
  const allState: boolean | "indeterminate" =
    shownTables.length > 0 && shownChecked === shownTables.length
      ? true
      : shownChecked > 0
        ? "indeterminate"
        : false;

  const setKeys = (keys: readonly string[], on: boolean) =>
    setChecked((c) => {
      const next = new Set(c);
      for (const k of keys) {
        if (on) next.add(k);
        else next.delete(k);
      }
      return next;
    });
  const toggleDatabase = (db: Db, on: boolean) => {
    const tables = loaded.get(db);
    if (tables) {
      setKeys(
        tables.map((t) => t.key),
        on,
      );
      return;
    }
    setPending((p) => {
      const next = new Map(p);
      if (on) next.set(db, { kind: "all" });
      else next.delete(db);
      return next;
    });
    if (on) open(db);
  };

  const groups = groupChosen(databases, loaded, checked, currentDb);
  const chosen = groups.flatMap((g) => g.tables);
  const severalDbs = groups.length > 1;
  const loadedCount = [...loaded.values()].reduce((n, l) => n + l.length, 0);

  const running = run !== null;
  const layout = multiLayout(options);
  const nothing =
    options.format === "sqlInsert" &&
    !options.sql.content &&
    !options.sql.structure &&
    !options.sql.dropTable;
  const csvIssue = options.format === "csv" ? csvProblem(options.csv) : null;
  const canSubmit = !running && !picking && chosen.length > 0 && !nothing && csvIssue === null;
  // Another PostgreSQL database than the session's: read through a session of its own.
  const dbParam = (db: Db) => (db === currentDb ? null : db);
  const single = groups.length === 1 && groups[0]!.tables.length === 1 ? groups[0]! : null;
  const singleTable = single && single.database === currentDb ? single.tables[0]! : null;

  const fileBase = (db: Db) =>
    db !== null && db !== currentDb
      ? databaseFileBase(db)
      : safeFileBase(schema && schema !== "public" ? schema : (currentDb ?? sessionDb ?? "export"));

  const submit = async () => {
    if (!canSubmit) return;
    const finalOptions: ExportOptions = {
      ...options,
      sql: { ...options.sql, batchSize: clampBatchSize(Number.parseInt(batchText, 10)) },
    };
    const oneFile = multiLayout(finalOptions) === "singleFile";
    const ext = fileExtension(finalOptions);
    setPicking(true);
    let destination: string | null;
    try {
      destination =
        oneFile && !severalDbs
          ? await pickExportPath(
              `${fileBase(groups[0]!.database)}_${timestamp(new Date())}.${ext}`,
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
    saveExportOptions(
      following ? { ...finalOptions, encoding: rememberedEncoding.current } : finalOptions,
    );
    const dir = oneFile && !severalDbs ? dirname(destination) : destination;
    if (dir) saveLastDir(dir);
    const stamp = timestamp(new Date());
    // One distinct name per database: names that reduce to the same file name (`my db` / `my_db`,
    // `Sales` / `sales`) must not write over each other.
    const bases = uniqueDatabaseBases(groups.map((g) => g.database ?? "export"));
    let reqs;
    try {
      reqs = groups.map((g, i) =>
        buildExportTablesRequest({
          exportId: crypto.randomUUID(),
          tables: g.tables,
          options: finalOptions,
          // Several databases: a file (or a folder of files) per database in the chosen folder.
          target: !severalDbs
            ? destination
            : oneFile
              ? `${destination}/${bases[i]}_${stamp}.${ext}`
              : `${destination}/${bases[i]}`,
          database: dbParam(g.database),
          createFolder: severalDbs && !oneFile,
        }),
      );
    } catch (e) {
      useExportStore.setState({ error: toIpcError(e).message });
      return;
    }
    void runExportTables(connectionId, reqs, {
      labels: groups.map((g) => ({ database: g.database })),
    });
  };

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    void submit();
  };

  const layoutRow = (options.format === "sqlInsert" ||
    options.format === "xlsx" ||
    options.format === "html") && (
    <Row
      id="export-layout"
      label="Files"
      hint={
        layout === "singleFile"
          ? options.format === "xlsx"
            ? "One workbook with a sheet per table"
            : options.format === "html"
              ? "One page with a table per table"
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
                : o.format === "html"
                  ? { ...o.multi, htmlSingleFile: v === "one" }
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

  const tableRow = (t: TreeTable, multiSchema: boolean) => {
    const Icon = isView(t.summary) ? Eye : Table2;
    const label = tableLabel(t, multiSchema);
    return (
      <li key={t.key}>
        <label
          className={cn(
            "hover:bg-accent/60 flex h-7 cursor-default items-center gap-2 pr-2 text-xs",
            leveled ? "pl-8" : "pl-2",
          )}
        >
          <Checkbox
            checked={checked.has(t.key)}
            onCheckedChange={(v) => setKeys([t.key], v === true)}
          />
          <Icon className="text-muted-foreground size-3.5 shrink-0" />
          <span className="truncate font-mono">{label}</span>
          {t.summary.rowEstimate !== null && (
            <span className="text-muted-foreground ml-auto shrink-0 tabular-nums">
              ~{formatCount(t.summary.rowEstimate)}
            </span>
          )}
        </label>
      </li>
    );
  };

  const tablesOf = (db: Db) => {
    const i = opened.indexOf(db);
    const q = i >= 0 ? queries[i] : undefined;
    const all = loaded.get(db) ?? NO_TABLES;
    const shown = matchTables(all, search);
    const multiSchema = hasSeveralSchemas(all);
    return (
      <ul
        className={leveled ? undefined : "py-0.5"}
        aria-label={leveled ? `Tables of ${db}` : "Tables"}
      >
        {q?.isPending && (
          <li className="text-muted-foreground flex items-center gap-2 py-1.5 pl-8 text-xs">
            <Loader2 className="size-3.5 animate-spin" /> Loading tables…
          </li>
        )}
        {q?.isError && (
          <li className={cn("text-error py-1.5 pr-2 text-xs", leveled ? "pl-8" : "pl-2")}>
            {toIpcError(q.error).message}
          </li>
        )}
        {q?.isSuccess && shown.length === 0 && (
          <li
            className={cn("text-muted-foreground py-1.5 pr-2 text-xs", leveled ? "pl-8" : "pl-2")}
          >
            {all.length === 0 ? "No tables." : "No table matches."}
          </li>
        )}
        {shown.map((t) => tableRow(t, multiSchema))}
      </ul>
    );
  };

  const title = severalDbs
    ? `${formatCount(groups.length)} databases`
    : leveled
      ? (groups[0]?.database ?? currentDb ?? "—")
      : (schema ?? "—");

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
          <span className="text-foreground truncate font-mono">{title}</span>
          <span className="shrink-0">
            · {formatCount(chosen.length)} of {formatCount(loadedCount)} table
            {loadedCount === 1 ? "" : "s"}
          </span>
        </DialogDescription>
      </DialogHeader>

      <fieldset disabled={running} className="flex min-w-0 flex-col gap-3 px-4 py-4">
        <div className="border-border flex flex-col overflow-hidden rounded-md border">
          <div className="border-border bg-surface-2/60 flex items-center gap-2 border-b px-2 py-1">
            <Checkbox
              aria-label="Select all tables"
              checked={allState}
              onCheckedChange={(v) =>
                setKeys(
                  shownTables.map((t) => t.key),
                  v === true,
                )
              }
            />
            <div className="relative min-w-0 flex-1">
              <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-1.5 size-3.5 -translate-y-1/2" />
              <Input
                aria-label="Search tables"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder={leveled ? "Search tables and databases" : "Search tables"}
                spellCheck={false}
                className="h-6 border-0 bg-transparent pl-6 text-xs shadow-none focus-visible:ring-0"
              />
            </div>
          </div>
          <div className="max-h-56 min-h-16 overflow-y-auto py-0.5">
            {leveled ? (
              <ul aria-label="Databases">
                {dbList.isError && (
                  <li className="text-error px-2 py-1.5 text-xs">
                    {toIpcError(dbList.error).message}
                  </li>
                )}
                {shownDbs.map((db) => {
                  const isOpen = expanded.has(db) || (searching && shownOf(db).length > 0);
                  const tables = loaded.get(db);
                  const n = tables?.filter((t) => checked.has(t.key)).length ?? 0;
                  return (
                    <li key={db ?? ""}>
                      <div className="hover:bg-accent/60 flex h-7 items-center gap-1 pr-2 text-xs">
                        <button
                          type="button"
                          aria-label={`${isOpen ? "Close" : "Open"} ${db}`}
                          aria-expanded={isOpen}
                          onClick={() => toggleOpen(db)}
                          className="text-muted-foreground hover:text-foreground flex size-6 shrink-0 items-center justify-center"
                        >
                          <ChevronRight
                            className={cn("size-3.5 transition-transform", isOpen && "rotate-90")}
                          />
                        </button>
                        <Checkbox
                          aria-label={`Select all tables of ${db}`}
                          checked={databaseState(tables, checked, pending.get(db)?.kind === "all")}
                          onCheckedChange={(v) => toggleDatabase(db, v === true)}
                        />
                        <Database className="text-muted-foreground ml-1 size-3.5 shrink-0" />
                        <span
                          className="min-w-0 cursor-default truncate font-mono"
                          onDoubleClick={() => toggleOpen(db)}
                        >
                          {db}
                        </span>
                        {db === currentDb && (
                          <span className="text-muted-foreground shrink-0 text-[10px]">
                            current
                          </span>
                        )}
                        {tables && (
                          <span className="text-muted-foreground ml-auto shrink-0 tabular-nums">
                            {n > 0 ? `${formatCount(n)} / ` : ""}
                            {formatCount(tables.length)}
                          </span>
                        )}
                      </div>
                      {isOpen && tablesOf(db)}
                    </li>
                  );
                })}
              </ul>
            ) : (
              tablesOf(null)
            )}
          </div>
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
          encodingFollows={following ? `Follows View Using Encoding (${charset})` : undefined}
          onEncodingChange={() => setEncodingTouched(true)}
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
                  <span className="text-foreground font-mono">
                    {run.table.database ? `${run.table.database} · ` : ""}
                    {run.table.name}
                  </span>
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
          ) : singleTable ? (
            <Button
              type="button"
              variant="link"
              size="xs"
              className="h-auto px-0 text-[11px]"
              title="Export this table with a filter, a column choice or the current page"
              onClick={() =>
                openExport({ connectionId, source: { kind: "table", table: singleTable } })
              }
            >
              Export Single Table…
            </Button>
          ) : (
            <span className="text-muted-foreground block truncate text-[11px]">
              {chosen.length === 0
                ? "Choose the tables to export"
                : severalDbs
                  ? `${formatCount(chosen.length)} tables of ${formatCount(groups.length)} databases into a folder, ${layout === "singleFile" ? "a file" : "a folder"} per database`
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
