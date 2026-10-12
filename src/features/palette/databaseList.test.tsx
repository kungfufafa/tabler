import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { useAdminStore } from "@/features/admin/store";
import { useDatabaseSwitchStore } from "@/features/sidebar/switchDatabase";
import { ipc, type ConnectionInfo, type DatabaseInfo, type Driver } from "@/lib/ipc";
import { profile } from "@/lib/ipc/mock/fixtures";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { actions } from "./actions";
import { CommandPalette } from "./CommandPalette";
import { usePaletteStore, type PaletteMode } from "./store";

const toast = vi.hoisted(() => ({
  error: vi.fn(),
  success: vi.fn(),
  message: vi.fn(),
  loading: vi.fn(),
  dismiss: vi.fn(),
}));
vi.mock("sonner", () => ({ toast }));
const clipboard = vi.hoisted(() => ({ write: vi.fn(async (_text: string) => {}) }));
vi.mock("@/features/datagrid/clipboard", () => ({ writeClipboard: clipboard.write }));

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

const databases: DatabaseInfo[] = ["app", "shop"].map((name) => ({
  name,
  owner: null,
  encoding: "UTF8",
  isCurrent: name === "app",
}));

function renderPalette(driver: Driver = "postgres", mode: PaletteMode = "databases") {
  useConnectionsStore.getState().add(info(driver));
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  qc.setQueryData(["profiles"], [profile({ id: "p1", name: "Local", driver })]);
  qc.setQueryData(["databases", "c1"], databases);
  qc.setQueryData(
    ["schemas", "c1"],
    [
      { name: "public", isDefault: true },
      { name: "inventory", isDefault: false },
    ],
  );
  qc.setQueryData(["tables", "c1", "public"], []);
  render(
    <QueryClientProvider client={qc}>
      <CommandPalette />
    </QueryClientProvider>,
  );
  act(() => usePaletteStore.getState().toggle(mode));
  return screen.getByRole("combobox");
}

const option = (label: string) =>
  screen.getAllByRole("option").find((o) => o.textContent?.startsWith(label)) ??
  (() => {
    throw new Error(`no option ${label}`);
  })();

const menuItems = () => screen.getAllByRole("menuitem").map((m) => m.textContent);

beforeAll(() => {
  if (!Element.prototype.scrollIntoView) Element.prototype.scrollIntoView = () => {};
});

const cleanups: Array<() => void> = [];

beforeEach(() => {
  usePaletteStore.setState({ open: false, mode: "all", session: 0 });
  useDatabaseSwitchStore.setState({ pending: {} });
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useUiStore.setState({ view: "workspace", themePref: "system" });
  useAdminStore.setState({ request: null });
});

afterEach(() => {
  cleanups.splice(0).forEach((c) => c());
  vi.restoreAllMocks();
  Object.values(toast).forEach((f) => f.mockReset());
  clipboard.write.mockClear();
});

