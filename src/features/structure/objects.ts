import type { CheckDef, DdlOp, Dialect, TableMeta, TableOptions, TableRef } from "@/lib/ipc";
import { qualify, quoteIdent } from "@/lib/sql/quote";
import { checkColumns, foldStructure, parseTriggerSql, targets, type StructureView } from "./model";

/**
 * Staged edits of the table itself (name, comment, MySQL options) and of its CHECK constraints
 * and triggers. Same rules as the column reducers in `model.ts`: a repeated edit replaces the
 * earlier op, setting something back to the database's value removes it, an edit of an existing
 * check / trigger stages a drop directly followed by the new definition.
 */

const replaceAt = (ops: readonly DdlOp[], i: number, op: DdlOp): DdlOp[] => {
  const next = ops.slice();
  next[i] = op;
  return next;
};

const without = (ops: readonly DdlOp[], ...at: Array<number | null | undefined>): DdlOp[] =>
  ops.filter((_, i) => !at.includes(i));

/**
 * `ops` with the table's staged `renameTable` moved to the end: every other op names the table by
 * its database name, so the rename has to run last.
 */
export function withRenameLast(ops: DdlOp[], table: TableRef): DdlOp[] {
  const i = ops.findIndex((op) => op.op === "renameTable" && targets(op, table));
  if (i < 0 || i === ops.length - 1) return ops;
  return [...ops.slice(0, i), ...ops.slice(i + 1), ops[i]!];
}

/** The new name the staged ops give the table, or null when it keeps its name. */
export function stagedRename(ops: readonly DdlOp[], table: TableRef): string | null {
  const op = ops.find((o) => o.op === "renameTable" && targets(o, table));
  return op?.op === "renameTable" ? op.newName : null;
}

// --- table -------------------------------------------------------------------------------------

/** Stages renaming the table (TablePlus: edit the Name field of the structure header). */
export function setTableName(meta: TableMeta, ops: readonly DdlOp[], name: string): DdlOp[] {
  const newName = name.trim();
  const view = foldStructure(meta, ops);
  const k = view.info.opAt.name;
  if (!newName || newName === view.info.name) return ops as DdlOp[];
  if (newName === meta.table.name) return without(ops, k);
  const op: DdlOp = { op: "renameTable", table: meta.table, newName };
  return k === undefined ? [...ops, op] : replaceAt(ops, k, op);
}

export function setTableComment(
  meta: TableMeta,
  ops: readonly DdlOp[],
  comment: string | null,
): DdlOp[] {
  const value = comment !== null && comment.trim() !== "" ? comment : null;
  const view = foldStructure(meta, ops);
  const k = view.info.opAt.comment;
  if ((value ?? "") === (view.info.comment ?? "")) return ops as DdlOp[];
  if ((value ?? "") === (meta.comment ?? "")) return without(ops, k);
  const op: DdlOp = { op: "setTableComment", table: meta.table, comment: value };
  return k === undefined ? [...ops, op] : replaceAt(ops, k, op);
}

export type TableOptionField = "engine" | "charset" | "collation" | "autoIncrement" | "rowFormat";

const OPTION_FIELDS: TableOptionField[] = [
  "engine",
  "charset",
  "collation",
  "autoIncrement",
  "rowFormat",
];

const NO_OPTIONS: TableOptions = {
  engine: null,
  charset: null,
  collation: null,
  autoIncrement: null,
  rowFormat: null,
};

const sameOption = (a: string | number | null, b: string | number | null) =>
  String(a ?? "").toLowerCase() === String(b ?? "").toLowerCase();

/**
 * Stages one MySQL table option. All option changes share one `alterTableOptions` op (one ALTER
 * TABLE); a field set back to its database value leaves it. Picking a character set drops the
 * staged collation (the set's default applies); picking a collation sets its character set.
 */
