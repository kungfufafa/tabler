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
 * the delimiter, the quoting (`"` / `'` quote every value, "ifNeeded" only those holding the
 * delimiter, the quote or a line break), the line break and the decimal separator.
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

/** The CSV default options of the preferences. */
export function csvDefaults(prefs: CsvPrefs = getPrefs()): CsvOptions {
  return {
    delimiter: prefs.csvDelimiter,
    quote: prefs.csvQuote,
    lineBreak: prefs.csvLineBreak,
    decimal: prefs.csvDecimal,
  };
}

/** The quote character of an option (`"` for "Quote if needed"). */
export const quoteChar = (quote: CsvQuote): '"' | "'" => (quote === "single" ? "'" : '"');

/** One field: NULL stays empty and unquoted; a quote inside a quoted field is doubled. */
export function csvField(text: string | null, o: CsvOptions): string {
  if (text === null) return "";
  const q = quoteChar(o.quote);
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
  return v.t === "float" || v.t === "decimal" ? text.replace(".", ",") : text;
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
 * The CSV import's starting quote: single quotes when the last import used them or the CSV
 * default options ask for them, else double quotes.
 */
export function importQuote(remembered: string, preferred: CsvQuote): '"' | "'" {
  return remembered === "'" || preferred === "single" ? "'" : '"';
}
