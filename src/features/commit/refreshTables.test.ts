import { beforeEach, describe, expect, it } from "vitest";
import { emptyStaging, insertRow } from "@/features/staging/model";
import type { TableRef } from "@/lib/ipc";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore } from "@/stores/tabs";
import { refreshOpenTableTabs } from "./refreshTables";

const USERS: TableRef = { schema: "public", name: "users" };
const POSTS: TableRef = { schema: "public", name: "posts" };

function tab(id: string, table: TableRef, connectionId = "c1", loaded = true) {
  useTabsStore.getState().open({ id, kind: "table", connectionId, table, title: table.name });
  if (loaded) useTableTabsStore.getState().ensure(id);
}
const token = (id: string) => useTableTabsStore.getState().slices[id]?.refreshToken;

beforeEach(() => {
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  useTableTabsStore.setState({ slices: {} });
});

describe("refreshOpenTableTabs", () => {
  it("bumps the refresh token of the other tabs on the same table of the connection", () => {
    tab("a", USERS);
    tab("b", USERS);
    tab("c", POSTS);
    tab("d", USERS, "c2");
    const done = refreshOpenTableTabs("c1", USERS, "a");
    expect(done).toEqual(["b"]);
    expect([token("a"), token("b"), token("c"), token("d")]).toEqual([0, 1, 0, 0]);
  });

  it("treats a missing schema like an empty one, and skips every tab named in `except`", () => {
    tab("a", { schema: null, name: "users" });
    tab("b", { schema: null, name: "users" });
    tab("c", { schema: null, name: "users" });
    expect(refreshOpenTableTabs("c1", { schema: null, name: "users" }, ["a", "b"])).toEqual(["c"]);
    expect([token("a"), token("b"), token("c")]).toEqual([0, 0, 1]);
  });

  it("leaves a tab with staged edits of its own alone: its rows must not shift", () => {
    tab("a", USERS);
    tab("held", USERS);
    useTableTabsStore.getState().update("held", {
      staging: insertRow(emptyStaging(), "new:1", []),
    });
    expect(refreshOpenTableTabs("c1", USERS)).toEqual(["a"]);
    expect(token("held")).toBe(0);
  });

  it("leaves a tab that never loaded its page alone, and never invents a slice for it", () => {
    tab("never", USERS, "c1", false);
    expect(refreshOpenTableTabs("c1", USERS)).toEqual([]);
    expect(token("never")).toBeUndefined();
  });

  it("covers the structure view of a table too, but no query tab", () => {
    useTabsStore.getState().open({
      id: "s",
      kind: "structure",
      connectionId: "c1",
      table: USERS,
      title: "users",
    });
    useTableTabsStore.getState().ensure("s");
    useTabsStore.getState().open({ id: "q", kind: "query", connectionId: "c1", title: "Query 1" });
    expect(refreshOpenTableTabs("c1", USERS)).toEqual(["s"]);
  });
});
