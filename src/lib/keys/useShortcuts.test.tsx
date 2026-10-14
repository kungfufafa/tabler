import { EditorState } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { act, render, renderHook } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { actions, type ActionId } from "@/features/palette/actions";
import { useDefaultActions } from "@/features/palette/defaultActions";
import { dispatchMenuAction } from "@/hooks/useNativeMenu";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { isMac } from "./combo";
import { isShortcutDispatch, KeyboardProvider, resolveScope, runShortcut } from "./useShortcuts";

// jsdom has no layout: CodeMirror measures through these.
beforeAll(() => {
  const rects = () => Object.assign([], { item: () => null }) as unknown as DOMRectList;
  Range.prototype.getClientRects ??= rects;
  Range.prototype.getBoundingClientRect ??= () => new DOMRect();
  Element.prototype.getClientRects ??= rects;
});

const offs: (() => void)[] = [];
afterEach(() => {
  offs.splice(0).forEach((off) => off());
  document.body.innerHTML = "";
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  useUiStore.setState({ view: "launcher" });
});

function register(id: ActionId) {
  const fn = vi.fn();
  offs.push(actions.register(id, fn));
  return fn;
}

/** A keydown with the platform's Mod key, as a user would press it. */
function press(target: Element, key: string, code: string, extra: KeyboardEventInit = {}) {
  const event = new KeyboardEvent("keydown", {
    key,
    code,
    metaKey: isMac,
    ctrlKey: !isMac,
    bubbles: true,
    cancelable: true,
    ...extra,
  });
  act(() => void target.dispatchEvent(event));
  return event;
}

/**
 * A small CodeMirror editor with its own ⌘↩ / ⌘S save (like Bootstrap commands, or the trigger
 * editor), mounted in `host`.
 */
function mountEditor(host: HTMLElement, save: () => void): EditorView {
  const view = new EditorView({
    parent: host,
    state: EditorState.create({
      doc: "select 1;",
      extensions: keymap.of(
        ["Mod-Enter", "Mod-s"].map((key) => ({
          key,
          run: () => {
            save();
            return true;
          },
        })),
      ),
    }),
  });
  offs.push(() => view.destroy());
  return view;
}

describe("resolveScope", () => {
  const html = (markup: string) => {
    document.body.innerHTML = markup;
    return document.querySelector("[data-target]")!;
  };

  it("gives a CodeMirror editor the editor scope, unless it sits in a dialog", () => {
    expect(resolveScope(html(`<div class="cm-editor"><div data-target></div></div>`))).toBe(
      "editor",
    );
    expect(
      resolveScope(
        html(`<div role="dialog"><div class="cm-editor"><div data-target></div></div></div>`),
      ),
    ).toBe("dialog");
    expect(
      resolveScope(
        html(`<div role="alertdialog"><div class="cm-editor"><div data-target></div></div></div>`),
      ),
    ).toBe("dialog");
  });

  it("lets a dialog keep its editor in the editor scope with data-shortcut-scope", () => {
    expect(
      resolveScope(
        html(
          `<div role="dialog" data-shortcut-scope="editor"><div class="cm-editor"><div data-target></div></div></div>`,
        ),
      ),
    ).toBe("editor");
    // Only for its editor: its buttons are still the dialog's.
    expect(
      resolveScope(
        html(`<div role="dialog" data-shortcut-scope="editor"><button data-target></button></div>`),
      ),
    ).toBe("dialog");
    // An opt-in outside the dialog does not count.
    expect(
      resolveScope(
        html(
          `<div data-shortcut-scope="editor"><div role="dialog"><div class="cm-editor"><div data-target></div></div></div></div>`,
        ),
      ),
    ).toBe("dialog");
  });

  it("falls back to data-scope, then global", () => {
    expect(resolveScope(html(`<div data-scope="grid"><span data-target></span></div>`))).toBe(
      "grid",
    );
    expect(resolveScope(html(`<span data-target></span>`))).toBe("global");
    expect(resolveScope(null)).toBe("global");
  });
});

