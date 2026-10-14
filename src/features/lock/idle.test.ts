import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { startIdleTimer } from "./idle";

const MIN = 60_000;

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(0);
});
afterEach(() => vi.useRealTimers());

describe("startIdleTimer", () => {
  it("fires once after the idle time without activity", () => {
    const onIdle = vi.fn();
    const timer = startIdleTimer({ idleMs: 5 * MIN, onIdle });
    vi.advanceTimersByTime(5 * MIN - 1);
    expect(onIdle).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onIdle).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(20 * MIN);
    expect(onIdle).toHaveBeenCalledTimes(1);
    timer.stop();
  });

  it("restarts the wait on keyboard and mouse activity in the window", () => {
    const onIdle = vi.fn();
    const timer = startIdleTimer({ idleMs: MIN, onIdle });
    vi.advanceTimersByTime(50_000);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "a" }));
    vi.advanceTimersByTime(50_000);
    window.dispatchEvent(new MouseEvent("mousemove"));
    vi.advanceTimersByTime(59_000);
    expect(onIdle).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1_000);
    expect(onIdle).toHaveBeenCalledTimes(1);
    timer.stop();
  });

  it("activity after a missed deadline (sleep, throttled timer) locks instead of counting", () => {
    const onIdle = vi.fn();
    let now = 0;
    const timer = startIdleTimer({ idleMs: MIN, onIdle, now: () => now });
    // The clock jumps (the Mac slept) while the timer has not fired yet.
    now = 10 * MIN;
    window.dispatchEvent(new MouseEvent("mousedown"));
    expect(onIdle).toHaveBeenCalledTimes(1);
    timer.stop();
  });

  it("checks when the window becomes visible again", () => {
    const onIdle = vi.fn();
    let now = 0;
    let visibility: DocumentVisibilityState = "hidden";
    const listeners = new Set<() => void>();
    const doc = {
      get visibilityState() {
        return visibility;
      },
      addEventListener: (_: string, fn: () => void) => listeners.add(fn),
      removeEventListener: (_: string, fn: () => void) => listeners.delete(fn),
    } as unknown as Document;
    const timer = startIdleTimer({ idleMs: MIN, onIdle, now: () => now, doc });
    now = 2 * MIN;
    visibility = "visible";
    listeners.forEach((fn) => fn());
    expect(onIdle).toHaveBeenCalledTimes(1);
    timer.stop();
    expect(listeners.size).toBe(0);
  });

  it("stop() removes the listeners and the timer", () => {
    const onIdle = vi.fn();
    const remove = vi.spyOn(window, "removeEventListener");
    startIdleTimer({ idleMs: MIN, onIdle }).stop();
    expect(remove).toHaveBeenCalledWith("keydown", expect.any(Function), expect.anything());
    vi.advanceTimersByTime(10 * MIN);
    expect(onIdle).not.toHaveBeenCalled();
  });
});
