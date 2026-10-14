import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { resetThemesForTests, useThemes } from "@/features/appearance/themesStore";
import { emptyStaging, insertRow } from "@/features/staging/model";
import { useStructureStore } from "@/features/structure/store";
import { useDatabaseSwitchStore } from "@/features/sidebar/switchDatabase";
import { ipc, type ConnectionInfo, type DatabaseInfo, type TableSummary } from "@/lib/ipc";
import { profile } from "@/lib/ipc/mock/fixtures";
import { isMac } from "@/lib/keys/combo";
import { useConnectionsStore } from "@/stores/connections";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { actions } from "./actions";
import { CommandPalette } from "./CommandPalette";
import { MRU_STORAGE_KEY } from "./mru";
import { usePaletteStore, type PaletteMode } from "./store";
import { cancelTabSurfaceFocus } from "./focus";

const mod = isMac ? { metaKey: true } : { ctrlKey: true };

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

const tables: TableSummary[] = ["users", "orders", "order_items", "audit_log"].map((name) => ({
  name,
  schema: "public",
  kind: "table",
  rowEstimate: 10,
  sizeBytes: null,
  comment: null,
}));

const databases: DatabaseInfo[] = ["app", "shop"].map((name) => ({
  name,
  owner: null,
  encoding: "UTF8",
  isCurrent: name === "app",
}));

function renderPalette(
  extra?: ReactNode,
  { mode = "all", setup }: { mode?: PaletteMode; setup?: (qc: QueryClient) => void } = {},
) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  qc.setQueryData(
    ["profiles"],
    [
      profile({ id: "p1", name: "Local PG", driver: "postgres" }),
      profile({ id: "p2", name: "Staging MySQL", driver: "mysql" }),
    ],
  );
  qc.setQueryData(["schemas", "c1"], [{ name: "public", isDefault: true }]);
  qc.setQueryData(["tables", "c1", "public"], tables);
  setup?.(qc);
  render(
    <QueryClientProvider client={qc}>
      {extra}
      <CommandPalette />
    </QueryClientProvider>,
  );
  act(() => usePaletteStore.getState().toggle(mode));
  return screen.getByRole("combobox");
}

/** Two databases ("app" is current) and two schemas on the active connection. */
function withDatabases(qc: QueryClient) {
  qc.setQueryData(["databases", "c1"], databases);
  qc.setQueryData(
    ["schemas", "c1"],
    [
      { name: "public", isDefault: true },
      { name: "inventory", isDefault: false },
    ],
  );
}

const press = (el: HTMLElement, key: string, shift = false) =>
  fireEvent.keyDown(el, { key, code: `Key${key.toUpperCase()}`, ...mod, shiftKey: shift });

function groupLabels(): string[] {
  return Array.from(document.querySelectorAll("[cmdk-group-heading]")).map(
    (h) => h.textContent ?? "",
  );
}

const optionTexts = () => screen.getAllByRole("option").map((o) => o.textContent ?? "");

/** The option labelled `label` whose kind (shown last) is `kind`. */
function option(label: string, kind: string): HTMLElement {
  const found = screen
    .getAllByRole("option")
    .find((o) => o.textContent?.startsWith(label) && o.textContent.endsWith(kind));
  if (!found) throw new Error(`No ${kind} option "${label}" in ${JSON.stringify(optionTexts())}`);
  return found;
}

const type = (value: string) =>
  fireEvent.change(screen.getByRole("combobox"), { target: { value } });

/** A real click: the pointer goes down on the row, then the click arrives. */
const click = (el: HTMLElement, init: object = {}) => {
  fireEvent.pointerDown(el, init);
  fireEvent.click(el, init);
};
const doubleClick = (el: HTMLElement) => {
  click(el);
  click(el);
  fireEvent.doubleClick(el);
};
const tabsOf = () =>
  useTabsStore
    .getState()
    .tabs.filter((t) => t.kind === "table")
    .map((t) => t.id);

beforeAll(() => {
  // jsdom has no layout; cmdk scrolls the selected item into view.
  if (!Element.prototype.scrollIntoView) {
    Element.prototype.scrollIntoView = function scrollIntoView() {};
  }
});

const cleanups: Array<() => void> = [];

beforeEach(() => {
  localStorage.removeItem(MRU_STORAGE_KEY);
  usePaletteStore.setState({ open: false, mode: "all", session: 0 });
  useStructureStore.setState({ byTab: {} });
  useDatabaseSwitchStore.setState({ pending: {} });
  useTabsStore.setState({
    tabs: [],
    activeId: null,
    closed: [],
    queryCounter: 0,
    closeRequest: null,
  });
  useTableTabsStore.setState({ slices: {} });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useConnectionsStore.getState().add(info);
  useUiStore.setState({ view: "workspace", themePref: "system" });
});

afterEach(() => {
  // The focus poll of a chosen item must not outlive the test (jsdom is gone by then).
  cancelTabSurfaceFocus();
  cleanups.splice(0).forEach((c) => c());
  vi.restoreAllMocks();
});

