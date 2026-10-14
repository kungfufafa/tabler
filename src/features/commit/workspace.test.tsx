import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ResultSetInfo, RunState } from "@/features/editor/runModel";
import { bufferKey } from "@/features/results/changes";
import type { ResultEditTarget } from "@/features/results/editability";
import {
  setResultEditability,
  setResultStaging,
  useResultsStore,
  pendingForTab,
} from "@/features/results/store";
import {
  pendingObjectCount,
  pendingObjectOps,
  resetPendingObjectOps,
  stageObjectOp,
} from "@/features/objects/pendingOps";
import { emptyStaging, insertRow, pendingCount, setCell } from "@/features/staging/model";
import { exprValue } from "@/features/staging/expr";
import { useStructureStore } from "@/features/structure/store";
import type {
  ColumnMeta,
  ConnectionInfo,
  GroupResult,
  ResultColumn,
  TableMeta,
  TableRef,
  Value,
  WorkspaceGroup,
  WorkspaceResult,
} from "@/lib/ipc";
import { ipc } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import { updateQueryTab, useQueryTabsStore } from "@/stores/queryTabs";
import { dropResults, resultData } from "@/stores/resultData";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore } from "@/stores/tabs";
import { resetRowContexts, setRowContext } from "./rowContext";
import { resetCommitStore, useCommitStore } from "./store";
import {
  applyWorkspace,
  collectWorkspace,
  commitWorkspace,
  discardWorkspace,
  setCommitQueryClient,
  workspacePendingCount,
} from "./workspace";

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
const USERS: TableRef = { schema: "public", name: "users" };
const POSTS: TableRef = { schema: "public", name: "posts" };

const int = (v: number): Value => ({ t: "int", v });
const text = (v: string): Value => ({ t: "text", v });

const columns = [
  { name: "id", isPk: true },
  { name: "name", isPk: false },
  { name: "seen", isPk: false },
].map((c, i) => ({
  ...c,
  ordinal: i + 1,
  dataType: c.name === "id" ? "int4" : c.name === "seen" ? "timestamp" : "text",
  kind: c.name === "id" ? "int" : c.name === "seen" ? "timestamp" : "text",
  nullable: !c.isPk,
  defaultExpr: null,
  isAutoIncrement: c.isPk,
}));

/** The page a table tab was last showing: its columns and the backend keys of its rows. */
function rowContext(tabId: string, table: TableRef, signature = "id\u0000name\u0000seen") {
  setRowContext(tabId, {
    table,
    columns: columns as never,
    signature,
    identity: "pk",
    keys: new Map([
      ["pk:1", { kind: "pk", value: { id: int(1) } }],
      ["pk:2", { kind: "pk", value: { id: int(2) } }],
    ]),
  });
}

/** A table tab with `edits` staged: `name` of row 1, and optionally the server's NOW() in `seen`. */
function tableTab(table: TableRef, { now = false }: { now?: boolean } = {}) {
  const tabId = useTabsStore.getState().openTable(CONN, table, "table");
  const original = [int(1), text("a"), { t: "timestamp", v: "2020-01-01 00:00:00" } as Value];
  let staging = setCell(emptyStaging(), "pk:1", 1, text("z"), original);
  if (now) staging = setCell(staging, "pk:1", 2, exprValue("now"), original);
  useTableTabsStore.getState().ensure(tabId);
  useTableTabsStore.getState().update(tabId, { staging, stagedOn: "id\u0000name\u0000seen" });
  rowContext(tabId, table);
  return tabId;
}

function structureOps(tabId: string) {
  useStructureStore
    .getState()
    .stage(tabId, () => [{ op: "setTableComment", table: USERS, comment: "people" }]);
}

const RESULT = "run1:0:0";
const resultColumns: ResultColumn[] = ["id", "email"].map((name) => ({
  name,
  dataType: name === "id" ? "int4" : "text",
  kind: name === "id" ? "int" : "text",
  nullable: null,
  table: "users",
  origin: { table: USERS, column: name },
}));
const resultTarget: ResultEditTarget = {
  table: USERS,
  meta: { table: USERS, primaryKey: ["id"], kind: "table" } as unknown as TableMeta,
  columns: ["id", "email"].map(
    (name) =>
      ({
        name,
        isPk: name === "id",
        nullable: name !== "id",
        isAutoIncrement: name === "id",
      }) as ColumnMeta,
  ),
  keyIndexes: [0],
};

