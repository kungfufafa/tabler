import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getEditorState } from "@/features/editor/editorStates";
import { useImportStore } from "@/features/import/store";
import { useGroupDialogStore } from "@/features/sidebar/groups";
import { ipc, type TableMeta } from "@/lib/ipc";
import { usePinsStore, resetPins } from "@/stores/pins";
import { useTabsStore } from "@/stores/tabs";
import {
  copyLaravelMigrations,
  importInto,
  openNewObject,
  runNew,
  scriptOf,
  togglePins,
  type ObjectContext,
} from "./actions";

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
  afterEach(() => vi.restoreAllMocks());

  it("opens New Group… for the sidebar's scope", () => {
    useGroupDialogStore.setState({ request: null });
    runNew({ ...CTX, pinScope: null }, "group");
    expect(useGroupDialogStore.getState().request).toBeNull();
    runNew(CTX, "group");
    expect(useGroupDialogStore.getState().request).toEqual({ scope: "p1|db" });
  });

  it("copies one Laravel migration per table, from its metadata", async () => {
    const writeText = vi.fn(async () => undefined);
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    const meta = (name: string) =>
      ({
        table: { schema: "public", name },
        kind: "table",
        columns: [],
        primaryKey: [],
        indexes: [],
        foreignKeys: [],
        comment: null,
      }) as unknown as TableMeta;
    const table = vi.spyOn(ipc.schema, "table").mockImplementation(async (_c, t) => meta(t.name));
    const summary = (name: string) => ({
      schema: "public",
      name,
      kind: "table" as const,
      rowEstimate: null,
      sizeBytes: null,
      comment: null,
    });
    await copyLaravelMigrations(CTX, [summary("users"), summary("posts")]);
    expect(table).toHaveBeenCalledTimes(2);
    const text = (writeText.mock.calls[0] as unknown as [string])[0];
    expect(text).toMatch(/^\/\/ ----- users -----\n<\?php/);
    expect(text).toContain("// ----- posts -----");
    expect(text).toContain("Schema::create('posts'");
    vi.unstubAllGlobals();
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
