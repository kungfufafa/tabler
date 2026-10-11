import { undo } from "@codemirror/commands";
import { forEachDiagnostic } from "@codemirror/lint";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { resetExportStore, useExportStore } from "@/features/export/store";
import { actions } from "@/features/palette/actions";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";
import { resetMockQueryState } from "@/lib/ipc/mock/query";
import { useConnectionsStore } from "@/stores/connections";
import { resetPrefsForTests } from "@/stores/prefs";
import { getQueryTab, useQueryTabsStore } from "@/stores/queryTabs";
import { flushResultVersions, resultData } from "@/stores/resultData";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { getEditorState, getEditorView } from "./editorStates";
import { QueryTab } from "./QueryTab";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

// jsdom has no layout: CodeMirror measures through these.
beforeAll(() => {
  const rects = () => Object.assign([], { item: () => null }) as unknown as DOMRectList;
  Range.prototype.getClientRects ??= rects;
  Range.prototype.getBoundingClientRect ??= () => new DOMRect();
  Element.prototype.getClientRects ??= rects;
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
  resetExportStore();
});

afterEach(() => {
  Object.values(toast).forEach((fn) => fn.mockReset());
});

function setup(opts: { keyboard?: boolean } = {}) {
  const id = useTabsStore.getState().newQuery(info.connectionId);
  const tab = useTabsStore.getState().tabs.find((t) => t.id === id) as Extract<
    Tab,
    { kind: "query" }
  >;
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const content = (
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <QueryTab tab={tab} />
      </TooltipProvider>
    </QueryClientProvider>
  );
  const utils = render(opts.keyboard ? <KeyboardProvider>{content}</KeyboardProvider> : content);
  const view = () => {
    const v = getEditorView(id);
    if (!v) throw new Error("editor not mounted");
    return v;
  };
  /** Replaces the document (what typing would do) and puts the cursor at `cursor` (default: end). */
  const type = (text: string, cursor = text.length) =>
    act(() => {
      const v = view();
      v.dispatch({
        changes: { from: 0, to: v.state.doc.length, insert: text },
        selection: { anchor: cursor },
      });
    });
  return { id, qc, view, type, ...utils };
}

