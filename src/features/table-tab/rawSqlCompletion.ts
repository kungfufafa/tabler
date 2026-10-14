import {
  CompletionContext,
  type Completion,
  type CompletionResult,
} from "@codemirror/autocomplete";
import {
  keywordCompletionSource,
  MySQL,
  PostgreSQL,
  schemaCompletionSource,
  sql,
  SQLite,
} from "@codemirror/lang-sql";
import { EditorState } from "@codemirror/state";
import type { Dialect } from "@/lib/ipc";

const DIALECTS = { postgres: PostgreSQL, mysql: MySQL, sqlite: SQLite } as const;

export interface RawSqlSuggestions {
  /** Where the word being completed starts (it is replaced from there to the caret). */
  from: number;
  options: Completion[];
}

/** Most suggestions listed. */
const MAX_OPTIONS = 12;

function resultOf(r: CompletionResult | Promise<CompletionResult | null> | null) {
  // lang-sql's keyword and schema sources answer synchronously.
  return r instanceof Promise ? null : r;
}

/**
 * The Raw SQL filter's autocomplete: the SQL editor's completion sources (lang-sql keywords and
 * schema completion, here with the table's columns at the top level) run headless on the field's
 * text, for the word before the caret. Columns come first, then keywords; null when there is no
 * word to complete or nothing matches.
 */
export function rawSqlSuggestions(
  text: string,
  caret: number,
  dialect: Dialect,
  table: string,
  columns: readonly string[],
): RawSqlSuggestions | null {
  const d = DIALECTS[dialect];
  const state = EditorState.create({
    doc: text,
    selection: { anchor: caret },
    extensions: [sql({ dialect: d })],
  });
  const ctx = new CompletionContext(state, caret, false);
  const word = ctx.matchBefore(/[\w$]+/);
  if (!word || word.from === word.to) return null;
  const prefix = word.text.toLowerCase();
  const sources = [
    schemaCompletionSource({ dialect: d, schema: { [table]: [...columns] }, defaultTable: table }),
    keywordCompletionSource(d, true),
  ];
  const seen = new Set<string>();
  const columnsFirst: Completion[] = [];
  const rest: Completion[] = [];
  for (const source of sources) {
    const r = resultOf(source(ctx));
    if (!r) continue;
    for (const o of r.options) {
      const key = o.label.toLowerCase();
      if (!key.startsWith(prefix) || key === prefix || seen.has(key)) continue;
      seen.add(key);
      (columns.includes(o.label) ? columnsFirst : rest).push(o);
    }
  }
  const options = [...columnsFirst, ...rest].slice(0, MAX_OPTIONS);
  return options.length > 0 ? { from: word.from, options } : null;
}

/** `text` with the word from `from` to `caret` replaced by `label`; the new caret. */
export function applySuggestion(
  text: string,
  from: number,
  caret: number,
  label: string,
): { text: string; caret: number } {
  return { text: text.slice(0, from) + label + text.slice(caret), caret: from + label.length };
}
