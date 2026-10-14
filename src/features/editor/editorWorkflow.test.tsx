import { forEachDiagnostic } from "@codemirror/lint";
import { EditorSelection } from "@codemirror/state";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { actions } from "@/features/palette/actions";
import { closeSavedQueryDialog, useSavedQueryDialogStore } from "@/features/queries/dialogStore";
import { guardExecution } from "@/features/safety/guard";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { resetMockQueryState } from "@/lib/ipc/mock/query";
import { useConnectionsStore } from "@/stores/connections";
import { getPrefs, resetPrefsForTests, setPref } from "@/stores/prefs";
import { getQueryTab, useQueryTabsStore } from "@/stores/queryTabs";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { selectAllOccurrences } from "./cmExtensions";
import { getEditorState, getEditorView } from "./editorStates";
import { useParamsPromptStore } from "./paramsPrompt";
import { QueryTab } from "./QueryTab";
import { isSplitOpen, splitKey, useSplitStore } from "./split";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));
vi.mock("@/features/safety/guard", () => ({ guardExecution: vi.fn(async () => true) }));
const guard = vi.mocked(guardExecution);

beforeAll(() => {
  const rects = () => Object.assign([], { item: () => null }) as unknown as DOMRectList;
  Range.prototype.getClientRects ??= rects;
  Range.prototype.getBoundingClientRect ??= () => new DOMRect();
  Element.prototype.getClientRects ??= rects;
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
  closeSavedQueryDialog();
  useParamsPromptStore.setState({ request: null });
  useSplitStore.setState({ open: {}, focused: {} });
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
  guard.mockReset();
  guard.mockResolvedValue(true);
});

afterEach(() => {
  Object.values(toast).forEach((fn) => fn.mockReset());
  vi.restoreAllMocks();
});

function setup() {
  const id = useTabsStore.getState().newQuery(info.connectionId);
  const tab = useTabsStore.getState().tabs.find((t) => t.id === id) as Extract<
    Tab,
    { kind: "query" }
  >;
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <QueryTab tab={tab} />
      </TooltipProvider>
    </QueryClientProvider>,
  );
  const view = (key = id) => {
    const v = getEditorView(key);
    if (!v) throw new Error("editor not mounted");
    return v;
  };
  const type = (text: string, cursor = text.length, key = id) =>
    act(() => {
      const v = view(key);
      v.dispatch({
        changes: { from: 0, to: v.state.doc.length, insert: text },
        selection: { anchor: cursor },
      });
    });
  return { id, view, type };
}

describe("safe mode guard", () => {
  it("asks before every run with the statements about to run", async () => {
    const { id, type } = setup();
    type("select 1;\nselect 2;");
    await act(() => actions.run("query.run"));
    expect(guard).toHaveBeenCalledWith(info.connectionId, {
      kind: "query",
      statements: ["select 1", "select 2"],
    });
    expect(getQueryTab(id).run?.status).toBe("done");
  });

  it("does not run, and says nothing, when the guard declines", async () => {
    guard.mockResolvedValue(false);
    const run = vi.spyOn(ipc.query, "run");
    const { id, type } = setup();
    type("delete from users");
    await act(() => actions.run("query.run"));
    expect(run).not.toHaveBeenCalled();
    expect(getQueryTab(id).run).toBeNull();
    expect(toast.error).not.toHaveBeenCalled();
  });

  it("guards an explain as the EXPLAIN statement", async () => {
    guard.mockResolvedValue(false);
    const explain = vi.spyOn(ipc.query, "explain");
    const { type } = setup();
    type("select * from users");
    await act(() => actions.run("query.explain"));
    expect(guard).toHaveBeenCalledWith(info.connectionId, {
      kind: "query",
      statements: ["EXPLAIN select * from users"],
    });
    expect(explain).not.toHaveBeenCalled();
  });
});

describe("Return on Error", () => {
  it("stops at the first error by default, and carries on when it is off", async () => {
    const run = vi.spyOn(ipc.query, "run");
    const { id, type } = setup();
    type("select * from nope;\nselect 1;");
    await act(() => actions.run("query.run"));
    expect(run.mock.calls.at(-1)?.[3]).toMatchObject({ stopOnError: true });
    expect(getQueryTab(id).run?.statements).toHaveLength(1);

    setPref("editorStopOnError", false);
    await act(() => actions.run("query.run"));
    expect(run.mock.calls.at(-1)?.[3]).toMatchObject({ stopOnError: false });
    expect(getQueryTab(id).run?.statements.map((s) => s.status)).toEqual(["error", "ok"]);
  });
});

