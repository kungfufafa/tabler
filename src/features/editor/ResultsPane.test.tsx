import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useActivePendingCount } from "@/app/useActivePendingCount";
import { resetExportStore } from "@/features/export/store";
import { actions } from "@/features/palette/actions";
import { READ_ONLY_RESULT } from "@/features/results/editability";
import { CLOSED_PREVIEW, pendingForTab, useResultsStore } from "@/features/results/store";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { resetMockQueryState } from "@/lib/ipc/mock/query";
import { useConnectionsStore } from "@/stores/connections";
import { getPrefs, resetPrefsForTests, setPref } from "@/stores/prefs";
import { getQueryTab, useQueryTabsStore } from "@/stores/queryTabs";
import { flushResultVersions, resultData } from "@/stores/resultData";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { getEditorView } from "./editorStates";
import { QueryTab } from "./QueryTab";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

// jsdom has no layout: CodeMirror measures through these, the grids' virtualizers need a size.
const sizes: Array<[string, number]> = [
  ["offsetWidth", 1200],
  ["offsetHeight", 600],
  ["clientWidth", 1200],
  ["clientHeight", 600],
];
const saved = new Map<string, PropertyDescriptor | undefined>();
beforeAll(() => {
  const rects = () => Object.assign([], { item: () => null }) as unknown as DOMRectList;
  Range.prototype.getClientRects ??= rects;
  Range.prototype.getBoundingClientRect ??= () => new DOMRect();
  Element.prototype.getClientRects ??= rects;
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
  for (const [prop, value] of sizes) {
    saved.set(prop, Object.getOwnPropertyDescriptor(HTMLElement.prototype, prop));
    Object.defineProperty(HTMLElement.prototype, prop, { configurable: true, get: () => value });
  }
});
afterAll(() => {
  for (const [prop, desc] of saved) {
    if (desc) Object.defineProperty(HTMLElement.prototype, prop, desc);
    else delete (HTMLElement.prototype as unknown as Record<string, unknown>)[prop];
  }
});

let info: ConnectionInfo;

beforeAll(async () => {
  info = await ipc.connections.connect("p-local-pg");
});

afterAll(async () => {
  await ipc.connections.disconnect(info.connectionId);
});

beforeEach(() => {
  resetMockQueryState({ emptyHistory: true });
  resetPrefsForTests();
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useConnectionsStore.getState().add(info);
  useTabsStore.setState({
    tabs: [],
    activeId: null,
    closed: [],
    closeRequest: null,
    queryCounter: 0,
  });
  useQueryTabsStore.setState({ slices: {} });
  useResultsStore.setState({ edits: {}, editability: {}, pins: {}, preview: CLOSED_PREVIEW });
  resetExportStore();
});

afterEach(() => {
  Object.values(toast).forEach((fn) => fn.mockReset());
});

/** The toolbar's count (what the action control shows). */
function PendingProbe() {
  return <output data-testid="pending">{useActivePendingCount()}</output>;
}

function setup() {
  const id = useTabsStore.getState().newQuery(info.connectionId);
  const tab = useTabsStore.getState().tabs.find((t) => t.id === id) as Extract<
    Tab,
    { kind: "query" }
  >;
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const utils = render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <QueryTab tab={tab} />
        <PendingProbe />
      </TooltipProvider>
    </QueryClientProvider>,
  );
  const run = async (text: string) => {
    act(() => {
      const v = getEditorView(id)!;
      v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: text } });
    });
    await act(() => actions.run("query.run"));
    act(() => flushResultVersions());
  };
  return { id, run, ...utils };
}

const footer = () => screen.getByRole("toolbar", { name: "Result" });

