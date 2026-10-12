import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import { menuActionAllowed } from "@/hooks/useNativeMenu";
import { findShortcut } from "@/lib/keys/keymap";
import {
  ipc,
  type AdminResult,
  type ConnectionInfo,
  type DatabaseOptions,
  type Driver,
  type ProcessRow,
  type UserState,
} from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { AdminHost } from "./AdminHost";
import { newUserState } from "./users";
import {
  openCreateDatabase,
  openDropDatabase,
  openProcessList,
  openRenameDatabase,
  openUserManagement,
  useAdminStore,
} from "./store";

const guard = vi.hoisted(() => ({ allow: true, calls: [] as unknown[] }));
vi.mock("@/features/safety/guard", () => ({
  guardExecution: vi.fn(async (connectionId: string, request: unknown) => {
    guard.calls.push([connectionId, request]);
    return guard.allow;
  }),
}));
const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

function info(driver: Driver): ConnectionInfo {
  return {
    connectionId: "c1",
    profileId: "p1",
    name: "Local",
    color: null,
    driver,
    serverVersion: "x",
    currentDatabase: "app",
    currentUser: "me",
    tunnel: null,
    readOnly: false,
  };
}

const ok = (executed: string[]): AdminResult => ({
  executed,
  failedIndex: null,
  error: null,
  durationMs: 2,
});

const options: DatabaseOptions = {
  encodings: [
    { name: "UTF8", description: null, defaultCollation: null },
    { name: "LATIN1", description: null, defaultCollation: null },
  ],
  collations: [
    { name: "C", encoding: null },
    { name: "en_US.UTF-8", encoding: "UTF8" },
  ],
  templates: ["template0", "template1"],
  defaultEncoding: "UTF8",
  defaultCollation: "en_US.UTF-8",
};

let qc: QueryClient;

function setup(driver: Driver = "postgres") {
  useConnectionsStore.getState().add(info(driver));
  qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  qc.setQueryData(
    ["databases", "c1"],
    ["app", "shop"].map((name) => ({
      name,
      owner: null,
      encoding: "UTF8",
      isCurrent: name === "app",
    })),
  );
  qc.setQueryData(["schemas", "c1"], [{ name: "public", isDefault: true }]);
  render(
    <QueryClientProvider client={qc}>
      <AdminHost />
    </QueryClientProvider>,
  );
}

beforeAll(() => {
  if (!Element.prototype.scrollIntoView) Element.prototype.scrollIntoView = () => {};
});

beforeEach(() => {
  guard.allow = true;
  guard.calls = [];
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useAdminStore.setState({ request: null });
});

afterEach(() => {
  vi.restoreAllMocks();
  Object.values(toast).forEach((f) => f.mockReset());
});

