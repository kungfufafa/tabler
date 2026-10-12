import type { QueryParamSyntax } from "@/stores/prefs";
import { skippedSpans, type SqlDialect } from "./scanner";

/**
 * Query parameters (TablePlus's "query params"): variables such as `:id` in the SQL about to run
 * are replaced by values the user types, as raw SQL text, before the run. Which syntax counts is a
 * preference (one of `QUERY_PARAM_SYNTAXES`); variables in comments never count, and those in
 * strings or quoted identifiers only with "Allow Searching for Variables in Quoted String".
 */

/** One variable occurrence in the scanned text (UTF-16 offsets). */
export interface ParamOccurrence {
  /** The prompt field it fills: the variable as written, or `?1`, `?2`… for positional `?`. */
  key: string;
  start: number;
  end: number;
}

/** One input of the parameters dialog. */
export interface ParamField {
  key: string;
  /** What the dialog shows: the variable as written (`:id`, `${name}`, `?` #2…). */
  label: string;
}

export interface ParamScanOptions {
  syntax: QueryParamSyntax;
  /** Look inside strings and quoted identifiers too. */
  inQuotes: boolean;
}

const POSITIONAL: QueryParamSyntax = "\\?";

/** Every variable of `text` outside comments (and outside quotes unless `inQuotes`), in order. */
export function findParams(
  text: string,
  dialect: SqlDialect,
  { syntax, inQuotes }: ParamScanOptions,
): ParamOccurrence[] {
  const skipped = skippedSpans(text, dialect).filter((s) => !(inQuotes && s.kind === "quoted"));
  const re = new RegExp(syntax, "g");
  const out: ParamOccurrence[] = [];
  let span = 0;
  let positional = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    if (m[0] === "") {
      re.lastIndex++;
      continue;
    }
    const start = m.index;
    const end = start + m[0].length;
    while (span < skipped.length && skipped[span]!.end <= start) span++;
    const s = skipped[span];
    if (s && s.start < end) {
      // Inside a comment / string: carry on after it.
      re.lastIndex = Math.max(re.lastIndex, s.end);
      continue;
    }
    // `::type` casts (PostgreSQL) are not `:type` variables.
    if (syntax === ":[\\w.]+" && text[start - 1] === ":") continue;
    const key = syntax === POSITIONAL ? `?${++positional}` : m[0];
    out.push({ key, start, end });
  }
  return out;
}

/** The dialog's inputs: one per distinct variable, in order of first appearance. */
export function paramFields(occurrences: readonly ParamOccurrence[]): ParamField[] {
  const seen = new Set<string>();
  const fields: ParamField[] = [];
  for (const o of occurrences) {
    if (seen.has(o.key)) continue;
    seen.add(o.key);
    fields.push({ key: o.key, label: /^\?\d+$/.test(o.key) ? `? #${o.key.slice(1)}` : o.key });
  }
  return fields;
}

/** One replaced variable: `start..end` of the original text became `length` characters. */
export interface Replacement {
  start: number;
  end: number;
  length: number;
}

export interface Substituted {
  sql: string;
  replacements: Replacement[];
}

/** Replaces every occurrence by its value, as SQL text (missing values become empty). */
export function substituteParams(
  text: string,
  occurrences: readonly ParamOccurrence[],
  values: Readonly<Record<string, string>>,
): Substituted {
  let sql = "";
  let pos = 0;
  const replacements: Replacement[] = [];
  for (const o of occurrences) {
    const value = values[o.key] ?? "";
    sql += text.slice(pos, o.start) + value;
    replacements.push({ start: o.start, end: o.end, length: value.length });
    pos = o.end;
  }
  sql += text.slice(pos);
  return { sql, replacements };
}

/**
 * Maps an offset of the substituted text back to the original one: a position inside a value
 * lands on the variable's start (`side` -1) or end (`side` 1).
 */
export function originalOffset(
  replacements: readonly Replacement[],
  pos: number,
  side: -1 | 1 = -1,
): number {
  let delta = 0;
  for (const r of replacements) {
    const start = r.start + delta;
    if (pos < start) break;
    // Right before the value; an empty value's end is its start, `side` picks.
    if (pos === start && (side < 0 || r.length > 0)) return r.start;
    if (pos < start + r.length) return side < 0 ? r.start : r.end;
    delta += r.length - (r.end - r.start);
  }
  return pos - delta;
}
