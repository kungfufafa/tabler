import { beforeEach, describe, expect, it } from "vitest";
import {
  collapseLazyNodes,
  initLazyTree,
  LAZY_STORAGE_KEY,
  lazyKey,
  lazyTreeOf,
  loadLazyNodes,
  profileLazyLoad,
  resetLazyLoad,
  resolveLazyLoad,
  revealLazyNode,
  setLazyLoad,
  supportsLazyLoad,
  toggleLazyNode,
  useLazyStore,
} from "./lazyLoad";

describe("lazy load setting", () => {
  beforeEach(() => {
    localStorage.removeItem(LAZY_STORAGE_KEY);
    resetLazyLoad();
  });

  it("applies to PostgreSQL and MySQL only", () => {
    expect(supportsLazyLoad("postgres")).toBe(true);
    expect(supportsLazyLoad("mysql")).toBe(true);
    expect(supportsLazyLoad("sqlite")).toBe(false);
  });

  it("reads the connection form's option defensively", () => {
    expect(profileLazyLoad({ options: { lazyLoad: true } as never })).toBe(true);
    expect(profileLazyLoad({ options: { lazyLoad: false } as never })).toBe(false);
    expect(profileLazyLoad({ options: { lazyLoad: "yes" } as never })).toBeNull();
    expect(profileLazyLoad({ options: {} as never })).toBeNull();
    expect(profileLazyLoad(undefined)).toBeNull();
  });

  it("prefers the sidebar toggle, then the form's option, then the preference", () => {
    expect(resolveLazyLoad(undefined, null, false)).toBe(false);
    expect(resolveLazyLoad(undefined, null, true)).toBe(true);
    expect(resolveLazyLoad(undefined, true, false)).toBe(true);
    expect(resolveLazyLoad(undefined, false, true)).toBe(false);
    expect(resolveLazyLoad({ value: true, base: null }, null, false)).toBe(true);
    expect(resolveLazyLoad({ value: false, base: true }, true, true)).toBe(false);
    // The form changed since the toggle was set: the form wins again.
    expect(resolveLazyLoad({ value: false, base: true }, null, true)).toBe(true);
    expect(resolveLazyLoad({ value: true, base: null }, false, false)).toBe(false);
  });

  it("keeps the toggle per saved connection in localStorage", () => {
    setLazyLoad("p1", true, null);
    setLazyLoad("p2", false, true);
    expect(JSON.parse(localStorage.getItem(LAZY_STORAGE_KEY)!)).toEqual({
      p1: { value: true, base: null },
      p2: { value: false, base: true },
    });
    localStorage.setItem(
      LAZY_STORAGE_KEY,
      JSON.stringify({ p1: { value: true, base: null }, bad: { value: "x" }, p3: 1 }),
    );
    resetLazyLoad();
    expect(useLazyStore.getState().overrides).toEqual({ p1: { value: true, base: null } });
  });
});

describe("lazy tree state", () => {
  const key = lazyKey("c1", "tabler_dev");

  beforeEach(() => resetLazyLoad());

  it("starts with the current schema expanded, once per connection and database", () => {
    expect(lazyKey("c1", null)).toBe("c1|");
    initLazyTree(key, ["public"]);
    initLazyTree(key, ["other"]);
    expect(lazyTreeOf(key)).toEqual({ expanded: ["public"], loaded: ["public"] });
  });

  it("expands (loading) and collapses nodes; Load all loads without expanding", () => {
    toggleLazyNode(key, "sales");
    expect(lazyTreeOf(key)).toEqual({ expanded: ["sales"], loaded: ["sales"] });
    toggleLazyNode(key, "sales");
    expect(lazyTreeOf(key)).toEqual({ expanded: [], loaded: ["sales"] });
    const before = lazyTreeOf(key);
    toggleLazyNode(key, "sales", false);
    expect(lazyTreeOf(key)).toBe(before);
    revealLazyNode(key, "hr");
    loadLazyNodes(key, ["sales", "hr", "ops"]);
    expect(lazyTreeOf(key)).toEqual({ expanded: ["hr"], loaded: ["sales", "hr", "ops"] });
    collapseLazyNodes(key);
    expect(lazyTreeOf(key).expanded).toEqual([]);
  });
});
