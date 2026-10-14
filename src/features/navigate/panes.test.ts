import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cyclePane, focusTarget, paneOf, paneStops, visiblePanes } from "./panes";

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

  // A SQL tab with its editor split (⇧⌘D): the main editor, then the split one, in two panes of
  // the content panel (EditorPanes), and the results grid below.
  const SPLIT_EDITORS = `
    <div data-panel id="sidebar"><button data-row-index="0">a</button></div>
    <div data-panel id="content">
      <div data-pane="main"><div class="cm-editor"><div class="cm-content" contenteditable="true" id="main"></div></div></div>
      <div data-pane="split"><div class="cm-editor"><div class="cm-content" contenteditable="true" id="split"></div></div>
        <button aria-label="Close split pane">x</button></div>
      <div data-scope="grid" tabindex="0" id="results"></div>
    </div>
    <div data-panel id="inspector"><button id="field">Field</button></div>`;

  const focused = () => document.activeElement?.id;

  it("visits the main and the split editor before moving on", () => {
    mount(SPLIT_EDITORS);
    (document.activeElement as HTMLElement | null)?.blur();
    expect(paneStops(root).map((s) => [s.pane, s.target.id || s.target.textContent])).toEqual([
      ["sidebar", "a"],
      ["content", "main"],
      ["content", "split"],
      ["inspector", "field"],
    ]);
    const order: Array<string | undefined> = [];
    for (let i = 0; i < 5; i++) {
      expect(cyclePane(1, root)).toBe(["sidebar", "content", "content", "inspector", "sidebar"][i]);
      order.push(focused() || document.activeElement?.textContent || undefined);
    }
    expect(order).toEqual(
      ["", "main", "split", "field", ""].map((x, i) => (i === 0 || i === 4 ? "a" : x)),
    );
    // and backwards, through the split editor
    expect(cyclePane(-1, root)).toBe("inspector");
    expect(cyclePane(-1, root)).toBe("content");
    expect(focused()).toBe("split");
    expect(cyclePane(-1, root)).toBe("content");
    expect(focused()).toBe("main");
    expect(cyclePane(-1, root)).toBe("sidebar");
  });

  it("counts what sits in an editor's pane, or below the editors, as that stop", () => {
    mount(SPLIT_EDITORS);
    // a button inside the split pane: still the split stop, so next leaves for the row detail
    root.querySelector<HTMLElement>("button[aria-label='Close split pane']")!.focus();
    expect(cyclePane(1, root)).toBe("inspector");
    // the results grid below both editors belongs to the last one: previous is the main editor
    root.querySelector<HTMLElement>("#results")!.focus();
    expect(cyclePane(-1, root)).toBe("content");
    expect(focused()).toBe("main");
    root.querySelector<HTMLElement>("#results")!.focus();
    expect(cyclePane(1, root)).toBe("inspector");
  });

  it("a tab with one editor stays one stop", () => {
    mount(`<div data-panel id="content"><div class="cm-content" contenteditable="true" id="only"></div>
      <div data-scope="grid" tabindex="0" id="results"></div></div>
      <div data-panel id="inspector"><button>Field</button></div>`);
    expect(paneStops(root)).toHaveLength(2);
    expect(cyclePane(1, root)).toBe("content");
    expect(focused()).toBe("only");
    expect(cyclePane(1, root)).toBe("inspector");
  });

  // Split Right (⇧⌘D with tabs): two tab groups, each with its own tab content.
  const SPLIT_GROUPS = `
    <div data-panel id="sidebar"><button data-row-index="0">a</button></div>
    <div data-panel id="content">
      <div data-tab-group="0"><div data-scope="grid" tabindex="0" id="left"></div></div>
      <div data-tab-group="1"><div class="cm-content" contenteditable="true" id="right"></div></div>
    </div>
    <div data-panel id="console"></div>`;

  it("visits both groups of a split workspace, each with its own focus target", () => {
    mount(SPLIT_GROUPS);
    (document.activeElement as HTMLElement | null)?.blur();
    expect(cyclePane(1, root)).toBe("sidebar");
    expect(cyclePane(1, root)).toBe("content");
    expect(focused()).toBe("left");
    expect(cyclePane(1, root)).toBe("content");
    expect(focused()).toBe("right");
    expect(cyclePane(1, root)).toBe("console");
    expect(cyclePane(-1, root)).toBe("content");
    expect(focused()).toBe("right");
    expect(cyclePane(-1, root)).toBe("content");
    expect(focused()).toBe("left");
    expect(cyclePane(-1, root)).toBe("sidebar");
  });

  it("the tab bar and tab list marked data-tab-group inside a group are not stops of their own", () => {
    // WorkspaceGroups' exact nesting: the group pane, its tab bar and the bar's tab list all carry
    // the attribute
    const group = (g: number, body: string) => `
      <div data-slot="tab-group" data-tab-group="${g}">
        <div data-slot="tab-bar" data-tab-group="${g}"><button>Back</button>
          <div role="tablist" data-tab-group="${g}"><button>Close ${g}</button></div></div>
        <div>${body}</div></div>`;
    mount(`<div data-panel id="sidebar"><button data-row-index="0">a</button></div>
      <div data-panel id="content">${group(0, `<div data-scope="grid" tabindex="0" id="left"></div>`)}
        ${group(1, `<div data-scope="grid" tabindex="0" id="right"></div>`)}</div>
      <div data-panel id="inspector"><button id="field">Field</button></div>`);
    expect(paneStops(root).map((s) => s.zone.getAttribute("data-slot") ?? s.zone.id)).toEqual([
      "sidebar",
      "tab-group",
      "tab-group",
      "inspector",
    ]);
    (document.activeElement as HTMLElement | null)?.blur();
    const seen: Array<string | undefined> = [];
    for (let i = 0; i < 5; i++) {
      cyclePane(1, root);
      seen.push(focused() || document.activeElement?.textContent || undefined);
    }
    expect(seen).toEqual(["a", "left", "right", "field", "a"]);
    // focus on a bar button belongs to its group: next is the other group
    root.querySelector<HTMLElement>("[data-tab-group='0'] button")!.focus();
    cyclePane(1, root);
    expect(focused()).toBe("right");
  });

  it("a group with split editors adds its editors between the groups", () => {
    mount(`<div data-panel id="content">
      <div data-tab-group="0"><div data-pane="main"><div class="cm-content" contenteditable="true" id="a1"></div></div>
        <div data-pane="split"><div class="cm-content" contenteditable="true" id="a2"></div></div></div>
      <div data-tab-group="1"><div data-scope="grid" tabindex="0" id="b"></div></div></div>`);
    const seen: Array<string | undefined> = [];
    for (let i = 0; i < 4; i++) {
      cyclePane(1, root);
      seen.push(focused());
    }
    expect(seen).toEqual(["a1", "a2", "b", "a1"]);
  });
});
