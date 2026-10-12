import { EditorView } from "@codemirror/view";
import { act } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { actions, type ActionId } from "@/features/palette/actions";
import { isMac } from "@/lib/keys/combo";
import { applyKeymapOverrides } from "@/lib/keys/keymap";
import { EMPTY_OVERRIDES } from "@/lib/keys/overrides";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import { dropEditor, registerView, takeEditorState } from "./editorStates";

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
  resetPrefsForTests();
  applyKeymapOverrides(EMPTY_OVERRIDES);
});

function register(id: ActionId) {
  const fn = vi.fn();
  offs.push(actions.register(id, fn));
  return fn;
}

/**
 * A query tab's editor (outside the KeyboardProvider, so only CodeMirror sees the keys), mounted
 * the way EditorPanes does it: from the tab's stored state, registered while shown.
 */
function mount(key = "cm-test"): EditorView {
  const view = new EditorView({ parent: document.body, state: takeEditorState(key).state });
  const unregister = registerView(key, view);
  offs.push(() => {
    unregister();
    view.destroy();
    dropEditor(key);
  });
  return view;
}

function press(view: EditorView, key: string, code: string, extra: KeyboardEventInit = {}) {
  const event = new KeyboardEvent("keydown", {
    key,
    code,
    metaKey: isMac,
    ctrlKey: !isMac,
    bubbles: true,
    cancelable: true,
    ...extra,
  });
  act(() => void view.contentDOM.dispatchEvent(event));
  return event;
}

describe("the editor's query shortcuts", () => {
  it("follow a remapped or removed binding in an editor that is already open", () => {
    const format = register("query.format");
    const view = mount();
    press(view, "i", "KeyI");
    expect(format).toHaveBeenCalledTimes(1);

    setPref("keymapOverrides", { "query.format": ["Mod+U", "Alt+Shift+F"] });
    press(view, "i", "KeyI");
    expect(format).toHaveBeenCalledTimes(1);
    press(view, "u", "KeyU");
    expect(format).toHaveBeenCalledTimes(2);

    setPref("keymapOverrides", { "query.format": ["", ""] });
    press(view, "u", "KeyU");
    press(view, "i", "KeyI");
    press(view, "F", "KeyF", { metaKey: false, ctrlKey: false, altKey: true, shiftKey: true });
    expect(format).toHaveBeenCalledTimes(2);
  });

  it("follow a remapping made while the tab was not shown", () => {
    const format = register("query.format");
    takeEditorState("cm-hidden");
    setPref("keymapOverrides", { "query.format": ["Mod+U", "Alt+Shift+F"] });
    const view = mount("cm-hidden");
    press(view, "i", "KeyI");
    expect(format).not.toHaveBeenCalled();
    press(view, "u", "KeyU");
    expect(format).toHaveBeenCalledTimes(1);
  });

  it("⌘. cancels a running query, and opens the Process List when nothing runs", () => {
    const processList = register("admin.process-list");
    const view = mount();
    press(view, ".", "Period");
    expect(processList).toHaveBeenCalledTimes(1);
    const cancel = register("query.cancel");
    press(view, ".", "Period");
    expect(cancel).toHaveBeenCalledTimes(1);
    expect(processList).toHaveBeenCalledTimes(1);
  });

  it("keeps ⌘↩ from inserting a blank line and ⌘R from reloading the page", () => {
    const view = mount();
    const run = press(view, "Enter", "Enter");
    expect(run.defaultPrevented).toBe(true);
    expect(view.state.doc.toString()).toBe("");
    expect(press(view, "r", "KeyR").defaultPrevented).toBe(true);
  });
});
