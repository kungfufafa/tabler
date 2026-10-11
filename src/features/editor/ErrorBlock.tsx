import { CircleX } from "lucide-react";
import type { ErrorPayload } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { toCurrentDoc, useDocVersion } from "./docAnchors";
import { focusEditor, getEditorState } from "./editorStates";
import type { DocRange } from "./errorPos";

/** 1-based line / column of a document offset in the tab's editor. */
function docLocation(tabId: string, offset: number): { line: number; column: number } | null {
  const state = getEditorState(tabId);
  if (!state || offset > state.doc.length) return null;
  const line = state.doc.lineAt(offset);
  return { line: line.number, column: offset - line.from + 1 };
}

/**
 * A database error in red. Clicking it selects the offending range in the editor (unless the user
 * is selecting the message text to copy it). `range` is in the document as it was submitted
 * (`submission` is the run / explain id); edits made since are mapped, so the location follows the
 * text it points at.
 */
export function ErrorBlock({
  tabId,
  submission,
  error,
  range: submitted,
  className,
}: {
  tabId: string;
  submission: string;
  error: ErrorPayload;
  range: DocRange | null;
  className?: string;
}) {
  useDocVersion(tabId);
  const range = submitted ? toCurrentDoc(tabId, submission, submitted) : null;
  const loc = range ? docLocation(tabId, range.from) : null;
  const go = () => {
    if (range) focusEditor(tabId, range);
  };
  const meta = [
    loc && `Line ${loc.line}, column ${loc.column}`,
    error.sqlstate && `SQLSTATE ${error.sqlstate}`,
    error.vendorCode !== null && `Error ${error.vendorCode}`,
    error.code !== "DB_ERROR" && error.code,
  ].filter(Boolean);
  return (
    <div
      role={range ? "button" : undefined}
      tabIndex={range ? 0 : undefined}
      title={range ? "Show in editor" : undefined}
      onClick={() => {
        if (window.getSelection()?.toString()) return;
        go();
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          go();
        }
      }}
      className={cn(
        "border-error/30 bg-error/10 text-error rounded-md border px-2 py-1.5 text-xs outline-none",
        range &&
          "hover:bg-error/15 focus-visible:ring-error/40 cursor-pointer focus-visible:ring-2",
        className,
      )}
    >
      <div className="flex items-start gap-1.5">
        <CircleX className="mt-0.5 size-3.5 shrink-0" />
        <div className="selectable min-w-0 flex-1 break-words whitespace-pre-wrap">
          {error.message}
          {error.detail && <div className="text-error/80 mt-0.5">Detail: {error.detail}</div>}
          {error.hint && <div className="text-error/80 mt-0.5">Hint: {error.hint}</div>}
        </div>
      </div>
      {meta.length > 0 && (
        <div className="text-error/75 mt-0.5 pl-5 text-[10px] tabular-nums">{meta.join(" · ")}</div>
      )}
    </div>
  );
}
