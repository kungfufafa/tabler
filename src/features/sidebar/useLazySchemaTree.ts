import { useQueries, type UseQueryResult } from "@tanstack/react-query";
import { useEffect, useMemo } from "react";
import { objectKeys, PG_SYSTEM_SCHEMAS, useSystemSchemas } from "@/features/objects/queries";
import type { MatchStrategy } from "@/features/objects/search";
import {
  ipc,
  toIpcError,
  type RoutineSummary,
  type SequenceSummary,
  type TableSummary,
} from "@/lib/ipc";
import type { OpenConnection } from "@/stores/connections";
import type { SidebarGroup } from "./groups";
import { initLazyTree, lazyKey, useLazyTree } from "./lazyLoad";
import { buildLazyRows, type LazyNode } from "./lazyTree";
import { useSidebarPrefs, type SidebarSection } from "./sidebarPrefs";
import { schemaKeys, useCurrentSchema, useSchemas, type TreeRow } from "./useSchemaTree";

interface Loaded<T> {
  data: ReadonlyArray<T | undefined>;
  fetching: readonly boolean[];
  errors: ReadonlyArray<string | null>;
}

/** Stable (module-level) `combine`: the result keeps its identity until a query changes. */
function combineLoaded<T>(results: UseQueryResult<T, Error>[]): Loaded<T> {
  return {
    data: results.map((r) => r.data),
    fetching: results.map((r) => r.isFetching),
    errors: results.map((r) => (r.isError ? toIpcError(r.error).message : null)),
  };
}

const NO_NAMES: readonly string[] = [];

/**
 * The schemas (PostgreSQL) / databases (MySQL) of a lazy-loading connection, with the system
 * schemas when "Show System Schemas" is on, and the loaded ones' items: tables (always), routines
 * and sequences when asked for. Queries share the keys of the regular tree (`schemaKeys.tables`,
 * `objectKeys`), so a node, the regular tree and Open Anything never fetch the same list twice.
 */
export function useLazyItems(
  connection: OpenConnection | null,
  enabled: boolean,
  { routines = false, sequences = false }: { routines?: boolean; sequences?: boolean } = {},
) {
  const connectionId = enabled ? (connection?.info.connectionId ?? null) : null;
  const driver = connection?.info.driver ?? null;
  const key = lazyKey(
    connection?.info.connectionId ?? "",
    connection?.info.currentDatabase ?? null,
  );
  const schemas = useSchemas(connectionId);
  const showSystem = useSidebarPrefs((s) => s.options.showSystemSchemas) && driver === "postgres";
  const system = useSystemSchemas(connectionId, showSystem);
  const current = useCurrentSchema(connectionId);
  const tree = useLazyTree(key);

  // First look: the current (default) schema starts expanded.
  useEffect(() => {
    if (connectionId !== null && current !== null) initLazyTree(key, [current]);
  }, [connectionId, current, key]);

  const userNames = useMemo(() => (schemas.data ?? []).map((s) => s.name), [schemas.data]);
  const names = useMemo(() => {
    if (connectionId === null) return NO_NAMES;
    const out = [...userNames];
    if (showSystem) for (const s of system.data ?? []) if (!out.includes(s.name)) out.push(s.name);
    return out;
  }, [connectionId, userNames, showSystem, system.data]);
  const loaded = useMemo(() => new Set(tree.loaded), [tree.loaded]);
  const id = connectionId ?? "";

  const tables = useQueries({
    queries: names.map((s) => ({
      queryKey: schemaKeys.tables(id, s),
      queryFn: () => ipc.schema.tables(id, s),
      enabled: loaded.has(s),
      staleTime: 60_000,
    })),
    combine: combineLoaded<TableSummary[]>,
  });
  const routineLists = useQueries({
    queries: names.map((s) => ({
      queryKey: objectKeys.routines(id, s),
      queryFn: () => ipc.objects.routines(id, s),
      enabled: routines && loaded.has(s),
      staleTime: 60_000,
      retry: false,
    })),
    combine: combineLoaded<RoutineSummary[]>,
  });
  const sequenceLists = useQueries({
    queries: names.map((s) => ({
      queryKey: objectKeys.sequences(id, s),
      queryFn: () => ipc.objects.sequences(id, s),
      enabled: sequences && loaded.has(s),
      staleTime: 60_000,
      retry: false,
    })),
    combine: combineLoaded<SequenceSummary[]>,
  });

  const nodes = useMemo<LazyNode[]>(
    () =>
      names.map((schema, i) => ({
        schema,
        system: PG_SYSTEM_SCHEMAS.includes(schema) && !userNames.includes(schema),
        expanded: tree.expanded.includes(schema),
        // Only what is loaded counts (a cached list of a node not loaded here is not shown).
        tables: loaded.has(schema) ? tables.data[i] : undefined,
        routines: routines && loaded.has(schema) ? routineLists.data[i] : undefined,
        sequences: sequences && loaded.has(schema) ? sequenceLists.data[i] : undefined,
        loading: tables.fetching[i] ?? false,
        error: tables.errors[i] ?? null,
      })),
    [
      names,
      userNames,
      tree.expanded,
      loaded,
      tables,
      routines,
      routineLists,
      sequences,
      sequenceLists,
    ],
  );

  return {
    key,
    nodes,
    /** Every node's name (user schemas first). */
    names,
    /** Nodes whose items are not loaded (not searched by Open Anything). */
    notLoaded: names.filter((s) => !loaded.has(s)),
    /** The schema list itself is still loading. */
    pending: connectionId !== null && schemas.isPending,
    error: schemas.isError ? toIpcError(schemas.error).message : null,
  };
}

/** The sidebar's lazy-loaded tree rows (`buildLazyRows`) of a connection. */
export function useLazySchemaTree(
  connection: OpenConnection,
  enabled: boolean,
  o: {
    search: string;
    strategy: MatchStrategy;
    pinned: ReadonlySet<string>;
    recent: readonly string[];
    collapsed: ReadonlySet<SidebarSection>;
    groups: readonly SidebarGroup[];
  },
): { rows: TreeRow[]; loading: boolean; error: string | null } {
  const driver = connection.info.driver;
  const showFunctions = useSidebarPrefs((s) => s.options.showFunctions) && driver !== "sqlite";
  const showSequences = useSidebarPrefs((s) => s.options.showSequences) && driver === "postgres";
  const items = useLazyItems(connection, enabled, {
    routines: showFunctions,
    sequences: showSequences,
  });
  const { nodes } = items;
  const { search, strategy, pinned, recent, collapsed, groups } = o;
  const rows = useMemo(
    () =>
      enabled ? buildLazyRows(nodes, { search, strategy, pinned, recent, collapsed, groups }) : [],
    [enabled, nodes, search, strategy, pinned, recent, collapsed, groups],
  );
  return { rows, loading: items.pending, error: items.error };
}
