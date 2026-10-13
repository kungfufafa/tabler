import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import { guardExecution } from "@/features/safety/guard";
import { ipc, type DdlResult, type ObjectOp, type TableRef } from "@/lib/ipc";
import type { Tab } from "@/stores/tabs";
import {
  armCommitChain,
  cancelCommitChain,
  commitChainArmed,
  reportTabCommit,
} from "./commitChain";
import {
  commitObjectOps,
  countOf,
  discardObjectOps,
  hostTabId,
  isCommittingObjectOps,
  opTables,
  pendingMarks,
  pendingObjectCount,
  pendingObjectOps,
  resetPendingObjectOps,
  retargetObjectOps,
  stageObjectOp,
  unstageObjectOp,
  type StagedObjectOp,
} from "./pendingOps";

vi.mock("@/features/safety/guard", () => ({ guardExecution: vi.fn(async () => true) }));

const t = (name: string): TableRef => ({ schema: "public", name });

const drop = (...names: string[]): StagedObjectOp => ({
  op: "dropTables",
  tables: names.map((n) => ({ table: t(n), kind: "table" })),
  cascade: false,
  ignoreForeignKeys: false,
});

const truncate = (...names: string[]): StagedObjectOp => ({
  op: "truncateTables",
  tables: names.map(t),
  cascade: false,
  restartIdentity: false,
  ignoreForeignKeys: false,
});

function sqlOf(op: ObjectOp): string[] {
  if (op.op === "dropTables") return op.tables.map((x) => `DROP TABLE ${x.table.name}`);
  if (op.op === "truncateTables") return op.tables.map((x) => `TRUNCATE ${x.name}`);
  return [];
}

const ok: DdlResult = { executed: [], failedIndex: null, error: null, durationMs: 1 };

