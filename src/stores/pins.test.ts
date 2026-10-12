import { renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc } from "@/lib/ipc";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
import {
  EMPTY_PINS,
  isPinned,
  loadPins,
  parsePins,
  PINS_SAVE_DELAY_MS,
  PINS_SETTING_KEY,
  pinScope,
  resetPins,
  togglePin,
  usePinned,
  usePinsStore,
} from "./pins";

const users = { schema: "public", name: "users" };
const posts = { schema: "public", name: "posts" };
const scope = pinScope("p-local-pg", "tabler_dev");

describe("pins", () => {
  beforeEach(() => {
    resetMockSettings();
    resetPins();
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("scopes pins by profile and database", () => {
    expect(scope).toBe("p-local-pg|tabler_dev");
    expect(pinScope("p", null)).toBe("p|");
  });

  it("toggles a table and saves once, debounced", async () => {
    vi.useFakeTimers();
    const set = vi.spyOn(ipc.settings, "set");
    await loadPins();
    togglePin(scope, users);
    togglePin(scope, posts);
    expect(isPinned(scope, users)).toBe(true);
    expect(usePinsStore.getState().pins[scope]).toEqual(["public.users", "public.posts"]);
    expect(set).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(PINS_SAVE_DELAY_MS);
    expect(set).toHaveBeenCalledTimes(1);
    expect(set).toHaveBeenCalledWith(PINS_SETTING_KEY, {
      [scope]: ["public.users", "public.posts"],
    });

    togglePin(scope, users);
    togglePin(scope, posts);
    expect(isPinned(scope, users)).toBe(false);
    await vi.advanceTimersByTimeAsync(PINS_SAVE_DELAY_MS);
    // An emptied scope is dropped instead of stored as [].
    expect(set).toHaveBeenLastCalledWith(PINS_SETTING_KEY, {});
    expect((await ipc.settings.get())[PINS_SETTING_KEY]).toEqual({});
  });

  it("loads stored pins once and keeps unrelated scopes when saving", async () => {
    await ipc.settings.set(PINS_SETTING_KEY, { "other|db": ["s.t"], [scope]: ["public.posts"] });
    const get = vi.spyOn(ipc.settings, "get");
    await Promise.all([loadPins(), loadPins()]);
    expect(get).toHaveBeenCalledTimes(1);
    expect(isPinned(scope, posts)).toBe(true);
    vi.useFakeTimers();
    togglePin(scope, users);
    await vi.advanceTimersByTimeAsync(PINS_SAVE_DELAY_MS);
    expect((await ipc.settings.get())[PINS_SETTING_KEY]).toEqual({
      "other|db": ["s.t"],
      [scope]: ["public.posts", "public.users"],
    });
  });

  it("ignores malformed stored data", async () => {
    expect(parsePins(null)).toEqual({});
    expect(parsePins("x")).toEqual({});
    expect(parsePins(["a"])).toEqual({});
    expect(
      parsePins({ a: ["s.t", 3, "", "s.t", null], b: "s.t", c: [], d: { x: 1 }, e: ["x.y"] }),
    ).toEqual({ a: ["s.t"], e: ["x.y"] });
    await ipc.settings.set(PINS_SETTING_KEY, { [scope]: "public.users", ok: ["public.posts"] });
    await loadPins();
    expect(usePinsStore.getState().pins).toEqual({ ok: ["public.posts"] });
    expect(usePinsStore.getState().loaded).toBe(true);
  });

  it("keeps a pin made before the stored pins arrived", async () => {
    await ipc.settings.set(PINS_SETTING_KEY, { [scope]: ["public.posts"], "other|": ["a.b"] });
    togglePin(scope, users);
    await loadPins();
    expect(usePinsStore.getState().pins).toEqual({
      [scope]: ["public.users"],
      "other|": ["a.b"],
    });
  });

  it("gives a stable set per scope", async () => {
    const { result, rerender } = renderHook(({ s }) => usePinned(s), {
      initialProps: { s: scope as string | null },
    });
    await vi.waitFor(() => expect(usePinsStore.getState().loaded).toBe(true));
    expect(result.current).toBe(EMPTY_PINS);
    togglePin(scope, users);
    rerender({ s: scope });
    const first = result.current;
    expect(first.has("public.users")).toBe(true);
    // Another scope changing leaves this one's set alone.
    togglePin("other|", posts);
    rerender({ s: scope });
    expect(result.current).toBe(first);
    rerender({ s: null });
    expect(result.current).toBe(EMPTY_PINS);
  });
});