// The first test also pays for mounting CodeMirror and the grid.
describe("results pane", { timeout: 20_000 }, () => {
  it("shows TablePlus's footer: rows, time, Show Message, Pin, row numbers, Export…", async () => {
    const { id, run } = setup();
    await run("select id, email from users limit 5");
    const bar = await waitFor(() => footer());
    expect(within(bar).getByText("5 rows")).toBeInTheDocument();
    expect(within(bar).getByText(/^\d+ ms$/)).toBeInTheDocument();
    fireEvent.click(within(bar).getByRole("button", { name: /Show Message/ }));
    expect(getQueryTab(id).pane).toBe("messages");
  });

  it("says the row limit cut the result", async () => {
    const { id, run } = setup();
    act(() => useQueryTabsStore.getState().update(id, { rowLimit: 100 }));
    await run("select * from events_big");
    expect(await within(footer()).findByText("auto limited to 100 rows")).toBeInTheDocument();
  });

  it("toggles the row numbers (a preference)", async () => {
    const { run } = setup();
    await run("select id from users limit 2");
    fireEvent.click(await within(footer()).findByRole("button", { name: "Hide Row Numbers" }));
    expect(getPrefs().resultRowNumbers).toBe(false);
    expect(within(footer()).getByRole("button", { name: "Show Row Numbers" })).toBeInTheDocument();
  });

  it("edits a result from one table and commits it with ⌘S", async () => {
    const { id, run } = setup();
    await run("select id, email from users limit 5");
    const resultId = getQueryTab(id).run!.results[0]!.id;
    await waitFor(() =>
      expect(useResultsStore.getState().editability[resultId]?.kind).toBe("editable"),
    );
    // ⌘I (the grid's New row) stages a row; the toolbar counts it.
    await waitFor(() => expect(actions.has("grid.new-row")).toBe(true));
    await act(() => actions.run("grid.new-row"));
    expect(pendingForTab(useResultsStore.getState(), id)).toBe(1);
    expect(screen.getByTestId("pending")).toHaveTextContent("1");
    expect(actions.has("grid.commit")).toBe(true);
    // ⌘S in the editor commits instead of saving the file.
    await act(() => actions.run("query.save"));
    await waitFor(() => expect(pendingForTab(useResultsStore.getState(), id)).toBe(0));
    expect(resultData.rowCount(resultId)).toBe(6);
    expect(toast.success).toHaveBeenCalledWith(
      expect.stringMatching(/^Applied 1 change/),
      expect.objectContaining({ action: expect.objectContaining({ label: "Re-run" }) }),
    );
    expect(getQueryTab(id).filePath).toBeNull();
  });

  it("discards staged result edits with ⇧⌘⌫", async () => {
    const { id, run } = setup();
    await run("select id, email from users limit 5");
    await waitFor(() => expect(actions.has("grid.new-row")).toBe(true));
    await act(() => actions.run("grid.new-row"));
    await act(() => actions.run("grid.discard"));
    expect(pendingForTab(useResultsStore.getState(), id)).toBe(0);
  });

  it("says why a computed result cannot be edited", async () => {
    const { id, run } = setup();
    await run("select 1 as n");
    const resultId = getQueryTab(id).run!.results[0]!.id;
    await waitFor(() =>
      expect(useResultsStore.getState().editability[resultId]?.kind).toBe("readOnly"),
    );
    const grid = document.querySelector<HTMLElement>('[data-scope="grid"]')!;
    fireEvent.keyDown(grid, { key: "Enter" });
    expect(toast.message).toHaveBeenCalledWith(READ_ONLY_RESULT, expect.anything());
    expect(actions.has("grid.new-row")).toBe(false);
  });

  it("keeps a self-join read-only", async () => {
    const { id, run } = setup();
    // Editing "manager" must never write the employee's own row.
    await run("select e.id, m.email as manager from users e join users m on m.id = e.id");
    const resultId = getQueryTab(id).run!.results[0]!.id;
    await waitFor(() =>
      expect(useResultsStore.getState().editability[resultId]?.kind).toBe("readOnly"),
    );
    expect(actions.has("grid.new-row")).toBe(false);
  });

  it("stacks every result set in one tab in append mode", async () => {
    setPref("resultViewMode", "append");
    const { run } = setup();
    await run("select id from users limit 2; select id from posts limit 3;");
    expect(await screen.findByRole("tab", { name: /Results/ })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.queryByRole("tab", { name: /Result 1/ })).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Result 1" })).toHaveTextContent(/Result 1 · 2 rows/);
    expect(screen.getByRole("region", { name: "Result 2" })).toHaveTextContent(/Result 2 · 3 rows/);
  });

  it("keeps a pinned result when the query runs again", async () => {
    const { id, run } = setup();
    await run("select id from users limit 2");
    fireEvent.click(await within(footer()).findByRole("button", { name: "Pin Result" }));
    expect(
      await screen.findByRole("tab", { name: /Pinned.*Result 1 · 2 rows/ }),
    ).toBeInTheDocument();
    await run("select id from posts limit 3");
    const tabs = screen.getAllByRole("tab", { name: /Result 1/ });
    expect(tabs.map((t) => t.textContent)).toEqual([
      expect.stringMatching(/Result 1 · 2 rows/),
      expect.stringMatching(/Result 1 · 3 rows/),
    ]);
    expect(Object.values(useResultsStore.getState().pins[id] ?? [])).toHaveLength(1);
  });

  it("cancels a statement past the query time out", async () => {
    setPref("queryTimeout", 1);
    const { id, run } = setup();
    await run("select pg_sleep(5)");
    const st = getQueryTab(id).run!.statements[0]!;
    expect(st.status).toBe("error");
    expect(st.error?.message).toBe("Query timed out after 1 s");
  }, 10_000);
});
