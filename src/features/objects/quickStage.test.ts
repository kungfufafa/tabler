import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { toast } from "sonner";
import { ipc } from "@/lib/ipc";
import { pendingObjectOps, resetPendingObjectOps, type StagedObjectOp } from "./pendingOps";
import { stagedSummary, stageWithoutDialog } from "./quickStage";

const t = (name: string) => ({ schema: "public", name });

describe("stagedSummary", () => {
  it("names what a shortcut staged", () => {
    expect(
      stagedSummary({
        op: "dropTables",
        tables: [{ table: t("users"), kind: "table" }],
        cascade: true,
        ignoreForeignKeys: false,
      }),
    ).toBe("drop table users CASCADE");
    expect(
      stagedSummary({
        op: "dropTables",
        tables: [
          { table: t("a"), kind: "table" },
          { table: t("b"), kind: "partitioned" },
        ],
        cascade: false,
        ignoreForeignKeys: true,
      }),
    ).toBe("drop 2 tables without foreign key checks");
    expect(
      stagedSummary({
        op: "dropTables",
        tables: [
          { table: t("v"), kind: "view" },
          { table: t("m"), kind: "materializedView" },
        ],
        cascade: false,
        ignoreForeignKeys: false,
      }),
    ).toBe("drop 2 views");
    expect(
      stagedSummary({
        op: "dropTables",
        tables: [
          { table: t("v"), kind: "view" },
          { table: t("a"), kind: "table" },
        ],
        cascade: false,
        ignoreForeignKeys: false,
      }),
    ).toBe("drop 2 items");
    expect(
      stagedSummary({
        op: "truncateTables",
        tables: [t("a"), t("b"), t("c")],
        cascade: false,
        restartIdentity: false,
        ignoreForeignKeys: false,
      }),
    ).toBe("truncate 3 tables");
    expect(
      stagedSummary({
        op: "dropRoutines",
        routines: [
          { schema: "public", name: "slugify", kind: "function", identityArguments: "value text" },
        ],
        cascade: false,
      }),
    ).toBe("drop function slugify(value text)");
    expect(
      stagedSummary({ op: "dropSequences", sequences: [t("a_seq"), t("b_seq")], cascade: false }),
    ).toBe("drop 2 sequences");
  });
});

describe("stageWithoutDialog", () => {
  const op: StagedObjectOp = {
    op: "truncateTables",
    tables: [t("tags")],
    cascade: false,
    restartIdentity: false,
    ignoreForeignKeys: false,
  };

  beforeEach(() => resetPendingObjectOps());
  afterEach(() => vi.restoreAllMocks());

  it("stages the previewed plan once, even when the key repeats", async () => {
    vi.spyOn(ipc.objects, "preview").mockResolvedValue({
      statements: ['TRUNCATE TABLE "public"."tags"'],
      warnings: ["MySQL TRUNCATE always resets AUTO_INCREMENT"],
      transactional: true,
    });
    const message = vi.spyOn(toast, "message");
    const [first, second] = await Promise.all([
      stageWithoutDialog("c1", op),
      stageWithoutDialog("c1", op),
    ]);
    expect([first, second]).toEqual([true, false]);
    expect(pendingObjectOps("c1")).toHaveLength(1);
    expect(pendingObjectOps("c1")[0]!.statements).toEqual(['TRUNCATE TABLE "public"."tags"']);
    expect(message).toHaveBeenCalledOnce();
    expect(message.mock.calls[0]![0]).toBe("Staged: truncate tags");
    expect(String((message.mock.calls[0]![1] as { description: string }).description)).toMatch(
      /AUTO_INCREMENT/,
    );
  });

  it("stages nothing when the plan fails, and says why", async () => {
    vi.spyOn(ipc.objects, "preview").mockRejectedValue(new Error("no such table"));
    const error = vi.spyOn(toast, "error");
    expect(await stageWithoutDialog("c1", op)).toBe(false);
    expect(pendingObjectOps("c1")).toEqual([]);
    expect(error).toHaveBeenCalledWith(
      "Could not truncate tags",
      expect.objectContaining({ description: expect.stringMatching(/no such table/) }),
    );
  });
});
