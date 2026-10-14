import { useLayoutEffect, useRef, useState, type RefObject } from "react";
import { createPortal } from "react-dom";
import { cn } from "@/lib/utils";
import type { EditorExit } from "./registry";
import { commitTabSoon, isSaveKey } from "./saveKey";
import { insertAtCaret, useGrabFocus, type CaretPlacement } from "./useGrabFocus";

const MIN_W = 360;
const TEXT_H = 160;
const FOOTER_H = 22;
const MARGIN = 8;

interface Placement {
  left: number;
  top: number;
  width: number;
}

/** Where the panel goes: over the cell, at least 360px wide, kept (or flipped) inside the viewport. */
function place(anchor: DOMRect): Placement {
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  const width = Math.max(200, Math.min(Math.max(anchor.width, MIN_W), vw - 2 * MARGIN));
  const height = TEXT_H + FOOTER_H + 4;
  const left = Math.max(MARGIN, Math.min(anchor.left, vw - width - MARGIN));
  let top = anchor.top;
  if (top + height > vh - MARGIN) top = anchor.bottom - height; // flip: grow upwards from the cell
  top = Math.max(MARGIN, Math.min(top, vh - height - MARGIN));
  return { left, top, width };
}

/**
 * Multi-line text editor anchored to the cell. Rendered in a portal (the grid scroller has
 * `contain: strict`, which would clip it). Line breaks are kept exactly as typed.
 */
export function MultilinePanel({
  anchorRef,
  label,
  text,
  caret,
  mono,
  error,
  onTextChange,
  onCommit,
  onSave,
  onCancel,
}: {
  anchorRef: RefObject<HTMLElement | null>;
  label: string;
  text: string;
  caret: CaretPlacement;
  mono?: boolean;
  /** Validation error from the last commit attempt, shown in the footer. */
  error?: string | null;
  onTextChange: (text: string) => void;
  onCommit: (exit: EditorExit) => boolean | void;
  /** ⌘S: stages the text and says whether it did, then the tab is committed (see `InlineInput`). */
  onSave?: () => boolean;
  onCancel: () => void;
}) {
  const ref = useRef<HTMLTextAreaElement>(null);
  // Off-screen until measured, but still focusable.
  const [pos, setPos] = useState<Placement>({ left: -10_000, top: 0, width: MIN_W });
  useLayoutEffect(() => {
    const anchor = anchorRef.current;
    if (anchor) setPos(place(anchor.getBoundingClientRect()));
  }, [anchorRef]);
  useGrabFocus(ref, caret);

  return createPortal(
    <div
      data-scope="grid-edit"
      className="border-brand bg-popover text-popover-foreground fixed z-50 flex flex-col overflow-hidden rounded-md border-2 shadow-lg"
      style={{ left: pos.left, top: pos.top }}
      onPointerDown={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.stopPropagation()}
    >
      <textarea
        ref={ref}
        value={text}
        aria-label={label}
        spellCheck={false}
        autoComplete="off"
        onChange={(e) => onTextChange(e.target.value)}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.nativeEvent.isComposing) return;
          if (isSaveKey(e.nativeEvent)) {
            e.preventDefault();
            if ((onSave ? onSave() : onCommit("blur")) !== false) commitTabSoon();
          } else if (e.key === "Enter" && e.altKey) {
            e.preventDefault();
            insertAtCaret(e.currentTarget, "\n");
          } else if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            onCommit("enter");
          } else if (e.key === "Tab") {
            e.preventDefault();
            onCommit(e.shiftKey ? "shiftTab" : "tab");
          } else if (e.key === "Escape") {
            e.preventDefault();
            onCancel();
          }
        }}
        onBlur={() => onCommit("blur")}
        className={cn(
          "bg-surface min-h-16 max-w-[90vw] min-w-48 resize px-2 py-1.5 text-[12.5px] leading-[18px] outline-none",
          mono && "font-mono",
        )}
        style={{ width: pos.width, height: TEXT_H, maxHeight: "70vh" }}
      />
      <div
        className={cn(
          "border-border bg-surface-2 flex shrink-0 items-center truncate border-t px-2 text-[11px]",
          error ? "text-error" : "text-muted-foreground",
        )}
        style={{ height: FOOTER_H }}
      >
        {error ?? "↩ save · ⌥↩ new line · esc cancel"}
      </div>
    </div>,
    document.body,
  );
}
