import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as deeplink from "@/features/deeplink/open";
import * as viewer from "@/features/sqlviewer/open";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
// Loaded up front: the first IPC call would otherwise wait for every mock module to load.
import "@/lib/ipc/mock";
import { mockRecentItems, mockRecentMenuRebuilds, resetMockRecent } from "@/lib/ipc/mock/recent";
import { useConnectionsStore } from "@/stores/connections";
import { updateQueryTab, useQueryTabsStore } from "@/stores/queryTabs";
import { parseRecentMenuId } from "./menuIds";
import { openRecent } from "./openRecent";
import { installRecentTracking, trackRecentFile } from "./track";

const info = (id: string, profileId = `p-${id}`): ConnectionInfo => ({
  connectionId: id,
  profileId,
  name: id,
  color: null,
  driver: "postgres",
  serverVersion: "17",
  currentDatabase: "app",
  currentUser: "me",
  tunnel: null,
  readOnly: false,
});

const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  resetMockRecent();
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useQueryTabsStore.setState({ slices: {} });
});
afterEach(() => vi.restoreAllMocks());

describe("parseRecentMenuId", () => {
  it("reads Open Recent's item ids and nothing else", () => {
    expect(parseRecentMenuId("recent.connection:abc")).toEqual({
      kind: "connection",
      profileId: "abc",
    });
    expect(parseRecentMenuId("recent.file:/a/b c.sql")).toEqual({
      kind: "file",
      path: "/a/b c.sql",
    });
    for (const id of ["recent.file:", "recent.connection:", "recent.clear", "tab.close", 3, null])
      expect(parseRecentMenuId(id), String(id)).toBeNull();
  });
});

describe("installRecentTracking", () => {
  it("records connections as they open and query tab files as they change", async () => {
    const stop = installRecentTracking();
    useConnectionsStore.getState().add(info("c1", "p1"));
    useConnectionsStore.getState().add(info("c2", "p2"));
    // A status change is not a new connection.
    useConnectionsStore.getState().setStatus("c1", "reconnecting");
    updateQueryTab("t1", { filePath: "/q/a.sql" });
    updateQueryTab("t1", { resultsCollapsed: true });
    updateQueryTab("t2", { filePath: "/q/b.sql" });
    await vi.waitFor(() =>
      expect(mockRecentItems()).toEqual({
        connections: ["p2", "p1"],
        files: ["/q/b.sql", "/q/a.sql"],
      }),
    );
    expect(mockRecentMenuRebuilds()).toBe(4);
    stop();
    useConnectionsStore.getState().add(info("c3", "p3"));
    await flush();
    await flush();
    expect(mockRecentItems().connections).toEqual(["p2", "p1"]);
  });

  it("never reports a failure to the user", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.spyOn(ipc.recent, "add").mockRejectedValue(new Error("db locked"));
    trackRecentFile("/q/a.sql");
    await vi.waitFor(() => expect(warn).toHaveBeenCalled());
  });
});

describe("openRecent", () => {
  it("opens a connection through the profile opener", () => {
    const openProfile = vi.fn();
    openRecent({ kind: "connection", profileId: "p1" }, openProfile);
    expect(openProfile).toHaveBeenCalledWith("p1");
  });

  it("opens a file in a query tab of the active connection, else in the SQL Viewer", () => {
    const inTab = vi.spyOn(deeplink, "openSqlFile").mockResolvedValue(undefined);
    const inViewer = vi.spyOn(viewer, "openSqlViewer").mockImplementation(() => {});
    openRecent({ kind: "file", path: "/q/a.sql" }, vi.fn());
    expect(inViewer).toHaveBeenCalledWith("/q/a.sql");
    useConnectionsStore.getState().add(info("c1"));
    openRecent({ kind: "file", path: "/q/a.sql" }, vi.fn());
    expect(inTab).toHaveBeenCalledWith("c1", "/q/a.sql");
  });
});
