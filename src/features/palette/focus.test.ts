import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cancelTabSurfaceFocus, focusTabSurface } from "./focus";

function surface(): HTMLElement {
  const el = document.createElement("div");
  el.setAttribute("data-scope", "grid");
  el.tabIndex = 0;
  document.body.append(el);
  return el;
}

beforeEach(() => {
  vi.useFakeTimers();
  document.body.innerHTML = "";
});
afterEach(() => {
  cancelTabSurfaceFocus();
  vi.useRealTimers();
  document.body.innerHTML = "";
});

describe("focusTabSurface", () => {
  it("focuses the surface once it has mounted", () => {
    focusTabSurface();
    vi.advanceTimersByTime(200);
    const el = surface();
    expect(el).not.toHaveFocus();
    vi.advanceTimersByTime(100);
    expect(el).toHaveFocus();
  });

  it("gives up after a few seconds", () => {
    focusTabSurface();
    vi.advanceTimersByTime(4000);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("can be cancelled: nothing is polled afterwards", () => {
    focusTabSurface();
    expect(vi.getTimerCount()).toBe(1);
    cancelTabSurfaceFocus();
    expect(vi.getTimerCount()).toBe(0);
    const el = surface();
    vi.advanceTimersByTime(4000);
    expect(el).not.toHaveFocus();
  });

  it("a newer call replaces the poll of the older one (one timer, not two)", () => {
    focusTabSurface();
    focusTabSurface();
    focusTabSurface();
    expect(vi.getTimerCount()).toBe(1);
    const el = surface();
    vi.advanceTimersByTime(100);
    expect(el).toHaveFocus();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("leaves focus alone once it moved elsewhere", () => {
    const other = document.createElement("input");
    document.body.append(other);
    focusTabSurface();
    other.focus();
    const el = surface();
    vi.advanceTimersByTime(4000);
    expect(other).toHaveFocus();
    expect(el).not.toHaveFocus();
  });
});
