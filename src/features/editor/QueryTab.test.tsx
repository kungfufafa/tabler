import { undo } from "@codemirror/commands";
import { forEachDiagnostic } from "@codemirror/lint";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { resetExportStore, useExportStore } from "@/features/export/store";
import { actions } from "@/features/palette/actions";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";
import { resetMockQueryState } from "@/lib/ipc/mock/query";
import { useConnectionsStore } from "@/stores/connections";
import { resetPrefsForTests } from "@/stores/prefs";
import { getQueryTab, updateQueryTab, useQueryTabsStore } from "@/stores/queryTabs";
import { flushResultVersions, resultData } from "@/stores/resultData";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { explainQuery, NO_LIMIT_MAX_ROWS } from "./controller";
import { getEditorState, getEditorView } from "./editorStates";
import { QueryTab } from "./QueryTab";
import { RUN_BUTTON_KEY, useRunButtonStore } from "./runButton";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

// jsdom has no layout: CodeMirror measures through these.
beforeAll(() => {
  const rects = () => Object.assign([], { item: () => null }) as unknown as DOMRectList;
  Range.prototype.getClientRects ??= rects;
  Range.prototype.getBoundingClientRect ??= () => new DOMRect();
  Element.prototype.getClientRects ??= rects;
  // Radix Select (the row limit) needs these.
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.scrollIntoView ??= () => {};
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
  localStorage.removeItem(RUN_BUTTON_KEY);
  useRunButtonStore.setState({ mode: "current" });
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
    expect(screen.getByText(/^1 statement · 5 rows · \d+ ms$/)).toBeInTheDocument();
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
    // Cancel takes the Run button's place while the statement runs.
    expect(screen.queryByRole("button", { name: "Run Current" })).toBeNull();
    expect(
      within(screen.getByRole("toolbar", { name: "Query" })).getByText("Running…"),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await act(async () => {
      await done;
    });
    const run = getQueryTab(id).run!;
    expect(run.status).toBe("cancelled");
    expect(run.statements[0]?.status).toBe("cancelled");
    expect(screen.getByText(/^Cancelled · /)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
    expect(screen.getByRole("button", { name: "Run Current" })).toBeEnabled();
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

  it("beautifies and uglifies the statement under the cursor, or the selection", async () => {
    const { type, view } = setup();
    const doc = () => view().state.doc.toString();
    const sql = "select 1;\nselect id,name -- who\nfrom users where id=1;\nselect 3;";
    type(sql, sql.indexOf("users"));
    await act(() => actions.run("query.format"));
    expect(doc()).toBe(
      "select 1;\nSELECT\n  id,\n  name -- who\nFROM\n  users\nWHERE\n  id = 1;\nselect 3;",
    );
    // The cursor stays in the statement it formatted.
    await act(() => actions.run("query.minify"));
    expect(doc()).toBe("select 1;\nSELECT id, name FROM users WHERE id = 1;\nselect 3;");
    // A selection is rewritten as it is, and stays selected.
    act(() => view().dispatch({ selection: { anchor: 0, head: view().state.doc.length } }));
    await act(() => actions.run("query.minify"));
    expect(doc()).toBe("select 1; SELECT id, name FROM users WHERE id = 1; select 3;");
    const sel = view().state.selection.main;
    expect([sel.from, sel.to]).toEqual([0, doc().length]);
  });

  it("runs the current statement from the Run button, or everything once chosen", async () => {
    const { id, type } = setup();
    const sql = "select 1;\nselect * from tags;\nselect 3;";
    type(sql, sql.indexOf("tags"));
    fireEvent.click(screen.getByRole("button", { name: "Run Current" }));
    await waitFor(() => expect(getQueryTab(id).run?.status).toBe("done"));
    expect(getQueryTab(id).run?.sql).toBe("select * from tags");

    fireEvent.keyDown(screen.getByRole("button", { name: "Run options" }), { key: "Enter" });
    expect(await screen.findByRole("menuitem", { name: /Run Current/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("menuitemradio", { name: "All statements" }));
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());
    expect(useRunButtonStore.getState().mode).toBe("all");
    expect(localStorage.getItem(RUN_BUTTON_KEY)).toBe("all");

    fireEvent.click(screen.getByRole("button", { name: "Run All" }));
    await waitFor(() => expect(getQueryTab(id).run).toMatchObject({ sql, status: "done" }));
    expect(getQueryTab(id).run?.statements).toHaveLength(3);
  });

  it("binds the query keys in CodeMirror too (⌘↩, ⇧⌘↩, ⌘I, ⇧⌘I, ⌃`)", async () => {
    const { view } = setup();
    await waitFor(() => expect(actions.has("query.minify")).toBe(true));
    const ids = [
      "query.run-current",
      "query.run",
      "query.format",
      "query.minify",
      "query.toggle-results",
    ] as const;
    const calls: string[] = [];
    const off = ids.map((id) => actions.register(id, () => void calls.push(id)));
    try {
      const key = (init: KeyboardEventInit & { keyCode: number }) =>
        fireEvent.keyDown(view().contentDOM, init);
      // jsdom is not macOS: Mod is Ctrl.
      key({ key: "Enter", code: "Enter", keyCode: 13, ctrlKey: true });
      key({ key: "Enter", code: "Enter", keyCode: 13, ctrlKey: true, shiftKey: true });
      key({ key: "i", code: "KeyI", keyCode: 73, ctrlKey: true });
      key({ key: "I", code: "KeyI", keyCode: 73, ctrlKey: true, shiftKey: true });
      key({ key: "`", code: "Backquote", keyCode: 192, ctrlKey: true });
      await waitFor(() => expect(calls).toEqual(ids));
      expect(view().state.doc.toString()).toBe("");
    } finally {
      off.forEach((dispose) => dispose());
    }
  });

  it("collapses the result pane (⌃`) and shows it again for a new run", async () => {
    const { id, type } = setup();
    await waitFor(() => expect(actions.has("query.toggle-results")).toBe(true));
    const pane = screen.getByTestId("results");
    expect(pane).not.toHaveAttribute("inert");
    await act(() => actions.run("query.toggle-results"));
    expect(getQueryTab(id).resultsCollapsed).toBe(true);
    expect(pane).toHaveAttribute("inert");

    fireEvent.click(screen.getByRole("button", { name: "Show results" }));
    expect(getQueryTab(id).resultsCollapsed).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Hide results" }));
    expect(getQueryTab(id).resultsCollapsed).toBe(true);

    type("select 1");
    await act(() => actions.run("query.run"));
    expect(getQueryTab(id).resultsCollapsed).toBe(false);
    expect(pane).not.toHaveAttribute("inert");
    expect(screen.getByRole("button", { name: "Hide results" })).toBeInTheDocument();
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

  it("says that an Explain Analyze was rolled back", async () => {
    const { id, type } = setup();
    type("delete from posts where id = 1");
    await act(() => explainQuery(id, "analyze"));
    expect(getQueryTab(id).plan).toMatchObject({ status: "done", mode: "analyze" });
    expect(screen.getByText("EXPLAIN ANALYZE")).toBeInTheDocument();
    expect(
      screen.getByText(/Ran inside a transaction that was rolled back — no changes were kept/),
    ).toBeInTheDocument();
    // A plain EXPLAIN executes nothing: no note.
    await act(() => explainQuery(id, "plan"));
    expect(screen.queryByText(/Ran inside a transaction/)).toBeNull();
  });

  it("refreshes cached schema data after DDL runs in the editor", async () => {
    const { qc, type } = setup();
    // Queries nobody shows here (the sidebar's, a table tab's) stay marked stale.
    const seeded = [
      ["tables", info.connectionId, "public"],
      ["table", info.connectionId, "public", "users"],
      ["rows", info.connectionId, "public.users", {}, 0],
    ];
    const other = ["tables", "another-connection", "public"];
    for (const key of [...seeded, other]) qc.setQueryData(key, { seeded: true });
    const stale = (key: unknown[]) =>
      qc.getQueryCache().find({ queryKey: key, exact: true })?.state.isInvalidated;
    const invalidate = vi.spyOn(qc, "invalidateQueries");
    const completion = { queryKey: ["completion", info.connectionId] };

    type("select * from users limit 1");
    await act(() => actions.run("query.run"));
    expect(seeded.map(stale)).toEqual([false, false, false]);
    expect(invalidate).not.toHaveBeenCalledWith(completion);

    type("-- tidy up\nALTER TABLE users DROP COLUMN bio;");
    await act(() => actions.run("query.run"));
    expect(seeded.map(stale)).toEqual([true, true, true]);
    expect(stale(other)).toBe(false);
    // The editor's own completion refetches.
    expect(invalidate).toHaveBeenCalledWith(completion);
    invalidate.mockRestore();
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

  it("keeps ⌘R from reloading the page from the editor bar", () => {
    setup({ keyboard: true });
    const runButton = screen.getByRole("button", { name: "Run Current" });
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
    const off = [actions.register("query.save", save), actions.register("query.run-current", run)];
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

describe("Editor bar", () => {
  it("sits under the editor, with no toolbar above it", () => {
    setup();
    const bar = screen.getByRole("toolbar", { name: "Query" });
    const editor = screen.getByTestId("sql-editor");
    // The editor comes first in the document; the bar follows it.
    expect(editor.compareDocumentPosition(bar) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getAllByRole("toolbar")).toHaveLength(1);
  });

  it("shows the cursor position, or how much is selected", () => {
    const { view, type } = setup();
    const position = screen.getByTestId("cursor-position");
    expect(position).toHaveTextContent("Line 1, Column 1");
    type("select 1;\nselect 22", 19);
    expect(position).toHaveTextContent("Line 2, Column 10");
    act(() => view().dispatch({ selection: { anchor: 0, head: 6 } }));
    expect(position).toHaveTextContent("6 characters selected");
    act(() => view().dispatch({ selection: { anchor: 3, head: 4 } }));
    expect(position).toHaveTextContent("1 character selected");
    act(() => view().dispatch({ selection: { anchor: 12 } }));
    expect(position).toHaveTextContent("Line 2, Column 3");
  });

  it("summarizes rows changed and errors of the last run", async () => {
    const { type } = setup();
    type("update users set email = email where id = 1");
    await act(() => actions.run("query.run"));
    expect(screen.getByText(/^1 statement · \d+ rows? affected · \d+ ms$/)).toBeInTheDocument();

    type("select 1;\nselect * from nope;");
    await act(() => actions.run("query.run"));
    const summary = screen.getByText(/^2 statements · 1 error · \d+ ms$/);
    expect(summary).toHaveClass("text-error");
  });

  it("caps each run at the tab's row limit", async () => {
    const user = userEvent.setup();
    const { id, type } = setup();
    const run = vi.spyOn(ipc.query, "run");
    try {
      const limit = screen.getByRole("combobox", { name: "Row limit" });
      expect(limit).toHaveTextContent("Limit 10,000");
      type("select * from events_big");
      await act(() => actions.run("query.run"));
      expect(run.mock.calls.at(-1)?.[3]).toEqual({ maxRows: 10_000, stopOnError: true });

      act(() => limit.focus());
      await user.keyboard("{Enter}");
      await waitFor(() => expect(screen.getByRole("option", { name: "10,000" })).toHaveFocus());
      await user.keyboard("{ArrowUp}{ArrowUp}{Enter}");
      await waitFor(() => expect(getQueryTab(id).rowLimit).toBe(100));
      expect(limit).toHaveTextContent("Limit 100");

      await act(() => actions.run("query.run"));
      act(() => flushResultVersions());
      expect(run.mock.calls.at(-1)?.[3]).toEqual({ maxRows: 100, stopOnError: true });
      expect(getQueryTab(id).run?.results[0]?.truncated).toBe(true);
      expect(screen.getByText(/^1 statement · 100 rows · \d+ ms$/)).toBeInTheDocument();
      expect(screen.getByText("limit reached")).toBeInTheDocument();

      // "No limit" asks for the backend's highest cap (0 would mean its default).
      act(() => updateQueryTab(id, { rowLimit: null }));
      expect(limit).toHaveTextContent("No limit");
      type("select 1");
      await act(() => actions.run("query.run"));
      expect(run.mock.calls.at(-1)?.[3]).toEqual({ maxRows: NO_LIMIT_MAX_ROWS, stopOnError: true });
    } finally {
      run.mockRestore();
    }
  });

  it("keeps the row limit per query tab", () => {
    const first = setup();
    act(() => updateQueryTab(first.id, { rowLimit: 1_000 }));
    first.unmount();
    const second = setup();
    expect(getQueryTab(second.id).rowLimit).toBe(10_000);
    expect(screen.getByRole("combobox", { name: "Row limit" })).toHaveTextContent("Limit 10,000");
    expect(getQueryTab(first.id).rowLimit).toBe(1_000);
  });

  it("shows the plan's outcome while the Plan pane is selected", async () => {
    const { type } = setup();
    type("select * from posts limit 5");
    await act(() => actions.run("query.explain"));
    expect(screen.getByText(/^Explain · \d+ ms$/)).toBeInTheDocument();
  });

  it("beautifies and uglifies from the Beautify button and its menu", async () => {
    const { type, view } = setup();
    type("select id,name from users");
    fireEvent.click(screen.getByRole("button", { name: "Beautify" }));
    await waitFor(() =>
      expect(view().state.doc.toString()).toBe("SELECT\n  id,\n  name\nFROM\n  users"),
    );
    fireEvent.keyDown(screen.getByRole("button", { name: "Format options" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: /Uglify SQL/ }));
    await waitFor(() => expect(view().state.doc.toString()).toBe("SELECT id, name FROM users"));
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

  it("exports the result the pane shows, as its rows (not the statement under the cursor)", async () => {
    const { id, type, view } = setup();
    type("select id, email from users limit 3;\nselect 1;");
    await act(() => actions.run("query.run"));
    act(() => flushResultVersions());
    const run = getQueryTab(id).run!;
    expect(run.results).toHaveLength(2);
    // The cursor sits in the second statement; the first result is shown.
    act(() => view().dispatch({ selection: { anchor: view().state.doc.length } }));
    act(() => updateQueryTab(id, { pane: run.results[0]!.id }));
    fireEvent.click(screen.getByRole("button", { name: "Export results…" }));
    const source = useExportStore.getState().target?.source;
    expect(source).toMatchObject({
      kind: "rows",
      label: "Result 1",
      table: "users",
      sql: "select id, email from users limit 3",
      truncated: false,
      stopped: null,
    });
    if (source?.kind !== "rows") throw new Error("not a rows source");
    expect(source.columns.map((c) => c.name)).toEqual(["id", "email"]);
    const buffer = resultData.get(run.results[0]!.id)!;
    expect(source.rows).toEqual(buffer.rows);
    // A copy: a later run cannot change what is being exported.
    expect(source.rows).not.toBe(buffer.rows);
    expect(source.rows[0]).not.toBe(buffer.rows[0]);

    act(() => updateQueryTab(id, { pane: run.results[1]!.id }));
    await act(() => actions.run("export.open"));
    expect(useExportStore.getState().target?.source).toMatchObject({
      kind: "rows",
      label: "Result 2",
      rows: [[{ t: "int", v: 1 }]],
    });

    // The messages pane shows no result: the statement under the cursor is exported.
    act(() => updateQueryTab(id, { pane: "messages" }));
    await act(() => actions.run("export.open"));
    expect(useExportStore.getState().target?.source).toEqual({ kind: "sql", sql: "select 1" });
  });

  it("exports a cancelled result as incomplete, so the dialog can say so", async () => {
    const { id, type } = setup();
    type("select * from events_big");
    let done: Promise<boolean> | undefined;
    act(() => {
      done = actions.run("query.run");
    });
    // Some rows arrived; then Stop.
    await waitFor(() => {
      const first = getQueryTab(id).run?.results[0];
      expect(first && (resultData.get(first.id)?.rows.length ?? 0)).toBeGreaterThan(0);
    });
    await waitFor(() => expect(actions.has("query.cancel")).toBe(true));
    await act(() => actions.run("query.cancel"));
    await act(async () => {
      await done;
    });
    act(() => flushResultVersions());
    const run = getQueryTab(id).run!;
    expect(run.status).toBe("cancelled");
    act(() => updateQueryTab(id, { pane: run.results[0]!.id }));
    await act(() => actions.run("export.open"));
    const source = useExportStore.getState().target?.source;
    expect(source).toMatchObject({
      kind: "rows",
      label: "Result 1",
      sql: "select * from events_big",
      truncated: false,
      stopped: "cancelled",
    });
    if (source?.kind !== "rows") throw new Error("not a rows source");
    expect(source.rows.length).toBe(resultData.get(run.results[0]!.id)!.rows.length);
    expect(source.rows.length).toBeLessThan(10_000);
  });

  it("says so when there is nothing to export", () => {
    setup();
    fireEvent.click(screen.getByRole("button", { name: "Export results…" }));
    expect(useExportStore.getState().open).toBe(false);
    expect(toast.message).toHaveBeenCalledWith("Nothing to export", expect.anything());
  });
});
