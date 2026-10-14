import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";
import { saveQuery } from "@/features/editor/controller";
import { applyToEditor, getEditorState } from "@/features/editor/editorStates";
import * as files from "@/features/editor/files";
import { openSqlInNewQueryTab } from "@/features/editor/openSql";
import { openNewObject, openRoutine, type ObjectContext } from "@/features/objects/actions";
import {
  ipc,
  type ConnectionInfo,
  type DefinitionRestore,
  type RoutineRef,
  type WorkspaceGroup,
} from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import { useTabsStore } from "@/stores/tabs";
import {
  definitionLabel,
  definitionLoadedText,
  definitionOf,
  markDefinition,
  resetDefinitions,
  restoreNote,
} from "./definitions";
import { resetCommitStore, useCommitStore } from "./store";
import { applyWorkspace, collectWorkspace, setCommitQueryClient } from "./workspace";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));
const guard = vi.hoisted(() => ({ allow: true, calls: [] as unknown[] }));
vi.mock("@/features/safety/guard", () => ({
  guardExecution: async (connectionId: string, request: unknown) => {
    guard.calls.push({ connectionId, request });
    return guard.allow;
  },
}));

const CONN = "c1";
const ADD: RoutineRef = {
  schema: "public",
  name: "add",
  kind: "function",
  identityArguments: "a integer, b integer",
};
const TEXT =
  "CREATE OR REPLACE FUNCTION public.add(a integer, b integer) RETURNS integer AS $$ SELECT a + b $$ LANGUAGE sql;\n";

const CTX: ObjectContext = {
  connectionId: CONN,
  driver: "postgres",
  readOnly: false,
  schema: "public",
  pinScope: null,
};

function stub(
  over: {
    error?: string;
    executed?: string[];
    routine?: RoutineRef | null;
    restore?: DefinitionRestore;
  } = {},
) {
  const preview = vi.spyOn(ipc.workspace, "preview").mockImplementation(async (_c, groups) => ({
    groups: groups.map((g) => ({
      statements:
        g.kind === "definition"
          ? [...(g.drop ? [`DROP FUNCTION IF EXISTS ${g.drop.name}`] : []), g.sql.trim()]
          : [],
      warnings: [],
      transactional: true,
      error: null,
    })),
    atomic: true,
  }));
  const apply = vi.spyOn(ipc.workspace, "apply").mockImplementation(async () => {
    const error = over.error
      ? {
          code: "DB_ERROR",
          message: over.error,
          detail: null,
          hint: null,
          sqlstate: null,
          vendorCode: null,
          position: null,
          statementIndex: 1,
          data: null,
        }
      : null;
    return {
      success: error === null,
      atomic: true,
      groups: [
        {
          status: error ? ("failed" as const) : ("applied" as const),
          sql: [],
          executed: over.executed ?? [],
          failedIndex: error ? 1 : null,
          rows: [],
          routine: over.routine === undefined ? ADD : over.routine,
          ...(over.restore ? { restore: over.restore } : {}),
          error,
        },
      ],
      failedGroup: error ? 0 : null,
      error,
      durationMs: 2,
    };
  });
  return { preview, apply };
}

/** A SQL tab holding `TEXT`, marked as the definition of `drop`. */
function definitionTab(drop: RoutineRef | null = ADD, text = TEXT) {
  const id = openSqlInNewQueryTab(CONN, text, "postgres", "add");
  markDefinition(id, { drop });
  return id;
}

/** Edits the tab's text (a comment appended): it is now a pending change of the workspace. */
function edit(id: string, extra = "-- edited\n") {
  applyToEditor(id, (state) => ({ changes: { from: state.doc.length, insert: extra } }));
}

/**
 * ⌘S in the tab, then the Preview sheet's Commit (`applyWorkspace`): a definition always shows
 * its SQL first, so ⌘S itself only opens the sheet.
 */
async function saveAndCommit(id: string): Promise<boolean> {
  expect(await saveQuery(id)).toBe(false);
  expect(useCommitStore.getState().sheet?.connectionId).toBe(CONN);
  return applyWorkspace(CONN);
}

let queries: QueryClient;
let invalidate: MockInstance<QueryClient["invalidateQueries"]>;

const PREVIOUS = "CREATE DEFINER=`root`@`%` FUNCTION `shop`.`add`(a INT) RETURNS int RETURN a";

const queryTabs = () => useTabsStore.getState().tabs.filter((t) => t.kind === "query");

const isDirty = (id: string) => {
  const tab = useTabsStore.getState().tabs.find((t) => t.id === id);
  return tab?.kind === "query" && tab.dirty === true;
};

