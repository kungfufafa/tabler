import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { HISTORY_SEARCH_OPTIONS_KEY } from "@/features/sidebar/historyEntry";
import { ipc } from "@/lib/ipc";
import { resetMockQueryState } from "@/lib/ipc/mock/query";
import { HistorySheet } from "./HistorySheet";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

// Seeded mock history, newest first:
//   4 select count(*) from events_big
//   3 update posts set views = views + 1 where id = 7
//   2 selec * from posts            (failed)
//   1 select * from users limit 50
const NEWEST = "select count(*) from events_big";
const UPDATE = "update posts set views = views + 1 where id = 7";
const USERS = "select * from users limit 50";

beforeAll(() => {
  // jsdom has no layout; the sheet keeps the active option scrolled into view.
  Element.prototype.scrollIntoView ??= () => {};
});

beforeEach(() => {
  localStorage.removeItem(HISTORY_SEARCH_OPTIONS_KEY);
  resetMockQueryState();
});
afterEach(() => vi.restoreAllMocks());

function setup() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onInsert = vi.fn();
  const onRun = vi.fn();
  // The sheet stays open: what a closing sheet still receives during its exit animation.
  const onOpenChange = vi.fn();
  const ui = (open: boolean) => (
    <QueryClientProvider client={qc}>
      <HistorySheet
        tabId="t-history"
        open={open}
        onOpenChange={onOpenChange}
        onInsert={onInsert}
        onRun={onRun}
      />
    </QueryClientProvider>
  );
  const utils = render(ui(true));
  const reopen = () => {
    utils.rerender(ui(false));
    utils.rerender(ui(true));
  };
  const input = () => screen.getByRole("combobox", { name: "Search history" });
  const option = async (sql: string) =>
    (await screen.findByText(sql, { selector: "pre" })).closest<HTMLElement>("[role=option]")!;
  return { onInsert, onRun, onOpenChange, input, option, reopen };
}

const ids = async () => (await ipc.history.list({ search: null, limit: 200 })).map((e) => e.id);
const wait = (ms: number) => act(() => new Promise<void>((r) => setTimeout(r, ms)));