/** A query tab whose result has one staged edit. */
function queryTabWithEdit() {
  const tabId = useTabsStore.getState().newQuery(CONN);
  const info: ResultSetInfo = {
    id: RESULT,
    statementIndex: 0,
    set: 0,
    columns: resultColumns,
    done: true,
    truncated: false,
    durationMs: 1,
  };
  const sql = "select id, email from users";
  const run: RunState = {
    id: "run1",
    sql,
    baseOffset: 0,
    startedAt: 0,
    finishedAt: 1,
    totalMs: 1,
    status: "done",
    statements: [
      {
        index: 0,
        sql,
        span: { start: 0, end: sql.length, line: 1 },
        status: "ok",
        result: null,
        error: null,
      },
    ],
    results: [info],
    error: null,
    cancelling: false,
  };
  updateQueryTab(tabId, { run, pane: RESULT });
  resultData.setColumns(RESULT, resultColumns);
  resultData.append(RESULT, [
    [int(1), text("ana@x")],
    [int(2), text("bo@x")],
  ]);
  setResultEditability(RESULT, { kind: "editable", target: resultTarget });
  setResultStaging(
    tabId,
    RESULT,
    (s) => setCell(s, bufferKey(1), 1, text("bo@y"), resultData.get(RESULT)!.rows[1]),
    resultTarget,
  );
  return tabId;
}

const truncate = (name: string) => ({
  op: "truncateTables" as const,
  tables: [{ schema: "public", name }],
  cascade: false,
  restartIdentity: false,
  ignoreForeignKeys: false,
});

const group = (over: Partial<GroupResult> = {}): GroupResult => ({
  status: "applied",
  sql: [],
  executed: [],
  failedIndex: null,
  rows: [],
  routine: null,
  error: null,
  ...over,
});

const error = (message: string) => ({
  code: "DB_ERROR",
  message,
  detail: null,
  hint: null,
  sqlstate: null,
  vendorCode: null,
  position: null,
  statementIndex: 0,
  data: null,
});

/** Previews every group as one statement named after it; applies them all, or as `results` says. */
function stub(results?: (groups: WorkspaceGroup[]) => WorkspaceResult) {
  const sql = (g: WorkspaceGroup) => `${g.kind} statement`;
  const preview = vi.spyOn(ipc.workspace, "preview").mockImplementation(async (_c, groups) => ({
    groups: groups.map((g) => ({
      statements: [sql(g)],
      warnings: [],
      transactional: true,
      error: null,
    })),
    atomic: true,
  }));
  const apply = vi.spyOn(ipc.workspace, "apply").mockImplementation(async (_c, groups) =>
    results
      ? results(groups)
      : {
          success: true,
          atomic: true,
          groups: groups.map((g) => group({ sql: [sql(g)], executed: [sql(g)] })),
          failedGroup: null,
          error: null,
          durationMs: 5,
        },
  );
  return { preview, apply };
}

beforeEach(() => {
  resetPrefsForTests({ ...DEFAULT_PREFS, confirmCommit: false });
  resetPendingObjectOps();
  resetCommitStore();
  resetRowContexts();
  guard.allow = true;
  guard.calls = [];
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
  useResultsStore.setState({ edits: {}, editability: {}, pins: {} });
  useQueryTabsStore.setState({ slices: {} });
  dropResults(() => true);
  setCommitQueryClient(new QueryClient());
});

afterEach(() => {
  vi.restoreAllMocks();
  Object.values(toast).forEach((fn) => fn.mockReset());
});

