import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { toast } from "sonner";
import { actions } from "@/features/palette/actions";
import { dispatchMenuAction } from "@/hooks/useNativeMenu";
import { ipc } from "@/lib/ipc";
// Loaded up front: the first IPC call would otherwise wait for every mock module to load.
import "@/lib/ipc/mock";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
import {
  MOCK_APP_VERSION,
  mockUpdateInstalls,
  resetMockUpdater,
  setMockUpdate,
} from "@/lib/ipc/mock/updater";
import { buildLabel, useAppInfoStore } from "@/stores/appInfo";
import { resetPrefsForTests, setPref, usePrefsStore } from "@/stores/prefs";
import { registerWindowWork } from "@/stores/tabs";
import {
  cancelDownload,
  checkAtLaunch,
  checkForUpdates,
  closeUpdateDialog,
  downloadUpdate,
  installUpdate,
  resetUpdaterStore,
  useUpdaterStore,
} from "./store";
import { notesSince, progressLabel, progressPercent, publishedLabel } from "./labels";
import { UpdateDialog } from "./UpdateDialog";
import { LAUNCH_CHECK_DELAY_MS, UpdaterHost } from "./UpdaterHost";
import { UpdateSettingRow } from "./UpdateSettingRow";

vi.mock("sonner", () => ({
  toast: { message: vi.fn(), error: vi.fn(), success: vi.fn(), warning: vi.fn() },
}));

const NEXT = {
  version: "0.1.3",
  notes:
    "0.1.3\n\nNew\n- Updates install from inside the app\n\nFixed\n- The sidebar keeps its width",
  date: "2026-11-12T08:00:00Z",
  size: 48 * 1024 * 1024,
};

const phase = () => useUpdaterStore.getState().phase;

beforeEach(() => {
  localStorage.clear();
  resetMockSettings();
  resetMockUpdater();
  resetPrefsForTests();
  resetUpdaterStore();
  useAppInfoStore.setState({ info: null });
  vi.mocked(toast.message).mockClear();
  vi.mocked(toast.error).mockClear();
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  resetPrefsForTests();
});

