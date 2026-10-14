import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { routineItemKey, sequenceItemKey } from "@/features/sidebar/groups";
import { ipc, type ObjectOp, type RoutineRef, type RoutineSummary, type TableRef } from "@/lib/ipc";
import {
  clearOpError,
  countOf,
  describeOp,
  discardObjectOps,
  kindOf,
  opTables,
  opTargets,
  pendingMarks,
  pendingObjectCount,
  pendingObjectOps,
  resetPendingObjectOps,
  retargetObjectOps,
  settleCommittedOp,
  settleFailedOp,
  stageObjectOp,
  unstageObjectOp,
  type StagedObjectOp,
} from "./pendingOps";

type Op<K extends StagedObjectOp["op"]> = Extract<StagedObjectOp, { op: K }>;

const t = (name: string): TableRef => ({ schema: "public", name });

const fn = (name: string, args = "", kind: RoutineRef["kind"] = "function"): RoutineRef => ({
  schema: "public",
  name,
  kind,
  identityArguments: args,
});

const drop = (...names: string[]): Op<"dropTables"> => ({
  op: "dropTables",
  tables: names.map((n) => ({ table: t(n), kind: "table" })),
  cascade: false,
  ignoreForeignKeys: false,
});

const truncate = (...names: string[]): Op<"truncateTables"> => ({
  op: "truncateTables",
  tables: names.map(t),
  cascade: false,
  restartIdentity: false,
  ignoreForeignKeys: false,
});

const dropRoutines = (...routines: RoutineRef[]): Op<"dropRoutines"> => ({
  op: "dropRoutines",
  routines,
  cascade: false,
});

const dropSequences = (...names: string[]): Op<"dropSequences"> => ({
  op: "dropSequences",
  sequences: names.map(t),
  cascade: false,
});

function sqlOf(op: ObjectOp): string[] {
  if (op.op === "dropTables") return op.tables.map((x) => `DROP TABLE ${x.table.name}`);
  if (op.op === "truncateTables") return op.tables.map((x) => `TRUNCATE ${x.name}`);
  if (op.op === "dropRoutines")
    return op.routines.map((r) => `DROP FUNCTION ${r.name}(${r.identityArguments})`);
  if (op.op === "dropSequences") return op.sequences.map((x) => `DROP SEQUENCE ${x.name}`);
  return [];
}

