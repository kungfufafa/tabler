import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo } from "react";
import { compileSearch, isSearching, type MatchStrategy } from "@/features/objects/search";
import {
  ipc,
  tableKey,
  type RoutineSummary,
  type SequenceSummary,
  type TableMeta,
  type TableSummary,
} from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { EMPTY_PINS } from "@/stores/pins";
import { EMPTY_RECENT } from "@/stores/recent";
import { EMPTY_GROUPS, groupOf, routineItem, tableItem, type SidebarGroup } from "./groups";
import { EMPTY_COLLAPSED, type SidebarSection } from "./sidebarPrefs";

export const schemaKeys = {
  databases: (c: string) => ["databases", c] as const,
  schemas: (c: string) => ["schemas", c] as const,
  tables: (c: string, schema: string) => ["tables", c, schema] as const,
  table: (c: string, schema: string | null, name: string) =>
    ["table", c, schema ?? "", name] as const,
  completion: (c: string) => ["completion", c] as const,
};

export type TreeRow =
  | {
      type: "header";
      key: string;
      label: SidebarSection;
      /** Tables in the section (matching the search), shown or not. */
      count: number;
      /** The section's rows are left out of the list. */
      collapsed: boolean;
      /** Lazy load: the schema node the section belongs to. */
      schema?: string;
    }
  | {
      /**
       * Lazy load: a schema (PostgreSQL) or database (MySQL) whose items load once it is expanded;
       * its rows follow, keyed `n:<encoded schema>:…` (`nodePrefix`).
       */
      type: "schemaNode";
      key: string;
      schema: string;
      expanded: boolean;
      /** Items loaded for it (matching the search); null while not loaded. */
      count: number | null;
      loading: boolean;
      /** PostgreSQL's information_schema / pg_catalog / pg_toast ("Show System Schemas"). */
      system: boolean;
    }
  | {
      /** An expanded schema node whose items are loading or failed to load. */
      type: "nodeStatus";
      key: string;
      parentKey: string;
      status: "loading" | "error";
      message: string | null;
    }
  | {
      /** A sidebar group's header (`groups.ts`); its items follow, keyed `g:<id>:…`. */
      type: "groupHeader";
      key: string;
      group: SidebarGroup;
      /** Items in the group (matching the search), shown or not. */
      count: number;
      collapsed: boolean;
    }
  | { type: "table"; key: string; table: TableSummary }
  | { type: "routine"; key: string; routine: RoutineSummary }
  | { type: "sequence"; key: string; sequence: SequenceSummary }
  | {
      /** One column of an expanded table row ("Show Columns"). */
      type: "column";
      key: string;
      /** The table row it belongs to. */
      parentKey: string;
      table: TableSummary;
      name: string;
      dataType: string;
      pk: boolean;
      fk: boolean;
    }
  | {
      /** An expanded table row whose columns are loading or failed to load. */
      type: "columnStatus";
      key: string;
      parentKey: string;
      status: "loading" | "error";
      message: string | null;
    };

/** A row that stands for a database object (not a header nor a column). */
export type ItemRow = Extract<TreeRow, { type: "table" | "routine" | "sequence" }>;

export function isItemRow(row: TreeRow | undefined): row is ItemRow {
  return (
    row !== undefined && (row.type === "table" || row.type === "routine" || row.type === "sequence")
  );
}

/** Rows of a group section are keyed `g:<group id>:<item row key>` (after a `nodePrefix`). */
export function groupIdOfRow(key: string): string | null {
  const m = /^(?:n:[^:]*:)?g:([^:]+):/.exec(key);
  return m ? m[1]! : null;
}

/** Lazy load: the key prefix of a schema node's rows (`n:<encoded schema>:`). */
export function nodePrefix(schema: string): string {
  return `n:${encodeURIComponent(schema)}:`;
}

/** Lazy load: the schema node a row belongs to (by its key), or null outside the nodes. */
export function nodeSchemaOfRow(key: string): string | null {
  const m = /^n:([^:]*):/.exec(key);
  if (!m) return null;
  try {
    return decodeURIComponent(m[1]!);
  } catch {
    return null;
  }
}

