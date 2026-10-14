import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RefreshMode } from "./model";
import { RefreshScheduler, type RunTrigger, type Scheduled } from "./scheduler";

const obj = (id: string, refresh: RefreshMode, extra: Partial<Scheduled> = {}): Scheduled => ({
  id,
  refresh,
  runs: true,
  targets: [],
  ...extra,
});

/** A runner whose runs finish after `ms` (fake time). */
function recorder(ms = 0) {
  const calls: Array<[string, RunTrigger]> = [];
  const runner = vi.fn((id: string, trigger: RunTrigger) => {
    calls.push([id, trigger]);
    return new Promise<void>((resolve) => setTimeout(resolve, ms));
  });
  return { calls, runner };
}

describe("RefreshScheduler", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("runs every object once when the board opens", async () => {
    const { calls, runner } = recorder();
    const s = new RefreshScheduler(runner);
    s.start([
      obj("a", { mode: "none" }),
      obj("b", { mode: "event" }),
      obj("c", { mode: "interval", seconds: 5 }),
      obj("d", { mode: "none" }, { runs: false }),
    ]);
    await vi.advanceTimersByTimeAsync(0);
    expect(calls).toEqual([
      ["a", "open"],
      ["b", "open"],
      ["c", "open"],
    ]);
    s.stop();
  });

  it("re-runs “After N seconds” N seconds after the previous run finished", async () => {
    const { calls, runner } = recorder(2_000);
    const s = new RefreshScheduler(runner);
    s.start([obj("a", { mode: "interval", seconds: 5 })]);
    expect(calls).toHaveLength(1);
    // The first run takes 2 s, then the timer waits 5 s: no run at 5 s, one at 7 s.
    await vi.advanceTimersByTimeAsync(5_000);
    expect(calls).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(2_000);
    expect(calls).toEqual([
      ["a", "open"],
      ["a", "interval"],
    ]);
    await vi.advanceTimersByTimeAsync(7_000);
    expect(calls).toHaveLength(3);
    s.stop();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(calls).toHaveLength(3);
  });

  it("never refreshes “Do not refresh” on its own, only on a manual refresh", async () => {
    const { calls, runner } = recorder();
    const s = new RefreshScheduler(runner);
    s.start([obj("a", { mode: "none" }), obj("in", { mode: "none" }, { targets: ["a"] })]);
    await vi.advanceTimersByTimeAsync(3_600_000);
    s.fire("in");
    await vi.advanceTimersByTimeAsync(0);
    expect(calls.filter(([id]) => id === "a")).toEqual([["a", "open"]]);
    s.refreshAll();
    s.refresh("a");
    await vi.advanceTimersByTimeAsync(0);
    expect(calls.filter(([id]) => id === "a")).toEqual([
      ["a", "open"],
      ["a", "manual"],
      ["a", "manual"],
    ]);
    s.stop();
  });

  it("refreshes the event targets of a source, and only those", async () => {
    const { calls, runner } = recorder();
    const s = new RefreshScheduler(runner);
    s.start(
      [
        obj("in", { mode: "none" }, { runs: false, targets: ["t1", "t2", "t3"] }),
        obj("t1", { mode: "event" }),
        obj("t2", { mode: "interval", seconds: 60 }),
        obj("t3", { mode: "event" }, { runs: false }),
        obj("other", { mode: "event" }),
      ],
      () => false,
    );
    await vi.advanceTimersByTimeAsync(0);
    expect(calls).toEqual([]);
    s.fire("in");
    await vi.advanceTimersByTimeAsync(0);
    expect(calls).toEqual([["t1", "event"]]);
    s.stop();
  });

  it("refreshes the objects wired to a button, which itself never runs", async () => {
    const { calls, runner } = recorder();
    const s = new RefreshScheduler(runner);
    s.start(
      [
        // A button is in the list (runs: false: no query of its own) and wired to a table.
        obj("go", { mode: "none" }, { runs: false, targets: ["t", "other"] }),
        obj("t", { mode: "event" }),
        obj("other", { mode: "none" }),
      ],
      () => false,
    );
    await vi.advanceTimersByTimeAsync(0);
    expect(calls).toEqual([]);
    s.fire("go");
    await vi.advanceTimersByTimeAsync(0);
    expect(calls).toEqual([["t", "event"]]);
    s.fire("go");
    await vi.advanceTimersByTimeAsync(0);
    expect(calls).toEqual([
      ["t", "event"],
      ["t", "event"],
    ]);
    // Not on open, not on a timer, not on Refresh all: the button has no query.
    s.refreshAll();
    s.refresh("go");
    await vi.advanceTimersByTimeAsync(3_600_000);
    expect(calls.filter(([id]) => id === "go")).toEqual([]);
    s.stop();
  });

  it("keeps a button in the list through edits", async () => {
    const { calls, runner } = recorder();
    const s = new RefreshScheduler(runner);
    const button = obj("go", { mode: "none" }, { runs: false, targets: ["t"] });
    s.start([button, obj("t", { mode: "event" })], () => false);
    s.update([button, obj("t", { mode: "event" }), obj("x", { mode: "interval", seconds: 5 })]);
    s.fire("go");
    await vi.advanceTimersByTimeAsync(0);
    expect(calls).toEqual([["t", "event"]]);
    s.stop();
  });

  it("marks what a source fires on its own as an open run", async () => {
    const { calls, runner } = recorder();
    const s = new RefreshScheduler(runner);
    s.start(
      [obj("dd", { mode: "none" }, { targets: ["t"] }), obj("t", { mode: "event" })],
      () => false,
    );
    s.fire("dd", "open");
    s.fire("dd");
    await vi.advanceTimersByTimeAsync(0);
    // The second one waits for the first (one run at a time), then runs once more.
    expect(calls[0]).toEqual(["t", "open"]);
    await vi.advanceTimersByTimeAsync(0);
    expect(calls).toEqual([
      ["t", "open"],
      ["t", "event"],
    ]);
    s.stop();
  });

  it("runs once more after a run when an event arrived meanwhile (never overlapping)", async () => {
    const { calls, runner } = recorder(1_000);
    const s = new RefreshScheduler(runner);
    s.start([
      obj("in", { mode: "none" }, { runs: false, targets: ["t"] }),
      obj("t", { mode: "event" }),
    ]);
    s.fire("in");
    s.fire("in");
    s.fire("in");
    expect(runner).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(calls).toEqual([
      ["t", "open"],
      ["t", "event"],
    ]);
    await vi.advanceTimersByTimeAsync(5_000);
    expect(calls).toHaveLength(2);
    s.stop();
  });

  it("follows edits: a new interval re-arms, another mode clears the timer", async () => {
    const { calls, runner } = recorder();
    const s = new RefreshScheduler(runner);
    s.start([obj("a", { mode: "interval", seconds: 10 })], () => false);
    s.update([obj("a", { mode: "interval", seconds: 2 })]);
    await vi.advanceTimersByTimeAsync(2_000);
    expect(calls).toEqual([["a", "interval"]]);
    s.update([obj("a", { mode: "none" })]);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(calls).toHaveLength(1);
    s.stop();
  });

  it("keeps going after a run fails", async () => {
    const runner = vi.fn(() => Promise.reject(new Error("boom")));
    const s = new RefreshScheduler(runner);
    s.start([obj("a", { mode: "interval", seconds: 1 })]);
    await vi.advanceTimersByTimeAsync(3_000);
    expect(runner.mock.calls.length).toBeGreaterThanOrEqual(3);
    s.stop();
  });
});