/** Stand-in for TabContent: only the active tab's grid is mounted (keyed by tab id). */
function FakeTabContent() {
  const activeId = useTabsStore((s) => s.activeId);
  return activeId ? (
    <div key={activeId} data-scope="grid" tabIndex={0} data-testid={`grid-${activeId}`} />
  ) : null;
}

/**
 * jsdom has no CSS animations. Gives the dialog content and overlay an exit animation while
 * closed, so Radix keeps them mounted until `animationend` like in the browser.
 */
function simulateDialogAnimations() {
  const real = window.getComputedStyle.bind(window);
  vi.spyOn(window, "getComputedStyle").mockImplementation((el, pseudo) => {
    const style = real(el, pseudo);
    const slot = el instanceof HTMLElement ? el.dataset.slot : undefined;
    if (slot !== "dialog-content" && slot !== "dialog-overlay") return style;
    return new Proxy(style, {
      get(target, prop) {
        if (prop === "animationName")
          return (el as HTMLElement).dataset.state === "closed" ? "exit" : "enter";
        const value: unknown = Reflect.get(target, prop);
        return typeof value === "function" ? (value as () => unknown).bind(target) : value;
      },
    });
  });
  if (!globalThis.CSS?.escape) {
    vi.stubGlobal("CSS", { ...globalThis.CSS, escape: (s: string) => s });
  }
  return () => {
    for (const el of document.querySelectorAll<HTMLElement>(
      "[data-slot=dialog-content], [data-slot=dialog-overlay]",
    )) {
      act(() => {
        el.dispatchEvent(Object.assign(new Event("animationend"), { animationName: "exit" }));
      });
    }
  };
}

function stageEdits(tabId: string, n: number) {
  let staging = emptyStaging();
  for (let i = 0; i < n; i++) staging = insertRow(staging, `new:${i}`, []);
  useTableTabsStore.getState().update(tabId, { staging });
}

