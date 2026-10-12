import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import { ipc, type BackupJobRequest, type BackupJobResult } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { BackupHost } from "./BackupHost";
import { BACKUP_OPTIONS_KEY, loadBackupOptions } from "./options";
import { openBackup, resetBackupStore, useBackupStore } from "./store";

const toast = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  message: vi.fn(),
  warning: vi.fn(),
  loading: vi.fn(),
  dismiss: vi.fn(),
}));
vi.mock("sonner", () => ({ toast }));

const guard = vi.hoisted(() => ({
  guardExecution: vi.fn(async () => true),
  READ_ONLY_MESSAGE: "This connection is read-only",
}));
vi.mock("@/features/safety/guard", () => guard);

beforeAll(() => {
  // cmdk scrolls the active option into view.
  Element.prototype.scrollIntoView ??= function scrollIntoView() {};
});

function setup() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <BackupHost />
    </QueryClientProvider>,
  );
}

async function openWindow(request: Parameters<typeof openBackup>[0]) {
  act(() => openBackup(request));
  return screen.findByRole("dialog");
}

const runRequest = (spy: { mock: { calls: unknown[][] } }) =>
  spy.mock.calls[0]![0] as BackupJobRequest;

beforeEach(() => {
  localStorage.clear();
  resetBackupStore();
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
});

afterEach(() => {
  vi.restoreAllMocks();
  Object.values(toast).forEach((fn) => fn.mockReset());
  guard.guardExecution.mockReset();
  guard.guardExecution.mockImplementation(async () => true);
});

