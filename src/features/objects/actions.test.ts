import { beforeEach, describe, expect, it } from "vitest";
import { getEditorState } from "@/features/editor/editorStates";
import { useImportStore } from "@/features/import/store";
import { usePinsStore, resetPins } from "@/stores/pins";
import { useTabsStore } from "@/stores/tabs";
import { importInto, openNewObject, scriptOf, togglePins, type ObjectContext } from "./actions";

const CTX: ObjectContext = {
  connectionId: "c1",
  driver: "postgres",
  readOnly: false,
  schema: "public",
  pinScope: "p1|db",
};

const users = { schema: "public", name: "users" };
const posts = { schema: "public", name: "posts" };

describe("object actions", () => {
  beforeEach(() => {
    useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
    resetPins();
  });

  it("opens New View… in a SQL tab with the placeholder name selected", () => {
    const id = openNewObject(CTX, "view");
    expect(useTabsStore.getState().tabs).toEqual([
      expect.objectContaining({ id, kind: "query", title: "New View" }),
    ]);
    const state = getEditorState(id)!;
    const { from, to } = state.selection.main;
    expect(state.doc.sliceString(from, to)).toBe("new_view");
    expect(state.doc.toString()).toMatch(/^CREATE VIEW "public"\.new_view AS/);
  });

  it("pins every target, or unpins them when all are pinned", () => {
    const pins = () => usePinsStore.getState().pins["p1|db"] ?? [];
    togglePins("p1|db", [users]);
    expect(pins()).toEqual(["public.users"]);
    togglePins("p1|db", [users, posts]);
    expect(pins()).toEqual(["public.users", "public.posts"]);
    togglePins("p1|db", [users, posts]);
    expect(pins()).toEqual([]);
  });

  it("imports CSV into the clicked table, SQL dumps into the connection", () => {
    importInto(CTX, "csv", {
      ...users,
      kind: "table",
      rowEstimate: null,
      sizeBytes: null,
      comment: null,
    });
    expect(useImportStore.getState().request).toEqual({
      connectionId: "c1",
      kind: "csv",
      table: users,
    });
    importInto(CTX, "sql", null);
    expect(useImportStore.getState().request).toMatchObject({ kind: "sql", table: null });
  });

  it("joins statements into a script", () => {
    expect(scriptOf(["DROP TABLE a", "DROP TABLE b;"])).toBe("DROP TABLE a;\nDROP TABLE b;");
  });
});
