import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { schemaKeys } from "@/features/sidebar/useSchemaTree";
import { ipc } from "@/lib/ipc";
import { actions } from "@/features/palette/actions";

/**
 * Query keys of the sidebar objects. They live under the table list's `["tables", c, schema]`
 * prefix, so everything that refreshes the table list (⌘R reload, DDL, a database switch, the
 * SQL editor's schema changes) refreshes routines, sequences and the overview too.
 */
/** PostgreSQL's system schemas, listed only with "Show System Schemas" on. */
export const PG_SYSTEM_SCHEMAS: readonly string[] = [
  "information_schema",
  "pg_catalog",
  "pg_toast",
];

export const objectKeys = {
  routines: (c: string, schema: string) => ["tables", c, schema, "routines"] as const,
  sequences: (c: string, schema: string) => ["tables", c, schema, "sequences"] as const,
  overview: (c: string, schema: string) => ["tables", c, schema, "overview"] as const,
  systemSchemas: (c: string) => ["tables", c, "", "systemSchemas"] as const,
};

export function useRoutines(connectionId: string | null, schema: string | null, enabled = true) {
  return useQuery({
    queryKey: objectKeys.routines(connectionId ?? "", schema ?? ""),
    queryFn: () => ipc.objects.routines(connectionId!, schema!),
    enabled: enabled && connectionId !== null && schema !== null,
    staleTime: 60_000,
    retry: false,
  });
}

export function useSequences(connectionId: string | null, schema: string | null, enabled = true) {
  return useQuery({
    queryKey: objectKeys.sequences(connectionId ?? "", schema ?? ""),
    queryFn: () => ipc.objects.sequences(connectionId!, schema!),
    enabled: enabled && connectionId !== null && schema !== null,
    staleTime: 60_000,
    retry: false,
  });
}

export function useSystemSchemas(connectionId: string | null, enabled = true) {
  return useQuery({
    queryKey: objectKeys.systemSchemas(connectionId ?? ""),
    queryFn: () => ipc.objects.systemSchemas(connectionId!),
    enabled: enabled && connectionId !== null,
    staleTime: 5 * 60_000,
    retry: false,
  });
}

export function useOverview(connectionId: string, schema: string | null, enabled = true) {
  return useQuery({
    queryKey: objectKeys.overview(connectionId, schema ?? ""),
    queryFn: () => ipc.objects.overview(connectionId, schema!),
    enabled: enabled && schema !== null,
    staleTime: 0,
    retry: false,
  });
}

/**
 * `view.refresh` (⌘R / ⌥⌘R, the toolbar's Refresh, View ▸ Refresh) reloads the open tab, and with
 * it the sidebar: the table list, routines and sequences of `schema` are refetched. Observes the
 * action rather than handling it, so whichever tab handles the refresh keeps doing so.
 */
export function useSidebarReload(connectionId: string, schema: string | null): void {
  const qc = useQueryClient();
  useEffect(() => {
    if (schema === null) return;
    return actions.observe("view.refresh", () => {
      void qc.invalidateQueries({ queryKey: schemaKeys.tables(connectionId, schema) });
    });
  }, [qc, connectionId, schema]);
}
