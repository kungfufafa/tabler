import { EditorState } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { act, render, renderHook } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { actions, type ActionId } from "@/features/palette/actions";
import { useDefaultActions } from "@/features/palette/defaultActions";
import { dispatchMenuAction } from "@/hooks/useNativeMenu";
import { useTabsStore } from "@/stores/tabs";
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