describe("Backup window", () => {
  it("backs up a database with pg_dump, options and a file name pattern", async () => {
    const user = userEvent.setup();
    setup();
    const dialog = await openWindow({ mode: "backup", profileId: "p-local-pg", database: null });
    expect(within(dialog).getByText("Backup database")).toBeInTheDocument();

    // Column 1: PostgreSQL / MySQL connections only.
    const conns = within(dialog).getByRole("listbox", { name: "Connections" });
    expect(await within(conns).findByText("Local Postgres")).toBeInTheDocument();
    expect(within(conns).getByText("Shop MySQL")).toBeInTheDocument();
    expect(within(conns).queryByText("sample.sqlite")).toBeNull();

    // Column 2: the databases (the profile's default one selected), with search.
    const dbs = within(dialog).getByRole("listbox", { name: "Databases" });
    const selected = await within(dbs).findByRole("option", { selected: true });
    expect(selected).toHaveTextContent("tabler_dev");
    await user.type(within(dialog).getByLabelText("Search databases"), "ecom");
    expect(
      within(dbs)
        .getAllByRole("option")
        .map((o) => o.textContent),
    ).toEqual(["ecommerce_sales"]);
    await user.clear(within(dialog).getByLabelText("Search databases"));

    // Column 3: the version matching the server, options, gzip and the file name.
    expect(await within(dialog).findByText("PostgreSQL 17.6 — pg_dump")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: /Add option/ }));
    await user.type(await screen.findByLabelText("Option"), "--no-ow");
    await user.click(await screen.findByText("--no-owner"));
    expect(within(dialog).getByRole("button", { name: "Remove --no-owner" })).toBeInTheDocument();
    await user.click(within(dialog).getByRole("checkbox", { name: /Compress file using Gzip/ }));
    expect(within(dialog).getByTestId("backup-file-preview").textContent).toMatch(
      /^tabler_dev_\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2}\.sql\.gz$/,
    );

    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Backups");
    const run = vi.spyOn(ipc.backup, "run");
    await user.click(within(dialog).getByRole("button", { name: /Start Backup/ }));

    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    const req = runRequest(run);
    expect(req).toMatchObject({
      kind: "backup",
      profileId: "p-local-pg",
      database: "tabler_dev",
      binaryPath: "/opt/homebrew/opt/postgresql@17/bin/pg_dump",
      flags: ["--no-owner"],
      gzip: true,
    });
    expect(req.path).toMatch(/^\/Users\/me\/Backups\/tabler_dev_.*\.sql\.gz$/);
    expect(toast.success.mock.calls[0]![0]).toBe("Backed up tabler_dev");
    const action = (toast.success.mock.calls[0]![1] as { action: { label: string } }).action;
    expect(action.label).toBe("Show in Finder");
    // The window closed; the options are remembered for PostgreSQL.
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(loadBackupOptions("postgres")).toMatchObject({
      flags: ["--no-owner"],
      gzip: true,
      binaryPath: "/opt/homebrew/opt/postgresql@17/bin/pg_dump",
    });
    expect(localStorage.getItem(BACKUP_OPTIONS_KEY)).toContain("/Users/me/Backups");
  });

  it("custom-format archives are not gzipped and get .dump", async () => {
    const user = userEvent.setup();
    setup();
    const dialog = await openWindow({ mode: "backup", profileId: "p-local-pg", database: null });
    await within(dialog).findByText("PostgreSQL 17.6 — pg_dump");
    await user.click(within(dialog).getByRole("checkbox", { name: /Compress file using Gzip/ }));
    await user.click(within(dialog).getByRole("button", { name: /Add option/ }));
    await user.click(await screen.findByText("--format=custom"));
    expect(
      within(dialog).getByRole("checkbox", { name: /Compress file using Gzip/ }),
    ).toBeDisabled();
    expect(within(dialog).getByTestId("backup-file-preview").textContent).toMatch(/\.dump$/);
  });

  it("shows a failure and its log, then goes back to the options", async () => {
    const user = userEvent.setup();
    setup();
    const dialog = await openWindow({ mode: "backup", profileId: "p-shop-mysql", database: "sys" });
    const dbs = within(dialog).getByRole("listbox", { name: "Databases" });
    expect(await within(dbs).findByRole("option", { selected: true })).toHaveTextContent("sys");
    expect(await within(dialog).findByText("MySQL 9.6.0 — mysqldump")).toBeInTheDocument();
    // MySQL starts from --single-transaction.
    expect(
      within(dialog).getByRole("button", { name: "Remove --single-transaction" }),
    ).toBeInTheDocument();
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Backups");
    await user.click(within(dialog).getByRole("button", { name: /Start Backup/ }));

    const alert = await within(dialog).findByText(/mysqldump failed: Got error: 1044/);
    expect(alert).toBeInTheDocument();
    const log = within(dialog).getByRole("log");
    expect(log).toHaveTextContent("Access denied for user");
    expect(log).toHaveTextContent("--protocol=TCP");
    expect(toast.error).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole("button", { name: /Back/ }));
    expect(within(dialog).getByRole("button", { name: /Start Backup/ })).toBeInTheDocument();
  });

  it("cancels a running backup", async () => {
    const user = userEvent.setup();
    setup();
    const dialog = await openWindow({ mode: "backup", profileId: "p-local-pg", database: null });
    await within(dialog).findByText("PostgreSQL 17.6 — pg_dump");
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Backups");
    let reject: (e: unknown) => void = () => {};
    vi.spyOn(ipc.backup, "run").mockImplementation(
      (_req, onEvent) =>
        new Promise<BackupJobResult>((_resolve, rej) => {
          onEvent({
            event: "started",
            data: { command: "pg_dump --dbname=tabler_dev", path: "/x" },
          });
          onEvent({ event: "progress", data: { bytes: 2048, total: null } });
          reject = rej;
        }),
    );
    const cancel = vi.spyOn(ipc.backup, "cancel").mockResolvedValue(undefined);
    await user.click(within(dialog).getByRole("button", { name: /Start Backup/ }));
    expect(await within(dialog).findByText("Backing up tabler_dev")).toBeInTheDocument();
    expect(within(dialog).getByRole("status")).toHaveTextContent("2.0 KB written");
    await user.click(within(dialog).getByRole("button", { name: "Cancel Backup" }));
    expect(cancel).toHaveBeenCalledWith(useBackupStore.getState().run!.jobId);
    await act(async () => reject({ code: "CANCELLED", message: "query cancelled" }));
    await waitFor(() =>
      expect(toast.message).toHaveBeenCalledWith("Backup cancelled", expect.anything()),
    );
    expect(useBackupStore.getState().run).toBeNull();
  });

  it("explains SQLite and copies its database file", async () => {
    const user = userEvent.setup();
    setup();
    const dialog = await openWindow({
      mode: "backup",
      profileId: "p-sample-sqlite",
      database: null,
    });
    expect(
      await within(dialog).findByText(
        "SQLite databases are backed up by copying the database file.",
      ),
    ).toBeInTheDocument();
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Backups/sample-copy.sqlite");
    const copy = vi.spyOn(ipc.backup, "copySqlite");
    await user.click(within(dialog).getByRole("button", { name: /Copy Database File/ }));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(copy).toHaveBeenCalledWith("p-sample-sqlite", "/Users/me/Backups/sample-copy.sqlite");
    await user.click(within(dialog).getByRole("button", { name: "Choose another connection" }));
    expect(await within(dialog).findByRole("listbox", { name: "Connections" })).toBeInTheDocument();
  });

  it("opens from File ▸ Backup… / Restore… on the active connection", async () => {
    setup();
    const info = await ipc.connections.connect("p-shop-mysql");
    useConnectionsStore.getState().add(info);
    useConnectionsStore.getState().setCurrentSchema(info.connectionId, "shop");
    await waitFor(() => expect(actions.has("restore.open")).toBe(true));
    await act(() => actions.run("restore.open"));
    expect(useBackupStore.getState().request).toEqual({
      mode: "restore",
      profileId: "p-shop-mysql",
      database: "shop",
    });
    expect(await screen.findByText("Restore database")).toBeInTheDocument();
  });
});

