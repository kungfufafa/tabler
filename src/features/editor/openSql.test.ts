import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useTabsStore } from "@/stores/tabs";
import { saveQuery } from "./controller";
import { getEditorState } from "./editorStates";
import * as files from "./files";
import { openSqlInNewQueryTab } from "./openSql";

afterEach(() => {
  vi.restoreAllMocks();
});

beforeEach(() => {
  useTabsStore.setState({
    tabs: [],
    activeId: null,
    closed: [],
    closeRequest: null,
    queryCounter: 0,
  });
});

describe("openSqlInNewQueryTab", () => {
  it("opens an active query tab that already holds the SQL", () => {
    const ddl = 'CREATE TABLE "public"."users" (\n  id bigint NOT NULL\n);';
    const id = openSqlInNewQueryTab("c1", ddl, "postgres", "users.sql");
    const { tabs, activeId } = useTabsStore.getState();
    expect(activeId).toBe(id);
    expect(tabs.find((t) => t.id === id)).toMatchObject({
      kind: "query",
      connectionId: "c1",
      title: "users.sql",
    });
    expect(getEditorState(id)?.doc.toString()).toBe(ddl);
    // Seeded as its saved text: nothing to lose when it is closed unchanged.
    const tab = tabs.find((t) => t.id === id);
    expect(tab?.kind === "query" && tab.dirty === true).toBe(false);
  });

  it("suggests the tab's title as the file name when saving, without a second .sql", async () => {
    const pick = vi.spyOn(files, "pickSavePath").mockResolvedValue(null);
    const id = openSqlInNewQueryTab("c1", "CREATE TABLE users (id int);", "postgres", "users.sql");
    await saveQuery(id);
    expect(pick).toHaveBeenLastCalledWith("users.sql");
    // A plain title gets the extension.
    const plain = openSqlInNewQueryTab("c1", "select 1", "postgres");
    await saveQuery(plain);
    expect(pick).toHaveBeenLastCalledWith("Query-2.sql");
  });

  it("keeps the default title without one, and opens a new tab every time", () => {
    const a = openSqlInNewQueryTab("c1", "select 1", "mysql");
    const b = openSqlInNewQueryTab("c1", "select 2", "mysql");
    expect(a).not.toBe(b);
    const titles = useTabsStore.getState().tabs.map((t) => t.title);
    expect(titles).toEqual(["Query 1", "Query 2"]);
    expect(getEditorState(b)?.doc.toString()).toBe("select 2");
    expect(useTabsStore.getState().activeId).toBe(b);
  });
});
