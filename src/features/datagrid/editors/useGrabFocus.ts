import { useEffect, type RefObject } from "react";

/** Where the caret goes when an editor takes focus: select everything, the end, or an offset. */
export type CaretPlacement = "all" | "end" | number;

type Focusable = HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement;

/**
 * Focuses a freshly mounted editor. A double-click's trailing click (or the grid refocusing itself)
 * can steal focus right after mount, so it tries again on the next frame and shortly after; the caret
 * is only placed when focus actually moves (never after the user started typing).
 */
export function useGrabFocus(ref: RefObject<Focusable | null>, caret: CaretPlacement = "end") {
  useEffect(() => {
    const grab = () => {
      const el = ref.current;
      if (!el || document.activeElement === el) return;
      el.focus({ preventScroll: true });
      if (el instanceof HTMLSelectElement) return;
      const len = el.value.length;
      if (caret === "all") el.select();
      else {
        const at = caret === "end" ? len : Math.max(0, Math.min(caret, len));
        el.setSelectionRange(at, at);
      }
    };
    grab();
    const id = requestAnimationFrame(grab);
    const id2 = window.setTimeout(grab, 50);
    return () => {
      cancelAnimationFrame(id);
      window.clearTimeout(id2);
    };
    // Mount-only: later caret changes belong to the user.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
}

/**
 * Inserts `insert` at the selection of a controlled input/textarea, as if typed: the browser's
 * `insertText` keeps native undo working; the fallback edits the value and fires `input` so React's
 * onChange sees it (writing the value directly would leave React's value tracker stale).
 */
export function insertAtCaret(el: HTMLInputElement | HTMLTextAreaElement, insert: string): void {
  const viaEditing = () => {
    try {
      return (
        typeof document.execCommand === "function" &&
        document.execCommand("insertText", false, insert)
      );
    } catch {
      return false;
    }
  };
  if (viaEditing()) return;
  const start = el.selectionStart ?? el.value.length;
  const end = el.selectionEnd ?? start;
  el.setRangeText(insert, start, end, "end");
  el.dispatchEvent(new Event("input", { bubbles: true }));
}
