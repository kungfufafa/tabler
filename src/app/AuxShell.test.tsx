import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { LOCK_LOCAL_KEY, resetLockForTests, useLockStore } from "@/features/lock/store";
import { actions } from "@/features/palette/actions";
import { useSettingsDialog } from "@/features/settings/store";
import { ipc } from "@/lib/ipc";
import { resetMockPasscode } from "@/lib/ipc/mock/security";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
import { mockEmit, observeMock } from "@/lib/ipc/mock/transport";
import { resetMockSettingsWindow } from "@/lib/ipc/mock/windows";
import { resetPrefsForTests } from "@/stores/prefs";
import { AuxShell } from "./AuxShell";
import { Providers } from "./providers";

/** The Settings window's native window, as `@tauri-apps/api/window` hands it out. */
const native = vi.hoisted(() => ({
  /** `isTauri()` as the Settings window sees it (its close button and ⌘W act only in the app). */
  inTauri: false,
  close: vi.fn(() => Promise.resolve()),
}));
vi.mock("@/lib/ipc", async (importOriginal) => {
  const mod = await importOriginal<typeof import("@/lib/ipc")>();
  return { ...mod, isTauri: () => native.inTauri };
});
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    close: native.close,
    destroy: () => Promise.resolve(),
    show: () => Promise.resolve(),
    setFocus: () => Promise.resolve(),
    setTheme: () => Promise.resolve(),
    startDragging: () => Promise.resolve(),
    onCloseRequested: () => Promise.resolve(() => undefined),
  }),
}));

const setUrl = (search: string) => window.history.replaceState(null, "", `/${search}`);

function renderSettingsWindow(strict = false) {
  const shell = (
    <Providers aux>
      <AuxShell />
    </Providers>
  );
  return render(strict ? <StrictMode>{shell}</StrictMode> : shell);
}

describe("the Settings window (AuxShell)", () => {
  beforeAll(() => {
    Element.prototype.hasPointerCapture ??= () => false;
    Element.prototype.releasePointerCapture ??= () => {};
    Element.prototype.scrollIntoView ??= () => {};
  });
  beforeEach(() => {
    localStorage.clear();
    resetMockSettings();
    resetPrefsForTests();
    resetMockSettingsWindow();
    resetMockPasscode();
    resetLockForTests();
    useSettingsDialog.setState({ open: false, section: "general", query: "" });
    native.inTauri = false;
    native.close.mockClear();
  });
  afterEach(() => {
    setUrl("");
    resetPrefsForTests();
    resetLockForTests();
    document.documentElement.removeAttribute("style");
  });

  it("keeps the section it was opened on under StrictMode (effects run twice)", async () => {
    setUrl("?view=settings&section=security");
    renderSettingsWindow(true);
    const security = await screen.findByRole("tab", { name: /Security/ });
    await waitFor(() => expect(security).toHaveAttribute("aria-selected", "true"));
  });

  it("opens unlocked with a passcode set (opened from an unlocked window), and still locks", async () => {
    await ipc.security.setPasscode(null, "1234");
    localStorage.setItem(LOCK_LOCAL_KEY, "1");
    setUrl("?view=settings");
    resetLockForTests();
    renderSettingsWindow();
    await screen.findByRole("tab", { name: /General/ });
    // The passcode status is read: the window knows a passcode is set, and stays unlocked.
    await waitFor(() => expect(useLockStore.getState().checked).toBe(true));
    expect(useLockStore.getState()).toMatchObject({ enabled: true, status: "unlocked" });
    expect(screen.queryByRole("dialog", { name: "Tabler is locked" })).toBeNull();
    // Lock Tabler (and the system's lock) still lock it.
    act(() => mockEmit("lock:system", "screen-locked"));
    expect(await screen.findByRole("dialog", { name: "Tabler is locked" })).toBeInTheDocument();
  });

  it("shows the settings panel itself, on the section it was opened on", async () => {
    setUrl("?view=settings&section=security");
    renderSettingsWindow();
    const security = await screen.findByRole("tab", { name: /Security/ });
    await waitFor(() => expect(security).toHaveAttribute("aria-selected", "true"));
    // Not a dialog over a workspace: the window is Settings.
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("heading", { name: "Settings", level: 1 })).toBeInTheDocument();
    // The section list has the focus, so ⌘1…⌘9 and the arrows work at once.
    await waitFor(() => expect(security).toHaveFocus());
    // None of the app window's parts: no toolbar, connection list or workspace.
    expect(screen.queryByRole("button", { name: "New Connection" })).toBeNull();
    expect(document.title).toBe("Settings");
  });

  it("switches to the section another window asks for", async () => {
    setUrl("?view=settings");
    renderSettingsWindow();
    const general = await screen.findByRole("tab", { name: /General/ });
    await waitFor(() => expect(general).toHaveAttribute("aria-selected", "true"));
    act(() => mockEmit("settings:section", "shortcuts"));
    await waitFor(() =>
      expect(screen.getByRole("tab", { name: /Shortcuts/ })).toHaveAttribute(
        "aria-selected",
        "true",
      ),
    );
    // Something that is not a section is ignored.
    act(() => mockEmit("settings:section", "nope"));
    expect(useSettingsDialog.getState().section).toBe("shortcuts");
  });

  it("switches sections with Mod+digit, and Esc closes nothing", async () => {
    setUrl("?view=settings");
    native.inTauri = true;
    renderSettingsWindow();
    const general = await screen.findByRole("tab", { name: /General/ });
    await waitFor(() => expect(general).toHaveFocus());
    fireEvent.keyDown(general, { code: "Digit9", key: "9", ctrlKey: true });
    expect(useSettingsDialog.getState().section).toBe("appearance");
    fireEvent.keyDown(document.activeElement ?? general, { key: "Escape", code: "Escape" });
    fireEvent.keyDown(document.body, { key: "Escape", code: "Escape" });
    await act(() => new Promise((r) => setTimeout(r, 0)));
    expect(native.close).not.toHaveBeenCalled();
    expect(screen.getByRole("tab", { name: /Appearance/ })).toBeInTheDocument();
  });

  it("closes its window on ⌘W", async () => {
    setUrl("?view=settings");
    native.inTauri = true;
    renderSettingsWindow();
    await screen.findByRole("tab", { name: /General/ });
    act(() => void actions.run("tab.close"));
    await waitFor(() => expect(native.close).toHaveBeenCalledTimes(1));
  });

  it("answers ⌘, and ⌘W itself, and agrees to a quit at once", async () => {
    setUrl("?view=settings");
    const calls: string[] = [];
    const off = observeMock((cmd) => calls.push(cmd));
    try {
      renderSettingsWindow();
      await screen.findByRole("tab", { name: /General/ });
      expect(actions.has("settings.open")).toBe(true);
      expect(actions.has("tab.close")).toBe(true);
      // Lock Tabler and Check for Updates… reach it from the menu while it has the focus.
      expect(actions.has("app.lock")).toBe(true);
      expect(actions.has("app.check-updates")).toBe(true);
      act(() => mockEmit("app:quit-requested", null));
      await waitFor(() => expect(calls).toContain("quit_app"));
    } finally {
      off();
    }
  });
});
