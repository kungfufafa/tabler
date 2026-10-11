import type { Diagnostic } from "@codemirror/lint";
import type { ErrorPayload, StatementSpan } from "@/lib/ipc";
import { charToUtf16 } from "./offsets";
import type { RunState } from "./runModel";

/** A range in the editor document (UTF-16 offsets). */
export interface DocRange {
  from: number;
  to: number;
}

/** What was submitted and where it sits in the document. */
export interface Submission {
  sql: string;
  /** UTF-16 offset of `sql` in the document (selection start, statement start, or 0). */
  baseOffset: number;
}

const WORD = /[\p{L}\p{N}_$]/u;
const QUOTES = new Set(["'", '"', "`"]);

/** End of the token starting at `from` (a quoted run, a word, or one character). */
export function tokenEnd(text: string, from: number): number {
  if (from >= text.length) return from;
  const c = text[from]!;
  if (QUOTES.has(c)) {
    const close = text.indexOf(c, from + 1);
    const nl = text.indexOf("\n", from + 1);
    if (close >= 0 && (nl < 0 || close < nl)) return close + 1;
    return nl >= 0 ? nl : text.length;
  }
  if (!WORD.test(c)) return from + 1;
  let i = from + 1;
  while (i < text.length && WORD.test(text[i]!)) i++;
  return i;
}

/** Document range of a statement span (char offsets relative to the submitted text). */
export function spanRange(span: StatementSpan, sub: Submission): DocRange {
  return {
    from: sub.baseOffset + charToUtf16(sub.sql, span.start),
    to: sub.baseOffset + charToUtf16(sub.sql, span.end),
  };
}

/**
 * Maps an error to a document range: its `position` (a char offset into the whole submitted text)
 * plus the submission's offset, covering the offending token; otherwise the statement's span.
 */
export function errorRange(
  error: Pick<ErrorPayload, "position">,
  sub: Submission,
  span?: StatementSpan | null,
): DocRange | null {
  if (error.position) {
    const local = charToUtf16(sub.sql, error.position.offset);
    return { from: sub.baseOffset + local, to: sub.baseOffset + tokenEnd(sub.sql, local) };
  }
  return span ? spanRange(span, sub) : null;
}

export function clampRange(r: DocRange, docLength: number): DocRange {
  const from = Math.max(0, Math.min(r.from, docLength));
  return { from, to: Math.max(from, Math.min(r.to, docLength)) };
}

/** Message shown for an error (with detail / hint lines when the server sent them). */
export function errorText(e: Pick<ErrorPayload, "message" | "detail" | "hint">): string {
  return [e.message, e.detail && `Detail: ${e.detail}`, e.hint && `Hint: ${e.hint}`]
    .filter(Boolean)
    .join("\n");
}

/**
 * Lint diagnostics for every failed statement of a run (cancellations are not errors). `locate`
 * maps a range of the submitted document to the current one (null drops the diagnostic).
 */
export function runDiagnostics(
  run: RunState,
  docLength: number,
  locate: (range: DocRange) => DocRange | null = (r) => r,
): Diagnostic[] {
  const out: Diagnostic[] = [];
  const push = (range: DocRange | null, error: ErrorPayload) => {
    const at = range && locate(range);
    if (at) out.push(diagnostic(clampRange(at, docLength), error));
  };
  let failed = false;
  for (const st of run.statements) {
    if (st.status !== "error" || !st.error) continue;
    failed = true;
    push(errorRange(st.error, run, st.span), st.error);
  }
  if (!failed && run.error && run.error.code !== "CANCELLED")
    push(errorRange(run.error, run), run.error);
  return out;
}

export function diagnostic(range: DocRange, error: ErrorPayload): Diagnostic {
  return {
    from: range.from,
    to: range.to,
    severity: "error",
    source: error.sqlstate ? `SQLSTATE ${error.sqlstate}` : error.code,
    message: errorText(error),
  };
}
