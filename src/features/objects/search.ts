import { MAX_TESTED_NAME, regexError } from "@/features/sidebar/groups";

/**
 * The sidebar's item search (TablePlus spec §2.3). A query is a list of alternatives separated by
 * `|` or `,` (any one may match). Inside an alternative, whitespace-separated terms must all
 * match. A term is
 * - `"exact"`: a case-insensitive substring (spaces kept, `|` and `,` inside are literal);
 * - anchored with `^` and/or `$` (`^user`, `_log$`, `^users$`): prefix / suffix / whole name;
 * - anything else: matched by the search menu's Matching Strategy (`MatchStrategy`): fuzzy (the
 *   default: every character in order, `usr` finds `users`), contains (a substring) or prefix.
 * The Regex strategy instead tests the whole query as one regular expression (`|` is its own
 * alternation); a pattern that does not compile (or could run away) matches as plain text.
 * Matching ignores case. An empty query matches everything.
 */

export type NameMatcher = (name: string) => boolean;

/** The sidebar search menu's "Matching Strategy ▸". */
export type MatchStrategy = "fuzzy" | "contains" | "prefix" | "regex";

export const MATCH_STRATEGIES: readonly MatchStrategy[] = ["fuzzy", "contains", "prefix", "regex"];

export const MATCH_STRATEGY_LABELS: Record<MatchStrategy, string> = {
  fuzzy: "Fuzzy",
  contains: "Contains",
  prefix: "Prefix",
  regex: "Regex",
};

const STRATEGY_HINTS: Record<MatchStrategy, string> = {
  fuzzy:
    'Fuzzy match. ^name$ anchors the start / end, | or , separates alternatives, "quotes" match exactly.',
  contains:
    'Contains. ^name$ anchors the start / end, | or , separates alternatives, "quotes" match exactly.',
  prefix:
    'Prefix. ^name$ anchors the start / end, | or , separates alternatives, "quotes" match exactly.',
  regex: "Regular expression, ignoring case (e.g. ^user_|_log$).",
};

/** The item search field's tooltip under a Matching Strategy. */
export function searchHint(strategy: MatchStrategy): string {
  return STRATEGY_HINTS[strategy];
}

export function isMatchStrategy(value: unknown): value is MatchStrategy {
  return MATCH_STRATEGIES.includes(value as MatchStrategy);
}

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

/** A plain (unquoted, unanchored) term under `strategy`. */
function plainTerm(text: string, strategy: Exclude<MatchStrategy, "regex">): Term {
  switch (strategy) {
    case "contains":
      return (name) => name.includes(text);
    case "prefix":
      return (name) => name.startsWith(text);
    case "fuzzy":
      return fuzzy(text);
  }
}

/** The Regex strategy: the query as one case-insensitive pattern, plain text when unusable. */
function compileRegex(query: string): NameMatcher {
  const source = query.trim();
  if (source === "") return MATCH_ALL;
  if (regexError(source) !== null) {
    const text = source.toLowerCase();
    return (name) => name.toLowerCase().includes(text);
  }
  const re = new RegExp(source, "i");
  return (name) => re.test(name.slice(0, MAX_TESTED_NAME));
}

/** Builds the matcher of a sidebar search query (see the module comment). */
export function compileSearch(query: string, strategy: MatchStrategy = "fuzzy"): NameMatcher {
  if (strategy === "regex") return compileRegex(query);
  const alternatives: Term[][] = [[]];
  for (const tok of tokens(query.toLowerCase())) {
    if (tok.sep) {
      alternatives.push([]);
      continue;
    }
    if (tok.text === "") continue;
    const term = tok.quoted
      ? (name: string) => name.includes(tok.text)
      : (anchored(tok.text) ?? plainTerm(tok.text, strategy));
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
export function isSearching(query: string, strategy: MatchStrategy = "fuzzy"): boolean {
  return compileSearch(query, strategy) !== MATCH_ALL;
}