describe("databases", () => {
  it("creates a database behind the safe-mode check", async () => {
    vi.spyOn(ipc.admin, "databaseOptions").mockResolvedValue(options);
    const apply = vi.spyOn(ipc.admin, "applyDdl").mockResolvedValue(ok(["CREATE DATABASE x"]));
    setup();
    act(() => openCreateDatabase("c1"));
    const dialog = await screen.findByRole("dialog", { name: "New Database" });
    await waitFor(() => expect(within(dialog).getByLabelText("Encoding")).toHaveValue("UTF8"));
    fireEvent.change(within(dialog).getByLabelText("Name"), { target: { value: "reports" } });
    fireEvent.change(within(dialog).getByLabelText("Encoding"), { target: { value: "LATIN1" } });
    // template1 only allows its own encoding: the template switches to template0 by itself.
    expect(within(dialog).getByLabelText("Template")).toHaveValue("template0");
    fireEvent.click(within(dialog).getByRole("button", { name: /Create/ }));
    await waitFor(() =>
      expect(apply).toHaveBeenCalledWith("c1", {
        op: "createDatabase",
        name: "reports",
        encoding: "LATIN1",
        collation: null,
        template: "template0",
      }),
    );
    expect(guard.calls).toEqual([
      ["c1", { kind: "operation", label: "Create database reports", write: true }],
    ]);
    await waitFor(() => expect(useAdminStore.getState().request).toBeNull());
    expect(toast.success).toHaveBeenCalledWith("Created database reports", expect.anything());
  });

  it("sends nothing when safe mode says no, and shows server errors", async () => {
    vi.spyOn(ipc.admin, "databaseOptions").mockResolvedValue(options);
    const apply = vi.spyOn(ipc.admin, "applyDdl").mockResolvedValue({
      ...ok([]),
      failedIndex: 0,
      error: {
        code: "DB_ERROR",
        message: 'database "shop2" already exists',
        detail: null,
        hint: null,
        sqlstate: "42P04",
        vendorCode: null,
        position: null,
        statementIndex: 0,
        data: null,
      },
    });
    setup();
    act(() => openCreateDatabase("c1"));
    const dialog = await screen.findByRole("dialog", { name: "New Database" });
    fireEvent.change(within(dialog).getByLabelText("Name"), { target: { value: "shop2" } });
    guard.allow = false;
    fireEvent.click(within(dialog).getByRole("button", { name: /Create/ }));
    await waitFor(() => expect(guard.calls).toHaveLength(1));
    expect(apply).not.toHaveBeenCalled();
    guard.allow = true;
    fireEvent.click(within(dialog).getByRole("button", { name: /Create/ }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("already exists");
    expect(useAdminStore.getState().request).not.toBeNull();
  });

  it("refuses an existing name before asking the server", async () => {
    vi.spyOn(ipc.admin, "databaseOptions").mockResolvedValue(options);
    const apply = vi.spyOn(ipc.admin, "applyDdl");
    setup();
    act(() => openCreateDatabase("c1"));
    const dialog = await screen.findByRole("dialog", { name: "New Database" });
    fireEvent.change(within(dialog).getByLabelText("Name"), { target: { value: "shop" } });
    fireEvent.click(within(dialog).getByRole("button", { name: /Create/ }));
    expect(await within(dialog).findByText("A database named shop already exists")).toBeVisible();
    expect(apply).not.toHaveBeenCalled();
  });

  it("drops only once the name is typed", async () => {
    const apply = vi.spyOn(ipc.admin, "applyDdl").mockResolvedValue(ok(["DROP DATABASE shop"]));
    setup();
    act(() => openDropDatabase("c1", "shop"));
    const dialog = await screen.findByRole("alertdialog");
    const drop = within(dialog).getByRole("button", { name: "Drop" });
    expect(drop).toBeDisabled();
    fireEvent.change(within(dialog).getByLabelText("Type shop to confirm"), {
      target: { value: "shop" },
    });
    fireEvent.click(drop);
    await waitFor(() =>
      expect(apply).toHaveBeenCalledWith("c1", { op: "dropDatabase", name: "shop" }),
    );
    await waitFor(() => expect(useAdminStore.getState().request).toBeNull());
  });

  it("refuses to rename the current database", async () => {
    setup();
    act(() => openRenameDatabase("c1", "app"));
    const dialog = await screen.findByRole("dialog", { name: "Rename Database" });
    expect(within(dialog).getByRole("alert")).toHaveTextContent(
      "app is the database this connection is using",
    );
    expect(within(dialog).getByRole("button", { name: "Rename" })).toBeDisabled();
  });

  it("closes when its connection goes away", async () => {
    setup();
    act(() => openRenameDatabase("c1", "shop"));
    await screen.findByRole("dialog");
    act(() => useConnectionsStore.getState().remove("c1"));
    await waitFor(() => expect(useAdminStore.getState().request).toBeNull());
  });
});

function proc(id: number, patch: Partial<ProcessRow> = {}): ProcessRow {
  return {
    id,
    user: "app",
    database: "shop",
    application: "psql",
    client: "10.0.0.5:5000",
    state: "active",
    command: null,
    startedAt: null,
    durationMs: 4000,
    waitEvent: null,
    query: `select ${id}`,
    current: false,
    own: false,
    ...patch,
  };
}

describe("process list", () => {
  it("lists sessions and kills the selected queries, warning about Tabler's own", async () => {
    vi.spyOn(ipc.admin, "processes").mockResolvedValue([
      proc(10, { application: "tabler", own: true, state: "idle" }),
      proc(11),
      proc(12, { database: "app" }),
    ]);
    const kill = vi.spyOn(ipc.admin, "kill").mockResolvedValue([
      { id: 10, ok: true, message: null },
      { id: 11, ok: true, message: null },
    ]);
    setup();
    act(() => openProcessList("c1"));
    const dialog = await screen.findByRole("dialog", { name: "Running Queries" });
    const rowOf = (id: number) => dialog.querySelector(`[data-process="${id}"]`) as HTMLElement;
    await waitFor(() => expect(rowOf(11)).not.toBeNull());
    expect(within(rowOf(10)).getByText("Tabler")).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole("checkbox", { name: "Hide idle" }));
    expect(rowOf(10)).toBeNull();
    fireEvent.click(within(dialog).getByRole("checkbox", { name: "Hide idle" }));

    fireEvent.click(rowOf(10));
    fireEvent.click(rowOf(11), { shiftKey: true });
    expect(within(dialog).getByText(/2 selected/)).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Kill Query" }));
    const confirm = await screen.findByRole("alertdialog");
    expect(confirm).toHaveTextContent("Kill the queries of 2 processes?");
    expect(within(confirm).getByRole("alert")).toHaveTextContent("Tabler's own session");
    fireEvent.click(within(confirm).getByRole("button", { name: "Kill Query" }));
    await waitFor(() => expect(kill).toHaveBeenCalledWith("c1", [10, 11], "query"));
    expect(guard.calls).toEqual([
      ["c1", { kind: "operation", label: "Kill the query of 2 processes", write: true }],
    ]);
    await waitFor(() =>
      expect(toast.success).toHaveBeenCalledWith("Cancelled the query of 2 processes"),
    );
  });

  it("filters by database and sorts by a column", async () => {
    vi.spyOn(ipc.admin, "processes").mockResolvedValue([
      proc(1, { durationMs: 10 }),
      proc(2, { durationMs: 900, database: "app" }),
    ]);
    setup();
    act(() => openProcessList("c1"));
    const dialog = await screen.findByRole("dialog", { name: "Running Queries" });
    await waitFor(() => expect(dialog.querySelectorAll("[data-process]")).toHaveLength(2));
    const ids = () =>
      [...dialog.querySelectorAll<HTMLElement>("[data-process]")].map((r) => r.dataset.process);
    fireEvent.click(within(dialog).getByRole("button", { name: "Started" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Started" }));
    expect(ids()).toEqual(["2", "1"]);
    fireEvent.change(within(dialog).getByLabelText("Database filter"), {
      target: { value: "shop" },
    });
    expect(ids()).toEqual(["1"]);
  });

  it("is not available for SQLite", async () => {
    const list = vi.spyOn(ipc.admin, "processes");
    setup("sqlite");
    act(() => openProcessList("c1"));
    expect(await screen.findByText("Process list is not available")).toBeInTheDocument();
    expect(list).not.toHaveBeenCalled();
  });
});

describe("user management", () => {
  const app: UserState = {
    ...newUserState("mysql"),
    name: "app",
    host: "%",
    globalPrivileges: ["SELECT"],
  };

  it("edits an account and applies the diff through the admin command", async () => {
    vi.spyOn(ipc.admin, "users").mockResolvedValue([
      { name: "app", host: "%", canLogin: true, superuser: false },
      { name: "root", host: "localhost", canLogin: true, superuser: true },
    ]);
    vi.spyOn(ipc.admin, "user").mockResolvedValue({ state: app, schemas: [] });
    const preview = vi
      .spyOn(ipc.admin, "previewUser")
      .mockResolvedValue(["ALTER USER 'app'@'%' IDENTIFIED BY '********'"]);
    const apply = vi.spyOn(ipc.admin, "applyUser").mockResolvedValue(ok(["GRANT …"]));
    setup("mysql");
    act(() => openUserManagement("c1"));
    const dialog = await screen.findByRole("dialog", { name: "User Management" });
    fireEvent.click(await within(dialog).findByRole("option", { name: /^app/ }));
    const grid = await within(dialog).findByRole("group", { name: "Global privileges" });
    expect(within(grid).getByRole("checkbox", { name: "SELECT" })).toBeChecked();
    const apply_ = within(dialog).getByRole("button", { name: "Apply" });
    expect(apply_).toBeDisabled();
    fireEvent.click(within(grid).getByRole("checkbox", { name: "PROCESS" }));
    fireEvent.change(within(dialog).getByLabelText("Password"), { target: { value: "s3cret!" } });

    fireEvent.click(within(dialog).getByRole("button", { name: "Preview SQL" }));
    await waitFor(() => expect(preview).toHaveBeenCalled());
    // The preview never carries the password itself.
    expect(preview.mock.calls.every(([, change]) => change.password === "********")).toBe(true);

    fireEvent.click(apply_);
    await waitFor(() =>
      expect(apply).toHaveBeenCalledWith("c1", {
        original: app,
        edited: { ...app, globalPrivileges: ["SELECT", "PROCESS"] },
        password: "s3cret!",
      }),
    );
    expect(guard.calls).toEqual([
      ["c1", { kind: "operation", label: "Apply changes to user 'app'@'%'", write: true }],
    ]);
  });

  it("asks before dropping unapplied edits, and drops accounts after a confirm", async () => {
    vi.spyOn(ipc.admin, "users").mockResolvedValue([
      { name: "app", host: "%", canLogin: true, superuser: false },
      { name: "root", host: "localhost", canLogin: true, superuser: true },
    ]);
    vi.spyOn(ipc.admin, "user").mockResolvedValue({ state: app, schemas: [] });
    const apply = vi.spyOn(ipc.admin, "applyUser").mockResolvedValue(ok(["DROP USER 'app'@'%'"]));
    setup("mysql");
    act(() => openUserManagement("c1"));
    const dialog = await screen.findByRole("dialog", { name: "User Management" });
    fireEvent.click(await within(dialog).findByRole("option", { name: /^app/ }));
    const name = await within(dialog).findByLabelText("User name");
    fireEvent.change(name, { target: { value: "app2" } });
    fireEvent.click(within(dialog).getByRole("option", { name: /root/ }));
    const ask = await screen.findByRole("alertdialog");
    expect(ask).toHaveTextContent("Discard changes?");
    fireEvent.click(within(ask).getByRole("button", { name: "Keep editing" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(within(dialog).getByLabelText("User name")).toHaveValue("app2");
    fireEvent.click(within(dialog).getByRole("button", { name: "Discard" }));
    expect(within(dialog).getByLabelText("User name")).toHaveValue("app");

    fireEvent.click(within(dialog).getByRole("button", { name: "Drop user" }));
    const confirm = await screen.findByRole("alertdialog");
    expect(confirm).toHaveTextContent("Drop user 'app'@'%'?");
    fireEvent.click(within(confirm).getByRole("button", { name: "Drop" }));
    await waitFor(() =>
      expect(apply).toHaveBeenCalledWith("c1", { original: app, edited: null, password: null }),
    );
  });

  it("creates a PostgreSQL role", async () => {
    vi.spyOn(ipc.admin, "users").mockResolvedValue([]);
    const apply = vi.spyOn(ipc.admin, "applyUser").mockResolvedValue(ok(["CREATE ROLE"]));
    setup("postgres");
    act(() => openUserManagement("c1"));
    const dialog = await screen.findByRole("dialog", { name: "User Management" });
    fireEvent.click(within(dialog).getByRole("button", { name: "New role" }));
    fireEvent.change(await within(dialog).findByLabelText("Role name"), {
      target: { value: "reader" },
    });
    fireEvent.click(within(dialog).getByRole("checkbox", { name: /Create databases/ }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(apply).toHaveBeenCalled());
    const change = apply.mock.calls[0]![1];
    expect(change.original).toBeNull();
    expect(change.edited).toMatchObject({
      name: "reader",
      host: null,
      attributes: { login: true, createDb: true },
    });
  });
});

describe("actions", () => {
  it("registers the Connection menu actions for the active connection", async () => {
    setup("mysql");
    expect(actions.has("admin.process-list")).toBe(true);
    expect(actions.has("admin.users")).toBe(true);
    expect(actions.has("admin.new-database")).toBe(true);
    // Schemas are PostgreSQL's.
    expect(actions.has("admin.new-schema")).toBe(false);
    expect(actions.has("connection.disconnect")).toBe(true);
    vi.spyOn(ipc.admin, "processes").mockResolvedValue([]);
    await act(() => actions.run("admin.process-list"));
    expect(useAdminStore.getState().request).toEqual({ panel: "processList", connectionId: "c1" });
  });

  it("binds ⌘. to the process list outside the SQL editor, where it stays Cancel", () => {
    expect(findShortcut("Mod+.", "global")?.id).toBe("admin.process-list");
    expect(findShortcut("Mod+.", "grid")?.id).toBe("admin.process-list");
    expect(findShortcut("Mod+.", "editor")?.id).toBe("query.cancel");
    expect(findShortcut("Mod+.", "dialog")).toBeUndefined();
  });

  it("lets the Connection menu items run from the editor, not from dialogs", () => {
    for (const id of ["admin.process-list", "admin.users", "connection.disconnect"] as const) {
      expect(menuActionAllowed(id, "editor")).toBe(true);
      expect(menuActionAllowed(id, "grid-edit")).toBe(true);
      expect(menuActionAllowed(id, "dialog")).toBe(false);
    }
  });
});