export function setTableOption(
  meta: TableMeta,
  ops: readonly DdlOp[],
  field: TableOptionField,
  value: string | number | null,
): DdlOp[] {
  const view = foldStructure(meta, ops);
  const k = view.info.opAt.options;
  const staged =
    k === undefined ? NO_OPTIONS : (ops[k] as Extract<DdlOp, { op: "alterTableOptions" }>).options;
  const db = meta.options ?? NO_OPTIONS;
  const next: TableOptions = { ...staged };
  const v = value === "" ? null : value;
  if (field === "autoIncrement") next.autoIncrement = v === null ? null : Number(v);
  else next[field] = v === null ? null : String(v);
  if (field === "charset") next.collation = null;
  if (field === "collation" && typeof v === "string") next.charset = v.split("_")[0]!;
  // Fields equal to the database's value are not changed.
  for (const f of OPTION_FIELDS) if (next[f] !== null && sameOption(next[f], db[f])) next[f] = null;
  const empty = OPTION_FIELDS.every((f) => next[f] === null);
  if (empty) return k === undefined ? (ops as DdlOp[]) : without(ops, k);
  const op: DdlOp = { op: "alterTableOptions", table: meta.table, options: next };
  return k === undefined ? [...ops, op] : replaceAt(ops, k, op);
}

// --- CHECK constraints --------------------------------------------------------------------------

/** A free `<table>_<column>_check` / `<table>_check` style name (PostgreSQL's convention). */
export function suggestCheckName(view: StructureView, table: string, column?: string): string {
  const base = `${table}_${column ? `${column}_` : ""}check`.replace(/[^\w$]+/g, "_").slice(0, 60);
  const taken = new Set(view.checks.map((c) => c.name.toLowerCase()));
  let name = base;
  for (let n = 1; taken.has(name.toLowerCase()); n++) name = `${base}${n}`;
  return name;
}

export function addCheck(meta: TableMeta, ops: readonly DdlOp[], check: CheckDef): DdlOp[] {
  return [...ops, { op: "addCheck", table: meta.table, check }];
}

/**
 * Stages an edit of check `key` (name and / or expression). A staged new check changes in place;
 * an existing one is dropped and added again under the same name unless renamed.
 */
export function editCheck(
  meta: TableMeta,
  ops: readonly DdlOp[],
  key: string,
  patch: Partial<CheckDef>,
): DdlOp[] {
  const current = ops as DdlOp[];
  const row = foldStructure(meta, ops).checks.find((x) => x.key === key);
  if (!row || row.status === "dropped") return current;
  const next: CheckDef = {
    name: patch.name !== undefined ? patch.name : row.name || null,
    expression: (patch.expression ?? row.expression).trim(),
  };
  if (next.name === (row.name || null) && next.expression === row.expression) return current;
  const table = meta.table;
  if (row.status === "added") {
    // An emptied new check goes away.
    if (next.expression === "") return without(ops, row.opIndex);
    return replaceAt(ops, row.opIndex!, { op: "addCheck", table, check: next });
  }
  const original = row.original!;
  if (next.expression === "") return dropCheck(meta, ops, key);
  if (row.status === "changed") {
    if (next.name === original.name && next.expression === original.expression)
      return without(ops, row.opIndex, row.addAt);
    return replaceAt(ops, row.addAt!, { op: "addCheck", table, check: next });
  }
  return [
    ...ops,
    { op: "dropCheck", table, name: row.name },
    { op: "addCheck", table, check: next },
  ];
}

export function dropCheck(meta: TableMeta, ops: readonly DdlOp[], key: string): DdlOp[] {
  const row = foldStructure(meta, ops).checks.find((x) => x.key === key);
  if (!row || row.status === "dropped") return ops as DdlOp[];
  if (row.status === "added") return without(ops, row.opIndex);
  if (row.status === "changed") return without(ops, row.addAt);
  return [...ops, { op: "dropCheck", table: meta.table, name: row.name }];
}

