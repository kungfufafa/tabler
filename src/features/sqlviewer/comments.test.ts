import { EditorView } from "@codemirror/view";
import { act } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { isMac } from "@/lib/keys/combo";
import { applyKeymapOverrides } from "@/lib/keys/keymap";
import { EMPTY_OVERRIDES } from "@/lib/keys/overrides";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import { createViewerState, dropViewerBuffer, registerViewerView } from "./buffers";

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
  document.body.innerHTML = "";
});

/** A keydown with the platform's Mod key, as CodeMirror sees it. */
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
}

function mount(doc: string): EditorView {
  const host = document.createElement("div");
  document.body.append(host);
  const view = new EditorView({ parent: host, state: createViewerState("t1", doc, "postgres") });
  offs.push(
    registerViewerView("t1", view),
    () => view.destroy(),
    () => dropViewerBuffer("t1"),
  );
  return view;
}

describe("Toggle Line / Block Comment (Keymap ▸ SQL Editor)", () => {
  it("runs ⌘/ and ⌥⌘/ in the focused editor", () => {
    const view = mount("select 1;");
    press(view, "/", "Slash");
    expect(view.state.doc.toString()).toBe("-- select 1;");
    press(view, "/", "Slash");
    expect(view.state.doc.toString()).toBe("select 1;");
    press(view, "/", "Slash", { altKey: true });
    expect(view.state.doc.toString()).toBe("/* select 1; */");
  });

  it("follows a remap: the new key comments, ⌘/ no longer does", () => {
    const view = mount("select 1;");
    setPref("keymapOverrides", { "query.comment-line": ["Mod+J"] });
    press(view, "/", "Slash");
    expect(view.state.doc.toString()).toBe("select 1;");
    press(view, "j", "KeyJ");
    expect(view.state.doc.toString()).toBe("-- select 1;");
  });
});