describe("Check for Updates…", () => {
  it("says the build is up to date when nothing newer is published", async () => {
    await checkForUpdates();
    expect(phase()).toEqual({ kind: "upToDate", version: MOCK_APP_VERSION });
  });

  it("offers the newer build with its notes", async () => {
    setMockUpdate(NEXT);
    await checkForUpdates();
    const p = phase();
    expect(p.kind).toBe("available");
    if (p.kind !== "available") return;
    expect(p.update.version).toBe(NEXT.version);
    expect(p.update.currentVersion).toBe(MOCK_APP_VERSION);
    expect(p.update.notes).toBe(NEXT.notes);
    expect(p.update.downloaded).toBe(false);
  });

  it("shows a failed check with Try Again", async () => {
    const check = vi
      .spyOn(ipc.updater, "check")
      .mockRejectedValueOnce({ code: "INTERNAL", message: "No release information was found." });
    await checkForUpdates();
    expect(phase()).toEqual({
      kind: "error",
      message: "No release information was found.",
      update: null,
    });
    render(<UpdateDialog />);
    expect(screen.getByText("Could not check for updates")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Try Again" }));
    await waitFor(() => expect(phase().kind).toBe("upToDate"));
    expect(check).toHaveBeenCalledTimes(2);
  });

  it("downloads with progress, then installs and restarts", async () => {
    setMockUpdate(NEXT);
    await checkForUpdates();
    const seen: number[] = [];
    const unsubscribe = useUpdaterStore.subscribe((s) => {
      if (s.phase.kind === "downloading" && s.phase.progress)
        seen.push(s.phase.progress.downloaded);
    });
    await downloadUpdate();
    unsubscribe();
    expect(seen.length).toBeGreaterThan(2);
    expect(seen.at(-1)).toBe(NEXT.size);
    expect([...seen].sort((a, b) => a - b)).toEqual(seen);
    const ready = phase();
    expect(ready.kind).toBe("ready");

    await installUpdate();
    expect(mockUpdateInstalls).toEqual([NEXT.version]);
    // The app restarts from here; the dialog stays on "Installing…" until it does.
    expect(phase().kind).toBe("installing");
  });

  it("goes back to the offer when the download is cancelled", async () => {
    setMockUpdate(NEXT);
    await checkForUpdates();
    const done = downloadUpdate();
    await waitFor(() => expect(phase().kind).toBe("downloading"));
    await cancelDownload();
    await done;
    expect(phase().kind).toBe("available");
    // Nothing was downloaded, so there is nothing to install.
    await installUpdate();
    expect(mockUpdateInstalls).toEqual([]);
  });

  it("keeps the update on screen when the download fails, to try again", async () => {
    setMockUpdate({ ...NEXT, failDownload: "Could not download the update: status 404" });
    await checkForUpdates();
    await downloadUpdate();
    const failed = phase();
    expect(failed.kind).toBe("error");
    if (failed.kind !== "error") return;
    expect(failed.message).toBe("Could not download the update: status 404");
    expect(failed.update?.version).toBe(NEXT.version);

    setMockUpdate(NEXT);
    await downloadUpdate();
    expect(phase().kind).toBe("ready");
  });

  it("does not restart over unsaved work in this window", async () => {
    setMockUpdate(NEXT);
    await checkForUpdates();
    await downloadUpdate();
    const remove = registerWindowWork(() => [{ label: "query.sql" } as never]);
    try {
      await installUpdate();
    } finally {
      remove();
    }
    expect(mockUpdateInstalls).toEqual([]);
    expect(phase().kind).toBe("ready");
    expect(toast.message).toHaveBeenCalledWith(
      "Save or close the unsaved work first",
      expect.anything(),
    );
  });

  it("stays ready when the backend refuses the restart (another window's unsaved work)", async () => {
    const refusal = "Another window has unsaved work. Save or close it first, then restart.";
    setMockUpdate({ ...NEXT, failInstall: refusal });
    await checkForUpdates();
    await downloadUpdate();
    await installUpdate();
    expect(phase().kind).toBe("ready");
    expect(toast.error).toHaveBeenCalledWith("Could not install the update", {
      description: refusal,
    });
  });

  it("opens on the restart offer when the update was already downloaded", async () => {
    setMockUpdate(NEXT);
    await checkForUpdates();
    await downloadUpdate();
    closeUpdateDialog();
    expect(phase().kind).toBe("closed");
    await checkForUpdates();
    expect(phase().kind).toBe("ready");
  });

  it("cannot be closed while a download runs", async () => {
    setMockUpdate(NEXT);
    await checkForUpdates();
    const done = downloadUpdate();
    await waitFor(() => expect(phase().kind).toBe("downloading"));
    closeUpdateDialog();
    expect(phase().kind).toBe("downloading");
    await done;
    closeUpdateDialog();
    expect(phase().kind).toBe("closed");
  });
});

describe("the check at launch", () => {
  it("says nothing when the build is up to date", async () => {
    await checkAtLaunch();
    expect(toast.message).not.toHaveBeenCalled();
    expect(phase().kind).toBe("closed");
  });

  it("raises a notice whose button opens the dialog", async () => {
    setMockUpdate(NEXT);
    await checkAtLaunch();
    expect(phase().kind).toBe("closed");
    expect(toast.message).toHaveBeenCalledTimes(1);
    const [title, options] = vi.mocked(toast.message).mock.calls[0]!;
    expect(title).toBe(`Tabler ${NEXT.version} is available`);
    const action = (options as { action: { label: string; onClick: () => void } }).action;
    expect(action.label).toBe("Update…");
    act(() => action.onClick());
    expect(phase().kind).toBe("available");
  });

  it("is made once per run, whichever window asks", async () => {
    setMockUpdate(NEXT);
    await checkAtLaunch();
    await checkAtLaunch();
    expect(toast.message).toHaveBeenCalledTimes(1);
  });

  it("keeps a failure to the console", async () => {
    vi.spyOn(ipc.updater, "check").mockRejectedValueOnce({ code: "INTERNAL", message: "offline" });
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await checkAtLaunch();
    expect(warn).toHaveBeenCalled();
    expect(toast.message).not.toHaveBeenCalled();
    expect(toast.error).not.toHaveBeenCalled();
    expect(phase().kind).toBe("closed");
  });
});

describe("UpdaterHost", () => {
  it("checks at launch once the preferences are loaded, when the preference is on", async () => {
    vi.useFakeTimers();
    const check = vi.spyOn(ipc.updater, "check");
    render(<UpdaterHost />);
    await act(() => vi.advanceTimersByTimeAsync(LAUNCH_CHECK_DELAY_MS + 100));
    // The preferences have not loaded yet: nothing is asked on a guess.
    expect(check).not.toHaveBeenCalled();
    act(() => usePrefsStore.setState({ loaded: true }));
    await act(() => vi.advanceTimersByTimeAsync(LAUNCH_CHECK_DELAY_MS + 100));
    expect(check).toHaveBeenCalledTimes(1);
    expect(check).toHaveBeenCalledWith(true);
  });

  it("does not check at launch when the preference is off", async () => {
    vi.useFakeTimers();
    const check = vi.spyOn(ipc.updater, "check");
    usePrefsStore.setState({ loaded: true });
    setPref("autoCheckUpdates", false);
    render(<UpdaterHost />);
    await act(() => vi.advanceTimersByTimeAsync(LAUNCH_CHECK_DELAY_MS * 3));
    expect(check).not.toHaveBeenCalled();
  });

  it("runs Check for Updates… from the native menu", async () => {
    setMockUpdate(NEXT);
    render(<UpdaterHost />);
    expect(actions.has("app.check-updates")).toBe(true);
    await act(async () => {
      expect(await dispatchMenuAction("app.check-updates")).toBe(true);
    });
    expect(await screen.findByText(`Tabler ${NEXT.version} is available`)).toBeInTheDocument();
    expect(screen.getByLabelText("What changed")).toHaveTextContent("Updates install from inside");
    expect(screen.getByText("Published November 12, 2026")).toBeInTheDocument();
  });

  it("walks the dialog from the offer to the restart", async () => {
    setMockUpdate(NEXT);
    useAppInfoStore.setState({
      info: {
        version: "0.1.2",
        platform: "macos",
        debug: false,
        commit: "1a2b3c4",
        channel: "nightly",
      },
    });
    render(<UpdaterHost />);
    await act(() => checkForUpdates());
    expect(screen.getByText("You have 0.1.2 (1a2b3c4).")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Download Update" }));
    expect(await screen.findByRole("progressbar")).toBeInTheDocument();
    const restart = await screen.findByRole("button", { name: "Install and Restart" });
    expect(screen.getByText(`Tabler ${NEXT.version} is ready to install`)).toBeInTheDocument();
    fireEvent.click(restart);
    await waitFor(() => expect(mockUpdateInstalls).toEqual([NEXT.version]));
  });

  it("names the channel of an up-to-date build", async () => {
    useAppInfoStore.setState({
      info: {
        version: "0.1.2",
        platform: "macos",
        debug: false,
        commit: "1a2b3c4",
        channel: "nightly",
      },
    });
    render(<UpdaterHost />);
    await act(() => checkForUpdates());
    expect(screen.getByText("Tabler is up to date")).toBeInTheDocument();
    expect(
      screen.getByText("0.1.2 (1a2b3c4) is the newest build of the nightly channel."),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "OK" }));
    expect(phase().kind).toBe("closed");
  });

  it("says why a build does not update itself", async () => {
    vi.spyOn(ipc.updater, "check").mockResolvedValueOnce({
      state: "unsupported",
      reason: "This is a development build: it does not update itself.",
    });
    render(<UpdaterHost />);
    await act(() => checkForUpdates());
    expect(screen.getByText("Updates are not available")).toBeInTheDocument();
    expect(
      screen.getByText("This is a development build: it does not update itself."),
    ).toBeInTheDocument();
  });
});

