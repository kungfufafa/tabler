import { afterEach, describe, expect, it, vi } from "vitest";
import { setLauncherElsewhere, useUiStore } from "./ui";

afterEach(() => {
  setLauncherElsewhere(null);
  useUiStore.setState({ view: "launcher" });
});

describe("the connection list of a window", () => {
  it("shows in the window by default", () => {
    useUiStore.setState({ view: "workspace" });
    useUiStore.getState().setView("launcher");
    expect(useUiStore.getState().view).toBe("launcher");
  });

  it("leaves the window as it is when it showed elsewhere (the macOS Welcome window)", () => {
    const elsewhere = vi.fn(() => true);
    setLauncherElsewhere(elsewhere);
    useUiStore.setState({ view: "workspace" });
    useUiStore.getState().setView("launcher");
    expect(elsewhere).toHaveBeenCalledTimes(1);
    expect(useUiStore.getState().view).toBe("workspace");
    // Only the list is asked about: the workspace always shows.
    useUiStore.getState().setView("workspace");
    expect(elsewhere).toHaveBeenCalledTimes(1);
  });

  it("shows in the window when it could not show elsewhere", () => {
    setLauncherElsewhere(() => false);
    useUiStore.setState({ view: "workspace" });
    useUiStore.getState().setView("launcher");
    expect(useUiStore.getState().view).toBe("launcher");
  });
});
