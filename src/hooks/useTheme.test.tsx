import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useThemes } from "@/features/appearance/themesStore";
import { useUiStore } from "@/stores/ui";
import { useTheme } from "./useTheme";

const native = vi.hoisted(() => ({ setTheme: vi.fn<(t?: string | null) => Promise<void>>() }));
vi.mock("@tauri-apps/api/app", () => ({ setTheme: native.setTheme }));

describe("useTheme drives the native app appearance too", () => {
  beforeEach(() => {
    native.setTheme.mockReset();
    native.setTheme.mockResolvedValue(undefined);
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    useUiStore.setState({ themePref: "system" });
    useThemes.setState({ themes: [] });
  });
  afterEach(() => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  it("follows the system for Auto, then forces dark, light and back", async () => {
    renderHook(() => useTheme());
    await vi.waitFor(() => expect(native.setTheme).toHaveBeenLastCalledWith(null));

    act(() => useUiStore.setState({ themePref: "dark" }));
    await vi.waitFor(() => expect(native.setTheme).toHaveBeenLastCalledWith("dark"));
    expect(document.documentElement.classList.contains("dark")).toBe(true);

    act(() => useUiStore.setState({ themePref: "light" }));
    await vi.waitFor(() => expect(native.setTheme).toHaveBeenLastCalledWith("light"));

    act(() => useUiStore.setState({ themePref: "system" }));
    await vi.waitFor(() => expect(native.setTheme).toHaveBeenLastCalledWith(null));
  });

  it("asks for a custom theme's own base", async () => {
    useThemes.setState({
      themes: [{ id: "night", name: "Night", base: "dark", colors: {} }] as never,
    });
    act(() => useUiStore.setState({ themePref: "custom:night" }));
    renderHook(() => useTheme());
    await vi.waitFor(() => expect(native.setTheme).toHaveBeenLastCalledWith("dark"));
  });

  it("keeps rendering when the native call fails", async () => {
    native.setTheme.mockRejectedValue(new Error("nope"));
    const { result } = renderHook(() => useTheme());
    act(() => useUiStore.setState({ themePref: "dark" }));
    await vi.waitFor(() => expect(native.setTheme).toHaveBeenCalledWith("dark"));
    expect(result.current).toBe("dark");
  });
});
