import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
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

beforeEach(() => resetMockQueryState());

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

  it("ignores the trailing clicks of a slow double-click once the entry was inserted", async () => {
    const { option, onInsert, onRun, reopen } = setup();
    const entry = await option(NEWEST);
    fireEvent.click(entry, { detail: 1 });
    await wait(350);
    expect(onInsert).toHaveBeenCalledExactlyOnceWith(NEWEST);
    // The second click of a double-click slower than the wait, while the sheet is closing.
    fireEvent.click(entry, { detail: 2 });
    fireEvent.doubleClick(entry, { detail: 2 });
    fireEvent.keyDown(entry, { key: "Enter", ctrlKey: true });
    await wait(350);
    expect(onInsert).toHaveBeenCalledOnce();
    expect(onRun).not.toHaveBeenCalled();

    // Opened again, it reacts as usual: a quick double-click runs (and does not insert).
    reopen();
    const again = await option(NEWEST);
    fireEvent.click(again, { detail: 1 });
    fireEvent.click(again, { detail: 2 });
    fireEvent.doubleClick(again, { detail: 2 });
    await wait(350);
    expect(onRun).toHaveBeenCalledExactlyOnceWith(NEWEST);
    expect(onInsert).toHaveBeenCalledOnce();
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
