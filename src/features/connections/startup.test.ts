import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { MAIN_WINDOW, resetWindowLabelForTests } from "@/features/tabs/windowHandoff";
import { ipc } from "@/lib/ipc";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import { SESSION_KEY, SESSION_VERSION, type Session } from "@/stores/session";
import { useWindowShape } from "@/stores/windowShape";
import { openStartupConnection, resetStartupConnection } from "./startup";
import { useConnectionDialog } from "./store";
import { profileKeys } from "./useConnections";

const PG = SEED_PROFILES[0]!;

function deps(label = MAIN_WINDOW) {
  const qc = new QueryClient();
  qc.setQueryData(profileKeys.all, SEED_PROFILES);
  const openProfile = vi.fn();
  return {
    openProfile,
    run: () =>
      openStartupConnection({
        qc,
        openProfile,
        prefsLoaded: () => Promise.resolve(),
        windowLabel: () => Promise.resolve(label),
      }),
  };
}

beforeAll(async () => {
  await import("@/lib/ipc/mock");
});

beforeEach(() => {
  resetMockSettings();
  resetPrefsForTests();
  resetStartupConnection();
  useWindowShape.setState({ decided: false, opening: false });
});

afterEach(() => {
  useConnectionDialog.setState({ open: false, profile: null });
  vi.restoreAllMocks();
});

describe("At launch: the default connection (§10.1 Connection startup)", () => {
  it("shows the welcome screen by default", async () => {
    const d = deps();
    expect(await d.run()).toBe("welcome");
    expect(d.openProfile).not.toHaveBeenCalled();
  });

  it("opens the chosen connection, once", async () => {
    setPref("startupAction", "connection");
    setPref("startupProfileId", PG.id);
    const d = deps();
    expect(await d.run()).toBe("opened");
    expect(d.openProfile).toHaveBeenCalledWith(PG.id);
    // The window takes the workspace shape while it connects, not the Welcome one.
    expect(useWindowShape.getState().opening).toBe(true);
    expect(await d.run()).toBe("welcome");
    expect(d.openProfile).toHaveBeenCalledTimes(1);
  });

  it("Prompt the input form: opens the connection's form instead", async () => {
    setPref("startupAction", "connection");
    setPref("startupProfileId", PG.id);
    setPref("startupPromptForm", true);
    const d = deps();
    expect(await d.run()).toBe("form");
    expect(d.openProfile).not.toHaveBeenCalled();
    expect(useConnectionDialog.getState()).toMatchObject({ open: true, profile: { id: PG.id } });
  });

  it("opens the chosen connection even if another workspace was open at quit", async () => {
    const chosen = SEED_PROFILES[1]!;
    await ipc.settings.set(SESSION_KEY, {
      version: SESSION_VERSION,
      workspaces: [
        { profileId: PG.id, open: true, database: null, schema: null, tabs: [], activeTab: null },
      ],
      activeProfileId: PG.id,
    } satisfies Session);
    setPref("startupAction", "connection");
    setPref("startupProfileId", chosen.id);
    const d = deps();
    expect(await d.run()).toBe("opened");
    expect(d.openProfile).toHaveBeenCalledExactlyOnceWith(chosen.id);
  });

  it("does nothing for the saved profile when the launch action is to reopen workspaces", async () => {
    setPref("startupAction", "workspaces");
    setPref("startupProfileId", PG.id);
    const d = deps();
    await d.run();
    expect(d.openProfile).not.toHaveBeenCalled();
  });

  it("opens nothing when the chosen connection was deleted", async () => {
    setPref("startupAction", "connection");
    setPref("startupProfileId", "gone");
    const d = deps();
    expect(await d.run()).toBe("missing");
    expect(d.openProfile).not.toHaveBeenCalled();
  });
});

describe("At launch: only the main window runs it", () => {
  beforeEach(() => {
    setPref("startupAction", "connection");
    setPref("startupProfileId", PG.id);
  });

  it("does nothing in a window opened later (File ▸ New Window, Open in New Window)", async () => {
    const d = deps("w-2");
    expect(await d.run()).toBe("other-window");
    expect(d.openProfile).not.toHaveBeenCalled();
    expect(useConnectionDialog.getState().open).toBe(false);
  });

  it("does not pop the connection form in another window either", async () => {
    setPref("startupPromptForm", true);
    expect(await deps("w-3").run()).toBe("other-window");
    expect(useConnectionDialog.getState().open).toBe(false);
  });

  it("still opens the chosen connection once in the main window", async () => {
    const d = deps(MAIN_WINDOW);
    expect(await d.run()).toBe("opened");
    expect(d.openProfile).toHaveBeenCalledTimes(1);
  });

  it("asks the backend for this window's label by default", async () => {
    vi.spyOn(ipc.windows, "label").mockResolvedValue("w-2");
    resetWindowLabelForTests();
    const qc = new QueryClient();
    qc.setQueryData(profileKeys.all, SEED_PROFILES);
    const openProfile = vi.fn();
    const outcome = await openStartupConnection({
      qc,
      openProfile,
      prefsLoaded: () => Promise.resolve(),
    });
    expect(outcome).toBe("other-window");
    expect(openProfile).not.toHaveBeenCalled();
    resetWindowLabelForTests();
  });
});
