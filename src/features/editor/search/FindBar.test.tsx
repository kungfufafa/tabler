import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useChartViews } from "@/features/charts/store";
import { resetExportStore } from "@/features/export/store";
import { actions } from "@/features/palette/actions";
import {
  CLOSED_PREVIEW,
  pendingForTab,
  setResultStaging,
  useResultsStore,
} from "@/features/results/store";
import { setCell } from "@/features/staging/model";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { resetMockQueryState } from "@/lib/ipc/mock/query";
import { useConnectionsStore } from "@/stores/connections";
import { useInspectorStore } from "@/stores/inspector";
import { resetPrefsForTests } from "@/stores/prefs";
import { getQueryTab, useQueryTabsStore } from "@/stores/queryTabs";
import { flushResultVersions } from "@/stores/resultData";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { getEditorView } from "../editorStates";
import { QueryTab } from "../QueryTab";
import { useFindStore } from "./store";

const toast = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  message: vi.fn(),
  warning: vi.fn(),
}));
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
  useFindStore.setState({ byTab: {} });
  useChartViews.setState({ views: {} });
  resetExportStore();
});

afterEach(() => {
  Object.values(toast).forEach((fn) => fn.mockReset());
});

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
      </TooltipProvider>
    </QueryClientProvider>,
  );
  const type = (text: string, cursor?: number) =>
    act(() => {
      const v = getEditorView(id)!;
      v.dispatch({
        changes: { from: 0, to: v.state.doc.length, insert: text },
        selection: { anchor: cursor ?? text.length },
      });
    });
  const run = async (text: string, action: "query.run" | "query.run-current" = "query.run") => {
    type(text);
    await act(() => actions.run(action));
    act(() => flushResultVersions());
  };
  return { id, run, ...utils };
}

const findBar = () => screen.getByRole("search", { name: "Find in result" });
const counter = () => within(findBar()).getByRole("status");

