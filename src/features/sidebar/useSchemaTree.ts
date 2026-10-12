import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo } from "react";
import { compileSearch, isSearching } from "@/features/objects/search";
import {
  ipc,
  tableKey,
  type RoutineSummary,
  type SequenceSummary,
  type TableSummary,
} from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { EMPTY_PINS } from "@/stores/pins";
import { EMPTY_RECENT } from "@/stores/recent";
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
    }
  | { type: "table"; key: string; table: TableSummary }
  | { type: "routine"; key: string; routine: RoutineSummary }
  | { type: "sequence"; key: string; sequence: SequenceSummary };

/** A row that stands for a database object (not a section header). */
export type ItemRow = Exclude<TreeRow, { type: "header" }>;

export function isItemRow(row: TreeRow | undefined): row is ItemRow {
  return row !== undefined && row.type !== "header";
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
 * `compileSearch`: fuzzy terms, `^name$` anchors, `|` / `,` alternatives, "exact" quotes). With an
 * empty search, recently opened tables (`tableKey`s, most recent first) come first under "Recent"
 * (rows keyed `r:`). Pinned tables that match come next under "Pinned" (rows keyed `p:`). Both
 * stay in their regular group too. A `collapsed` section keeps its header (with its count) but
 * leaves its rows out, so ↑/↓ skip them.
 */
export function buildTreeRows(
  tables: TableSummary[],
  search: string,
  pinned: ReadonlySet<string> = EMPTY_PINS,
  recent: readonly string[] = EMPTY_RECENT,
  collapsed: ReadonlySet<SidebarSection> = EMPTY_COLLAPSED,
): TreeRow[] {
  const matchName = compileSearch(search);
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
  if (!isSearching(search) && recent.length > 0) {
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
  for (const [kinds, label] of GROUP_ORDER)
    section(
      label,
      "t",
      tables.filter((t) => kinds.includes(t.kind) && matches(t)),
    );
  return rows;
}

export function useTreeRows(
  tables: TableSummary[] | undefined,
  search: string,
  pinned: ReadonlySet<string> = EMPTY_PINS,
  recent: readonly string[] = EMPTY_RECENT,
  collapsed: ReadonlySet<SidebarSection> = EMPTY_COLLAPSED,
): TreeRow[] {
  return useMemo(
    () => buildTreeRows(tables ?? [], search, pinned, recent, collapsed),
    [tables, search, pinned, recent, collapsed],
  );
}

/** Where the Functions section goes: before the first of Tables / Views / Materialized views. */
const AFTER_FUNCTIONS = new Set<SidebarSection>(["Tables", "Views", "Materialized views"]);

/**
 * Adds the "Functions" (functions and procedures, before the tables) and "Sequences" (last)
 * sections to the table rows of `buildTreeRows`, filtered by the same `search`. `undefined`
 * lists (section turned off, or not loaded) add nothing.
 */
export function mergeObjectRows(
  rows: readonly TreeRow[],
  routines: readonly RoutineSummary[] | undefined,
  sequences: readonly SequenceSummary[] | undefined,
  search: string,
  collapsed: ReadonlySet<SidebarSection> = EMPTY_COLLAPSED,
): TreeRow[] {
  const matches = compileSearch(search);
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
    (routines ?? [])
      .filter((r) => matches(r.name))
      .map((routine) => ({ type: "routine", key: `f:${routineKey(routine)}`, routine })),
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
