import type { CellPos, GridColumn, ViewRow } from "@/features/datagrid/types";
import type { Value } from "@/lib/ipc";
import { displayValue, parseEditText } from "@/lib/values";

/**
 * Find & replace inside a SQL editor result (TablePlus b314): client-side, over the rows the
 * result has loaded, on the values as the grid shows them. Distinct from a table's filter, which
 * runs SQL. Cells are addressed as the grid's actions do: `r` the row index in the grid's order,
 * `c` the SOURCE column index (see `GridAction`).
 */

export interface FindOptions {
  matchCase: boolean;
  wholeWord: boolean;
  regex: boolean;
}

export interface FindQuery extends FindOptions {
  text: string;
}

/** Matches found are kept up to this many (the counter then reads "10,000+"). */
export const MAX_MATCHES = 10_000;

export interface Matcher {
  /** Whether `text` holds a match. */
  test: (text: string) => boolean;
  /** `text` with every match replaced (`$1`… refer to regex groups). */
  replace: (text: string, replacement: string) => string;
}

export type Compiled =
  { kind: "empty" } | { kind: "invalid"; error: string } | { kind: "ok"; matcher: Matcher };

const escapeRegExp = (s: string) => s.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&");

function build(source: string, opts: FindOptions, unicode: boolean): RegExp {
  const word = unicode ? "[\\p{L}\\p{N}_]" : "\\w";
  const body = opts.wholeWord ? `(?<!${word})(?:${source})(?!${word})` : source;
  return new RegExp(body, `g${opts.matchCase ? "" : "i"}${unicode ? "u" : ""}`);
}

/** A matcher for the query; `empty` without text, `invalid` for a regex that does not compile. */
export function compileQuery(q: FindQuery): Compiled {
  if (q.text === "") return { kind: "empty" };
  const source = q.regex ? q.text : escapeRegExp(q.text);
  let re: RegExp;
  try {
    re = build(source, q, true);
  } catch (e) {
    // Patterns written for non-Unicode mode (`[\-x]`…) are valid there.
    try {
      re = build(source, q, false);
    } catch {
      return {
        kind: "invalid",
        error: (e as Error).message.replace(/^Invalid regular expression: /, ""),
      };
    }
  }
  const matcher: Matcher = {
    test: (text) => {
      re.lastIndex = 0;
      return re.test(text);
    },
    replace: (text, replacement) => {
      re.lastIndex = 0;
      // A literal search inserts the replacement as typed (no `$&` expansion).
      return q.regex ? text.replace(re, replacement) : text.replace(re, () => replacement);
    },
  };
  return { kind: "ok", matcher };
}

/**
 * The text a cell is searched in: what the grid shows, except that text and JSON keep their line
 * breaks and spacing (the grid folds them onto one line). NULL / DEFAULT placeholders and binary
 * values never match.
 */
export function searchText(v: Value, lacksFields = false): string | null {
  switch (v.t) {
    case "null":
    case "default":
    case "bytes":
      return null;
    case "text":
    case "enum":
    case "json":
      return v.v;
    default:
      return displayValue(v, lacksFields).text;
  }
}

/**
 * Every matching cell, row by row, left to right (at most `MAX_MATCHES`). `columns` are the
 * SOURCE indexes searched, in display order: the grid's visible columns (a hidden column's match
 * could not be shown, nor should Replace All change it unseen). A number searches that many
 * columns from the first.
 */
export function findMatches(
  rowCount: number,
  getRow: (r: number) => ViewRow | undefined,
  columns: number | readonly number[],
  matcher: Matcher,
  lacksFields = false,
): CellPos[] {
  const cols = typeof columns === "number" ? Array.from({ length: columns }, (_, c) => c) : columns;
  const out: CellPos[] = [];
  for (let r = 0; r < rowCount; r++) {
    const row = getRow(r);
    if (!row) continue;
    for (const c of cols) {
      const cell = row.cells[c];
      const text = cell ? searchText(cell, lacksFields) : null;
      if (text === null || !matcher.test(text)) continue;
      out.push({ r, c });
      if (out.length >= MAX_MATCHES) return out;
    }
  }
  return out;
}

/** The next (`dir` 1) or previous (-1) match, wrapping around; the first one without a current. */
export function stepMatch(current: number | null, count: number, dir: 1 | -1): number | null {
  if (count === 0) return null;
  if (current === null) return dir === 1 ? 0 : count - 1;
  return (current + dir + count) % count;
}

/**
 * Where the current match goes after the matches were recomputed (rows streamed in, an edit):
 * the same cell when it still matches, else the first match after it, else the first one.
 * `after`: never the same cell (it was just replaced). `columns`: the searched columns in display
 * order (`findMatches`), which "after" follows; source order without them.
 */
export function rematch(
  anchor: CellPos | null,
  matches: readonly CellPos[],
  after = false,
  columns?: readonly number[],
): number | null {
  if (matches.length === 0) return null;
  if (!anchor) return 0;
  // A column no longer searched (just hidden) ranks first: the next match is the row's first.
  const rank = columns ? (c: number) => columns.indexOf(c) : (c: number) => c;
  const at = rank(anchor.c);
  const i = matches.findIndex(
    (m) => m.r > anchor.r || (m.r === anchor.r && (after ? rank(m.c) > at : rank(m.c) >= at)),
  );
  return i < 0 ? 0 : i;
}

/** "3 of 12", "No results", "10,000+ results"… */
export function counterLabel(current: number | null, total: number): string {
  if (total === 0) return "No results";
  const of = total >= MAX_MATCHES ? `${MAX_MATCHES.toLocaleString("en-US")}+` : String(total);
  return current === null ? `${of} results` : `${current + 1} of ${of}`;
}

export interface ReplaceEdit {
  r: number;
  c: number;
  value: Value;
}

export interface ReplacePlan {
  edits: ReplaceEdit[];
  /** Matching cells left alone: read-only column, deleted row, or a value its type rejects. */
  skipped: number;
}

/**
 * The cell edits that replace every match of `matcher` in `cells` with `replacement`. Values are
 * re-typed for their column (`parseEditText`): a cell whose new text its type rejects (letters in
 * an integer column) is skipped, as are read-only columns and deleted rows.
 */
export function planReplace(
  cells: readonly CellPos[],
  getRow: (r: number) => ViewRow | undefined,
  columns: readonly GridColumn[],
  matcher: Matcher,
  replacement: string,
  lacksFields = false,
): ReplacePlan {
  const edits: ReplaceEdit[] = [];
  let skipped = 0;
  for (const { r, c } of cells) {
    const row = getRow(r);
    const column = columns[c];
    const cell = row?.cells[c];
    const text = cell ? searchText(cell, lacksFields) : null;
    if (!row || !column?.editable || row.status === "deleted" || !cell || text === null) {
      skipped++;
      continue;
    }
    const next = matcher.replace(text, replacement);
    if (next === text) continue;
    const parsed = parseEditText(column.kind, next, cell, lacksFields);
    if (!parsed.ok) {
      skipped++;
      continue;
    }
    edits.push({ r, c, value: parsed.value });
  }
  return { edits, skipped };
}