describe("find in results", { timeout: 20_000 }, () => {
  it("opens with ⌘F in the results, steps with ↩ / ⇧↩ and closes with ⎋", async () => {
    const { run } = setup();
    await run("select id, email from users limit 5");
    await screen.findByRole("toolbar", { name: "Result" });
    await waitFor(() => expect(actions.has("grid.filter")).toBe(true));
    await act(() => actions.run("grid.filter"));
    const field = within(findBar()).getByRole("textbox", { name: "Find" });
    expect(field).toHaveFocus();
    fireEvent.change(field, { target: { value: "EXAMPLE" } });
    await waitFor(() => expect(counter()).toHaveTextContent("1 of 5"));
    // The grid selects the match.
    await waitFor(() => expect(useInspectorStore.getState().cell?.rowIndex).toBe(0));
    fireEvent.keyDown(field, { key: "Enter" });
    expect(counter()).toHaveTextContent("2 of 5");
    await waitFor(() => expect(useInspectorStore.getState().cell?.rowIndex).toBe(1));
    fireEvent.keyDown(field, { key: "Enter", shiftKey: true });
    fireEvent.keyDown(field, { key: "Enter", shiftKey: true });
    expect(counter()).toHaveTextContent("5 of 5");
    // Match Case: the emails are lower case.
    fireEvent.click(within(findBar()).getByRole("button", { name: "Match Case" }));
    await waitFor(() => expect(counter()).toHaveTextContent("No results"));
    fireEvent.keyDown(field, { key: "Escape" });
    expect(screen.queryByRole("search", { name: "Find in result" })).not.toBeInTheDocument();
  });

  it("stages Replace All in an editable result, undone in one step", async () => {
    const { id, run } = setup();
    await run("select id, email from users limit 5");
    const resultId = getQueryTab(id).run!.results[0]!.id;
    await waitFor(() =>
      expect(useResultsStore.getState().editability[resultId]?.kind).toBe("editable"),
    );
    fireEvent.click(screen.getByRole("button", { name: "Find in Result" }));
    const bar = findBar();
    fireEvent.change(within(bar).getByRole("textbox", { name: "Find" }), {
      target: { value: "example.com" },
    });
    fireEvent.click(within(bar).getByRole("button", { name: "Show Replace" }));
    fireEvent.change(within(bar).getByRole("textbox", { name: "Replace" }), {
      target: { value: "test.org" },
    });
    await waitFor(() => expect(counter()).toHaveTextContent("1 of 5"));
    fireEvent.click(within(bar).getByRole("button", { name: "Replace All" }));
    expect(pendingForTab(useResultsStore.getState(), id)).toBe(5);
    expect(toast.message).toHaveBeenCalledWith("Replaced 5 values", {});
    await waitFor(() => expect(counter()).toHaveTextContent("No results"));
    const staged = [...useResultsStore.getState().edits[resultId]!.staging.changes.values()];
    expect(staged.flatMap((c) => [...c.patch.values()])[0]).toEqual({
      t: "text",
      v: "ayu0@test.org",
    });
    await act(() => actions.run("grid.undo"));
    expect(pendingForTab(useResultsStore.getState(), id)).toBe(0);
  });

  it("replaces one match at a time and moves on", async () => {
    const { id, run } = setup();
    await run("select id, email from users limit 5");
    const resultId = getQueryTab(id).run!.results[0]!.id;
    await waitFor(() =>
      expect(useResultsStore.getState().editability[resultId]?.kind).toBe("editable"),
    );
    await act(() => actions.run("grid.filter"));
    const bar = findBar();
    fireEvent.change(within(bar).getByRole("textbox", { name: "Find" }), {
      target: { value: "@example" },
    });
    fireEvent.click(within(bar).getByRole("button", { name: "Show Replace" }));
    await waitFor(() => expect(counter()).toHaveTextContent("1 of 5"));
    fireEvent.click(within(bar).getByRole("button", { name: "Replace" }));
    expect(pendingForTab(useResultsStore.getState(), id)).toBe(1);
    await waitFor(() => expect(counter()).toHaveTextContent("1 of 4"));
  });

  it("searches only the columns the grid shows (Hide Column)", async () => {
    const { id, run } = setup();
    await run("select id, email from users limit 5");
    const resultId = getQueryTab(id).run!.results[0]!.id;
    await waitFor(() =>
      expect(useResultsStore.getState().editability[resultId]?.kind).toBe("editable"),
    );
    await act(() => actions.run("grid.filter"));
    const bar = findBar();
    fireEvent.change(within(bar).getByRole("textbox", { name: "Find" }), {
      target: { value: "example" },
    });
    fireEvent.click(within(bar).getByRole("button", { name: "Show Replace" }));
    await waitFor(() => expect(counter()).toHaveTextContent("1 of 5"));
    // Header menu ▸ Hide Column on "email".
    const header = [...document.querySelectorAll<HTMLElement>("[data-header]")].find((h) =>
      h.textContent?.includes("email"),
    )!;
    fireEvent.pointerDown(header, { button: 2 });
    fireEvent.contextMenu(header);
    fireEvent.click(screen.getByRole("menuitem", { name: /Hide Column/ }));
    await waitFor(() => expect(counter()).toHaveTextContent("No results"));
    // Nothing to replace unseen.
    expect(within(bar).getByRole("button", { name: "Replace All" })).toBeDisabled();
    expect(pendingForTab(useResultsStore.getState(), id)).toBe(0);
  });

  it("reports only what Replace All staged", async () => {
    const { id, run } = setup();
    await run("select id, email from users limit 5");
    const resultId = getQueryTab(id).run!.results[0]!.id;
    await waitFor(() =>
      expect(useResultsStore.getState().editability[resultId]?.kind).toBe("editable"),
    );
    // Another result of the tab holds a change: this one refuses edits until it is committed.
    act(() =>
      setResultStaging(id, "another-result", (s) =>
        setCell(s, "k", 0, { t: "text", v: "x" }, [{ t: "text", v: "y" }]),
      ),
    );
    await act(() => actions.run("grid.filter"));
    const bar = findBar();
    fireEvent.change(within(bar).getByRole("textbox", { name: "Find" }), {
      target: { value: "example.com" },
    });
    fireEvent.click(within(bar).getByRole("button", { name: "Show Replace" }));
    await waitFor(() => expect(counter()).toHaveTextContent("1 of 5"));
    fireEvent.click(within(bar).getByRole("button", { name: "Replace All" }));
    expect(toast.message).toHaveBeenCalledWith(
      "Commit or discard the other result's changes first",
      expect.anything(),
    );
    expect(toast.message).not.toHaveBeenCalledWith("Replaced 5 values", expect.anything());
    expect(useResultsStore.getState().edits[resultId]).toBeUndefined();
    // Replace (one) is refused the same way and stays on its match.
    fireEvent.click(within(bar).getByRole("button", { name: "Replace" }));
    expect(useResultsStore.getState().edits[resultId]).toBeUndefined();
    expect(counter()).toHaveTextContent("1 of 5");
  });

  it("keeps Replace off for a read-only result", async () => {
    const { run } = setup();
    await run("select 1 as n");
    await screen.findByRole("toolbar", { name: "Result" });
    fireEvent.click(screen.getByRole("button", { name: "Find in Result" }));
    const bar = findBar();
    fireEvent.change(within(bar).getByRole("textbox", { name: "Find" }), {
      target: { value: "1" },
    });
    fireEvent.click(within(bar).getByRole("button", { name: "Show Replace" }));
    await waitFor(() => expect(counter()).toHaveTextContent("1 of 1"));
    await waitFor(() => expect(within(bar).getByText("Read-only result")).toBeInTheDocument());
    expect(within(bar).getByRole("button", { name: "Replace All" })).toBeDisabled();
    expect(within(bar).getByRole("button", { name: "Replace" })).toBeDisabled();
  });
});

describe("comment-driven charts", { timeout: 20_000 }, () => {
  it("shows the chart a comment asks for, with a Chart | Grid switch", async () => {
    const { run } = setup();
    await run("-- TablePlus BarChart x: email, y: id\nselect id, email from users limit 5");
    expect(await screen.findByRole("radio", { name: /Chart/ })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByRole("img", { name: "id by email" })).toBeInTheDocument();
    expect(document.querySelector('[data-scope="grid"]')).toBeNull();
    fireEvent.click(screen.getByRole("radio", { name: /Grid/ }));
    expect(document.querySelector('[data-scope="grid"]')).not.toBeNull();
    fireEvent.click(screen.getByRole("radio", { name: /Chart/ }));
    // Find switches back to the grid.
    fireEvent.click(screen.getByRole("button", { name: "Find in Result" }));
    expect(screen.getByRole("radio", { name: /Grid/ })).toHaveAttribute("aria-checked", "true");
    expect(findBar()).toBeInTheDocument();
  });

  it("reads the directive above the statement in the editor for ⌘↩", async () => {
    const { run } = setup();
    await run(
      "select 1;\n-- Tabler LineChart x: id, y: id\nselect id from users limit 3",
      "query.run-current",
    );
    expect(await screen.findByRole("img", { name: "id by id" })).toBeInTheDocument();
  });

  it("leaves results without a directive as grids", async () => {
    const { run } = setup();
    await run("-- a note\nselect id from users limit 2");
    await screen.findByRole("toolbar", { name: "Result" });
    expect(screen.queryByRole("radiogroup", { name: "Result view" })).not.toBeInTheDocument();
  });
});