describe("collecting the workspace's pending changes", () => {
  it("gathers the staged rows of every table tab, its structure, result edits and sidebar ops", () => {
    const users = tableTab(USERS, { now: true });
    const posts = tableTab(POSTS);
    structureOps(users);
    const query = queryTabWithEdit();
    stageObjectOp(CONN, truncate("tags"), ['TRUNCATE TABLE "public"."tags"'], null);

    const { sources, problems } = collectWorkspace(CONN);
    expect(problems).toEqual([]);
    // TablePlus's order: data edits (tabs, then results), structure, then the sidebar's operations.
    expect(sources.map((s) => s.kind)).toEqual(["rows", "rows", "results", "ddl", "object"]);
    expect(sources.map((s) => s.count)).toEqual([1, 1, 1, 1, 1]);
    expect(workspacePendingCount(CONN)).toBe(5);

    const [usersRows, postsRows, results, ddl, object] = sources;
    if (usersRows?.kind !== "rows") throw new Error("rows expected");
    expect(usersRows.tabId).toBe(users);
    // The server's NOW() goes as a whitelisted expression, never as a value.
    expect(usersRows.group).toMatchObject({
      kind: "rows",
      request: {
        table: USERS,
        returning: true,
        changes: [
          {
            op: "update",
            key: { kind: "pk", value: { id: int(1) } },
            changes: { name: text("z") },
            exprs: { seen: "now" },
          },
        ],
      },
    });
    expect(postsRows).toMatchObject({ kind: "rows", tabId: posts });
    expect(results).toMatchObject({ kind: "results", tabId: query });
    expect(ddl?.group).toEqual({
      kind: "ddl",
      table: USERS,
      ops: [{ op: "setTableComment", table: USERS, comment: "people" }],
    });
    expect(object?.group).toEqual({ kind: "object", op: truncate("tags") });
  });

  it("leaves other connections' tabs and changes out", () => {
    tableTab(USERS);
    useTabsStore.getState().openTable("c2", POSTS, "table");
    stageObjectOp("c2", truncate("x"), [], null);
    expect(collectWorkspace(CONN).sources).toHaveLength(1);
    expect(workspacePendingCount("c2")).toBe(1);
    expect(workspacePendingCount(null)).toBe(0);
  });

  it("refuses to commit anything while a tab's edits cannot be built", () => {
    const tabId = tableTab(USERS);
    // Its columns changed since the edit was staged.
    useTableTabsStore.getState().update(tabId, { stagedOn: "id\u0000gone" });
    const { sources, problems } = collectWorkspace(CONN);
    expect(sources).toEqual([]);
    expect(problems).toEqual([
      expect.objectContaining({
        message: "The table's columns changed since these edits were made",
      }),
    ]);
    // A tab that never showed its page since the edits were staged has no key context either.
    resetRowContexts();
    useTableTabsStore.getState().update(tabId, { stagedOn: null });
    expect(collectWorkspace(CONN).problems[0]?.message).toMatch(/Show the tab once more/);
  });
});

