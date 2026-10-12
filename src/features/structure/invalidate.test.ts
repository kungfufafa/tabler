import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { schemaKeys, useCurrentSchema, useTables } from "@/features/sidebar/useSchemaTree";
import { rowKeys } from "@/features/table-tab/useTableRows";
import { ipc, type SchemaInfo, type TableRef, type TableSummary } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { ddlKey, invalidateConnectionSchema, resetDatabaseCaches } from "./invalidate";

afterEach(() => {
  vi.restoreAllMocks();
});

const users: TableRef = { schema: "public", name: "users" };

/** A client holding one fresh query of every kind for connection `c1`, plus one of `c2`. */
function seeded() {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Infinity } },
  });
  const page = { columns: null, filter: null, sort: [], limit: 100, offset: 0 };
  const keys = {
    tables: schemaKeys.tables("c1", "public"),
    schemas: schemaKeys.schemas("c1"),
    databases: schemaKeys.databases("c1"),
    completion: schemaKeys.completion("c1"),
    ddl: ddlKey("c1", users),
    meta: rowKeys.meta("c1", users),
    rows: rowKeys.page("c1", users, page, 0),
    count: rowKeys.count("c1", users, null, 0),
  };
  for (const key of Object.values(keys)) qc.setQueryData(key, { seeded: true });
  const other = rowKeys.meta("c2", users);
  qc.setQueryData(other, { seeded: true });
  const state = (key: readonly unknown[]) =>
    qc.getQueryCache().find({ queryKey: key, exact: true });
  return { qc, keys, other, state };
}

describe("invalidateConnectionSchema", () => {
  it("marks every schema-derived query of the connection stale, and only those", () => {
    const { qc, keys, other, state } = seeded();
    invalidateConnectionSchema(qc, "c1");
    for (const [name, key] of Object.entries(keys)) {
      expect(state(key)?.state.isInvalidated, name).toBe(true);
      // Still cached: the old data shows until the refetch lands.
      expect(qc.getQueryData(key), name).toEqual({ seeded: true });
    }
    expect(state(other)?.state.isInvalidated).toBe(false);
  });
});

describe("resetDatabaseCaches", () => {
  it("drops the old database's metadata, rows, counts, DDL, completion and lists", () => {
    const { qc, keys, other, state } = seeded();
    resetDatabaseCaches(qc, "c1");
    for (const name of ["meta", "rows", "count", "ddl", "completion"] as const)
      expect(state(keys[name]), name).toBeUndefined();
    // The schema and table lists are gone too: they load again for the new database.
    for (const name of ["tables", "schemas"] as const)
      expect(qc.getQueryData(keys[name]), name).toBeUndefined();
    // The server's databases are the same: that list only refetches.
    expect(state(keys.databases)?.state.isInvalidated).toBe(true);
    expect(qc.getQueryData(keys.databases)).toEqual({ seeded: true });
    // Other connections keep theirs.
    expect(qc.getQueryData(other)).toEqual({ seeded: true });
    expect(state(other)?.state.isInvalidated).toBe(false);
  });

  it("never shows the old database's tables, nor picks its schema again (MySQL)", async () => {
    const info = await ipc.connections.connect("p-shop-mysql");
    const id = info.connectionId;
    useConnectionsStore.getState().add(info);
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const oldTables: TableSummary[] = [
      {
        name: "old_only_table",
        schema: "tabler_dev",
        kind: "table",
        rowEstimate: 1,
        sizeBytes: null,
        comment: null,
      },
    ];
    qc.setQueryData<SchemaInfo[]>(schemaKeys.schemas(id), [
      { name: "tabler_dev", isDefault: true },
    ]);
    qc.setQueryData(schemaKeys.tables(id, "tabler_dev"), oldTables);
    useConnectionsStore.getState().setCurrentSchema(id, "tabler_dev");
    // MySQL lists only the current database as its schema; hold the new list back.
    let resolveSchemas: (v: SchemaInfo[]) => void = () => {};
    vi.spyOn(ipc.schema, "schemas").mockImplementation(
      () => new Promise<SchemaInfo[]>((r) => (resolveSchemas = r)),
    );
    vi.spyOn(ipc.schema, "tables").mockImplementation((_c, schema) =>
      Promise.resolve(schema === "other_db" ? [] : oldTables),
    );
    const wrapper = ({ children }: { children: ReactNode }) =>
      createElement(QueryClientProvider, { client: qc }, children);
    const { result } = renderHook(
      () => {
        const schema = useCurrentSchema(id);
        return { schema, tables: useTables(id, schema).data };
      },
      { wrapper },
    );
    expect(result.current).toEqual({ schema: "tabler_dev", tables: oldTables });

    // DatabasePicker's onSuccess after `switch_database`.
    act(() => {
      useConnectionsStore.getState().setCurrentSchema(id, null);
      resetDatabaseCaches(qc, id);
    });
    expect(result.current.tables).toBeUndefined();
    expect(result.current.schema).toBeNull();
    expect(useConnectionsStore.getState().byId[id]?.currentSchema).toBeNull();

    act(() => resolveSchemas([{ name: "other_db", isDefault: true }]));
    await waitFor(() => expect(result.current).toEqual({ schema: "other_db", tables: [] }));
    expect(useConnectionsStore.getState().byId[id]?.currentSchema).toBe("other_db");
    await ipc.connections.disconnect(id);
  });
});