describe("HistorySheet", () => {
  it("runs the entry that matches what was just typed, not the stale list (⌘↩)", async () => {
    const { input, option, onRun } = setup();
    await option(NEWEST);
    fireEvent.change(input(), { target: { value: "users" } });
    // Within the search debounce: the previous (unfiltered) result is still the cached one.
    fireEvent.keyDown(input(), { key: "Enter", ctrlKey: true });
    expect(onRun).toHaveBeenCalledExactlyOnceWith(USERS);
  });

  it("inserts the entry that matches what was just typed (Enter)", async () => {
    const { input, option, onInsert } = setup();
    await option(NEWEST);
    fireEvent.change(input(), { target: { value: "UPDATE" } });
    expect(screen.queryByText(NEWEST, { selector: "pre" })).toBeNull();
    fireEvent.keyDown(input(), { key: "Enter" });
    expect(onInsert).toHaveBeenCalledExactlyOnceWith(UPDATE);
  });

  it("leaves ⌘⌫ to the search field and deletes entries only from the focused list", async () => {
    const { input, option } = setup();
    await option(NEWEST);
    fireEvent.change(input(), { target: { value: "selec" } });
    const notPrevented = fireEvent.keyDown(input(), { key: "Backspace", ctrlKey: true });
    expect(notPrevented).toBe(true);
    await wait(20);
    expect(await ids()).toEqual([4, 3, 2, 1]);

    fireEvent.change(input(), { target: { value: "" } });
    const listbox = screen.getByRole("listbox", { name: "History" });
    act(() => listbox.focus());
    fireEvent.keyDown(listbox, { key: "ArrowDown" });
    expect(fireEvent.keyDown(listbox, { key: "Backspace", ctrlKey: true })).toBe(false);
    await waitFor(async () => expect(await ids()).toEqual([4, 2, 1]));
  });

  it("only selects on a click, inserts on a double-click and never runs it", async () => {
    const { option, onInsert, onRun, onOpenChange } = setup();
    const users = await option(USERS);
    fireEvent.click(users, { detail: 1 });
    await wait(500);
    // A click selects the entry: nothing is inserted, run or closed.
    expect(users.getAttribute("aria-selected")).toBe("true");
    expect(onInsert).not.toHaveBeenCalled();
    expect(onRun).not.toHaveBeenCalled();
    expect(onOpenChange).not.toHaveBeenCalled();

    fireEvent.click(users, { detail: 1 });
    fireEvent.click(users, { detail: 2 });
    fireEvent.doubleClick(users, { detail: 2 });
    expect(onInsert).toHaveBeenCalledExactlyOnceWith(USERS);
    expect(onRun).not.toHaveBeenCalled();
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("ignores the trailing clicks of a slow double-click once the entry was inserted", async () => {
    const { option, onInsert, onRun, reopen } = setup();
    const entry = await option(NEWEST);
    fireEvent.click(entry, { detail: 1 });
    fireEvent.click(entry, { detail: 2 });
    fireEvent.doubleClick(entry, { detail: 2 });
    expect(onInsert).toHaveBeenCalledExactlyOnceWith(NEWEST);
    // More clicks of the gesture while the sheet is closing.
    fireEvent.click(entry, { detail: 3 });
    fireEvent.doubleClick(entry, { detail: 2 });
    fireEvent.keyDown(entry, { key: "Enter", ctrlKey: true });
    await wait(350);
    expect(onInsert).toHaveBeenCalledOnce();
    expect(onRun).not.toHaveBeenCalled();

    // Opened again, it reacts as usual: a double-click inserts (and does not run).
    reopen();
    const again = await option(NEWEST);
    fireEvent.click(again, { detail: 1 });
    fireEvent.click(again, { detail: 2 });
    fireEvent.doubleClick(again, { detail: 2 });
    await wait(350);
    expect(onInsert).toHaveBeenCalledTimes(2);
    expect(onRun).not.toHaveBeenCalled();
  });

  it("runs an entry from its Run button or ⌘↩ only", async () => {
    const { option, input, onInsert, onRun } = setup();
    const entry = await option(UPDATE);
    fireEvent.click(within(entry).getByTestId("history-run"));
    expect(onRun).toHaveBeenCalledExactlyOnceWith(UPDATE);
    expect(onInsert).not.toHaveBeenCalled();
    expect(input()).toBeInTheDocument();
  });

  it("searches with Match Case, Whole Word and Regex, remembered like the History pane's", async () => {
    const { input, option, reopen } = setup();
    await option(NEWEST);
    const list = vi.spyOn(ipc.history, "list");
    const toggle = (name: string) => screen.getByRole("button", { name });
    const shown = () => screen.queryAllByRole("option").length;

    // Match Case: "USERS" no longer finds "users".
    fireEvent.change(input(), { target: { value: "USERS" } });
    await waitFor(() => expect(shown()).toBe(1));
    fireEvent.click(toggle("Match Case"));
    expect(toggle("Match Case")).toHaveAttribute("aria-pressed", "true");
    await waitFor(() => expect(shown()).toBe(0));
    await waitFor(() =>
      expect(list).toHaveBeenLastCalledWith(
        expect.objectContaining({ search: "USERS", matchCase: true, wholeWord: false }),
      ),
    );
    expect(await screen.findByText("No matches")).toBeInTheDocument();

    // Whole Word: "select" is a word; "selec" is only a prefix of it.
    fireEvent.click(toggle("Match Case"));
    fireEvent.change(input(), { target: { value: "selec" } });
    await waitFor(() => expect(shown()).toBe(4 - 1));
    fireEvent.click(toggle("Whole Word"));
    await waitFor(() => expect(shown()).toBe(1));
    expect(await option("selec * from posts")).toBeInTheDocument();

    // Regex: a pattern, and an invalid one says so without asking the backend.
    fireEvent.click(toggle("Whole Word"));
    fireEvent.click(toggle("Regex"));
    expect(input()).toHaveAttribute("placeholder", "Search SQL (regex)…");
    fireEvent.change(input(), { target: { value: "^update .* id = \\d$" } });
    await waitFor(() => expect(shown()).toBe(1));
    expect(await option(UPDATE)).toBeInTheDocument();
    list.mockClear();
    fireEvent.change(input(), { target: { value: "(" } });
    expect(await screen.findByText("Invalid regular expression")).toBeInTheDocument();
    expect(input()).toHaveAttribute("aria-invalid", "true");
    await wait(300);
    expect(list).not.toHaveBeenCalled();

    // Remembered (shared with the pane through localStorage) and re-read when the sheet opens.
    expect(JSON.parse(localStorage.getItem(HISTORY_SEARCH_OPTIONS_KEY) ?? "null")).toEqual({
      matchCase: false,
      wholeWord: false,
      regex: true,
    });
    localStorage.setItem(
      HISTORY_SEARCH_OPTIONS_KEY,
      JSON.stringify({ matchCase: true, wholeWord: true, regex: false }),
    );
    reopen();
    await waitFor(() => expect(toggle("Whole Word")).toHaveAttribute("aria-pressed", "true"));
    expect(toggle("Match Case")).toHaveAttribute("aria-pressed", "true");
    expect(toggle("Regex")).toHaveAttribute("aria-pressed", "false");
  });

  it("badges the statements an MCP client ran", async () => {
    const base = (await ipc.history.list({ search: null, limit: 1 }))[0]!;
    vi.spyOn(ipc.history, "list").mockResolvedValue([
      { ...base, id: 9, sql: "select 9", source: "mcp" },
      { ...base, id: 8, sql: "select 8", source: "editor" },
    ]);
    const { option } = setup();
    const mcp = await option("select 9");
    expect(within(mcp).getByText("MCP")).toBeInTheDocument();
    expect(within(await option("select 8")).queryByText("MCP")).toBeNull();
  });

  it("exposes the list as a combobox popup with the keyboard-active option", async () => {
    const { input, option } = setup();
    await option(NEWEST);
    const listbox = screen.getByRole("listbox", { name: "History" });
    expect(input()).toHaveAttribute("aria-controls", listbox.id);
    expect(input()).toHaveAttribute("aria-expanded", "true");
    const options = screen.getAllByRole("option");
    expect(options).toHaveLength(4);
    expect(new Set(options.map((o) => o.id)).size).toBe(4);
    expect(input()).toHaveAttribute("aria-activedescendant", options[0]!.id);
    fireEvent.keyDown(input(), { key: "ArrowDown" });
    expect(input()).toHaveAttribute("aria-activedescendant", options[1]!.id);
    expect(options[1]).toHaveAttribute("aria-selected", "true");
    expect(options[0]).toHaveAttribute("aria-selected", "false");
    // Only groups (one per day, named after it) inside the listbox, options inside them.
    for (const child of Array.from(listbox.children))
      expect(child).toHaveAttribute("role", "group");
    const today = screen.getByRole("group", { name: "Today" });
    expect(within(today).getAllByRole("option")).toContain(options[0]);
  });
});