describe("QueryTab", () => {
  it("mounts an empty, focused editor and registers the query actions", async () => {
    const { id, view } = setup();
    expect(view().state.doc.toString()).toBe("");
    expect(view().hasFocus).toBe(true);
    await waitFor(() => expect(actions.has("query.run")).toBe(true));
    expect(actions.has("query.cancel")).toBe(false);
    expect(screen.getByText("Run a query")).toBeInTheDocument();
    expect(useTabsStore.getState().tabs.find((t) => t.id === id)?.title).toBe("Query 1");
  });

  it("runs the editor text and shows the rows", async () => {
    const { id, type } = setup();
    type("select id, email from users limit 5");
    await act(() => actions.run("query.run"));
    act(() => flushResultVersions());
    const run = getQueryTab(id).run!;
    expect(run.status).toBe("done");
    expect(resultData.rowCount(run.results[0]!.id)).toBe(5);
    expect(await screen.findByRole("tab", { name: /Result 1 · 5 rows · \d+ ms/ })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByText(/1 statement · \d+ ms/)).toBeInTheDocument();
  });

  it("does not start a run for comments only", async () => {
    const { id, type } = setup();
    type("-- nothing here\n/* still nothing */");
    await act(() => actions.run("query.run"));
    expect(getQueryTab(id).run).toBeNull();
    expect(toast.message).toHaveBeenCalledWith("Nothing to run");
  });

  it("runs only the statement under the cursor", async () => {
    const { id, type } = setup();
    const sql = "select 1;\nselect * from tags;\nselect 3;";
    type(sql, sql.indexOf("tags"));
    await act(() => actions.run("query.run-current"));
    const run = getQueryTab(id).run!;
    expect(run.sql).toBe("select * from tags");
    expect(run.baseOffset).toBe(10);
    expect(run.statements).toHaveLength(1);
    expect(await screen.findByRole("tab", { name: /Result 1 · 15 rows/ })).toBeInTheDocument();
  });

  it("maps an error in a selection to a diagnostic at the right offset", async () => {
    const { id, type, view } = setup();
    const sql = "select 1;\nselect * from nope;";
    type(sql);
    // Select the second statement only.
    act(() => view().dispatch({ selection: { anchor: 10, head: sql.length } }));
    await act(() => actions.run("query.run"));
    const run = getQueryTab(id).run!;
    expect(run.baseOffset).toBe(10);
    expect(run.statements[0]?.status).toBe("error");
    const diagnostics: Array<{ from: number; to: number; message: string }> = [];
    forEachDiagnostic(getEditorState(id)!, (d, from, to) =>
      diagnostics.push({ from, to, message: d.message }),
    );
    // The error position is relative to the submitted selection; "nope" is at 10 + 14 in the doc.
    expect(diagnostics).toEqual([{ from: 24, to: 28, message: 'relation "nope" does not exist' }]);
    expect(await screen.findByRole("tab", { name: /Messages/ })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByText('relation "nope" does not exist')).toBeInTheDocument();
    expect(screen.getByText("Line 2, column 15 · SQLSTATE 42P01")).toBeInTheDocument();
    // Editing clears the squiggles.
    act(() => view().dispatch({ changes: { from: 0, insert: " " } }));
    await waitFor(() => {
      let n = 0;
      forEachDiagnostic(getEditorState(id)!, () => n++);
      expect(n).toBe(0);
    });
  });

  it("cancels a long-running statement with Stop", async () => {
    const { id, type } = setup();
    type("select pg_sleep(10)");
    let done: Promise<boolean> | undefined;
    act(() => {
      done = actions.run("query.run");
    });
    await waitFor(() => expect(getQueryTab(id).run?.statements).toHaveLength(1));
    await waitFor(() => expect(actions.has("query.cancel")).toBe(true));
    expect(screen.getByRole("button", { name: "Run" })).toBeDisabled();
    await act(() => actions.run("query.cancel"));
    await act(async () => {
      await done;
    });
    const run = getQueryTab(id).run!;
    expect(run.status).toBe("cancelled");
    expect(run.statements[0]?.status).toBe("cancelled");
    expect(screen.getByText(/^Cancelled · /)).toBeInTheDocument();
    expect(screen.getByText("canceling statement due to user request")).toBeInTheDocument();
  });

  it("marks the tab dirty on edit and saves to a file", async () => {
    const { id, type } = setup();
    type("select 42;");
    expect(useTabsStore.getState().tabs.find((t) => t.id === id)).toMatchObject({ dirty: true });
    const prompt = vi.spyOn(window, "prompt").mockReturnValue("/tmp/report");
    await act(() => actions.run("query.save"));
    expect(prompt).toHaveBeenCalledOnce();
    expect(await ipc.files.readText("/tmp/report.sql")).toBe("select 42;");
    const tab = useTabsStore.getState().tabs.find((t) => t.id === id);
    expect(tab).toMatchObject({ title: "report.sql", dirty: false });
    // A second save writes to the same file without asking.
    type("select 43;");
    await act(() => actions.run("query.save"));
    expect(prompt).toHaveBeenCalledOnce();
    expect(await ipc.files.readText("/tmp/report.sql")).toBe("select 43;");
    prompt.mockRestore();
  });

  it("formats the SQL", async () => {
    const { type, view } = setup();
    type("select id,name from users where id=1");
    await act(() => actions.run("query.format"));
    expect(view().state.doc.toString()).toBe(
      "SELECT\n  id,\n  name\nFROM\n  users\nWHERE\n  id = 1",
    );
  });

  it("keeps the document and undo history across remounts", async () => {
    const first = setup();
    first.type("select 'kept'");
    first.unmount();
    expect(getEditorView(first.id)).toBeNull();
    const tab = useTabsStore.getState().tabs.find((t) => t.id === first.id) as Extract<
      Tab,
      { kind: "query" }
    >;
    render(
      <QueryClientProvider client={first.qc}>
        <TooltipProvider>
          <QueryTab tab={tab} />
        </TooltipProvider>
      </QueryClientProvider>,
    );
    const view = getEditorView(first.id)!;
    expect(view.state.doc.toString()).toBe("select 'kept'");
    act(() => {
      undo(view);
    });
    expect(view.state.doc.toString()).toBe("");
  });

  it("opens a .sql file into the empty tab, or into a new tab when this one has work", async () => {
    const { id, type, view } = setup();
    const prompt = vi.spyOn(window, "prompt").mockReturnValue("/Users/me/queries/sample.sql");
    await act(() => actions.run("query.open"));
    expect(view().state.doc.toString()).toContain("FROM posts p");
    expect(useTabsStore.getState().tabs.find((t) => t.id === id)).toMatchObject({
      title: "sample.sql",
      dirty: false,
    });
    expect(getQueryTab(id).filePath).toBe("/Users/me/queries/sample.sql");

    // The same file again just focuses the tab that shows it.
    await act(() => actions.run("query.open"));
    expect(useTabsStore.getState().tabs).toHaveLength(1);

    await ipc.files.writeText("/tmp/other.sql", "select 'other';");
    prompt.mockReturnValue("/tmp/other.sql");
    type("select 'mine';");
    await act(() => actions.run("query.open"));
    const tabs = useTabsStore.getState().tabs;
    expect(tabs.map((t) => t.title)).toEqual(["sample.sql", "other.sql"]);
    expect(getEditorState(tabs[1]!.id)?.doc.toString()).toBe("select 'other';");
    expect(view().state.doc.toString()).toBe("select 'mine';");
    prompt.mockRestore();
  });

  it("explains the statement under the cursor into the Plan pane", async () => {
    const { id, type } = setup();
    const sql = "select 1;\nselect * from posts limit 5;";
    type(sql);
    await act(() => actions.run("query.explain"));
    const plan = getQueryTab(id).plan!;
    expect(plan).toMatchObject({
      status: "done",
      mode: "plan",
      sql: "select * from posts limit 5",
    });
    expect(screen.getByRole("tab", { name: "Plan" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText(/Seq Scan on posts/)).toBeInTheDocument();
  });

  it("inserts a history entry at the cursor from the history sheet", async () => {
    const { id, view } = setup();
    await act(() => actions.run("query.history"));
    expect(getQueryTab(id).historyOpen).toBe(true);
    // The mock history is empty in tests; run something first.
    act(() => useQueryTabsStore.getState().update(id, { historyOpen: false }));
    act(() => view().dispatch({ changes: { from: 0, insert: "select * from tags limit 3" } }));
    await act(() => actions.run("query.run"));
    act(() => view().dispatch({ changes: { from: 0, to: view().state.doc.length, insert: "" } }));
    await act(() => actions.run("query.history"));
    const entry = await screen.findByText("select * from tags limit 3", { selector: "pre" });
    act(() => entry.click());
    await waitFor(() => expect(getQueryTab(id).historyOpen).toBe(false));
    await waitFor(() => expect(view().state.doc.toString()).toBe("select * from tags limit 3;"));
  });

  it("keeps error locations on the right text when the document changes during a run", async () => {
    const { id, type, view } = setup();
    type("select pg_sleep(0.3);\nfoo bar;");
    let done: Promise<boolean> | undefined;
    act(() => {
      done = actions.run("query.run");
    });
    await waitFor(() => expect(getQueryTab(id).run?.statements).toHaveLength(1));
    // Typed while the first statement sleeps.
    act(() => view().dispatch({ changes: { from: 0, insert: "-- note\n" } }));
    await act(async () => {
      await done;
    });
    const run = getQueryTab(id).run!;
    expect(run.statements.map((s) => s.status)).toEqual(["ok", "error"]);
    const marked: string[] = [];
    forEachDiagnostic(getEditorState(id)!, (_, from, to) =>
      marked.push(view().state.sliceDoc(from, to)),
    );
    expect(marked).toEqual(["foo"]);
    expect(screen.getByText(/^Line 3, column 1 · /)).toBeInTheDocument();
    // Later edits move the location along with the text.
    act(() => view().dispatch({ changes: { from: 0, insert: "\n" } }));
    expect(screen.getByText(/^Line 4, column 1 · /)).toBeInTheDocument();
    const selected = () => {
      const sel = view().state.selection.main;
      return view().state.sliceDoc(sel.from, sel.to);
    };
    act(() => screen.getByTitle("Show in editor").click());
    expect(selected()).toBe("foo");
    act(() => screen.getAllByTitle("Select this statement in the editor")[0]!.click());
    expect(selected()).toBe("select pg_sleep(0.3)");
  });

  it("commits a long script's progress at most once per frame", async () => {
    const { id, type } = setup();
    const n = 250;
    type(
      Array.from(
        { length: n },
        (_, i) => `update users set email = email where id = ${i + 1};`,
      ).join("\n"),
    );
    let commits = 0;
    const unsubscribe = useQueryTabsStore.subscribe((s, prev) => {
      if (s.slices[id]?.run !== prev.slices[id]?.run) commits++;
    });
    try {
      await act(() => actions.run("query.run"));
    } finally {
      unsubscribe();
    }
    const run = getQueryTab(id).run!;
    expect(run.status).toBe("done");
    expect(run.statements).toHaveLength(n);
    expect(run.statements.every((s) => s.status === "ok")).toBe(true);
    // One store write per event used to be 2n + 2.
    expect(commits).toBeLessThan(n / 2);
    expect(screen.getByText(new RegExp(`^${n} statements · `))).toBeInTheDocument();
  });

  it("keeps ⌘R from reloading the page from the toolbar", () => {
    setup({ keyboard: true });
    const runButton = screen.getByRole("button", { name: "Run" });
    act(() => runButton.focus());
    expect(fireEvent.keyDown(runButton, { key: "r", code: "KeyR", ctrlKey: true })).toBe(false);
  });

  it("handles the query shortcuts while the result grid has focus", async () => {
    const { type } = setup({ keyboard: true });
    type("select id, email from users limit 5");
    await act(() => actions.run("query.run"));
    const grid = document.querySelector<HTMLElement>('[data-scope="grid"]')!;
    act(() => grid.focus());
    expect(document.activeElement).toBe(grid);
    const save = vi.fn();
    const run = vi.fn();
    const off = [actions.register("query.save", save), actions.register("query.run", run)];
    try {
      expect(fireEvent.keyDown(grid, { key: "s", code: "KeyS", ctrlKey: true })).toBe(false);
      expect(fireEvent.keyDown(grid, { key: "Enter", code: "Enter", ctrlKey: true })).toBe(false);
      expect(save).toHaveBeenCalledOnce();
      expect(run).toHaveBeenCalledOnce();
      // Keys without a query binding stay with the grid.
      expect(fireEvent.keyDown(grid, { key: "q", code: "KeyQ", ctrlKey: true })).toBe(true);
    } finally {
      off.forEach((dispose) => dispose());
    }
  });
});

describe("Export results", () => {
  it("exports the statement under the cursor, overriding the app-wide ⌘⇧E default", async () => {
    // Stands in for <ExportHost/>, which the app shell mounts before any tab.
    const fallback = vi.fn();
    const off = actions.register("export.open", fallback);
    try {
      const { type } = setup();
      type("select 1;\n-- recent signups\nselect id, email from users;\n", 35);
      await waitFor(() => expect(actions.has("query.run")).toBe(true));
      await act(() => actions.run("export.open"));
      expect(fallback).not.toHaveBeenCalled();
      expect(useExportStore.getState()).toMatchObject({
        open: true,
        target: {
          connectionId: info.connectionId,
          source: { kind: "sql", sql: "select id, email from users" },
        },
      });
    } finally {
      off();
    }
  });

  it("exports the selection from the toolbar, but only a single statement", async () => {
    const { view, type } = setup();
    type("select 1;\nselect 2;");
    act(() => view().dispatch({ selection: { anchor: 0, head: view().state.doc.length } }));
    fireEvent.click(screen.getByRole("button", { name: "Export results…" }));
    expect(useExportStore.getState().open).toBe(false);
    expect(toast.message).toHaveBeenCalledWith(
      "Select a single statement to export",
      expect.anything(),
    );

    act(() => view().dispatch({ selection: { anchor: 10, head: view().state.doc.length } }));
    fireEvent.click(screen.getByRole("button", { name: "Export results…" }));
    expect(useExportStore.getState().target?.source).toEqual({ kind: "sql", sql: "select 2" });
  });

  it("says so when there is nothing to export", () => {
    setup();
    fireEvent.click(screen.getByRole("button", { name: "Export results…" }));
    expect(useExportStore.getState().open).toBe(false);
    expect(toast.message).toHaveBeenCalledWith("Nothing to export", expect.anything());
  });
});