describe("committing the workspace", () => {
  it("runs every tab's changes as ONE commit after ONE safe-mode check", async () => {
    const users = tableTab(USERS);
    const posts = tableTab(POSTS);
    structureOps(users);
    stageObjectOp(CONN, truncate("tags"), [], null);
    const { apply, preview } = stub();
    const generations = () => useCommitStore.getState().generation;
    const before = useTableTabsStore.getState().slices[posts]!.refreshToken;

    // ⌘S commits at once, staged structure included: no second action, no sheet.
    expect(await commitWorkspace(CONN)).toBe(true);

    expect(preview).toHaveBeenCalledOnce();
    expect(guard.calls).toEqual([
      {
        connectionId: CONN,
        request: {
          kind: "commit",
          statements: ["rows statement", "rows statement", "ddl statement", "object statement"],
        },
      },
    ]);
    expect(apply).toHaveBeenCalledOnce();
    expect(apply.mock.calls[0]![1].map((g) => g.kind)).toEqual(["rows", "rows", "ddl", "object"]);

    // Everything involved cleared its staging and refreshes.
    for (const id of [users, posts])
      expect(pendingCount(useTableTabsStore.getState().slices[id]!.staging)).toBe(0);
    // The grids deselect and the structure view resets: users had rows AND structure.
    expect(generations()[users]).toBe(2);
    expect(generations()[posts]).toBe(1);
    expect(useTableTabsStore.getState().slices[posts]!.refreshToken).toBe(before + 1);
    expect(useStructureStore.getState().byTab[users]).toBeUndefined();
    expect(pendingObjectCount(CONN)).toBe(0);
    expect(useCommitStore.getState().sheet).toBeNull();
    expect(toast.success).toHaveBeenCalledWith(
      expect.stringMatching(/^Committed 4 changes/),
      expect.objectContaining({ description: "Truncated tags" }),
    );
  });

  it("applies right away, with or without staged structure, with 'Confirm before commit' off", async () => {
    tableTab(USERS);
    const { apply } = stub();
    expect(await commitWorkspace(CONN)).toBe(true);
    expect(apply).toHaveBeenCalledOnce();
    expect(useCommitStore.getState().sheet).toBeNull();

    // ⌘S after a column was added commits it too (⇧⌘P is the way to see the SQL first).
    const users = useTabsStore.getState().openTable(CONN, USERS, "structure");
    structureOps(users);
    expect(await commitWorkspace(CONN)).toBe(true);
    expect(apply).toHaveBeenCalledTimes(2);
    expect(apply.mock.calls[1]![1].map((g) => g.kind)).toEqual(["ddl"]);
    expect(useCommitStore.getState().sheet).toBeNull();
    expect(useStructureStore.getState().byTab[users]).toBeUndefined();
  });

  it("previews staged structure first only with 'Confirm before commit' on, or for a table without a key", async () => {
    const { apply } = stub();
    const users = useTabsStore.getState().openTable(CONN, USERS, "structure");
    structureOps(users);
    resetPrefsForTests({ ...DEFAULT_PREFS, confirmCommit: true });
    expect(await commitWorkspace(CONN)).toBe(false);
    expect(useCommitStore.getState().sheet?.connectionId).toBe(CONN);
    expect(apply).not.toHaveBeenCalled();

    // Off again: a table without a usable key still opens the sheet (its UPDATE matches every
    // original column value), structure alongside or not.
    resetCommitStore();
    resetPrefsForTests({ ...DEFAULT_PREFS, confirmCommit: false });
    const keyless = tableTab(POSTS);
    setRowContext(keyless, {
      table: POSTS,
      columns: columns as never,
      signature: "id\u0000name\u0000seen",
      identity: "none",
      keys: new Map([["pk:1", { kind: "pk", value: { id: int(1) } }]]),
    });
    expect(await commitWorkspace(CONN)).toBe(false);
    expect(useCommitStore.getState().sheet?.connectionId).toBe(CONN);
    expect(apply).not.toHaveBeenCalled();
    // The sheet's own Commit still runs everything.
    expect(await applyWorkspace(CONN)).toBe(true);
    expect(apply).toHaveBeenCalledOnce();
  });

  it("previews first with 'Confirm before commit', and runs a follow-up once committed", async () => {
    resetPrefsForTests({ ...DEFAULT_PREFS, confirmCommit: true });
    tableTab(USERS);
    const { apply } = stub();
    expect(await commitWorkspace(CONN)).toBe(false);
    expect(apply).not.toHaveBeenCalled();
    expect(useCommitStore.getState().sheet).toEqual({ connectionId: CONN, onCommitted: null });

    const next = vi.fn();
    resetCommitStore();
    expect(await commitWorkspace(CONN, { onCommitted: next })).toBe(false);
    expect(next).not.toHaveBeenCalled();
    expect(await applyWorkspace(CONN)).toBe(true);
    expect(next).toHaveBeenCalledOnce();
    // Once: a later commit does not run it again.
    tableTab(POSTS);
    expect(await applyWorkspace(CONN)).toBe(true);
    expect(next).toHaveBeenCalledOnce();
  });

  it("sends nothing when the safe mode declines, and keeps everything staged", async () => {
    const id = tableTab(USERS);
    const { apply } = stub();
    guard.allow = false;
    expect(await applyWorkspace(CONN)).toBe(false);
    expect(apply).not.toHaveBeenCalled();
    expect(pendingCount(useTableTabsStore.getState().slices[id]!.staging)).toBe(1);
  });

  it("a failure reverts everything: nothing is cleared, the failing tab and statement are shown", async () => {
    const users = tableTab(USERS);
    const posts = tableTab(POSTS);
    stageObjectOp(CONN, truncate("tags"), [], null);
    const err = error("duplicate key value");
    stub((groups) => ({
      success: false,
      atomic: true,
      groups: groups.map((_g, i) =>
        i === 1
          ? group({
              status: "failed",
              sql: ["rows statement"],
              failedIndex: 0,
              rows: [{ rowId: "pk:1", op: "update", affected: 0, returned: null, error: err }],
              error: err,
            })
          : group({ status: "rolledBack", sql: ["x"] }),
      ),
      failedGroup: 1,
      error: err,
      durationMs: 1,
    }));
    expect(await applyWorkspace(CONN)).toBe(false);

    for (const id of [users, posts])
      expect(pendingCount(useTableTabsStore.getState().slices[id]!.staging)).toBe(1);
    expect(pendingObjectCount(CONN)).toBe(1);
    // The failed row carries its error; the sheet opens on the failing part.
    const failedRow = useTableTabsStore.getState().slices[posts]!.staging.changes.get("pk:1");
    expect(failedRow?.error).toBe("duplicate key value");
    const failure = useCommitStore.getState().failure;
    expect(failure).toMatchObject({
      connectionId: CONN,
      key: `rows:${posts}`,
      message: "duplicate key value",
      statementIndex: 0,
      atomic: true,
    });
    expect(useCommitStore.getState().sheet?.connectionId).toBe(CONN);
    expect(toast.error).toHaveBeenCalledWith(
      "All changes were reverted",
      expect.objectContaining({ description: expect.stringContaining("duplicate key value") }),
    );
  });

  it("without a transaction (MySQL) what ran stays committed and the failing part stays pending", async () => {
    const users = tableTab(USERS);
    const posts = tableTab(POSTS);
    structureOps(posts);
    stageObjectOp(CONN, truncate("tags"), [], null);
    const err = error("lock timeout");
    stub((groups) => ({
      success: false,
      atomic: false,
      groups: [
        group({ sql: ["rows statement"], executed: ["rows statement"] }),
        group({ sql: ["rows statement"], executed: ["rows statement"] }),
        group({
          status: "failed",
          sql: ["ddl statement"],
          executed: [],
          failedIndex: 0,
          error: err,
        }),
        group({ status: "notRun", sql: ["object statement"] }),
      ].slice(0, groups.length),
      failedGroup: 2,
      error: err,
      durationMs: 1,
    }));
    expect(await applyWorkspace(CONN)).toBe(false);
    // The data edits ran and are gone; the structure and the operation wait.
    for (const id of [users, posts])
      expect(pendingCount(useTableTabsStore.getState().slices[id]!.staging)).toBe(0);
    expect(useStructureStore.getState().byTab[posts]?.ops).toHaveLength(1);
    expect(pendingObjectCount(CONN)).toBe(1);
    expect(useCommitStore.getState().failure).toMatchObject({
      key: `ddl:${posts}`,
      atomic: false,
    });
    expect(toast.error).toHaveBeenCalledWith(
      "The commit stopped partway",
      expect.objectContaining({ description: expect.stringContaining("2 changes before it") }),
    );
  });

  it("commits the edits of a query tab's result with the rest and updates it in place", async () => {
    const query = queryTabWithEdit();
    stub((groups) => ({
      success: true,
      atomic: true,
      groups: groups.map(() =>
        group({
          sql: ["rows statement"],
          rows: [
            {
              rowId: "r1",
              op: "update",
              affected: 1,
              returned: { id: int(2), email: text("BO@Y") },
              error: null,
            },
          ],
        }),
      ),
      failedGroup: null,
      error: null,
      durationMs: 1,
    }));
    expect(pendingForTab(useResultsStore.getState(), query)).toBe(1);
    expect(await applyWorkspace(CONN)).toBe(true);
    expect(pendingForTab(useResultsStore.getState(), query)).toBe(0);
    // The stored row (as the server returned it) replaces the edited one.
    expect(resultData.get(RESULT)!.rows[1]).toEqual([int(2), text("BO@Y")]);
  });

  it("does not run twice at once, and has nothing to do without changes", async () => {
    tableTab(USERS);
    let release!: () => void;
    const { apply } = stub();
    apply.mockImplementation(
      (_c, groups) =>
        new Promise<WorkspaceResult>((resolve) => {
          release = () =>
            resolve({
              success: true,
              atomic: true,
              groups: groups.map(() => group()),
              failedGroup: null,
              error: null,
              durationMs: 1,
            });
        }),
    );
    const first = applyWorkspace(CONN);
    await vi.waitFor(() => expect(apply).toHaveBeenCalledOnce());
    expect(await applyWorkspace(CONN)).toBe(false);
    expect(await commitWorkspace(CONN)).toBe(false);
    release();
    expect(await first).toBe(true);
    expect(apply).toHaveBeenCalledOnce();
    // Nothing pending any more.
    expect(await applyWorkspace(CONN)).toBe(false);
    expect(apply).toHaveBeenCalledOnce();
  });
});

