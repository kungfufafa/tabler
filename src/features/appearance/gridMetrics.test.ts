import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_ROW_H, ROW_H, setRowHeight } from "@/features/datagrid/types";
import {
  DEFAULT_GRID_ROW_HEIGHT,
  gridLineHeightPx,
  gridRowHeight,
  paddingPreset,
  useRowHeightChange,
} from "./gridMetrics";

afterEach(() => setRowHeight(DEFAULT_ROW_H));

describe("grid metrics", () => {
  it("gives the 22px row the grid always had for the defaults", () => {
    expect(DEFAULT_GRID_ROW_HEIGHT).toBe(DEFAULT_ROW_H);
    expect(gridLineHeightPx(12, 1.35)).toBe(16);
  });

  it("is the text line plus the padding twice", () => {
    expect(gridRowHeight(12, 1.35, 3)).toBe(22);
    expect(gridRowHeight(12, 1.35, 1)).toBe(18);
    expect(gridRowHeight(16, 1.5, 6)).toBe(36);
  });

  it("never lets the line collapse", () => {
    expect(gridLineHeightPx(9, 1)).toBe(10);
  });

  it("recognises the presets and calls other paddings custom", () => {
    expect(paddingPreset(1)).toBe("compact");
    expect(paddingPreset(3)).toBe("default");
    expect(paddingPreset(6)).toBe("comfortable");
    expect(paddingPreset(4)).toBeNull();
  });
});

describe("the live row height", () => {
  it("is a live binding that setRowHeight changes", () => {
    expect(ROW_H).toBe(22);
    expect(setRowHeight(28)).toBe(true);
    expect(ROW_H).toBe(28);
    expect(setRowHeight(28)).toBe(false);
  });

  it("is rounded and kept within 14–64", () => {
    setRowHeight(3);
    expect(ROW_H).toBe(14);
    setRowHeight(500);
    expect(ROW_H).toBe(64);
    setRowHeight(20.4);
    expect(ROW_H).toBe(20);
  });

  it("re-renders a grid and runs its re-measure when the height changes, not on mount", () => {
    const measure = vi.fn();
    const { result } = renderHook(() => useRowHeightChange(measure));
    expect(result.current).toBe(22);
    expect(measure).not.toHaveBeenCalled();
    act(() => void setRowHeight(18));
    expect(result.current).toBe(18);
    expect(measure).toHaveBeenCalledTimes(1);
    act(() => void setRowHeight(18));
    expect(measure).toHaveBeenCalledTimes(1);
  });
});
