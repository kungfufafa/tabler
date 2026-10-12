/**
 * Matchers for the command palette.
 *
 * `fuzzyMatch` (the narrowed ⌘K / ⇧⌘K lists): every query character must appear in the text in
 * order (case-insensitive; whitespace in the query is ignored). Among all possible alignments the
 * best-scoring one is chosen with a small dynamic program, so "ord" against "customer_order"
 * highlights the `ord` in `order` rather than the scattered first occurrences.
 *
 * `strictMatch` (Open Anything, ⌘P): the query must be a prefix of the text, a contiguous
 * substring of it, or a sequence of runs that each start a word ("po" finds `posts` but not
 * "Duplicate row"; "oi" finds `order_items`).
 */

/** Half-open `[start, end)` range of matched characters in the original text. */
export type MatchRange = readonly [start: number, end: number];

export interface FuzzyMatch {
  score: number;
  ranges: MatchRange[];
}

const SCORE_MATCH = 1;
/** Bonus for a character that directly follows the previous matched character. */
const BONUS_CONSECUTIVE = 5;
/** Bonus for matching at the start of a word (`_`, `-`, `.`, space, camelCase, digits). */
const BONUS_WORD_START = 6;
/** Bonus for matching the very first character of the text. */
const BONUS_FIRST_CHAR = 8;
/** Opening a gap between two matched characters. */
const PENALTY_GAP_START = 1;
/** Each further skipped character inside a gap. */
const PENALTY_GAP_EXTEND = 0.2;
/** The whole query is a prefix of the text ("ord" → "orders"). */
const BONUS_PREFIX = 20;
/** The query equals the text. */
const BONUS_EXACT = 10;
/** Per unmatched character, so shorter texts win ties ("users" before "users_archive"). */
const PENALTY_LENGTH = 0.05;

const SEPARATORS = new Set(["_", "-", ".", " ", "/", "\\", ":", "(", ")", "[", "]", ",", "·"]);

function isUpper(c: string): boolean {
  return c !== c.toLowerCase() && c === c.toUpperCase();
}

function isLower(c: string): boolean {
  return c !== c.toUpperCase() && c === c.toLowerCase();
}

function isDigit(c: string): boolean {
  return c >= "0" && c <= "9";
}

/** Bonus for matching `text[j]` based on its position in a word. */
function positionBonus(text: string, j: number): number {
  if (j === 0) return BONUS_FIRST_CHAR;
  const prev = text[j - 1]!;
  const cur = text[j]!;
  if (SEPARATORS.has(prev)) return SEPARATORS.has(cur) ? 0 : BONUS_WORD_START;
  if (isUpper(cur) && isLower(prev)) return BONUS_WORD_START;
  if (isDigit(cur) && !isDigit(prev)) return BONUS_WORD_START;
  return 0;
}

/** True when `text[j]` starts a word (text start, after a separator, camelCase or digit run). */
export function isWordStart(text: string, j: number): boolean {
  return j >= 0 && j < text.length && positionBonus(text, j) > 0;
}

/** Normalizes a query: lower-case, whitespace removed. */
export function normalizeQuery(query: string): string {
  return query.replace(/\s+/g, "").toLowerCase();
}

function toRanges(positions: number[]): MatchRange[] {
  const ranges: MatchRange[] = [];
  let start = -1;
  let end = -1;
  for (const p of positions) {
    if (p === end) {
      end = p + 1;
      continue;
    }
    if (start >= 0) ranges.push([start, end]);
    start = p;
    end = p + 1;
  }
  if (start >= 0) ranges.push([start, end]);
  return ranges;
}

function isPrefix(t: ArrayLike<string>, q: string): boolean {
  for (let i = 0; i < q.length; i++) if (t[i] !== q[i]) return false;
  return true;
}

/**
 * Lower-cased characters of `text`, index-aligned with it. toLowerCase can change the length for
 * a few exotic code points; fall back per character so indices keep pointing into the original.
 */
function lowerChars(text: string): ArrayLike<string> {
  const lower = text.toLowerCase();
  return lower.length === text.length
    ? lower
    : Array.from({ length: text.length }, (_, i) => text[i]!.toLowerCase());
}