describe("⌘K database list", () => {
  it("selects on click and opens on double-click", async () => {
    const spy = vi
      .spyOn(ipc.connections, "switchDatabase")
      .mockResolvedValue({ ...info("postgres"), currentDatabase: "shop" });
    renderPalette();
    const shop = option("shop");
    fireEvent.pointerDown(shop);
    fireEvent.click(shop);
    expect(usePaletteStore.getState().open).toBe(true);
    expect(spy).not.toHaveBeenCalled();
    fireEvent.doubleClick(shop);
    await waitFor(() => expect(spy).toHaveBeenCalledWith("c1", "shop"));
  });

  it("offers Open, Copy Name, Rename… and Drop… on right-click (PostgreSQL)", async () => {
    renderPalette();
    fireEvent.contextMenu(option("shop"));
    expect(menuItems()).toEqual(["Open in Current Workspace", "Copy Name", "Rename…", "Drop…"]);
    fireEvent.click(screen.getByRole("menuitem", { name: "Copy Name" }));
    await waitFor(() => expect(clipboard.write).toHaveBeenCalledWith("shop"));
    // Copy Name keeps the list open.
    expect(usePaletteStore.getState().open).toBe(true);

    fireEvent.contextMenu(option("shop"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Drop…" }));
    await waitFor(() =>
      expect(useAdminStore.getState().request).toEqual({
        panel: "dropDatabase",
        connectionId: "c1",
        target: "shop",
      }),
    );
    expect(usePaletteStore.getState().open).toBe(false);
  });

  it("refuses to rename or drop the current database, saying why", async () => {
    renderPalette();
    fireEvent.contextMenu(option("app"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Rename…" }));
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(
        "Cannot rename the current database",
        expect.objectContaining({ description: expect.stringContaining("app is the database") }),
      ),
    );
    expect(useAdminStore.getState().request).toBeNull();
  });

  it("hides Rename for MySQL", () => {
    renderPalette("mysql");
    fireEvent.contextMenu(option("shop"));
    expect(menuItems()).toEqual(["Open in Current Workspace", "Copy Name", "Drop…"]);
  });

  it("creates databases from the + row and the + button, drops the selection with −", async () => {
    const input = renderPalette();
    fireEvent.change(input, { target: { value: "new data" } });
    fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
    await waitFor(() =>
      expect(useAdminStore.getState().request).toEqual({
        panel: "createDatabase",
        connectionId: "c1",
      }),
    );

    useAdminStore.setState({ request: null });
    act(() => usePaletteStore.getState().toggle("databases"));
    // "app" (the current one, first) cannot be dropped.
    expect(screen.getByRole("button", { name: "Drop Database…" })).toBeDisabled();
    const search = screen.getByRole("combobox");
    fireEvent.change(search, { target: { value: "shop" } });
    fireEvent.click(screen.getByRole("button", { name: "Drop Database…" }));
    await waitFor(() =>
      expect(useAdminStore.getState().request).toMatchObject({
        panel: "dropDatabase",
        target: "shop",
      }),
    );
  });

  it("has no database admin for SQLite", () => {
    renderPalette("sqlite");
    expect(screen.queryByText("New Database…")).toBeNull();
    expect(screen.queryByRole("button", { name: "New Database…" })).toBeNull();
    fireEvent.contextMenu(option("public"));
    expect(menuItems()).toEqual(["Show in Sidebar", "Copy Name"]);
  });

  it("drops a schema from its right-click menu (PostgreSQL)", async () => {
    renderPalette();
    fireEvent.contextMenu(option("inventory"));
    expect(menuItems()).toEqual(["Show in Sidebar", "Copy Name", "Drop Schema…"]);
    fireEvent.click(screen.getByRole("menuitem", { name: "Drop Schema…" }));
    await waitFor(() =>
      expect(useAdminStore.getState().request).toEqual({
        panel: "dropSchema",
        connectionId: "c1",
        target: "inventory",
      }),
    );
  });
});

describe("admin commands", () => {
  it("lists the registered admin actions after >", async () => {
    const ran: string[] = [];
    cleanups.push(
      actions.register("admin.users", () => void ran.push("users")),
      actions.register("admin.drop-schema", () => void ran.push("drop-schema")),
      actions.register("admin.process-list", () => void ran.push("processes")),
    );
    const input = renderPalette("postgres", "all");
    fireEvent.change(input, { target: { value: "> user man" } });
    expect(screen.getAllByRole("option")[0]?.textContent).toMatch(/^User Management…/);
    fireEvent.change(input, { target: { value: "> process" } });
    expect(screen.getAllByRole("option")[0]?.textContent).toMatch(/^Process list/);
    fireEvent.change(input, { target: { value: "> drop schema" } });
    fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
    await waitFor(() => expect(ran).toEqual(["drop-schema"]));
    // Not registered: not listed.
    act(() => usePaletteStore.getState().toggle("all"));
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "> new database" } });
    expect(screen.queryByText("New Database…")).toBeNull();
  });
});
