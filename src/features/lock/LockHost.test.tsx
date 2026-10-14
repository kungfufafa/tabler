import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import { ipc } from "@/lib/ipc";
import { mockEmit, mockListen } from "@/lib/ipc/mock/transport";
import { resetMockPasscode } from "@/lib/ipc/mock/security";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import { LockHost } from "./LockHost";
import { lockTabler } from "./lockTabler";
import { COOLDOWN_MS, MAX_ATTEMPTS } from "./machine";
import { RENDERER_ID } from "@/lib/ipc/events";
import {
  loadLockStatus,
  lockNow,
  LOCK_LOCAL_KEY,
  resetLockForTests,
  setPasscodeEnabled,
  unlockWithPasscode,
  useLockStore,
} from "./store";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), info: vi.fn() } }));

async function setPasscode(code: string) {
  await ipc.security.setPasscode(null, code);
}

function renderApp() {
  return render(
    <>
      <div data-testid="app">
        <button type="button">Secret table</button>
      </div>
      <LockHost />
    </>,
  );
}

beforeEach(() => {
  localStorage.clear();
  resetMockPasscode();
  resetPrefsForTests();
  resetLockForTests();
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("lock store", () => {
  it("starts unlocked without a passcode and locked from the first paint with one", async () => {
    expect(useLockStore.getState().status).toBe("unlocked");
    await loadLockStatus();
    expect(useLockStore.getState()).toMatchObject({ status: "unlocked", enabled: false });
    expect(lockNow("user")).toBe(false);

    await setPasscode("1234");
    // A new session: the localStorage flag was written by the status check.
    await loadLockStatus();
    expect(localStorage.getItem(LOCK_LOCAL_KEY)).toBe("1");
    resetLockForTests();
    expect(useLockStore.getState()).toMatchObject({ status: "locked", reason: "startup" });
  });

  it("locks at startup when the backend has a passcode the flag did not know of", async () => {
    await setPasscode("1234");
    localStorage.removeItem(LOCK_LOCAL_KEY);
    resetLockForTests();
    expect(useLockStore.getState().status).toBe("unlocked");
    await loadLockStatus();
    expect(useLockStore.getState()).toMatchObject({ status: "locked", enabled: true });
  });

  it("unlocks with the passcode; 5 wrong ones start a 30 s cooldown", async () => {
    await setPasscode("1234");
    await loadLockStatus();
    let now = 1_000;
    const clock = () => now;
    for (let i = 1; i < MAX_ATTEMPTS; i++) {
      const r = await unlockWithPasscode("0000", clock);
      expect(r).toEqual({
        ok: false,
        message: `Wrong passcode. ${MAX_ATTEMPTS - i} attempt${MAX_ATTEMPTS - i === 1 ? "" : "s"} left.`,
      });
    }
    expect(await unlockWithPasscode("0000", clock)).toEqual({
      ok: false,
      message: "Too many attempts. Try again in 30 s.",
    });
    // Even the right passcode is not checked during the cooldown.
    const verify = vi.spyOn(ipc.security, "verifyPasscode");
    now += COOLDOWN_MS - 1;
    expect((await unlockWithPasscode("1234", clock)).ok).toBe(false);
    expect(verify).not.toHaveBeenCalled();
    now += 1;
    expect(await unlockWithPasscode("1234", clock)).toEqual({ ok: true });
    expect(useLockStore.getState()).toMatchObject({
      status: "unlocked",
      attempts: { failures: 0, cooldownUntil: null },
    });
  });
});

describe("LockHost", () => {
  it("covers the app, makes it inert and refuses every action but Lock Tabler", async () => {
    await setPasscode("1234");
    await loadLockStatus();
    const ran = vi.fn();
    const off = actions.register("tab.new-query", ran);
    renderApp();
    expect(await screen.findByRole("dialog", { name: "Tabler is locked" })).toBeInTheDocument();
    // The render container stands for #root: every other child of <body> goes inert.
    const app = document.querySelector("[data-testid=app]")!.parentElement!;
    expect(app.parentElement).toBe(document.body);
    expect(app).toHaveAttribute("inert");
    expect(app).toHaveAttribute("aria-hidden", "true");
    expect(screen.queryByRole("button", { name: "Secret table" })).toBeNull();
    expect(await actions.run("tab.new-query")).toBe(false);
    expect(ran).not.toHaveBeenCalled();
    expect(await actions.run("app.lock")).toBe(true);

    const field = screen.getByLabelText("Passcode");
    fireEvent.change(field, { target: { value: "9999" } });
    fireEvent.submit(field.closest("form")!);
    expect(await screen.findByText("Wrong passcode. 4 attempts left.")).toBeInTheDocument();
    fireEvent.change(field, { target: { value: "1234" } });
    fireEvent.submit(field.closest("form")!);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(app).not.toHaveAttribute("inert");
    expect(app).not.toHaveAttribute("aria-hidden");
    expect(await actions.run("tab.new-query")).toBe(true);
    expect(ran).toHaveBeenCalledTimes(1);
    off();
  });

  it("locks on the system's sleep / screen lock event and on Lock Tabler", async () => {
    await setPasscode("1234");
    await loadLockStatus();
    useLockStore.setState({ status: "unlocked" });
    renderApp();
    await act(async () => {
      await Promise.resolve();
    });
    act(() => mockEmit("lock:system", "screen-locked"));
    expect(useLockStore.getState()).toMatchObject({ status: "locked", reason: "system" });
    useLockStore.setState({ status: "unlocked" });
    act(() => lockTabler());
    expect(useLockStore.getState()).toMatchObject({ status: "locked", reason: "user" });
  });

  it("Lock Tabler without a passcode says where to set one", async () => {
    const { toast } = await import("sonner");
    await loadLockStatus();
    lockTabler();
    expect(useLockStore.getState().status).toBe("unlocked");
    expect(toast.info).toHaveBeenCalledWith("Set a passcode to lock Tabler", expect.anything());
  });

  it("locks after the idle time, and not with Never", async () => {
    await setPasscode("1234");
    await loadLockStatus();
    useLockStore.setState({ status: "unlocked" });
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
    setPref("autoLockMinutes", 1);
    renderApp();
    act(() => vi.advanceTimersByTime(59_000));
    expect(useLockStore.getState().status).toBe("unlocked");
    act(() => vi.advanceTimersByTime(1_000));
    expect(useLockStore.getState()).toMatchObject({ status: "locked", reason: "idle" });

    useLockStore.setState({ status: "unlocked" });
    act(() => setPref("autoLockMinutes", 0));
    act(() => vi.advanceTimersByTime(3_600_000));
    expect(useLockStore.getState().status).toBe("unlocked");
  });

  it("offers Touch ID once per lock with Unlock Passcode with Touch ID", async () => {
    await setPasscode("1234");
    await loadLockStatus();
    setPref("passcodeTouchId", true);
    useLockStore.setState({ biometrics: true });
    vi.spyOn(document, "hasFocus").mockReturnValue(true);
    const auth = vi.spyOn(ipc.security, "authenticate").mockResolvedValue(true);
    renderApp();
    await waitFor(() => expect(useLockStore.getState().status).toBe("unlocked"));
    expect(auth).toHaveBeenCalledTimes(1);
    expect(auth).toHaveBeenCalledWith("unlock Tabler");
  });
});

describe("passcode changes in another window", () => {
  /** The event reaches the window once its listener is up (it loads lazily): send until then. */
  async function sendUntil(payload: { enabled: boolean }, done: () => void) {
    await waitFor(() => {
      act(() => mockEmit("lock:changed", { from: "another-window", ...payload }));
      done();
    });
  }

  it("are followed at once: set, then turned off (which unlocks)", async () => {
    renderApp();
    await act(async () => {});
    expect(useLockStore.getState().enabled).toBe(false);
    await sendUntil({ enabled: true }, () => expect(useLockStore.getState().enabled).toBe(true));
    expect(localStorage.getItem(LOCK_LOCAL_KEY)).toBe("1");
    // Lock Tabler works here now.
    act(() => {
      expect(lockNow("user")).toBe(true);
    });
    expect(useLockStore.getState().status).toBe("locked");
    await sendUntil({ enabled: false }, () =>
      expect(useLockStore.getState().status).toBe("unlocked"),
    );
    expect(localStorage.getItem(LOCK_LOCAL_KEY)).toBeNull();
  });

  it("are announced by this window's own change, and ignored when they come back", async () => {
    await setPasscode("1234");
    const heard: unknown[] = [];
    const off = mockListen("lock:changed", (p) => heard.push(p));
    renderApp();
    await waitFor(() => expect(useLockStore.getState().checked).toBe(true));
    act(() => setPasscodeEnabled(true));
    await waitFor(() => expect(heard).toEqual([{ from: RENDERER_ID, enabled: true }]));
    expect(useLockStore.getState().enabled).toBe(true);
    // Its own message coming back changes nothing (turning it off here would unlock).
    act(() => lockNow("user"));
    await act(async () => {});
    expect(useLockStore.getState().status).toBe("locked");
    off();
  });
});
