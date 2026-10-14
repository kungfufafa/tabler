import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cyclePane, focusTarget, paneOf, visiblePanes } from "./panes";

describe("pane navigation", () => {
  let root: HTMLElement;
  const mount = (html: string) => {
    root.innerHTML = html;
  };
  beforeEach(() => {
    root = document.createElement("div");
    document.body.appendChild(root);
  });
  afterEach(() => root.remove());

  const SHELL = `
    <div data-panel id="sidebar"><input aria-label="Search" /><button data-row-index="0">a</button>
      <button data-row-index="1" data-selected="true">b</button></div>
    <div data-panel id="content"><div data-scope="grid" tabindex="0" aria-label="grid"></div></div>
    <div data-panel id="inspector"><button>Field</button></div>
    <div data-panel id="console"></div>`;

  it("lists the panes on screen in reading order", () => {
    mount(`<div data-panel id="console"></div><div data-panel id="sidebar"></div>`);
    expect(visiblePanes(root).map((p) => p.id)).toEqual(["sidebar", "console"]);
  });

  it("picks the editor, the grid, the selected sidebar item or the first control", () => {
    mount(SHELL);
    const pane = (id: string) => root.querySelector<HTMLElement>(`[id="${id}"]`)!;
    expect(focusTarget(pane("sidebar")).textContent).toBe("b");
    expect(focusTarget(pane("content")).getAttribute("aria-label")).toBe("grid");
    expect(focusTarget(pane("inspector")).textContent).toBe("Field");
    expect(focusTarget(pane("console"))).toBe(pane("console"));
    mount(`<div data-panel id="content"><div class="cm-content" contenteditable="true"></div>
      <div data-scope="grid" tabindex="0"></div></div>`);
    expect(focusTarget(pane("content")).className).toBe("cm-content");
  });

  it("cycles sidebar → tab content → row detail → console and wraps around", () => {
    mount(SHELL);
    expect(cyclePane(1, root)).toBe("sidebar");
    expect(paneOf(document.activeElement, root)).toBe("sidebar");
    expect(cyclePane(1, root)).toBe("content");
    expect(cyclePane(1, root)).toBe("inspector");
    expect(cyclePane(1, root)).toBe("console");
    // An empty pane takes the focus itself.
    expect(document.activeElement?.id).toBe("console");
    expect(cyclePane(1, root)).toBe("sidebar");
    expect(cyclePane(-1, root)).toBe("console");
    expect(cyclePane(-1, root)).toBe("inspector");
  });

  it("skips hidden panes and starts from the last one going backwards", () => {
    mount(`<div data-panel id="sidebar"><button data-row-index="0">a</button></div>
      <div data-panel id="content"><div data-scope="grid" tabindex="0"></div></div>`);
    (document.activeElement as HTMLElement | null)?.blur();
    expect(cyclePane(-1, root)).toBe("content");
    expect(cyclePane(1, root)).toBe("sidebar");
    mount("");
    expect(cyclePane(1, root)).toBeNull();
  });
});