describe("General ▸ Application ▸ Automatically check for updates", () => {
  it("turns the check at launch off and on, and names the running build", () => {
    useAppInfoStore.setState({
      info: {
        version: "0.1.2",
        platform: "macos",
        debug: false,
        commit: "1a2b3c4",
        channel: "nightly",
      },
    });
    render(<UpdateSettingRow />);
    const toggle = screen.getByRole("switch", { name: "Automatically check for updates" });
    expect(toggle).toBeChecked();
    expect(screen.getByText("0.1.2 (1a2b3c4)")).toBeInTheDocument();
    expect(screen.getByText(/on the nightly channel/)).toBeInTheDocument();
    fireEvent.click(toggle);
    expect(usePrefsStore.getState().prefs.autoCheckUpdates).toBe(false);
    expect(screen.getByText(/Only Check for Updates… looks for a newer build/)).toBeInTheDocument();
    fireEvent.click(toggle);
    expect(usePrefsStore.getState().prefs.autoCheckUpdates).toBe(true);
  });

  it("checks now", async () => {
    render(<UpdateSettingRow />);
    fireEvent.click(screen.getByRole("button", { name: "Check Now" }));
    await waitFor(() => expect(phase().kind).toBe("upToDate"));
  });
});

describe("the dialog's labels", () => {
  it("names a build by its version and commit", () => {
    expect(buildLabel(null)).toBe("");
    expect(buildLabel({ version: "0.1.0" })).toBe("0.1.0");
    expect(buildLabel({ version: "0.1.0", commit: null })).toBe("0.1.0");
    expect(buildLabel({ version: "0.1.2", commit: "1a2b3c4" })).toBe("0.1.2 (1a2b3c4)");
  });

  it("describes the download", () => {
    expect(progressLabel(null)).toBe("Starting the download…");
    expect(progressLabel({ downloaded: 1536, total: null })).toBe("1.5 KB");
    expect(progressLabel({ downloaded: 12 * 1024 * 1024, total: 48 * 1024 * 1024 })).toBe(
      "12.0 MB of 48.0 MB",
    );
    expect(progressPercent(null)).toBeNull();
    expect(progressPercent({ downloaded: 10, total: null })).toBeNull();
    expect(progressPercent({ downloaded: 12, total: 48 })).toBe(25);
    expect(progressPercent({ downloaded: 60, total: 48 })).toBe(100);
  });

  it("shows only the versions newer than the running one", () => {
    const notes = [
      "0.1.4",
      "",
      "Fixed",
      "- Quitting keeps unsaved work",
      "",
      "0.1.3",
      "",
      "New",
      "- Updates install from inside the app",
      "",
      "0.1.2",
      "",
      "Fixed",
      "- The sidebar keeps its width",
    ].join("\n");
    // From 0.1.2: what 0.1.3 and 0.1.4 brought.
    expect(notesSince(notes, "0.1.2")).toBe(notes.slice(0, notes.indexOf("\n\n0.1.2")));
    // From 0.1.3: only 0.1.4.
    expect(notesSince(notes, "0.1.3")).toBe("0.1.4\n\nFixed\n- Quitting keeps unsaved work");
    // A version the notes do not go back to, or notes in another shape: everything.
    expect(notesSince(notes, "0.1.0")).toBe(notes);
    expect(notesSince("- fixes 0.1.2 crash", "0.1.2")).toBe("- fixes 0.1.2 crash");
    expect(notesSince(null, "0.1.2")).toBeNull();
    expect(notesSince("  \n", "0.1.2")).toBeNull();
  });

  it("shows the publish date, or nothing for a missing or broken one", () => {
    expect(publishedLabel("2026-11-12T08:00:00Z")).toBe("November 12, 2026");
    expect(publishedLabel(null)).toBe("");
    expect(publishedLabel("soon")).toBe("");
  });
});