/** The name an item row shows (and that Copy Name copies). */
export function itemName(row: ItemRow): string {
  switch (row.type) {
    case "table":
      return row.table.name;
    case "routine":
      return row.routine.name;
    case "sequence":
      return row.sequence.name;
  }
}

/** Overloads share a name: routine rows are keyed by their identity arguments too. */
export function routineKey(r: RoutineSummary): string {
  return `${r.schema ?? ""}.${r.name}(${r.identityArguments})`;
}

export function useDatabases(connectionId: string | null) {
  return useQuery({
    queryKey: schemaKeys.databases(connectionId ?? ""),
    queryFn: () => ipc.schema.databases(connectionId!),
    enabled: connectionId !== null,
    staleTime: 5 * 60_000,
  });
}

export function useSchemas(connectionId: string | null) {
  return useQuery({
    queryKey: schemaKeys.schemas(connectionId ?? ""),
    queryFn: () => ipc.schema.schemas(connectionId!),
    enabled: connectionId !== null,
    staleTime: 5 * 60_000,
  });
}

/** Resolves the schema shown in the sidebar for a connection (defaults to the server default). */
export function useCurrentSchema(connectionId: string | null): string | null {
  const schemas = useSchemas(connectionId);
  const selected = useConnectionsStore((s) =>
    connectionId ? (s.byId[connectionId]?.currentSchema ?? null) : null,
  );
  const setCurrentSchema = useConnectionsStore((s) => s.setCurrentSchema);
  const fallback = schemas.data?.find((s) => s.isDefault)?.name ?? schemas.data?.[0]?.name ?? null;
  useEffect(() => {
    if (connectionId && selected === null && fallback !== null)
      setCurrentSchema(connectionId, fallback);
  }, [connectionId, selected, fallback, setCurrentSchema]);
  return selected ?? fallback;
}

export function useTables(connectionId: string | null, schema: string | null) {
  return useQuery({
    queryKey: schemaKeys.tables(connectionId ?? "", schema ?? ""),
    queryFn: () => ipc.schema.tables(connectionId!, schema!),
    enabled: connectionId !== null && schema !== null,
    staleTime: 60_000,
  });
}

const GROUP_ORDER: Array<[TableSummary["kind"][], SidebarSection]> = [
  [["table", "partitioned", "foreignTable"], "Tables"],
  [["view", "systemView"], "Views"],
  [["materializedView"], "Materialized views"],
];

/**
 * Flattens tables into header+row entries for the virtualized tree, filtered by `search` (see
 * `compileSearch`: `strategy` terms, `^name$` anchors, `|` / `,` alternatives, "exact" quotes).
 * With an empty search, recently opened tables (`tableKey`s, most recent first) come first under
 * "Recent" (rows keyed `r:`); they stay in their regular section too. Pinned tables that match come
 * next under "Pinned" (rows keyed `p:`) and leave the alphabetical sections (TablePlus). Then every
 * sidebar group (`groups`, in their order, even an empty one: items are dragged into it) with the
 * tables it holds (rows keyed `g:<id>:t:`); those leave Tables / Views / Materialized views. A
 * `collapsed` section (or group) keeps its header (with its count) but leaves its rows out, so
 * ↑/↓ skip them.
 */