describe("CommandPalette", () => {
  it("lists only the schema's tables for an empty query: flat, alphabetical, with their kind", () => {
    useTabsStore.getState().newQuery("c1");
    cleanups.push(actions.register("ui.toggle-sidebar", () => {}));
    const input = renderPalette(undefined, { setup: withDatabases });
    expect(input).toHaveAttribute(
      "placeholder",
      "Search tables, views, databases, connections and commands…",
    );
    expect(groupLabels()).toEqual([]);
    expect(optionTexts()).toEqual([
      expect.stringMatching(/^audit_log.*table$/),
      expect.stringMatching(/^order_items.*table$/),
      expect.stringMatching(/^orders.*table$/),
      expect.stringMatching(/^users.*table$/),
    ]);
    // The row estimate stays, muted.
    expect(within(option("users", "table")).getByText("10")).toBeInTheDocument();
    expect(screen.getByTestId("palette-command-hint")).toHaveTextContent("Type > for all commands");
    // The header tells whose tables these are.
    expect(screen.getByText("· public")).toBeInTheDocument();
  });

  it("ranks tables, tabs, schemas, databases and connections in one flat list", () => {
    useTabsStore.getState().newQuery("c1");
    cleanups.push(actions.register("ui.toggle-sidebar", () => {}));
    renderPalette(undefined, { setup: withDatabases });
    type("app");
    // The objects come first, without a heading; the commands that match ("appearance" is a
    // keyword of the themes) follow under their own.
    expect(groupLabels()).toEqual(["Commands"]);
    const texts = optionTexts();
    expect(texts[0]).toMatch(/^app.*database$/);
    expect(texts.slice(1).every((t) => t.startsWith("Theme: "))).toBe(true);
    expect(within(option("app", "database")).getByLabelText("Current")).toBeInTheDocument();
    type("inv");
    expect(optionTexts()).toEqual([expect.stringMatching(/^inventory.*schema$/)]);
    type("query");
    expect(optionTexts()).toEqual([expect.stringMatching(/^Query 1.*tab$/)]);
    // Prefix hits first, then contiguous ones; ties alphabetically whatever their kind.
    type("o");
    expect(optionTexts()).toEqual([
      expect.stringMatching(/^order_items.*table$/),
      expect.stringMatching(/^orders.*table$/),
      expect.stringMatching(/^audit_log.*table$/),
      expect.stringMatching(/^inventory.*schema$/),
      expect.stringMatching(/^Local PG.*connection$/),
      expect.stringMatching(/^shop.*database$/),
    ]);
    type("stm");
    expect(optionTexts()).toEqual([expect.stringMatching(/^Staging MySQL.*connection$/)]);
    // Nothing but a command matches "sidebar": it comes under "Commands".
    type("sidebar");
    expect(groupLabels()).toEqual(["Commands"]);
    expect(optionTexts()).toEqual([expect.stringMatching(/^Toggle sidebar/)]);
  });

  it("lists a few matching commands after the objects, every command after >", () => {
    cleanups.push(
      actions.register("ui.toggle-sidebar", () => {}),
      actions.register("grid.duplicate-row", () => {}),
    );
    renderPalette();
    // One character: objects only (no command).
    type("d");
    expect(groupLabels()).toEqual([]);
    expect(screen.queryByRole("option", { name: /Duplicate row/ })).toBeNull();
    // Two characters: the matching commands follow the objects, each with its shortcut.
    type("du");
    expect(groupLabels()).toEqual(["Commands"]);
    expect(optionTexts()).toEqual([expect.stringMatching(/^Duplicate row/)]);
    type("sidebar");
    const hit = screen.getByRole("option", { name: /Toggle sidebar/ });
    expect(within(hit).getByText(isMac ? "⌘0" : "Ctrl+0")).toBeInTheDocument();
    // An object of that name stays first.
    type("users");
    expect(groupLabels()).toEqual([]);
    type("> du");
    expect(optionTexts()).toEqual([expect.stringMatching(/^Duplicate row/)]);
    // No kinds, no "Type >" hint among commands.
    expect(screen.queryByTestId("palette-command-hint")).toBeNull();
    type(">");
    const toggle = screen.getByRole("option", { name: /Toggle sidebar/ });
    expect(within(toggle).getByText(isMac ? "⌘0" : "Ctrl+0")).toBeInTheDocument();
    // …and actions without a handler are hidden.
    expect(screen.queryByRole("option", { name: /Commit changes/ })).toBeNull();
    expect(screen.queryByRole("option", { name: /Open anything/ })).toBeNull();
    expect(screen.getByRole("option", { name: /Theme: Dark/ })).toBeInTheDocument();
  });

  it("matches strictly and highlights matched characters", () => {
    const input = renderPalette();
    fireEvent.change(input, { target: { value: "ord" } });
    expect(groupLabels()).toEqual([]);
    expect(optionTexts()).toEqual([
      expect.stringMatching(/^order_items/),
      expect.stringMatching(/^orders/),
    ]);
    const first = screen.getAllByRole("option")[0]!;
    expect(first).toHaveAttribute("aria-selected", "true");
    expect(first.querySelector("mark")?.textContent).toBe("ord");
    // Scattered characters do not match ("ua" is in "users" and "audit_log" only as a subsequence).
    fireEvent.change(input, { target: { value: "ua" } });
    expect(screen.queryAllByRole("option")).toHaveLength(0);

    fireEvent.change(input, { target: { value: "qqqq" } });
    expect(screen.queryAllByRole("option")).toHaveLength(0);
    expect(screen.getByText(/No results for/)).toBeInTheDocument();
  });

  it("completes the selected name inline and accepts it with Tab or →", () => {
    const input = renderPalette();
    type("aud");
    expect(screen.getByTestId("palette-completion")).toHaveTextContent("audit_log");
    expect(screen.getByTestId("palette-completion").lastChild).toHaveTextContent(/^it_log$/);
    fireEvent.keyDown(input, { key: "Tab", code: "Tab" });
    expect(input).toHaveValue("audit_log");
    expect(screen.queryByTestId("palette-completion")).toBeNull();

    type("us");
    (input as HTMLInputElement).setSelectionRange(2, 2);
    fireEvent.keyDown(input, { key: "ArrowRight", code: "ArrowRight" });
    expect(input).toHaveValue("users");
    // Only prefixes complete; commands never do.
    type("dit");
    expect(screen.queryByTestId("palette-completion")).toBeNull();
    type(">");
    expect(screen.queryByTestId("palette-completion")).toBeNull();
  });

  it("lists the saved connections when no connection is open", () => {
    useConnectionsStore.getState().remove("c1");
    renderPalette();
    expect(optionTexts()).toEqual([
      expect.stringMatching(/^Local PG.*connection$/),
      expect.stringMatching(/^Staging MySQL.*connection$/),
    ]);
  });

  it("shows the loading placeholder inline while the tables load", () => {
    vi.spyOn(ipc.schema, "tables").mockReturnValue(new Promise(() => {}));
    renderPalette(undefined, {
      setup: (qc) => qc.removeQueries({ queryKey: ["tables", "c1", "public"] }),
    });
    expect(groupLabels()).toEqual([]);
    expect(screen.getByText("Loading tables…")).toBeInTheDocument();
    type("stag");
    expect(optionTexts()).toEqual([expect.stringMatching(/^Staging MySQL/)]);
    expect(screen.getByText("Loading tables…")).toBeInTheDocument();
  });

  describe("a click selects, Return or a double-click opens, ⌘ opens in a new tab", () => {
    const orders = { schema: "public", name: "orders" };

    it("does not open a table on a single click; a double-click and Return do", async () => {
      renderPalette();
      type("orders");
      const row = option("orders", "table");
      click(row);
      // Still open, nothing opened, the row is the selected one.
      expect(usePaletteStore.getState().open).toBe(true);
      expect(useTabsStore.getState().tabs).toHaveLength(0);
      expect(row).toHaveAttribute("aria-selected", "true");
      click(row);
      expect(useTabsStore.getState().tabs).toHaveLength(0);

      doubleClick(row);
      await waitFor(() => expect(useTabsStore.getState().tabs).toHaveLength(1));
      expect(useTabsStore.getState().tabs[0]).toMatchObject({ kind: "table", title: "orders" });
      expect(usePaletteStore.getState().open).toBe(false);

      // Return opens as before.
      act(() => usePaletteStore.getState().setOpen(true));
      const input = screen.getByRole("combobox");
      fireEvent.change(input, { target: { value: "users" } });
      fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
      await waitFor(() =>
        expect(useTabsStore.getState().tabs.some((t) => t.title === "users")).toBe(true),
      );
    });

    it("a click moves the selection to the row it lands on", () => {
      renderPalette();
      type("or");
      const rows = screen.getAllByRole("option");
      expect(rows.length).toBeGreaterThan(1);
      const last = rows[rows.length - 1]!;
      expect(last).toHaveAttribute("aria-selected", "false");
      click(last);
      expect(last).toHaveAttribute("aria-selected", "true");
      expect(useTabsStore.getState().tabs).toHaveLength(0);
    });

    it("⌘-click opens the table in a new tab at once, even when it is open already", async () => {
      useTabsStore.getState().openTable("c1", orders);
      renderPalette();
      type("orders");
      expect(tabsOf()).toHaveLength(1);
      click(option("orders", "table"), mod);
      await waitFor(() => expect(tabsOf()).toHaveLength(2));
      const [first, second] = useTabsStore.getState().tabs;
      expect(second!.id).not.toBe(first!.id);
      expect(second).toMatchObject({ kind: "table", table: orders });
      expect(second).not.toHaveProperty("preview");
      expect(usePaletteStore.getState().open).toBe(false);
    });

    it("⌘↩ opens another tab of an open table; ↩ shows the one that is open", async () => {
      const first = useTabsStore.getState().openTable("c1", orders);
      useTabsStore.getState().newQuery("c1");
      renderPalette();
      const pick = async (alt: boolean) => {
        act(() => usePaletteStore.getState().setOpen(true));
        const input = screen.getByRole("combobox");
        fireEvent.change(input, { target: { value: "orders" } });
        fireEvent.keyDown(input, { key: "Enter", code: "Enter", ...(alt ? mod : {}) });
        await waitFor(() => expect(usePaletteStore.getState().open).toBe(false));
      };
      // Plain ↩: the table's own tab is activated, no second one appears.
      await pick(false);
      await waitFor(() => expect(useTabsStore.getState().activeId).toBe(first));
      expect(tabsOf()).toEqual([first]);
      // ⌘↩: another tab, every time.
      await pick(true);
      await waitFor(() => expect(tabsOf()).toHaveLength(2));
      await pick(true);
      await waitFor(() => expect(tabsOf()).toHaveLength(3));
      expect(new Set(tabsOf()).size).toBe(3);
      expect(useTabsStore.getState().tabs.filter((t) => t.kind === "table")).toEqual(
        Array(3).fill(expect.objectContaining({ table: orders })),
      );
    });

    it("Search in Database… follows the same rules", async () => {
      vi.spyOn(ipc.objects, "databaseItems").mockResolvedValue({
        tables: tables.slice(0, 2),
        routines: [],
      });
      renderPalette(undefined, { mode: "objects" });
      type("orders");
      const row = await waitFor(() => option("orders", "table"));
      click(row);
      expect(usePaletteStore.getState().open).toBe(true);
      expect(useTabsStore.getState().tabs).toHaveLength(0);
      doubleClick(option("orders", "table"));
      await waitFor(() => expect(tabsOf()).toHaveLength(1));
      // ⌘-click: another tab of the same table.
      act(() => usePaletteStore.getState().openPalette("objects", "orders"));
      click(await waitFor(() => option("orders", "table")), mod);
      await waitFor(() => expect(tabsOf()).toHaveLength(2));
    });

    it("Switch connection: a click only selects, a double-click switches", async () => {
      useConnectionsStore.getState().add({
        ...info,
        connectionId: "c2",
        profileId: "p2",
        name: "Staging MySQL",
        driver: "mysql",
      });
      useConnectionsStore.getState().setActive("c1");
      renderPalette(undefined, { mode: "connections" });
      const row = screen.getByRole("option", { name: /^Staging MySQL/ });
      click(row);
      expect(usePaletteStore.getState().open).toBe(true);
      expect(useConnectionsStore.getState().activeId).toBe("c1");
      expect(row).toHaveAttribute("aria-selected", "true");
      doubleClick(row);
      await waitFor(() => expect(useConnectionsStore.getState().activeId).toBe("c2"));
      expect(usePaletteStore.getState().open).toBe(false);
    });

    it("Switch database keeps its click rules", async () => {
      renderPalette(undefined, { mode: "databases", setup: withDatabases });
      const row = screen.getByRole("option", { name: /^inventory/ });
      click(row);
      expect(usePaletteStore.getState().open).toBe(true);
      expect(useConnectionsStore.getState().byId.c1!.currentSchema).not.toBe("inventory");
      doubleClick(row);
      await waitFor(() =>
        expect(useConnectionsStore.getState().byId.c1!.currentSchema).toBe("inventory"),
      );
    });

    it("a command row opens on a double-click only", async () => {
      const run = vi.fn();
      cleanups.push(actions.register("ui.toggle-sidebar", run));
      renderPalette();
      type("> toggle sidebar");
      const row = screen.getByRole("option", { name: /Toggle sidebar/ });
      click(row);
      expect(run).not.toHaveBeenCalled();
      expect(usePaletteStore.getState().open).toBe(true);
      doubleClick(row);
      await waitFor(() => expect(run).toHaveBeenCalledTimes(1));
    });
  });

  it("opens a table in a new tab on ⌘-click", async () => {
    renderPalette();
    type("orders");
    const row = option("orders", "table");
    fireEvent.pointerDown(row, mod);
    fireEvent.click(row, mod);
    await waitFor(() => expect(useTabsStore.getState().tabs).toHaveLength(1));
    expect(useTabsStore.getState().tabs[0]).toMatchObject({ title: "orders" });
    expect(useTabsStore.getState().tabs[0]).not.toHaveProperty("preview");
  });

  it("opens the data tab on Enter after closing, and records it as recent", async () => {
    const input = renderPalette();
    fireEvent.change(input, { target: { value: "orders" } });
    fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
    await waitFor(() => expect(useTabsStore.getState().tabs).toHaveLength(1));
    expect(usePaletteStore.getState().open).toBe(false);
    expect(useTabsStore.getState().tabs[0]).toMatchObject({
      kind: "table",
      connectionId: "c1",
      table: { schema: "public", name: "orders" },
    });
    expect(JSON.parse(localStorage.getItem(MRU_STORAGE_KEY)!)).toEqual(["table:p1:public:orders"]);
  });

  it("opens tables in the preview tab on Enter, and in a new tab on ⌘Enter", async () => {
    const pick = async (name: string, alt = false) => {
      act(() => usePaletteStore.getState().setOpen(true));
      const input = screen.getByRole("combobox");
      fireEvent.change(input, { target: { value: name } });
      fireEvent.keyDown(input, { key: "Enter", code: "Enter", ...(alt ? mod : {}) });
      await waitFor(() => expect(usePaletteStore.getState().open).toBe(false));
      await waitFor(() =>
        expect(useTabsStore.getState().tabs.some((t) => t.title === name)).toBe(true),
      );
    };
    renderPalette();
    act(() => usePaletteStore.getState().setOpen(false));
    const titles = () =>
      useTabsStore.getState().tabs.map((t) => [t.title, t.kind !== "query" && t.preview === true]);

    await pick("orders");
    expect(titles()).toEqual([["orders", true]]);
    // The next one replaces the preview tab.
    await pick("users");
    expect(titles()).toEqual([["users", true]]);
    await pick("audit_log", true);
    expect(titles()).toEqual([
      ["users", true],
      ["audit_log", false],
    ]);
    expect(useTabsStore.getState().tabs.every((t) => t.kind === "table")).toBe(true);
  });

  it("lists recently opened tables first when the query is empty, and nothing else", () => {
    localStorage.setItem(
      MRU_STORAGE_KEY,
      JSON.stringify([
        "cmd:theme.dark",
        "table:p1:public:users",
        "conn:p2",
        "table:p1:public:orders",
      ]),
    );
    renderPalette();
    expect(groupLabels()).toEqual([]);
    expect(optionTexts()).toEqual([
      expect.stringMatching(/^users/),
      expect.stringMatching(/^orders/),
      expect.stringMatching(/^audit_log/),
      expect.stringMatching(/^order_items/),
    ]);
    // Recently used commands lead the > list.
    type(">");
    expect(optionTexts()[0]).toMatch(/^Theme: Dark/);
  });

  it("runs actions once the dialog has closed", async () => {
    let ran = 0;
    cleanups.push(actions.register("ui.toggle-inspector", () => void ran++));
    const input = renderPalette();
    fireEvent.change(input, { target: { value: "> inspector" } });
    fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
    expect(ran).toBe(0);
    await waitFor(() => expect(ran).toBe(1));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("closes on ⌘P and on Escape", () => {
    const input = renderPalette();
    press(input, "p");
    expect(usePaletteStore.getState().open).toBe(false);

    act(() => usePaletteStore.getState().setOpen(true));
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "ord" } });
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Escape", code: "Escape" });
    expect(usePaletteStore.getState().open).toBe(false);

    // Every opening starts from scratch.
    act(() => usePaletteStore.getState().setOpen(true));
    expect(screen.getByRole("combobox")).toHaveValue("");
  });

  it("gives focus back to the element focused before opening", async () => {
    const grid = document.createElement("div");
    grid.tabIndex = 0;
    document.body.appendChild(grid);
    cleanups.push(() => grid.remove());
    grid.focus();
    const input = renderPalette();
    await waitFor(() => expect(input).toHaveFocus());
    fireEvent.keyDown(input, { key: "Escape", code: "Escape" });
    await waitFor(() => expect(grid).toHaveFocus());

    // Commands run after focus is restored, so they see the grid as the active element.
    let focusedWhenRun: Element | null = null;
    cleanups.push(
      actions.register("ui.toggle-inspector", () => {
        focusedWhenRun = document.activeElement;
      }),
    );
    act(() => usePaletteStore.getState().setOpen(true));
    const again = screen.getByRole("combobox");
    fireEvent.change(again, { target: { value: "> inspector" } });
    fireEvent.keyDown(again, { key: "Enter", code: "Enter" });
    await waitFor(() => expect(focusedWhenRun).toBe(grid));
  });

  it("switches theme from the palette", async () => {
    const input = renderPalette();
    fireEvent.change(input, { target: { value: "> theme dark" } });
    fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
    await waitFor(() => expect(useUiStore.getState().themePref).toBe("dark"));
  });

  describe("theme commands", () => {
    afterEach(() => resetThemesForTests());
    const themeOptions = () =>
      optionTexts()
        .filter((t) => t.startsWith("Theme:"))
        .map((t) => t.replace(/Apply.*$/, ""));

    it("lists every theme: Auto, Light, Dark, both Solarized and each custom theme", () => {
      useThemes.getState().add({ name: "Midnight", base: "dark", tokens: {} });
      renderPalette();
      type("> theme");
      const listed = themeOptions();
      for (const name of ["Auto", "Light", "Dark", "Solarized Light", "Solarized Dark", "Midnight"])
        expect(listed.some((t) => t.startsWith(`Theme: ${name}`))).toBe(true);
    });

    it("applies Solarized Dark, which a plain 'dark' search does not pick", async () => {
      const input = renderPalette();
      fireEvent.change(input, { target: { value: "> theme solarized dark" } });
      fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
      await waitFor(() => expect(useUiStore.getState().themePref).toBe("solarized-dark"));
    });

    it("applies a custom theme by its name, and follows the themes being added", async () => {
      const input = renderPalette();
      fireEvent.change(input, { target: { value: "> midnight" } });
      expect(
        screen.queryAllByRole("option").some((o) => /Midnight/.test(o.textContent ?? "")),
      ).toBe(false);
      let id = "";
      act(() => {
        id = useThemes.getState().add({ name: "Midnight", base: "dark", tokens: {} })!.id;
      });
      fireEvent.change(input, { target: { value: "> theme midnight" } });
      fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
      await waitFor(() => expect(useUiStore.getState().themePref).toBe(`custom:${id}`));
    });

    it("marks the theme in use, a custom one included", () => {
      const theme = useThemes.getState().add({ name: "Midnight", base: "dark", tokens: {} })!;
      useUiStore.setState({ themePref: `custom:${theme.id}` });
      renderPalette();
      type("> theme");
      const current = screen
        .getAllByRole("option")
        .filter(
          (o) => /^Theme:/.test(o.textContent ?? "") && within(o).queryByLabelText("Current"),
        );
      expect(current.map((o) => o.textContent)).toEqual([expect.stringMatching(/Midnight/)]);
    });
  });

  it("scrolls back to the top when the query changes, where the selected best match is", () => {
    const input = renderPalette();
    const list = document.querySelector<HTMLElement>("[cmdk-list]")!;
    let scrollTop = 0;
    Object.defineProperty(list, "scrollTop", {
      configurable: true,
      get: () => scrollTop,
      set: (v: number) => void (scrollTop = v),
    });
    scrollTop = 1000;
    fireEvent.change(input, { target: { value: "e" } });
    expect(screen.getAllByRole("option")[0]).toHaveAttribute("aria-selected", "true");
    expect(scrollTop).toBe(0);
  });

  it("marks query tabs with unsaved SQL as dirty", () => {
    const id = useTabsStore.getState().newQuery("c1");
    useTabsStore.getState().setDirty(id, true);
    renderPalette();
    type("query");
    const row = option("Query 1", "tab");
    expect(within(row).getByLabelText("Unsaved changes")).toBeInTheDocument();
  });

  it("marks table tabs dirty for staged rows and for staged structure changes alike", () => {
    const tabs = useTabsStore.getState();
    const orders = tabs.openTable("c1", { schema: "public", name: "orders" });
    const users = tabs.openTable("c1", { schema: "public", name: "users" }, "structure");
    tabs.openTable("c1", { schema: "public", name: "audit_log" });
    // The table tab shows its rows, but holds staged DDL from its structure view.
    useStructureStore
      .getState()
      .stage(orders, () => [
        { op: "dropColumn", table: { schema: "public", name: "orders" }, name: "note" },
      ]);
    stageEdits(users, 1);
    renderPalette();
    const dirty = (name: string) => {
      type(name);
      return within(option(name, "tab")).queryByLabelText("Unsaved changes") !== null;
    };
    expect(dirty("orders")).toBe(true);
    expect(dirty("audit_log")).toBe(false);
    expect(dirty("users")).toBe(true);
    // The table rows themselves carry no dirty marker.
    expect(within(option("users", "table")).queryByLabelText("Unsaved changes")).toBeNull();
  });

  it("highlights matches with a colour that stays legible on every accent and theme", () => {
    const input = renderPalette();
    fireEvent.change(input, { target: { value: "ord" } });
    const mark = screen.getAllByRole("option")[0]!.querySelector("mark")!;
    expect(mark.className.split(/\s+/)).not.toContain("text-brand");
    expect(mark.className).toContain("color-mix(in_oklab,var(--brand)_50%,var(--foreground))");
  });

  it("asks before Disconnect drops staged edits, and never makes it a Recent default", async () => {
    const tab = useTabsStore.getState().openTable("c1", { schema: "public", name: "orders" });
    stageEdits(tab, 2);
    const input = renderPalette();
    fireEvent.change(input, { target: { value: "> disconnect" } });
    expect(screen.getAllByRole("option")[0]).toHaveTextContent(/^Disconnect/);
    fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
    await waitFor(() => expect(useTabsStore.getState().closeRequest).not.toBeNull());
    // Nothing is dropped until the user confirms.
    expect(useConnectionsStore.getState().byId.c1).toBeDefined();
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual([tab]);
    expect(useTableTabsStore.getState().slices[tab]?.staging.changes.size).toBe(2);
    expect(localStorage.getItem(MRU_STORAGE_KEY)).toBeNull();

    act(() => useTabsStore.getState().cancelClose());
    act(() => usePaletteStore.getState().setOpen(true));
    type(">");
    expect(screen.getAllByRole("option")[0]).not.toHaveTextContent(/^Disconnect/);
  });

  it("disconnects straight away when nothing is unsaved", async () => {
    useTabsStore.getState().openTable("c1", { schema: "public", name: "orders" });
    const input = renderPalette();
    fireEvent.change(input, { target: { value: "> disconnect" } });
    fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
    await waitFor(() => expect(useConnectionsStore.getState().byId.c1).toBeUndefined());
    expect(useTabsStore.getState().tabs).toEqual([]);
    expect(useTabsStore.getState().closeRequest).toBeNull();
  });

  it("moves focus into the grid of the tab a command switched to", async () => {
    const tabs = useTabsStore.getState();
    const users = tabs.openTable("c1", { schema: "public", name: "users" });
    const orders = tabs.openTable("c1", { schema: "public", name: "orders" });
    const input = renderPalette(<FakeTabContent />);
    act(() => usePaletteStore.getState().setOpen(false));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    screen.getByTestId(`grid-${orders}`).focus();

    act(() => usePaletteStore.getState().setOpen(true));
    const again = screen.getByRole("combobox");
    expect(again).not.toBe(input);
    fireEvent.change(again, { target: { value: "users" } });
    fireEvent.keyDown(again, { key: "Enter", code: "Enter" });
    await waitFor(() => expect(useTabsStore.getState().activeId).toBe(users));
    await waitFor(() => expect(screen.getByTestId(`grid-${users}`)).toHaveFocus());
  });

  describe("modes", () => {
    it("⌘K lists only the databases and schemas of the active connection", () => {
      useTabsStore.getState().newQuery("c1");
      const input = renderPalette(undefined, { mode: "databases", setup: withDatabases });
      expect(input).toHaveAttribute("placeholder", "Switch database…");
      expect(groupLabels()).toEqual(["Databases", "Schemas"]);
      expect(optionTexts()).toEqual([
        expect.stringMatching(/^app/),
        expect.stringMatching(/^shop/),
        expect.stringMatching(/^New Database…/),
        expect.stringMatching(/^public.*default/),
        expect.stringMatching(/^inventory/),
        expect.stringMatching(/^New Schema…/),
      ]);
      expect(screen.getByText("Switch")).toBeInTheDocument();
      fireEvent.change(input, { target: { value: "inv" } });
      expect(optionTexts()).toEqual([expect.stringMatching(/^inventory/)]);
    });

    it("opens another database in a new workspace (TablePlus)", async () => {
      const tab = useTabsStore.getState().openTable("c1", { schema: "public", name: "orders" });
      const spy = vi
        .spyOn(ipc.connections, "connect")
        .mockResolvedValue({ ...info, connectionId: "c9", currentDatabase: "shop" });
      const input = renderPalette(undefined, { mode: "databases", setup: withDatabases });
      fireEvent.change(input, { target: { value: "shop" } });
      fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
      await waitFor(() => expect(useConnectionsStore.getState().activeId).toBe("c9"));
      expect(spy).toHaveBeenCalledWith("p1", null, "shop");
      // The first workspace keeps its database and its tabs.
      expect(useConnectionsStore.getState().byId.c1!.info.currentDatabase).toBe("app");
      expect(useTabsStore.getState().tabs.some((t) => t.id === tab)).toBe(true);
      // Opening a database never becomes a Recent default.
      expect(localStorage.getItem(MRU_STORAGE_KEY)).toBeNull();
    });

    it("shows another schema in the sidebar", async () => {
      const input = renderPalette(undefined, { mode: "databases", setup: withDatabases });
      fireEvent.change(input, { target: { value: "inventory" } });
      fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
      await waitFor(() =>
        expect(useConnectionsStore.getState().byId.c1!.currentSchema).toBe("inventory"),
      );
    });

    it("⇧⌘K lists only the connections", () => {
      useTabsStore.getState().newQuery("c1");
      const input = renderPalette(undefined, { mode: "connections", setup: withDatabases });
      expect(input).toHaveAttribute("placeholder", "Switch connection…");
      expect(groupLabels()).toEqual(["Connections"]);
      expect(optionTexts()).toEqual([
        expect.stringMatching(/^Local PG/),
        expect.stringMatching(/^Staging MySQL/),
      ]);
    });

    it("leaves Recent out of the narrowed lists", () => {
      localStorage.setItem(MRU_STORAGE_KEY, JSON.stringify(["conn:p2", "schema:p1:app:inventory"]));
      renderPalette(undefined, { mode: "connections", setup: withDatabases });
      expect(groupLabels()).toEqual(["Connections"]);
      act(() => usePaletteStore.getState().toggle("databases"));
      expect(groupLabels()).toEqual(["Databases", "Schemas"]);
    });

    it("switches modes with ⌘P / ⌘K / ⇧⌘K inside the palette and closes on the same combo", () => {
      const input = renderPalette(undefined, { setup: withDatabases });
      press(input, "k");
      expect(usePaletteStore.getState()).toMatchObject({ open: true, mode: "databases" });
      expect(screen.getByRole("combobox")).toHaveAttribute("placeholder", "Switch database…");
      press(screen.getByRole("combobox"), "k", true);
      expect(usePaletteStore.getState()).toMatchObject({ open: true, mode: "connections" });
      expect(groupLabels()).toEqual(["Connections"]);
      press(screen.getByRole("combobox"), "p");
      expect(usePaletteStore.getState()).toMatchObject({ open: true, mode: "all" });
      expect(groupLabels()).toEqual([]);
      expect(optionTexts()[0]).toMatch(/^audit_log.*table$/);
      press(screen.getByRole("combobox"), "p");
      expect(usePaletteStore.getState().open).toBe(false);
    });

    it("lists the connections on ⌘K without a connection", () => {
      const input = renderPalette();
      act(() => useConnectionsStore.getState().remove("c1"));
      press(input, "k");
      expect(usePaletteStore.getState()).toMatchObject({ open: true, mode: "connections" });
    });

    it("shows a placeholder while the databases load", () => {
      vi.spyOn(ipc.schema, "databases").mockReturnValue(new Promise(() => {}));
      renderPalette(undefined, { mode: "databases" });
      // The schemas are in already: only their "+" row (one schema is no choice to list).
      expect(groupLabels()).toEqual(["Databases", "Schemas"]);
      expect(screen.getByText("Loading databases…")).toBeInTheDocument();
      expect(screen.queryByText("New Database…")).toBeNull();
    });
  });

  describe("reopened during the close animation", () => {
    it("runs the earlier choice at once instead of behind the reopened palette", async () => {
      const endAnimations = simulateDialogAnimations();
      const grid = document.createElement("div");
      grid.tabIndex = 0;
      document.body.appendChild(grid);
      cleanups.push(() => grid.remove());
      grid.focus();
      const ran: Array<Element | null> = [];
      cleanups.push(
        actions.register("ui.toggle-inspector", () => void ran.push(document.activeElement)),
      );
      const input = renderPalette();
      fireEvent.change(input, { target: { value: "> inspector" } });
      fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
      expect(usePaletteStore.getState().open).toBe(false);
      // Still animating out: the content is mounted and keeps focus.
      expect(document.querySelector("[data-slot=dialog-content]")).toHaveAttribute(
        "data-state",
        "closed",
      );
      press(input, "p");
      expect(usePaletteStore.getState().open).toBe(true);
      // Ran right away, from where focus was before the palette opened.
      expect(ran).toEqual([grid]);

      await new Promise((r) => setTimeout(r, 600));
      expect(ran).toHaveLength(1);
      endAnimations();
    });

    it("runs both choices when a second one is picked right away", async () => {
      const endAnimations = simulateDialogAnimations();
      const ran: string[] = [];
      cleanups.push(
        actions.register("ui.toggle-inspector", () => void ran.push("inspector")),
        actions.register("ui.toggle-sidebar", () => void ran.push("sidebar")),
      );
      const input = renderPalette();
      fireEvent.change(input, { target: { value: "> inspector" } });
      fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
      press(input, "p");
      const again = screen.getByRole("combobox");
      fireEvent.change(again, { target: { value: "> sidebar" } });
      fireEvent.keyDown(again, { key: "Enter", code: "Enter" });
      endAnimations();
      await waitFor(() => expect(ran).toEqual(["inspector", "sidebar"]));
      await new Promise((r) => setTimeout(r, 600));
      expect(ran).toEqual(["inspector", "sidebar"]);
    });
  });
});
