import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as controller from "@/features/editor/controller";
import { getEditorState } from "@/features/editor/editorStates";
import { closeSavedQueryDialog, useSavedQueryDialogStore } from "@/features/queries/dialogStore";
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
  it("badges the statements an MCP client ran", async () => {
    const base = (await ipc.history.list({ search: null, limit: 1 }))[0]!;
    vi.spyOn(ipc.history, "list").mockResolvedValue([
      { ...base, id: 9, sql: "select 9", source: "mcp" },
      { ...base, id: 8, sql: "select 8", source: "editor" },
    ]);
    const { entry } = setup();
    expect(within(await entry("select 9")).getByText("MCP")).toBeInTheDocument();
    expect(within(await entry("select 8")).queryByText("MCP")).toBeNull();
  });

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
    const noOptions = { matchCase: false, wholeWord: false, regex: false };
    expect(list).toHaveBeenLastCalledWith({
      profileId: "p-other",
      search: null,
      limit: 200,
      ...noOptions,
    });

    const toggle = screen.getByRole("button", { name: "All connections" });
    expect(toggle).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(toggle);
    expect(await screen.findByText(NEWEST)).toBeInTheDocument();
    expect(list).toHaveBeenLastCalledWith({
      profileId: null,
      search: null,
      limit: 200,
      ...noOptions,
    });
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

  it("only selects on a click, and opens a new query tab on a double-click without running it", async () => {
    const run = vi.spyOn(controller, "runQuery").mockResolvedValue();
    const { entry } = setup();
    const update = await entry(UPDATE);
    fireEvent.click(update, { detail: 1 });
    // A click selects the entry (to copy it, say): no tab opens, now or later.
    await wait(500);
    expect(tabs()).toHaveLength(0);
    expect(update.getAttribute("aria-selected")).toBe("true");
    expect(run).not.toHaveBeenCalled();

    const row = await entry(USERS);
    fireEvent.click(row, { detail: 1 });
    fireEvent.click(row, { detail: 2 });
    fireEvent.doubleClick(row);
    await wait(500);
    expect(tabs()).toHaveLength(1);
    const first = tabs()[0]!;
    expect(first).toMatchObject({ kind: "query", connectionId: "c1" });
    expect(useTabsStore.getState().activeId).toBe(first.id);
    expect(sqlOf(first.id)).toBe(USERS);
    // Opening is not running: a double-click on an old DELETE / UPDATE must be harmless.
    expect(run).not.toHaveBeenCalled();
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

    // Enter (a keyboard click, the double-click of the keyboard) opens without running; ⌘↩ opens
    // and runs.
    fireEvent.click(await entry(NEWEST), { detail: 0 });
    expect(tabs()).toHaveLength(1);
    expect(sqlOf(tabs()[0]!.id)).toBe(NEWEST);
    expect(run).not.toHaveBeenCalled();
    fireEvent.keyDown(await entry(FAILED), { key: "Enter", metaKey: true });
    expect(tabs()).toHaveLength(2);
    expect(run).toHaveBeenCalledExactlyOnceWith(tabs()[1]!.id, "auto", expect.any(Object));

    fireEvent.keyDown(await entry(FAILED), { key: "Backspace", metaKey: true });
    await waitFor(() => expect(screen.queryByText(FAILED)).toBeNull());
    expect((await ipc.history.list({})).map((e) => e.sql)).not.toContain(FAILED);
  });

  it("lists Copy, Run, Open in New Tab, Insert in that order in the context menu", async () => {
    const { entry } = setup();
    fireEvent.contextMenu(await entry(UPDATE));
    const names = screen.getAllByRole("menuitem").map((m) => (m.textContent ?? "").trim());
    const at = (label: RegExp) => names.findIndex((n) => label.test(n));
    expect(at(/^Copy SQL/)).toBe(0);
    expect(at(/^Run in New Tab/)).toBe(1);
    expect(at(/^Open in New Tab/)).toBe(2);
    expect(at(/^Insert to SQL Editor/)).toBe(3);
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
    fireEvent.click(screen.getByRole("menuitem", { name: /Copy SQL/ }));
    expect(writeText).toHaveBeenCalledWith(UPDATE);
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Copied SQL"));

    fireEvent.contextMenu(await entry(UPDATE));
    fireEvent.click(screen.getByRole("menuitem", { name: /Delete Entry/ }));
    await waitFor(() => expect(screen.queryByText(UPDATE)).toBeNull());
    expect(screen.getByText(NEWEST)).toBeInTheDocument();
  });

  it("selects several entries with ⌘-click / ⇧-click and copies them all with ⌘C", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const { entry } = setup();
    fireEvent.click(await entry(NEWEST), { metaKey: true, detail: 1 });
    fireEvent.click(await entry(FAILED), { shiftKey: true, detail: 1 });
    await wait(400);
    // Selecting opens nothing.
    expect(tabs()).toHaveLength(0);
    expect((await entry(UPDATE)).getAttribute("aria-selected")).toBe("true");
    expect((await entry(USERS)).getAttribute("aria-selected")).toBe("false");
    fireEvent.keyDown(await entry(UPDATE), { key: "c", metaKey: true });
    expect(writeText).toHaveBeenCalledWith(`${NEWEST};\n${UPDATE};\n${FAILED};`);
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Copied 3 statements"));
    fireEvent.contextMenu(await entry(UPDATE));
    fireEvent.click(screen.getByRole("menuitem", { name: /Delete 3 Entries/ }));
    await waitFor(() => expect(screen.queryByText(UPDATE)).toBeNull());
    expect(screen.getByText(USERS)).toBeInTheDocument();
  });

  it("searches with Match Case / Whole Word / Regex from the filter menu", async () => {
    const list = vi.spyOn(ipc.history, "list");
    const { entry, search } = setup();
    await entry(NEWEST);
    fireEvent.pointerDown(screen.getByRole("button", { name: "History options" }), {
      button: 0,
      ctrlKey: false,
    });
    fireEvent.click(await screen.findByRole("menuitemcheckbox", { name: "Regex" }));
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    fireEvent.change(search(), { target: { value: "^(select|update)\\b" } });
    await waitFor(() =>
      expect(list).toHaveBeenLastCalledWith(
        expect.objectContaining({ search: "^(select|update)\\b", regex: true }),
      ),
    );
    expect(screen.queryByText(FAILED)).toBeNull();
    expect(screen.getByText(UPDATE)).toBeInTheDocument();
    fireEvent.change(search(), { target: { value: "(" } });
    expect(await screen.findByText("Invalid regular expression")).toBeInTheDocument();
    localStorage.removeItem("tabler.history.searchOptions");
  });

  it("clears the history older than a day, and all of it", async () => {
    const { entry } = setup();
    fireEvent.contextMenu(await entry(UPDATE));
    fireEvent.click(screen.getByRole("menuitem", { name: /Clear History Older Than/ }));
    const date = await screen.findByLabelText("Clear history before");
    // Before yesterday afternoon: the 3-day-old entry and the failed one (1 day 1 h ago) go.
    fireEvent.change(date, { target: { value: "2026-10-11" } });
    fireEvent.click(screen.getByRole("button", { name: "Clear" }));
    await waitFor(() => expect(screen.queryByText(USERS)).toBeNull());
    expect(screen.getByText(UPDATE)).toBeInTheDocument();
    expect(toast.success).toHaveBeenCalledWith("Deleted 1 entry");

    fireEvent.contextMenu(await entry(UPDATE));
    fireEvent.click(screen.getByRole("menuitem", { name: /Clear All History/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Clear History" }));
    expect(await screen.findByText("No history yet")).toBeInTheDocument();
  });

  it("adds an entry to the saved queries", async () => {
    const { entry } = setup();
    fireEvent.contextMenu(await entry(UPDATE));
    fireEvent.click(screen.getByRole("menuitem", { name: "Add to Queries…" }));
    expect(useSavedQueryDialogStore.getState().request).toMatchObject({
      draft: { id: null, sql: UPDATE },
      connection: { profileId: "p-local-pg", name: "Local Postgres" },
    });
    closeSavedQueryDialog();
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