describe("pending object ops", () => {
  beforeEach(() => {
    resetPendingObjectOps();
    vi.spyOn(ipc.objects, "preview").mockImplementation(async (_c, op) => ({
      statements: sqlOf(op),
      warnings: [],
      transactional: true,
    }));
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
      ["t:public.users", "drop"],
      ["t:public.posts", "drop"],
      ["t:public.tags", "truncate"],
    ]);
    unstageObjectOp("c1", a.id);
    expect(pendingObjectOps("c1").map((o) => o.kind)).toEqual(["truncate"]);
    retargetObjectOps("c2", "c3");
    expect(pendingObjectCount("c2")).toBe(0);
    expect(pendingObjectCount("c3")).toBe(1);
    expect(discardObjectOps("c1")).toBe(2);
    expect(discardObjectOps("c1")).toBe(0);
  });

  it("stages routine and sequence drops: one change per object, marked by sidebar item key", () => {
    const slugify = fn("slugify", "value text");
    const archive = fn("archive_posts", "", "procedure");
    stageObjectOp("c1", dropRoutines(slugify, archive), [], "db");
    stageObjectOp("c1", dropSequences("s1", "s2"), [], "db");
    stageObjectOp("c1", truncate("users"), [], "db");
    expect(pendingObjectCount("c1")).toBe(5);
    expect(pendingObjectOps("c1").map((o) => o.kind)).toEqual(["drop", "drop", "truncate"]);
    expect(kindOf(dropSequences("s"))).toBe("drop");
    const marks = pendingMarks(pendingObjectOps("c1"));
    expect([...marks]).toEqual([
      ["f:public.slugify(value text)", "drop"],
      ["f:public.archive_posts()", "drop"],
      ["s:public.s1", "drop"],
      ["s:public.s2", "drop"],
      ["t:public.users", "truncate"],
    ]);
    // The keys of the sidebar's rows.
    const row: RoutineSummary = {
      ...slugify,
      arguments: "value text",
      returnType: "text",
      language: "sql",
      comment: null,
    };
    expect(marks.get(routineItemKey(row))).toBe("drop");
    expect(marks.get(sequenceItemKey(t("s2")))).toBe("drop");
    expect(opTables(dropRoutines(slugify))).toEqual([]);
    expect(opTargets(dropRoutines(slugify, archive)).map((x) => x.name)).toEqual([
      "slugify(value text)",
      "archive_posts",
    ]);
  });

  it("describes every kind of op for the sheet", () => {
    const text = (op: StagedObjectOp) => {
      const { verb, what } = describeOp(op);
      return `${verb} ${what}`;
    };
    expect(text(drop("users", "posts"))).toBe("Delete users, posts");
    expect(text(truncate("tags"))).toBe("Truncate tags");
    expect(text(dropRoutines(fn("x", "integer")))).toBe("Drop function x(integer)");
    expect(text(dropRoutines(fn("p", "", "procedure")))).toBe("Drop procedure p");
    expect(text(dropRoutines(fn("a"), fn("b", "text")))).toBe("Drop functions a, b(text)");
    expect(text(dropRoutines(fn("a"), fn("p", "", "procedure")))).toBe("Drop routines a, p");
    expect(text(dropSequences("s"))).toBe("Drop sequence s");
    expect(text(dropSequences("s1", "s2"))).toBe("Drop sequences s1, s2");
  });

  it("settles a committed op: it leaves the pending list, the others stay", () => {
    const first = stageObjectOp("c1", truncate("tags"), [], "db");
    stageObjectOp("c1", dropRoutines(fn("slugify", "value text")), [], "db");
    settleCommittedOp("c1", first);
    expect(pendingObjectOps("c1").map((o) => o.op.op)).toEqual(["dropRoutines"]);
  });

  it("keeps a failed op whole, marked with the error, when nothing of it ran", async () => {
    const staged = stageObjectOp("c1", truncate("a", "b"), [], "db");
    stageObjectOp("c1", dropSequences("s1"), [], "db");
    expect(await settleFailedOp("c1", staged, [], "lock timeout")).toBeNull();
    expect(pendingObjectOps("c1").map((o) => [o.op.op, o.error])).toEqual([
      ["truncateTables", "lock timeout"],
      ["dropSequences", null],
    ]);
    // The next commit starts clean.
    clearOpError("c1", staged.id);
    expect(pendingObjectOps("c1").map((o) => o.error)).toEqual([null, null]);
  });

  describe("an op a non-transactional plan (MySQL) stopped partway", () => {
    const FK_OFF = "SET FOREIGN_KEY_CHECKS = 0";
    const FK_ON = "SET FOREIGN_KEY_CHECKS = 1";
    /**
     * Like the backend's MySQL plans: one TRUNCATE per table, one DROP per kind of table-like
     * object, one DROP per routine (and, for the split logic, per sequence).
     */
    function mysqlPlan(op: ObjectOp): string[] {
      let core: string[] = [];
      if (op.op === "truncateTables") core = op.tables.map((x) => `TRUNCATE TABLE \`${x.name}\``);
      if (op.op === "dropRoutines")
        core = op.routines.map(
          (r) => `DROP ${r.kind === "procedure" ? "PROCEDURE" : "FUNCTION"} \`${r.name}\``,
        );
      if (op.op === "dropSequences") core = op.sequences.map((x) => `DROP SEQUENCE \`${x.name}\``);
      if (op.op === "dropTables") {
        const of = (kind: string) =>
          op.tables.filter((x) => x.kind === kind).map((x) => `\`${x.table.name}\``);
        if (of("view").length > 0) core.push(`DROP VIEW ${of("view").join(", ")}`);
        if (of("table").length > 0) core.push(`DROP TABLE ${of("table").join(", ")}`);
      }
      const fk = "ignoreForeignKeys" in op && op.ignoreForeignKeys;
      return fk ? [FK_OFF, ...core, FK_ON] : core;
    }
    const ERROR = "Lock wait timeout exceeded";

    beforeEach(() => {
      vi.spyOn(ipc.objects, "preview").mockImplementation(async (_c, op) => ({
        statements: mysqlPlan(op),
        warnings: [],
        transactional: false,
      }));
    });

    it("keeps only the tables that did not run, with the error", async () => {
      const op: Op<"truncateTables"> = { ...truncate("a", "b", "c"), ignoreForeignKeys: true };
      const staged = stageObjectOp("c1", op, mysqlPlan(op), "db");
      const done = await settleFailedOp("c1", staged, [FK_OFF, "TRUNCATE TABLE `a`"], ERROR);
      expect(done).toMatchObject({ op: { ...op, tables: [t("a")] } });
      const rest = { ...op, tables: [t("b"), t("c")] };
      expect(pendingObjectOps("c1")).toEqual([
        {
          ...staged,
          op: rest,
          statements: [FK_OFF, "TRUNCATE TABLE `b`", "TRUNCATE TABLE `c`", FK_ON],
          error: ERROR,
        },
      ]);
    });

    it("splits a drop of views and tables by what ran", async () => {
      const op: StagedObjectOp = {
        op: "dropTables",
        tables: [
          { table: t("v"), kind: "view" },
          { table: t("a"), kind: "table" },
        ],
        cascade: false,
        ignoreForeignKeys: false,
      };
      const staged = stageObjectOp("c1", op, mysqlPlan(op), "db");
      const done = await settleFailedOp("c1", staged, ["DROP VIEW `v`"], ERROR);
      expect(done && opTables(done.op)).toEqual([t("v")]);
      expect(pendingObjectOps("c1").map((o) => [opTables(o.op), o.statements])).toEqual([
        [[t("a")], ["DROP TABLE `a`"]],
      ]);
    });

    it("splits a drop of routines by what ran", async () => {
      const op = dropRoutines(fn("a"), fn("b"), fn("p", "", "procedure"));
      const staged = stageObjectOp("c1", op, mysqlPlan(op), "db");
      const done = await settleFailedOp("c1", staged, ["DROP FUNCTION `a`"], ERROR);
      expect(done).toMatchObject({
        op: { ...op, routines: [fn("a")] },
        statements: ["DROP FUNCTION `a`"],
      });
      const rest = { ...op, routines: [fn("b"), fn("p", "", "procedure")] };
      expect(pendingObjectOps("c1")).toEqual([
        {
          ...staged,
          op: rest,
          statements: ["DROP FUNCTION `b`", "DROP PROCEDURE `p`"],
          error: ERROR,
        },
      ]);
      expect(pendingObjectCount("c1")).toBe(2);
    });

    it("splits a drop of sequences the same way", async () => {
      const op = dropSequences("s1", "s2", "s3");
      const staged = stageObjectOp("c1", op, mysqlPlan(op), "db");
      const done = await settleFailedOp(
        "c1",
        staged,
        ["DROP SEQUENCE `s1`", "DROP SEQUENCE `s2`"],
        ERROR,
      );
      expect(done?.op).toEqual({ ...op, sequences: [t("s1"), t("s2")] });
      expect(pendingObjectOps("c1").map((o) => [o.op, o.statements])).toEqual([
        [{ ...op, sequences: [t("s3")] }, ["DROP SEQUENCE `s3`"]],
      ]);
    });

    it("drops an op that ran completely although the commit stopped in the next one", async () => {
      const op = dropSequences("s1");
      const staged = stageObjectOp("c1", op, mysqlPlan(op), "db");
      const done = await settleFailedOp("c1", staged, ["DROP SEQUENCE `s1`"], ERROR);
      expect(done?.op).toEqual(op);
      expect(pendingObjectOps("c1")).toEqual([]);
    });

    it("keeps the whole op when only the foreign-key switch ran", async () => {
      const op: Op<"truncateTables"> = { ...truncate("a", "b"), ignoreForeignKeys: true };
      const staged = stageObjectOp("c1", op, mysqlPlan(op), "db");
      expect(await settleFailedOp("c1", staged, [FK_OFF], ERROR)).toBeNull();
      expect(pendingObjectOps("c1").map((o) => [o.op, o.error])).toEqual([[op, ERROR]]);
    });
  });
});
