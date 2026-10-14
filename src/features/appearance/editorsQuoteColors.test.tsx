import { EditorView } from "@codemirror/view";
import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { BootstrapCommandsDialog } from "@/features/connections/BootstrapCommandsDialog";
import { editorExtensions, editorPrefsOf, languagePrefsOf } from "@/features/editor/cmExtensions";
import { editorTheme, sqlEditorTheme } from "@/features/editor/cmTheme";
import { BoardSqlEditor } from "@/features/metrics/BoardSqlEditor";
import { StructureSqlEditor } from "@/features/structure/StructureSqlEditor";
import { createViewerState } from "@/features/sqlviewer/buffers";
import { getPrefs } from "@/stores/prefs";
import { QUOTE_CLASS } from "./quoteMarks";

/**
 * Settings ▸ Appearance ▸ SQL editor colours double quotes, backticks and brackets separately. It
 * has to reach every CodeMirror SQL editor of the app, not only the query tab's: they all take
 * the look from the one `sqlEditorTheme` extension.
 */

beforeAll(() => {
  // jsdom has no layout; CodeMirror measures text with these.
  const rects = () => Object.assign([], { item: () => null }) as unknown as DOMRectList;
  Range.prototype.getClientRects ??= rects;
  Range.prototype.getBoundingClientRect ??= () => new DOMRect();
  Element.prototype.getClientRects ??= rects;
});

const views: EditorView[] = [];
afterEach(() => views.splice(0).forEach((v) => v.destroy()));

const SQL = "SELECT \"my col\", `back tick`, 'plain' FROM t";
const quoted = (root: ParentNode, kind: keyof typeof QUOTE_CLASS) =>
  [...root.querySelectorAll(`.${QUOTE_CLASS[kind]}`)].map((el) => el.textContent);

describe("the shared SQL editor theme", () => {
  it("is the chrome plus the syntax colours plus the quote kind marks", () => {
    expect(Array.isArray(sqlEditorTheme)).toBe(true);
    expect(sqlEditorTheme as unknown[]).toContain(editorTheme);
    expect((sqlEditorTheme as unknown[]).length).toBe(3);
  });
});

describe("the double-quote and backtick colours reach the other CodeMirror editors", () => {
  it("the Metrics board's inspector SQL editor", () => {
    const { container } = render(
      <BoardSqlEditor
        value={SQL}
        dialect="mysql"
        label="Query"
        hint="SELECT …"
        onChange={() => {}}
        onRun={() => {}}
      />,
    );
    expect(quoted(container, "double")).toEqual(['"my col"']);
    expect(quoted(container, "backtick")).toEqual(["`back tick`"]);
  });

  it("the Structure tab's SQL editor", () => {
    const { container } = render(
      <StructureSqlEditor
        value={SQL}
        resetKey={0}
        onChange={() => {}}
        dialect="mysql"
        label="Definition"
      />,
    );
    expect(quoted(container, "double")).toEqual(['"my col"']);
    expect(quoted(container, "backtick")).toEqual(["`back tick`"]);
  });

  it("the connection's bootstrap commands editor", () => {
    render(
      <BootstrapCommandsDialog
        open
        value={SQL}
        driver="mysql"
        onOpenChange={() => {}}
        onSave={() => {}}
      />,
    );
    const editor = screen.getByTestId("bootstrap-editor");
    expect(quoted(editor, "double")).toEqual(['"my col"']);
    expect(quoted(editor, "backtick")).toEqual(["`back tick`"]);
  });

  it("the SQL Viewer, and so the query tab: both are built from the same editor extensions", () => {
    const state = createViewerState("viewer-quotes", SQL, "mysql");
    const view = new EditorView({ parent: document.body, state });
    views.push(view);
    expect(quoted(view.dom, "double")).toEqual(['"my col"']);
    expect(quoted(view.dom, "backtick")).toEqual(["`back tick`"]);
  });

  it("the query tab's own extension list", () => {
    const p = getPrefs();
    const view = new EditorView({
      parent: document.body,
      doc: SQL,
      extensions: editorExtensions({
        language: { dialect: "mysql", ...languagePrefsOf(p) },
        prefs: editorPrefsOf(p),
        onUpdate: vi.fn(),
      }),
    });
    views.push(view);
    act(() => undefined);
    expect(quoted(view.dom, "double")).toEqual(['"my col"']);
    expect(quoted(view.dom, "backtick")).toEqual(["`back tick`"]);
  });
});
