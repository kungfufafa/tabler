import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  ArrowDownAZ,
  ListOrdered,
  Lock,
  Plus,
  Redo2,
  RefreshCw,
  Search,
  SquareTerminal,
  Undo2,
  X,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent,
  type ReactNode,
} from "react";
import { toast } from "sonner";
import { EmptyState } from "@/components/EmptyState";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/skeleton";
import { NotAvailable } from "@/features/drivers/NotAvailable";
import { isCoreDriver } from "@/features/drivers/catalog";
import { laneBuiltFor, useDriverUi } from "@/features/drivers/registry";
import { openSqlInNewQueryTab } from "@/features/editor/openSql";
import { bumpGeneration, openWorkspaceSheet, useCommitGeneration } from "@/features/commit/store";
import { commitWorkspace, workspacePendingCount } from "@/features/commit/workspace";
import { actions, useAction } from "@/features/palette/actions";
import { schemaKeys } from "@/features/sidebar/useSchemaTree";
import {
  BarIconButton,
  BarSeparator,
  BarTooltip,
  BottomBarFrame,
} from "@/features/table-tab/BarParts";
import { ViewSwitch } from "@/features/tabs/ViewSwitch";
import {
  ipc,
  toIpcError,
  type DdlOp,
  type Dialect,
  type FkDef,
  type IndexDef,
  type TableMeta,
  type TableRef,
} from "@/lib/ipc";
import { capsOf, has, sessionDialect } from "@/lib/caps";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { setPref, usePrefs, type StructureColumnOrder } from "@/stores/prefs";
import { useTabsStore, type Tab } from "@/stores/tabs";
import {
  columnEditError,
  parseColumnInput,
  type ColumnField,
  type ColumnValue,
} from "./columnEdits";
import { ColumnsSection, type ColumnHandlers } from "./ColumnsSection";
import { orderColumns, ordinalNumbers } from "./columnOrder";
import { nextSelection, useColumnSelectionStore } from "./columnSelection";
import { reloadStructure } from "./commitFinish";
import { constantDefault, type DefaultPreset } from "./defaults";
import { focusCell } from "./dom";
import { ddlKey } from "./invalidate";
import { AddForeignKeyDialog } from "./KeyDialogs";
import {
  addColumn,
  addForeignKey,
  addIndex,
  columnNameError,
  dropColumn,
  dropColumns,
  dropForeignKey,
  dropIndex,
  editColumn,
  editIndex,
  entryBase,
  checkColumns,
  foldStructure,
  replaceForeignKey,
  restoreColumn,
  restoreColumns,
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
  setTriggerEnabled,
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
import { ReadOnlyStructure } from "./ReadOnlyStructure";
import { ShowDdlDialog } from "./ShowDdlDialog";
import {
  stagedOps,
  useCanRedo,
  useCanUndo,
  usePartialApply,
  useRevertedApply,
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
  const info = useConnectionsStore((s) => s.byId[tab.connectionId]?.info);
  const driver = info?.driver ?? "postgres";
  const ui = useDriverUi(info?.driver ?? null);
  // The lane's module is still loading.
  if (ui === undefined)
    return (
      <div className="flex flex-col gap-2 p-4" data-slot="lane-loading">
        <Skeleton className="h-5 w-48" />
        <Skeleton className="h-64 w-full" />
      </div>
    );
  const caps = capsOf(info ?? { driver, capabilities: null });
  // A session of a lane this build lacks: nothing of the lane's, nor the core editor.
  if (!isCoreDriver(driver) && !laneBuiltFor({ driver }))
    return <NotAvailable driver={driver} what="The structure editor" />;
  // A lane's own view of its tables' structure (indexes of a collection, a key's type).
  const View = ui?.StructureView;
  if (View)
    return (
      <div className="flex h-full min-h-0 flex-col">
        <div className="min-h-0 flex-1">
          <View connectionId={tab.connectionId} table={tab.table} caps={caps} />
        </div>
        <BottomBar tabId={tab.id} />
      </div>
    );
  if (!has(caps, "structureView"))
    return <NotAvailable driver={driver} what="The structure view" />;
  // Its edits are planned as a core dialect's DDL; another session's structure is read-only.
  const dialect = sessionDialect(caps);
  if (!dialect)
    return (
      <ReadOnlyStructure
        tabId={tab.id}
        connectionId={tab.connectionId}
        table={tab.table}
        caps={caps}
      />
    );
  return <StructureTabView tab={tab} dialect={dialect} canEdit={has(caps, "structureEdit")} />;
}

function StructureTabView({
  tab,
  dialect,
  canEdit,
}: {
  tab: Extract<Tab, { kind: "structure" }>;
  dialect: Dialect;
  /** The session offers editing the structure (`Feature::StructureEdit`). */
  canEdit: boolean;
}) {
  const { connectionId, table } = tab;
  const qc = useQueryClient();
  // Without editing the structure, it shows as on a read-only connection.
  const readOnlyConnection =
    useConnectionsStore((s) => s.byId[connectionId]?.info.readOnly ?? false) || !canEdit;
  const isActive = useTabsStore((s) => s.activeId === tab.id);
  const openTable = useTabsStore((s) => s.openTable);
  const setView = useTabsStore((s) => s.setView);
  const pin = useTabsStore((s) => s.pin);
  const stage = useStructureStore((s) => s.stage);
  const replaceStaged = useStructureStore((s) => s.replace);
  const undoStaged = useStructureStore((s) => s.undo);
  const redoStaged = useStructureStore((s) => s.redo);
  const ops = useStagedOps(tab.id);
  const canUndo = useCanUndo(tab.id);
  const canRedo = useCanRedo(tab.id);
  const partialRan = usePartialApply(tab.id);
  const reverted = useRevertedApply(tab.id);
  const rootRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const pendingFocus = useRef<string | null>(null);
  const focusRoot = useCallback(() => rootRef.current?.focus({ preventScroll: true }), []);
  const [showTriggers, setShowTriggers] = useState(false);
  const [filter, setFilter] = useState("");
  // Bumped when the staged work was committed or discarded (the view drops its local edits).
  const viewReset = useCommitGeneration(tab.id);

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

  /** Validates `field` = `raw` of column `key`: the parsed value, or why it is refused. */
  const validate = useCallback(
    (
      key: string,
      field: ColumnField,
      raw: string | boolean,
    ): { ok: true; value: ColumnValue } | { ok: false; error: string } => {
      const parsed = parseColumnInput(dialect, field, raw);
      if (!parsed.ok) return parsed;
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
        if (err) return { ok: false, error: err };
      }
      return parsed;
    },
    [dialect, view, isKeyColumn],
  );

  /** Like `validate`, with the refusal toasted; the parsed value or null. */
  const checked = useCallback(
    (key: string, field: ColumnField, raw: string | boolean) => {
      const r = validate(key, field, raw);
      if (!r.ok) {
        toast.error(r.error);
        return null;
      }
      return r;
    },
    [validate],
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

  // ---- columns: order, selection, bulk edit ---------------------------------------
  const columnOrder = usePrefs((p) => p.structureColumnOrder);
  const orderedView = useMemo(
    () => (view ? { ...view, columns: orderColumns(view.columns, columnOrder) } : null),
    [view, columnOrder],
  );
  const numbers = useMemo(() => (view ? ordinalNumbers(view.columns) : undefined), [view]);
  const [selectedKeys, setSelectedKeys] = useState<string[]>([]);
  const anchorKey = useRef<string | null>(null);
  const orderedKeys = useMemo(() => orderedView?.columns.map((c) => c.key) ?? [], [orderedView]);
  const onSelectColumn = useCallback(
    (key: string, mods: { meta: boolean; shift: boolean }) => {
      const next = nextSelection(selectedKeys, anchorKey.current, orderedKeys, key, mods);
      anchorKey.current = next.anchor;
      setSelectedKeys(next.selected);
    },
    [selectedKeys, orderedKeys],
  );
  // Columns that are gone (committed, discarded, a removed new one) leave the selection.
  useEffect(() => {
    setSelectedKeys((current) => {
      const live = current.filter((k) => orderedKeys.includes(k));
      return live.length === current.length ? current : live;
    });
  }, [orderedKeys]);
  const selectedSet = useMemo(() => new Set(selectedKeys), [selectedKeys]);
  const selectedRows = useMemo(
    () => (orderedView ? orderedView.columns.filter((c) => selectedSet.has(c.key)) : []),
    [orderedView, selectedSet],
  );

  /**
   * The right sidebar's edit of the selected columns: `raw` as `field` on every live one, as one
   * undo step. Refused as a whole (one message) when any of them cannot take it.
   */
  const onBulkEdit = useCallback(
    (field: ColumnField, raw: string | boolean) => {
      const keys = selectedRows.filter((r) => r.status !== "dropped").map((r) => r.key);
      if (keys.length === 0) return;
      const values = new Map<string, ColumnValue>();
      for (const key of keys) {
        const r = validate(key, field, raw);
        if (!r.ok) {
          const name = selectedRows.find((c) => c.key === key)?.name ?? "";
          toast.error(keys.length > 1 ? `${name}: ${r.error}` : r.error);
          return;
        }
        values.set(key, r.value);
      }
      apply((m, current) =>
        keys.reduce(
          (ops, key) => editColumn(m, ops, dialect, key, field, values.get(key) ?? null),
          current,
        ),
      );
    },
    [selectedRows, validate, apply, dialect],
  );
  // The sidebar shows the selected columns while this view is the one in front.
  const publishSelection = useColumnSelectionStore((s) => s.publish);
  const clearSelection = useColumnSelectionStore((s) => s.clear);
  useEffect(() => {
    if (!isActive || selectedRows.length === 0) {
      clearSelection(tab.id);
      return;
    }
    publishSelection({
      tabId: tab.id,
      table: table.name,
      dialect,
      rows: selectedRows,
      editable,
      onEdit: onBulkEdit,
    });
    return () => clearSelection(tab.id);
  }, [
    isActive,
    selectedRows,
    tab.id,
    table.name,
    dialect,
    editable,
    onBulkEdit,
    publishSelection,
    clearSelection,
  ]);

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
      // several columns at once are one staged step: one ⌘Z brings them all back
      onDropMany: (keys: string[]) => apply((m, current) => dropColumns(m, current, keys)),
      onRestoreMany: (keys: string[]) => apply((m, current) => restoreColumns(m, current, keys)),
      onAdd: onAddColumn,
      onSelect: onSelectColumn,
    }),
    [onEdit, onPreset, onSetCheck, apply, onAddColumn, onSelectColumn],
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
      include: [],
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
  // Staged structure is part of the workspace commit: ⌘S commits every pending change of the
  // connection, ⇧⌘P previews them all in one sheet, ⇧⌘⌫ discards them (`features/commit`).
  const openPreview = useCallback(() => {
    if (workspacePendingCount(connectionId) === 0) {
      toast.message("No changes to apply");
      return;
    }
    openWorkspaceSheet(connectionId);
  }, [connectionId]);

  /** A plan that stopped partway left the table changed: reload it, dropping the staged ops. */
  const discardAndRefresh = useCallback(() => {
    void reloadStructure(qc, connectionId, tab.id, table, true);
    bumpGeneration(tab.id);
  }, [qc, connectionId, tab.id, table]);

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
  /**
   * ⌘⌫: drops the row holding the focus (column, index, check, foreign key or trigger). A column
   * of a selection of several takes the whole selection with it, in one step.
   */
  const dropFocused = useCallback(() => {
    const row = document.activeElement?.closest<HTMLElement>("[data-row-kind]");
    const key = row?.dataset.rowKey;
    if (!row || !key) return;
    if (row.dataset.rowKind === "column" && selectedSet.has(key)) {
      const keys = selectedRows.filter((r) => r.status !== "dropped").map((r) => r.key);
      if (keys.length > 1 && keys.includes(key)) {
        apply((mm, current) => dropColumns(mm, current, keys));
        return;
      }
    }
    const drop: Record<string, Reducer> = {
      column: (mm, current) => dropColumn(mm, current, key),
      index: (mm, current) => dropIndex(mm, current, key),
      check: (mm, current) => dropCheck(mm, current, key),
      fk: (mm, current) => dropForeignKey(mm, current, key),
      trigger: (mm, current) => dropTrigger(mm, current, key),
    };
    const fn = drop[row.dataset.rowKind ?? ""];
    if (fn) apply(fn);
  }, [apply, selectedSet, selectedRows]);
  /**
   * Double-click on blank space adds a row (TablePlus): the blank row that ends a grid, its "No …"
   * row, or the empty space of a grid or below them. Clicks on real rows, controls, popovers and
   * menus are theirs.
   */
  const onBlankDoubleClick = useCallback(
    (e: MouseEvent<HTMLElement>) => {
      const target = e.target as Element;
      if (!editable || !e.currentTarget.contains(target)) return;
      if (
        target.closest(
          "input, textarea, select, button, a, label, [role='menu'], [role='menuitem'], [role='combobox'], [contenteditable='true'], [data-section-header], thead, [data-row-kind]",
        )
      )
        return;
      let kind = target.closest<HTMLElement>("[data-blank-row]")?.dataset.blankRow;
      if (!kind) {
        // empty space: the grid's own section, else the grid the view is about
        const section = target.closest("section")?.getAttribute("aria-label");
        if (section === "Checks" || section === "Foreign keys") return;
        kind =
          section === "Indexes"
            ? "indexes"
            : section === "Triggers" || (!section && showTriggers)
              ? "triggers"
              : "columns";
      }
      if (kind === "indexes") onAddIndex();
      else if (kind === "triggers") onAddTrigger();
      else onAddColumn();
    },
    [editable, showTriggers, onAddColumn, onAddIndex, onAddTrigger],
  );
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
  // Registered while the view can stage, not only once it has staged: a cell's ⌘S stages its edit
  // and dispatches in the same tick, before any re-render (the host's own handlers register once
  // the workspace holds changes).
  const commitStaged = useCallback(() => void commitWorkspace(connectionId), [connectionId]);
  useAction("grid.commit", isActive && canStage ? commitStaged : null);
  useAction("grid.preview", isActive && canStage ? openPreview : null);
  useAction("grid.undo", isActive && canUndo ? undo : null);
  useAction("grid.redo", isActive && canRedo ? redo : null);
  useAction("grid.new-row", isActive && editable ? newRow : null);
  useAction("grid.delete-rows", isActive && editable ? dropFocused : null);
  useAction("grid.filter", isActive && !!meta.data && !isView ? focusSearch : null);
  useAction("view.refresh", isActive ? refresh : null);
  useAction("view.data", isActive ? showData : null);
  // ⌘↩ resolves to Quick Look in this grid scope, which a structure has nothing to show for: it
  // opens the SQL editor, as it does from every other tab that is not a SQL tab.
  const openEditor = useCallback(() => void actions.run("query.open-editor"), []);
  useAction("grid.quick-look", isActive ? openEditor : null);

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
  const columnTypes = useMemo(
    () =>
      new Map(view?.columns.filter((c) => c.status !== "dropped").map((c) => [c.name, c.dataType])),
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
  const ordered = orderedView!;

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
      {reverted && partialRan === 0 && (
        <div
          role="alert"
          className="border-border bg-surface-2 text-muted-foreground flex shrink-0 items-center gap-2 border-b px-3 py-1 text-[11px]"
        >
          <AlertTriangle className="text-warning size-3.5 shrink-0" />
          <span className="min-w-0 flex-1">
            The last apply failed. All changes were reverted: the table is as it was, and the staged
            changes are still here.
          </span>
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
              onReview={openPreview}
            />
          </div>
          <div className="border-border @container min-h-0 flex-[2] overflow-auto border-t">
            <div className="min-w-[640px]">
              <ColumnsSection
                view={ordered}
                dialect={dialect}
                editable={false}
                connectionId={connectionId}
                table={m.table}
                filter=""
                userTypes={userTypes}
                collations={collations}
                handlers={columnHandlers}
                selected={selectedSet}
                numbers={numbers}
              />
            </div>
          </div>
        </div>
      ) : (
        <div className="min-h-0 flex-1 overflow-auto" onDoubleClick={onBlankDoubleClick}>
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
                  onSetEnabled={(key, enabled) =>
                    apply((mm, current) => setTriggerEnabled(mm, current, key, enabled))
                  }
                />
              ) : (
                <>
                  <ColumnsSection
                    view={ordered}
                    dialect={dialect}
                    editable={editable}
                    connectionId={connectionId}
                    table={m.table}
                    filter={filter}
                    userTypes={userTypes}
                    collations={collations}
                    handlers={columnHandlers}
                    selected={selectedSet}
                    numbers={numbers}
                  />
                  <IndexesSection
                    rows={v.indexes}
                    dialect={dialect}
                    editable={editable}
                    columns={liveColumns}
                    columnTypes={columnTypes}
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
            <BarTooltip
              label={`Add an index (${formatCombo(shortcutFor("grid.new-row") ?? "Mod+I")} while the index grid has focus)`}
            >
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
        {!isView && (
          <DropdownMenu>
            <BarTooltip label="Order the columns by name or by their position in the table">
              <DropdownMenuTrigger asChild>
                <Button
                  variant="ghost"
                  size="xs"
                  className="h-6 shrink-0 px-1.5"
                  aria-label="Order columns by"
                >
                  {columnOrder === "name" ? <ArrowDownAZ /> : <ListOrdered />}
                  {columnOrder === "name" ? "Name" : "Ordinal position"}
                </Button>
              </DropdownMenuTrigger>
            </BarTooltip>
            <DropdownMenuContent align="start" className="min-w-44">
              <DropdownMenuRadioGroup
                value={columnOrder}
                onValueChange={(v) => setPref("structureColumnOrder", v as StructureColumnOrder)}
              >
                <DropdownMenuRadioItem value="name">Order columns by Name</DropdownMenuRadioItem>
                <DropdownMenuRadioItem value="ordinal">
                  Order columns by Ordinal position
                </DropdownMenuRadioItem>
              </DropdownMenuRadioGroup>
            </DropdownMenuContent>
          </DropdownMenu>
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
                <BarIconButton
                  label="Discard all changes"
                  action="grid.discard"
                  // Same as ⇧⌘⌫ (what the tooltip shows): its observers see the discard too.
                  onClick={() => void actions.run("grid.discard")}
                >
                  <X />
                </BarIconButton>
                <BarTooltip label="Review and apply the changes" action="grid.commit">
                  <Button
                    size="xs"
                    onClick={openPreview}
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
