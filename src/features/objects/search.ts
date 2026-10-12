/**
 * The sidebar's item search (TablePlus spec §2.3). A query is a list of alternatives separated by
 * `|` or `,` (any one may match). Inside an alternative, whitespace-separated terms must all
 * match. A term is
 * - `"exact"`: a case-insensitive substring (spaces kept, `|` and `,` inside are literal);
 * - anchored with `^` and/or `$` (`^user`, `_log$`, `^users$`): prefix / suffix / whole name;
 * - anything else: a fuzzy match, every character in order (`usr` finds `users`).
 * Matching ignores case. An empty query matches everything.
 */

export type NameMatcher = (name: string) => boolean;

type Term = (name: string) => boolean;

const MATCH_ALL: NameMatcher = () => true;

function fuzzy(needle: string): Term {
  return (name) => {
    let i = 0;
    for (const c of name) {
      if (c === needle[i]) i++;
      if (i === needle.length) return true;
    }
    return i === needle.length;
  };
}

function anchored(raw: string): Term | null {
  const start = raw.startsWith("^");
  const end = raw.length > 1 && raw.endsWith("$");
  if (!start && !end) return null;
  const core = raw.slice(start ? 1 : 0, end ? -1 : undefined);
  if (start && end) return (name) => name === core;
  if (start) return (name) => name.startsWith(core);
  return (name) => name.endsWith(core);
}

/** Splits `query` on `sep` characters and whitespace (`split`), keeping `"…"` groups whole. */
function tokens(query: string): Array<{ text: string; quoted: boolean; sep: boolean }> {
  const out: Array<{ text: string; quoted: boolean; sep: boolean }> = [];
  let i = 0;
  while (i < query.length) {
    const c = query[i]!;
    if (c === "|" || c === ",") {
      out.push({ text: c, quoted: false, sep: true });
      i++;
    } else if (/\s/.test(c)) {
      i++;
    } else if (c === '"') {
      const close = query.indexOf('"', i + 1);
      const end = close < 0 ? query.length : close;
      out.push({ text: query.slice(i + 1, end), quoted: true, sep: false });
      i = end + 1;
    } else {
      let j = i;
      while (j < query.length && !/[\s|,"]/.test(query[j]!)) j++;
      out.push({ text: query.slice(i, j), quoted: false, sep: false });
      i = j;
    }
  }
  return out;
}

/** Builds the matcher of a sidebar search query (see the module comment). */
export function compileSearch(query: string): NameMatcher {
  const alternatives: Term[][] = [[]];
  for (const tok of tokens(query.toLowerCase())) {
    if (tok.sep) {
      alternatives.push([]);
      continue;
    }
    if (tok.text === "") continue;
    const term = tok.quoted
      ? (name: string) => name.includes(tok.text)
      : (anchored(tok.text) ?? fuzzy(tok.text));
    alternatives[alternatives.length - 1]!.push(term);
  }
  const used = alternatives.filter((terms) => terms.length > 0);
  if (used.length === 0) return MATCH_ALL;
  return (name) => {
    const lower = name.toLowerCase();
    return used.some((terms) => terms.every((t) => t(lower)));
  };
}

/** Whether `query` filters anything (an empty or blank query lists every item). */
export function isSearching(query: string): boolean {
  return compileSearch(query) !== MATCH_ALL;
}
