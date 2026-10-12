import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ResultSetInfo, RunState } from "@/features/editor/runModel";
import { setCell, undo } from "@/features/staging/model";
import {
  ipc,
  type ApplyResult,
  type ColumnMeta,
  type ConnectionInfo,
  type ResultColumn,
  type TableMeta,
  type TableRef,
  type Value,
} from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import { updateQueryTab, useQueryTabsStore } from "@/stores/queryTabs";
import { dropResults, resultData } from "@/stores/resultData";
import { useTabsStore } from "@/stores/tabs";
import { bufferKey } from "./changes";
import {
  applyPreviewedResultEdits,
  commitResultEdits,
  discardResultChanges,
  hasPendingResultEdits,
  isReadOnlyQuery,
  previewResultEdits,
  rerunOf,
  setResultRerun,
} from "./commit";
import type { ResultEditTarget } from "./editability";
import {
  CLOSED_PREVIEW,
  canStageAgainst,
  installResultsLifecycle,
  pendingForTab,
  pinResult,
  pinsOf,
  setResultEditability,
  setResultStaging,
  stagedTargetOf,
  stagingOf,
  unpinResult,
  useResultsStore,
} from "./store";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));
const guard = vi.hoisted(() => ({ guardExecution: vi.fn(async () => true) }));
vi.mock("@/features/safety/guard", () => guard);

const USERS: TableRef = { schema: "public", name: "users" };
const TAB = "q1";
const CONN = "c1";
const RESULT = "run1:0:0";

const int = (v: number): Value => ({ t: "int", v });
const text = (v: string): Value => ({ t: "text", v });

const columns: ResultColumn[] = ["id", "email"].map((name) => ({
  name,
  dataType: name === "id" ? "int4" : "text",
  kind: name === "id" ? "int" : "text",
  nullable: null,
  table: "users",
  origin: { table: USERS, column: name },
}));
const meta = (name: string): ColumnMeta =>
  ({
    name,
    isPk: name === "id",
    nullable: name !== "id",
    isAutoIncrement: name === "id",
  }) as ColumnMeta;
const target: ResultEditTarget = {
  table: USERS,
  meta: { table: USERS, primaryKey: ["id"], kind: "table" } as unknown as TableMeta,
  columns: [meta("id"), meta("email")],
  keyIndexes: [0],
};

const info: ResultSetInfo = {
  id: RESULT,
  statementIndex: 0,
  set: 0,
  columns,
  done: true,
  truncated: false,
  durationMs: 4,
};

