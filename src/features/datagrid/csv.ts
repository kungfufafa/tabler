import type { CsvQuote, Value } from "@/lib/ipc";
import { valueText } from "@/lib/values";
import {
  getPrefs,
  type CsvDecimalPref,
  type CsvDelimiterPref,
  type CsvLineBreakPref,
  type Prefs,
} from "@/stores/prefs";

/**
 * How CSV text is written, as Settings ▸ General ▸ CSV default options names it (TablePlus):
 * the delimiter, the quoting (`"` / `'` / a space quote every value, "ifNeeded" only those holding
 * the delimiter, the quote or a line break), the line break and the decimal separator (`.`, `,` or
 * a custom character).
 */
export interface CsvOptions {
  delimiter: CsvDelimiterPref;
  quote: CsvQuote;
  lineBreak: CsvLineBreakPref;
  decimal: CsvDecimalPref;
}

export const DEFAULT_CSV_OPTIONS: Readonly<CsvOptions> = Object.freeze({
  delimiter: ",",
  quote: "ifNeeded",
  lineBreak: "\n",
  decimal: ".",
});

type CsvPrefs = Pick<Prefs, "csvDelimiter" | "csvQuote" | "csvLineBreak" | "csvDecimal">;

/**
 * Whether `decimal` can be written next to `delimiter`: a custom separator must differ from the
 * delimiter (`,` is the exception: numbers holding it are quoted).
 */
export const decimalClashes = (decimal: string, delimiter: string): boolean =>
  decimal === delimiter && decimal !== ",";

/**
 * The CSV default options of the preferences. A custom decimal separator that equals the delimiter
 * is not used (`.` instead): the two preferences are chosen apart.
 */
export function csvDefaults(prefs: CsvPrefs = getPrefs()): CsvOptions {
  return {
    delimiter: prefs.csvDelimiter,
    quote: prefs.csvQuote,
    lineBreak: prefs.csvLineBreak,
    decimal: decimalClashes(prefs.csvDecimal, prefs.csvDelimiter) ? "." : prefs.csvDecimal,
  };
}

/**
 * The quote character a CSV is *read* with (the paste sniffer): `'` for single quotes, else `"`
 * (also for "Quote if needed"; a Space-quoted paste is not sniffed).
 */
export const quoteChar = (quote: CsvQuote): '"' | "'" => (quote === "single" ? "'" : '"');

/** The quote character a CSV is *written* with: `"`, `'` or a space (`"` for "Quote if needed"). */
export const csvQuoteChar = (quote: CsvQuote): '"' | "'" | " " =>
  quote === "space" ? " " : quoteChar(quote);

/** One field: NULL stays empty and unquoted; a quote inside a quoted field is doubled. */
export function csvField(text: string | null, o: CsvOptions): string {
  if (text === null) return "";
  const q = csvQuoteChar(o.quote);
  const needs =
    o.quote !== "ifNeeded" ||
    text.includes(o.delimiter) ||
    text.includes(q) ||
    /[\r\n]/.test(text) ||
    // PostgreSQL's end-of-data marker on a line of its own.
    text === "\\.";
  return needs ? `${q}${text.split(q).join(q + q)}${q}` : text;
}

/** A cell's CSV text: numbers with the chosen decimal separator. */
export function csvText(v: Value, o: CsvOptions): string | null {
  const text = valueText(v);
  if (text === null || o.decimal === ".") return text;
  // `replace` with a string pattern changes the first point only, and `() =>` keeps `$` literal.
  return v.t === "float" || v.t === "decimal" ? text.replace(".", () => o.decimal) : text;
}

/** Rows (and an optional header line) as CSV text, without a trailing line break. */
export function formatCsv(
  header: readonly string[] | null,
  rows: readonly (readonly Value[])[],
  o: CsvOptions,
): string {
  const lines = rows.map((r) => r.map((v) => csvField(csvText(v, o), o)).join(o.delimiter));
  if (header) lines.unshift(header.map((name) => csvField(name, o)).join(o.delimiter));
  return lines.join(o.lineBreak);
}

/**
 * The CSV import's delimiter from its detection: a file the detected delimiter does not split into
 * several columns is ambiguous (any delimiter reads it as one column), so the CSV default
 * option's delimiter is used instead.
 */
export function importDelimiter(
  detected: string,
  detectedColumns: number,
  fallback: string,
): string {
  return detectedColumns > 1 ? detected : fallback;
}

/**
 * The CSV import's starting quote: the one the last import used when it was not the double quote,
 * else the CSV default options' (single quotes, spaces), else double quotes.
 */
export function importQuote(remembered: string, preferred: CsvQuote): '"' | "'" | " " {
  if (remembered === "'" || remembered === " ") return remembered;
  return preferred === "single" ? "'" : preferred === "space" ? " " : '"';
}