/** Undoes a staged drop or edit of an existing check. */
export function restoreCheck(meta: TableMeta, ops: readonly DdlOp[], key: string): DdlOp[] {
  const row = foldStructure(meta, ops).checks.find((x) => x.key === key);
  if (row?.status !== "dropped" && row?.status !== "changed") return ops as DdlOp[];
  return without(ops, row.opIndex, row.addAt);
}

/** The check shown in a column's `check` cell: the first live check on that column alone. */
export function columnCheck(view: StructureView, column: string) {
  return (
    view.checks.find(
      (c) =>
        c.status !== "dropped" &&
        c.columns.length === 1 &&
        c.columns[0]!.toLowerCase() === column.toLowerCase(),
    ) ?? null
  );
}

/**
 * Stages "the `check` cell of `column` is `expression`" (PostgreSQL column CHECK): adds a
 * `<table>_<column>_check`, replaces the expression of the column's check, or drops it when
 * emptied.
 */
export function setColumnCheck(
  meta: TableMeta,
  ops: readonly DdlOp[],
  column: string,
  expression: string | null,
): DdlOp[] {
  const view = foldStructure(meta, ops);
  const expr = expression?.trim() ?? "";
  const row = columnCheck(view, column);
  if (!row) {
    if (!expr) return ops as DdlOp[];
    // The expression must use the column, or the cell would not show it.
    const names = view.columns.filter((c) => c.status !== "dropped").map((c) => c.name);
    const uses = checkColumns(expr, names);
    if (uses.length !== 1 || uses[0]!.toLowerCase() !== column.toLowerCase()) return ops as DdlOp[];
    return addCheck(meta, ops, {
      name: suggestCheckName(view, meta.table.name, column),
      expression: expr,
    });
  }
  if (!expr) return dropCheck(meta, ops, row.key);
  return editCheck(meta, ops, row.key, { expression: expr });
}

// --- triggers ------------------------------------------------------------------------------------

/** A starting `CREATE TRIGGER` statement for the table, per dialect (TablePlus "+ Trigger"). */
export function triggerTemplate(dialect: Dialect, table: TableRef, name: string): string {
  const target =
    dialect === "sqlite"
      ? table.schema && table.schema !== "main"
        ? qualify(dialect, table.schema, table.name)
        : quoteIdent(dialect, table.name)
      : qualify(dialect, table.schema, table.name);
  const trigger = quoteIdent(dialect, name);
  if (dialect === "postgres")
    return [
      `CREATE TRIGGER ${trigger}`,
      `BEFORE INSERT OR UPDATE ON ${target}`,
      "FOR EACH ROW",
      "-- the function must exist: CREATE FUNCTION … RETURNS trigger",
      "EXECUTE FUNCTION trigger_function();",
    ].join("\n");
  if (dialect === "mysql")
    return [
      `CREATE TRIGGER ${trigger}`,
      `BEFORE INSERT ON ${target}`,
      "FOR EACH ROW",
      "BEGIN",
      "  -- SET NEW.created_at = NOW();",
      "END",
    ].join("\n");
  return [
    `CREATE TRIGGER ${trigger}`,
    `AFTER INSERT ON ${target}`,
    "FOR EACH ROW",
    "BEGIN",
    "  SELECT 1;",
    "END",
  ].join("\n");
}

/**
 * The statement to edit an existing trigger with. PostgreSQL and SQLite report the whole CREATE
 * statement; MySQL only the body, so the head is rebuilt from timing and event.
 */
export function triggerSource(
  dialect: Dialect,
  table: TableRef,
  t: {
    name: string;
    timing: string | null;
    events: string[];
    definition: string;
    sql: string | null;
  },
): string {
  if (t.sql !== null) return t.sql;
  if (dialect !== "mysql" || /^\s*create\b/i.test(t.definition)) return t.definition.trim();
  return [
    `CREATE TRIGGER ${quoteIdent(dialect, t.name)}`,
    `${t.timing ?? "BEFORE"} ${t.events[0] ?? "INSERT"} ON ${qualify(dialect, table.schema, table.name)}`,
    "FOR EACH ROW",
    t.definition.trim(),
  ].join("\n");
}