beforeEach(() => {
  resetPrefsForTests({ ...DEFAULT_PREFS, confirmCommit: false });
  resetDefinitions();
  resetCommitStore();
  guard.allow = true;
  guard.calls = [];
  useConnectionsStore.setState({
    byId: { [CONN]: { info: { connectionId: CONN, driver: "postgres" } as ConnectionInfo } },
    order: [CONN],
    activeId: CONN,
  } as never);
  useTabsStore.setState({
    tabs: [],
    activeId: null,
    closed: [],
    closeRequest: null,
    queryCounter: 0,
  });
  queries = new QueryClient();
  invalidate = vi.spyOn(queries, "invalidateQueries");
  setCommitQueryClient(queries);
});

afterEach(() => {
  vi.restoreAllMocks();
  Object.values(toast).forEach((fn) => fn.mockReset());
});

describe("⌘S in a routine's SQL tab drops and recreates it", () => {
  it("opens the Preview sheet; its Commit asks the safe mode, runs one definition group and marks the buffer saved", async () => {
    const { preview, apply } = stub();
    const id = definitionTab();
    // Edited since it was opened: dirty until saved.
    edit(id);
    expect(isDirty(id)).toBe(true);

    // ⌘S only opens the sheet: nothing runs before its Commit.
    expect(await saveQuery(id)).toBe(false);
    expect(useCommitStore.getState().sheet).toEqual({ connectionId: CONN, onCommitted: null });
    expect(apply).not.toHaveBeenCalled();
    expect(guard.calls).toEqual([]);

    expect(await applyWorkspace(CONN)).toBe(true);
    expect(preview).toHaveBeenCalledOnce();
    expect(guard.calls).toEqual([
      {
        connectionId: CONN,
        request: {
          kind: "commit",
          statements: ["DROP FUNCTION IF EXISTS add", `${TEXT}-- edited`],
        },
      },
    ]);
    const group = apply.mock.calls[0]![1][0] as WorkspaceGroup;
    expect(group).toEqual({ kind: "definition", drop: ADD, sql: `${TEXT}-- edited\n` });
    expect(useCommitStore.getState().sheet).toBeNull();
    expect(toast.success).toHaveBeenCalledWith(
      expect.stringMatching(/^Committed 1 change/),
      expect.objectContaining({ description: "Saved function add" }),
    );
    expect(isDirty(id)).toBe(false);
    // What it holds is now what the server has: the next Discard puts back THIS text.
    expect(definitionLoadedText(id)).toBe(`${TEXT}-- edited\n`);
  });

  it("does nothing, and says so, for a tab that was not edited", async () => {
    const { apply } = stub();
    const id = definitionTab();
    expect(await saveQuery(id)).toBe(false);
    expect(toast.message).toHaveBeenCalledWith("Nothing to commit", expect.anything());
    expect(useCommitStore.getState().sheet).toBeNull();
    expect(apply).not.toHaveBeenCalled();
  });

  it("keeps saving the routine its text now names", async () => {
    stub({ routine: { ...ADD, identityArguments: "a integer, b integer, c integer" } });
    const id = definitionTab();
    edit(id);
    await saveAndCommit(id);
    expect(definitionOf(id)?.drop).toEqual({
      ...ADD,
      identityArguments: "a integer, b integer, c integer",
    });
    // A template (no routine behind it) keeps creating.
    const template = definitionTab(null, "CREATE VIEW v AS SELECT 1");
    stub({ routine: null });
    edit(template, "\n-- more");
    await saveAndCommit(template);
    expect(definitionOf(template)?.drop).toBeNull();
    expect(toast.success).toHaveBeenLastCalledWith(
      expect.stringMatching(/^Committed 1 change/),
      expect.objectContaining({ description: "Saved view" }),
    );
  });

  it("sends nothing when the safe mode declines, and the edit stays pending", async () => {
    const { apply } = stub();
    const id = definitionTab();
    edit(id);
    await saveQuery(id);
    guard.allow = false;
    expect(await applyWorkspace(CONN)).toBe(false);
    expect(apply).not.toHaveBeenCalled();
    expect(useCommitStore.getState().sheet).not.toBeNull();
    expect(isDirty(id)).toBe(true);
  });

  it("keeps the sheet open on the reason it failed", async () => {
    stub({ error: "syntax error at or near AS" });
    const id = definitionTab();
    edit(id);
    expect(await saveAndCommit(id)).toBe(false);
    expect(useCommitStore.getState().sheet?.connectionId).toBe(CONN);
    expect(useCommitStore.getState().failure).toMatchObject({
      key: `def:${id}`,
      message: "syntax error at or near AS",
      restore: null,
    });
    // Still unsaved: the edit stays pending for the next try.
    expect(isDirty(id)).toBe(true);
    // Nothing was dropped and put back: no outcome, no extra tab, nothing to read again.
    expect(queryTabs()).toHaveLength(1);
    expect(invalidate).not.toHaveBeenCalled();
  });

  it("says the previous version was put back when MySQL's CREATE failed after the DROP", async () => {
    const restore: DefinitionRestore = {
      previous: PREVIOUS,
      restored: true,
      withoutDefiner: false,
      error: null,
    };
    stub({ error: "You have an error in your SQL syntax", executed: [], restore });
    const id = definitionTab();
    edit(id);
    expect(await saveAndCommit(id)).toBe(false);
    expect(useCommitStore.getState().failure).toMatchObject({
      message: "You have an error in your SQL syntax",
      restore,
    });
    expect(restoreNote(restore)).toMatch(/previous version was put back; nothing changed/);
    // Nothing to keep: no editor tab for the old text, and the tab stays a definition editor.
    expect(queryTabs()).toHaveLength(1);
    expect(definitionOf(id)).toEqual({ drop: ADD });
    expect(toast.success).not.toHaveBeenCalled();
    // The routine list is read again (the routine was dropped and created again).
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["tables", CONN] });
  });

  it("says when the old routine was put back without its DEFINER", () => {
    const note = restoreNote({
      previous: PREVIOUS,
      restored: true,
      withoutDefiner: true,
      error: null,
    });
    expect(note).toMatch(/without its DEFINER/);
    expect(note).toMatch(/connected account owns it/);
  });

  it("opens the old text in a new tab, behind the current one, when it could not be put back", async () => {
    const restore: DefinitionRestore = {
      previous: PREVIOUS,
      restored: false,
      withoutDefiner: false,
      error: {
        code: "DB_ERROR",
        message: "CREATE command denied to user 'ro'@'%' for routine 'shop.add'",
        detail: null,
        hint: null,
        sqlstate: null,
        vendorCode: 1044,
        position: null,
        statementIndex: null,
        data: null,
      },
    };
    stub({
      error: "You have an error in your SQL syntax",
      executed: ["DROP FUNCTION IF EXISTS `add`"],
      restore,
    });
    const id = definitionTab();
    useTabsStore.getState().activate(id);
    edit(id);
    expect(await saveAndCommit(id)).toBe(false);
    expect(useCommitStore.getState().failure).toMatchObject({
      message: "You have an error in your SQL syntax",
      restore,
    });
    const note = restoreNote(restore);
    expect(note).toContain("could not be put back");
    expect(note).toContain("CREATE command denied");
    expect(note).toContain("new editor tab");
    // The old version's text is in a tab of its own, where ⌘S creates it again ...
    const tabs = queryTabs();
    expect(tabs).toHaveLength(2);
    const kept = tabs.find((t) => t.id !== id)!;
    expect(kept.title).toBe("add (previous)");
    expect(getEditorState(kept.id)?.doc.toString()).toBe(`${PREVIOUS};\n`);
    expect(definitionOf(kept.id)).toEqual({ drop: null });
    // ... which is a pending creation (nothing exists on the server for that text any more) ...
    expect(isDirty(kept.id)).toBe(true);
    expect(collectWorkspace(CONN).sources.map((x) => x.key)).toEqual([
      `def:${id}`,
      `def:${kept.id}`,
    ]);
    // ... and the tab being saved is still the one in front, with its text.
    expect(useTabsStore.getState().activeId).toBe(id);
    expect(getEditorState(id)?.doc.toString()).toBe(`${TEXT}-- edited\n`);
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["tables", CONN] });
    // A second failure of the same save finds nothing to put back: no second tab.
    stub({ error: "You have an error in your SQL syntax" });
    await applyWorkspace(CONN);
    expect(queryTabs()).toHaveLength(2);
  });

  it("refuses text that is not a definition without opening anything", async () => {
    const { apply } = stub();
    vi.spyOn(ipc.workspace, "preview").mockImplementation(async () => ({
      groups: [
        {
          statements: [],
          warnings: [],
          transactional: true,
          error: {
            code: "INVALID_INPUT",
            message: "a definition starts with CREATE (CREATE VIEW / FUNCTION / PROCEDURE …)",
            detail: null,
            hint: null,
            sqlstate: null,
            vendorCode: null,
            position: null,
            statementIndex: null,
            data: null,
          },
        },
      ],
      atomic: true,
    }));
    const id = definitionTab(ADD, "DROP TABLE users");
    edit(id, "\n");
    expect(await saveAndCommit(id)).toBe(false);
    expect(apply).not.toHaveBeenCalled();
    expect(queryTabs()).toHaveLength(1);
    expect(toast.error).toHaveBeenCalledWith(expect.stringContaining("Could not commit"), {
      description: expect.stringContaining("starts with CREATE"),
    });
  });

  it("Save As and ordinary SQL tabs still save files", async () => {
    const { apply } = stub();
    const pick = vi.spyOn(files, "pickSavePath").mockResolvedValue(null);
    const plain = openSqlInNewQueryTab(CONN, "select 1", "postgres", "scratch");
    await saveQuery(plain);
    expect(pick).toHaveBeenCalledOnce();
    expect(apply).not.toHaveBeenCalled();
    // Save As on a definition tab still asks for a file.
    const id = definitionTab();
    edit(id);
    await saveQuery(id, { saveAs: true });
    expect(pick).toHaveBeenCalledTimes(2);
    expect(apply).not.toHaveBeenCalled();
    expect(useCommitStore.getState().sheet).toBeNull();
  });
});