describe("refreshing the open tables after a commit", () => {
  const tokenOf = (id: string) => useTableTabsStore.getState().slices[id]!.refreshToken;

  /** Another tab on `table` (a split view's copy, "Open in New Tab"), with its own id. */
  function anotherTab(id: string, table: TableRef, connectionId = CONN) {
    useTabsStore.getState().open({ id, kind: "table", connectionId, table, title: table.name });
    useTableTabsStore.getState().ensure(id);
    return id;
  }

  it("reloads every other open tab of the committed table, and only those", async () => {
    const a = tableTab(USERS);
    // Unchanged, no staged edits: a stale page until now.
    const b = anotherTab("second-users", USERS);
    // A tab of another table, which was not written (b256), and the same table on another
    // connection.
    const other = anotherTab("posts-tab", POSTS);
    const foreign = anotherTab("foreign-users", USERS, "c2");
    stub();
    const before = [a, b, other, foreign].map(tokenOf);

    expect(await commitWorkspace(CONN)).toBe(true);
    expect([a, b, other, foreign].map(tokenOf)).toEqual([
      before[0]! + 1, // the committing tab
      before[1]! + 1, // the other tab of the same table
      before[2]!,
      before[3]!,
    ]);
  });

  it("reloads each tab once when several tabs of one table committed together", async () => {
    const a = tableTab(USERS);
    // A second tab of the same table with edits of its own: part of the same commit.
    const b = anotherTab("second-users", USERS);
    const staged = useTableTabsStore.getState().slices[a]!;
    useTableTabsStore.getState().update(b, { staging: staged.staging, stagedOn: staged.stagedOn });
    rowContext(b, USERS);
    const c = anotherTab("third-users", USERS);
    const { apply } = stub();
    const before = [a, b, c].map(tokenOf);

    expect(await commitWorkspace(CONN)).toBe(true);
    expect(apply.mock.calls[0]![1]).toHaveLength(2);
    expect([a, b, c].map(tokenOf)).toEqual([before[0]! + 1, before[1]! + 1, before[2]! + 1]);
  });

  it("a failed commit reloads nothing", async () => {
    const a = tableTab(USERS);
    const b = anotherTab("second-users", USERS);
    const err = error("duplicate key value");
    stub((groups) => ({
      success: false,
      atomic: true,
      groups: groups.map(() =>
        group({ status: "failed", sql: ["rows statement"], failedIndex: 0, error: err }),
      ),
      failedGroup: 0,
      error: err,
      durationMs: 1,
    }));
    const before = [a, b].map(tokenOf);
    expect(await commitWorkspace(CONN)).toBe(false);
    expect([a, b].map(tokenOf)).toEqual(before);
  });
});

