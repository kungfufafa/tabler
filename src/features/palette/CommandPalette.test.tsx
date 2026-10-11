import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { emptyStaging, insertRow } from "@/features/staging/model";
import type { ConnectionInfo, TableSummary } from "@/lib/ipc";
import { profile } from "@/lib/ipc/mock/fixtures";
import { isMac } from "@/lib/keys/combo";
import { useConnectionsStore } from "@/stores/connections";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { actions } from "./actions";
import { CommandPalette } from "./CommandPalette";
import { MRU_STORAGE_KEY } from "./mru";
import { usePaletteStore } from "./store";

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

function renderPalette(extra?: ReactNode) {
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
  render(
    <QueryClientProvider client={qc}>
      {extra}
      <CommandPalette />
    </QueryClientProvider>,
  );
  act(() => usePaletteStore.getState().setOpen(true));
  return screen.getByRole("combobox");
}

function groupLabels(): string[] {
  return Array.from(document.querySelectorAll("[cmdk-group-heading]")).map(
    (h) => h.textContent ?? "",
  );
}

const optionTexts = () => screen.getAllByRole("option").map((o) => o.textContent ?? "");

beforeAll(() => {
  // jsdom has no layout; cmdk scrolls the selected item into view.
  if (!Element.prototype.scrollIntoView) {
    Element.prototype.scrollIntoView = function scrollIntoView() {};
  }
});

const cleanups: Array<() => void> = [];

beforeEach(() => {
  localStorage.removeItem(MRU_STORAGE_KEY);
  usePaletteStore.setState({ open: false, session: 0 });
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
  it("lists tables, open tabs, connections and actions in that order", () => {
    useTabsStore.getState().newQuery("c1");
    cleanups.push(actions.register("ui.toggle-sidebar", () => {}));
    renderPalette();
    expect(groupLabels()).toEqual(["Tables· public", "Open tabs", "Connections", "Actions"]);
    const texts = optionTexts();
    expect(texts.some((t) => t.startsWith("users"))).toBe(true);
    expect(texts.some((t) => t.startsWith("Query 1"))).toBe(true);
    expect(texts.some((t) => t.startsWith("Staging MySQL"))).toBe(true);
    // Shortcut is rendered next to actions that have a handler…
    const toggle = screen.getByRole("option", { name: /Toggle sidebar/ });
    expect(within(toggle).getByText(isMac ? "⌘B" : "Ctrl+B")).toBeInTheDocument();
    // …and actions without one are hidden.
    expect(screen.queryByRole("option", { name: /Commit changes/ })).toBeNull();
    expect(screen.queryByRole("option", { name: /Command palette/ })).toBeNull();
  });

  it("filters fuzzily and highlights matched characters", () => {
    const input = renderPalette();
    fireEvent.change(input, { target: { value: "ord" } });
    expect(groupLabels()).toEqual(["Tables· public"]);
    expect(optionTexts()[0]).toMatch(/^orders/);
    const first = screen.getAllByRole("option")[0]!;
    expect(first).toHaveAttribute("aria-selected", "true");
    expect(first.querySelector("mark")?.textContent).toBe("ord");

    fireEvent.change(input, { target: { value: "qqqq" } });
    expect(screen.queryAllByRole("option")).toHaveLength(0);
    expect(screen.getByText(/No results for/)).toBeInTheDocument();
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

  it("opens the structure tab on ⌘Enter", async () => {
    const input = renderPalette();
    fireEvent.change(input, { target: { value: "users" } });
    fireEvent.keyDown(input, { key: "Enter", code: "Enter", ...mod });
    await waitFor(() => expect(useTabsStore.getState().tabs).toHaveLength(1));
    expect(useTabsStore.getState().tabs[0]).toMatchObject({
      kind: "structure",
      table: { name: "users" },
    });
  });

  it("shows recently used items first when the query is empty", () => {
    localStorage.setItem(
      MRU_STORAGE_KEY,
      JSON.stringify(["cmd:theme.dark", "table:p1:public:audit_log"]),
    );
    renderPalette();
    expect(groupLabels()[0]).toBe("Recent");
    expect(optionTexts().slice(0, 2)).toEqual([
      expect.stringMatching(/^Theme: Dark/),
      expect.stringMatching(/^audit_log/),
    ]);
  });

  it("runs actions once the dialog has closed", async () => {
    let ran = 0;
    cleanups.push(actions.register("ui.toggle-inspector", () => void ran++));
    const input = renderPalette();
    fireEvent.change(input, { target: { value: "inspector" } });
    fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
    expect(ran).toBe(0);
    await waitFor(() => expect(ran).toBe(1));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("closes on ⌘K and on Escape", () => {
    const input = renderPalette();
    fireEvent.keyDown(input, { key: "k", code: "KeyK", ...mod });
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
    fireEvent.change(again, { target: { value: "inspector" } });
    fireEvent.keyDown(again, { key: "Enter", code: "Enter" });
    await waitFor(() => expect(focusedWhenRun).toBe(grid));
  });

  it("switches theme from the palette", async () => {
    const input = renderPalette();
    fireEvent.change(input, { target: { value: "theme dark" } });
    fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
    await waitFor(() => expect(useUiStore.getState().themePref).toBe("dark"));
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
    const row = screen.getByRole("option", { name: /Query 1/ });
    expect(within(row).getByLabelText("Unsaved changes")).toBeInTheDocument();
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
    fireEvent.change(input, { target: { value: "disconnect" } });
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
    expect(groupLabels()[0]).not.toBe("Recent");
    expect(screen.getAllByRole("option")[0]).not.toHaveTextContent(/^Disconnect/);
  });

  it("disconnects straight away when nothing is unsaved", async () => {
    useTabsStore.getState().openTable("c1", { schema: "public", name: "orders" });
    const input = renderPalette();
    fireEvent.change(input, { target: { value: "disconnect" } });
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
      fireEvent.change(input, { target: { value: "inspector" } });
      fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
      expect(usePaletteStore.getState().open).toBe(false);
      // Still animating out: the content is mounted and keeps focus.
      expect(document.querySelector("[data-slot=dialog-content]")).toHaveAttribute(
        "data-state",
        "closed",
      );
      fireEvent.keyDown(input, { key: "k", code: "KeyK", ...mod });
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
      fireEvent.change(input, { target: { value: "inspector" } });
      fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
      fireEvent.keyDown(input, { key: "k", code: "KeyK", ...mod });
      const again = screen.getByRole("combobox");
      fireEvent.change(again, { target: { value: "sidebar" } });
      fireEvent.keyDown(again, { key: "Enter", code: "Enter" });
      endAnimations();
      await waitFor(() => expect(ran).toEqual(["inspector", "sidebar"]));
      await new Promise((r) => setTimeout(r, 600));
      expect(ran).toEqual(["inspector", "sidebar"]);
    });
  });
});
