import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { MAIN_WINDOW, resetWindowLabelForTests } from "@/features/tabs/windowHandoff";
import { ipc } from "@/lib/ipc";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { useConnectionsStore } from "@/stores/connections";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import type { Session } from "@/stores/session";
import { openStartupConnection, resetStartupConnection, restoreReopens } from "./startup";
import { useConnectionDialog } from "./store";
import { profileKeys } from "./useConnections";

const PG = SEED_PROFILES[0]!;

function session(open: boolean): Session {
  return {
    version: 1,
    workspaces: [
      { profileId: PG.id, open, database: null, schema: null, tabs: [], activeTab: null },
    ],
    activeProfileId: PG.id,
  } as unknown as Session;
}

function deps(s: Session, label = MAIN_WINDOW) {
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
        session: () => Promise.resolve(s),
        windowLabel: () => Promise.resolve(label),
      }),
  };
}

beforeAll(async () => {
  await import("@/lib/ipc/mock");
});

beforeEach(() => {
  resetPrefsForTests();
  resetStartupConnection();
});

afterEach(() => {
  useConnectionDialog.setState({ open: false, profile: null });
  vi.restoreAllMocks();
});

describe("At launch: the default connection (§10.1 Connection startup)", () => {
  it("shows the welcome screen by default", async () => {
    const d = deps(session(false));
    expect(await d.run()).toBe("welcome");
    expect(d.openProfile).not.toHaveBeenCalled();
  });

  it("opens the chosen connection, once", async () => {
    setPref("startupAction", "connection");
    setPref("startupProfileId", PG.id);
    const d = deps(session(false));
    expect(await d.run()).toBe("opened");
    expect(d.openProfile).toHaveBeenCalledWith(PG.id);
    expect(await d.run()).toBe("welcome");
    expect(d.openProfile).toHaveBeenCalledTimes(1);
  });

  it("Prompt the input form: opens the connection's form instead", async () => {
    setPref("startupAction", "connection");
    setPref("startupProfileId", PG.id);
    setPref("startupPromptForm", true);
    const d = deps(session(false));
    expect(await d.run()).toBe("form");
    expect(d.openProfile).not.toHaveBeenCalled();
    expect(useConnectionDialog.getState()).toMatchObject({ open: true, profile: { id: PG.id } });
  });

  it("reopened workspaces win; a deleted connection opens nothing", async () => {
    setPref("startupAction", "connection");
    setPref("startupProfileId", PG.id);
    expect(await deps(session(true)).run()).toBe("restored");
    expect(restoreReopens(session(true), SEED_PROFILES, false)).toBe(false);
    resetStartupConnection();
    setPref("reopenWorkspaces", false);
    const d = deps(session(true));
    expect(await d.run()).toBe("opened");
    resetStartupConnection();
    setPref("startupProfileId", "gone");
    expect(await deps(session(false)).run()).toBe("missing");
    expect(useConnectionsStore.getState().order).toEqual([]);
  });
});

describe("At launch: only the main window runs it", () => {
  beforeEach(() => {
    setPref("startupAction", "connection");
    setPref("startupProfileId", PG.id);
  });

  it("does nothing in a window opened later (File ▸ New Window, Open in New Window)", async () => {
    const d = deps(session(false), "w-2");
    expect(await d.run()).toBe("other-window");
    expect(d.openProfile).not.toHaveBeenCalled();
    expect(useConnectionDialog.getState().open).toBe(false);
  });

  it("does not pop the connection form in another window either", async () => {
    setPref("startupPromptForm", true);
    expect(await deps(session(false), "w-3").run()).toBe("other-window");
    expect(useConnectionDialog.getState().open).toBe(false);
  });

  it("still opens the chosen connection once in the main window", async () => {
    const d = deps(session(false), MAIN_WINDOW);
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
      session: () => Promise.resolve(session(false)),
    });
    expect(outcome).toBe("other-window");
    expect(openProfile).not.toHaveBeenCalled();
    resetWindowLabelForTests();
  });
});
