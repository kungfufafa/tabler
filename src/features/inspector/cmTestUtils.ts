import { EditorView } from "@codemirror/view";
import { act, screen } from "@testing-library/react";

/** The CodeMirror content element of the field (or editor) with this accessible name. */
export function cmContent(name: string): HTMLElement {
  return screen.getByRole("textbox", { name });
}

/** The CodeMirror view of the field (or editor) with this accessible name. */
export function cmView(name: string): EditorView {
  const dom = cmContent(name).closest<HTMLElement>(".cm-editor");
  const view = dom && EditorView.findFromDOM(dom);
  if (!view) throw new Error(`no CodeMirror view named ${name}`);
  return view;
}

/** The text of the field (or editor) with this accessible name. */
export function cmDoc(name: string): string {
  return cmView(name).state.doc.toString();
}

/** Types `text` over the whole document, as the user would (one edit). */
export function cmType(name: string, text: string): void {
  const view = cmView(name);
  act(() => view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } }));
}

/** jsdom has no layout: CodeMirror measures through these when it moves the caret or focuses. */
export function polyfillLayout(): void {
  const rects = () => Object.assign([], { item: () => null }) as unknown as DOMRectList;
  Range.prototype.getClientRects ??= rects;
  Range.prototype.getBoundingClientRect ??= () => new DOMRect();
  Element.prototype.getClientRects ??= rects;
}
