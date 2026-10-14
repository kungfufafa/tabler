import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import {
  initLazyTree,
  LAZY_STORAGE_KEY,
  lazyKey,
  lazyTreeOf,
  resetLazyLoad,
  setLazyLoad,
} from "@/features/sidebar/lazyLoad";
import { ipc, type ConnectionInfo, type TableSummary } from "@/lib/ipc";
import { profile } from "@/lib/ipc/mock/fixtures";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { CommandPalette } from "./CommandPalette";
import { MRU_STORAGE_KEY } from "./mru";
import { usePaletteStore } from "./store";
import { cancelTabSurfaceFocus } from "./focus";

const info: ConnectionInfo = {
  connectionId: "c1",
  profileId: "p1",
  name: "Local PG",
  color: "green",
  driver: "postgres",
  serverVersion: "PostgreSQL 17",
  currentDatabase: "app",
  currentUser: "me",
  tunnel: null,
  readOnly: false,
};

const table = (name: string, schema: string): TableSummary => ({
  name,
  schema,
  kind: "table",
  rowEstimate: 10,
  sizeBytes: null,
  comment: null,
});

const PUBLIC = [table("users", "public"), table("orders", "public")];
const INVENTORY = [table("items", "inventory"), table("stock_orders", "inventory")];

function renderPalette(driver: "postgres" | "mysql" = "postgres") {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  qc.setQueryData(["profiles"], [profile({ id: "p1", name: "Local PG", driver })]);
  qc.setQueryData(
    ["schemas", "c1"],
    [
      { name: "public", isDefault: true },
      { name: "inventory", isDefault: false },
    ],
  );
  qc.setQueryData(["tables", "c1", "public"], PUBLIC);
  render(
    <QueryClientProvider client={qc}>
      <CommandPalette />
    </QueryClientProvider>,
  );
  return qc;
}

const optionTexts = () => screen.queryAllByRole("option").map((o) => o.textContent ?? "");
const type = (value: string) =>
  fireEvent.change(screen.getByRole("combobox"), { target: { value } });

beforeAll(() => {
  if (!Element.prototype.scrollIntoView) {
    Element.prototype.scrollIntoView = function scrollIntoView() {};
  }
});

beforeEach(() => {
  localStorage.removeItem(MRU_STORAGE_KEY);
  localStorage.removeItem(LAZY_STORAGE_KEY);
  resetLazyLoad();
  usePaletteStore.setState({ open: false, mode: "all", session: 0, initialQuery: "" });
  useTabsStore.setState({
    tabs: [],
    activeId: null,
    closed: [],
    queryCounter: 0,
    closeRequest: null,
  });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useConnectionsStore.getState().add(info);
  useConnectionsStore.getState().setCurrentSchema("c1", "public");
  useUiStore.setState({ view: "workspace", themePref: "system" });
  vi.spyOn(ipc.objects, "routines").mockResolvedValue([]);
});

afterEach(() => {
  // The focus poll of a chosen item must not outlive the test (jsdom is gone by then).
  cancelTabSurfaceFocus();
  vi.restoreAllMocks();
});

describe("Search in Database…", () => {
  it("lists every schema's objects from one backend call, starting from the sidebar search", async () => {
    const items = vi.spyOn(ipc.objects, "databaseItems").mockResolvedValue({
      tables: [...PUBLIC, ...INVENTORY],
      routines: [
        {
          schema: "inventory",
          name: "reorder",
          kind: "function",
          arguments: "",
          identityArguments: "",
          returnType: "void",
          language: "sql",
          comment: null,
        },
      ],
    });
    renderPalette();
    act(() => usePaletteStore.getState().openPalette("objects", "ord"));
    expect(screen.getByRole("combobox")).toHaveValue("ord");
    await waitFor(() => expect(optionTexts()).toContain("orderspublictable"));
    expect(items).toHaveBeenCalledWith("c1", false);
    expect(optionTexts()).toEqual(
      expect.arrayContaining(["stock_ordersinventorytable", "reorderinventoryfunction"]),
    );
    // Opening an object of another schema shows that schema in the sidebar.
    // (A double-click opens; a single click only selects.)
    fireEvent.doubleClick(screen.getByRole("option", { name: /^stock_orders/ }));
    await waitFor(() => expect(usePaletteStore.getState().open).toBe(false));
    await waitFor(() => expect(useTabsStore.getState().tabs).toHaveLength(1));
    expect(useTabsStore.getState().tabs[0]).toMatchObject({
      kind: "table",
      table: { schema: "inventory", name: "stock_orders" },
    });
    expect(useConnectionsStore.getState().byId.c1!.currentSchema).toBe("inventory");
  });
});

describe("Search in Database… on MySQL", () => {
  const mysqlInfo: ConnectionInfo = { ...info, driver: "mysql", currentDatabase: null };

  beforeEach(() => {
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
    useConnectionsStore.getState().add(mysqlInfo);
  });

  it("tells same-named tables of different databases apart when the connection has no default one", async () => {
    vi.spyOn(ipc.objects, "databaseItems").mockResolvedValue({
      tables: [table("users", "shop"), table("users", "blog")],
      routines: [],
    });
    renderPalette("mysql");
    act(() => usePaletteStore.getState().openPalette("objects", "users"));
    await waitFor(() => expect(optionTexts()).toEqual(["usersshoptable", "usersblogtable"]));
  });

  it("keeps the row estimate while the result is of a single database", async () => {
    vi.spyOn(ipc.objects, "databaseItems").mockResolvedValue({
      tables: [table("users", "shop"), table("orders", "shop")],
      routines: [],
    });
    renderPalette("mysql");
    act(() => usePaletteStore.getState().openPalette("objects", "s"));
    await waitFor(() => expect(optionTexts()).toHaveLength(2));
    expect(optionTexts()).toEqual(["orders10table", "users10table"]);
  });
});

describe("Open Anything with lazy load", () => {
  it("searches only the loaded schemas, and Load all schemas loads the others in place", async () => {
    const tables = vi
      .spyOn(ipc.schema, "tables")
      .mockImplementation(async (_c, schema) => (schema === "inventory" ? INVENTORY : PUBLIC));
    setLazyLoad("p1", true, null);
    const key = lazyKey("c1", "app");
    initLazyTree(key, ["public"]);
    renderPalette();
    act(() => usePaletteStore.getState().toggle("all"));
    await waitFor(() => expect(optionTexts()).toContain("users10table"));
    expect(screen.getByText(/1 of 2 schemas/)).toBeInTheDocument();

    type("items");
    // Nothing loaded matches; the row that loads the rest stays.
    await waitFor(() => expect(optionTexts()).toEqual(["Load all schemas1 not searched"]));
    expect(tables).not.toHaveBeenCalledWith("c1", "inventory");
    fireEvent.click(screen.getByRole("option", { name: /^Load all schemas/ }));
    // The palette stays open and the list grows.
    expect(usePaletteStore.getState().open).toBe(true);
    await waitFor(() => expect(optionTexts()).toContain("itemsinventorytable"));
    expect(tables).toHaveBeenCalledWith("c1", "inventory");
    expect(lazyTreeOf(key).loaded).toEqual(["public", "inventory"]);
    expect(optionTexts().some((t) => t.startsWith("Load all schemas"))).toBe(false);

    // Opening it expands its node in the sidebar.
    fireEvent.click(screen.getByRole("option", { name: /^items/ }));
    await waitFor(() => expect(lazyTreeOf(key).expanded).toEqual(["public", "inventory"]));
  });
});