/** Cheap rejection: is `q` a subsequence of `t` at all? */
function isSubsequence(t: ArrayLike<string>, q: string): boolean {
  let k = 0;
  for (let j = 0; j < t.length && k < q.length; j++) if (t[j] === q[k]) k++;
  return k === q.length;
}

/**
 * Scores `text` against `query`. Returns `null` when the query is not a subsequence of the text.
 * An empty query matches everything with score 0 and no highlights.
 */
export function fuzzyMatch(query: string, text: string): FuzzyMatch | null {
  const q = normalizeQuery(query);
  const n = q.length;
  if (n === 0) return { score: 0, ranges: [] };
  const m = text.length;
  if (n > m) return null;
  const t = lowerChars(text);
  if (!isSubsequence(t, q)) return null;

  const lengthPenalty = (m - n) * PENALTY_LENGTH;
  if (isPrefix(t, q)) {
    // A contiguous match at the very start is the best possible alignment; highlight exactly it.
    let prefixScore = SCORE_MATCH + BONUS_FIRST_CHAR;
    for (let j = 1; j < n; j++)
      prefixScore += SCORE_MATCH + Math.max(positionBonus(text, j), BONUS_CONSECUTIVE);
    const exact = m === n ? BONUS_EXACT : 0;
    return { score: prefixScore - lengthPenalty + BONUS_PREFIX + exact, ranges: [[0, n]] };
  }

  // score[i*m + j]: best score with q[i] matched at text[j]; -Infinity when impossible.
  // from[i*m + j]: the position q[i-1] was matched at for that best score.
  const score = new Float64Array(n * m).fill(-Infinity);
  const from = new Int32Array(n * m).fill(-1);

  for (let j = 0; j < m; j++) {
    if (t[j] === q[0]) score[j] = SCORE_MATCH + positionBonus(text, j);
  }

  for (let i = 1; i < n; i++) {
    const row = i * m;
    const prevRow = (i - 1) * m;
    // Best predecessor that leaves a gap of at least one character, decayed by the gap length.
    let gapBest = -Infinity;
    let gapFrom = -1;
    for (let j = i; j < m; j++) {
      if (j >= 2) {
        gapBest -= PENALTY_GAP_EXTEND;
        const cand = score[prevRow + j - 2]!;
        if (cand > gapBest) {
          gapBest = cand;
          gapFrom = j - 2;
        }
      }
      if (t[j] !== q[i]) continue;
      const bonus = positionBonus(text, j);
      const consecutive = score[prevRow + j - 1]!;
      let best = -Infinity;
      let bestFrom = -1;
      if (consecutive > -Infinity) {
        best = consecutive + SCORE_MATCH + Math.max(bonus, BONUS_CONSECUTIVE);
        bestFrom = j - 1;
      }
      if (gapBest > -Infinity) {
        const viaGap = gapBest - PENALTY_GAP_START + SCORE_MATCH + bonus;
        if (viaGap > best) {
          best = viaGap;
          bestFrom = gapFrom;
        }
      }
      score[row + j] = best;
      from[row + j] = bestFrom;
    }
  }

  const last = (n - 1) * m;
  let bestScore = -Infinity;
  let bestEnd = -1;
  for (let j = n - 1; j < m; j++) {
    if (score[last + j]! > bestScore) {
      bestScore = score[last + j]!;
      bestEnd = j;
    }
  }
  if (bestEnd < 0) return null;

  const positions = new Array<number>(n);
  for (let i = n - 1, j = bestEnd; i >= 0; i--) {
    positions[i] = j;
    j = from[i * m + j]!;
  }

  return { score: bestScore - lengthPenalty, ranges: toRanges(positions) };
}

/** How a strict match lines up with the text, best first. */
export type StrictTier = "exact" | "prefix" | "wordStart" | "substring";

export interface StrictMatch {
  tier: StrictTier;
  ranges: MatchRange[];
}

/** Larger is better; for sorting strict matches. */
export const STRICT_TIER_RANK: Record<StrictTier, number> = {
  exact: 4,
  prefix: 3,
  wordStart: 2,
  substring: 1,
};

