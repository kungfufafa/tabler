import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { ActionControl } from "@/app/ActionControl";
import { TooltipProvider } from "@/components/ui/tooltip";
import { resetCommitStore } from "@/features/commit/store";
import { actions } from "@/features/palette/actions";
import { guardExecution } from "@/features/safety/guard";
import { useStructureStore } from "@/features/structure/store";
import { StructureTab } from "@/features/structure/StructureTab";
import {
  ipc,
  type ConnectionInfo,
  type DdlOp,
  type WorkspaceGroup,
  type WorkspaceResult,
} from "@/lib/ipc";
import { resetMockDdl } from "@/lib/ipc/mock/ddl";
import { useConnectionsStore } from "@/stores/connections";
import { hasUnsaved, useTabsStore } from "@/stores/tabs";
import { ObjectsHost } from "./ObjectsHost";
import {
  pendingObjectCount,
  pendingObjectOps,
  resetPendingObjectOps,
  stageObjectOp,
  type StagedObjectOp,
} from "./pendingOps";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));
vi.mock("@/features/safety/guard", () => ({ guardExecution: vi.fn(async () => true) }));

// CodeMirror (the SQL blocks) measures text ranges, which jsdom does not lay out.
if (!Range.prototype.getClientRects) {
  const empty = () => ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] });
  Range.prototype.getClientRects = empty as unknown as Range["getClientRects"];
  Range.prototype.getBoundingClientRect = () => new DOMRect();
}

// Runs against the in-memory mock backend (a PostgreSQL connection).

let info: ConnectionInfo;
/** The SQL a group is previewed with (the backend plans it; the test only needs distinct text). */
function sqlOfGroup(g: WorkspaceGroup): string[] {
  switch (g.kind) {
    case "rows":
      return ["UPDATE users SET name = 'x'"];
    case "ddl":
      return g.ops.map((o) => (o.op === "rawSql" ? o.sql : `ALTER TABLE users /* ${o.op} */`));
    case "definition":
      return [g.sql];
    case "object":
      switch (g.op.op) {
        case "truncateTables":
          return g.op.tables.map((t) => `TRUNCATE TABLE "${t.schema}"."${t.name}"`);
        case "dropSequences":
          return g.op.sequences.map((t) => `DROP SEQUENCE "${t.schema}"."${t.name}" CASCADE`);
        case "dropRoutines":
          return g.op.routines.map(
            (r) => `DROP FUNCTION "${r.schema}"."${r.name}"(${r.identityArguments})`,
          );
        default:
          return [`-- ${g.op.op}`];
      }
  }
}

/** Plans every group with `sqlOfGroup` and applies them all. */
function stubWorkspace() {
  vi.spyOn(ipc.workspace, "preview").mockImplementation(async (_c, groups) => ({
    groups: groups.map((g) => ({
      statements: sqlOfGroup(g),
      warnings: [],
      transactional: true,
      error: null,
    })),
    atomic: true,
  }));
  return vi.spyOn(ipc.workspace, "apply").mockImplementation(async (_c, groups) => ({
    success: true,
    atomic: true,
    groups: groups.map((g) => ({
      status: "applied" as const,
      sql: sqlOfGroup(g),
      executed: sqlOfGroup(g),
      failedIndex: null,
      rows: [],
      routine: null,
      error: null,
    })),
    failedGroup: null,
    error: null,
    durationMs: 1,
  }));
}

/** A commit that fails in group `at` (a single-transaction commit, so nothing stays applied). */
function failAt(groups: WorkspaceGroup[], at: number, message: string): WorkspaceResult {
  const error = {
    code: "DB_ERROR",
    message,
    detail: null,
    hint: null,
    sqlstate: null,
    vendorCode: null,
    position: null,
    statementIndex: 0,
    data: null,
  };
  return {
    success: false,
    atomic: true,
    groups: groups.map((g, i) => ({
      status: i === at ? ("failed" as const) : ("rolledBack" as const),
      sql: sqlOfGroup(g),
      executed: [],
      failedIndex: i === at ? 0 : null,
      rows: [],
      routine: null,
      error: i === at ? error : null,
    })),
    failedGroup: at,
    error,
    durationMs: 1,
  };
}

