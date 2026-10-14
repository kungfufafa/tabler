import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, renderHook, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ActionControl } from "@/app/ActionControl";
import { TooltipProvider } from "@/components/ui/tooltip";
import { saveQuery } from "@/features/editor/controller";
import { applyToEditor, getEditorState } from "@/features/editor/editorStates";
import { openSqlInNewQueryTab } from "@/features/editor/openSql";
import { openNewObject, type ObjectContext } from "@/features/objects/actions";
import { actions } from "@/features/palette/actions";
import { emptyStaging, pendingCount, setCell } from "@/features/staging/model";
import { useStructureStore } from "@/features/structure/store";
import {
  ipc,
  type ConnectionInfo,
  type TableRef,
  type Value,
  type WorkspaceGroup,
} from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore } from "@/stores/tabs";
import { definitionLoadedText, markDefinition, resetDefinitions } from "./definitions";
import { resetRowContexts, setRowContext } from "./rowContext";
import { resetCommitStore, useCommitStore } from "./store";
import { CommitTestHost } from "./testHost";
import {
  applyWorkspace,
  collectWorkspace,
  discardWorkspace,
  setCommitQueryClient,
  useWorkspacePendingCount,
  useWorkspaceSources,
  workspacePendingCount,
} from "./workspace";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));
vi.mock("@/features/safety/guard", () => ({ guardExecution: vi.fn(async () => true) }));

// CodeMirror (the SQL blocks) measures text ranges, which jsdom does not lay out.
if (!Range.prototype.getClientRects) {
  const empty = () => ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] });
  Range.prototype.getClientRects = empty as unknown as Range["getClientRects"];
  Range.prototype.getBoundingClientRect = () => new DOMRect();
}

const CONN = "c1";
const USERS: TableRef = { schema: "public", name: "users" };
const CTX: ObjectContext = {
  connectionId: CONN,
  driver: "postgres",
  readOnly: false,
  schema: "public",
  pinScope: null,
};
const int = (v: number): Value => ({ t: "int", v });
const text = (v: string): Value => ({ t: "text", v });

/** A table tab with one staged edit. */
function rowsTab() {
  const tabId = useTabsStore.getState().openTable(CONN, USERS, "table");
  useTableTabsStore.getState().ensure(tabId);
  useTableTabsStore.getState().update(tabId, {
    staging: setCell(emptyStaging(), "pk:1", 1, text("z"), [int(1), text("a")]),
    stagedOn: "id\u0000name",
  });
  setRowContext(tabId, {
    table: USERS,
    columns: [
      { name: "id", isPk: true },
      { name: "name", isPk: false },
    ].map((c, i) => ({
      ...c,
      ordinal: i + 1,
      dataType: i === 0 ? "int4" : "text",
      kind: i === 0 ? "int" : "text",
      nullable: !c.isPk,
      defaultExpr: null,
      isAutoIncrement: c.isPk,
    })) as never,
    signature: "id\u0000name",
    identity: "pk",
    keys: new Map([["pk:1", { kind: "pk", value: { id: int(1) } }]]),
  });
  return tabId;
}

const isDirty = (id: string) => {
  const tab = useTabsStore.getState().tabs.find((t) => t.id === id);
  return tab?.kind === "query" && tab.dirty === true;
};
const edit = (id: string, extra = "\n-- edited") =>
  applyToEditor(id, (s) => ({ changes: { from: s.doc.length, insert: extra } }));
const docOf = (id: string) => getEditorState(id)?.doc.toString();
/** Replaces the whole buffer, like select-all and type. */
const replace = (id: string, insert: string) =>
  applyToEditor(id, (s) => ({ changes: { from: 0, to: s.doc.length, insert } }));

function stubWorkspace() {
  const preview = vi.spyOn(ipc.workspace, "preview").mockImplementation(async (_c, groups) => ({
    groups: groups.map((g: WorkspaceGroup) => ({
      statements: [g.kind === "definition" ? g.sql : `${g.kind} statement`],
      warnings: [],
      transactional: true,
      error: null,
    })),
    atomic: true,
  }));
  const apply = vi.spyOn(ipc.workspace, "apply").mockImplementation(async (_c, groups) => ({
    success: true,
    atomic: true,
    groups: groups.map(() => ({
      status: "applied" as const,
      sql: [],
      executed: [],
      failedIndex: null,
      rows: [],
      routine: null,
      error: null,
    })),
    failedGroup: null,
    error: null,
    durationMs: 3,
  }));
  return { preview, apply };
}

