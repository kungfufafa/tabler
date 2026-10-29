import { afterEach, describe, expect, it } from "vitest";
import { useConnectionsStore } from "@/stores/connections";
import type { ConnectionInfo } from "./bindings/ConnectionInfo";
import type { Driver } from "./bindings/Driver";
import { IpcError, isConnectionFailure, type ErrorPayload } from "./transport";

const info = (connectionId: string, driver: Driver): ConnectionInfo => ({
  connectionId,
  profileId: `p-${connectionId}`,
  name: connectionId,
  color: null,
  driver,
  serverVersion: "1",
  currentDatabase: null,
  currentUser: null,
  tunnel: null,
  readOnly: false,
});

const failed = (connectionId: string, payload: Partial<ErrorPayload>) =>
  isConnectionFailure(
    "run_sql",
    { connectionId },
    new IpcError({ code: "DB_ERROR", message: "boom", ...payload }),
  );

afterEach(() => useConnectionsStore.setState({ byId: {}, order: [], activeId: null }));

describe("MySQL's gone-away codes", () => {
  it("count on a MySQL connection, and on one the store does not know", () => {
    useConnectionsStore.getState().add(info("my", "mysql"));
    expect(failed("my", { vendorCode: 2006 })).toBe(true);
    expect(failed("my", { vendorCode: 2013 })).toBe(true);
    expect(failed("unknown", { vendorCode: 2013 })).toBe(true);
  });

  it("mean nothing on another driver's connection", () => {
    // The catalog's drivers beyond MySQL: their error numbers are their own.
    const others = (["postgres", "sqlite", "sqlserver"] as const).map((d) => info(`c-${d}`, d));
    for (const c of others) useConnectionsStore.getState().add(c);
    for (const c of others) {
      expect(failed(c.connectionId, { vendorCode: 2006 })).toBe(false);
      // A connection exception is one whatever the engine.
      expect(failed(c.connectionId, { sqlstate: "08006" })).toBe(true);
    }
  });

  it("follow the store: a closed connection is forgotten, its id may come back as MySQL", () => {
    useConnectionsStore.getState().add(info("c1", "postgres"));
    expect(failed("c1", { vendorCode: 2006 })).toBe(false);
    useConnectionsStore.getState().remove("c1");
    expect(failed("c1", { vendorCode: 2006 })).toBe(true);
    // Any store update counts, a direct one (a reconnect's swap) too.
    useConnectionsStore.setState({
      byId: {
        c2: { info: info("c2", "sqlite"), status: "connected", message: null, currentSchema: null },
      },
      order: ["c2"],
    });
    expect(failed("c2", { vendorCode: 2013 })).toBe(false);
  });
});