const truncate = (name: string): StagedObjectOp => ({
  op: "truncateTables",
  tables: [{ schema: "public", name }],
  cascade: false,
  restartIdentity: false,
  ignoreForeignKeys: false,
});

const dropSlugify: StagedObjectOp = {
  op: "dropRoutines",
  routines: [
    { schema: "public", name: "slugify", kind: "function", identityArguments: "value text" },
  ],
  cascade: false,
};

const dropSeq: StagedObjectOp = {
  op: "dropSequences",
  sequences: [{ schema: "public", name: "posts_id_seq" }],
  cascade: true,
};

/** The real structure view of `tabId` (while the tab is open and shows its structure). */
function StructureHost({ tabId }: { tabId: string }) {
  const tab = useTabsStore((s) => s.tabs.find((t) => t.id === tabId));
  return tab?.kind === "structure" ? <StructureTab tab={tab} /> : null;
}

function renderHost(structureTabId?: string) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <ActionControl />
        <ObjectsHost />
        {structureTabId && <StructureHost tabId={structureTabId} />}
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

const users = { schema: "public", name: "users" };

/** A structure tab of `users` (active), with `ddl` staged, shown by the real StructureTab. */
async function structureTab(ddl: DdlOp[] = []) {
  const tabId = useTabsStore.getState().openTable(info.connectionId, users, "structure");
  if (ddl.length > 0) useStructureStore.setState({ byTab: { [tabId]: { ops: ddl, past: [] } } });
  renderHost(tabId);
  await screen.findByRole("region", { name: "Columns" });
  return tabId;
}

const control = () => screen.getByRole("group", { name: "Staged changes" });

beforeAll(async () => {
  info = await ipc.connections.connect("p-local-pg");
});

afterAll(async () => {
  await ipc.connections.disconnect(info.connectionId);
});

