import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { ConnectionInfo, SessionCaps } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { BASE_CAPS, capsOf, has, isCoreSession, useCaps } from "./caps";

function info(over: Partial<ConnectionInfo> = {}): ConnectionInfo {
  return {
    connectionId: "c1",
    profileId: "p1",
    name: "Local",
    color: null,
    driver: "postgres",
    serverVersion: "17",
    currentDatabase: null,
    currentUser: null,
    tunnel: null,
    readOnly: false,
    ...over,
  };
}

/** A session that reported caps of its own: the editor and nothing else. */
const reported: SessionCaps = { ...BASE_CAPS.sqlserver, features: ["editor"] };

afterEach(() => useConnectionsStore.setState({ byId: {}, order: [], activeId: null }));

describe("caps", () => {
  it("are what the session reported, else what its driver starts from", () => {
    expect(capsOf(info({ capabilities: reported }))).toBe(reported);
    expect(capsOf(info())).toBe(BASE_CAPS.postgres);
    expect(capsOf(info({ capabilities: null }))).toBe(BASE_CAPS.postgres);
  });

  it("give the core drivers every feature and the others only what they grant", () => {
    for (const d of ["postgres", "mysql", "sqlite"] as const) {
      expect(has(info({ driver: d }), "editRows")).toBe(true);
      expect(has(info({ driver: d }), "backup")).toBe(true);
    }
    expect(BASE_CAPS.mongodb.features).toEqual(["structureView"]);
    expect(has(BASE_CAPS.mongodb, "structureView")).toBe(true);
    expect(has(BASE_CAPS.mongodb, "editRows")).toBe(false);
    expect(has(info({ driver: "postgres", capabilities: reported }), "editRows")).toBe(false);
    expect(has(info({ capabilities: reported }), "editor")).toBe(true);
  });

  it("tell a core session from a flavor that speaks its own dialect", () => {
    expect(isCoreSession(BASE_CAPS.postgres)).toBe(true);
    expect(isCoreSession(BASE_CAPS.mysql)).toBe(true);
    expect(isCoreSession({ ...BASE_CAPS.postgres, dialect: "redshift" })).toBe(false);
    expect(isCoreSession(BASE_CAPS.libsql)).toBe(false);
    expect(isCoreSession(BASE_CAPS.redis)).toBe(false);
  });

  it("follow an open connection in useCaps", () => {
    const { result } = renderHook(() => useCaps("c1"));
    expect(result.current).toBeNull();
    act(() => useConnectionsStore.getState().add(info()));
    expect(result.current).toBe(BASE_CAPS.postgres);
    act(() => useConnectionsStore.getState().setInfo(info({ capabilities: reported })));
    expect(result.current).toBe(reported);
    act(() => useConnectionsStore.getState().remove("c1"));
    expect(result.current).toBeNull();
    expect(renderHook(() => useCaps(null)).result.current).toBeNull();
  });
});