describe("an editor inside a dialog", () => {
  function setup() {
    const save = vi.fn();
    const runCurrent = register("query.run-current");
    const querySave = register("query.save");
    const open = register("query.open");
    const palette = register("palette.open");
    render(
      <KeyboardProvider>
        <div role="dialog" data-testid="dialog">
          <div data-testid="host" />
        </div>
      </KeyboardProvider>,
    );
    const view = mountEditor(document.querySelector<HTMLElement>("[data-testid=host]")!, save);
    act(() => view.focus());
    return { view, save, runCurrent, querySave, open, palette };
  }

  it("keeps the query tab's keys away and leaves ⌘↩ / ⌘S to the editor's own keymap", () => {
    const { view, save, runCurrent, querySave, open, palette } = setup();
    expect(resolveScope(view.contentDOM)).toBe("dialog");
    press(view.contentDOM, "Enter", "Enter");
    expect(runCurrent).not.toHaveBeenCalled();
    expect(save).toHaveBeenCalledTimes(1);
    press(view.contentDOM, "s", "KeyS");
    expect(querySave).not.toHaveBeenCalled();
    expect(save).toHaveBeenCalledTimes(2);
    press(view.contentDOM, "o", "KeyO");
    press(view.contentDOM, "p", "KeyP");
    expect(open).not.toHaveBeenCalled();
    expect(palette).not.toHaveBeenCalled();
  });

  it("does not close the tab behind it with ⌘W, from the key or the menu", async () => {
    renderHook(() => useDefaultActions());
    // ⌘W closes the tab of the workspace on screen (not the Connection Manager's).
    useUiStore.setState({ view: "workspace" });
    const tab = useTabsStore.getState().newQuery("c1");
    const { view } = setup();
    press(view.contentDOM, "w", "KeyW");
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual([tab]);
    await act(() => dispatchMenuAction("tab.close"));
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual([tab]);
    // Outside the dialog ⌘W closes it as usual.
    act(() => view.contentDOM.blur());
    await act(() => actions.run("tab.close"));
    expect(useTabsStore.getState().tabs).toEqual([]);
  });

  it("keeps the native menu's editor items out of it", async () => {
    const { open } = setup();
    await expect(dispatchMenuAction("query.open")).resolves.toBe(false);
    await expect(dispatchMenuAction("palette.open")).resolves.toBe(false);
    expect(open).not.toHaveBeenCalled();
  });
});

describe("a grid key the grid does not handle", () => {
  /** A focused grid in a query tab that forwards editor keys from it (as QueryTab does). */
  function setup() {
    const forwarded: string[] = [];
    render(
      <KeyboardProvider>
        <div
          onKeyDownCapture={(e) => {
            if (!e.defaultPrevented) forwarded.push(e.key);
          }}
        >
          <div data-scope="grid" tabIndex={0} data-testid="grid" />
        </div>
      </KeyboardProvider>,
    );
    const grid = document.querySelector<HTMLElement>("[data-testid=grid]")!;
    act(() => grid.focus());
    return { grid, forwarded };
  }

  it("is spent in a read-only grid: no Beautify / Split Pane from the menu or the tab", () => {
    const { grid, forwarded } = setup();
    // ⌘I New Row and ⌥⌘D Set DEFAULT have no handler in a read-only result grid.
    expect(press(grid, "i", "KeyI").defaultPrevented).toBe(true);
    expect(press(grid, "d", "KeyD", { altKey: true }).defaultPrevented).toBe(true);
    expect(forwarded).toEqual([]);
    // ⌘↩ stays with the query tab (Run Current) while the grid does not bind Quick Look.
    expect(press(grid, "Enter", "Enter").defaultPrevented).toBe(false);
    expect(forwarded).toEqual(["Enter"]);
  });

  it("runs the grid's handler when there is one", () => {
    const newRow = register("grid.new-row");
    const { grid, forwarded } = setup();
    expect(press(grid, "i", "KeyI").defaultPrevented).toBe(true);
    expect(newRow).toHaveBeenCalledTimes(1);
    expect(forwarded).toEqual([]);
  });

  it("is left alone outside a grid", () => {
    render(
      <KeyboardProvider>
        <button type="button" data-testid="plain" />
      </KeyboardProvider>,
    );
    const plain = document.querySelector<HTMLElement>("[data-testid=plain]")!;
    expect(press(plain, "i", "KeyI").defaultPrevented).toBe(false);
  });
});

describe("runShortcut", () => {
  it("tells the handler it runs from its key, and nothing else does", async () => {
    const seen: boolean[] = [];
    offs.push(actions.register("admin.process-list", () => void seen.push(isShortcutDispatch())));
    runShortcut("admin.process-list");
    await actions.run("admin.process-list");
    expect(seen).toEqual([true, false]);
    expect(isShortcutDispatch()).toBe(false);
  });
});

