import {
  acceptCompletion,
  closeCompletion,
  ifNotIn,
  insertCompletionText,
  moveCompletionSelection,
  pickedCompletion,
  startCompletion,
  type Completion,
  type CompletionContext,
  type CompletionResult,
  type CompletionSource,
} from "@codemirror/autocomplete";
import {
  keywordCompletionSource,
  schemaCompletionSource,
  type SQLDialect,
  type SQLNamespace,
} from "@codemirror/lang-sql";
import { EditorSelection, EditorState, Prec, type Extension } from "@codemirror/state";
import { keymap, type EditorView, type KeyBinding } from "@codemirror/view";
import type { TableMeta, TableRef } from "@/lib/ipc";
import type { EditorAcceptKey } from "@/stores/prefs";
import { splitStatements, statementAt, type SqlDialect } from "./scanner";
import { builtinFunctions } from "./sqlFunctions";
import {
  findTable,
  identParts,
  quoteIdent,
  resolveQualifier,
  tablesInStatement,
  type FoundTable,
  type TableColumns,
} from "./statementTables";

/**
 * The SQL editor's own completion sources on top of `@codemirror/lang-sql` (TablePlus §5.2):
 * functions, "add all table fields" (`users.*`), JOIN … ON from the foreign keys, tables prefixed
 * with their schema, a whitespace after an accepted suggestion, and the accept key.
 */

const IN_TEXT = ["String", "LineComment", "BlockComment"];
const IN_TEXT_OR_QUOTED = [...IN_TEXT, "QuotedIdentifier"];

/** The statement around `pos` (searched in a window, so huge documents stay cheap). */
export function statementAround(
  state: EditorState,
  pos: number,
  dialect: SqlDialect,
): { text: string; start: number } {
  const WINDOW = 100_000;
  const from = Math.max(0, pos - WINDOW);
  const text = state.doc.sliceString(from, Math.min(state.doc.length, pos + WINDOW));
  const st = statementAt(splitStatements(text, dialect), text, pos - from, true);
  return st && st.start <= pos - from
    ? { text: text.slice(st.start, Math.max(st.end, pos - from)), start: from + st.start }
    : { text: text.slice(0, pos - from), start: from };
}

function mapResult(
  source: CompletionSource,
  f: (r: CompletionResult, ctx: CompletionContext) => CompletionResult | null,
): CompletionSource {
  return (ctx) => {
    const r = source(ctx);
    if (r instanceof Promise) return r.then((x) => (x ? f(x, ctx) : x));
    return r ? f(r, ctx) : r;
  };
}

// --- functions -------------------------------------------------------------------------------

/** A routine of the schema (the sidebar's Functions), offered like the built-in functions. */
export interface RoutineName {
  name: string;
  /** `function` or `procedure`. */
  kind: string;
}

/** Inserts `name()` with the caret between the brackets (or after `name(` when one follows). */
function applyFunction(name: string, closeBrackets: boolean) {
  return (view: EditorView, completion: Completion, from: number, to: number) => {
    const hasParen = view.state.sliceDoc(to, to + 1) === "(";
    const text = hasParen ? name : closeBrackets ? `${name}()` : `${name}(`;
    view.dispatch({
      ...insertCompletionText(view.state, text, from, to),
      annotations: pickedCompletion.of(completion),
      userEvent: "input.complete",
    });
    const back = hasParen ? -1 : closeBrackets ? 1 : 0;
    if (back === 0) return;
    const sel = view.state.selection;
    view.dispatch({
      selection: EditorSelection.create(
        sel.ranges.map((r) => EditorSelection.cursor(r.head - back)),
        sel.mainIndex,
      ),
    });
  };
}

/** Words that stay keyword completions although a function has the same name. */
const KEYWORD_TOO = new Set(
  (
    "left right replace if date time char year day hour minute month week second values user " +
    "current_timestamp like glob json truncate default format mod position"
  ).split(" "),
);

export interface FunctionSourceConfig {
  dialect: SqlDialect;
  upperCase: boolean;
  closeBrackets: boolean;
  routines?: readonly RoutineName[];
}

