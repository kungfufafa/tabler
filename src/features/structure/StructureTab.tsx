import { useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, FileCode2, Lock, RefreshCw, SquareTerminal, Undo2, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { toast } from "sonner";
import { EmptyState } from "@/components/EmptyState";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { openSqlInNewQueryTab } from "@/features/editor/openSql";
import { useAction } from "@/features/palette/actions";
import { schemaKeys } from "@/features/sidebar/useSchemaTree";
import {
  BarIconButton,
  BarSeparator,
  BarTooltip,
  BottomBarFrame,
} from "@/features/table-tab/BarParts";
import { ViewSwitch } from "@/features/tabs/ViewSwitch";
import { formatDuration } from "@/lib/format/time";
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

/**
 * The bottom bar of the structure view: the same strip as the data view's, with the Data |
 * Structure switch on the left and the structure's own actions after it.
 */
function BottomBar({ tabId, children }: { tabId: string; children?: ReactNode }) {
  return (
    <BottomBarFrame aria-label="Table structure">
      <ViewSwitch tabId={tabId} view="structure" />
      {children}
    </BottomBarFrame>
  );
}

const plural = (n: number, word: string, many = `${word}s`) =>
  `${n.toLocaleString()} ${n === 1 ? word : many}`;

/**
 * Table structure editor (TablePlus "Structure" view of a table's tab): columns, indexes, foreign
 * keys and triggers. Edits are staged as DDL ops per tab; ⌘S reviews and applies them.
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
  const setView = useTabsStore((s) => s.setView);
  const pin = useTabsStore((s) => s.pin);
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
  const showData = useCallback(() => setView(tab.id, "table"), [setView, tab.id]);
  const hasOps = ops.length > 0;
  // Editing pins a preview tab (TablePlus), so the next table opened from the sidebar gets a tab of
  // its own instead of replacing this one.
  const isPreview = tab.preview === true;
  useEffect(() => {
    if (hasOps && isPreview) pin(tab.id);
  }, [hasOps, isPreview, pin, tab.id]);
  useAction("grid.commit", isActive && editable ? openPreview : null);
  useAction("grid.preview", isActive && editable ? openPreview : null);
  useAction("grid.discard", isActive && hasOps ? discard : null);
  useAction("grid.undo", isActive && canUndo ? undo : null);
  useAction("grid.new-row", isActive && editable ? newColumn : null);
  useAction("grid.delete-rows", isActive && editable ? dropFocusedColumn : null);
  useAction("view.refresh", isActive ? refresh : null);
  useAction("view.data", isActive ? showData : null);

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

  // The view switch stays reachable while the metadata loads or failed to.
  if (meta.isPending) {
    return (
      <div className="flex h-full min-h-0 flex-col">
        <div className="flex min-h-0 flex-1 flex-col gap-2 p-4">
          <Skeleton className="h-5 w-48" />
          <Skeleton className="h-64 w-full" />
        </div>
        <BottomBar tabId={tab.id} />
      </div>
    );
  }
  if (meta.isError) {
    return (
      <div className="flex h-full min-h-0 flex-col">
        <div className="min-h-0 flex-1">
          <EmptyState title="Could not load table" description={meta.error.message} />
        </div>
        <BottomBar tabId={tab.id} />
      </div>
    );
  }
  const m = meta.data;

  const qualifiedName = m.table.schema ? `${m.table.schema}.${m.table.name}` : m.table.name;
  const live = <T extends { status: string }>(rows: T[]) =>
    rows.filter((r) => r.status !== "dropped").length;
  const summary = [
    ...(m.kind === "table" ? [] : [KIND_LABEL[m.kind]]),
    plural(live(view!.columns), "column"),
    plural(live(view!.indexes), "index", "indexes"),
    ...(view!.foreignKeys.length > 0 ? [plural(live(view!.foreignKeys), "foreign key")] : []),
  ].join(" · ");

  return (
    <div
      ref={rootRef}
      tabIndex={-1}
      data-scope="grid"
      className="flex h-full min-h-0 flex-col outline-none"
    >
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
      <BottomBar tabId={tab.id}>
        <div className="flex min-w-0 flex-1 justify-center px-1">
          <span
            className="text-foreground/80 min-w-0 truncate whitespace-nowrap tabular-nums"
            title={m.comment ? `${qualifiedName} — ${m.comment}` : qualifiedName}
          >
            {summary}
          </span>
        </div>
        {hasOps && (
          <>
            <span
              className="bg-brand-soft text-brand shrink-0 rounded px-1.5 text-[11px] leading-5 font-medium whitespace-nowrap tabular-nums"
              title={`${plural(ops.length, "staged change")}`}
            >
              {plural(ops.length, "change")}
            </span>
            <BarIconButton
              label="Undo last change"
              action="grid.undo"
              onClick={undo}
              disabled={!canUndo}
            >
              <Undo2 />
            </BarIconButton>
            <BarIconButton label="Discard all changes" action="grid.discard" onClick={discard}>
              <X />
            </BarIconButton>
            <BarTooltip label="Review and apply the changes" action="grid.commit">
              <Button
                size="xs"
                onClick={() => void openPreview()}
                aria-label="Review & apply"
                className="h-6 shrink-0 px-2"
              >
                Review
              </Button>
            </BarTooltip>
            <BarSeparator />
          </>
        )}
        <BarIconButton
          label="Show DDL"
          tooltip="Show the CREATE statement"
          onClick={() => setShowDdl(true)}
        >
          <FileCode2 />
        </BarIconButton>
        <BarIconButton
          label="Open in SQL"
          tooltip="Open the CREATE statement in a new SQL editor tab"
          onClick={() => void openDdlInEditor()}
        >
          <SquareTerminal />
        </BarIconButton>
        <BarIconButton
          label="Refresh"
          action="view.refresh"
          onClick={() => void refresh()}
          disabled={meta.isFetching}
        >
          <RefreshCw className={cn(meta.isFetching && "animate-spin")} />
        </BarIconButton>
      </BottomBar>

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