describe("pending object ops", () => {
  beforeEach(() => {
    resetPendingObjectOps();
    vi.spyOn(ipc.objects, "preview").mockImplementation(async (_c, op) => ({
      statements: sqlOf(op),
      warnings: [],
      transactional: true,
    }));
    vi.mocked(guardExecution).mockClear().mockResolvedValue(true);
  });
  afterEach(() => vi.restoreAllMocks());

  it("stages, counts (one change per table), marks and discards per connection", () => {
    const a = stageObjectOp("c1", drop("users", "posts"), ["DROP TABLE users"], "db");
    stageObjectOp("c1", truncate("tags", "users"), ["TRUNCATE tags"], "db");
    stageObjectOp("c2", truncate("x"), [], null);
    expect(pendingObjectCount("c1")).toBe(4);
    expect(countOf(pendingObjectOps("c2"))).toBe(1);
    expect(pendingObjectOps(null)).toEqual([]);
    // A pending drop wins over a truncate of the same table.
    expect([...pendingMarks(pendingObjectOps("c1"))]).toEqual([
      ["public.users", "drop"],
      ["public.posts", "drop"],
      ["public.tags", "truncate"],
    ]);
    unstageObjectOp("c1", a.id);
    expect(pendingObjectOps("c1").map((o) => o.kind)).toEqual(["truncate"]);
    retargetObjectOps("c2", "c3");
    expect(pendingObjectCount("c2")).toBe(0);
    expect(pendingObjectCount("c3")).toBe(1);
    expect(discardObjectOps("c1")).toBe(2);
    expect(discardObjectOps("c1")).toBe(0);
  });

  it("commits in order after one safe-mode check of every statement", async () => {
    const calls: string[] = [];
    const apply = vi.spyOn(ipc.objects, "apply").mockImplementation(async (_c, op) => {
      calls.push(sqlOf(op).join("; "));
      return ok;
    });
    stageObjectOp("c1", truncate("tags"), [], "db");
    stageObjectOp("c1", drop("users", "posts"), [], "db");
    const outcome = await commitObjectOps("c1");
    expect(guardExecution).toHaveBeenCalledOnce();
    expect(guardExecution).toHaveBeenCalledWith("c1", {
      kind: "commit",
      statements: ["TRUNCATE tags", "DROP TABLE users", "DROP TABLE posts"],
    });
    expect(calls).toEqual(["TRUNCATE tags", "DROP TABLE users; DROP TABLE posts"]);
    expect(apply).toHaveBeenCalledTimes(2);
    expect(outcome).toMatchObject({ status: "done" });
    expect(pendingObjectOps("c1")).toEqual([]);
    expect(await commitObjectOps("c1")).toEqual({ status: "empty" });
  });

  it("sends nothing when the safe mode declines", async () => {
    const apply = vi.spyOn(ipc.objects, "apply");
    vi.mocked(guardExecution).mockResolvedValue(false);
    stageObjectOp("c1", truncate("tags"), [], "db");
    expect(await commitObjectOps("c1")).toEqual({ status: "cancelled" });
    expect(apply).not.toHaveBeenCalled();
    expect(pendingObjectCount("c1")).toBe(1);
  });

  it("stops at the first failure: that op and the rest stay pending", async () => {
    vi.spyOn(ipc.objects, "apply")
      .mockResolvedValueOnce(ok)
      .mockRejectedValueOnce({ code: "DB", message: "lock timeout" });
    stageObjectOp("c1", truncate("a"), [], "db");
    stageObjectOp("c1", truncate("b"), [], "db");
    stageObjectOp("c1", truncate("c"), [], "db");
    const outcome = await commitObjectOps("c1");
    expect(outcome).toMatchObject({ status: "failed", error: "lock timeout" });
    if (outcome.status === "failed") expect(outcome.committed).toHaveLength(1);
    const left = pendingObjectOps("c1");
    expect(left.map((o) => (o.op.op === "truncateTables" ? o.op.tables[0]!.name : ""))).toEqual([
      "b",
      "c",
    ]);
    expect(left.map((o) => o.error)).toEqual(["lock timeout", null]);
  });

  it("skips the ops taken out while the commit runs, and blocks × / discard meanwhile", async () => {
    let release!: () => void;
    const applied: string[] = [];
    vi.spyOn(ipc.objects, "apply").mockImplementation(async (_c, op) => {
      applied.push(sqlOf(op).join("; "));
      if (applied.length === 1) await new Promise<void>((r) => (release = r));
      return ok;
    });
    stageObjectOp("c1", drop("a"), [], "db");
    const second = stageObjectOp("c1", drop("b"), [], "db");
    stageObjectOp("c1", drop("c"), [], "db");
    const run = commitObjectOps("c1");
    await vi.waitFor(() => expect(applied).toEqual(["DROP TABLE a"]));
    expect(isCommittingObjectOps("c1")).toBe(true);
    expect(await commitObjectOps("c1")).toEqual({ status: "busy" });
    unstageObjectOp("c1", second.id);
    discardObjectOps("c1");
    release();
    const outcome = await run;
    expect(applied).toEqual(["DROP TABLE a"]);
    expect(outcome).toMatchObject({ status: "done", committed: [{ op: drop("a") }] });
    expect(isCommittingObjectOps("c1")).toBe(false);
  });

  describe("an op a non-transactional plan (MySQL) stopped partway", () => {
    const FK_OFF = "SET FOREIGN_KEY_CHECKS = 0";
    const FK_ON = "SET FOREIGN_KEY_CHECKS = 1";
    /** Like the backend's MySQL plans: one TRUNCATE per table, one DROP per kind of object. */
    function mysqlPlan(op: ObjectOp): string[] {
      let core: string[] = [];
      if (op.op === "truncateTables") core = op.tables.map((x) => `TRUNCATE TABLE \`${x.name}\``);
      if (op.op === "dropTables") {
        const of = (kind: string) =>
          op.tables.filter((x) => x.kind === kind).map((x) => `\`${x.table.name}\``);
        if (of("view").length > 0) core.push(`DROP VIEW ${of("view").join(", ")}`);
        if (of("table").length > 0) core.push(`DROP TABLE ${of("table").join(", ")}`);
      }
      const fk = "ignoreForeignKeys" in op && op.ignoreForeignKeys;
      return fk ? [FK_OFF, ...core, FK_ON] : core;
    }
    const failure = (executed: string[]): DdlResult => ({
      executed,
      failedIndex: executed.length,
      error: {
        code: "DB",
        message: "Lock wait timeout exceeded",
        detail: null,
        hint: null,
        sqlstate: null,
        vendorCode: null,
        position: null,
        statementIndex: executed.length,
        data: null,
      },
      durationMs: 1,
    });

    beforeEach(() => {
      vi.spyOn(ipc.objects, "preview").mockImplementation(async (_c, op) => ({
        statements: mysqlPlan(op),
        warnings: [],
        transactional: false,
      }));
    });

    it("keeps only the tables that did not run, with the error", async () => {
      const apply = vi
        .spyOn(ipc.objects, "apply")
        .mockResolvedValueOnce(failure([FK_OFF, "TRUNCATE TABLE `a`"]))
        .mockResolvedValue(ok);
      const op: StagedObjectOp = { ...truncate("a", "b", "c"), ignoreForeignKeys: true };
      const staged = stageObjectOp("c1", op, mysqlPlan(op), "db");
      const outcome = await commitObjectOps("c1");
      expect(outcome).toMatchObject({
        status: "failed",
        error: "Lock wait timeout exceeded",
        committed: [{ op: { ...op, tables: [t("a")] } }],
      });
      const rest = { ...op, tables: [t("b"), t("c")] };
      expect(pendingObjectOps("c1")).toEqual([
        {
          ...staged,
          op: rest,
          statements: [FK_OFF, "TRUNCATE TABLE `b`", "TRUNCATE TABLE `c`", FK_ON],
          error: "Lock wait timeout exceeded",
        },
      ]);
      // The retry sends what is left.
      expect(await commitObjectOps("c1")).toMatchObject({ status: "done" });
      expect(apply).toHaveBeenLastCalledWith("c1", rest);
    });

    it("splits a drop of views and tables by what ran", async () => {
      vi.spyOn(ipc.objects, "apply").mockResolvedValueOnce(failure(["DROP VIEW `v`"]));
      const op: StagedObjectOp = {
        op: "dropTables",
        tables: [
          { table: t("v"), kind: "view" },
          { table: t("a"), kind: "table" },
        ],
        cascade: false,
        ignoreForeignKeys: false,
      };
      stageObjectOp("c1", op, mysqlPlan(op), "db");
      const outcome = await commitObjectOps("c1");
      expect(outcome).toMatchObject({ status: "failed" });
      if (outcome.status !== "failed") return;
      expect(outcome.committed.map((o) => opTables(o.op))).toEqual([[t("v")]]);
      expect(pendingObjectOps("c1").map((o) => [opTables(o.op), o.statements])).toEqual([
        [[t("a")], ["DROP TABLE `a`"]],
      ]);
    });

    it("keeps the whole op when nothing of it ran", async () => {
      vi.spyOn(ipc.objects, "apply").mockResolvedValueOnce(failure([FK_OFF]));
      const op: StagedObjectOp = { ...truncate("a", "b"), ignoreForeignKeys: true };
      stageObjectOp("c1", op, mysqlPlan(op), "db");
      expect(await commitObjectOps("c1")).toMatchObject({ status: "failed", committed: [] });
      expect(pendingObjectOps("c1").map((o) => [o.op, o.error])).toEqual([
        [op, "Lock wait timeout exceeded"],
      ]);
    });
  });

  it("picks the tab that carries the count in the unsaved-changes guard", () => {
    const table = (id: string, connectionId: string, preview = false): Tab => ({
      id,
      kind: "table",
      connectionId,
      table: t(id),
      title: id,
      ...(preview ? { preview: true } : {}),
    });
    const query = (id: string, connectionId: string): Tab => ({
      id,
      kind: "query",
      connectionId,
      title: id,
    });
    const tabs = [query("q", "c1"), table("p", "c1", true), table("a", "c1"), table("b", "c1")];
    expect(hostTabId(tabs, "b", "c1")).toBe("b");
    expect(hostTabId(tabs, "q", "c1")).toBe("a");
    expect(hostTabId(tabs.slice(0, 2), "q", "c1")).toBe("p");
    expect(hostTabId(tabs.slice(0, 1), null, "c1")).toBe("q");
    expect(hostTabId(tabs, "a", "c2")).toBeNull();
  });
});

