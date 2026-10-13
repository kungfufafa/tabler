/**
 * Comment-driven charts (TablePlus b512): a comment right above a SELECT asks for a chart of its
 * result, e.g.
 *
 *     -- TablePlus BarChart x: month, y: revenue, cost
 *     SELECT month, revenue, cost FROM sales;
 *
 * `BarChart`, `LineChart` and `PieChart`; `Tabler` works as the keyword too. `y:` may name
 * several columns (bar and line charts draw one series each). Other comments are ignored, and a
 * directive only applies to the statement directly below it.
 */

export type ChartKind = "bar" | "line" | "pie";

export interface ChartDirective {
  kind: ChartKind;
  /** The category / x column; null: the result's first column. */
  x: string | null;
  /** The value columns; empty: every numeric column but x. */
  y: string[];
}

const DIRECTIVE =
  /^(?:--|#|\/\*)\s*(?:tableplus|tabler)\s+(bar|line|pie)chart\b(.*?)(?:\*\/)?\s*$/i;
const KEY = /(^|[\s,;])([xy])\s*:/gi;

const KINDS: Record<string, ChartKind> = { bar: "bar", line: "line", pie: "pie" };

/** A column name as written: quotes (`"…"`, `` `…` ``, `[…]`) removed. */
function unquoteName(raw: string): string {
  const s = raw
    .trim()
    .replace(/[;,]+$/, "")
    .trim();
  const m = /^"(.*)"$|^`(.*)`$|^\[(.*)\]$/.exec(s);
  return m ? (m[1] ?? m[2] ?? m[3] ?? "") : s;
}

function names(list: string): string[] {
  return list
    .split(",")
    .map(unquoteName)
    .filter((n) => n !== "");
}

/** The directive in one comment line (`-- TablePlus BarChart x: a, y: b`), or null. */
export function parseDirective(comment: string): ChartDirective | null {
  const m = DIRECTIVE.exec(comment.trim());
  if (!m) return null;
  const kind = KINDS[m[1]!.toLowerCase()];
  if (!kind) return null;
  const args = m[2] ?? "";
  const keys = [...args.matchAll(KEY)];
  let x: string | null = null;
  let y: string[] = [];
  keys.forEach((k, i) => {
    const from = k.index + k[0].length;
    const to = keys[i + 1]?.index ?? args.length;
    const value = args.slice(from, to);
    if (k[2]!.toLowerCase() === "x") x = names(value)[0] ?? null;
    else y = names(value);
  });
  return { kind, x, y };
}

const isComment = (line: string) =>
  line.startsWith("--") ||
  line.startsWith("#") ||
  line.startsWith("/*") ||
  line.startsWith("*") ||
  line.endsWith("*/");

/**
 * The chart directive of the statement starting at `start` (a UTF-16 index into `text`): the
 * closest directive among the comment lines right above it (blank lines and other comments may
 * sit in between; any code line ends the search).
 */
export function directiveAbove(text: string, start: number): ChartDirective | null {
  const lines = text.slice(0, start).split(/\r?\n/);
  // The statement's own line, before it: a comment there (`/* TablePlus … */ SELECT`) counts.
  const own = lines.pop()?.trim() ?? "";
  if (own !== "") {
    if (!isComment(own)) return null;
    const d = parseDirective(own);
    if (d) return d;
  }
  for (let i = lines.length - 1; i >= 0; i--) {
    const line = lines[i]!.trim();
    if (line === "") continue;
    if (!isComment(line)) return null;
    const d = parseDirective(line);
    if (d) return d;
  }
  return null;
}
