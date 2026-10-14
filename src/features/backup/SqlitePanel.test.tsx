import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc, type ConnectionProfile } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import { SqlitePanel } from "./SqlitePanel";

const toast = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  message: vi.fn(),
  warning: vi.fn(),
  loading: vi.fn(),
  dismiss: vi.fn(),
}));
vi.mock("sonner", () => ({ toast }));

const DEST = "/Users/me/Backups/remote-copy.sqlite";

async function remoteProfile(): Promise<ConnectionProfile> {
  const base = (await ipc.profiles.list()).find((p) => p.id === "p-sample-sqlite")!;
  return {
    ...base,
    sqlitePath: "/srv/app/data.sqlite",
    ssh: {
      host: "files.example.com",
      port: 22,
      user: "deploy",
      auth: "agent",
      keyPath: null,
      localPort: null,
      keepaliveSecs: null,
    },
  };
}

beforeEach(() => {
  resetPrefsForTests({ ...DEFAULT_PREFS, exportRevealPopup: false });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  vi.spyOn(window, "prompt").mockReturnValue(DEST);
});
afterEach(() => vi.restoreAllMocks());

describe("SqlitePanel over SSH", () => {
  it("warns that an open connection's backup is the cached copy, and offers a fresh one", async () => {
    const user = userEvent.setup();
    const profile = await remoteProfile();
    const info = await ipc.connections.connect(profile.id);
    act(() => useConnectionsStore.getState().add(info));
    const copy = vi.spyOn(ipc.backup, "copySqlite");
    render(<SqlitePanel profile={profile} mode="backup" onChooseOther={null} />);

    expect(screen.getByRole("note")).toHaveTextContent(/cached copy/);
    expect(screen.getByText("files.example.com:/srv/app/data.sqlite")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Copy Cached File/ }));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(copy).toHaveBeenLastCalledWith(profile.id, DEST, false);
    expect(JSON.stringify(toast.success.mock.calls[0])).toContain("cached copy");

    toast.success.mockClear();
    await user.click(screen.getByRole("button", { name: /Download Fresh Copy First/ }));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(copy).toHaveBeenLastCalledWith(profile.id, DEST, true);
    expect(JSON.stringify(toast.success.mock.calls[0])).not.toContain("cached copy");
  });

  it("downloads the file for the backup when no connection of the profile is open", async () => {
    const user = userEvent.setup();
    const profile = await remoteProfile();
    const copy = vi.spyOn(ipc.backup, "copySqlite");
    render(<SqlitePanel profile={profile} mode="backup" onChooseOther={null} />);
    expect(screen.getByRole("note")).toHaveTextContent(/downloaded for the backup/);
    expect(screen.queryByRole("button", { name: /Download Fresh Copy First/ })).toBeNull();
    await user.click(screen.getByRole("button", { name: /Download and Copy/ }));
    await waitFor(() => expect(copy).toHaveBeenCalledWith(profile.id, DEST, false));
  });

  it("a local file has no such note and no third argument", async () => {
    const user = userEvent.setup();
    const profile = (await ipc.profiles.list()).find((p) => p.id === "p-sample-sqlite")!;
    const copy = vi.spyOn(ipc.backup, "copySqlite");
    render(<SqlitePanel profile={profile} mode="backup" onChooseOther={null} />);
    expect(screen.queryByRole("note")).toBeNull();
    await user.click(screen.getByRole("button", { name: /Copy Database File/ }));
    await waitFor(() => expect(copy).toHaveBeenCalledWith(profile.id, DEST));
  });
});
