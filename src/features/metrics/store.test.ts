import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc } from "@/lib/ipc";
import { resetMockMetrics } from "@/lib/ipc/mock/metrics";
import { newObject } from "./model";
import {
  boardsFor,
  createBoard,
  deleteBoard,
  flushSave,
  loadBoards,
  resetMetricsStoreForTests,
  SAVE_DELAY,
  setBoardProfile,
  updateObjects,
  useMetricsStore,
} from "./store";

describe("metrics store", () => {
  beforeEach(() => {
    resetMockMetrics();
    resetMetricsStoreForTests();
  });
  afterEach(() => vi.useRealTimers());

  it("loads a profile's boards with the boards of every connection", async () => {
    await createBoard("p-other", "Theirs");
    resetMetricsStoreForTests();
    await loadBoards("p1");
    const s = useMetricsStore.getState();
    expect(s.loaded.p1).toBe("ok");
    expect(boardsFor(s, "p1").map((b) => b.name)).toEqual(["Sales overview"]);
    // The demo board's objects come back normalized.
    const demo = boardsFor(s, "p1")[0]!;
    expect(demo.objects.length).toBeGreaterThan(5);
    expect(demo.objects.every((o) => o.layout.w > 0 && Array.isArray(o.targets))).toBe(true);
  });

  it("saves edits after a pause, and only once", async () => {
    vi.useFakeTimers();
    const board = await createBoard("p1", "Ops");
    const save = vi.spyOn(ipc.metrics, "saveBoard");
    const o = newObject("scoreboard", []);
    updateObjects(board.id, (list) => [...list, o]);
    updateObjects(board.id, (list) => list.map((x) => ({ ...x, sql: "SELECT 1" })));
    expect(useMetricsStore.getState().boards[board.id]!.dirty).toBe(true);
    await vi.advanceTimersByTimeAsync(SAVE_DELAY - 1);
    expect(save).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    await flushSave(board.id);
    expect(save).toHaveBeenCalledTimes(1);
    expect(save.mock.calls[0]![0]).toMatchObject({
      id: board.id,
      name: "Ops",
      objects: [{ ...o, sql: "SELECT 1" }],
    });
    expect(useMetricsStore.getState().boards[board.id]!.dirty).toBe(false);
    // What was saved reads back the same.
    resetMetricsStoreForTests();
    await loadBoards("p1");
    expect(useMetricsStore.getState().boards[board.id]!.objects).toEqual([
      { ...o, sql: "SELECT 1" },
    ]);
    save.mockRestore();
  });

  it("moves a board to every connection and deletes it", async () => {
    const board = await createBoard("p1", "Mine");
    await setBoardProfile(board.id, null);
    resetMetricsStoreForTests();
    await loadBoards("p2");
    expect(boardsFor(useMetricsStore.getState(), "p2").map((b) => b.name)).toContain("Mine");
    await deleteBoard(board.id);
    expect(useMetricsStore.getState().boards[board.id]).toBeUndefined();
    await loadBoards("p2");
    expect(boardsFor(useMetricsStore.getState(), "p2").map((b) => b.name)).not.toContain("Mine");
  });

  it("keeps the save error and unsaved edits when saving fails", async () => {
    const board = await createBoard("p1", "Ops");
    const save = vi.spyOn(ipc.metrics, "saveBoard").mockRejectedValue({
      code: "APP_DB",
      message: "disk full",
    });
    updateObjects(board.id, (list) => [...list, newObject("table", [])]);
    await flushSave(board.id);
    const draft = useMetricsStore.getState().boards[board.id]!;
    expect(draft.dirty).toBe(true);
    expect(draft.saveError).toContain("disk full");
    save.mockRestore();
    await flushSave(board.id);
    expect(useMetricsStore.getState().boards[board.id]!.saveError).toBeNull();
  });
});