describe("⌘↩, ⌘R and the comment keys", () => {
  function mount(markup: string) {
    render(
      <KeyboardProvider>
        <div dangerouslySetInnerHTML={{ __html: markup }} />
      </KeyboardProvider>,
    );
    const target = document.querySelector<HTMLElement>("[data-testid=target]")!;
    act(() => target.focus());
    return target;
  }

  it("opens the SQL editor with ⌘↩ from the tab's chrome, not from the sidebar", () => {
    const openEditor = register("query.open-editor");
    const button = mount(`<button type="button" data-testid="target">Refresh</button>`);
    expect(press(button, "Enter", "Enter").defaultPrevented).toBe(true);
    expect(openEditor).toHaveBeenCalledTimes(1);
    document.body.innerHTML = "";
    // The History / Queries panes open their row with ⌘↩ themselves.
    const row = mount(`<div data-scope="sidebar"><button data-testid="target">q</button></div>`);
    expect(press(row, "Enter", "Enter").defaultPrevented).toBe(false);
    expect(openEditor).toHaveBeenCalledTimes(1);
  });

  it("splits the workspace with ⇧⌘D from the chrome, the grid, the sidebar and the filter bar", () => {
    const split = register("workspace.split-right");
    const shiftD = { shiftKey: true };
    const button = mount(`<button type="button" data-testid="target">Tab</button>`);
    expect(press(button, "D", "KeyD", shiftD).defaultPrevented).toBe(true);
    expect(split).toHaveBeenCalledTimes(1);
    let calls = 1;
    // A grid (read-only or the structure tab: nothing there handles Set DEFAULT), the sidebar and
    // the filter bar's own fields: the key splits from all of them.
    for (const scope of ["grid", "sidebar", "filter"]) {
      document.body.innerHTML = "";
      const field = mount(
        `<div data-scope="${scope}"><input data-testid="target" aria-label="field" /></div>`,
      );
      expect(press(field, "D", "KeyD", shiftD).defaultPrevented, scope).toBe(true);
      expect(split, scope).toHaveBeenCalledTimes(++calls);
    }
  });

  it("keeps ⇧⌘D away from a cell editor and a dialog (splitting remounts the tab under them)", () => {
    const split = register("workspace.split-right");
    for (const markup of [
      `<div data-scope="grid-edit"><input data-testid="target" aria-label="cell" /></div>`,
      `<div role="dialog"><input data-testid="target" aria-label="field" /></div>`,
    ]) {
      document.body.innerHTML = "";
      const field = mount(markup);
      expect(press(field, "D", "KeyD", { shiftKey: true }).defaultPrevented, markup).toBe(false);
    }
    expect(split).not.toHaveBeenCalled();
  });

  it("runs Set DEFAULT with ⌥⌘D in the grid, and ⇧⌘D in the SQL editor still splits its pane", () => {
    const split = register("workspace.split-right");
    const setDefault = register("grid.set-default");
    const querySplit = register("query.split");
    const grid = mount(`<div data-scope="grid" data-testid="target" tabindex="0"></div>`);
    expect(press(grid, "d", "KeyD", { altKey: true }).defaultPrevented).toBe(true);
    expect(setDefault).toHaveBeenCalledTimes(1);
    expect(press(grid, "D", "KeyD", { shiftKey: true }).defaultPrevented).toBe(true);
    expect(setDefault).toHaveBeenCalledTimes(1);
    expect(split).toHaveBeenCalledTimes(1);

    const host = mount(`<div data-testid="target" tabindex="0"></div>`);
    const view = new EditorView({ parent: host, state: EditorState.create({ doc: "select 1;" }) });
    offs.push(() => view.destroy());
    act(() => view.focus());
    expect(press(view.contentDOM, "D", "KeyD", { shiftKey: true }).defaultPrevented).toBe(true);
    expect(querySplit).toHaveBeenCalledTimes(1);
    expect(split).toHaveBeenCalledTimes(1);
  });

  it("opens a SQL editor with ⌘E from the SQL editor, where ⌥⌘E is Explain", () => {
    const newQuery = register("tab.new-query");
    const explain = register("query.explain");
    const host = mount(`<div data-testid="target" tabindex="0"></div>`);
    const view = new EditorView({ parent: host, state: EditorState.create({ doc: "select 1;" }) });
    offs.push(() => view.destroy());
    act(() => view.focus());
    expect(press(view.contentDOM, "e", "KeyE").defaultPrevented).toBe(true);
    expect(newQuery).toHaveBeenCalledTimes(1);
    expect(explain).not.toHaveBeenCalled();
    expect(press(view.contentDOM, "e", "KeyE", { altKey: true }).defaultPrevented).toBe(true);
    expect(explain).toHaveBeenCalledTimes(1);
    expect(newQuery).toHaveBeenCalledTimes(1);
  });

  it("previews and discards pending changes from the SQL editor, and swallows nothing without any", () => {
    const host = mount(`<div data-testid="target" tabindex="0"></div>`);
    const view = new EditorView({ parent: host, state: EditorState.create({ doc: "select 1;" }) });
    offs.push(() => view.destroy());
    act(() => view.focus());
    const shiftP = ["P", "KeyP", { shiftKey: true }] as const;
    const shiftBackspace = ["Backspace", "Backspace", { shiftKey: true }] as const;
    // Nothing pending: no handler, the key falls through to CodeMirror untouched.
    expect(press(view.contentDOM, ...shiftP).defaultPrevented).toBe(false);
    expect(press(view.contentDOM, ...shiftBackspace).defaultPrevented).toBe(false);
    // Staged changes elsewhere in the workspace register the handlers.
    const preview = register("grid.preview");
    const discard = register("grid.discard");
    expect(press(view.contentDOM, ...shiftP).defaultPrevented).toBe(true);
    expect(preview).toHaveBeenCalledTimes(1);
    expect(press(view.contentDOM, ...shiftBackspace).defaultPrevented).toBe(true);
    expect(discard).toHaveBeenCalledTimes(1);
  });

  it("copies the cell value with ⇧⌘C in a grid and toggles the console log everywhere else", () => {
    const copyCell = register("grid.copy-cell");
    const console_ = register("ui.toggle-console");
    const shiftC = { shiftKey: true };
    const grid = mount(`<div data-scope="grid" data-testid="target" tabindex="0"></div>`);
    expect(press(grid, "C", "KeyC", shiftC).defaultPrevented).toBe(true);
    expect(copyCell).toHaveBeenCalledTimes(1);
    expect(console_).not.toHaveBeenCalled();
    let toggles = 0;
    for (const markup of [
      `<button type="button" data-testid="target">chrome</button>`,
      `<div data-scope="sidebar"><button data-testid="target">item</button></div>`,
      `<div data-scope="filter"><input data-testid="target" aria-label="filter" /></div>`,
    ]) {
      document.body.innerHTML = "";
      const target = mount(markup);
      expect(press(target, "C", "KeyC", shiftC).defaultPrevented, markup).toBe(true);
      expect(console_, markup).toHaveBeenCalledTimes(++toggles);
    }
    expect(copyCell).toHaveBeenCalledTimes(1);
  });

  it("⇧⌘C in a grid area with nothing to copy (the structure tab) still toggles the console", () => {
    const console_ = register("ui.toggle-console");
    const grid = mount(`<div data-scope="grid" data-testid="target" tabindex="0"></div>`);
    expect(press(grid, "C", "KeyC", { shiftKey: true }).defaultPrevented).toBe(true);
    expect(console_).toHaveBeenCalledTimes(1);
    // Neither handler: the key is spent in a grid like any grid key it refuses.
    document.body.innerHTML = "";
    offs.splice(0).forEach((off) => off());
    const bare = mount(`<div data-scope="grid" data-testid="target" tabindex="0"></div>`);
    expect(press(bare, "C", "KeyC", { shiftKey: true }).defaultPrevented).toBe(true);
  });

  it("leaves the comment keys to the focused CodeMirror editor", () => {
    const comment = register("query.comment-line");
    const host = mount(`<div data-testid="target" tabindex="0"></div>`);
    const view = new EditorView({
      parent: host,
      state: EditorState.create({ doc: "select 1;" }),
    });
    offs.push(() => view.destroy());
    act(() => view.focus());
    press(view.contentDOM, "/", "Slash");
    // The registry's handler (the Edit menu item's) did not run: the editor's keymap has the key.
    expect(comment).not.toHaveBeenCalled();
  });

  it("keeps ⌘R and ⌥⌘R from reloading the webview even with nothing to reload", () => {
    const button = mount(`<button type="button" data-testid="target">x</button>`);
    expect(press(button, "r", "KeyR").defaultPrevented).toBe(true);
    expect(press(button, "r", "KeyR", { altKey: true }).defaultPrevented).toBe(true);
  });
});