const NO_RUNS = 0x3fffffff;

/**
 * Aligns `q` with `text` so that every run of consecutive matched characters starts a word,
 * using as few runs as possible (ties: the earliest alignment). Returns the matched positions.
 */
function wordStartPositions(text: string, t: ArrayLike<string>, q: string): number[] | null {
  const n = q.length;
  const m = t.length;
  // runs[i*m + j]: fewest runs that still have to start to match q[i+1..] once q[i] sits at j.
  const runs = new Int32Array(n * m).fill(NO_RUNS);
  const next = new Int32Array(n * m).fill(-1);
  for (let j = 0; j < m; j++) if (t[j] === q[n - 1]) runs[(n - 1) * m + j] = 0;
  // Best word start at or after k for the following row: its run count and position.
  const sufRuns = new Int32Array(m + 1);
  const sufPos = new Int32Array(m + 1);
  for (let i = n - 2; i >= 0; i--) {
    const row = i * m;
    const nextRow = (i + 1) * m;
    sufRuns[m] = NO_RUNS;
    sufPos[m] = -1;
    for (let k = m - 1; k >= 0; k--) {
      const r = runs[nextRow + k]!;
      if (r <= sufRuns[k + 1]! && r < NO_RUNS && isWordStart(text, k)) {
        sufRuns[k] = r;
        sufPos[k] = k;
      } else {
        sufRuns[k] = sufRuns[k + 1]!;
        sufPos[k] = sufPos[k + 1]!;
      }
    }
    for (let j = i; j < m; j++) {
      if (t[j] !== q[i]) continue;
      // Continue the current run: no new run.
      let best = j + 1 < m ? runs[nextRow + j + 1]! : NO_RUNS;
      let bestNext = best < NO_RUNS ? j + 1 : -1;
      // Or skip ahead and start a new run at a word start.
      if (j + 2 <= m && sufRuns[j + 2]! < NO_RUNS && sufRuns[j + 2]! + 1 < best) {
        best = sufRuns[j + 2]! + 1;
        bestNext = sufPos[j + 2]!;
      }
      runs[row + j] = best;
      next[row + j] = bestNext;
    }
  }
  let start = -1;
  for (let j = 0; j < m; j++) {
    if (t[j] !== q[0] || !isWordStart(text, j) || runs[j]! >= NO_RUNS) continue;
    if (start < 0 || runs[j]! < runs[start]!) start = j;
  }
  if (start < 0) return null;
  const positions = new Array<number>(n);
  for (let i = 0, j = start; i < n; i++) {
    positions[i] = j;
    j = next[i * m + j]!;
  }
  return positions;
}

function indexOfChars(t: ArrayLike<string>, q: string): number {
  outer: for (let j = 0; j + q.length <= t.length; j++) {
    for (let i = 0; i < q.length; i++) if (t[j + i] !== q[i]) continue outer;
    return j;
  }
  return -1;
}

/**
 * Strict matcher for Open Anything (⌘P). `text` matches when the query (case-insensitive,
 * whitespace ignored) is
 * - a prefix of it (`exact` when it is the whole text): "po" → `posts`;
 * - a sequence of runs that each start a word: "oi" → `order_items`, "pc" → `post_counts`;
 * - or a contiguous substring of it: "ost" → `posts`.
 * Anything else is no match, so "po" does not find "Duplicate row". An empty query matches
 * everything as a prefix with no highlights.
 */
export function strictMatch(query: string, text: string): StrictMatch | null {
  const q = normalizeQuery(query);
  const n = q.length;
  if (n === 0) return { tier: "prefix", ranges: [] };
  const m = text.length;
  if (n > m) return null;
  const t = lowerChars(text);
  if (isPrefix(t, q)) return { tier: m === n ? "exact" : "prefix", ranges: [[0, n]] };
  if (!isSubsequence(t, q)) return null;
  const positions = wordStartPositions(text, t, q);
  if (positions) return { tier: "wordStart", ranges: toRanges(positions) };
  const at = indexOfChars(t, q);
  return at >= 0 ? { tier: "substring", ranges: [[at, at + n]] } : null;
}