/** Function names: the engine's built-ins and the schema's routines. */
export function functionCompletionSource(cfg: FunctionSourceConfig): CompletionSource {
  const builtins: Completion[] = builtinFunctions(cfg.dialect).map((name) => {
    const label = cfg.upperCase ? name.toUpperCase() : name;
    return {
      label,
      type: "function",
      detail: "function",
      boost: -1,
      apply: applyFunction(label, cfg.closeBrackets),
    };
  });
  const routines: Completion[] = (cfg.routines ?? []).map((r) => ({
    label: r.name,
    type: "function",
    detail: r.kind,
    apply: applyFunction(quoteIdent(r.name, cfg.dialect), cfg.closeBrackets),
  }));
  const options = [...routines, ...builtins];
  return ifNotIn(IN_TEXT_OR_QUOTED, (ctx) => {
    const word = ctx.matchBefore(/[A-Za-z_][\w$]*$/);
    if (!word && !ctx.explicit) return null;
    const from = word?.from ?? ctx.pos;
    if (ctx.state.sliceDoc(from - 1, from) === ".") return null;
    return { from, options, validFor: /^[\w$]*$/ };
  });
}

/** Keywords, minus the ones the function source offers as functions (no duplicate COUNT). */
export function keywordSource(
  d: SQLDialect,
  dialect: SqlDialect,
  upperCase: boolean,
): CompletionSource {
  const fns = new Set(builtinFunctions(dialect).filter((f) => !KEYWORD_TOO.has(f)));
  return mapResult(keywordCompletionSource(d, upperCase), (r) => ({
    ...r,
    options: r.options.filter((o) => !fns.has(o.label.toLowerCase())),
  }));
}

// --- tables ----------------------------------------------------------------------------------

export interface SchemaSourceConfig {
  d: SQLDialect;
  dialect: SqlDialect;
  namespace: SQLNamespace;
  defaultSchema?: string;
  tables?: TableColumns;
  /** "Automatically prefix schemas": tables complete as `schema.table`, from every schema. */
  prefixSchemas: boolean;
}

/** Most tables of the other schemas offered (prefix mode). */
const OTHER_TABLES_MAX = 5_000;

/** lang-sql's schema / table / column completion, with the schema prefix when asked for. */
export function schemaSource(cfg: SchemaSourceConfig): CompletionSource {
  const base = schemaCompletionSource({
    dialect: cfg.d,
    schema: cfg.namespace,
    defaultSchema: cfg.defaultSchema,
  });
  const def = cfg.defaultSchema;
  if (!cfg.prefixSchemas || !def || !cfg.tables) return base;
  const q = (n: string) => quoteIdent(n, cfg.dialect);
  const schemaNames = new Set(Object.keys(cfg.tables));
  const defTables = new Set(Object.keys(cfg.tables[def] ?? {}));
  const others: Completion[] = [];
  for (const [schema, tables] of Object.entries(cfg.tables)) {
    if (schema === def) continue;
    for (const table of Object.keys(tables)) {
      if (others.length >= OTHER_TABLES_MAX) break;
      others.push({
        label: table,
        type: "type",
        detail: schema,
        boost: -1,
        apply: `${q(schema)}.${q(table)}`,
      });
    }
  }
  return mapResult(base, (r, ctx) => {
    const before = ctx.state.sliceDoc(r.from - 1, r.from);
    const quoted = /["`[]/.test(ctx.state.sliceDoc(r.from, r.from + 1));
    if (before === "." || quoted) return r;
    const options = r.options.map((o) =>
      o.type === "type" && defTables.has(o.label) && !schemaNames.has(o.label)
        ? { ...o, detail: def, apply: `${q(def)}.${q(o.label)}` }
        : o,
    );
    return { ...r, options: [...options, ...others] };
  });
}

/** `alias.` / `alias.*` at the cursor: its whole qualifier, without the dot and star. */
const QUALIFIED_STAR =
  /(?:[A-Za-z_][\w$]*|"(?:[^"]|"")+"|`[^`]+`)(?:\.(?:[A-Za-z_][\w$]*|"(?:[^"]|"")+"|`[^`]+`))?\.\*?$/;

export interface TablesConfig {
  dialect: SqlDialect;
  tables?: TableColumns;
  defaultSchema?: string;
}

/**
 * "Add all table fields": after `users.` (or `u.` for an alias, or `users.*`) a `users.*`
 * suggestion expands to every column, `users.id, users.name, …`.
 */
export function allFieldsSource(cfg: TablesConfig): CompletionSource {
  return ifNotIn(IN_TEXT, (ctx) => {
    const m = ctx.matchBefore(QUALIFIED_STAR);
    if (!m) return null;
    const qualifier = m.text.replace(/\.\*?$/, "");
    const stmt = statementAround(ctx.state, ctx.pos, cfg.dialect);
    const parts = resolveQualifier(stmt.text, identParts(qualifier));
    const table = findTable(cfg.tables, parts, cfg.defaultSchema);
    if (!table || table.columns.length === 0) return null;
    const apply = table.columns.map((c) => `${qualifier}.${quoteIdent(c, cfg.dialect)}`).join(", ");
    return {
      from: m.from,
      options: [
        {
          label: `${qualifier}.*`,
          detail: "add all table fields",
          type: "property",
          boost: 60,
          apply,
          info: apply.length > 400 ? `${apply.slice(0, 400)}…` : apply,
        },
      ],
    };
  });
}

// --- JOIN … ON ---------------------------------------------------------------------------------

const ID = String.raw`(?:"(?:[^"]|"")+"|\`[^\`]+\`|[A-Za-z_][\w$]*)`;
/** `JOIN table [[AS] alias] [ON] partial` right before the cursor. */
const JOIN_TAIL = new RegExp(
  String.raw`\bjoin\s+(${ID}(?:\s*\.\s*${ID})?)(?:\s+(?:as\s+)?(${ID}))?\s+(?:(on)\s+)?([\w$]*)$`,
  "i",
);
/** What can follow the joined table instead of an alias. */
const NOT_ALIAS =
  /^(using|where|join|inner|left|right|full|outer|cross|natural|group|order|limit|union|set)$/i;