/** A free `<table>_<n>_trigger`-style name. */
export function suggestTriggerName(view: StructureView, table: string): string {
  const base = `${table}_trigger`.replace(/[^\w$]+/g, "_").slice(0, 60);
  const taken = new Set(view.triggers.map((t) => t.name.toLowerCase()));
  let name = base;
  for (let n = 1; taken.has(name.toLowerCase()); n++) name = `${base}${n}`;
  return name;
}

export type TriggerResult = { ok: true; ops: DdlOp[] } | { ok: false; error: string };

const NO_TRIGGER_NAME = "Write a CREATE TRIGGER statement (with the trigger name)";

/** Stages a new trigger from its CREATE TRIGGER statement. */
export function addTrigger(meta: TableMeta, ops: readonly DdlOp[], sql: string): TriggerResult {
  const name = parseTriggerSql(sql).name;
  if (!name) return { ok: false, error: NO_TRIGGER_NAME };
  const view = foldStructure(meta, ops);
  if (view.triggers.some((t) => t.status !== "dropped" && t.name === name))
    return { ok: false, error: `A trigger named “${name}” already exists` };
  return {
    ok: true,
    ops: [...ops, { op: "createTrigger", table: meta.table, name, sql: sql.trim() }],
  };
}

/**
 * Stages an edited trigger statement: a staged new trigger changes in place; an existing one is
 * dropped and created again (adjacent ops). Unchanged text stages nothing.
 */
export function editTrigger(
  meta: TableMeta,
  ops: readonly DdlOp[],
  dialect: Dialect,
  key: string,
  sql: string,
): TriggerResult {
  const view = foldStructure(meta, ops);
  const row = view.triggers.find((t) => t.key === key);
  if (!row || row.status === "dropped") return { ok: true, ops: ops as DdlOp[] };
  const name = parseTriggerSql(sql).name;
  if (!name) return { ok: false, error: NO_TRIGGER_NAME };
  if (name !== row.name && view.triggers.some((t) => t.status !== "dropped" && t.name === name))
    return { ok: false, error: `A trigger named “${name}” already exists` };
  const text = sql.trim();
  const table = meta.table;
  const create: DdlOp = { op: "createTrigger", table, name, sql: text };
  if (row.status === "added") return { ok: true, ops: replaceAt(ops, row.opIndex!, create) };
  const original = triggerSource(dialect, table, { ...row.original!, sql: null }).trim();
  if (row.status === "changed") {
    if (text === original && name === row.original!.name)
      return { ok: true, ops: without(ops, row.opIndex, row.addAt) };
    return { ok: true, ops: replaceAt(ops, row.addAt!, create) };
  }
  if (text === original && name === row.name) return { ok: true, ops: ops as DdlOp[] };
  return {
    ok: true,
    ops: [...ops, { op: "dropTrigger", table, name: row.name }, create],
  };
}

export function dropTrigger(meta: TableMeta, ops: readonly DdlOp[], key: string): DdlOp[] {
  const row = foldStructure(meta, ops).triggers.find((x) => x.key === key);
  if (!row || row.status === "dropped") return ops as DdlOp[];
  if (row.status === "added") return without(ops, row.opIndex);
  if (row.status === "changed") return without(ops, row.addAt);
  return [...ops, { op: "dropTrigger", table: meta.table, name: row.name }];
}

export function restoreTrigger(meta: TableMeta, ops: readonly DdlOp[], key: string): DdlOp[] {
  const row = foldStructure(meta, ops).triggers.find((x) => x.key === key);
  if (row?.status !== "dropped" && row?.status !== "changed") return ops as DdlOp[];
  return without(ops, row.opIndex, row.addAt);
}
