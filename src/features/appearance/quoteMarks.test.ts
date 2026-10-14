import { MySQL, PostgreSQL, sql } from "@codemirror/lang-sql";
import { syntaxHighlighting } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { sqlHighlightStyle } from "@/features/editor/cmTheme";
import { QUOTE_CLASS, quoteKind, quoteKindMarks } from "./quoteMarks";

beforeAll(() => {
  const rects = () => Object.assign([], { item: () => null }) as unknown as DOMRectList;
  Range.prototype.getClientRects ??= rects;
  Range.prototype.getBoundingClientRect ??= () => new DOMRect();
  Element.prototype.getClientRects ??= rects;
});

const views: EditorView[] = [];
afterEach(() => views.splice(0).forEach((v) => v.destroy()));

function mount(doc: string, dialect = PostgreSQL): EditorView {
  const view = new EditorView({
    parent: document.body,
    state: EditorState.create({
      doc,
      extensions: [sql({ dialect }), syntaxHighlighting(sqlHighlightStyle), quoteKindMarks],
    }),
  });
  views.push(view);
  return view;
}

const marked = (view: EditorView, cls: string) =>
  [...view.dom.querySelectorAll(`.${cls}`)].map((el) => el.textContent);

describe("quoteKind", () => {
  it("tells the quote kinds apart by the first character", () => {
    expect(quoteKind("String", "'")).toBe("single");
    expect(quoteKind("String", '"')).toBe("double");
    expect(quoteKind("String", "N")).toBe("single");
    expect(quoteKind("QuotedIdentifier", '"')).toBe("double");
    expect(quoteKind("QuotedIdentifier", "`")).toBe("backtick");
    expect(quoteKind("QuotedIdentifier", "[")).toBe("bracket");
  });

  it("ignores other nodes", () => {
    expect(quoteKind("Keyword", "S")).toBeNull();
    expect(quoteKind("Identifier", "a")).toBeNull();
    expect(quoteKind("QuotedIdentifier", "x")).toBeNull();
  });
});

describe("the quote kind marks", () => {
  it("marks double-quoted identifiers (PostgreSQL) and leaves single quotes to the highlighter", () => {
    const view = mount(`SELECT "my col", 'text' FROM "t"`);
    expect(marked(view, QUOTE_CLASS.double)).toEqual(['"my col"', '"t"']);
    expect(marked(view, QUOTE_CLASS.backtick)).toEqual([]);
    expect(view.dom.textContent).toContain("'text'");
  });

  it("marks backtick identifiers (MySQL)", () => {
    const view = mount("SELECT `a b`, `c` FROM t", MySQL);
    expect(marked(view, QUOTE_CLASS.backtick)).toEqual(["`a b`", "`c`"]);
  });

  it("marks double-quoted strings in MySQL, where they are strings", () => {
    const view = mount(`SELECT "text", 'other'`, MySQL);
    expect(marked(view, QUOTE_CLASS.double)).toEqual(['"text"']);
  });

  it("follows edits", () => {
    const view = mount(`SELECT 1`);
    expect(marked(view, QUOTE_CLASS.double)).toEqual([]);
    view.dispatch({ changes: { from: view.state.doc.length, insert: ` "x"` } });
    expect(marked(view, QUOTE_CLASS.double)).toEqual(['"x"']);
  });
});