export interface JoinSourceConfig extends TablesConfig {
  upperCase: boolean;
  /** Table metadata (its foreign keys); null when it cannot be loaded. */
  tableMeta: (table: TableRef) => Promise<TableMeta | null>;
}

const sameTable = (ref: TableRef, t: FoundTable) =>
  ref.name.toLowerCase() === t.name.toLowerCase() &&
  (ref.schema === null || ref.schema.toLowerCase() === t.schema.toLowerCase());

/**
 * JOIN generation (TablePlus b422): after `JOIN orders o `, suggestions like
 * `ON o.user_id = u.id`, one per foreign key between the joined table and a table the statement
 * already uses (either direction).
 */
export function joinCompletionSource(cfg: JoinSourceConfig): CompletionSource {
  return ifNotIn(IN_TEXT_OR_QUOTED, async (ctx) => {
    const stmt = statementAround(ctx.state, ctx.pos, cfg.dialect);
    const before = stmt.text.slice(0, ctx.pos - stmt.start);
    const tail = before.slice(-500);
    const m = JOIN_TAIL.exec(tail);
    if (!m) return null;
    let alias = m[2] ?? null;
    let on = m[3] !== undefined;
    const partial = m[4] ?? "";
    if (alias && /^on$/i.test(alias)) {
      if (on) return null;
      alias = null;
      on = true;
    }
    if (alias && NOT_ALIAS.test(alias)) return null;
    if (!on && partial && !"on".startsWith(partial.toLowerCase())) return null;
    const joined = findTable(cfg.tables, identParts(m[1]!), cfg.defaultSchema);
    if (!joined) return null;
    const head = before.slice(0, before.length - tail.length + m.index);
    const earlier = tablesInStatement(head).flatMap((use) => {
      const table = findTable(cfg.tables, use.parts, cfg.defaultSchema);
      return table ? [{ use, table }] : [];
    });
    if (earlier.length === 0) return null;
    const meta = (t: FoundTable) =>
      cfg.tableMeta({ schema: t.schema, name: t.name }).catch(() => null);
    const [joinedMeta, ...metas] = await Promise.all([
      meta(joined),
      ...earlier.map((e) => meta(e.table)),
    ]);
    const q = (n: string) => quoteIdent(n, cfg.dialect);
    const jRef = alias ?? m[1]!.replace(/\s+/g, "");
    const seen = new Set<string>();
    const conditions: { text: string; detail: string }[] = [];
    const add = (text: string, detail: string) => {
      if (seen.has(text)) return;
      seen.add(text);
      conditions.push({ text, detail });
    };
    earlier.forEach(({ use, table }, k) => {
      const oRef = use.alias ?? use.text;
      if (oRef === jRef) return;
      for (const fk of joinedMeta?.foreignKeys ?? [])
        if (sameTable(fk.refTable, table))
          add(
            fk.columns
              .map((c, i) => `${jRef}.${q(c)} = ${oRef}.${q(fk.refColumns[i] ?? c)}`)
              .join(" AND "),
            fk.name,
          );
      for (const fk of metas[k]?.foreignKeys ?? [])
        if (sameTable(fk.refTable, joined))
          add(
            fk.columns
              .map((c, i) => `${jRef}.${q(fk.refColumns[i] ?? c)} = ${oRef}.${q(c)}`)
              .join(" AND "),
            fk.name,
          );
    });
    if (conditions.length === 0) return null;
    const prefix = on ? "" : cfg.upperCase ? "ON " : "on ";
    return {
      from: ctx.pos - partial.length,
      options: conditions.map((c, i) => ({
        label: prefix + c.text,
        detail: c.detail,
        type: "join",
        boost: 90 - i,
      })),
    };
  });
}

