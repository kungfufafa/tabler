import { afterEach, describe, expect, it } from "vitest";
import { BASE_CAPS } from "@/lib/caps";
import type { ConnectionInfo, Driver } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { healthCheckTargets, pingsFreely } from "./useConnectionEvents";

function info(connectionId: string, driver: Driver, keepaliveSecs?: number | null): ConnectionInfo {
  const caps = BASE_CAPS[driver];
  return {
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
    ...(keepaliveSecs === undefined
      ? {}
      : { capabilities: { ...caps, session: { ...caps.session, keepaliveSecs } } }),
  };
}

afterEach(() => useConnectionsStore.setState({ byId: {}, order: [], activeId: null }));

describe("the health check's pings", () => {
  it("go to the core connections, as before", () => {
    useConnectionsStore.getState().add(info("pg", "postgres"));
    useConnectionsStore.getState().add(info("my", "mysql"));
    useConnectionsStore.getState().setStatus("my", "lost", "gone");
    // A lost connection is still checked: a recovery shows up.
    expect(healthCheckTargets()).toEqual(["pg", "my"]);
  });

  it("skip a session whose ping would bill or wake it", () => {
    useConnectionsStore.getState().add(info("pg", "postgres"));
    useConnectionsStore.getState().add(info("billed", "postgres", null));
    expect(healthCheckTargets()).toEqual(["pg"]);
    expect(pingsFreely(useConnectionsStore.getState().byId.billed)).toBe(false);
    expect(pingsFreely(undefined)).toBe(false);
  });

  it("skip a session of another driver that reported no caps (its base caps ping never)", () => {
    useConnectionsStore.getState().add(info("other", "sqlserver"));
    expect(BASE_CAPS.sqlserver.session.keepaliveSecs).toBeNull();
    expect(healthCheckTargets()).toEqual([]);
    useConnectionsStore.getState().add(info("reported", "sqlserver", 30));
    expect(healthCheckTargets()).toEqual(["reported"]);
  });
});
