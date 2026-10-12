import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo } from "react";
import { ipc, tableKey, type TableSummary } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { EMPTY_PINS } from "@/stores/pins";

export const schemaKeys = {
  databases: (c: string) => ["databases", c] as const,
  schemas: (c: string) => ["schemas", c] as const,
  tables: (c: string, schema: string) => ["tables", c, schema] as const,
  table: (c: string, schema: string | null, name: string) =>
    ["table", c, schema ?? "", name] as const,
  completion: (c: string) => ["completion", c] as const,
};

export type TreeRow =
  | { type: "header"; key: string; label: string; count: number }
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

const GROUP_ORDER: Array<[TableSummary["kind"][], string]> = [
  [["table", "partitioned", "foreignTable"], "Tables"],
  [["view", "systemView"], "Views"],
  [["materializedView"], "Materialized views"],
];

/**
 * Flattens tables into header+row entries for the virtualized tree, filtered by `search`. Pinned
 * tables (`tableKey`s) that match come first under "Pinned" (rows keyed `p:`), and stay in their
 * regular group too.
 */
export function buildTreeRows(
  tables: TableSummary[],
  search: string,
  pinned: ReadonlySet<string> = EMPTY_PINS,
): TreeRow[] {
  const q = search.trim().toLowerCase();
  const matches = (t: TableSummary) => !q || t.name.toLowerCase().includes(q);
  const rows: TreeRow[] = [];
  if (pinned.size > 0) {
    const items = tables.filter((t) => pinned.has(tableKey(t)) && matches(t));
    if (items.length > 0) {
      rows.push({ type: "header", key: "h:Pinned", label: "Pinned", count: items.length });
      for (const t of items)
        rows.push({ type: "table", key: `p:${t.schema ?? ""}.${t.name}`, table: t });
    }
  }
  for (const [kinds, label] of GROUP_ORDER) {
    const items = tables.filter((t) => kinds.includes(t.kind) && matches(t));
    if (items.length === 0) continue;
    rows.push({ type: "header", key: `h:${label}`, label, count: items.length });
    for (const t of items)
      rows.push({ type: "table", key: `t:${t.schema ?? ""}.${t.name}`, table: t });
  }
  return rows;
}

export function useTreeRows(
  tables: TableSummary[] | undefined,
  search: string,
  pinned: ReadonlySet<string> = EMPTY_PINS,
): TreeRow[] {
  return useMemo(() => buildTreeRows(tables ?? [], search, pinned), [tables, search, pinned]);
}