describe("query parameters", () => {
  it("asks for each variable, runs the substituted SQL and remembers the values", async () => {
    setPref("editorQueryParams", true);
    const run = vi.spyOn(ipc.query, "run");
    const { id, type } = setup();
    type("select * from users where id = :id or id = :id + :step");
    let done: Promise<boolean> | undefined;
    act(() => {
      done = actions.run("query.run");
    });
    const dialog = await screen.findByRole("dialog", { name: "Query Parameters" });
    const inputs = within(dialog).getAllByRole("textbox");
    expect(
      inputs.map((i) => i.id && dialog.querySelector(`label[for="${i.id}"]`)?.textContent),
    ).toEqual([":id", ":step"]);
    fireEvent.change(inputs[0]!, { target: { value: "7" } });
    fireEvent.change(inputs[1]!, { target: { value: "1" } });
    // ⌘↩ applies.
    fireEvent.keyDown(inputs[1]!, { key: "Enter", metaKey: true });
    await act(async () => {
      await done;
    });
    expect(run.mock.calls.at(-1)?.[2]).toBe("select * from users where id = 7 or id = 7 + 1");
    expect(guard).toHaveBeenLastCalledWith(info.connectionId, {
      kind: "query",
      statements: ["select * from users where id = 7 or id = 7 + 1"],
    });
    expect(getQueryTab(id).run?.sql).toBe("select * from users where id = 7 or id = 7 + 1");

    // The next run starts from the values used last.
    act(() => {
      done = actions.run("query.run");
    });
    const again = await screen.findByRole("dialog", { name: "Query Parameters" });
    expect(
      within(again)
        .getAllByRole("textbox")
        .map((i) => (i as HTMLInputElement).value),
    ).toEqual(["7", "1"]);
    // Cancel: nothing runs.
    fireEvent.click(within(again).getByRole("button", { name: "Cancel" }));
    await act(async () => {
      await done;
    });
    expect(run).toHaveBeenCalledTimes(1);
  });

  it("leaves SQL without variables (or with params off) alone", async () => {
    const run = vi.spyOn(ipc.query, "run");
    const { type } = setup();
    type("select ':nope', 1 -- :neither");
    setPref("editorQueryParams", true);
    await act(() => actions.run("query.run"));
    expect(screen.queryByRole("dialog")).toBeNull();
    setPref("editorQueryParams", false);
    type("select :x");
    await act(() => actions.run("query.run"));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(run.mock.calls.map((c) => c[2])).toEqual(["select ':nope', 1 -- :neither", "select :x"]);
  });

  it("maps an error after a substituted variable back to the editor text", async () => {
    setPref("editorQueryParams", true);
    const { id, type } = setup();
    type("select :v, * from nope;");
    let done: Promise<boolean> | undefined;
    act(() => {
      done = actions.run("query.run");
    });
    const dialog = await screen.findByRole("dialog", { name: "Query Parameters" });
    const input = within(dialog).getByRole("textbox");
    fireEvent.change(input, { target: { value: "123456" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await act(async () => {
      await done;
    });
    const found: Array<[number, number]> = [];
    forEachDiagnostic(getEditorState(id)!, (_d, from, to) => found.push([from, to]));
    const at = "select :v, * from ".length;
    expect(found).toEqual([[at, at + 4]]);
  });
});

describe("editor bar", () => {
  it("has the ⚙ settings and the Run menu's save commands", async () => {
    setup();
    fireEvent.keyDown(screen.getByRole("button", { name: "Editor settings" }), { key: "Enter" });
    const menu = await screen.findByRole("menu");
    for (const label of [
      "Font Size",
      "Wrap Lines to Editor Width",
      "Highlight Current Query",
      "Show Invisible Characters",
      "Autocomplete Options",
      "Query Params Options",
      "Return on Error",
    ])
      expect(within(menu).getByText(label)).toBeInTheDocument();
    expect(
      within(menu).getByRole("menuitemcheckbox", { name: "Highlight Current Query" }),
    ).toBeChecked();
    expect(within(menu).getByRole("menuitemcheckbox", { name: "Return on Error" })).toBeChecked();
    fireEvent.click(
      within(menu).getByRole("menuitemcheckbox", { name: "Show Invisible Characters" }),
    );
    expect(getPrefs().editorShowInvisibles).toBe(true);
    fireEvent.click(
      within(menu).getByRole("menuitemcheckbox", { name: "Highlight Current Query" }),
    );
    expect(getPrefs().editorHighlightCurrentQuery).toBe(false);
    fireEvent.keyDown(menu, { key: "Escape" });

    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    fireEvent.keyDown(screen.getByRole("button", { name: "Run options" }), { key: "Enter" });
    const run = await screen.findByRole("menu");
    expect(within(run).getByRole("menuitem", { name: /Add to Queries…/ })).toBeInTheDocument();
    expect(within(run).getByRole("menuitem", { name: "Save SQL As…" })).toBeInTheDocument();
    expect(
      within(run).getByRole("menuitem", { name: /Save Selected SQL Statements As…/ }),
    ).toBeInTheDocument();
  });

  it("applies the editor settings to the open editor", async () => {
    const { view, type } = setup();
    type("select 1;\nselect  2;", 3);
    expect(view().dom.querySelector(".cm-current-statement")).not.toBeNull();
    act(() => setPref("editorHighlightCurrentQuery", false));
    expect(view().dom.querySelector(".cm-current-statement")).toBeNull();
    act(() => setPref("editorShowInvisibles", true));
    expect(view().dom.querySelector(".cm-highlightSpace")).not.toBeNull();
    expect(view().dom.querySelector(".cm-eol-marker")).not.toBeNull();
  });

  it("changes the font size with ⌘+ / ⌘−", async () => {
    setup();
    const before = getPrefs().editorFontSize;
    await act(() => actions.run("query.font-bigger"));
    expect(getPrefs().editorFontSize).toBe(before + 1);
    await act(() => actions.run("query.font-smaller"));
    await act(() => actions.run("query.font-smaller"));
    expect(getPrefs().editorFontSize).toBe(before - 1);
  });

  it("opens Add to Queries… (⌥⌘S) with the selection or the current statement", async () => {
    const { type, view } = setup();
    const sql = "select 1;\nselect * from users;";
    type(sql, sql.length - 3);
    await act(() => actions.run("query.add-to-queries"));
    expect(useSavedQueryDialogStore.getState().request?.draft).toMatchObject({
      sql: "select * from users",
      name: "select * from users",
    });
    expect(useSavedQueryDialogStore.getState().request?.connection).toEqual({
      profileId: "p-local-pg",
      name: info.name,
    });
    act(() => view().dispatch({ selection: EditorSelection.single(0, 8) }));
    await act(() => actions.run("query.add-to-queries"));
    expect(useSavedQueryDialogStore.getState().request?.draft.sql).toBe("select 1");
  });

  it("saves the selected statements to a new file (⇧⌘S) without changing the tab", async () => {
    const prompt = vi.spyOn(window, "prompt").mockReturnValue("/tmp/picked.sql");
    const write = vi.spyOn(ipc.sqlFiles, "write");
    const { id, type, view } = setup();
    type("select 1;\nselect 2;");
    act(() => view().dispatch({ selection: EditorSelection.single(10, 19) }));
    await act(() => actions.run("query.save-selection"));
    expect(prompt).toHaveBeenCalled();
    expect(write).toHaveBeenCalledWith("/tmp/picked.sql", "select 2;\n");
    expect(getQueryTab(id).filePath).toBeNull();
  });
});

describe("multiple carets", () => {
  it("selects every occurrence of the word under the caret (⌃⌘G)", () => {
    const { view, type } = setup();
    type("select id, name from t where id > 1 and id < 9", 8);
    act(() => void selectAllOccurrences(view()));
    const ranges = view().state.selection.ranges.map((r) => [r.from, r.to]);
    expect(ranges).toEqual([
      [7, 9],
      [29, 31],
      [40, 42],
    ]);
  });

  it("binds TablePlus's caret and comment keys", () => {
    const { view, type } = setup();
    type("select 1\nselect 2\nselect 3", 3);
    const key = (init: KeyboardEventInit) =>
      act(
        () =>
          void view().contentDOM.dispatchEvent(
            new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
          ),
      );
    // ⌃⇧↓ adds a caret below; Esc goes back to one.
    key({ key: "ArrowDown", code: "ArrowDown", ctrlKey: true, shiftKey: true });
    expect(view().state.selection.ranges).toHaveLength(2);
    key({ key: "Escape", code: "Escape" });
    expect(view().state.selection.ranges).toHaveLength(1);
    // ⌥⌘/ (Ctrl+Alt+/ off macOS) toggles a block comment around the caret's line, ⌘/ a line
    // comment.
    act(() => view().dispatch({ selection: { anchor: 3 } }));
    key({ key: "/", code: "Slash", ctrlKey: true, altKey: true });
    expect(view().state.doc.line(1).text).toBe("/* select 1 */");
    key({ key: "/", code: "Slash", ctrlKey: true, altKey: true });
    expect(view().state.doc.line(1).text).toBe("select 1");
    key({ key: "/", code: "Slash", ctrlKey: true });
    expect(view().state.doc.line(1).text).toBe("-- select 1");
  });
});

describe("split pane", () => {
  it("opens a second editor (⇧⌘D) and runs from the pane used last", async () => {
    const run = vi.spyOn(ipc.query, "run");
    const { id, type, view } = setup();
    const dirty = () => {
      const tab = useTabsStore.getState().tabs.find((t) => t.id === id);
      return tab?.kind === "query" && tab.dirty === true;
    };
    await act(() => actions.run("query.split"));
    expect(isSplitOpen(id)).toBe(true);
    const second = splitKey(id);
    await waitFor(() => expect(getEditorView(second)).not.toBeNull());
    type("select 2", 8, second);
    // The split pane is a scratch editor: it never makes the tab dirty.
    expect(dirty()).toBe(false);
    act(() => fireEvent.focus(view(second).contentDOM));
    await act(() => actions.run("query.run"));
    expect(run.mock.calls.at(-1)?.[2]).toBe("select 2");
    type("select 1");
    expect(dirty()).toBe(true);
    act(() => fireEvent.focus(view(id).contentDOM));
    await act(() => actions.run("query.run"));
    expect(run.mock.calls.at(-1)?.[2]).toBe("select 1");

    fireEvent.click(screen.getByRole("button", { name: "Close split pane" }));
    expect(isSplitOpen(id)).toBe(false);
    expect(getEditorState(second)).toBeNull();
  });
});
