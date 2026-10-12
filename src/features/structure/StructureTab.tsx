import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  Lock,
  Plus,
  Redo2,
  RefreshCw,
  Search,
  SquareTerminal,
  Undo2,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { toast } from "sonner";
import { EmptyState } from "@/components/EmptyState";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { openSqlInNewQueryTab } from "@/features/editor/openSql";
import { useAction } from "@/features/palette/actions";
import { guardExecution } from "@/features/safety/guard";
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
  type FkDef,
  type IndexDef,
  type TableMeta,
  type TableRef,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { sameTable, useTabsStore, type Tab } from "@/stores/tabs";
import { columnEditError, parseColumnInput, type ColumnField } from "./columnEdits";
import { ColumnsSection, type ColumnHandlers } from "./ColumnsSection";
import { DdlPreviewSheet } from "./DdlPreviewSheet";
import { constantDefault, type DefaultPreset } from "./defaults";
import { focusCell } from "./dom";
import { ddlKey, invalidateSchema, reportHeldTables, tablesWithPendingEdits } from "./invalidate";
import { AddForeignKeyDialog } from "./KeyDialogs";
import {
  addColumn,
  addForeignKey,
  addIndex,
  columnNameError,
  dropColumn,
  dropForeignKey,
  dropIndex,
  editColumn,
  editIndex,
  entryBase,
  checkColumns,
  foldStructure,
  replaceForeignKey,
  restoreColumn,
  restoreForeignKey,
  restoreIndex,
  setPrimaryKey,
  type StructureView,
} from "./model";
import {
  addCheck,
  addTrigger,
  dropCheck,
  dropTrigger,
  editCheck,
  editTrigger,
  restoreCheck,
  restoreTrigger,
  setColumnCheck,
  setTableComment,
  setTableName,
  setTableOption,
  stagedRename,
  suggestCheckName,
  suggestTriggerName,
  triggerSource,
  triggerTemplate,
  withRenameLast,
  type TableOptionField,
} from "./objects";
import {
  ChecksSection,
  ForeignKeysSection,
  IndexesSection,
  TriggersSection,
} from "./OtherSections";
import {
  changesSchema,
  errorInfo,
  payloadInfo,
  PREVIEW_CLOSED,
  type DdlPreviewState,
} from "./previewState";
import { followRename } from "./rename";
import { ShowDdlDialog } from "./ShowDdlDialog";
import {
  stagedOps,
  useCanRedo,
  useCanUndo,
  usePartialApply,
  useStagedOps,
  useStructureStore,
} from "./store";
import { StructureHeader } from "./StructureHeader";
import { isAlterable, isViewKind, KIND_LABEL, suggestIndexName } from "./tableActions";
import { TriggerEditorDialog, type TriggerEditing } from "./TriggerEditorDialog";
import { ViewDefinitionPane } from "./ViewDefinitionPane";

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

/** A free index name (`<table>_<cols>_idx`, numbered when taken). */
function freeIndexName(view: StructureView, table: string, columns: string[], unique: boolean) {
  const base = suggestIndexName(table, columns, unique);
  const taken = new Set(view.indexes.map((i) => i.name.toLowerCase()));
  let name = base;
  for (let n = 1; taken.has(name.toLowerCase()); n++) name = `${base.slice(0, 60)}${n}`;
  return name;
}

/** The column (current name) whose row holds the focus, if any. */
function focusedColumn(view: StructureView): string | null {
  const key = document.activeElement?.closest<HTMLElement>("[data-column-key]")?.dataset.columnKey;
  return view.columns.find((c) => c.key === key && c.status !== "dropped")?.name ?? null;
}