function runState(sql = "select id, email from users"): RunState {
  return {
    id: "run1",
    sql,
    baseOffset: 3,
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
}

function applied(patch: Partial<ApplyResult> = {}): ApplyResult {
  return {
    success: true,
    results: [
      {
        rowId: "r1",
        op: "update",
        affected: 1,
        returned: { id: int(2), email: text("BO@Y") },
        error: null,
      },
    ],
    sql: ["UPDATE users SET email = 'bo@y' WHERE id = 2"],
    error: null,
    durationMs: 3,
    ...patch,
  };
}

beforeEach(() => {
  installResultsLifecycle();
  resetPrefsForTests();
  useResultsStore.setState({ edits: {}, editability: {}, pins: {}, preview: CLOSED_PREVIEW });
  useConnectionsStore.setState({
    byId: { [CONN]: { info: { connectionId: CONN, driver: "postgres" } as ConnectionInfo } },
  } as never);
  useTabsStore.setState({
    tabs: [{ id: TAB, kind: "query", connectionId: CONN, title: "Query 1" }],
    activeId: TAB,
  });
  useQueryTabsStore.setState({ slices: {} });
  updateQueryTab(TAB, { run: runState(), pane: RESULT });
  dropResults(() => true);
  resultData.setColumns(RESULT, columns);
  resultData.append(RESULT, [
    [int(1), text("ana@x")],
    [int(2), text("bo@x")],
  ]);
  setResultEditability(RESULT, { kind: "editable", target });
  setResultStaging(
    TAB,
    RESULT,
    (s) => setCell(s, bufferKey(1), 1, text("bo@y"), resultData.get(RESULT)!.rows[1]),
    target,
  );
  vi.spyOn(ipc.mutate, "preview").mockResolvedValue(["UPDATE users SET email = 'bo@y'"]);
});

afterEach(() => {
  vi.restoreAllMocks();
  Object.values(toast).forEach((fn) => fn.mockReset());
  guard.guardExecution.mockReset();
  guard.guardExecution.mockResolvedValue(true);
  setResultRerun(null);
});

describe("committing result edits", () => {
  it("applies them after the safe-mode check and updates the rows in place", async () => {
    const apply = vi.spyOn(ipc.mutate, "apply").mockResolvedValue(applied());
    const rerun = vi.fn();
    setResultRerun(rerun);
    expect(hasPendingResultEdits(TAB)).toBe(true);
    expect(pendingForTab(useResultsStore.getState(), TAB)).toBe(1);

    await expect(commitResultEdits(TAB)).resolves.toBe(true);
    expect(guard.guardExecution).toHaveBeenCalledWith(CONN, {
      kind: "commit",
      statements: ["UPDATE users SET email = 'bo@y'"],
    });
    expect(apply).toHaveBeenCalledWith(CONN, {
      table: USERS,
      changes: [
        {
          op: "update",
          rowId: "r1",
          key: { kind: "pk", value: { id: int(2) } },
          changes: { email: text("bo@y") },
        },
      ],
      returning: true,
    });
    // The stored row (as the server returned it) replaces the edited one.
    expect(resultData.get(RESULT)!.rows[1]).toEqual([int(2), text("BO@Y")]);
    expect(hasPendingResultEdits(TAB)).toBe(false);
    const [message, opts] = toast.success.mock.calls[0]!;
    expect(message).toBe("Applied 1 change · 3 ms");
    // The statement is a plain read: re-running it is offered.
    opts.action.onClick();
    expect(rerun).toHaveBeenCalledWith(TAB, { sql: "select id, email from users", baseOffset: 3 });
  });

  it("sends nothing when the safe-mode check is declined", async () => {
    const apply = vi.spyOn(ipc.mutate, "apply");
    guard.guardExecution.mockResolvedValue(false);
    await expect(commitResultEdits(TAB)).resolves.toBe(false);
    expect(apply).not.toHaveBeenCalled();
    expect(hasPendingResultEdits(TAB)).toBe(true);
  });

  it("previews first with Confirm before commit, then applies from the sheet", async () => {
    setPref("confirmCommit", true);
    const apply = vi.spyOn(ipc.mutate, "apply").mockResolvedValue(applied());
    await commitResultEdits(TAB);
    expect(apply).not.toHaveBeenCalled();
    expect(useResultsStore.getState().preview).toMatchObject({
      open: true,
      tabId: TAB,
      resultId: RESULT,
      statements: ["UPDATE users SET email = 'bo@y'"],
      loading: false,
    });
    await applyPreviewedResultEdits();
    expect(guard.guardExecution).toHaveBeenCalled();
    expect(apply).toHaveBeenCalledTimes(1);
    expect(useResultsStore.getState().preview.open).toBe(false);
  });

  it("keeps the edits and marks the failed row when the commit fails", async () => {
    const error = {
      code: "DB_ERROR",
      message: "duplicate key value",
      detail: null,
      hint: null,
      sqlstate: "23505",
      vendorCode: null,
      position: null,
      statementIndex: 0,
      data: null,
    };
    vi.spyOn(ipc.mutate, "apply").mockResolvedValue(
      applied({
        success: false,
        error,
        results: [{ rowId: "r1", op: "update", affected: 0, returned: null, error }],
      }),
    );
    await expect(commitResultEdits(TAB)).resolves.toBe(false);
    const staging = stagingOf(useResultsStore.getState(), RESULT);
    expect(staging.changes.get("r1")?.error).toBe("duplicate key value");
    expect(useResultsStore.getState().preview).toMatchObject({
      open: true,
      error: "duplicate key value",
      failedIndex: 0,
    });
    expect(resultData.get(RESULT)!.rows[1]).toEqual([int(2), text("bo@x")]);
  });

  it("refuses edits whose source table is no longer known", async () => {
    setResultEditability(RESULT, { kind: "readOnly", reason: "gone" });
    const apply = vi.spyOn(ipc.mutate, "apply");
    await commitResultEdits(TAB);
    expect(apply).not.toHaveBeenCalled();
    expect(toast.error).toHaveBeenCalled();
  });

  it("refuses edits staged against another target than the result has now", async () => {
    const apply = vi.spyOn(ipc.mutate, "apply");
    const preview = vi.mocked(ipc.mutate.preview);
    // The table's key changed (or its reload failed and another table took over): the staged
    // row keys name rows of the old target, so nothing is sent.
    const posts: ResultEditTarget = {
      ...target,
      table: { schema: "public", name: "posts" },
      meta: { ...target.meta, table: { schema: "public", name: "posts" } },
    };
    setResultEditability(RESULT, { kind: "editable", target: posts });
    await expect(commitResultEdits(TAB)).resolves.toBe(false);
    await previewResultEdits(TAB);
    expect(preview).not.toHaveBeenCalled();
    expect(apply).not.toHaveBeenCalled();
    expect(toast.error).toHaveBeenCalledWith(
      "The result's source table changed; discard the edits and re-run",
      expect.anything(),
    );
    expect(hasPendingResultEdits(TAB)).toBe(true);
    // Back to the target the edits were made against: they commit.
    expect(stagedTargetOf(useResultsStore.getState(), RESULT)).toBe(target);
    expect(canStageAgainst(useResultsStore.getState(), RESULT, posts)).toBe(false);
    setResultEditability(RESULT, { kind: "editable", target: { ...target } });
    apply.mockResolvedValue(applied());
    await expect(commitResultEdits(TAB)).resolves.toBe(true);
    // Committed: the next edit may record another target.
    expect(stagedTargetOf(useResultsStore.getState(), RESULT)).toBeNull();
    expect(canStageAgainst(useResultsStore.getState(), RESULT, posts)).toBe(true);
  });

  it("previews (⇧⌘P) and discards (⇧⌘⌫)", async () => {
    await previewResultEdits(TAB);
    expect(useResultsStore.getState().preview.statements).toHaveLength(1);
    discardResultChanges(TAB);
    expect(hasPendingResultEdits(TAB)).toBe(false);
    expect(toast.message).toHaveBeenCalledWith("Changes discarded");
  });
});

describe("the edit target of staged edits", () => {
  const posts: ResultEditTarget = { ...target, table: { schema: "public", name: "posts" } };
  const row0 = () => resultData.get(RESULT)!.rows[0];

  it("is recorded with the first edit and kept while edits (or their undo) remain", () => {
    expect(stagedTargetOf(useResultsStore.getState(), RESULT)).toBe(target);
    // Staging again with another target keeps the first one (callers check first).
    setResultStaging(TAB, RESULT, (s) => setCell(s, bufferKey(0), 1, text("x"), row0()), posts);
    expect(stagedTargetOf(useResultsStore.getState(), RESULT)).toBe(target);
    expect(canStageAgainst(useResultsStore.getState(), RESULT, posts)).toBe(false);
    // Everything undone: a new edit may be made against another target.
    expect(pendingForTab(useResultsStore.getState(), TAB)).toBe(2);
    setResultStaging(TAB, RESULT, (s) => undo(s));
    expect(canStageAgainst(useResultsStore.getState(), RESULT, posts)).toBe(false);
    setResultStaging(TAB, RESULT, (s) => undo(s));
    expect(hasPendingResultEdits(TAB)).toBe(false);
    expect(canStageAgainst(useResultsStore.getState(), RESULT, posts)).toBe(true);
    setResultStaging(TAB, RESULT, (s) => setCell(s, bufferKey(0), 1, text("y"), row0()), posts);
    expect(stagedTargetOf(useResultsStore.getState(), RESULT)).toBe(posts);
  });
});

describe("result lifecycle", () => {
  it("drops the edits of a result a new run replaced, saying so", () => {
    updateQueryTab(TAB, { run: { ...runState(), id: "run2", results: [] } });
    expect(hasPendingResultEdits(TAB)).toBe(false);
    expect(toast.message).toHaveBeenCalledWith(
      "1 uncommitted result change was discarded",
      expect.anything(),
    );
  });

  it("pins a result: its rows and edits survive the run being replaced", () => {
    const pin = pinResult(TAB, info, "Result 1", "select id, email from users")!;
    expect(pinsOf(useResultsStore.getState(), TAB).map((p) => p.sourceId)).toEqual([RESULT]);
    expect(resultData.get(pin)!.rows).toHaveLength(2);
    updateQueryTab(TAB, { run: { ...runState(), id: "run2", results: [] } });
    dropResults((id) => id.startsWith("run1:"));
    expect(resultData.get(pin)!.rows).toHaveLength(2);
    expect(hasPendingResultEdits(TAB)).toBe(true);
    unpinResult(TAB, pin);
    expect(resultData.get(pin)).toBeUndefined();
    expect(hasPendingResultEdits(TAB)).toBe(false);
  });

  it("forgets the pins of a closed tab", () => {
    const pin = pinResult(TAB, info, "Result 1", null)!;
    useTabsStore.setState({ tabs: [], activeId: null });
    expect(pinsOf(useResultsStore.getState(), TAB)).toEqual([]);
    expect(resultData.get(pin)).toBeUndefined();
  });
});

describe("re-running after a commit", () => {
  it("is only offered for plain reads", () => {
    for (const sql of [
      "select * from users",
      "  -- latest\n SELECT 1",
      "with x as (select 1) select * from x",
      "table users",
      "select 'delete me' from t",
    ])
      expect(isReadOnlyQuery(sql, "postgres"), sql).toBe(true);
    for (const sql of [
      "insert into t select 1 returning *",
      "update t set a = 1 returning *",
      "with d as (delete from t returning *) select * from d",
      "select * into t2 from t",
      "select * from t for update",
      "call p()",
    ])
      expect(isReadOnlyQuery(sql, "postgres"), sql).toBe(false);
  });

  it("re-runs the statement that produced the result", () => {
    const run = runState("select 1;\nselect id from users");
    run.statements = [
      { ...run.statements[0]!, index: 0, span: { start: 0, end: 8, line: 1 } },
      { ...run.statements[0]!, index: 1, span: { start: 10, end: 30, line: 2 } },
    ];
    run.results = [{ ...info, statementIndex: 1 }];
    expect(rerunOf(run, RESULT, "postgres")).toEqual({
      sql: "select id from users",
      baseOffset: 13,
    });
    expect(rerunOf(run, "nope", "postgres")).toBeNull();
  });
});
