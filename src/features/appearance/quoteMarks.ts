import { syntaxTree } from "@codemirror/language";
import { RangeSetBuilder, type Extension } from "@codemirror/state";
import {
  Decoration,
  ViewPlugin,
  type DecorationSet,
  type EditorView,
  type ViewUpdate,
} from "@codemirror/view";

/**
 * Settings ▸ Appearance colours single, double, backtick and bracket quoted text separately
 * (TablePlus's Fonts & Colors). The SQL grammar only says "String" or "QuotedIdentifier", so the
 * first character decides: this plugin marks everything but the single-quoted kind (that one keeps
 * the highlight style's `--syn-string`) with a class `cmTheme` colours from its own token.
 */

export type QuoteKind = "single" | "double" | "backtick" | "bracket";

/** The class a quote kind is marked with (single quotes need none). */
export const QUOTE_CLASS: Record<Exclude<QuoteKind, "single">, string> = {
  double: "cm-q-double",
  backtick: "cm-q-backtick",
  bracket: "cm-q-bracket",
};

/** The quote kind of a `String` / `QuotedIdentifier` node whose text starts with `first`. */
export function quoteKind(nodeName: string, first: string): QuoteKind | null {
  if (nodeName === "QuotedIdentifier") {
    if (first === '"') return "double";
    if (first === "`") return "backtick";
    if (first === "[") return "bracket";
    return null;
  }
  if (nodeName === "String") return first === '"' ? "double" : "single";
  return null;
}

const marks = {
  double: Decoration.mark({ class: QUOTE_CLASS.double }),
  backtick: Decoration.mark({ class: QUOTE_CLASS.backtick }),
  bracket: Decoration.mark({ class: QUOTE_CLASS.bracket }),
} as const;

function build(view: EditorView): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  const tree = syntaxTree(view.state);
  for (const { from, to } of view.visibleRanges) {
    tree.iterate({
      from,
      to,
      enter(node) {
        if (node.name !== "String" && node.name !== "QuotedIdentifier") return;
        const kind = quoteKind(node.name, view.state.doc.sliceString(node.from, node.from + 1));
        if (kind && kind !== "single") builder.add(node.from, node.to, marks[kind]);
        return false;
      },
    });
  }
  return builder.finish();
}

/** Marks double / backtick / bracket quoted text so the theme can colour each kind. */
export const quoteKindMarks: Extension = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = build(view);
    }
    update(u: ViewUpdate) {
      if (u.docChanged || u.viewportChanged || syntaxTree(u.state) !== syntaxTree(u.startState))
        this.decorations = build(u.view);
    }
  },
  { decorations: (v) => v.decorations },
);