describe("discarding the workspace", () => {
  it("drops the staged rows, results, structure and operations of every tab (⇧⌘⌫)", () => {
    const users = tableTab(USERS);
    const posts = tableTab(POSTS);
    structureOps(users);
    const query = queryTabWithEdit();
    stageObjectOp(CONN, truncate("tags"), [], null);
    useTableTabsStore.getState().update(users, {
      staging: insertRow(useTableTabsStore.getState().slices[users]!.staging, "new:1", []),
    });
    const other = useTabsStore.getState().openTable("c2", USERS, "table");
    useTableTabsStore.getState().ensure(other);
    useTableTabsStore.getState().update(other, { staging: insertRow(emptyStaging(), "new:1", []) });

    expect(discardWorkspace(CONN)).toBe(6);
    for (const id of [users, posts])
      expect(pendingCount(useTableTabsStore.getState().slices[id]!.staging)).toBe(0);
    expect(useStructureStore.getState().byTab[users]).toBeUndefined();
    expect(pendingForTab(useResultsStore.getState(), query)).toBe(0);
    expect(pendingObjectOps(CONN)).toEqual([]);
    expect(toast.message).toHaveBeenCalledWith("Discarded 6 changes");
    // Another connection's work is left alone.
    expect(pendingCount(useTableTabsStore.getState().slices[other]!.staging)).toBe(1);
    // The grids deselect, the structure views reset.
    expect(useCommitStore.getState().generation[users]).toBe(1);
    expect(discardWorkspace(CONN)).toBe(0);
  });
});
