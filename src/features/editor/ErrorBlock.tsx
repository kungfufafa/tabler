import { CircleX } from "lucide-react";
import type { ErrorPayload } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useDocVersion } from "./docAnchors";
import { focusEditor, getEditorState } from "./editorStates";
import type { DocRange } from "./errorPos";
import { locateSubmitted, submissionEditor, useSubmissionsVersion } from "./submissions";

/**
 * The app's own error codes (`AppError::code` in src-tauri/src/error.rs) said in plain words, for
 * the ones that tell the user something the message may not: where the failure was. The others
 * (DB_ERROR, QUERY_TIMEOUT, INTERNAL…) are left out; the server's SQLSTATE and error number stay,
 * as TablePlus shows the server's message without app-internal codes.
 */
const CODE_LABELS: Partial<Record<string, string>> = {
  NOT_CONNECTED: "Connection lost",
  CONNECT_FAILED: "Could not reach the server",
  AUTH_FAILED: "Sign-in failed",
  TIMEOUT: "The server stopped answering",
  SSH_TUNNEL: "SSH tunnel",
};

/** 1-based line / column of a document offset in an editor. */
function docLocation(key: string, offset: number): { line: number; column: number } | null {
  const state = getEditorState(key);
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
  // The pane the SQL was run from (the editor may be split), followed through later edits.
  useSubmissionsVersion();
  const key = submissionEditor(tabId, submission);
  useDocVersion(key);
  const range = submitted ? (locateSubmitted(tabId, submission, submitted)?.range ?? null) : null;
  const loc = range ? docLocation(key, range.from) : null;
  const go = () => {
    if (range) focusEditor(key, range);
  };
  const meta = [
    loc && `Line ${loc.line}, column ${loc.column}`,
    error.sqlstate && `SQLSTATE ${error.sqlstate}`,
    error.vendorCode !== null && `Error ${error.vendorCode}`,
    CODE_LABELS[error.code],
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
        <div className="text-error/80 mt-0.5 pl-5 tabular-nums">{meta.join(" · ")}</div>
      )}
    </div>
  );
}