describe("pending item operations", () => {
  beforeEach(() => {
    resetPendingObjectOps();
    resetCommitStore();
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
    useConnectionsStore.getState().add(info);
    useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
    useStructureStore.setState({ byTab: {} });
    vi.mocked(guardExecution).mockClear().mockResolvedValue(true);
    Object.values(toast).forEach((f) => f.mockReset());
  });
  afterEach(() => {
    resetMockDdl();
    vi.restoreAllMocks();
  });

  it("shows in the action control; Preview opens the workspace sheet, whose Commit runs them", async () => {
    const apply = stubWorkspace();
    renderHost();
    expect(within(control()).getByRole("button", { name: "Commit changes" })).toBeDisabled();
    act(() => {
      stageObjectOp(
        info.connectionId,
        truncate("posts"),
        ['TRUNCATE TABLE "public"."posts"'],
        null,
      );
    });
    expect(within(control()).getByRole("button", { name: "Commit 1 change" })).toBeEnabled();

    fireEvent.click(within(control()).getByRole("button", { name: "Preview SQL" }));
    const sheet = await screen.findByRole("dialog", { name: "Preview changes" });
    await waitFor(() => expect(sheet).toHaveTextContent('TRUNCATE TABLE "public"."posts";'));
    fireEvent.click(within(sheet).getByRole("button", { name: /^Commit/ }));
    await waitFor(() => expect(apply).toHaveBeenCalledOnce());
    expect(apply.mock.calls[0]![1]).toEqual([{ kind: "object", op: truncate("posts") }]);
    expect(guardExecution).toHaveBeenCalledWith(info.connectionId, {
      kind: "commit",
      statements: ['TRUNCATE TABLE "public"."posts"'],
    });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(within(control()).getByRole("button", { name: "Commit changes" })).toBeDisabled();
    expect(toast.success).toHaveBeenCalledWith(
      expect.stringMatching(/^Committed 1 change/),
      expect.objectContaining({ description: "Truncated posts" }),
    );
  });

  it("lists routine and sequence drops in the sheet; × takes one out, Commit runs the rest", async () => {
    const apply = stubWorkspace();
    renderHost();
    const c = info.connectionId;
    act(() => {
      stageObjectOp(c, dropSlugify, ['DROP FUNCTION "public"."slugify"(value text)'], null);
      stageObjectOp(c, dropSeq, ['DROP SEQUENCE "public"."posts_id_seq" CASCADE'], null);
    });
    expect(within(control()).getByRole("button", { name: "Commit 2 changes" })).toBeEnabled();

    fireEvent.click(within(control()).getByRole("button", { name: "Preview SQL" }));
    const sheet = await screen.findByRole("dialog", { name: "Preview changes" });
    const list = within(sheet).getByRole("region", { name: "Sidebar Item operations" });
    await waitFor(() =>
      expect(list).toHaveTextContent(
        'Drop function slugify(value text)DROP FUNCTION "public"."slugify"(value text);',
      ),
    );
    expect(list).toHaveTextContent(
      'Drop sequence posts_id_seqDROP SEQUENCE "public"."posts_id_seq" CASCADE;',
    );
    fireEvent.click(
      within(sheet).getByRole("button", { name: "Remove drop of function slugify(value text)" }),
    );
    expect(pendingObjectOps(c).map((o) => o.op)).toEqual([dropSeq]);

    fireEvent.click(within(sheet).getByRole("button", { name: /^Commit/ }));
    await waitFor(() => expect(apply).toHaveBeenCalledOnce());
    expect(apply.mock.calls[0]![1]).toEqual([{ kind: "object", op: dropSeq }]);
    expect(guardExecution).toHaveBeenCalledWith(c, {
      kind: "commit",
      statements: ['DROP SEQUENCE "public"."posts_id_seq" CASCADE'],
    });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(toast.success).toHaveBeenCalledWith(
      expect.stringMatching(/^Committed 1 change/),
      expect.objectContaining({ description: "Deleted posts_id_seq" }),
    );
    expect(pendingObjectCount(c)).toBe(0);
  });

  it("routine and sequence drops are the connection's work like the others", () => {
    renderHost();
    const c = info.connectionId;
    act(() => {
      stageObjectOp(c, dropSlugify, [], info.currentDatabase);
      stageObjectOp(c, dropSeq, [], info.currentDatabase);
    });
    expect(useTabsStore.getState().requestCloseConnection(c)).toBe(false);
    expect(useTabsStore.getState().closeRequest?.dirty).toMatchObject([{ count: 2 }]);
    act(() => useTabsStore.getState().cancelClose());

    // A reconnect keeps them; a database switch drops them.
    const next: ConnectionInfo = { ...info, connectionId: "reconnected" };
    act(() => {
      useConnectionsStore.setState((s) => {
        const byId = { ...s.byId };
        const old = byId[c]!;
        delete byId[c];
        byId[next.connectionId] = { ...old, info: next };
        return { byId, order: [next.connectionId], activeId: next.connectionId };
      });
    });
    expect(pendingObjectOps("reconnected").map((o) => o.op)).toEqual([dropSlugify, dropSeq]);
    act(() => useConnectionsStore.getState().setInfo({ ...next, currentDatabase: "other" }));
    expect(pendingObjectCount("reconnected")).toBe(0);
    expect(toast.message).toHaveBeenCalledWith("Discarded 2 pending item operations", {
      description: `${info.name} switched to another database.`,
    });
  });

  it("one ⌘S commits the structure tab's staged DDL and the pending operations together", async () => {
    const apply = stubWorkspace();
    const c = info.connectionId;
    const tabId = await structureTab([{ op: "rawSql", sql: "COMMENT ON TABLE users IS 'x'" }]);
    act(() => {
      stageObjectOp(c, truncate("posts"), [], null);
    });
    expect(within(control()).getByRole("button", { name: "Commit 2 changes" })).toBeEnabled();
    // ⌘S commits at once, staged structure included (⇧⌘P is the way to see the SQL first): no
    // second action, no sheet.
    await act(() => actions.run("grid.commit"));
    await waitFor(() => expect(apply).toHaveBeenCalledOnce());
    expect(screen.queryByRole("dialog", { name: "Preview changes" })).toBeNull();
    // ONE guard for every statement and ONE backend call: structure first, then the operations.
    expect(guardExecution).toHaveBeenCalledOnce();
    expect(guardExecution).toHaveBeenCalledWith(c, {
      kind: "commit",
      statements: ["COMMENT ON TABLE users IS 'x'", 'TRUNCATE TABLE "public"."posts"'],
    });
    expect(apply).toHaveBeenCalledOnce();
    expect(apply.mock.calls[0]![1].map((g) => g.kind)).toEqual(["ddl", "object"]);
    await waitFor(() => expect(pendingObjectCount(c)).toBe(0));
    await waitFor(() => expect(useStructureStore.getState().byTab[tabId]).toBeUndefined());
    expect(within(control()).getByRole("button", { name: "Commit changes" })).toBeDisabled();
  });

  it("a failed commit reverts everything: both stay pending, the failing part says why", async () => {
    const c = info.connectionId;
    const tabId = await structureTab([{ op: "rawSql", sql: "SELECT nosuchtype_x" }]);
    act(() => {
      stageObjectOp(c, truncate("posts"), [], null);
    });
    vi.spyOn(ipc.workspace, "preview").mockImplementation(async (_c, groups) => ({
      groups: groups.map((g) => ({
        statements: sqlOfGroup(g),
        warnings: [],
        transactional: true,
        error: null,
      })),
      atomic: true,
    }));
    vi.spyOn(ipc.workspace, "apply").mockImplementation(async (_c, groups) =>
      failAt(groups, 0, 'type "nosuchtype_x" does not exist'),
    );
    // ⌘S commits at once; the sheet opens only to show where it failed.
    await act(() => actions.run("grid.commit"));
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    // Nothing was cleared: the staged DDL and the operation are still pending.
    expect(pendingObjectCount(c)).toBe(1);
    expect(useStructureStore.getState().byTab[tabId]?.ops).toHaveLength(1);
    expect(toast.error).toHaveBeenCalledWith(
      "All changes were reverted",
      expect.objectContaining({ description: expect.stringContaining("nosuchtype_x") }),
    );
    // The sheet stays open on the failing tab's statement.
    const sheet = await screen.findByRole("dialog", { name: "Preview changes" });
    expect(within(sheet).getByRole("alert")).toHaveTextContent(/nosuchtype_x" does not exist/);
    expect(within(sheet).getByText("All changes were reverted.")).toBeInTheDocument();
    // ⇧⌘⌫ discards the operation along with the tab's changes.
    fireEvent.click(within(sheet).getByRole("button", { name: "Cancel" }));
    await act(() => actions.run("grid.discard"));
    expect(pendingObjectCount(c)).toBe(0);
    expect(useStructureStore.getState().byTab[tabId]).toBeUndefined();
  });

  it("a part that ran before a non-transactional failure leaves the pending list", async () => {
    const c = info.connectionId;
    stageObjectOp(c, truncate("posts"), ['TRUNCATE TABLE "public"."posts"'], null);
    stageObjectOp(c, dropSeq, ['DROP SEQUENCE "public"."posts_id_seq" CASCADE'], null);
    renderHost();
    stubWorkspace();
    vi.spyOn(ipc.objects, "preview").mockImplementation(async (_c, op) => ({
      statements:
        op.op === "truncateTables"
          ? op.tables.map((t) => `TRUNCATE TABLE "public"."${t.name}"`)
          : ['DROP SEQUENCE "public"."posts_id_seq" CASCADE'],
      warnings: [],
      transactional: false,
    }));
    vi.spyOn(ipc.workspace, "apply").mockImplementation(async (_c, groups) => {
      const error = {
        code: "DB_ERROR",
        message: "cannot drop sequence",
        detail: null,
        hint: null,
        sqlstate: null,
        vendorCode: null,
        position: null,
        statementIndex: 0,
        data: null,
      };
      return {
        success: false,
        atomic: false,
        groups: [
          {
            status: "applied" as const,
            sql: sqlOfGroup(groups[0]!),
            executed: sqlOfGroup(groups[0]!),
            failedIndex: null,
            rows: [],
            routine: null,
            error: null,
          },
          {
            status: "failed" as const,
            sql: sqlOfGroup(groups[1]!),
            executed: [],
            failedIndex: 0,
            rows: [],
            routine: null,
            error,
          },
        ],
        failedGroup: 1,
        error,
        durationMs: 1,
      };
    });
    await act(() => actions.run("grid.commit"));
    await waitFor(() => expect(pendingObjectOps(c).map((o) => o.op.op)).toEqual(["dropSequences"]));
    expect(pendingObjectOps(c)[0]!.error).toBe("cannot drop sequence");
    expect(toast.error).toHaveBeenCalledWith(
      "The commit stopped partway",
      expect.objectContaining({ description: expect.stringContaining("1 change before it") }),
    );
  });

  it("the Commit with a structure view in front runs the operations too; the sheet comes from ⇧⌘P", async () => {
    const apply = stubWorkspace();
    const c = info.connectionId;
    await structureTab();
    const stage = () =>
      act(() => {
        stageObjectOp(c, truncate("posts"), ['TRUNCATE TABLE "public"."posts"'], null);
      });

    stage();
    fireEvent.click(within(control()).getByRole("button", { name: "Commit 1 change" }));
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(pendingObjectCount(c)).toBe(0));

    stage();
    await act(() => actions.run("grid.commit"));
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(pendingObjectCount(c)).toBe(0));

    stage();
    await act(() => actions.run("grid.preview"));
    const sheet = await screen.findByRole("dialog", { name: "Preview changes" });
    fireEvent.click(within(sheet).getByRole("button", { name: /^Commit/ }));
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(3));
    expect(toast.message).not.toHaveBeenCalledWith("No structure changes to apply");
  });

  it("closing the structure tab with Discard drops its changes; the operations stay pending and nothing runs", async () => {
    const apply = stubWorkspace();
    const c = info.connectionId;
    const tabId = await structureTab([{ op: "rawSql", sql: "COMMENT ON TABLE users IS 'x'" }]);
    act(() => {
      stageObjectOp(c, truncate("posts"), [], info.currentDatabase);
    });
    expect(useTabsStore.getState().requestClose([tabId])).toBe(false);
    act(() => {
      useTabsStore.getState().confirmClose();
    });
    await new Promise((r) => setTimeout(r, 20));
    expect(apply).not.toHaveBeenCalled();
    expect(pendingObjectCount(c)).toBe(1);
  });

  it("the sheet's × and the control's Discard / Commit wait while they commit", async () => {
    let release!: () => void;
    stubWorkspace();
    vi.spyOn(ipc.workspace, "apply").mockImplementation(
      (_c, groups) =>
        new Promise<WorkspaceResult>((r) => {
          release = () =>
            r({
              success: true,
              atomic: true,
              groups: groups.map((g) => ({
                status: "applied" as const,
                sql: sqlOfGroup(g),
                executed: sqlOfGroup(g),
                failedIndex: null,
                rows: [],
                routine: null,
                error: null,
              })),
              failedGroup: null,
              error: null,
              durationMs: 1,
            });
        }),
    );
    renderHost();
    const c = info.connectionId;
    act(() => {
      stageObjectOp(c, truncate("posts"), [], null);
      stageObjectOp(c, truncate("tags"), [], null);
    });
    fireEvent.click(within(control()).getByRole("button", { name: "Preview SQL" }));
    const sheet = await screen.findByRole("dialog", { name: "Preview changes" });
    fireEvent.click(within(sheet).getByRole("button", { name: /^Commit/ }));
    const remove = await within(sheet).findByRole("button", { name: "Remove truncate of tags" });
    await waitFor(() => expect(remove).toBeDisabled());
    // The sheet is modal: the toolbar behind it is hidden from the accessibility tree.
    const toolbar = screen.getByRole("group", { name: "Staged changes", hidden: true });
    for (const name of ["Discard changes", "Commit 2 changes"])
      expect(within(toolbar).getByRole("button", { name, hidden: true })).toBeDisabled();
    await act(() => actions.run("grid.discard"));
    expect(pendingObjectCount(c)).toBe(2);
    release();
    await waitFor(() => expect(pendingObjectCount(c)).toBe(0));
  });

  it("belong to the connection: closing its tabs keeps them without asking", () => {
    renderHost();
    const c = info.connectionId;
    const table = useTabsStore.getState().openTable(c, users);
    const query = useTabsStore.getState().newQuery(c);
    act(() => {
      stageObjectOp(c, truncate("posts"), [], info.currentDatabase);
    });
    expect(useTabsStore.getState().requestClose([query])).toBe(true);
    expect(useTabsStore.getState().requestClose([table])).toBe(true);
    expect(useTabsStore.getState().tabs).toEqual([]);
    expect(pendingObjectCount(c)).toBe(1);
    // Still unsaved work: quit asks.
    expect(hasUnsaved()).toBe(true);
  });

  it("disconnect asks even with no tab open; Cancel keeps them, Discard drops them", () => {
    renderHost();
    const c = info.connectionId;
    act(() => {
      stageObjectOp(c, truncate("posts"), [], info.currentDatabase);
    });
    expect(useTabsStore.getState().requestCloseConnection(c)).toBe(false);
    expect(useTabsStore.getState().closeRequest?.dirty).toEqual([
      {
        id: `connection:${c}`,
        kind: "structure",
        title: `Pending item operations on ${info.name}`,
        count: 1,
        connectionId: c,
      },
    ]);
    act(() => useTabsStore.getState().cancelClose());
    expect(pendingObjectCount(c)).toBe(1);

    expect(useTabsStore.getState().requestCloseConnection(c)).toBe(false);
    act(() => {
      useTabsStore.getState().confirmClose();
    });
    expect(pendingObjectCount(c)).toBe(0);
    expect(hasUnsaved()).toBe(false);
    expect(toast.message).toHaveBeenCalledWith("Discarded 1 pending item operation", {
      description: "The connection's tabs were closed with Discard.",
    });
  });

  it("closing the window asks for them too", () => {
    renderHost();
    const c = info.connectionId;
    act(() => {
      stageObjectOp(c, truncate("posts"), [], info.currentDatabase);
    });
    expect(useTabsStore.getState().requestWindowClose()).toBe(false);
    expect(useTabsStore.getState().closeRequest).toMatchObject({ kind: "window", total: 1 });
    act(() => useTabsStore.getState().cancelClose());
  });

  it("asks before disconnect / quit, keeps them across a reconnect, drops them otherwise", () => {
    renderHost();
    const c = info.connectionId;
    useTabsStore.getState().openTable(c, { schema: "public", name: "users" });
    act(() => {
      stageObjectOp(c, truncate("posts"), [], info.currentDatabase);
    });
    expect(hasUnsaved()).toBe(true);
    expect(useTabsStore.getState().requestCloseConnection(c)).toBe(false);
    expect(useTabsStore.getState().closeRequest?.total).toBe(1);
    useTabsStore.getState().cancelClose();

    // A reconnect swaps the connection id (same profile and database): the ops follow.
    const next: ConnectionInfo = { ...info, connectionId: "reconnected" };
    act(() => {
      useConnectionsStore.setState((s) => {
        const byId = { ...s.byId };
        const old = byId[c]!;
        delete byId[c];
        byId[next.connectionId] = { ...old, info: next };
        return { byId, order: [next.connectionId], activeId: next.connectionId };
      });
    });
    expect(pendingObjectCount(c)).toBe(0);
    expect(pendingObjectCount("reconnected")).toBe(1);

    // Another database: they no longer apply.
    act(() => useConnectionsStore.getState().setInfo({ ...next, currentDatabase: "other" }));
    expect(pendingObjectOps("reconnected")).toEqual([]);

    act(() => {
      stageObjectOp("reconnected", truncate("posts"), [], "other");
    });
    act(() => useConnectionsStore.getState().remove("reconnected"));
    expect(pendingObjectCount("reconnected")).toBe(0);
  });
});