// --- after a completion ------------------------------------------------------------------------

/** No space goes before these (or at the end of a line, where there is nothing to separate). */
const NO_SPACE_BEFORE = /[\s,;)(.\]]/;

/**
 * "Automatically add a whitespace": a space after an accepted suggestion, unless the caret is
 * already before whitespace or punctuation. Not after a function (the caret is inside its
 * brackets), a saved query, or a schema name (a `.` comes next).
 */
export function autoWhitespace(schemaNames: ReadonlySet<string>): Extension {
  return EditorState.transactionFilter.of((tr) => {
    const picked = tr.annotation(pickedCompletion);
    if (!picked || !tr.docChanged) return tr;
    if (picked.type === "function" || picked.type === "keyword-binding") return tr;
    if (picked.type === "type" && schemaNames.has(picked.label)) return tr;
    const sel = tr.newSelection;
    if (sel.ranges.some((r) => !r.empty)) return tr;
    const doc = tr.newDoc;
    const heads = sel.ranges.map((r) => r.head);
    if (heads.some((h) => NO_SPACE_BEFORE.test(doc.sliceString(h, h + 1)))) return tr;
    const sorted = [...heads].sort((a, b) => a - b);
    return [
      tr,
      {
        changes: sorted.map((h) => ({ from: h, insert: " " })),
        selection: EditorSelection.create(
          heads.map((h) => EditorSelection.cursor(h + 1 + sorted.filter((x) => x < h).length)),
          sel.mainIndex,
        ),
        sequential: true,
      },
    ];
  });
}

/**
 * The completion keys, with "Auto-complete key: Enter or Tab" deciding which keys accept a
 * suggestion (the other one keeps its usual meaning: a new line, an indent).
 */
export function completionKeys(accept: EditorAcceptKey): Extension {
  const bindings: KeyBinding[] = [
    { key: "Ctrl-Space", run: startCompletion },
    { mac: "Alt-`", run: startCompletion },
    { mac: "Alt-i", run: startCompletion },
    { key: "Escape", run: closeCompletion },
    { key: "ArrowDown", run: moveCompletionSelection(true) },
    { key: "ArrowUp", run: moveCompletionSelection(false) },
    { key: "PageDown", run: moveCompletionSelection(true, "page") },
    { key: "PageUp", run: moveCompletionSelection(false, "page") },
  ];
  if (accept !== "tab") bindings.push({ key: "Enter", run: acceptCompletion });
  if (accept !== "enter") bindings.push({ key: "Tab", run: acceptCompletion });
  return Prec.highest(keymap.of(bindings));
}
