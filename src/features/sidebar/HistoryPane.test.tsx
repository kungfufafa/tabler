import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as controller from "@/features/editor/controller";
import { getEditorState } from "@/features/editor/editorStates";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { resetMockQueryState } from "@/lib/ipc/mock/query";
import type { OpenConnection } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { HistoryPane } from "./HistoryPane";
import { entryTime } from "./historyEntry";
import { OPTIONS_STORAGE_KEY, reloadSidebarPrefs, useSidebarPrefs } from "./sidebarPrefs";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

// Seeded mock history of profile `p-local-pg`, newest first:
//   4 select count(*) from events_big                     10 min ago
//   3 update posts set views = views + 1 where id = 7     1 day ago
//   2 selec * from posts            (failed)              1 day 1 h ago
//   1 select * from users limit 50                        3 days ago
const NEWEST = "select count(*) from events_big";
const UPDATE = "update posts set views = views + 1 where id = 7";
const FAILED = "selec * from posts";
const USERS = "select * from users limit 50";

function connection(profileId = "p-local-pg", name = "Local Postgres"): OpenConnection {
  const info: ConnectionInfo = {
    connectionId: "c1",
    profileId,
    name,
    color: null,
    driver: "postgres",
    serverVersion: "17.6",
    currentDatabase: "tabler_dev",
    currentUser: "tabler",
    tunnel: null,
    readOnly: false,
  };
  return { info, status: "connected", message: null, currentSchema: "public" };
}

function setup(conn = connection()) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <HistoryPane connection={conn} />
    </QueryClientProvider>,
  );
  const entry = async (sql: string) => (await screen.findByText(sql)).closest("button")!;
  const search = () => screen.getByRole("textbox", { name: "Search history" });
  return { entry, search };
}

const tabs = () => useTabsStore.getState().tabs;
const sqlOf = (tabId: string) => getEditorState(tabId)?.doc.toString();
const wait = (ms: number) => act(() => new Promise<void>((r) => setTimeout(r, ms)));

