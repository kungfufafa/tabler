import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc, type ConnectionInfo, type ConnectionProfile, type SafeModeLevel } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { guardExecution, SEND_QUERIES_TITLE, type GuardRequest } from "./guard";
import { SafetyHost } from "./SafetyHost";
import { changeSafeModeLevel } from "./setLevel";
import { useSafetyPromptStore } from "./store";

// The mock backend stores the DB password "secret" for this seeded profile.
const PROFILE_ID = "p-local-pg";

function info(patch: Partial<ConnectionInfo> = {}): ConnectionInfo {
  return {
    connectionId: "c1",
    profileId: PROFILE_ID,
    name: "Local Postgres",
    color: "blue",
    driver: "postgres",
    serverVersion: "PostgreSQL 17.6",
    currentDatabase: "tabler_dev",
    currentUser: "tabler",
    tunnel: null,
    readOnly: false,
    ...patch,
  };
}

function profile(
  safeMode: SafeModeLevel | null,
  options: Partial<ConnectionProfile["options"]> = {},
) {
  return {
    id: PROFILE_ID,
    name: "Local Postgres",
    driver: "postgres",
    options: {
      tag: "production",
      safeMode,
      timezone: null,
      connectTimeoutSecs: null,
      readOnly: false,
      startupSql: null,
      defaultRowLimit: null,
      sqliteForeignKeys: true,
      ...options,
    },
  } as ConnectionProfile;
}

let hasPassword = true;
function setup(level: SafeModeLevel | null, patch: Partial<ConnectionInfo> = {}) {
  vi.spyOn(ipc.profiles, "get").mockResolvedValue(profile(level));
  useConnectionsStore.getState().add(info(patch));
}

const query = (...statements: string[]): GuardRequest => ({ kind: "query", statements });

beforeEach(() => {
  hasPassword = true;
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useSafetyPromptStore.setState({ queue: [] });
  vi.spyOn(ipc.profiles, "secretStatus").mockImplementation(async () => ({
    hasPassword,
    hasSshPassword: false,
    hasSshPassphrase: false,
  }));
});
afterEach(() => vi.restoreAllMocks());

