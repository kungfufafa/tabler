/**
 * Subsequence fuzzy matcher for the command palette.
 *
 * Every query character must appear in the text in order (case-insensitive; whitespace in the
 * query is ignored). Among all possible alignments the best-scoring one is chosen with a small
 * dynamic program, so "ord" against "customer_order" highlights the `ord` in `order` rather than
 * the scattered first occurrences.
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
 * Scores `text` against `query`. Returns `null` when the query is not a subsequence of the text.
 * An empty query matches everything with score 0 and no highlights.
 */
export function fuzzyMatch(query: string, text: string): FuzzyMatch | null {
  const q = normalizeQuery(query);
  const n = q.length;
  if (n === 0) return { score: 0, ranges: [] };
  const m = text.length;
  if (n > m) return null;
  const lower = text.toLowerCase();
  // toLowerCase can change the length for a few exotic code points; fall back per character so
  // indices keep pointing into the original text.
  const t: ArrayLike<string> =
    lower.length === m ? lower : Array.from({ length: m }, (_, i) => text[i]!.toLowerCase());

  // Cheap rejection before the DP: is q a subsequence at all?
  let k = 0;
  for (let j = 0; j < m && k < n; j++) if (t[j] === q[k]) k++;
  if (k < n) return null;

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