beforeEach(() => {
  resetPrefsForTests({ ...DEFAULT_PREFS, confirmCommit: false });
  resetCommitStore();
  resetRowContexts();
  resetDefinitions();
  Object.values(toast).forEach((fn) => fn.mockReset());
  useConnectionsStore.setState({
    byId: {
      [CONN]: { info: { connectionId: CONN, driver: "postgres", name: "Local" } as ConnectionInfo },
    },
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
  useTableTabsStore.setState({ slices: {} });
  useStructureStore.setState({ byTab: {} });
  setCommitQueryClient(new QueryClient());
});

afterEach(() => vi.restoreAllMocks());

describe("a new view / function / routine edit is a pending change of the workspace", () => {
  it("a marked, edited query tab is collected as a `definition` source and counted", () => {
    const rows = rowsTab();
    const id = openNewObject(CTX, "view");
    // The template as it was opened is nothing to commit.
    expect(collectWorkspace(CONN).sources.map((s) => s.kind)).toEqual(["rows"]);
    expect(workspacePendingCount(CONN)).toBe(1);

    edit(id);
    const { sources, problems } = collectWorkspace(CONN);
    expect(problems).toEqual([]);
    // TablePlus's order: data edits first, then definitions.
    expect(sources.map((s) => s.kind)).toEqual(["rows", "definition"]);
    const def = sources[1]!;
    expect(def).toMatchObject({ kind: "definition", key: `def:${id}`, count: 1, drop: null });
    if (def.kind !== "definition") throw new Error("definition expected");
    expect(def.group).toEqual({ kind: "definition", drop: null, sql: docOf(id) });
    expect(def.sql).toBe(docOf(id));
    expect(workspacePendingCount(CONN)).toBe(2);
    expect(rows).toBeTruthy();
  });

  it("a clean template, a clean routine tab, an emptied buffer and an ordinary SQL tab are not pending", () => {
    const template = openNewObject(CTX, "function");
    const routine = openSqlInNewQueryTab(CONN, "CREATE FUNCTION f() ...", "postgres", "f");
    markDefinition(routine, { drop: null });
    const plain = openSqlInNewQueryTab(CONN, "select 1", "postgres", "scratch");
    edit(plain);
    expect(isDirty(template)).toBe(false);
    expect(workspacePendingCount(CONN)).toBe(0);
    // Cleared completely: dirty, but there is nothing to create.
    applyToEditor(routine, (s) => ({ changes: { from: 0, to: s.doc.length, insert: "" } }));
    expect(isDirty(routine)).toBe(true);
    expect(collectWorkspace(CONN).sources).toEqual([]);
    expect(workspacePendingCount(CONN)).toBe(0);
  });

  it("another connection's definition tabs are not this workspace's", () => {
    const id = openSqlInNewQueryTab("c2", "CREATE VIEW v AS SELECT 1", "postgres", "v");
    markDefinition(id, { drop: null });
    edit(id);
    expect(workspacePendingCount(CONN)).toBe(0);
    expect(workspacePendingCount("c2")).toBe(1);
  });

  it("commits together with a staged row edit in ONE apply call, and marks the tab saved", async () => {
    const rows = rowsTab();
    const id = openNewObject(CTX, "view");
    edit(id, "\n-- mine");
    const { apply } = stubWorkspace();

    // ⌘S inside the definition tab commits the whole workspace (the sheet first: a definition
    // always shows its SQL).
    expect(await saveQuery(id)).toBe(false);
    expect(useCommitStore.getState().sheet?.connectionId).toBe(CONN);
    expect(await applyWorkspace(CONN)).toBe(true);
    expect(apply).toHaveBeenCalledOnce();
    expect(apply.mock.calls[0]![1].map((g) => g.kind)).toEqual(["rows", "definition"]);
    expect(isDirty(id)).toBe(false);
    expect(pendingCount(useTableTabsStore.getState().slices[rows]!.staging)).toBe(0);
    expect(workspacePendingCount(CONN)).toBe(0);
    expect(toast.success).toHaveBeenCalledWith(
      expect.stringMatching(/^Committed 2 changes/),
      expect.objectContaining({ description: expect.stringContaining("Saved") }),
    );
  });

  it("⇧⌘⌫ puts a definition tab back to what it was opened with (⌘Z brings the edit back)", () => {
    const rows = rowsTab();
    const id = openNewObject(CTX, "view");
    const template = docOf(id)!;
    expect(definitionLoadedText(id)).toBe(template);
    edit(id, "\n-- my work");
    expect(workspacePendingCount(CONN)).toBe(2);

    expect(discardWorkspace(CONN)).toBe(2);
    expect(docOf(id)).toBe(template);
    expect(isDirty(id)).toBe(false);
    expect(pendingCount(useTableTabsStore.getState().slices[rows]!.staging)).toBe(0);
    expect(workspacePendingCount(CONN)).toBe(0);
    expect(toast.message).toHaveBeenCalledWith("Discarded 2 changes");
  });
});

describe("the toolbar, ⇧⌘P and ⇧⌘⌫ work on a new-view tab", () => {
  function renderHost() {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    return render(
      <QueryClientProvider client={qc}>
        <TooltipProvider>
          <ActionControl />
          <CommitTestHost />
        </TooltipProvider>
      </QueryClientProvider>,
    );
  }
  const control = () => screen.getByRole("group", { name: "Staged changes" });

  it("enables the action control, previews with the staged rows, and discards", async () => {
    stubWorkspace();
    rowsTab();
    const id = openNewObject(CTX, "view");
    renderHost();
    // Only the staged row so far: the template is not pending.
    expect(within(control()).getByRole("button", { name: "Commit 1 change" })).toBeEnabled();

    act(() => edit(id, "\n-- mine"));
    expect(within(control()).getByRole("button", { name: "Commit 2 changes" })).toBeEnabled();
    expect(actions.has("grid.preview")).toBe(true);
    expect(actions.has("grid.commit")).toBe(true);
    expect(actions.has("grid.discard")).toBe(true);

    // ⇧⌘P: one sheet with the rows and the definition.
    await act(() => actions.run("grid.preview"));
    const sheet = await screen.findByRole("dialog", { name: "Preview changes" });
    await waitFor(() => expect(sheet).toHaveTextContent("rows statement"));
    expect(within(sheet).getByRole("region", { name: /^Rows/ })).toBeInTheDocument();
    const def = within(sheet).getByRole("region", { name: /^Definition/ });
    await waitFor(() => expect(def).toHaveTextContent("-- mine"));
    act(() => useCommitStore.setState({ sheet: null }));

    // ⇧⌘⌫ (the control's Discard) drops both.
    await act(() => actions.run("grid.discard"));
    expect(within(control()).getByRole("button", { name: "Commit changes" })).toBeDisabled();
    expect(isDirty(id)).toBe(false);
  });

  it("a new-view tab alone enables Commit and Preview, which a clean one does not", async () => {
    stubWorkspace();
    const id = openNewObject(CTX, "view");
    renderHost();
    expect(within(control()).getByRole("button", { name: "Commit changes" })).toBeDisabled();
    expect(within(control()).getByRole("button", { name: "Preview SQL" })).toBeDisabled();
    expect(actions.has("grid.preview")).toBe(false);
    act(() => edit(id));
    expect(within(control()).getByRole("button", { name: "Commit 1 change" })).toBeEnabled();
    expect(within(control()).getByRole("button", { name: "Preview SQL" })).toBeEnabled();
    expect(actions.has("grid.preview")).toBe(true);
  });

  it("follows the typing: emptied, retyped, emptied again (the tab stays dirty all along)", () => {
    stubWorkspace();
    const id = openNewObject(CTX, "view");
    renderHost();
    const commit = () => within(control()).getByRole("button", { name: /^Commit/ });
    const preview = () => within(control()).getByRole("button", { name: "Preview SQL" });
    const discard = () => within(control()).getByRole("button", { name: /Discard/ });

    // Select-all + delete: dirty, but there is nothing to create.
    act(() => void replace(id, ""));
    expect(isDirty(id)).toBe(true);
    expect(commit()).toBeDisabled();
    expect(preview()).toBeDisabled();
    expect(discard()).toBeDisabled();
    expect(actions.has("grid.preview")).toBe(false);

    // Typing it again does not flip the dirty flag any more, yet the change is pending now.
    act(() => void replace(id, "create view v as select 1"));
    expect(isDirty(id)).toBe(true);
    expect(commit()).toBeEnabled();
    expect(commit()).toHaveAccessibleName("Commit 1 change");
    expect(preview()).toBeEnabled();
    expect(discard()).toBeEnabled();
    expect(actions.has("grid.preview")).toBe(true);
    expect(actions.has("grid.discard")).toBe(true);

    // Emptied again: no phantom change whose Commit does nothing.
    act(() => void replace(id, "  \n"));
    expect(commit()).toBeDisabled();
    expect(workspacePendingCount(CONN)).toBe(0);
    expect(actions.has("grid.preview")).toBe(false);
  });

  it("a routine tab cleared first and retyped is pending again", () => {
    stubWorkspace();
    const id = openSqlInNewQueryTab(CONN, "CREATE FUNCTION f() ...", "postgres", "f");
    markDefinition(id, {
      drop: { kind: "function", schema: "public", name: "f", identityArguments: "" },
    });
    renderHost();
    expect(within(control()).getByRole("button", { name: "Commit changes" })).toBeDisabled();
    act(() => void replace(id, ""));
    expect(within(control()).getByRole("button", { name: "Commit changes" })).toBeDisabled();
    act(() => void replace(id, "CREATE OR REPLACE FUNCTION f() ..."));
    expect(within(control()).getByRole("button", { name: "Commit 1 change" })).toBeEnabled();
  });

  it("the hooks behind it follow the typing: the count and the sheet's sources", () => {
    const id = openNewObject(CTX, "view");
    const count = renderHook(() => useWorkspacePendingCount(CONN));
    const sources = renderHook(() => useWorkspaceSources(CONN));
    expect(count.result.current).toBe(0);
    act(() => void replace(id, ""));
    expect(count.result.current).toBe(0);
    expect(sources.result.current.sources).toEqual([]);
    act(() => void replace(id, "create view v as select 1"));
    expect(count.result.current).toBe(1);
    expect(sources.result.current.sources.map((s) => s.kind)).toEqual(["definition"]);
    expect(sources.result.current.sources[0]).toMatchObject({ sql: "create view v as select 1" });
    act(() => void replace(id, "create view v as select 2"));
    expect(sources.result.current.sources[0]).toMatchObject({ sql: "create view v as select 2" });
    act(() => void replace(id, ""));
    expect(count.result.current).toBe(0);
    expect(sources.result.current.sources).toEqual([]);
  });
});
