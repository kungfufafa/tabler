import { MySQL, PostgreSQL, SQLite, type SQLDialect } from "@codemirror/lang-sql";
import { highlightCode, tagHighlighter, tags } from "@lezer/highlight";
import type { Dialect } from "@/lib/ipc";

export interface Segment {
  text: string;
  /** Utility classes for the token ("" for plain text). */
  cls: string;
}

const DIALECTS: Record<Dialect, SQLDialect> = {
  postgres: PostgreSQL,
  mysql: MySQL,
  sqlite: SQLite,
};

/** Token colours come from the editor's syntax variables so light and dark both work. */
const highlighter = tagHighlighter([
  { tag: [tags.keyword, tags.typeName, tags.bool, tags.null], class: "text-(--syn-keyword)" },
  { tag: tags.string, class: "text-(--syn-string)" },
  // Quoted identifiers are `special(string)` in lang-sql: keep them in the text colour.
  { tag: tags.special(tags.string), class: "text-foreground" },
  { tag: tags.number, class: "text-(--syn-number)" },
  { tag: [tags.lineComment, tags.blockComment], class: "text-(--syn-comment) italic" },
]);

/** Splits `code` into highlighted segments, one array per line. */
export function highlightSql(dialect: Dialect, code: string): Segment[][] {
  const lines: Segment[][] = [[]];
  try {
    const tree = DIALECTS[dialect].language.parser.parse(code);
    highlightCode(
      code,
      tree,
      highlighter,
      (text, cls) => lines[lines.length - 1]!.push({ text, cls }),
      () => lines.push([]),
    );
  } catch {
    return code.split("\n").map((text) => [{ text, cls: "" }]);
  }
  return lines;
}
