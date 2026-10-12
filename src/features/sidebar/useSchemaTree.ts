import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo } from "react";
import { ipc, tableKey, type TableSummary } from "@/lib/ipc";
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
  | { type: "table"; key: string; table: TableSummary };

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
 * Flattens tables into header+row entries for the virtualized tree, filtered by `search`. With an
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
  const q = search.trim().toLowerCase();
  const matches = (t: TableSummary) => !q || t.name.toLowerCase().includes(q);
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
  if (!q && recent.length > 0) {
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
