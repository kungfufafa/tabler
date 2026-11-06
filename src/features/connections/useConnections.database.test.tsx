import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { profile } from "@/lib/ipc/mock/fixtures";
import { usePaletteStore } from "@/features/palette/store";
import { useConnectionsStore } from "@/stores/connections";
import { profileKeys, useConnect } from "./useConnections";

beforeEach(() => {
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  usePaletteStore.getState().setOpen(false);
});
afterEach(() => vi.restoreAllMocks());

it.each([
  { driver: "mysql" as const, database: null, connectedDatabase: null },
  { driver: "mysql" as const, database: "shop", connectedDatabase: "shop" },
  { driver: "postgres" as const, database: null, connectedDatabase: "postgres" },
])(
  "opens the database chooser only when the profile has no database ($driver, $database)",
  async ({ driver, database, connectedDatabase }) => {
    const saved = profile({ id: "p1", name: "Local", driver, database });
    const info: ConnectionInfo = {
      connectionId: "c1",
      profileId: "p1",
      name: "Local",
      driver,
      currentDatabase: connectedDatabase,
      currentUser: "me",
      serverVersion: "8",
      color: null,
      tunnel: null,
      readOnly: false,
    };
    vi.spyOn(ipc.connections, "connect").mockResolvedValue(info);
    const qc = new QueryClient();
    qc.setQueryData(profileKeys.all, [saved]);
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={qc}>{children}</QueryClientProvider>
    );
    const { result } = renderHook(() => useConnect(), { wrapper });
    await act(async () => {
      await result.current.mutateAsync("p1");
    });
    expect(usePaletteStore.getState().open).toBe(database === null);
    if (database === null) expect(usePaletteStore.getState().mode).toBe("databases");
    expect(useConnectionsStore.getState().activeId).toBe("c1");
    expect(useConnectionsStore.getState().byId.c1!.info.currentDatabase).toBe(database);
  },
);
