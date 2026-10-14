import { highlightSql, type Segment } from "@/features/structure/sqlHighlight";
import type { Dialect } from "@/lib/ipc";

/** Longer SQL is shown as plain text (a pasted dump would cost more to parse than it helps). */
export const HIGHLIGHT_MAX_CHARS = 20_000;

/**
 * The console's syntax highlighting (TablePlus b320): the static lezer SQL highlighter of the
 * structure view, no editor instance per entry. One flat list of spans; line breaks are kept as
 * "\n" text segments.
 */
export function consoleSegments(sql: string, dialect: Dialect | null | undefined): Segment[] {
  if (sql.length > HIGHLIGHT_MAX_CHARS) return [{ text: sql, cls: "" }];
  const lines = highlightSql(dialect ?? "postgres", sql);
  const out: Segment[] = [];
  lines.forEach((line, i) => {
    if (i > 0) out.push({ text: "\n", cls: "" });
    for (const seg of line) {
      const prev = out.at(-1);
      // Merge neighbours of the same class: fewer spans to render.
      if (prev && prev.cls === seg.cls && prev.text !== "\n") prev.text += seg.text;
      else out.push({ ...seg });
    }
  });
  return out;
}
