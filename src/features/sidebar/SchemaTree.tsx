import { useQueries, type UseQueryResult } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  ArrowUpRight,
  ChevronRight,
  Folder,
  KeyRound,
  ListOrdered,
  Loader2,
  Pin,
  Sigma,
  SquareFunction,
  Table2,
  TriangleAlert,
  Workflow,
  type LucideIcon,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  type DragEvent,
  type KeyboardEvent,
  type MouseEvent,
  type Ref,
} from "react";
import { EmptyState } from "@/components/EmptyState";
import { TableIcon } from "@/components/TypeIcon";
import { ContextMenu, ContextMenuContent, ContextMenuTrigger } from "@/components/ui/context-menu";
import { Skeleton } from "@/components/ui/skeleton";
import { openDiagram } from "@/features/diagram/store";
import {
  copyLaravelMigrations,
  copyNames,
  copyRoutineScript,
  copySequenceScript,
  copyTableScript,
  exportTables,
  importInto,
  openRoutine,
  openSequence,
  refOf,
  runNew,
  togglePins,
  type ObjectContext,
} from "@/features/objects/actions";
import {
  groupEntries,
  groupHeaderMenu,
  headerMenu,
  objectMenu,
  tableMenu,
  type MenuEntry,
} from "@/features/objects/menu";
import { ContextMenuEntries } from "@/features/objects/MenuEntries";
import { routineName, usePendingMarks } from "@/features/objects/pendingOps";
import { useRoutines, useSequences, useSidebarReload } from "@/features/objects/queries";
import {
  clickSelection,
  EMPTY_SELECTION,
  pruneSelection,
  selectAll,
  selectOnly,
  selectRange,
  targetKeys,
  type TreeSelection,
} from "@/features/objects/selection";
import {
  openItemOverview,
  openObjectDialog,
  routineRef,
  type ObjectDialog,
} from "@/features/objects/store";
import { isAlterable } from "@/features/structure/tableActions";
import { formatCount } from "@/lib/format/time";
import { comboFromEvent, isMac } from "@/lib/keys/combo";
import {
  ipc,
  tableKey,
  toIpcError,
  type RoutineKind,
  type RoutineSummary,
  type SequenceSummary,
  type TableMeta,
  type TableRef,
  type TableSummary,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { EMPTY_PINS } from "@/stores/pins";
import { recordRecent } from "@/stores/recent";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import {
  addToGroup,
  deleteGroup,
  EMPTY_GROUPS,
  NAME_OPS,
  GROUP_KINDS,
  groupWarning,
  openGroupDialog,
  removeFromGroup,
  routineItemKey,
  sequenceItemKey,
  setGroupCollapsed,
  tableItemKey,
  type SidebarGroup,
} from "./groups";
import { setSectionCollapsed, toggleSection, useSidebarPrefs } from "./sidebarPrefs";
import {
  groupIdOfRow,
  isItemRow,
  itemName,
  mergeObjectRows,
  pruneEmptyGroups,
  schemaKeys,
  useCurrentSchema,
  withColumnRows,
  type ColumnState,
  type ItemRow,
  type TreeRow,
} from "./useSchemaTree";

type HeaderRow = Extract<TreeRow, { type: "header" }>;
type GroupHeaderRow = Extract<TreeRow, { type: "groupHeader" }>;
type TableRow = Extract<TreeRow, { type: "table" }>;

/** What a sidebar row carries while it is dragged onto a group (item keys, `groups.ts`). */
export const DRAG_MIME = "application/x-tabler-sidebar-items";

const NO_KEYS: ReadonlySet<string> = new Set();

/** "name has prefix log_ and kind is view", for a rule group's tooltip. */
function rulesSummary(g: SidebarGroup): string {
  if (g.rules === null) return "Drag items here to add them";
  return g.rules
    .map((r) =>
      r.field === "name"
        ? `name ${NAME_OPS.find(([op]) => op === r.op)?.[1] ?? r.op} ${r.value}`
        : r.field === "kind"
          ? `kind is ${GROUP_KINDS.find(([k]) => k === r.value)?.[1] ?? r.value}`
          : `schema is ${r.value}`,
    )
    .join(" and ");
}

/** Row height of the tree (headers and items). */
export const ROW_H = 22;

/**
 * Keyboard focus is a thin inset ring, never a fill: the one filled row is the open table
 * (TablePlus). Inset, so the neighbouring rows (absolutely positioned) do not cover it.
 */
const FOCUS_RING =
  "outline-none ring-inset focus-visible:ring-1 focus-visible:ring-sidebar-ring/70";

/** Headers whose menu offers the Item Overview (spec §2.3: the "Tables" header). */
const OVERVIEW_HEADERS = new Set(["Tables", "Views", "Materialized views"]);

const ROUTINE_ICONS: Record<RoutineKind, LucideIcon> = {
  function: SquareFunction,
  procedure: Workflow,
  aggregate: Sigma,
  window: SquareFunction,
};

const ROUTINE_LABEL: Record<RoutineKind, string> = {
  function: "Function",
  procedure: "Procedure",
  aggregate: "Aggregate function",
  window: "Window function",
};

export interface SchemaTreeHandle {
  /** Focuses the first item row (scrolling it into view); false when the tree lists none. */
  focusFirstRow: () => boolean;
}

/**
 * Recording an opened table reorders (or grows) the Recent section, which moves rows under the
 * pointer. Mouse opens are recorded once the double-click window has passed, so the second click
 * of a double-click still lands on the row it was aimed at.
 */
export const RECENT_DELAY_MS = 500;

let recentQueue: Array<[scope: string, table: TableRef]> = [];
let recentTimer: ReturnType<typeof setTimeout> | null = null;

function flushRecent(): void {
  if (recentTimer !== null) clearTimeout(recentTimer);
  recentTimer = null;
  const queued = recentQueue;
  recentQueue = [];
  for (const [scope, table] of queued) recordRecent(scope, table);
}

/** Records `table` as recently opened in `scope`, after `RECENT_DELAY_MS` when `later`. */
function rememberOpened(scope: string, table: TableRef, later: boolean): void {
  if (!later) {
    flushRecent();
    recordRecent(scope, table);
    return;
  }
  recentQueue.push([scope, table]);
  if (recentTimer !== null) clearTimeout(recentTimer);
  recentTimer = setTimeout(flushRecent, RECENT_DELAY_MS);
}

/** Index of the next item row after `from` in direction `step` (headers are skipped). */
function nextItemRow(rows: readonly TreeRow[], from: number, step: 1 | -1): number | null {
  for (let i = from + step; i >= 0 && i < rows.length; i += step) if (isItemRow(rows[i])) return i;
  return null;
}

/** Unique tables / views of `rows` (one table can be listed in Recent, Pinned and its group). */
function tablesOf(rows: readonly ItemRow[]): TableSummary[] {
  const seen = new Set<string>();
  const out: TableSummary[] = [];
  for (const r of rows)
    if (r.type === "table" && !seen.has(tableKey(r.table))) {
      seen.add(tableKey(r.table));
      out.push(r.table);
    }
  return out;
}

function routinesOf(rows: readonly ItemRow[]): RoutineSummary[] {
  return rows.flatMap((r) => (r.type === "routine" ? [r.routine] : []));
}

function sequencesOf(rows: readonly ItemRow[]): SequenceSummary[] {
  return rows.flatMap((r) => (r.type === "sequence" ? [r.sequence] : []));
}

/**
 * Table rows of the tree have keys `<section>:<tableKey>` (`t:`, `p:`, `r:`), in a group
 * `g:<id>:t:<tableKey>`.
 */
function tableKeyOfRow(key: string): string | null {
  const m = /^(?:g:[^:]+:)?[tpr]:(.*)$/s.exec(key);
  return m ? m[1]! : null;
}

function routineTitle(r: RoutineSummary): string {
  const head = `${ROUTINE_LABEL[r.kind]} ${r.name}(${r.arguments})`;
  const parts = [r.returnType ? `${head} → ${r.returnType}` : head];
  if (r.language) parts.push(r.language);
  if (r.comment) parts.push(r.comment);
  return parts.join(" · ");
}

export function SchemaTree({
  connectionId,
  rows: tableRows,
  loading,
  error,
  search,
  pinScope = null,
  pinned = EMPTY_PINS,
  showRowCounts = false,
  groups = EMPTY_GROUPS,
  ref,
}: {
  connectionId: string;
  /** Table rows (`buildTreeRows`); the Functions and Sequences sections are added here. */
  rows: TreeRow[];
  loading: boolean;
  error: string | null;
  search: string;
  /**
   * Where "Pin to Top" stores pins and opened tables are recorded as recent (`pinScope`); null
   * hides "Pin to Top" and records nothing.
   */
  pinScope?: string | null;
  /** Pinned `tableKey`s of `pinScope`. */
  pinned?: ReadonlySet<string>;
  /** Show the estimated row count on the right of each table row. */
  showRowCounts?: boolean;
  /** The sidebar groups of `pinScope` (`groups.ts`); `rows` were built with them. */
  groups?: readonly SidebarGroup[];
  ref?: Ref<SchemaTreeHandle>;
}) {
  const parentRef = useRef<HTMLDivElement>(null);
  const openTable = useTabsStore((s) => s.openTable);
  const activeTab = useTabsStore((s) => s.tabs.find((t) => t.id === s.activeId) ?? null);
  const driver = useConnectionsStore((s) => s.byId[connectionId]?.info.driver ?? "postgres");
  const readOnly = useConnectionsStore((s) => s.byId[connectionId]?.info.readOnly ?? false);
  const currentSchema = useCurrentSchema(connectionId);
  const collapsed = useSidebarPrefs((s) => s.collapsed);
  const showFunctions = useSidebarPrefs((s) => s.options.showFunctions) && driver !== "sqlite";
  const showSequences = useSidebarPrefs((s) => s.options.showSequences) && driver === "postgres";
  const routines = useRoutines(connectionId, currentSchema, showFunctions);
  const sequences = useSequences(connectionId, currentSchema, showSequences);
  useSidebarReload(connectionId, currentSchema);
  const baseRows = useMemo(
    () =>
      pruneEmptyGroups(
        mergeObjectRows(
          tableRows,
          showFunctions ? routines.data : undefined,
          showSequences ? sequences.data : undefined,
          search,
          collapsed,
          groups,
        ),
        search,
      ),
    [
      tableRows,
      showFunctions,
      routines.data,
      showSequences,
      sequences.data,
      search,
      collapsed,
      groups,
    ],
  );
  // "Show Columns": expanded table rows (by row key) list their columns, loaded on expand.
  const showColumns = useSidebarPrefs((s) => s.options.showColumns);
  const [rawExpanded, setExpanded] = useState<ReadonlySet<string>>(NO_KEYS);
  const expanded = showColumns ? rawExpanded : NO_KEYS;
  const expandedTables = useMemo(() => {
    const byKey = new Map<string, TableSummary>();
    for (const r of baseRows)
      if (r.type === "table" && expanded.has(r.key)) byKey.set(tableKey(r.table), r.table);
    return [...byKey.values()];
  }, [baseRows, expanded]);
  const combineColumns = useCallback(
    (results: UseQueryResult<TableMeta, Error>[]) => {
      const states = new Map<string, ColumnState>();
      results.forEach((q, i) => {
        const t = expandedTables[i];
        if (!t) return;
        states.set(
          tableKey(t),
          q.data
            ? { status: "ready", meta: q.data }
            : q.isError
              ? { status: "error", message: toIpcError(q.error).message }
              : { status: "loading" },
        );
      });
      return states;
    },
    [expandedTables],
  );
  const columnStates = useQueries({
    queries: expandedTables.map((t) => ({
      queryKey: schemaKeys.table(connectionId, t.schema, t.name),
      queryFn: () => ipc.schema.table(connectionId, { schema: t.schema, name: t.name }),
      staleTime: 5 * 60_000,
      retry: false,
    })),
    combine: combineColumns,
  });
  const rows = useMemo(
    () =>
      withColumnRows(
        baseRows,
        expanded,
        (t) => columnStates.get(tableKey(t)) ?? { status: "loading" },
      ),
    [baseRows, expanded, columnStates],
  );
  const toggleExpanded = (key: string, open?: boolean) =>
    setExpanded((prev) => {
      const isOpen = prev.has(key);
      if (open === isOpen) return prev;
      const next = new Set(prev);
      if (isOpen) next.delete(key);
      else next.add(key);
      return next;
    });
  const marks = usePendingMarks(connectionId);
  // The manual group a dragged item hovers.
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const order = useMemo(() => rows.filter(isItemRow).map((r) => r.key), [rows]);
  const [rawSelection, setRawSelection] = useState<TreeSelection>(EMPTY_SELECTION);
  // Rows a search or a refresh took away are no longer selected.
  const selection = useMemo(() => pruneSelection(rawSelection, order), [rawSelection, order]);
  const select = (next: TreeSelection) => setRawSelection(next);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_H,
    overscan: 12,
    getItemKey: (i) => rows[i]?.key ?? i,
  });
  // Set when a context-menu item opened the structure view: the closing menu must then not hand
  // focus back to the sidebar, or the structure (which focuses itself) loses its keyboard
  // shortcuts. The data view leaves focus in the sidebar, so the menu may return it there.
  const openedFromMenu = useRef(false);
  // Row that ↑/↓ moved to while it was outside the rendered window: focused once rendered.
  const pendingFocus = useRef<number | null>(null);
  useEffect(() => {
    const index = pendingFocus.current;
    if (index === null) return;
    const el = parentRef.current?.querySelector<HTMLElement>(`[data-row-index="${index}"]`);
    if (!el) return;
    pendingFocus.current = null;
    el.focus({ preventScroll: true });
  });

  // Another table opened elsewhere (a tab switch, Open Anything): a selection that does not hold
  // it is stale.
  const activeTable =
    activeTab && activeTab.kind !== "query" && activeTab.connectionId === connectionId
      ? tableKey(activeTab.table)
      : null;
  useEffect(() => {
    if (activeTable === null) return;
    setRawSelection((s) =>
      s.keys.size === 0 || [...s.keys].some((k) => tableKeyOfRow(k) === activeTable)
        ? s
        : EMPTY_SELECTION,
    );
  }, [activeTable]);

  const focusRow = (index: number) => {
    virtualizer.scrollToIndex(index, { align: "auto" });
    const el = parentRef.current?.querySelector<HTMLElement>(`[data-row-index="${index}"]`);
    if (el) el.focus({ preventScroll: true });
    else pendingFocus.current = index;
  };
  useImperativeHandle(ref, () => ({
    focusFirstRow: () => {
      const first = nextItemRow(rows, -1, 1);
      if (first === null) return false;
      focusRow(first);
      return true;
    },
  }));

  const ctx: ObjectContext = { connectionId, driver, readOnly, schema: currentSchema, pinScope };
  const dialog = (d: ObjectDialog) =>
    openObjectDialog({ connectionId, dialect: driver, dialog: d });
  const newHandlers = { newItem: (what: Parameters<typeof runNew>[1]) => runNew(ctx, what) };
  const overviewOf = (schema: string | null) => {
    const s = schema ?? currentSchema;
    if (s !== null) openItemOverview({ connectionId, dialect: driver, schema: s });
  };

  if (loading) {
    return (
      <div className="flex flex-col gap-1.5 px-3 py-2">
        {[0, 1, 2, 3, 4, 5].map((i) => (
          <Skeleton key={i} className="h-4 w-[70%]" />
        ))}
      </div>
    );
  }
  if (error) return <EmptyState title="Could not load tables" description={error} />;
  if (rows.length === 0) {
    return (
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div className="min-h-0 flex-1">
            <EmptyState
              icon={Table2}
              title={search ? "No items match" : "No tables"}
              description={
                search ? `Nothing matches “${search}”.` : "This schema has no tables or views."
              }
            />
          </div>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuEntries
            entries={headerMenu(
              { driver, readOnly, overview: false },
              {
                ...newHandlers,
                itemOverview: () => overviewOf(null),
              },
            )}
          />
        </ContextMenuContent>
      </ContextMenu>
    );
  }

  /** The connection manager may be showing: an opened table must be seen. */
  const showWorkspace = () => useUiStore.getState().setView("workspace");
  /** Single click, ↑/↓: shows the table in the connection's (replaceable) preview tab. */
  const preview = (t: TableSummary) => {
    openTable(connectionId, refOf(t), "table", { preview: true });
    showWorkspace();
  };
  /** Double-click, middle-click, Enter, "Open in New Tab", "Open Structure": a regular tab. */
  const openPinned = (t: TableSummary, kind: "table" | "structure" = "table") => {
    openTable(connectionId, refOf(t), kind);
    showWorkspace();
  };
  const remember = (t: TableSummary, later = false) => {
    if (pinScope !== null) rememberOpened(pinScope, refOf(t), later);
  };
  /** Routines and sequences open their SQL in a new query tab (click or Enter). */
  const openObject = (row: ItemRow) => {
    if (row.type === "routine") void openRoutine(ctx, row.routine);
    else if (row.type === "sequence") void openSequence(ctx, row.sequence);
  };

  /** The rows a menu or key acts on: the selection when it holds `key`, else that row alone. */
  const targetsOf = (key: string): ItemRow[] => {
    const keys = new Set(targetKeys(selection, key));
    return rows.filter((r): r is ItemRow => isItemRow(r) && keys.has(r.key));
  };

  /** What is pending for the row's object (`pendingMarks`, by sidebar item key). */
  const markOf = (row: ItemRow) =>
    marks.get(
      row.type === "table"
        ? tableItemKey(row.table)
        : row.type === "routine"
          ? routineItemKey(row.routine)
          : sequenceItemKey(row.sequence),
    );
  /** Items with a pending drop are not dropped (nor truncated) a second time. */
  const canDrop = (t: TableSummary) =>
    t.kind !== "systemView" && marks.get(tableItemKey(t)) !== "drop";
  const canTruncate = (t: TableSummary) => isAlterable(t.kind) && !marks.has(tableItemKey(t));
  /** The routine / sequence rows of `targets` of the same type as `row`, not pending a drop. */
  const droppableObjects = (row: Exclude<ItemRow, TableRow>, targets: readonly ItemRow[]) =>
    targets.filter((r) => r.type === row.type && markOf(r) !== "drop");
  const dropTables = (
    tables: TableSummary[],
    preset: { cascade?: boolean; ignoreFk?: boolean },
  ) => {
    const droppable = tables.filter(canDrop);
    if (readOnly || droppable.length === 0) return;
    dialog({
      kind: "dropTables",
      tables: droppable.map((t) => ({ table: refOf(t), kind: t.kind })),
      cascade: preset.cascade,
      ignoreForeignKeys: preset.ignoreFk,
    });
  };
  const truncateTables = (tables: TableSummary[]) => {
    const truncatable = tables.filter(canTruncate);
    if (readOnly || truncatable.length === 0) return;
    dialog({ kind: "truncateTables", tables: truncatable.map(refOf) });
  };
  /** Delete… / ⌫ for whatever kind of row `row` is (the targets of the same kind). */
  const deleteTargets = (row: ItemRow, preset: { cascade?: boolean; ignoreFk?: boolean } = {}) => {
    const targets = targetsOf(row.key);
    if (row.type === "table") return dropTables(tablesOf(targets), preset);
    const droppable = droppableObjects(row, targets);
    if (readOnly || droppable.length === 0) return;
    if (row.type === "routine") {
      const list = routinesOf(droppable).map(routineRef);
      dialog({ kind: "dropRoutines", routines: list, names: list.map(routineName) });
    } else {
      dialog({ kind: "dropSequences", sequences: sequencesOf(droppable).map(refOf) });
    }
  };

  /** The item keys (`groups.ts`) a group action moves: the targets' tables and routines. */
  const groupKeysOf = (row: ItemRow): string[] => [
    ...new Set(
      targetsOf(row.key).flatMap((r) =>
        r.type === "table"
          ? [tableItemKey(r.table)]
          : r.type === "routine"
            ? [routineItemKey(r.routine)]
            : [],
      ),
    ),
  ];
  /** Add to Group ▸ / Remove from <group> of a table or routine row. */
  const groupItemsOf = (row: ItemRow): MenuEntry[] => {
    if (pinScope === null || row.type === "sequence") return [];
    const scope = pinScope;
    const keys = groupKeysOf(row);
    const manual = groups.filter((g) => g.rules === null);
    const inGroup = groupIdOfRow(row.key);
    const current = manual.find((g) => g.id === inGroup) ?? null;
    return groupEntries(
      {
        manual: manual.map((g) => ({ id: g.id, name: g.name })),
        current: current && { id: current.id, name: current.name },
      },
      {
        addToGroup: (id) => addToGroup(scope, id, keys),
        removeFromGroup: () => {
          if (current) removeFromGroup(scope, current.id, keys);
        },
        newGroup: () => openGroupDialog({ scope, members: keys }),
      },
    );
  };

  const tableEntries = (row: TableRow): MenuEntry[] => {
    const t = row.table;
    const tables = tablesOf(targetsOf(row.key));
    return tableMenu(
      {
        driver,
        readOnly,
        count: tables.length,
        truncatable: tables.filter(canTruncate).length,
        droppable: tables.filter(canDrop).length,
        isTable: isAlterable(t.kind),
        renamable: isAlterable(t.kind),
        allPinned: pinScope === null ? null : tables.every((x) => pinned.has(tableKey(x))),
      },
      {
        ...newHandlers,
        openInNewTab: () => {
          for (const x of tables) openPinned(x);
          remember(t);
        },
        openStructure: () => {
          openedFromMenu.current = true;
          openPinned(t, "structure");
          remember(t);
        },
        itemOverview: () => overviewOf(t.schema),
        copyName: () => copyNames(tables.map((x) => x.name)),
        togglePin: () => {
          if (pinScope !== null) togglePins(pinScope, tables.map(refOf));
        },
        exportItems: () => exportTables(ctx, tables),
        importCsv: () => importInto(ctx, "csv", t),
        importSql: () => importInto(ctx, "sql", null),
        clone: () => dialog({ kind: "clone", table: t }),
        rename: () => dialog({ kind: "renameTable", table: t }),
        truncate: () => truncateTables(tables),
        drop: () => dropTables(tables, {}),
        copyScript: (kind) =>
          kind === "laravel"
            ? void copyLaravelMigrations(
                ctx,
                tables.filter((x) => isAlterable(x.kind)),
              )
            : void copyTableScript(ctx, t, kind),
        generateDiagram: () =>
          openDiagram({
            connectionId,
            schema: t.schema ?? currentSchema,
            tables: tables.map(refOf),
          }),
      },
      groupItemsOf(row),
    );
  };

  const objectEntries = (row: Exclude<ItemRow, TableRow>): MenuEntry[] => {
    const targets = targetsOf(row.key).filter((r) => r.type === row.type);
    return objectMenu(
      {
        driver,
        readOnly,
        count: targets.length,
        droppable: droppableObjects(row, targets).length,
      },
      {
        ...newHandlers,
        open: () => openObject(row),
        copyName: () => copyNames(targets.map(itemName)),
        copyScript: (kind) =>
          row.type === "routine"
            ? void copyRoutineScript(ctx, row.routine, kind)
            : void copySequenceScript(ctx, row.sequence, kind),
        drop: () => deleteTargets(row),
      },
      groupItemsOf(row),
    );
  };

  /** ↑/↓ from a row or a header: focus the next item (tables preview); ⇧ extends the selection. */
  const onArrow = (e: KeyboardEvent, index: number) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return false;
    if (e.metaKey || e.ctrlKey || e.altKey) return false;
    e.preventDefault();
    const next = nextItemRow(rows, index, e.key === "ArrowDown" ? 1 : -1);
    const row = next === null ? undefined : rows[next];
    if (next === null || !isItemRow(row)) return true;
    focusRow(next);
    if (e.shiftKey) {
      const from = rows[index];
      const anchored =
        selection.anchor === null && isItemRow(from) ? selectOnly(from.key) : selection;
      select(selectRange(anchored, order, row.key));
      return true;
    }
    select(selectOnly(row.key));
    // Focus stays in the sidebar (the grid does not take it), so ↑/↓ keep browsing. Browsing is
    // not recorded as recent: that would reorder the Recent section while it is being browsed.
    if (row.type === "table") preview(row.table);
    return true;
  };

  const onRowKeyDown = (e: KeyboardEvent, index: number, row: ItemRow) => {
    if (onArrow(e, index)) return;
    const combo = comboFromEvent(e);
    const handled = () => {
      e.preventDefault();
      e.stopPropagation();
    };
    switch (combo) {
      case "Enter":
        // Instead of the button's click (a preview open): Enter keeps the tab.
        handled();
        if (row.type === "table") {
          openPinned(row.table);
          remember(row.table);
        } else openObject(row);
        return;
      case "Backspace":
      case "Delete":
        handled();
        deleteTargets(row);
        return;
      case "Shift+Backspace":
        handled();
        deleteTargets(row, { cascade: driver === "postgres" });
        return;
      case "Mod+Backspace":
        handled();
        deleteTargets(row, { ignoreFk: driver === "mysql" });
        return;
      case "Alt+Backspace":
        handled();
        if (row.type === "table") truncateTables(tablesOf(targetsOf(row.key)));
        return;
      case "ArrowRight":
      case "ArrowLeft":
        // "Show Columns": → lists the table's columns, ← hides them.
        if (!showColumns || row.type !== "table") return;
        handled();
        toggleExpanded(row.key, combo === "ArrowRight");
        return;
      case "Mod+A":
        handled();
        select(selectAll(order));
        return;
      case "Mod+C":
        handled();
        copyNames([...new Set(targetsOf(row.key).map(itemName))]);
        return;
    }
  };

  /** ←/→ on a section header collapse / expand it; ↑/↓ move to the items around it. */
  const onHeaderKeyDown = (e: KeyboardEvent, index: number, row: HeaderRow) => {
    if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      e.preventDefault();
      setSectionCollapsed(row.label, e.key === "ArrowLeft");
      return;
    }
    onArrow(e, index);
  };

  /** ←/→ on a group header collapse / expand it; ↑/↓ move to the items around it. */
  const onGroupHeaderKeyDown = (e: KeyboardEvent, index: number, row: GroupHeaderRow) => {
    if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      e.preventDefault();
      if (pinScope !== null) setGroupCollapsed(pinScope, row.group.id, e.key === "ArrowLeft");
      return;
    }
    onArrow(e, index);
  };

  /** The item keys a drag onto a group carries (empty when the drag is something else). */
  const draggedKeys = (e: DragEvent): string[] => {
    try {
      const value: unknown = JSON.parse(e.dataTransfer.getData(DRAG_MIME) || "[]");
      return Array.isArray(value) ? value.filter((k): k is string => typeof k === "string") : [];
    } catch {
      return [];
    }
  };
  const carriesItems = (e: DragEvent) => [...e.dataTransfer.types].includes(DRAG_MIME);

  /** ⌘-click toggles, ⇧-click extends; returns true when the click only changed the selection. */
  const modifierClick = (e: MouseEvent, key: string): boolean => {
    const meta = isMac ? e.metaKey : e.ctrlKey;
    if (!meta && !e.shiftKey) return false;
    select(clickSelection(selection, order, key, { meta, shift: e.shiftKey }));
    return true;
  };

  const onCloseAutoFocus = (e: Event) => {
    if (!openedFromMenu.current) return;
    openedFromMenu.current = false;
    e.preventDefault();
  };

  return (
    <div ref={parentRef} className="min-h-0 flex-1 overflow-auto" data-scope="sidebar">
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {virtualizer.getVirtualItems().map((v) => {
          const row = rows[v.index]!;
          const style = {
            position: "absolute" as const,
            top: 0,
            left: 0,
            width: "100%",
            height: ROW_H,
            transform: `translateY(${v.start}px)`,
          };
          if (row.type === "header") {
            return (
              <ContextMenu key={row.key}>
                <ContextMenuTrigger asChild>
                  <button
                    type="button"
                    style={style}
                    data-row-index={v.index}
                    aria-expanded={!row.collapsed}
                    onClick={() => toggleSection(row.label)}
                    onKeyDown={(e) => onHeaderKeyDown(e, v.index, row)}
                    className={cn(
                      "text-muted-foreground hover:text-foreground flex items-center gap-1 pr-3 pl-1.5 text-left text-[11px] font-semibold",
                      FOCUS_RING,
                      "data-[state=open]:ring-sidebar-ring/70 data-[state=open]:ring-1",
                    )}
                  >
                    <ChevronRight
                      aria-hidden
                      className={cn(
                        "size-3 shrink-0 transition-transform duration-100",
                        !row.collapsed && "rotate-90",
                      )}
                    />
                    <span className="truncate">{row.label}</span>
                    <span className="text-muted-foreground/70 ml-auto pl-2 font-normal tabular-nums">
                      {row.count}
                    </span>
                  </button>
                </ContextMenuTrigger>
                <ContextMenuContent>
                  <ContextMenuEntries
                    entries={headerMenu(
                      {
                        driver,
                        readOnly,
                        overview: OVERVIEW_HEADERS.has(row.label),
                        diagram: row.label === "Tables",
                      },
                      {
                        ...newHandlers,
                        itemOverview: () => overviewOf(null),
                        generateDiagram: () =>
                          openDiagram({ connectionId, schema: currentSchema, tables: null }),
                      },
                    )}
                  />
                </ContextMenuContent>
              </ContextMenu>
            );
          }
          if (row.type === "groupHeader") {
            const g = row.group;
            const manual = g.rules === null;
            const dropping = dropTarget === g.id;
            // A stored rule that is unsafe to run switches the group off; say why on its header.
            const warning = groupWarning(g);
            return (
              <ContextMenu key={row.key}>
                <ContextMenuTrigger asChild>
                  <button
                    type="button"
                    style={style}
                    data-row-index={v.index}
                    data-group={g.id}
                    data-drop-target={dropping || undefined}
                    aria-expanded={!row.collapsed}
                    title={warning ?? rulesSummary(g)}
                    onClick={() => {
                      if (pinScope !== null) setGroupCollapsed(pinScope, g.id, !row.collapsed);
                    }}
                    onKeyDown={(e) => onGroupHeaderKeyDown(e, v.index, row)}
                    onDragOver={(e) => {
                      if (!manual || !carriesItems(e)) return;
                      e.preventDefault();
                      e.dataTransfer.dropEffect = "move";
                      if (!dropping) setDropTarget(g.id);
                    }}
                    onDragLeave={() => {
                      if (dropping) setDropTarget(null);
                    }}
                    onDrop={(e) => {
                      if (!manual) return;
                      e.preventDefault();
                      setDropTarget(null);
                      const keys = draggedKeys(e);
                      if (pinScope !== null && keys.length > 0) addToGroup(pinScope, g.id, keys);
                    }}
                    className={cn(
                      "text-muted-foreground hover:text-foreground flex items-center gap-1 pr-3 pl-1.5 text-left text-[11px] font-semibold",
                      FOCUS_RING,
                      "data-[state=open]:ring-sidebar-ring/70 data-[state=open]:ring-1",
                      dropping &&
                        "bg-sidebar-accent/60 text-foreground ring-sidebar-ring/70 ring-1",
                    )}
                  >
                    <ChevronRight
                      aria-hidden
                      className={cn(
                        "size-3 shrink-0 transition-transform duration-100",
                        !row.collapsed && "rotate-90",
                      )}
                    />
                    <Folder aria-hidden className="size-3 shrink-0" />
                    <span className="truncate">{g.name}</span>
                    {warning && (
                      <TriangleAlert
                        aria-label={warning}
                        className="text-warning size-3 shrink-0"
                      />
                    )}
                    <span className="text-muted-foreground/70 ml-auto pl-2 font-normal tabular-nums">
                      {row.count}
                    </span>
                  </button>
                </ContextMenuTrigger>
                <ContextMenuContent>
                  <ContextMenuEntries
                    entries={groupHeaderMenu({
                      edit: () => {
                        if (pinScope !== null) openGroupDialog({ scope: pinScope, group: g });
                      },
                      remove: () => {
                        if (pinScope !== null) deleteGroup(pinScope, g.id);
                      },
                    })}
                  />
                </ContextMenuContent>
              </ContextMenu>
            );
          }
          if (row.type === "column") {
            return (
              <button
                key={row.key}
                type="button"
                style={style}
                data-row-index={v.index}
                data-column={row.name}
                title={`${row.name} · ${row.dataType}`}
                onClick={(e) => {
                  // DataGrid has no column focus to hand over: the table opens.
                  preview(row.table);
                  remember(row.table, e.detail > 0);
                }}
                onDoubleClick={() => {
                  openPinned(row.table);
                  remember(row.table, true);
                }}
                className={cn(
                  "flex items-center gap-1.5 pr-3 pl-10 text-left text-[11px] select-none",
                  FOCUS_RING,
                  "hover:bg-black/[0.035] dark:hover:bg-white/[0.05]",
                )}
              >
                {row.pk ? (
                  <KeyRound aria-label="Primary key" className="text-warning size-3 shrink-0" />
                ) : row.fk ? (
                  <ArrowUpRight
                    aria-label="Foreign key"
                    className="text-brand/80 size-3 shrink-0"
                  />
                ) : (
                  <span aria-hidden className="size-3 shrink-0" />
                )}
                <span className="truncate">{row.name}</span>
                <span className="text-muted-foreground/70 ml-auto truncate pl-2 font-mono text-[10px]">
                  {row.dataType}
                </span>
              </button>
            );
          }
          if (row.type === "columnStatus") {
            return (
              <div
                key={row.key}
                style={style}
                data-row-index={v.index}
                role="status"
                className="text-muted-foreground flex items-center gap-1.5 pr-3 pl-10 text-[11px]"
              >
                {row.status === "loading" ? (
                  <>
                    <Loader2 aria-hidden className="size-3 shrink-0 animate-spin" />
                    Loading columns…
                  </>
                ) : (
                  <span className="text-error truncate" title={row.message ?? undefined}>
                    {row.message ?? "Could not load the columns"}
                  </span>
                )}
              </div>
            );
          }
          const selected = selection.keys.has(row.key);
          const isActive =
            row.type === "table" &&
            activeTab !== null &&
            activeTab.kind !== "query" &&
            activeTab.connectionId === connectionId &&
            activeTab.table.name === row.table.name &&
            (activeTab.table.schema ?? "") === (row.table.schema ?? "");
          const rowClass = cn(
            "flex items-center gap-1.5 pr-3 pl-5 text-left text-xs select-none",
            FOCUS_RING,
            // The right-clicked row is outlined like a focused one while its menu is open.
            "data-[state=open]:ring-sidebar-ring/70 data-[state=open]:ring-1",
            isActive
              ? "bg-sidebar-accent text-sidebar-accent-foreground font-medium"
              : selected
                ? "bg-sidebar-accent/55 text-sidebar-accent-foreground"
                : "hover:bg-black/[0.035] dark:hover:bg-white/[0.05]",
          );
          const common = {
            style,
            "data-row-index": v.index,
            "data-selected": selected || undefined,
            onKeyDown: (e: KeyboardEvent) => onRowKeyDown(e, v.index, row),
            onContextMenu: () => {
              if (!selection.keys.has(row.key)) select(selectOnly(row.key));
            },
            onMouseDown: (e: MouseEvent) => {
              // Middle-click must not start autoscroll.
              if (e.button === 1) e.preventDefault();
            },
            // Tables, views and routines can be dragged onto a manual group.
            draggable: pinScope !== null && row.type !== "sequence",
            onDragStart: (e: DragEvent) => {
              e.dataTransfer.setData(DRAG_MIME, JSON.stringify(groupKeysOf(row)));
              e.dataTransfer.effectAllowed = "move";
            },
            onDragEnd: () => setDropTarget(null),
          };
          let button;
          let entries: () => MenuEntry[];
          if (row.type === "table") {
            const t = row.table;
            const isPinned = pinned.has(tableKey(t));
            const mark = markOf(row);
            const open = expanded.has(row.key);
            entries = () => tableEntries(row);
            button = (
              <button
                type="button"
                {...common}
                onClick={(e) => {
                  if (modifierClick(e, row.key)) return;
                  select(selectOnly(row.key));
                  preview(t);
                  // detail 0: activated from the keyboard (Space), nothing moves under a pointer.
                  remember(t, e.detail > 0);
                }}
                onDoubleClick={(e) => {
                  if (e.metaKey || e.ctrlKey || e.shiftKey) return;
                  openPinned(t);
                  remember(t, true);
                }}
                onAuxClick={(e) => {
                  if (e.button !== 1) return;
                  e.preventDefault();
                  openPinned(t);
                  remember(t, true);
                }}
                title={
                  mark === "drop"
                    ? "Delete pending: commit to drop it"
                    : mark === "truncate"
                      ? "Truncate pending: commit to empty it"
                      : (t.comment ?? undefined)
                }
                data-active={isActive || undefined}
                data-pending={mark}
                aria-expanded={showColumns ? open : undefined}
                className={rowClass}
              >
                {showColumns && (
                  <span
                    aria-hidden
                    data-slot="disclosure"
                    onClick={(e) => {
                      // The row's own click previews the table; the chevron only expands it.
                      e.stopPropagation();
                      toggleExpanded(row.key);
                    }}
                    onDoubleClick={(e) => e.stopPropagation()}
                    className="text-muted-foreground hover:text-foreground absolute top-1/2 left-1 flex size-3.5 -translate-y-1/2 items-center justify-center"
                  >
                    <ChevronRight
                      className={cn(
                        "size-3 transition-transform duration-100",
                        open && "rotate-90",
                      )}
                    />
                  </span>
                )}
                <TableIcon kind={t.kind} />
                <span
                  className={cn(
                    "truncate",
                    mark === "drop" && "text-muted-foreground line-through",
                  )}
                >
                  {t.name}
                </span>
                {mark === "truncate" && (
                  <span
                    aria-hidden
                    data-slot="pending-truncate"
                    className="border-warning/50 text-warning shrink-0 rounded-sm border px-1 text-[9px] leading-3 font-medium"
                  >
                    Truncate
                  </span>
                )}
                {isPinned && (
                  <Pin
                    aria-label="Pinned"
                    className="text-muted-foreground/60 size-3 shrink-0 -rotate-45"
                  />
                )}
                {showRowCounts && t.rowEstimate !== null && (
                  <span className="text-muted-foreground/70 ml-auto pl-2 font-mono text-[10px] tabular-nums">
                    {formatCount(t.rowEstimate, t.rowEstimate > 1000)}
                  </span>
                )}
              </button>
            );
          } else {
            const Icon = row.type === "routine" ? ROUTINE_ICONS[row.routine.kind] : ListOrdered;
            const overloaded =
              row.type === "routine" &&
              (routines.data ?? []).filter((r) => r.name === row.routine.name).length > 1;
            const dropping = markOf(row) === "drop";
            entries = () => objectEntries(row);
            button = (
              <button
                type="button"
                {...common}
                onClick={(e) => {
                  if (modifierClick(e, row.key)) return;
                  select(selectOnly(row.key));
                  // The second click of a double-click must not open a second tab.
                  if (e.detail <= 1) openObject(row);
                }}
                title={
                  dropping
                    ? "Delete pending: commit to drop it"
                    : row.type === "routine"
                      ? routineTitle(row.routine)
                      : [
                          `Sequence ${row.sequence.name}`,
                          row.sequence.dataType,
                          row.sequence.ownedBy && `owned by ${row.sequence.ownedBy}`,
                        ]
                          .filter(Boolean)
                          .join(" · ")
                }
                data-pending={dropping ? "drop" : undefined}
                className={rowClass}
              >
                <Icon
                  aria-hidden
                  className={cn(
                    "size-3.5 shrink-0",
                    row.type === "routine" ? "text-brand/80" : "text-muted-foreground",
                  )}
                />
                <span className={cn("truncate", dropping && "text-muted-foreground line-through")}>
                  {itemName(row)}
                </span>
                {overloaded && (
                  // Overloads share a name: their argument types tell them apart.
                  <span className="text-muted-foreground/70 truncate font-mono text-[10px]">
                    ({row.type === "routine" ? row.routine.identityArguments : ""})
                  </span>
                )}
              </button>
            );
          }
          return (
            <ContextMenu key={row.key}>
              <ContextMenuTrigger asChild>{button}</ContextMenuTrigger>
              <ContextMenuContent onCloseAutoFocus={onCloseAutoFocus}>
                <LazyEntries build={entries} />
              </ContextMenuContent>
            </ContextMenu>
          );
        })}
      </div>
    </div>
  );
}

/** Builds a row's menu only while it is open (the content mounts on open). */
function LazyEntries({ build }: { build: () => MenuEntry[] }) {
  return <ContextMenuEntries entries={build()} />;
}