describe("guardExecution", () => {
  it("lets everything through in Silent Mode and for profiles without a level", async () => {
    render(<SafetyHost />);
    setup("silent");
    await expect(guardExecution("c1", query("DROP TABLE users"))).resolves.toBe(true);
    setup(null);
    await expect(guardExecution("c1", { kind: "commit", statements: ["x"] })).resolves.toBe(true);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("keeps asking what it asked last when the profile cannot be loaded (fails closed)", async () => {
    render(<SafetyHost />);
    setup("safeNonSelect");
    await expect(guardExecution("c1", query("SELECT 1"))).resolves.toBe(true);
    // The profile was deleted while connected.
    vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.spyOn(ipc.profiles, "get").mockRejectedValue(new Error("not found: profile"));
    await expect(guardExecution("c1", query("SELECT 2"))).resolves.toBe(true);
    const asked = guardExecution("c1", query("DELETE FROM t"));
    const dialog = await screen.findByRole("alertdialog", {
      name: "Type the name of Local Postgres to continue",
    });
    expect(dialog).toHaveTextContent("Safe Mode 2");
    fireEvent.keyDown(dialog, { key: "Escape" });
    await expect(asked).resolves.toBe(false);
  });

  it("asks with Alert Mode 1 when a profile it never loaded cannot be loaded", async () => {
    render(<SafetyHost />);
    vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.spyOn(ipc.profiles, "get").mockRejectedValue(new Error("app database: locked"));
    useConnectionsStore.getState().add(info({ connectionId: "c9", profileId: "p-never-loaded" }));
    const asked = guardExecution("c9", query("SELECT 1"));
    const dialog = await screen.findByRole("alertdialog", { name: SEND_QUERIES_TITLE });
    expect(dialog).toHaveTextContent("Alert Mode 1");
    fireEvent.click(within(dialog).getByRole("button", { name: "Send" }));
    await expect(asked).resolves.toBe(true);
  });

  it("does not guard a connection that is not open", async () => {
    const get = vi.spyOn(ipc.profiles, "get");
    await expect(guardExecution("nope", query("DELETE FROM t"))).resolves.toBe(true);
    expect(get).not.toHaveBeenCalled();
  });

  it("Alert Mode 1 asks before every query; Send goes ahead", async () => {
    render(<SafetyHost />);
    setup("alert");
    const asked = guardExecution("c1", query("SELECT 1", "SELECT 2"));
    const dialog = await screen.findByRole("alertdialog", {
      name: "Do you want to send these queries to the server?",
    });
    expect(dialog).toHaveTextContent("Local Postgres");
    expect(dialog).toHaveTextContent("production");
    expect(dialog).toHaveTextContent("Alert Mode 1");
    const list = within(dialog).getByRole("list", { name: "Queries" });
    expect(
      within(list)
        .getAllByRole("listitem")
        .map((li) => li.textContent),
    ).toEqual(["SELECT 1", "SELECT 2"]);
    const send = within(dialog).getByRole("button", { name: "Send" });
    expect(send).toHaveFocus();
    fireEvent.click(send);
    await expect(asked).resolves.toBe(true);
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  it("Escape cancels and ↩ confirms the alert", async () => {
    render(<SafetyHost />);
    setup("alert");
    const first = guardExecution("c1", query("SELECT 1"));
    fireEvent.keyDown(await screen.findByRole("alertdialog"), { key: "Escape" });
    await expect(first).resolves.toBe(false);

    const second = guardExecution("c1", query("SELECT 1"));
    const list = await screen.findByRole("list", { name: "Queries" });
    // The held ↩ / ⌘↩ that ran the query does not confirm it.
    fireEvent.keyDown(list, { key: "Enter", repeat: true });
    fireEvent.keyDown(list, { key: "Enter", metaKey: true });
    expect(useSafetyPromptStore.getState().queue).toHaveLength(1);
    fireEvent.keyDown(list, { key: "Enter" });
    await expect(second).resolves.toBe(true);
  });

  it("Alert Mode 2 skips reads and asks for writes; commits say Commit", async () => {
    render(<SafetyHost />);
    setup("alertNonSelect");
    await expect(
      guardExecution("c1", query("SELECT 1", "-- note\nEXPLAIN DELETE FROM t", "SHOW all")),
    ).resolves.toBe(true);
    expect(screen.queryByRole("alertdialog")).toBeNull();

    const commit = guardExecution("c1", { kind: "commit", statements: ["SELECT 1"] });
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent("Alert Mode 2");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await expect(commit).resolves.toBe(false);

    const run = guardExecution("c1", query("SELECT 1", "UPDATE t SET a = 1"));
    fireEvent.click(await screen.findByRole("button", { name: "Send" }));
    await expect(run).resolves.toBe(true);
  });

  it("titles an operation with its label and confirms with Continue", async () => {
    render(<SafetyHost />);
    setup("alertNonSelect");
    await expect(
      guardExecution("c1", { kind: "operation", label: "Back up tabler_dev", write: false }),
    ).resolves.toBe(true);
    const restore = guardExecution("c1", {
      kind: "operation",
      label: "Restore tabler_dev from backup.dump",
      write: true,
    });
    const dialog = await screen.findByRole("alertdialog", {
      name: "Restore tabler_dev from backup.dump",
    });
    expect(within(dialog).queryByRole("list")).toBeNull();
    fireEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
    await expect(restore).resolves.toBe(true);
  });

  it("lists at most 50 statements", async () => {
    render(<SafetyHost />);
    setup("alert");
    const statements = Array.from({ length: 53 }, (_, i) => `INSERT INTO t VALUES (${i})`);
    const asked = guardExecution("c1", { kind: "commit", statements });
    const list = await screen.findByRole("list", { name: "Queries" });
    const items = within(list).getAllByRole("listitem");
    expect(items).toHaveLength(51);
    expect(items.at(-1)).toHaveTextContent("+3 more");
    fireEvent.click(screen.getByRole("button", { name: "Commit" }));
    await expect(asked).resolves.toBe(true);
  });

  it("refuses writes on a read-only connection without asking", async () => {
    const error = vi.spyOn(toast, "error");
    render(<SafetyHost />);
    setup("alert", { readOnly: true });
    await expect(guardExecution("c1", query("DELETE FROM t"))).resolves.toBe(false);
    expect(error).toHaveBeenCalledWith("This connection is read-only", expect.anything());
    expect(screen.queryByRole("alertdialog")).toBeNull();
    // Reads still follow the level.
    const read = guardExecution("c1", query("SELECT 1"));
    fireEvent.click(await screen.findByRole("button", { name: "Send" }));
    await expect(read).resolves.toBe(true);
  });

  it("Safe Mode 1 asks for the password; a wrong one can be retried", async () => {
    const user = userEvent.setup();
    render(<SafetyHost />);
    setup("safe");
    const asked = guardExecution("c1", query("SELECT 1"));
    const dialog = await screen.findByRole("alertdialog", {
      name: "Enter the password of Local Postgres to continue",
    });
    expect(dialog).toHaveTextContent("Safe Mode 1");
    const field = within(dialog).getByLabelText("Password");
    expect(field).toHaveFocus();
    expect(field).toHaveAttribute("type", "password");
    await user.type(field, "nope{Enter}");
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Wrong password");
    expect(field).toHaveAttribute("aria-invalid", "true");

    await user.clear(field);
    await user.type(field, "secret");
    expect(within(dialog).queryByRole("alert")).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "Send" }));
    await expect(asked).resolves.toBe(true);
  });

  it("Safe Mode 2 only asks for writes; Cancel declines", async () => {
    render(<SafetyHost />);
    setup("safeNonSelect");
    await expect(guardExecution("c1", query("SELECT 1"))).resolves.toBe(true);
    const asked = guardExecution("c1", query("TRUNCATE t"));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await expect(asked).resolves.toBe(false);
  });

  it("asks for the connection's name when no password is stored", async () => {
    const user = userEvent.setup();
    hasPassword = false;
    const verify = vi.spyOn(ipc.safety, "verifyPassword");
    render(<SafetyHost />);
    setup("safe");
    const asked = guardExecution("c1", { kind: "commit", statements: ["DELETE FROM t"] });
    const dialog = await screen.findByRole("alertdialog", {
      name: "Type the name of Local Postgres to continue",
    });
    expect(dialog).toHaveTextContent("no saved password");
    const field = within(dialog).getByLabelText("Connection name");
    await user.type(field, "Local{Enter}");
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("not the name");
    await user.clear(field);
    await user.type(field, "Local Postgres{Enter}");
    await expect(asked).resolves.toBe(true);
    expect(verify).not.toHaveBeenCalled();
  });

  it("queues concurrent requests: one dialog at a time, in order", async () => {
    render(<SafetyHost />);
    setup("alert");
    const first = guardExecution("c1", query("SELECT 'first'"));
    const second = guardExecution("c1", query("SELECT 'second'"));
    const dialog = await screen.findByRole("alertdialog");
    await waitFor(() => expect(useSafetyPromptStore.getState().queue).toHaveLength(2));
    expect(screen.getAllByRole("alertdialog")).toHaveLength(1);
    expect(dialog).toHaveTextContent("SELECT 'first'");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await expect(first).resolves.toBe(false);
    await waitFor(() =>
      expect(screen.getByRole("alertdialog")).toHaveTextContent("SELECT 'second'"),
    );
    await waitFor(() => expect(screen.getByRole("button", { name: "Send" })).toHaveFocus());
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await expect(second).resolves.toBe(true);
  });

  it("declines when no dialog host is mounted, and pending prompts when it unmounts", async () => {
    setup("alert");
    await expect(guardExecution("c1", query("SELECT 1"))).resolves.toBe(false);

    const { unmount } = render(<SafetyHost />);
    const pending = guardExecution("c1", query("SELECT 1"));
    await screen.findByRole("alertdialog");
    act(() => unmount());
    await expect(pending).resolves.toBe(false);
  });
});

describe("changeSafeModeLevel", () => {
  const conn = { profileId: PROFILE_ID, name: "Local Postgres", driver: "postgres" as const };

  it("saves the new level right away when it asks for more", async () => {
    vi.spyOn(ipc.profiles, "get").mockResolvedValue(profile("alert"));
    const save = vi.spyOn(ipc.profiles, "save").mockImplementation(async (i) => i.profile);
    const saved = await changeSafeModeLevel(conn, "safe");
    expect(saved?.options.safeMode).toBe("safe");
    expect(save).toHaveBeenCalledWith(
      expect.objectContaining({ password: null, clearPassword: false }),
    );
  });

  it("asks for the password before leaving a Safe level", async () => {
    const user = userEvent.setup();
    render(<SafetyHost />);
    vi.spyOn(ipc.profiles, "get").mockResolvedValue(profile("safe"));
    const save = vi.spyOn(ipc.profiles, "save").mockImplementation(async (i) => i.profile);

    const cancelled = changeSafeModeLevel(conn, "silent");
    let dialog = await screen.findByRole("alertdialog", {
      name: "Enter the password of Local Postgres to continue",
    });
    expect(dialog).toHaveTextContent("Switching from Safe Mode 1 to Silent Mode.");
    fireEvent.keyDown(dialog, { key: "Escape" });
    await expect(cancelled).resolves.toBeNull();
    expect(save).not.toHaveBeenCalled();

    const changed = changeSafeModeLevel(conn, "safeNonSelect");
    dialog = await screen.findByRole("alertdialog");
    await user.type(within(dialog).getByLabelText("Password"), "secret{Enter}");
    await expect(changed).resolves.toMatchObject({ options: { safeMode: "safeNonSelect" } });
    expect(save).toHaveBeenCalledTimes(1);
  });
});
