import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  FileCode2,
  Lock,
  RefreshCw,
  SquareTerminal,
  Table2,
  Undo2,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { toast } from "sonner";
import { EmptyState } from "@/components/EmptyState";
import { Kbd } from "@/components/Kbd";
import { TableIcon } from "@/components/TypeIcon";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { openSqlInNewQueryTab } from "@/features/editor/openSql";
import { useAction } from "@/features/palette/actions";
import { schemaKeys } from "@/features/sidebar/useSchemaTree";
import { formatCount, formatDuration } from "@/lib/format/time";
import {
  ipc,
  tableKey,
  toIpcError,
  type DdlOp,
  type Dialect,
  type TableMeta,
  type TableRef,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { sameTable, useTabsStore, type Tab } from "@/stores/tabs";
import { columnEditError, parseColumnInput, type ColumnField } from "./columnEdits";
import { ColumnsSection } from "./ColumnsSection";
import { DdlPreviewSheet } from "./DdlPreviewSheet";
import { ddlKey, invalidateSchema, reportHeldTables, tablesWithPendingEdits } from "./invalidate";
import { AddForeignKeyDialog, AddIndexDialog } from "./KeyDialogs";
import {
  addColumn,
  addForeignKey,
  addIndex,
  columnNameError,
  dropColumn,
  dropForeignKey,
  dropIndex,
  editColumn,
  foldStructure,
  restoreColumn,
  restoreForeignKey,
  restoreIndex,
  togglePrimaryKey,
} from "./model";
import { ForeignKeysSection, IndexesSection, TriggersSection } from "./OtherSections";
import {
  changesSchema,
  errorInfo,
  payloadInfo,
  PREVIEW_CLOSED,
  type DdlPreviewState,
} from "./previewState";
import { ShowDdlDialog } from "./ShowDdlDialog";
import { stagedOps, useCanUndo, usePartialApply, useStagedOps, useStructureStore } from "./store";
import { isAlterable, KIND_LABEL } from "./tableActions";

type Reducer = (meta: TableMeta, ops: DdlOp[]) => DdlOp[];

function ToolButton({
  label,
  combo,
  onClick,
  disabled,
  className,
  children,
}: {
  label: string;
  combo?: string;
  onClick: () => void;
  disabled?: boolean;
  className?: string;
  children: ReactNode;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={label}
          onClick={onClick}
          disabled={disabled}
          className={className}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent className="flex items-center gap-1.5">
        {label}
        {combo && <Kbd combo={combo} className="border-border/40 bg-transparent text-inherit" />}
      </TooltipContent>
    </Tooltip>
  );
}

/**
 * Table structure editor (TablePlus "Structure" view): columns, indexes, foreign keys and
 * triggers. Edits are staged as DDL ops per tab; ⌘S reviews and applies them.
 */
export function StructureTab({ tab }: { tab: Extract<Tab, { kind: "structure" }> }) {
  const { connectionId, table } = tab;
  const qc = useQueryClient();
  const dialect: Dialect = useConnectionsStore(
    (s) => s.byId[connectionId]?.info.driver ?? "postgres",
  );
  const readOnlyConnection = useConnectionsStore(
    (s) => s.byId[connectionId]?.info.readOnly ?? false,
  );
  const isActive = useTabsStore((s) => s.activeId === tab.id);
  const openTable = useTabsStore((s) => s.openTable);
  const stage = useStructureStore((s) => s.stage);
  const undoStaged = useStructureStore((s) => s.undo);
  const discardStaged = useStructureStore((s) => s.discard);
  const ops = useStagedOps(tab.id);
  const canUndo = useCanUndo(tab.id);
  const partialRan = usePartialApply(tab.id);
  const rootRef = useRef<HTMLDivElement>(null);
  const pendingFocus = useRef<string | null>(null);
  const focusRoot = useCallback(() => rootRef.current?.focus({ preventScroll: true }), []);

  const metaKey = useMemo(
    () => schemaKeys.table(connectionId, table.schema, table.name),
    [connectionId, table.schema, table.name],
  );
  const meta = useQuery({
    queryKey: metaKey,
    queryFn: () => ipc.schema.table(connectionId, table),
    staleTime: 5 * 60_000,
  });
  const view = useMemo(() => (meta.data ? foldStructure(meta.data, ops) : null), [meta.data, ops]);
  const editable = !!meta.data && isAlterable(meta.data.kind) && !readOnlyConnection;

  // Forget the staged ops of tabs that are gone for good. Recently closed tabs keep theirs, so
  // reopening one (⌘⇧T) brings its staged work back.
  useEffect(() => {
    const { tabs, closed } = useTabsStore.getState();
    useStructureStore.getState().retain(new Set([...tabs, ...closed].map((t) => t.id)));
  }, []);

  // Take keyboard focus (so ⌘S / ⌘N / ⌘Z resolve to the grid scope) once the structure is first
  // shown, and again after a rename re-keys it. The root only exists once the metadata loaded,
  // which is after mount whenever it was not cached.
  const hasMeta = meta.data !== undefined;
  const shownKey = metaKey.join("\u0000");
  const focusedFor = useRef<string | null>(null);
  useEffect(() => {
    if (!hasMeta || !isActive || focusedFor.current === shownKey) return;
    focusedFor.current = shownKey;
    const root = rootRef.current;
    if (root && !root.contains(document.activeElement)) root.focus({ preventScroll: true });
  }, [hasMeta, isActive, shownKey]);

  const apply = useCallback(
    (fn: Reducer) => {
      const m = meta.data;
      if (!m || !editable) return;
      stage(tab.id, (current) => fn(m, current));
    },
    [meta.data, editable, stage, tab.id],
  );

  // ---- column edits ---------------------------------------------------------
  const onEdit = useCallback(
    (key: string, field: ColumnField, raw: string | boolean) => {
      const parsed = parseColumnInput(dialect, field, raw);
      if (!parsed.ok) {
        toast.error(parsed.error);
        return;
      }
      const row = view?.columns.find((c) => c.key === key);
      if (view && row) {
        const err =
          (field === "name" ? columnNameError(view, key, String(parsed.value)) : null) ??
          columnEditError(
            dialect,
            {
              added: row.status === "added",
              name: row.name,
              dataType: row.dataType,
              autoIncrement: row.autoIncrement,
            },
            field,
            parsed.value,
          );
        if (err) {
          toast.error(err);
          return;
        }
      }
      apply((m, current) => editColumn(m, current, dialect, key, field, parsed.value));
    },
    [apply, dialect, view],
  );

  const onAddColumn = useCallback(
    (after: string | null = null) => {
      const m = meta.data;
      if (!m || !editable) return;
      stage(tab.id, (current) => {
        const r = addColumn(m, current, dialect, after);
        pendingFocus.current = r.key;
        return r.ops;
      });
    },
    [meta.data, editable, stage, tab.id, dialect],
  );

  useEffect(() => {
    const key = pendingFocus.current;
    if (!key) return;
    pendingFocus.current = null;
    const el = rootRef.current?.querySelector<HTMLInputElement>(`[data-cell-key="${key}:name"]`);
    if (el) {
      el.scrollIntoView?.({ block: "nearest" });
      el.focus();
      el.select();
    }
  }, [view]);

  const dropFocusedColumn = useCallback(() => {
    const row = document.activeElement?.closest<HTMLElement>("[data-column-key]");
    const key = row?.dataset.columnKey;
    if (key) apply((m, current) => dropColumn(m, current, key));
  }, [apply]);

  // ---- review / apply -------------------------------------------------------
  const [preview, setPreview] = useState<DdlPreviewState>(PREVIEW_CLOSED);

  const openPreview = useCallback(async () => {
    const current = stagedOps(tab.id);
    if (current.length === 0) {
      toast.message("No structure changes to apply");
      return;
    }
    setPreview({ ...PREVIEW_CLOSED, open: true, loading: true, ops: current });
    try {
      const plan = await ipc.ddl.preview(connectionId, table, current);
      setPreview((p) => (p.ops === current ? { ...p, loading: false, plan } : p));
    } catch (e) {
      setPreview((p) => (p.ops === current ? { ...p, loading: false, error: errorInfo(e) } : p));
    }
  }, [connectionId, table, tab.id]);

  /** Reloads metadata; with `clear`, drops the staged ops in the same render. */
  const reload = useCallback(
    async (clear: boolean) => {
      try {
        await ipc.schema.invalidate(connectionId, table);
        const fresh = await ipc.schema.table(connectionId, table, true);
        qc.setQueryData(metaKey, fresh);
        if (clear) discardStaged(tab.id);
      } catch {
        if (clear) discardStaged(tab.id);
        void qc.invalidateQueries({ queryKey: metaKey });
      }
      invalidateSchema(qc, connectionId);
      // The table's data tab reloads too, unless it holds staged row edits: they point at columns
      // by position, which the new structure may have moved.
      if (tablesWithPendingEdits(connectionId).some((t) => sameTable(t, table))) {
        reportHeldTables([table]);
      } else {
        void qc.invalidateQueries({ queryKey: ["rows", connectionId, tableKey(table)] });
        void qc.invalidateQueries({ queryKey: ["count", connectionId, tableKey(table)] });
      }
    },
    [connectionId, table, qc, metaKey, discardStaged, tab.id],
  );

  const applyPreview = useCallback(async () => {
    const current = preview.ops;
    if (current.length === 0 || preview.applying) return;
    setPreview((p) => ({ ...p, applying: true, error: null, failedIndex: null, executed: null }));
    try {
      const result = await ipc.ddl.apply(connectionId, table, current);
      if (!result.error) {
        setPreview(PREVIEW_CLOSED);
        const n = result.executed.length;
        toast.success(`Structure updated · ${n} statement${n === 1 ? "" : "s"}`, {
          description: formatDuration(result.durationMs),
        });
        await reload(true);
        rootRef.current?.focus({ preventScroll: true });
        return;
      }
      setPreview((p) => ({
        ...p,
        applying: false,
        error: payloadInfo(result.error!),
        failedIndex: result.failedIndex ?? result.error!.statementIndex ?? null,
        executed: result.executed,
      }));
      // A non-transactional plan (MySQL) that stopped partway left the table changed: remember
      // it beyond the sheet, the staged ops no longer match the table.
      const ran = result.executed.filter(changesSchema).length;
      if (ran > 0) useStructureStore.getState().markPartial(tab.id, ran);
    } catch (e) {
      setPreview((p) => ({ ...p, applying: false, error: errorInfo(e) }));
    }
  }, [preview.ops, preview.applying, connectionId, table, reload, tab.id]);

  const discardAndRefresh = useCallback(() => {
    setPreview(PREVIEW_CLOSED);
    void reload(true);
  }, [reload]);

  const discard = useCallback(() => {
    if (stagedOps(tab.id).length === 0) return;
    // After a partial apply the metadata is stale too: reload it along with the discard.
    if (partialRan > 0) void reload(true);
    else discardStaged(tab.id);
    toast.message("Structure changes discarded");
  }, [discardStaged, tab.id, partialRan, reload]);

  const refresh = useCallback(async () => {
    try {
      await ipc.schema.invalidate(connectionId, table);
    } catch {
      // The refetch below reports connection problems.
    }
    await qc.invalidateQueries({ queryKey: metaKey });
    void qc.invalidateQueries({ queryKey: ddlKey(connectionId, table) });
  }, [connectionId, table, qc, metaKey]);

  const undo = useCallback(() => undoStaged(tab.id), [undoStaged, tab.id]);
  const newColumn = useCallback(() => onAddColumn(), [onAddColumn]);
  const hasOps = ops.length > 0;
  useAction("grid.commit", isActive && editable ? openPreview : null);
  useAction("grid.preview", isActive && editable ? openPreview : null);
  useAction("grid.discard", isActive && hasOps ? discard : null);
  useAction("grid.undo", isActive && canUndo ? undo : null);
  useAction("grid.new-row", isActive && editable ? newColumn : null);
  useAction("grid.delete-rows", isActive && editable ? dropFocusedColumn : null);
  useAction("view.refresh", isActive ? refresh : null);

  // ---- dialogs ---------------------------------------------------------------
  const [showDdl, setShowDdl] = useState(false);

  /** Opens the table's current CREATE statement in a new SQL editor tab. */
  const openDdlInEditor = useCallback(async () => {
    let sql: string;
    try {
      sql = await qc.fetchQuery({
        queryKey: ddlKey(connectionId, table),
        queryFn: () => ipc.ddl.tableDdl(connectionId, table),
      });
    } catch (e) {
      toast.error("Could not load the CREATE statement", { description: toIpcError(e).message });
      return;
    }
    openSqlInNewQueryTab(connectionId, sql, dialect, `${table.name}.sql`);
  }, [qc, connectionId, table, dialect]);
  const [indexOpen, setIndexOpen] = useState(false);
  const [fkOpen, setFkOpen] = useState(false);

  const liveColumns = useMemo(
    () => view?.columns.filter((c) => c.status !== "dropped").map((c) => c.name) ?? [],
    [view],
  );
  const fkColumns = useMemo(
    () =>
      new Set(
        view?.foreignKeys.filter((f) => f.status !== "dropped").flatMap((f) => f.columns) ?? [],
      ),
    [view],
  );
  const openStructure = (ref: TableRef) => openTable(connectionId, ref, "structure");

  if (meta.isPending) {
    return (
      <div className="flex flex-col gap-2 p-4">
        <Skeleton className="h-5 w-48" />
        <Skeleton className="h-64 w-full" />
      </div>
    );
  }
  if (meta.isError)
    return <EmptyState title="Could not load table" description={meta.error.message} />;
  const m = meta.data;

  const qualifiedName = m.table.schema ? `${m.table.schema}.${m.table.name}` : m.table.name;

  // The root is a size container: the header drops labels and secondary buttons as the tab gets
  // narrow (inspector open, wide sidebar) so the table name and "Review" always stay visible.
  return (
    <div
      ref={rootRef}
      tabIndex={-1}
      data-scope="grid"
      className="@container flex h-full min-h-0 flex-col outline-none"
    >
      <div className="border-border bg-surface-2 flex h-9 shrink-0 items-center gap-1.5 overflow-hidden border-b px-2">
        <TableIcon kind={m.kind} />
        {/* When squeezed, the schema prefix gives way first so the table name stays readable. */}
        <span className="flex min-w-12 font-mono text-xs" title={qualifiedName}>
          {m.table.schema && (
            <span className="text-muted-foreground min-w-0 shrink-[1000] truncate @max-lg:hidden">
              {m.table.schema}.
            </span>
          )}
          <span className="min-w-0 truncate font-semibold">{m.table.name}</span>
        </span>
        <span className="border-border text-muted-foreground shrink-0 rounded border px-1 text-[10px] leading-4 @max-xl:hidden">
          {KIND_LABEL[m.kind]}
        </span>
        {m.rowEstimate !== null && (
          <span className="text-muted-foreground shrink-0 text-[11px] tabular-nums @max-2xl:hidden">
            {formatCount(m.rowEstimate, m.rowEstimate > 1000)} rows
          </span>
        )}
        {m.comment && (
          <span className="text-muted-foreground min-w-0 truncate text-[11px] italic @max-2xl:hidden">
            {m.comment}
          </span>
        )}
        <span className="flex-1" />
        {hasOps && (
          <>
            <span
              className="bg-brand-soft text-brand shrink-0 rounded px-1.5 text-[11px] leading-5 font-medium tabular-nums"
              title={`${ops.length} staged change${ops.length === 1 ? "" : "s"}`}
            >
              <span className="@max-lg:hidden">
                {ops.length} change{ops.length === 1 ? "" : "s"}
              </span>
              <span className="@lg:hidden">{ops.length}</span>
            </span>
            <ToolButton
              label="Undo last change"
              combo="Mod+Z"
              onClick={undo}
              disabled={!canUndo}
              className="@max-sm:hidden"
            >
              <Undo2 />
            </ToolButton>
            <ToolButton label="Discard all changes" combo="Mod+Shift+Z" onClick={discard}>
              <X />
            </ToolButton>
            <Button
              size="sm"
              onClick={() => void openPreview()}
              aria-label="Review & apply"
              className="h-6 shrink-0 px-2 text-xs"
            >
              Review<span className="@max-lg:hidden">&amp; apply</span>
              <Kbd
                combo="Mod+S"
                className="border-primary-foreground/30 ml-0.5 bg-transparent text-inherit @max-2xl:hidden"
              />
            </Button>
            <span className="bg-border mx-0.5 h-4 w-px shrink-0 @max-sm:hidden" />
          </>
        )}
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="sm"
              aria-label="Show DDL"
              onClick={() => setShowDdl(true)}
              className="@max-xl:px-2"
            >
              <FileCode2 />
              <span className="@max-xl:hidden">DDL</span>
            </Button>
          </TooltipTrigger>
          <TooltipContent>Show the CREATE statement</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="sm"
              aria-label="Open in SQL"
              onClick={() => void openDdlInEditor()}
              // Its label goes before anything that names the table (wider with staged changes,
              // whose badge and buttons take the room).
              className={cn(hasOps ? "@max-4xl:px-2" : "@max-3xl:px-2")}
            >
              <SquareTerminal />
              <span className={cn(hasOps ? "@max-4xl:hidden" : "@max-3xl:hidden")}>
                Open in SQL
              </span>
            </Button>
          </TooltipTrigger>
          <TooltipContent>Open the CREATE statement in a new SQL editor tab</TooltipContent>
        </Tooltip>
        <ToolButton
          label="Open data"
          onClick={() => openTable(connectionId, table, "table")}
          className="@max-sm:hidden"
        >
          <Table2 />
        </ToolButton>
        <ToolButton
          label="Refresh"
          combo="Mod+R"
          onClick={() => void refresh()}
          disabled={meta.isFetching}
        >
          <RefreshCw className={cn(meta.isFetching && "animate-spin")} />
        </ToolButton>
      </div>
      {!editable && (
        <div className="border-border text-muted-foreground flex shrink-0 items-center gap-1.5 border-b px-3 py-1 text-[11px]">
          <Lock className="size-3" />
          {readOnlyConnection
            ? "Read-only connection: the structure cannot be changed."
            : `${KIND_LABEL[m.kind]}s are shown read-only.`}
        </div>
      )}
      {partialRan > 0 && (
        <div
          role="alert"
          className="border-warning/40 bg-warning/10 text-warning flex shrink-0 items-center gap-2 border-b px-3 py-1 text-[11px]"
        >
          <AlertTriangle className="size-3.5 shrink-0" />
          <span className="min-w-0 flex-1">
            The last apply stopped partway: {partialRan} statement{partialRan === 1 ? "" : "s"}{" "}
            already ran, so the table no longer matches the staged changes.
          </span>
          <Button variant="outline" size="xs" onClick={discardAndRefresh} className="shrink-0">
            Discard &amp; refresh
          </Button>
        </div>
      )}
      <div className="min-h-0 flex-1 overflow-auto">
        {/* Size container as wide as the scroll viewport: section bars span it (100cqw) and
            stick to its left edge while the tables scroll sideways. */}
        <div className="@container">
          <div className="min-w-[640px]">
            <ColumnsSection
              rows={view!.columns}
              dialect={dialect}
              editable={editable}
              fkColumns={fkColumns}
              onEdit={onEdit}
              onTogglePk={(key) => apply((mm, current) => togglePrimaryKey(mm, current, key))}
              onDrop={(key) => apply((mm, current) => dropColumn(mm, current, key))}
              onRestore={(key) => apply((mm, current) => restoreColumn(mm, current, key))}
              onAdd={onAddColumn}
            />
            <IndexesSection
              rows={view!.indexes}
              editable={editable}
              onAdd={() => setIndexOpen(true)}
              onDrop={(key) => apply((mm, current) => dropIndex(mm, current, key))}
              onRestore={(key) => apply((mm, current) => restoreIndex(mm, current, key))}
            />
            <ForeignKeysSection
              rows={view!.foreignKeys}
              editable={editable}
              onAdd={() => setFkOpen(true)}
              onDrop={(key) => apply((mm, current) => dropForeignKey(mm, current, key))}
              onRestore={(key) => apply((mm, current) => restoreForeignKey(mm, current, key))}
              onOpenTable={openStructure}
            />
            <TriggersSection triggers={m.triggers} dialect={dialect} />
          </div>
        </div>
      </div>

      <DdlPreviewSheet
        state={preview}
        dialect={dialect}
        table={m.table}
        onOpenChange={(open) => {
          if (open || preview.applying) return;
          setPreview(PREVIEW_CLOSED);
        }}
        onApply={() => void applyPreview()}
        returnFocus={focusRoot}
        staleRan={partialRan}
        onDiscardAndRefresh={discardAndRefresh}
      />
      <ShowDdlDialog
        open={showDdl}
        onOpenChange={setShowDdl}
        connectionId={connectionId}
        table={m.table}
        dialect={dialect}
        returnFocus={focusRoot}
      />
      {editable && (
        <>
          <AddIndexDialog
            open={indexOpen}
            onOpenChange={setIndexOpen}
            dialect={dialect}
            table={m.table}
            columns={liveColumns}
            existingNames={view!.indexes.filter((x) => x.status !== "dropped").map((x) => x.name)}
            onAdd={(index) => apply((mm, current) => addIndex(mm, current, index))}
            returnFocus={focusRoot}
          />
          <AddForeignKeyDialog
            open={fkOpen}
            onOpenChange={setFkOpen}
            connectionId={connectionId}
            table={m.table}
            columns={liveColumns}
            onAdd={(fk) => apply((mm, current) => addForeignKey(mm, current, fk))}
            returnFocus={focusRoot}
          />
        </>
      )}
    </div>
  );
}