describe("a MySQL routine opened from the sidebar saves as opened", () => {
  it("the DROP the opened text starts with is the save's own DROP, not a refusal", async () => {
    // The in-memory backend answers like the real one: the opened text is DROP … IF EXISTS, then CREATE.
    const c = (await ipc.connections.connect("p-shop-mysql")).connectionId;
    useConnectionsStore.setState({
      byId: { [c]: { info: { connectionId: c, driver: "mysql" } as ConnectionInfo } },
      order: [c],
      activeId: c,
    } as never);
    await openRoutine(
      { ...CTX, connectionId: c, driver: "mysql", schema: "shop" },
      {
        schema: "shop",
        name: "calc_total",
        kind: "function",
        arguments: "order_id int",
        identityArguments: "",
        returnType: "decimal(10,2)",
        language: "SQL",
        comment: null,
      },
    );
    const tab = queryTabs()[0]!;
    const opened = getEditorState(tab.id)!.doc.toString();
    expect(opened).toMatch(/^DROP FUNCTION IF EXISTS `calc_total`;\n\nCREATE DEFINER=/);

    // Saved with a trailing edit: the text the tab holds is the text as opened (its DROP line is
    // the save's own DROP, not a refusal), so the Preview SQL shows the DROP once, then the CREATE.
    edit(tab.id, "\n");
    expect(await saveQuery(tab.id)).toBe(false);
    expect(toast.error).not.toHaveBeenCalled();
    expect(useCommitStore.getState().sheet?.connectionId).toBe(c);
    const plan = await ipc.workspace.preview(
      c,
      collectWorkspace(c).sources.map((x) => x.group),
    );
    const statements = plan.groups[0]!.statements;
    expect(statements).toHaveLength(2);
    expect(statements[0]).toBe("DROP FUNCTION IF EXISTS `shop`.`calc_total`");
    expect(statements[1]).toMatch(
      /^CREATE DEFINER=`root`@`localhost` FUNCTION `shop`\.`calc_total`\(/,
    );
    // MySQL has no atomic replace: the sheet warns that the old version goes first.
    expect(plan.groups[0]!.warnings.join(" ")).toMatch(/cannot replace a routine in one step/);
    // MySQL commits a definition on its own, so the commit is not one transaction.
    expect(plan.atomic).toBe(false);
    expect(await applyWorkspace(c)).toBe(true);
    expect(toast.success).toHaveBeenCalledWith(
      expect.stringMatching(/^Committed 1 change/),
      expect.objectContaining({ description: "Saved function calc_total" }),
    );
    expect(useCommitStore.getState().sheet).toBeNull();
  });
});

describe("tabs that are definition editors", () => {
  it("a routine opened from the sidebar is marked with the routine to drop", async () => {
    vi.spyOn(ipc.objects, "routineDefinition").mockResolvedValue(TEXT);
    await openRoutine(CTX, {
      schema: "public",
      name: "add",
      kind: "function",
      arguments: "a integer, b integer",
      identityArguments: "a integer, b integer",
      returnType: "integer",
      language: "sql",
      comment: null,
    });
    const tab = useTabsStore.getState().tabs.find((t) => t.kind === "query")!;
    expect(tab.title).toBe("add");
    expect(definitionOf(tab.id)).toEqual({ drop: ADD });
  });

  it("New ▸ View / Function / Procedure templates are marked as creating", () => {
    for (const kind of ["view", "function", "procedure"] as const) {
      const id = openNewObject(CTX, kind);
      expect(definitionOf(id), kind).toEqual({ drop: null });
    }
  });

  it("names what a save created", () => {
    expect(definitionLabel("CREATE VIEW v AS SELECT 1", null)).toBe("view");
    expect(definitionLabel("-- note\nCREATE OR REPLACE VIEW v AS SELECT 1", null)).toBe("view");
    expect(definitionLabel("CREATE MATERIALIZED VIEW m AS SELECT 1", null)).toBe(
      "materialized view",
    );
    expect(definitionLabel(TEXT, ADD)).toBe("function add");
    expect(
      definitionLabel("CREATE PROCEDURE p() BEGIN END", { ...ADD, name: "p", kind: "procedure" }),
    ).toBe("procedure p");
    expect(definitionLabel("CREATE TRIGGER t", null)).toBe("definition");
  });
});