export function buildTreeRows(
  tables: TableSummary[],
  search: string,
  pinned: ReadonlySet<string> = EMPTY_PINS,
  recent: readonly string[] = EMPTY_RECENT,
  collapsed: ReadonlySet<SidebarSection> = EMPTY_COLLAPSED,
  groups: readonly SidebarGroup[] = EMPTY_GROUPS,
  strategy: MatchStrategy = "fuzzy",
): TreeRow[] {
  const matchName = compileSearch(search, strategy);
  const matches = (t: TableSummary) => matchName(t.name);
  const rows: TreeRow[] = [];
  const section = (label: SidebarSection, prefix: string, items: TableSummary[]) => {
    if (items.length === 0) return;
    const isCollapsed = collapsed.has(label);
    rows.push({
      type: "header",
      key: `h:${label}`,
      label,
      count: items.length,
      collapsed: isCollapsed,
    });
    if (isCollapsed) return;
    for (const t of items)
      rows.push({ type: "table", key: `${prefix}:${t.schema ?? ""}.${t.name}`, table: t });
  };
  if (!isSearching(search, strategy) && recent.length > 0) {
    const byKey = new Map(tables.map((t) => [tableKey(t), t]));
    section(
      "Recent",
      "r",
      recent.flatMap((k) => byKey.get(k) ?? []),
    );
  }
  if (pinned.size > 0)
    section(
      "Pinned",
      "p",
      tables.filter((t) => pinned.has(tableKey(t)) && matches(t)),
    );
  let ungrouped = tables;
  if (groups.length > 0) {
    const byGroup = new Map<string, TableSummary[]>();
    ungrouped = [];
    for (const t of tables) {
      const g = groupOf(groups, tableItem(t));
      if (!g) ungrouped.push(t);
      else if (matches(t)) byGroup.set(g.id, [...(byGroup.get(g.id) ?? []), t]);
    }
    for (const g of groups) {
      const items = byGroup.get(g.id) ?? [];
      rows.push({
        type: "groupHeader",
        key: `g:${g.id}`,
        group: g,
        count: items.length,
        collapsed: g.collapsed,
      });
      if (!g.collapsed)
        for (const t of items)
          rows.push({ type: "table", key: `g:${g.id}:t:${t.schema ?? ""}.${t.name}`, table: t });
    }
  }
  for (const [kinds, label] of GROUP_ORDER)
    section(
      label,
      "t",
      ungrouped.filter((t) => kinds.includes(t.kind) && !pinned.has(tableKey(t)) && matches(t)),
    );
  return rows;
}

export function useTreeRows(
  tables: TableSummary[] | undefined,
  search: string,
  pinned: ReadonlySet<string> = EMPTY_PINS,
  recent: readonly string[] = EMPTY_RECENT,
  collapsed: ReadonlySet<SidebarSection> = EMPTY_COLLAPSED,
  groups: readonly SidebarGroup[] = EMPTY_GROUPS,
  strategy: MatchStrategy = "fuzzy",
): TreeRow[] {
  return useMemo(
    () => buildTreeRows(tables ?? [], search, pinned, recent, collapsed, groups, strategy),
    [tables, search, pinned, recent, collapsed, groups, strategy],
  );
}

/** Where the Functions section goes: before the first of Tables / Views / Materialized views. */
const AFTER_FUNCTIONS = new Set<SidebarSection>(["Tables", "Views", "Materialized views"]);

/** Adds each group's routines after its tables (and to its count). */
function addGroupRoutines(
  rows: readonly TreeRow[],
  byGroup: ReadonlyMap<string, RoutineSummary[]>,
): TreeRow[] {
  const out: TreeRow[] = [];
  for (let i = 0; i < rows.length; i++) {
    const r = rows[i]!;
    const extra = r.type === "groupHeader" ? byGroup.get(r.group.id) : undefined;
    if (r.type !== "groupHeader" || !extra) {
      out.push(r);
      continue;
    }
    out.push({ ...r, count: r.count + extra.length });
    const prefix = `g:${r.group.id}:`;
    while (i + 1 < rows.length && rows[i + 1]!.key.startsWith(prefix)) out.push(rows[++i]!);
    if (!r.collapsed)
      for (const routine of extra)
        out.push({ type: "routine", key: `${prefix}f:${routineKey(routine)}`, routine });
  }
  return out;
}

/**
 * Adds the "Functions" (functions and procedures, before the tables) and "Sequences" (last)
 * sections to the table rows of `buildTreeRows`, filtered by the same `search`. Routines a group
 * holds (`groups`) are listed in that group instead. `undefined` lists (section turned off, or not
 * loaded) add nothing.
 */