describe("HistoryPane", () => {
  beforeEach(() => {
    // Only the clock is fixed (mid-afternoon), so "Today" / "Yesterday" never straddle midnight.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2026, 9, 12, 15, 0, 0));
    resetMockQueryState();
    useTabsStore.setState({
      tabs: [],
      activeId: null,
      closed: [],
      closeRequest: null,
      queryCounter: 0,
    });
    localStorage.removeItem(OPTIONS_STORAGE_KEY);
    reloadSidebarPrefs();
    toast.success.mockClear();
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("lists the connection's statements newest first, grouped by day", async () => {
    const { entry } = setup();
    await entry(NEWEST);
    const groups = screen.getAllByRole("group");
    expect(groups.map((g) => g.getAttribute("aria-label"))).toEqual([
      "Today",
      "Yesterday",
      expect.stringMatching(/Fri, 9 Oct|Fri, Oct 9/),
    ]);
    expect(within(groups[0]!).getByText(NEWEST)).toBeInTheDocument();
    expect(
      within(groups[1]!)
        .getAllByRole("button")
        .map((b) => b.textContent),
    ).toEqual([`${UPDATE}15:00`, `${FAILED}14:00`]);
    expect(within(groups[2]!).getByText(USERS)).toBeInTheDocument();
    expect(within(await entry(NEWEST)).getByText("10 min ago")).toBeInTheDocument();
    expect(within(await entry(UPDATE)).getByText("15:00")).toBeInTheDocument();
    expect(within(await entry(FAILED)).getByRole("img", { name: "Failed" })).toBeInTheDocument();
  });

  it("filters by the active connection unless “All connections” is on", async () => {
    const list = vi.spyOn(ipc.history, "list");
    setup(connection("p-other", "Shop"));
    expect(await screen.findByText("No history yet")).toBeInTheDocument();
    expect(screen.getByText("Statements you run on Shop appear here.")).toBeInTheDocument();
    expect(list).toHaveBeenLastCalledWith({ profileId: "p-other", search: null, limit: 200 });

    const toggle = screen.getByRole("button", { name: "All connections" });
    expect(toggle).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(toggle);
    expect(await screen.findByText(NEWEST)).toBeInTheDocument();
    expect(list).toHaveBeenLastCalledWith({ profileId: null, search: null, limit: 200 });
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    expect(useSidebarPrefs.getState().options.historyAllConnections).toBe(true);
    expect(JSON.parse(localStorage.getItem(OPTIONS_STORAGE_KEY)!)).toMatchObject({
      historyAllConnections: true,
    });
  });

  it("filters as you type, and says when nothing matches", async () => {
    const { entry, search } = setup();
    await entry(NEWEST);
    fireEvent.change(search(), { target: { value: "POSTS" } });
    // At once, before the debounced fetch.
    expect(screen.queryByText(NEWEST)).toBeNull();
    expect(screen.getByText(UPDATE)).toBeInTheDocument();
    expect(screen.getByText(FAILED)).toBeInTheDocument();
    fireEvent.change(search(), { target: { value: "nothing like this" } });
    expect(await screen.findByText("No matches")).toBeInTheDocument();
    // Esc clears the search.
    fireEvent.keyDown(search(), { key: "Escape" });
    expect(search()).toHaveValue("");
    expect(await screen.findByText(NEWEST)).toBeInTheDocument();
  });

  it("opens a clicked entry in a new query tab, and runs it on a double-click", async () => {
    const run = vi.spyOn(controller, "runQuery").mockResolvedValue();
    const { entry } = setup();
    fireEvent.click(await entry(UPDATE), { detail: 1 });
    // The click waits for a possible second one.
    expect(tabs()).toHaveLength(0);
    await waitFor(() => expect(tabs()).toHaveLength(1));
    const first = tabs()[0]!;
    expect(first).toMatchObject({ kind: "query", connectionId: "c1" });
    expect(useTabsStore.getState().activeId).toBe(first.id);
    expect(sqlOf(first.id)).toBe(UPDATE);
    expect(run).not.toHaveBeenCalled();

    const row = await entry(USERS);
    fireEvent.click(row, { detail: 1 });
    fireEvent.click(row, { detail: 2 });
    fireEvent.doubleClick(row);
    await wait(400);
    expect(tabs()).toHaveLength(2);
    const second = tabs()[1]!;
    expect(sqlOf(second.id)).toBe(USERS);
    expect(run).toHaveBeenCalledExactlyOnceWith(second.id, "auto", expect.any(Object));
  });

  it("moves through the entries with the keyboard: ↓ from the search, Enter, ⌘↩, ⌘⌫", async () => {
    const run = vi.spyOn(controller, "runQuery").mockResolvedValue();
    const { entry, search } = setup();
    await entry(NEWEST);
    fireEvent.keyDown(search(), { key: "ArrowDown" });
    expect(await entry(NEWEST)).toHaveFocus();
    fireEvent.keyDown(await entry(NEWEST), { key: "ArrowDown" });
    expect(await entry(UPDATE)).toHaveFocus();
    fireEvent.keyDown(await entry(UPDATE), { key: "ArrowUp" });
    fireEvent.keyDown(await entry(NEWEST), { key: "ArrowUp" });
    expect(search()).toHaveFocus();

    // Enter (a keyboard click) opens at once; ⌘↩ opens and runs.
    fireEvent.click(await entry(NEWEST), { detail: 0 });
    expect(tabs()).toHaveLength(1);
    expect(sqlOf(tabs()[0]!.id)).toBe(NEWEST);
    fireEvent.keyDown(await entry(FAILED), { key: "Enter", metaKey: true });
    expect(tabs()).toHaveLength(2);
    expect(run).toHaveBeenCalledExactlyOnceWith(tabs()[1]!.id, "auto", expect.any(Object));

    fireEvent.keyDown(await entry(FAILED), { key: "Backspace", metaKey: true });
    await waitFor(() => expect(screen.queryByText(FAILED)).toBeNull());
    expect((await ipc.history.list({})).map((e) => e.sql)).not.toContain(FAILED);
  });

  it("offers Open in New Tab, Run in New Tab, Copy SQL and Delete Entry in the context menu", async () => {
    const run = vi.spyOn(controller, "runQuery").mockResolvedValue();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const { entry } = setup();

    fireEvent.contextMenu(await entry(UPDATE));
    fireEvent.click(screen.getByRole("menuitem", { name: "Open in New Tab" }));
    expect(tabs()).toHaveLength(1);
    expect(sqlOf(tabs()[0]!.id)).toBe(UPDATE);

    fireEvent.contextMenu(await entry(UPDATE));
    fireEvent.click(screen.getByRole("menuitem", { name: /Run in New Tab/ }));
    expect(tabs()).toHaveLength(2);
    expect(run).toHaveBeenCalledOnce();

    fireEvent.contextMenu(await entry(UPDATE));
    fireEvent.click(screen.getByRole("menuitem", { name: "Copy SQL" }));
    expect(writeText).toHaveBeenCalledWith(UPDATE);
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Copied SQL"));

    fireEvent.contextMenu(await entry(UPDATE));
    fireEvent.click(screen.getByRole("menuitem", { name: /Delete Entry/ }));
    await waitFor(() => expect(screen.queryByText(UPDATE)).toBeNull());
    expect(screen.getByText(NEWEST)).toBeInTheDocument();
  });
});

describe("entryTime", () => {
  const now = new Date(2026, 9, 12, 15, 0, 0);
  const ago = (ms: number) => new Date(now.getTime() - ms).toISOString();
  it("is relative within the last day, else the time of day", () => {
    expect(entryTime(ago(10_000), now)).toBe("just now");
    expect(entryTime(ago(5 * 60_000), now)).toBe("5 min ago");
    expect(entryTime(ago(3 * 3_600_000), now)).toBe("3 h ago");
    expect(entryTime(ago(26 * 3_600_000), now)).toBe("13:00");
    expect(entryTime("not a date", now)).toBe("");
  });
});
