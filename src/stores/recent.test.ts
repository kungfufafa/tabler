import { renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { pinScope } from "./pins";
import {
  EMPTY_RECENT,
  MAX_RECENT,
  parseRecent,
  RECENT_STORAGE_KEY,
  recordRecent,
  reloadRecent,
  useRecent,
  useRecentStore,
} from "./recent";

const scope = pinScope("p-local-pg", "tabler_dev");
const t = (name: string) => ({ schema: "public", name });
const list = (s = scope) => useRecentStore.getState().recent[s];

describe("recent tables", () => {
  beforeEach(() => {
    localStorage.clear();
    reloadRecent();
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("keeps the last opened tables per scope, most recent first, at most 5", () => {
    recordRecent(scope, t("users"));
    recordRecent(scope, t("posts"));
    recordRecent(scope, t("users"));
    expect(list()).toEqual(["public.users", "public.posts"]);
    for (const name of ["a", "b", "c", "d"]) recordRecent(scope, t(name));
    expect(list()).toHaveLength(MAX_RECENT);
    expect(list()).toEqual(["public.d", "public.c", "public.b", "public.a", "public.users"]);
    recordRecent("other|", t("users"));
    expect(list("other|")).toEqual(["public.users"]);
    expect(list()).toHaveLength(MAX_RECENT);
  });

  it("leaves the state alone when the table already is the most recent", () => {
    recordRecent(scope, t("users"));
    const before = useRecentStore.getState();
    recordRecent(scope, t("users"));
    expect(useRecentStore.getState()).toBe(before);
  });

  it("persists to localStorage and reads it back", () => {
    recordRecent(scope, t("users"));
    recordRecent(scope, t("posts"));
    expect(JSON.parse(localStorage.getItem(RECENT_STORAGE_KEY)!)).toEqual({
      [scope]: ["public.posts", "public.users"],
    });
    useRecentStore.setState({ recent: {} });
    reloadRecent();
    expect(list()).toEqual(["public.posts", "public.users"]);
  });

  it("ignores malformed stored data", () => {
    expect(parseRecent(null)).toEqual({});
    expect(parseRecent("x")).toEqual({});
    expect(parseRecent(["a"])).toEqual({});
    expect(
      parseRecent({
        a: ["s.t", 3, "", "s.t", null, "s.u", "s.v", "s.w", "s.x", "s.y"],
        b: "s.t",
        c: [],
      }),
    ).toEqual({ a: ["s.t", "s.u", "s.v", "s.w", "s.x"] });
    localStorage.setItem(RECENT_STORAGE_KEY, "{not json");
    reloadRecent();
    expect(useRecentStore.getState().recent).toEqual({});
  });

  it("keeps working when storage is unavailable", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    reloadRecent();
    expect(useRecentStore.getState().recent).toEqual({});
    recordRecent(scope, t("users"));
    expect(list()).toEqual(["public.users"]);
  });

  it("gives a stable list per scope", () => {
    const { result, rerender } = renderHook(({ s }) => useRecent(s), {
      initialProps: { s: scope as string | null },
    });
    expect(result.current).toBe(EMPTY_RECENT);
    recordRecent(scope, t("users"));
    rerender({ s: scope });
    const first = result.current;
    expect(first).toEqual(["public.users"]);
    // Another scope changing leaves this one's list alone.
    recordRecent("other|", t("posts"));
    rerender({ s: scope });
    expect(result.current).toBe(first);
    rerender({ s: null });
    expect(result.current).toBe(EMPTY_RECENT);
  });
});