describe("armCommitChain", () => {
  let pending = 3;
  let alive = true;
  let onChange: () => void = () => undefined;
  const arm = (then: () => void, tabId = "t1") =>
    armCommitChain({
      tabId,
      pending: () => pending,
      alive: () => alive,
      subscribe: (fn) => {
        onChange = fn;
        return () => {
          onChange = () => undefined;
        };
      },
      then,
    });
  /** Lets the deferred "changes went away without a commit" check run. */
  const nextTask = () => new Promise((r) => setTimeout(r, 0));

  beforeEach(() => {
    pending = 3;
    alive = true;
    onChange = () => undefined;
  });
  afterEach(() => cancelCommitChain());

  it("fires only when the tab reports that its commit went through", async () => {
    const then = vi.fn();
    arm(then);
    reportTabCommit("t2", "committed");
    expect(then).not.toHaveBeenCalled();
    // The commit clears the changes and reports in the same task (either order).
    pending = 0;
    onChange();
    reportTabCommit("t1", "committed");
    expect(then).toHaveBeenCalledOnce();
    expect(commitChainArmed()).toBe(false);
    reportTabCommit("t1", "committed");
    await nextTask();
    expect(then).toHaveBeenCalledOnce();

    pending = 3;
    arm(then);
    reportTabCommit("t1", "committed");
    expect(then).toHaveBeenCalledTimes(2);
  });

  it("stops on a failed or cancelled commit", () => {
    const then = vi.fn();
    arm(then);
    reportTabCommit("t1", "failed");
    reportTabCommit("t1", "committed");
    arm(then);
    reportTabCommit("t1", "cancelled");
    reportTabCommit("t1", "committed");
    expect(then).not.toHaveBeenCalled();
  });

  it("stops when the changes go away without a commit (a discard by any path)", async () => {
    const then = vi.fn();
    arm(then);
    pending = 0;
    onChange();
    await nextTask();
    expect(commitChainArmed()).toBe(false);
    reportTabCommit("t1", "committed");
    expect(then).not.toHaveBeenCalled();

    // Some of them (a reverted row) or more of them.
    pending = 3;
    arm(then);
    pending = 2;
    onChange();
    expect(commitChainArmed()).toBe(false);
    pending = 3;
    arm(then);
    pending = 4;
    onChange();
    reportTabCommit("t1", "committed");
    expect(then).not.toHaveBeenCalled();
  });

  it("stops when the tab closes or loses focus, on discard / undo, or for a newer chain", async () => {
    const then = vi.fn();
    arm(then);
    alive = false;
    onChange();
    reportTabCommit("t1", "committed");

    alive = true;
    arm(then);
    await actions.run("grid.discard");
    reportTabCommit("t1", "committed");
    arm(then);
    await actions.run("grid.undo");
    reportTabCommit("t1", "committed");
    expect(then).not.toHaveBeenCalled();

    const first = vi.fn();
    arm(first);
    arm(then);
    reportTabCommit("t1", "committed");
    expect(first).not.toHaveBeenCalled();
    expect(then).toHaveBeenCalledOnce();
  });
});