export function mergeObjectRows(
  rows: readonly TreeRow[],
  routines: readonly RoutineSummary[] | undefined,
  sequences: readonly SequenceSummary[] | undefined,
  search: string,
  collapsed: ReadonlySet<SidebarSection> = EMPTY_COLLAPSED,
  groups: readonly SidebarGroup[] = EMPTY_GROUPS,
  strategy: MatchStrategy = "fuzzy",
): TreeRow[] {
  const matches = compileSearch(search, strategy);
  const byGroup = new Map<string, RoutineSummary[]>();
  const free: RoutineSummary[] = [];
  for (const r of routines ?? []) {
    if (!matches(r.name)) continue;
    const g = groups.length > 0 ? groupOf(groups, routineItem(r)) : undefined;
    if (g) byGroup.set(g.id, [...(byGroup.get(g.id) ?? []), r]);
    else free.push(r);
  }
  if (byGroup.size > 0) rows = addGroupRoutines(rows, byGroup);
  const section = (label: SidebarSection, items: ItemRow[]): TreeRow[] => {
    if (items.length === 0) return [];
    const isCollapsed = collapsed.has(label);
    const header: TreeRow = {
      type: "header",
      key: `h:${label}`,
      label,
      count: items.length,
      collapsed: isCollapsed,
    };
    return isCollapsed ? [header] : [header, ...items];
  };
  const functions = section(
    "Functions",
    free.map((routine) => ({ type: "routine", key: `f:${routineKey(routine)}`, routine })),
  );
  const seqs = section(
    "Sequences",
    (sequences ?? [])
      .filter((s) => matches(s.name))
      .map((sequence) => ({
        type: "sequence",
        key: `s:${sequence.schema ?? ""}.${sequence.name}`,
        sequence,
      })),
  );
  if (functions.length === 0 && seqs.length === 0) return rows as TreeRow[];
  let at = rows.findIndex((r) => r.type === "header" && AFTER_FUNCTIONS.has(r.label));
  if (at < 0) at = rows.length;
  return [...rows.slice(0, at), ...functions, ...rows.slice(at), ...seqs];
}

/** While searching, groups with no match leave the list (empty groups show otherwise). */
export function pruneEmptyGroups(
  rows: TreeRow[],
  search: string,
  strategy: MatchStrategy = "fuzzy",
): TreeRow[] {
  if (
    !isSearching(search, strategy) ||
    !rows.some((r) => r.type === "groupHeader" && r.count === 0)
  )
    return rows;
  return rows.filter((r) => r.type !== "groupHeader" || r.count > 0);
}

/** What an expanded table row lists under it. */
export type ColumnState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; meta: TableMeta };

/**
 * Inserts the columns of every `expanded` table row (by row key) after it ("Show Columns"): name,
 * type, primary / foreign key marks; or one loading / error row while `stateOf` has no metadata.
 */
export function withColumnRows(
  rows: TreeRow[],
  expanded: ReadonlySet<string>,
  stateOf: (table: TableSummary) => ColumnState,
): TreeRow[] {
  if (expanded.size === 0) return rows;
  const out: TreeRow[] = [];
  for (const r of rows) {
    out.push(r);
    if (r.type !== "table" || !expanded.has(r.key)) continue;
    const state = stateOf(r.table);
    if (state.status !== "ready") {
      out.push({
        type: "columnStatus",
        key: `${r.key}#status`,
        parentKey: r.key,
        status: state.status,
        message: state.status === "error" ? state.message : null,
      });
      continue;
    }
    const { meta } = state;
    const fks = new Set(meta.foreignKeys.flatMap((fk) => fk.columns));
    for (const c of [...meta.columns].sort((a, b) => a.ordinal - b.ordinal))
      out.push({
        type: "column",
        key: `${r.key}#c:${c.name}`,
        parentKey: r.key,
        table: r.table,
        name: c.name,
        dataType: c.dataType,
        pk: c.isPk || meta.primaryKey.includes(c.name),
        fk: fks.has(c.name),
      });
  }
  return out;
}
