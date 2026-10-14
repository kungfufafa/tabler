import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { ActionControl } from "@/app/ActionControl";
import { TooltipProvider } from "@/components/ui/tooltip";
import { actions } from "@/features/palette/actions";
import { guardExecution } from "@/features/safety/guard";
import { useStructureStore } from "@/features/structure/store";
import { StructureTab } from "@/features/structure/StructureTab";
import { ipc, type ConnectionInfo, type DdlOp, type DdlResult } from "@/lib/ipc";
import { resetMockDdl } from "@/lib/ipc/mock/ddl";
import { useConnectionsStore } from "@/stores/connections";
import { hasUnsaved, useTabsStore } from "@/stores/tabs";
import { cancelCommitChain, reportTabCommit } from "./commitChain";
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
const ok: DdlResult = { executed: ["TRUNCATE"], failedIndex: null, error: null, durationMs: 1 };

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
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
    useConnectionsStore.getState().add(info);
    useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
    useStructureStore.setState({ byTab: {} });
    vi.mocked(guardExecution).mockClear().mockResolvedValue(true);
    Object.values(toast).forEach((f) => f.mockReset());
  });
  afterEach(() => {
    cancelCommitChain();
    resetMockDdl();
    vi.restoreAllMocks();
  });

  it("shows in the action control; Preview opens their sheet, whose Commit runs them", async () => {
    const apply = vi.spyOn(ipc.objects, "apply").mockResolvedValue(ok);
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
    expect(sheet).toHaveTextContent('TRUNCATE TABLE "public"."posts";');
    fireEvent.click(within(sheet).getByRole("button", { name: /^Commit/ }));
    await waitFor(() => expect(apply).toHaveBeenCalledOnce());
    expect(guardExecution).toHaveBeenCalledWith(info.connectionId, {
      kind: "commit",
      statements: ['TRUNCATE TABLE "public"."posts"'],
    });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(within(control()).getByRole("button", { name: "Commit changes" })).toBeDisabled();
  });

  it("lists routine and sequence drops in the sheet; × takes one out, Commit runs the rest", async () => {
    const apply = vi.spyOn(ipc.objects, "apply").mockResolvedValue(ok);
    renderHost();
    const c = info.connectionId;
    act(() => {
      stageObjectOp(c, dropSlugify, ['DROP FUNCTION "public"."slugify"(value text)'], null);
      stageObjectOp(c, dropSeq, ['DROP SEQUENCE "public"."posts_id_seq" CASCADE'], null);
    });
    expect(within(control()).getByRole("button", { name: "Commit 2 changes" })).toBeEnabled();

    fireEvent.click(within(control()).getByRole("button", { name: "Preview SQL" }));
    const sheet = await screen.findByRole("dialog", { name: "Preview changes" });
    const list = within(sheet).getByRole("region", { name: "Item operations" });
    expect(list).toHaveTextContent(
      'Drop function slugify(value text)DROP FUNCTION "public"."slugify"(value text);',
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
    expect(apply).toHaveBeenCalledWith(c, dropSeq);
    expect(guardExecution).toHaveBeenCalledWith(c, {
      kind: "commit",
      statements: ['DROP SEQUENCE "public"."posts_id_seq" CASCADE'],
    });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(toast.success).toHaveBeenCalledWith("Deleted posts_id_seq");
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

  it("⌘S commits the active tab's changes first, then the operations", async () => {
    const order: string[] = [];
    vi.spyOn(ipc.objects, "apply").mockImplementation(async () => {
      order.push("operations");
      return ok;
    });
    const c = info.connectionId;
    const table = { schema: "public", name: "users" };
    useTabsStore.getState().openTable(c, table, "structure");
    const tabId = useTabsStore.getState().activeId!;
    useStructureStore.setState({
      byTab: {
        [tabId]: { ops: [{ op: "rawSql", sql: "COMMENT ON TABLE users IS 'x'" }], past: [] },
      },
    });
    renderHost();
    act(() => {
      stageObjectOp(c, truncate("posts"), [], null);
    });
    expect(within(control()).getByRole("button", { name: "Commit 2 changes" })).toBeEnabled();
    // A tab's commit handler (on top of the fallback): the ops wait for its report.
    const off = actions.register("grid.commit", async () => {
      order.push("tab");
      await ipc.ddl.apply(c, table, useStructureStore.getState().byTab[tabId]!.ops);
      useStructureStore.setState({ byTab: {} });
      reportTabCommit(tabId, "committed");
    });
    try {
      fireEvent.click(within(control()).getByRole("button", { name: "Commit 2 changes" }));
      await waitFor(() => expect(order).toEqual(["tab", "operations"]));
      await waitFor(() => expect(pendingObjectCount(c)).toBe(0));
    } finally {
      off();
    }
  });

  it("⇧⌘⌫ with a tab's changes discards the operations too; a failed tab commit keeps them", async () => {
    const apply = vi.spyOn(ipc.objects, "apply").mockResolvedValue(ok);
    const c = info.connectionId;
    const table = { schema: "public", name: "users" };
    useTabsStore.getState().openTable(c, table, "structure");
    const tabId = useTabsStore.getState().activeId!;
    const stage = () =>
      useStructureStore.setState({
        byTab: { [tabId]: { ops: [{ op: "rawSql", sql: "SELECT nosuchtype_x" }], past: [] } },
      });
    stage();
    renderHost();
    act(() => {
      stageObjectOp(c, truncate("posts"), [], null);
    });
    // The tab's commit fails (the mock rejects the unknown type): the operations wait.
    const offCommit = actions.register("grid.commit", async () => {
      const r = await ipc.ddl.apply(c, table, useStructureStore.getState().byTab[tabId]!.ops);
      reportTabCommit(tabId, r.error ? "failed" : "committed");
    });
    const offDiscard = actions.register("grid.discard", () => {
      useStructureStore.setState({ byTab: {} });
    });
    try {
      await act(() => actions.run("grid.commit"));
      expect(apply).not.toHaveBeenCalled();
      expect(pendingObjectCount(c)).toBe(1);
      await act(() => actions.run("grid.discard"));
      expect(pendingObjectCount(c)).toBe(0);
      expect(apply).not.toHaveBeenCalled();
    } finally {
      offCommit();
      offDiscard();
    }
  });

  it("with nothing staged in a structure view, ⌘S / ⇧⌘P and the Commits run the operations", async () => {
    const apply = vi.spyOn(ipc.objects, "apply").mockResolvedValue(ok);
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

  it("⌘S runs the operations once the structure view applied its changes", async () => {
    const order: string[] = [];
    const ddl = vi.spyOn(ipc.ddl, "apply");
    vi.spyOn(ipc.objects, "apply").mockImplementation(async () => {
      order.push(`operations after ${ddl.mock.calls.length} structure apply`);
      return ok;
    });
    const c = info.connectionId;
    await structureTab([{ op: "rawSql", sql: "COMMENT ON TABLE users IS 'x'" }]);
    act(() => {
      stageObjectOp(c, truncate("posts"), [], null);
    });
    await act(() => actions.run("grid.commit"));
    const sheet = await screen.findByRole("dialog", { name: "Review structure changes" });
    await waitFor(() =>
      expect(within(sheet).getByRole("button", { name: /^Apply/ })).toBeEnabled(),
    );
    fireEvent.click(within(sheet).getByRole("button", { name: /^Apply/ }));
    await waitFor(() => expect(order).toEqual(["operations after 1 structure apply"]));
  });

  it("a dismissed structure preview drops them from ⌘S: later loads and Discard run nothing", async () => {
    const apply = vi.spyOn(ipc.objects, "apply").mockResolvedValue(ok);
    const c = info.connectionId;
    const tabId = await structureTab([{ op: "rawSql", sql: "COMMENT ON TABLE users IS 'x'" }]);
    act(() => {
      stageObjectOp(c, truncate("posts"), [], info.currentDatabase);
    });
    await act(() => actions.run("grid.commit"));
    const sheet = await screen.findByRole("dialog", { name: "Review structure changes" });
    fireEvent.click(within(sheet).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // Another tab's grid loads its rows (logged as data statements of the connection).
    await ipc.browse.rows(c, {
      table: { schema: "public", name: "posts" },
      columns: null,
      filter: null,
      sort: [],
      limit: 10,
      offset: 0,
    });
    // The structure tab is closed with Discard: its changes go; the operations, which belong to
    // the connection, stay pending — and nothing runs.
    expect(useTabsStore.getState().requestClose([tabId])).toBe(false);
    act(() => {
      useTabsStore.getState().confirmClose();
    });
    await new Promise((r) => setTimeout(r, 20));
    expect(apply).not.toHaveBeenCalled();
    expect(pendingObjectCount(c)).toBe(1);
  });

  it("a failed structure apply drops them from ⌘S, even when retried from its sheet", async () => {
    const apply = vi.spyOn(ipc.objects, "apply").mockResolvedValue(ok);
    const c = info.connectionId;
    const tabId = await structureTab([{ op: "rawSql", sql: "SELECT nosuchtype_x" }]);
    act(() => {
      stageObjectOp(c, truncate("posts"), [], null);
    });
    await act(() => actions.run("grid.commit"));
    const sheet = await screen.findByRole("dialog", { name: "Review structure changes" });
    await waitFor(() =>
      expect(within(sheet).getByRole("button", { name: /^Apply/ })).toBeEnabled(),
    );
    fireEvent.click(within(sheet).getByRole("button", { name: /^Apply/ }));
    await within(sheet).findByRole("alert");
    // A later successful apply of the tab (from the same sheet) no longer brings them along.
    reportTabCommit(tabId, "committed");
    await new Promise((r) => setTimeout(r, 20));
    expect(apply).not.toHaveBeenCalled();
    expect(pendingObjectCount(c)).toBe(1);
  });

  it("the sheet's × and the control's Discard / Commit wait while they commit", async () => {
    let release!: () => void;
    vi.spyOn(ipc.objects, "apply").mockImplementation(
      () => new Promise<DdlResult>((r) => (release = () => r(ok))),
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
    await waitFor(() => expect(pendingObjectCount(c)).toBe(1));
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