describe("Restore window", () => {
  it("restores a pg_dump archive with pg_restore after the safe-mode check", async () => {
    const user = userEvent.setup();
    setup();
    // An open connection: the safe-mode check runs on it.
    const info = await ipc.connections.connect("p-local-pg");
    useConnectionsStore.getState().add(info);
    const dialog = await openWindow({
      mode: "restore",
      profileId: "p-local-pg",
      database: "analytics",
    });
    const dbs = within(dialog).getByRole("listbox", { name: "Databases" });
    expect(await within(dbs).findByRole("option", { selected: true })).toHaveTextContent(
      "analytics",
    );
    // Without a file the start button waits.
    expect(within(dialog).getByRole("button", { name: /Start Restore/ })).toBeDisabled();

    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Backups/app.dump");
    await user.click(within(dialog).getByRole("button", { name: /Choose File/ }));
    expect(await within(dialog).findByTestId("restore-file-name")).toHaveTextContent("app.dump");
    expect(within(dialog).getByText(/pg_dump custom archive/)).toBeInTheDocument();
    expect(await within(dialog).findByText("PostgreSQL 17.6 — pg_restore")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: /Add option/ }));
    await user.click(await screen.findByText("--clean"));

    // Declined: nothing runs.
    guard.guardExecution.mockResolvedValueOnce(false);
    const run = vi.spyOn(ipc.backup, "run");
    await user.click(within(dialog).getByRole("button", { name: /Start Restore/ }));
    await waitFor(() => expect(guard.guardExecution).toHaveBeenCalledOnce());
    expect(guard.guardExecution).toHaveBeenCalledWith(info.connectionId, {
      kind: "operation",
      label: "Restore app.dump into analytics",
      write: true,
    });
    expect(run).not.toHaveBeenCalled();

    await user.click(within(dialog).getByRole("button", { name: /Start Restore/ }));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(runRequest(run)).toMatchObject({
      kind: "restore",
      database: "analytics",
      binaryPath: "/opt/homebrew/opt/postgresql@17/bin/pg_restore",
      flags: ["--clean"],
      path: "/Users/me/Backups/app.dump",
    });
    expect(toast.success.mock.calls[0]![0]).toBe("Restored analytics");
  });

  it("uses psql for plain SQL and keeps the log open when statements failed", async () => {
    const user = userEvent.setup();
    setup();
    const dialog = await openWindow({ mode: "restore", profileId: "p-local-pg", database: null });
    await within(dialog).findByRole("option", { selected: true });
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Backups/broken.sql.gz");
    await user.click(within(dialog).getByRole("button", { name: /Choose File/ }));
    expect(await within(dialog).findByText(/Plain SQL \(gzip\)/)).toBeInTheDocument();
    expect(await within(dialog).findByText("PostgreSQL 17.6 — psql")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: /Start Restore/ }));
    // A silent profile without an open connection needs no safe-mode check.
    expect(guard.guardExecution).not.toHaveBeenCalled();
    expect(await within(dialog).findByText(/1 statement failed/)).toBeInTheDocument();
    expect(within(dialog).getByRole("log")).toHaveTextContent('relation "users" already exists');
    expect(toast.warning).toHaveBeenCalledOnce();
    await user.click(within(dialog).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("refuses a PostgreSQL archive for MySQL", async () => {
    const user = userEvent.setup();
    setup();
    const dialog = await openWindow({ mode: "restore", profileId: "p-shop-mysql", database: null });
    await within(dialog).findByRole("option", { selected: true });
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Backups/app.dump");
    await user.click(within(dialog).getByRole("button", { name: /Choose File/ }));
    expect(await within(dialog).findByText(/not a MySQL dump/)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: /Start Restore/ })).toBeDisabled();
  });
});