/**
 * Table structure editor (TablePlus "Structure" view of a table's tab): a header with the table's
 * name, primary key, comment and options; the columns, indexes, checks and foreign keys grids; a
 * triggers grid; and, for views, the definition. Edits are staged as DDL ops per tab; ⌘S reviews
 * and applies them.
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
  const replaceStaged = useStructureStore((s) => s.replace);
  const undoStaged = useStructureStore((s) => s.undo);
  const redoStaged = useStructureStore((s) => s.redo);
  const discardStaged = useStructureStore((s) => s.discard);
  const ops = useStagedOps(tab.id);
  const canUndo = useCanUndo(tab.id);
  const canRedo = useCanRedo(tab.id);
  const partialRan = usePartialApply(tab.id);
  const rootRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const pendingFocus = useRef<string | null>(null);
  const focusRoot = useCallback(() => rootRef.current?.focus({ preventScroll: true }), []);
  const [showTriggers, setShowTriggers] = useState(false);
  const [filter, setFilter] = useState("");
  const [viewReset, setViewReset] = useState(0);

  const metaKey = useMemo(
    () => schemaKeys.table(connectionId, table.schema, table.name),
    [connectionId, table.schema, table.name],
  );
  const meta = useQuery({
    queryKey: metaKey,
    queryFn: () => ipc.schema.table(connectionId, table),
    staleTime: 5 * 60_000,
  });
  const choices = useQuery({
    queryKey: ["structureChoices", connectionId],
    queryFn: () => ipc.schema.structureChoices(connectionId),
    staleTime: 10 * 60_000,
    enabled: dialect !== "sqlite",
  });
  const view = useMemo(() => (meta.data ? foldStructure(meta.data, ops) : null), [meta.data, ops]);
  const kind = meta.data?.kind ?? "table";
  const isView = isViewKind(kind);
  const editable = !!meta.data && isAlterable(kind) && !readOnlyConnection;
  const viewEditable = !!meta.data && isView && !readOnlyConnection;
  const canStage = editable || viewEditable;

  // Forget the staged ops of tabs that are gone for good. Recently closed tabs keep theirs, so
  // reopening one (⌘⇧T) brings its staged work back.
  useEffect(() => {
    const { tabs, closed } = useTabsStore.getState();
    useStructureStore.getState().retain(new Set([...tabs, ...closed].map((t) => t.id)));
  }, []);

  // Take keyboard focus (so ⌘S / ⌘I / ⌘Z resolve to the grid scope) once the structure is first
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
      // The rename (Name field) runs last: every other op names the table as it is now.
      stage(tab.id, (current) => withRenameLast(fn(m, current), m.table));
    },
    [meta.data, editable, stage, tab.id],
  );

  useEffect(() => {
    const key = pendingFocus.current;
    if (!key) return;
    pendingFocus.current = null;
    focusCell(key, rootRef.current ?? document);
  }, [view]);

  // ---- column edits ---------------------------------------------------------
  const isKeyColumn = useCallback(
    (name: string) =>
      !!view &&
      (view.primaryKey.includes(name) ||
        view.indexes.some(
          (i) =>
            i.status !== "dropped" &&
            i.columns[0] !== undefined &&
            entryBase(i.columns[0]) === name,
        )),
    [view],
  );

  /** Validates `field` = `raw` of column `key`; returns the parsed value or null (toasted). */
  const checked = useCallback(
    (key: string, field: ColumnField, raw: string | boolean) => {
      const parsed = parseColumnInput(dialect, field, raw);
      if (!parsed.ok) {
        toast.error(parsed.error);
        return null;
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
              isKey: isKeyColumn(row.name),
            },
            field,
            parsed.value,
          );
        if (err) {
          toast.error(err);
          return null;
        }
      }
      return parsed;
    },
    [dialect, view, isKeyColumn],
  );

  const onEdit = useCallback(
    (key: string, field: ColumnField, raw: string | boolean) => {
      const parsed = checked(key, field, raw);
      if (parsed) apply((m, current) => editColumn(m, current, dialect, key, field, parsed.value));
    },
    [apply, dialect, checked],
  );

  const onPreset = useCallback(
    (key: string, preset: DefaultPreset, value?: string) => {
      const row = view?.columns.find((c) => c.key === key);
      if (!row) return;
      if (preset.kind === "autoIncrement") {
        if (checked(key, "autoIncrement", true))
          apply((m, current) => editColumn(m, current, dialect, key, "autoIncrement", true));
        return;
      }
      if (preset.kind === "expression") return;
      const expr =
        preset.kind === "constant"
          ? value === undefined || value === ""
            ? null
            : constantDefault(dialect, row.dataType, value)
          : preset.expr;
      apply((m, current) => {
        // A default replaces auto increment (an identity / AUTO_INCREMENT column has none).
        const next = row.autoIncrement
          ? editColumn(m, current, dialect, key, "autoIncrement", false)
          : current;
        return editColumn(m, next, dialect, key, "defaultExpr", expr);
      });
    },
    [view, apply, dialect, checked],
  );

  const onAddColumn = useCallback(
    (after: string | null = null) => {
      const m = meta.data;
      if (!m || !editable) return;
      setShowTriggers(false);
      setFilter("");
      stage(tab.id, (current) => {
        const r = addColumn(m, current, dialect, after);
        pendingFocus.current = `${r.key}:name`;
        return withRenameLast(r.ops, m.table);
      });
    },
    [meta.data, editable, stage, tab.id, dialect],
  );

  const onSetCheck = useCallback(
    (column: string, expression: string) => {
      if (!view || !editable) return;
      const expr = expression.trim();
      const names = view.columns.filter((c) => c.status !== "dropped").map((c) => c.name);
      const uses = checkColumns(expr, names);
      if (expr && (uses.length !== 1 || uses[0]!.toLowerCase() !== column.toLowerCase())) {
        toast.error(
          `A column check uses “${column}” and no other column; add other checks under Checks`,
        );
        return;
      }
      apply((m, current) => setColumnCheck(m, current, column, expression));
    },
    [view, editable, apply],
  );

  const columnHandlers: ColumnHandlers = useMemo(
    () => ({
      onEdit,
      onPreset,
      onSetCheck,
      onSaveFk: (replaces: string | null, fk: FkDef) =>
        apply((m, current) =>
          replaces === null
            ? addForeignKey(m, current, fk)
            : replaceForeignKey(m, current, replaces, fk),
        ),
      onDeleteFk: (key: string) => apply((m, current) => dropForeignKey(m, current, key)),
      onDrop: (key: string) => apply((m, current) => dropColumn(m, current, key)),
      onRestore: (key: string) => apply((m, current) => restoreColumn(m, current, key)),
      onAdd: onAddColumn,
    }),
    [onEdit, onPreset, onSetCheck, apply, onAddColumn],
  );

  // ---- indexes, checks, table -------------------------------------------------
  const onAddIndex = useCallback(() => {
    const m = meta.data;
    if (!m || !editable || !view) return;
    setShowTriggers(false);
    const live = view.columns.filter((c) => c.status !== "dropped");
    const column = focusedColumn(view) ?? live.find((c) => !c.isPk)?.name ?? live[0]?.name ?? null;
    if (!column) return;
    const index: IndexDef = {
      name: freeIndexName(view, m.table.name, [column], false),
      columns: [column],
      unique: false,
      method: null,
      predicate: null,
      comment: null,
    };
    stage(tab.id, (current) => {
      const next = withRenameLast(addIndex(m, current, index), m.table);
      const row = foldStructure(m, next).indexes.find(
        (x) => x.status === "added" && x.name === index.name,
      );
      if (row) pendingFocus.current = `${row.key}:name`;
      return next;
    });
  }, [meta.data, editable, view, stage, tab.id]);

  const onEditIndex = useCallback(
    (key: string, patch: Partial<IndexDef>) => {
      const m = meta.data;
      const row = view?.indexes.find((x) => x.key === key);
      if (!m || !view || !row) return;
      const next = { ...patch };
      if (next.name !== undefined) {
        const taken = view.indexes.some(
          (x) =>
            x.key !== key &&
            x.status !== "dropped" &&
            x.name.toLowerCase() === next.name!.toLowerCase(),
        );
        if (taken) {
          toast.error(`An index named “${next.name}” already exists`);
          return;
        }
      }
      if (next.columns !== undefined && next.columns.length === 0) {
        toast.error("An index needs at least one column");
        return;
      }
      // A staged index still named after its columns follows them.
      if (
        row.status === "added" &&
        next.name === undefined &&
        (next.columns !== undefined || next.unique !== undefined) &&
        row.name.startsWith(suggestIndexName(m.table.name, row.columns, row.unique))
      ) {
        const others = { ...view, indexes: view.indexes.filter((x) => x.key !== key) };
        next.name = freeIndexName(
          others,
          m.table.name,
          next.columns ?? row.columns,
          next.unique ?? row.unique,
        );
      }
      apply((mm, current) => editIndex(mm, current, key, next));
    },
    [meta.data, view, apply],
  );

  const onAddCheck = useCallback(() => {
    const m = meta.data;
    if (!m || !editable || !view) return;
    setShowTriggers(false);
    const name = suggestCheckName(view, m.table.name);
    stage(tab.id, (current) => {
      const next = withRenameLast(addCheck(m, current, { name, expression: "" }), m.table);
      const row = foldStructure(m, next).checks.find(
        (x) => x.status === "added" && x.name === name,
      );
      if (row) pendingFocus.current = `${row.key}:expression`;
      return next;
    });
  }, [meta.data, editable, view, stage, tab.id]);

  const onRename = useCallback(
    (name: string) => {
      if (!name.trim()) {
        toast.error("Table name is required");
        return;
      }
      apply((m, current) => setTableName(m, current, name));
    },
    [apply],
  );

  // ---- triggers ------------------------------------------------------------------
  const [triggerEditing, setTriggerEditing] = useState<TriggerEditing>(null);
  const onAddTrigger = useCallback(() => {
    const m = meta.data;
    if (!m || !editable || !view) return;
    setTriggerEditing({
      key: null,
      sql: triggerTemplate(dialect, m.table, suggestTriggerName(view, m.table.name)),
    });
  }, [meta.data, editable, view, dialect]);
  const onOpenTrigger = useCallback(
    (key: string) => {
      const row = view?.triggers.find((t) => t.key === key);
      const m = meta.data;
      if (!row || !m) return;
      setTriggerEditing({ key, sql: triggerSource(dialect, m.table, row), readOnly: !editable });
    },
    [view, meta.data, dialect, editable],
  );
  const onStageTrigger = useCallback(
    (key: string | null, sql: string): string | null => {
      const m = meta.data;
      if (!m || !editable) return "The structure cannot be changed";
      const current = stagedOps(tab.id);
      const r =
        key === null ? addTrigger(m, current, sql) : editTrigger(m, current, dialect, key, sql);
      if (!r.ok) return r.error;
      stage(tab.id, () => withRenameLast(r.ops, m.table));
      return null;
    },
    [meta.data, editable, tab.id, dialect, stage],
  );

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

  /** Reloads metadata of `target`; with `clear`, drops the staged ops in the same render. */
  const reload = useCallback(
    async (clear: boolean, target: TableRef = table) => {
      const key = schemaKeys.table(connectionId, target.schema, target.name);
      try {
        await ipc.schema.invalidate(connectionId, target);
        const fresh = await ipc.schema.table(connectionId, target, true);
        qc.setQueryData(key, fresh);
        if (clear) discardStaged(tab.id);
      } catch {
        if (clear) discardStaged(tab.id);
        void qc.invalidateQueries({ queryKey: key });
      }
      invalidateSchema(qc, connectionId);
      void qc.invalidateQueries({ queryKey: ddlKey(connectionId, target) });
      // The table's data tab reloads too, unless it holds staged row edits: they point at columns
      // by position, which the new structure may have moved.
      if (tablesWithPendingEdits(connectionId).some((t) => sameTable(t, target))) {
        reportHeldTables([target]);
      } else {
        void qc.invalidateQueries({ queryKey: ["rows", connectionId, tableKey(target)] });
        void qc.invalidateQueries({ queryKey: ["count", connectionId, tableKey(target)] });
      }
    },
    [connectionId, table, qc, discardStaged, tab.id],
  );

  const applyPreview = useCallback(async () => {
    const current = preview.ops;
    if (current.length === 0 || preview.applying) return;
    // Safe mode: the generated SQL is what the connection's level asks about.
    const statements = preview.plan?.statements ?? [];
    if (!(await guardExecution(connectionId, { kind: "commit", statements }))) return;
    setPreview((p) => ({ ...p, applying: true, error: null, failedIndex: null, executed: null }));
    try {
      const result = await ipc.ddl.apply(connectionId, table, current);
      if (!result.error) {
        setPreview(PREVIEW_CLOSED);
        const n = result.executed.length;
        toast.success(`Structure updated · ${n} statement${n === 1 ? "" : "s"}`, {
          description: formatDuration(result.durationMs),
        });
        // A rename (the Name field) ran last: the tab follows the table to its new name.
        const renamed = stagedRename(current, table);
        const target = renamed ? followRename(qc, connectionId, table, renamed) : table;
        await reload(true, target);
        setViewReset((n) => n + 1);
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
  }, [preview.ops, preview.applying, preview.plan, connectionId, table, reload, tab.id, qc]);

  const discardAndRefresh = useCallback(() => {
    setPreview(PREVIEW_CLOSED);
    setViewReset((n) => n + 1);
    void reload(true);
  }, [reload]);

  const discard = useCallback(() => {
    if (stagedOps(tab.id).length === 0) return;
    // After a partial apply the metadata is stale too: reload it along with the discard.
    if (partialRan > 0) void reload(true);
    else discardStaged(tab.id);
    setViewReset((n) => n + 1);
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
    void qc.invalidateQueries({ queryKey: ["structureChoices", connectionId] });
  }, [connectionId, table, qc, metaKey]);

  const undo = useCallback(() => undoStaged(tab.id), [undoStaged, tab.id]);
  const redo = useCallback(() => redoStaged(tab.id), [redoStaged, tab.id]);
  /** ⌘I: a column, or an index while the index grid has focus, or a trigger in that view. */
  const newRow = useCallback(() => {
    const active = document.activeElement;
    if (active?.closest("[data-grid='indexes']")) onAddIndex();
    else if (active?.closest("[data-grid='checks']")) onAddCheck();
    else if (showTriggers) onAddTrigger();
    else onAddColumn();
  }, [onAddIndex, onAddCheck, onAddTrigger, onAddColumn, showTriggers]);
  /** ⌘⌫: drops the row holding the focus (column, index, check, foreign key or trigger). */
  const dropFocused = useCallback(() => {
    const row = document.activeElement?.closest<HTMLElement>("[data-row-kind]");
    const key = row?.dataset.rowKey;
    if (!row || !key) return;
    const drop: Record<string, Reducer> = {
      column: (mm, current) => dropColumn(mm, current, key),
      index: (mm, current) => dropIndex(mm, current, key),
      check: (mm, current) => dropCheck(mm, current, key),
      fk: (mm, current) => dropForeignKey(mm, current, key),
      trigger: (mm, current) => dropTrigger(mm, current, key),
    };
    const fn = drop[row.dataset.rowKind ?? ""];
    if (fn) apply(fn);
  }, [apply]);
  const focusSearch = useCallback(() => {
    searchRef.current?.focus();
    searchRef.current?.select();
  }, []);
  const showData = useCallback(() => setView(tab.id, "table"), [setView, tab.id]);
  const hasOps = ops.length > 0;
  // Editing pins a preview tab (TablePlus), so the next table opened from the sidebar gets a tab of
  // its own instead of replacing this one.
  const isPreview = tab.preview === true;
  useEffect(() => {
    if (hasOps && isPreview) pin(tab.id);
  }, [hasOps, isPreview, pin, tab.id]);
  useAction("grid.commit", isActive && canStage ? openPreview : null);
  useAction("grid.preview", isActive && canStage ? openPreview : null);
  useAction("grid.discard", isActive && hasOps ? discard : null);
  useAction("grid.undo", isActive && canUndo ? undo : null);
  useAction("grid.redo", isActive && canRedo ? redo : null);
  useAction("grid.new-row", isActive && editable ? newRow : null);
  useAction("grid.delete-rows", isActive && editable ? dropFocused : null);
  useAction("grid.filter", isActive && !!meta.data && !isView ? focusSearch : null);
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
  const [fkOpen, setFkOpen] = useState(false);

  const liveColumns = useMemo(
    () => view?.columns.filter((c) => c.status !== "dropped").map((c) => c.name) ?? [],
    [view],
  );
  const userTypes = useMemo(() => choices.data?.userTypes.map((t) => t.name) ?? [], [choices.data]);
  const collations = useMemo(
    () => choices.data?.charsets.flatMap((c) => c.collations) ?? [],
    [choices.data],
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
  const v = view!;

  const qualifiedName = m.table.schema ? `${m.table.schema}.${m.table.name}` : m.table.name;
  const live = <T extends { status: string }>(rows: T[]) =>
    rows.filter((r) => r.status !== "dropped").length;
  const summary = [
    ...(m.kind === "table" ? [] : [KIND_LABEL[m.kind]]),
    plural(live(v.columns), "column"),
    ...(isView ? [] : [plural(live(v.indexes), "index", "indexes")]),
    ...(v.foreignKeys.length > 0 ? [plural(live(v.foreignKeys), "foreign key")] : []),
    ...(v.checks.length > 0 ? [plural(live(v.checks), "check")] : []),
  ].join(" · ");
  const readOnlyNote = readOnlyConnection
    ? "Read-only connection: the structure cannot be changed."
    : !editable && !viewEditable
      ? `${KIND_LABEL[m.kind]}s are shown read-only.`
      : null;

  return (
    <div
      ref={rootRef}
      tabIndex={-1}
      data-scope="grid"
      className="flex h-full min-h-0 flex-col outline-none"
    >
      {readOnlyNote && (
        <div className="border-border text-muted-foreground flex shrink-0 items-center gap-1.5 border-b px-3 py-1 text-[11px]">
          <Lock className="size-3" />
          {readOnlyNote}
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
      {!isView && (
        <StructureHeader
          view={v}
          dialect={dialect}
          editable={editable}
          showTriggers={showTriggers}
          triggerCount={live(v.triggers)}
          charsets={choices.data?.charsets ?? []}
          engines={choices.data?.engines ?? []}
          onRename={onRename}
          onSetPrimaryKey={(columns) => apply((mm, current) => setPrimaryKey(mm, current, columns))}
          onSetComment={(comment) => apply((mm, current) => setTableComment(mm, current, comment))}
          onSetOption={(field: TableOptionField, value) =>
            apply((mm, current) => setTableOption(mm, current, field, value))
          }
          onToggleTriggers={() => setShowTriggers((s) => !s)}
          onInfo={() => setShowDdl(true)}
        />
      )}
      {isView ? (
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="min-h-0 flex-[3]">
            <ViewDefinitionPane
              connectionId={connectionId}
              table={m.table}
              kind={m.kind}
              dialect={dialect}
              editable={viewEditable}
              resetKey={viewReset}
              onStage={(next) => replaceStaged(tab.id, next)}
              onReview={() => void openPreview()}
            />
          </div>
          <div className="border-border @container min-h-0 flex-[2] overflow-auto border-t">
            <div className="min-w-[640px]">
              <ColumnsSection
                view={v}
                dialect={dialect}
                editable={false}
                connectionId={connectionId}
                table={m.table}
                filter=""
                userTypes={userTypes}
                collations={collations}
                handlers={columnHandlers}
              />
            </div>
          </div>
        </div>
      ) : (
        <div className="min-h-0 flex-1 overflow-auto">
          {/* Size container as wide as the scroll viewport: section bars span it (100cqw) and
              stick to its left edge while the tables scroll sideways. */}
          <div className="@container">
            <div className="min-w-[640px]">
              {showTriggers ? (
                <TriggersSection
                  rows={v.triggers}
                  dialect={dialect}
                  editable={editable}
                  onAdd={onAddTrigger}
                  onOpen={onOpenTrigger}
                  onDrop={(key) => apply((mm, current) => dropTrigger(mm, current, key))}
                  onRestore={(key) => apply((mm, current) => restoreTrigger(mm, current, key))}
                />
              ) : (
                <>
                  <ColumnsSection
                    view={v}
                    dialect={dialect}
                    editable={editable}
                    connectionId={connectionId}
                    table={m.table}
                    filter={filter}
                    userTypes={userTypes}
                    collations={collations}
                    handlers={columnHandlers}
                  />
                  <IndexesSection
                    rows={v.indexes}
                    dialect={dialect}
                    editable={editable}
                    columns={liveColumns}
                    onAdd={onAddIndex}
                    onEdit={onEditIndex}
                    onDrop={(key) => apply((mm, current) => dropIndex(mm, current, key))}
                    onRestore={(key) => apply((mm, current) => restoreIndex(mm, current, key))}
                  />
                  <ChecksSection
                    rows={v.checks}
                    editable={editable}
                    onAdd={onAddCheck}
                    onEdit={(key, patch) =>
                      apply((mm, current) => editCheck(mm, current, key, patch))
                    }
                    onDrop={(key) => apply((mm, current) => dropCheck(mm, current, key))}
                    onRestore={(key) => apply((mm, current) => restoreCheck(mm, current, key))}
                  />
                  <ForeignKeysSection
                    rows={v.foreignKeys}
                    editable={editable}
                    onAdd={() => setFkOpen(true)}
                    onDrop={(key) => apply((mm, current) => dropForeignKey(mm, current, key))}
                    onRestore={(key) => apply((mm, current) => restoreForeignKey(mm, current, key))}
                    onOpenTable={openStructure}
                  />
                </>
              )}
            </div>
          </div>
        </div>
      )}
      <BottomBar tabId={tab.id}>
        {editable && (
          <>
            <BarTooltip label="Add a column" action="grid.new-row">
              <Button
                variant="ghost"
                size="xs"
                className="h-6 shrink-0 px-1.5"
                onClick={() => onAddColumn()}
              >
                <Plus /> Column
              </Button>
            </BarTooltip>
            <BarTooltip label="Add an index (⌘I while the index grid has focus)">
              <Button
                variant="ghost"
                size="xs"
                className="h-6 shrink-0 px-1.5"
                onClick={onAddIndex}
              >
                <Plus /> Index
              </Button>
            </BarTooltip>
          </>
        )}
        {!isView && (
          <div
            className={cn(
              "border-border focus-within:border-ring bg-background ml-1 flex h-6 w-40 min-w-24 shrink items-center gap-1 rounded-md border px-1.5",
              filter && "border-ring",
            )}
          >
            <Search className="text-muted-foreground size-3 shrink-0" />
            <input
              ref={searchRef}
              data-scope="grid-edit"
              aria-label="Search columns"
              placeholder="Search columns"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  e.preventDefault();
                  setFilter("");
                  focusRoot();
                }
              }}
              spellCheck={false}
              autoComplete="off"
              className="placeholder:text-muted-foreground/60 min-w-0 flex-1 bg-transparent text-xs outline-none"
            />
          </div>
        )}
        <div className="flex min-w-0 flex-1 justify-center px-1">
          <span
            className="text-foreground/80 min-w-0 truncate whitespace-nowrap tabular-nums"
            title={m.comment ? `${qualifiedName} — ${m.comment}` : qualifiedName}
          >
            {summary}
          </span>
        </div>
        {(hasOps || canUndo || canRedo) && (
          <>
            {hasOps && (
              <span
                className="bg-brand-soft text-brand shrink-0 rounded px-1.5 text-[11px] leading-5 font-medium whitespace-nowrap tabular-nums"
                title={`${plural(ops.length, "staged change")}`}
              >
                {plural(ops.length, "change")}
              </span>
            )}
            <BarIconButton
              label="Undo last change"
              action="grid.undo"
              onClick={undo}
              disabled={!canUndo}
            >
              <Undo2 />
            </BarIconButton>
            <BarIconButton
              label="Redo change"
              action="grid.redo"
              onClick={redo}
              disabled={!canRedo}
            >
              <Redo2 />
            </BarIconButton>
            {hasOps && (
              <>
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
              </>
            )}
            <BarSeparator />
          </>
        )}
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
      <TriggerEditorDialog
        editing={triggerEditing}
        dialect={dialect}
        tableName={m.table.name}
        onClose={() => setTriggerEditing(null)}
        onStage={onStageTrigger}
      />
      {editable && (
        <AddForeignKeyDialog
          open={fkOpen}
          onOpenChange={setFkOpen}
          connectionId={connectionId}
          table={m.table}
          columns={liveColumns}
          onAdd={(fk) => apply((mm, current) => addForeignKey(mm, current, fk))}
          returnFocus={focusRoot}
        />
      )}
    </div>
  );
}
