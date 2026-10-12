import type {
  ColumnDef,
  ColumnMeta,
  DdlOp,
  Dialect,
  FkDef,
  IndexDef,
  TableMeta,
  TableRef,
} from "@/lib/ipc";
import {
  columnOp,
  newColumnDef,
  supportsColumnPosition,
  withField,
  type ColumnField,
  type ColumnValue,
} from "./columnEdits";

/**
 * Staged structure edits are an ordered list of `DdlOp`s, exactly what `preview_ddl` /
 * `apply_ddl` receive. Ops run in order, so every op names a column by the name it has at that
 * point of the list. The reducers below keep the list short and valid: a second edit of the same
 * attribute replaces the first one in place, setting an attribute back to its database value
 * removes the op, edits of a staged new column fold into its `addColumn` definition, and dropping
 * a column discards the staged edits that only concerned it.
 */

export type RowStatus = "existing" | "added" | "dropped";

type ExistingField = Exclude<ColumnField, "autoIncrement">;

export interface ColumnRow {
  /** Stable React key: `c:<db name>` for existing columns, `n:<k>` for the k-th staged column. */
  key: string;
  /** Name in the database; null for a staged new column. */
  origName: string | null;
  name: string;
  dataType: string;
  nullable: boolean;
  defaultExpr: string | null;
  comment: string | null;
  autoIncrement: boolean;
  isPk: boolean;
  status: RowStatus;
  /** Attributes of an existing column that differ from the database. */
  changed: ColumnField[];
  /** Position in the op list of the op that set each attribute / added / dropped the column. */
  opAt: Partial<Record<ExistingField | "add" | "drop", number>>;
  /** Positions of staged `addIndex` / `addForeignKey` ops that use this column. */
  refs: number[];
  /** MySQL `AFTER` of a staged new column. */
  after: string | null;
  meta: ColumnMeta | null;
}

export interface IndexRow {
  key: string;
  name: string;
  columns: string[];
  unique: boolean;
  primary: boolean;
  method: string | null;
  predicate: string | null;
  definition: string | null;
  status: RowStatus;
  /** Op that added (status added) or dropped (status dropped) the index. */
  opIndex: number | null;
}

export interface FkRow {
  key: string;
  name: string | null;
  columns: string[];
  refTable: TableRef;
  refColumns: string[];
  onUpdate: string | null;
  onDelete: string | null;
  status: RowStatus;
  opIndex: number | null;
}

export interface StructureView {
  columns: ColumnRow[];
  indexes: IndexRow[];
  foreignKeys: FkRow[];
  /** Primary key columns (current names) in key order, once the staged ops are applied. */
  primaryKey: string[];
}

type AddColumnOp = Extract<DdlOp, { op: "addColumn" }>;

export function sameRef(a: TableRef, b: TableRef): boolean {
  return a.name === b.name && (a.schema ?? "") === (b.schema ?? "");
}

/** True when `op` changes `table` (raw SQL targets no particular table). */
export function targets(op: DdlOp, table: TableRef): boolean {
  return op.op !== "rawSql" && sameRef(op.table, table);
}

/**
 * `ops` with every reference to table `from` pointed at `to` (the op's own table and the
 * referenced table of a self-referencing foreign key), after the table was renamed. Returns the
 * same array when nothing refers to `from`.
 */
export function retargetOps(ops: DdlOp[], from: TableRef, to: TableRef): DdlOp[] {
  let changed = false;
  const next = ops.map((op) => {
    if (op.op === "rawSql") return op;
    let out = op;
    if (sameRef(op.table, from)) out = { ...out, table: to };
    if (out.op === "addForeignKey" && sameRef(out.fk.refTable, from))
      out = { ...out, fk: { ...out.fk, refTable: to } };
    if (out !== op) changed = true;
    return out;
  });
  return changed ? next : ops;
}

/** Column name part of an index entry: `email`, `name(10)`, `created_at DESC` → the name. */
export function entryBase(entry: string): string {
  const m = /^(.*?)(\s*\(\s*\d+\s*\))?(\s+(?:asc|desc))?$/i.exec(entry.trim());
  return m?.[1] ?? entry;
}

function renameEntry(entry: string, from: string, to: string): string {
  if (entry === from) return to;
  const base = entryBase(entry);
  return base === from ? to + entry.trim().slice(base.length) : entry;
}

/**
 * `op` with every reference to column `from` of `table` renamed to `to` (column ops, MySQL
 * `AFTER`, staged index / foreign key / primary key column lists).
 */
export function renameRefs(op: DdlOp, table: TableRef, from: string, to: string): DdlOp {
  if (op.op === "addForeignKey" && sameRef(op.fk.refTable, table)) {
    op = {
      ...op,
      fk: { ...op.fk, refColumns: op.fk.refColumns.map((c) => (c === from ? to : c)) },
    };
  }
  if (!targets(op, table)) return op;
  switch (op.op) {
    case "dropColumn":
    case "renameColumn":
    case "alterColumnType":
    case "alterColumnNullable":
    case "alterColumnDefault":
    case "setColumnComment":
      return op.name === from ? { ...op, name: to } : op;
    case "addColumn":
      return op.after === from ? { ...op, after: to } : op;
    case "addIndex":
      return {
        ...op,
        index: { ...op.index, columns: op.index.columns.map((c) => renameEntry(c, from, to)) },
      };
    case "addForeignKey":
      return { ...op, fk: { ...op.fk, columns: op.fk.columns.map((c) => (c === from ? to : c)) } };
    case "addPrimaryKey":
      return { ...op, columns: op.columns.map((c) => (c === from ? to : c)) };
    default:
      return op;
  }
}

function existingRow(c: ColumnMeta): ColumnRow {
  return {
    key: `c:${c.name}`,
    origName: c.name,
    name: c.name,
    dataType: c.dataType,
    nullable: c.nullable,
    defaultExpr: c.defaultExpr,
    comment: c.comment,
    autoIncrement: c.isAutoIncrement,
    isPk: c.isPk,
    status: "existing",
    changed: [],
    opAt: {},
    refs: [],
    after: null,
    meta: c,
  };
}

function addedRow(op: AddColumnOp, index: number, key: string): ColumnRow {
  const d = op.column;
  return {
    key,
    origName: null,
    name: d.name,
    dataType: d.dataType,
    nullable: d.nullable,
    defaultExpr: d.defaultExpr,
    comment: d.comment,
    autoIncrement: d.autoIncrement,
    isPk: false,
    status: "added",
    changed: [],
    opAt: { add: index },
    refs: [],
    after: op.after,
    meta: null,
  };
}

const EXISTING_FIELDS: ExistingField[] = ["name", "dataType", "nullable", "defaultExpr", "comment"];

function originalOf(meta: ColumnMeta, field: ExistingField): ColumnValue {
  switch (field) {
    case "name":
      return meta.name;
    case "dataType":
      return meta.dataType;
    case "nullable":
      return meta.nullable;
    case "defaultExpr":
      return meta.defaultExpr;
    case "comment":
      return meta.comment;
  }
}

function currentOf(row: ColumnRow, field: ColumnField): ColumnValue {
  return row[field];
}

/** The table as it will look once the staged ops are applied, with per-row staging state. */
export function foldStructure(meta: TableMeta, ops: readonly DdlOp[]): StructureView {
  const table = meta.table;
  const columns = meta.columns.map(existingRow);
  const indexes: IndexRow[] = meta.indexes.map((i) => ({
    key: `i:${i.name}`,
    name: i.name,
    columns: i.columns,
    unique: i.unique,
    primary: i.primary,
    method: i.method,
    predicate: i.predicate,
    definition: i.definition,
    status: "existing",
    opIndex: null,
  }));
  const foreignKeys: FkRow[] = meta.foreignKeys.map((f) => ({
    key: `f:${f.name}`,
    name: f.name,
    columns: f.columns,
    refTable: f.refTable,
    refColumns: f.refColumns,
    onUpdate: f.onUpdate,
    onDelete: f.onDelete,
    status: "existing",
    opIndex: null,
  }));
  let addedColumns = 0;
  let addedIndexes = 0;
  let addedFks = 0;
  let primaryKey = [...meta.primaryKey];
  const live = (name: string) => columns.find((c) => c.status !== "dropped" && c.name === name);

  ops.forEach((op, i) => {
    if (!targets(op, table)) return;
    switch (op.op) {
      case "addColumn": {
        const row = addedRow(op, i, `n:${addedColumns++}`);
        let at = columns.length;
        if (op.after === "") at = 0;
        else if (op.after !== null) {
          const j = columns.findIndex((c) => c.status !== "dropped" && c.name === op.after);
          if (j >= 0) at = j + 1;
        }
        columns.splice(at, 0, row);
        return;
      }
      case "dropColumn": {
        const r = live(op.name);
        if (r) {
          r.status = "dropped";
          r.opAt.drop = i;
        }
        // As the backend models it: a dropped key column leaves the key.
        primaryKey = primaryKey.filter((c) => c !== op.name);
        return;
      }
      case "renameColumn": {
        const r = live(op.name);
        if (r) {
          r.name = op.newName;
          r.opAt.name = i;
        }
        primaryKey = primaryKey.map((c) => (c === op.name ? op.newName : c));
        return;
      }
      case "alterColumnType": {
        const r = live(op.name);
        if (r) {
          r.dataType = op.newType;
          r.opAt.dataType = i;
        }
        return;
      }
      case "alterColumnNullable": {
        const r = live(op.name);
        if (r) {
          r.nullable = op.nullable;
          r.opAt.nullable = i;
        }
        return;
      }
      case "alterColumnDefault": {
        const r = live(op.name);
        if (r) {
          r.defaultExpr = op.defaultExpr;
          r.opAt.defaultExpr = i;
        }
        return;
      }
      case "setColumnComment": {
        const r = live(op.name);
        if (r) {
          r.comment = op.comment;
          r.opAt.comment = i;
        }
        return;
      }
      case "addPrimaryKey":
        for (const name of op.columns) {
          const r = live(name);
          if (r) {
            r.isPk = true;
            // Key columns become NOT NULL (every dialect, also in the SQLite rebuild).
            r.nullable = false;
          }
        }
        primaryKey = [...op.columns];
        return;
      case "dropPrimaryKey":
        for (const c of columns) c.isPk = false;
        primaryKey = [];
        return;
      case "addIndex": {
        const x = op.index;
        indexes.push({
          key: `ni:${addedIndexes++}`,
          name: x.name,
          columns: x.columns,
          unique: x.unique,
          primary: false,
          method: x.method,
          predicate: x.predicate,
          definition: null,
          status: "added",
          opIndex: i,
        });
        for (const entry of x.columns) live(entryBase(entry))?.refs.push(i);
        return;
      }
      case "dropIndex": {
        const r = indexes.find((x) => x.status !== "dropped" && x.name === op.name);
        if (r) {
          r.status = "dropped";
          r.opIndex = i;
        }
        return;
      }
      case "addForeignKey": {
        const f = op.fk;
        foreignKeys.push({
          key: `nf:${addedFks++}`,
          name: f.name,
          columns: f.columns,
          refTable: f.refTable,
          refColumns: f.refColumns,
          onUpdate: f.onUpdate,
          onDelete: f.onDelete,
          status: "added",
          opIndex: i,
        });
        for (const name of f.columns) live(name)?.refs.push(i);
        return;
      }
      case "dropForeignKey": {
        const r = foreignKeys.find((x) => x.status !== "dropped" && x.name === op.name);
        if (r) {
          r.status = "dropped";
          r.opIndex = i;
        }
        return;
      }
      default:
        return;
    }
  });

  // Existing indexes / keys list database names; show the columns' staged names instead.
  const renamed = new Map<string, string>();
  for (const c of columns) {
    if (c.meta) {
      c.changed = EXISTING_FIELDS.filter((f) => currentOf(c, f) !== originalOf(c.meta!, f));
      if (c.name !== c.meta.name) renamed.set(c.meta.name, c.name);
    }
  }
  if (renamed.size > 0) {
    for (const x of indexes)
      if (x.status !== "added")
        x.columns = x.columns.map((e) => {
          const base = entryBase(e);
          const to = renamed.get(base);
          return to ? renameEntry(e, base, to) : e;
        });
    for (const f of foreignKeys)
      if (f.status !== "added") f.columns = f.columns.map((c) => renamed.get(c) ?? c);
  }
  return { columns, indexes, foreignKeys, primaryKey };
}

// --- reducers ------------------------------------------------------------------------------

function replaceAt(ops: readonly DdlOp[], i: number, op: DdlOp): DdlOp[] {
  const next = ops.slice();
  next[i] = op;
  return next;
}

/** Applies `renameRefs` to every op from position `start` on. */
function rewriteFrom(
  ops: DdlOp[],
  start: number,
  table: TableRef,
  from: string,
  to: string,
): DdlOp[] {
  if (from === to) return ops;
  return ops.map((op, j) => (j >= start ? renameRefs(op, table, from, to) : op));
}

/** Names a column may not take: every other column's current and database name. */
export function namesInUse(view: StructureView, exceptKey?: string): Set<string> {
  const names = new Set<string>();
  for (const c of view.columns) {
    if (c.key === exceptKey) continue;
    names.add(c.name.toLowerCase());
    if (c.origName) names.add(c.origName.toLowerCase());
  }
  return names;
}

/** Error message when `name` cannot be given to the column `rowKey`, else null. */
export function columnNameError(view: StructureView, rowKey: string, name: string): string | null {
  const row = view.columns.find((c) => c.key === rowKey);
  if (!name.trim()) return "Column name is required";
  if (row && row.origName === name) return null;
  if (namesInUse(view, rowKey).has(name.toLowerCase()))
    return `A column named “${name}” already exists`;
  return null;
}

/**
 * Stages "set `field` of column `rowKey` to `value`". `value` is already parsed
 * (`parseColumnInput`): trimmed names / types, null for "no default" / "no comment".
 */
export function editColumn(
  meta: TableMeta,
  ops: readonly DdlOp[],
  dialect: Dialect,
  rowKey: string,
  field: ColumnField,
  value: ColumnValue,
): DdlOp[] {
  const current = ops as DdlOp[];
  const view = foldStructure(meta, ops);
  const row = view.columns.find((c) => c.key === rowKey);
  if (!row || row.status === "dropped" || currentOf(row, field) === value) return current;
  const table = meta.table;

  if (row.status === "added") {
    const i = row.opAt.add!;
    const op = ops[i] as AddColumnOp;
    const column: ColumnDef = withField(dialect, op.column, field, value);
    if (column === op.column) return current;
    const next = replaceAt(ops, i, { ...op, column });
    return field === "name" ? rewriteFrom(next, i + 1, table, row.name, column.name) : next;
  }

  if (field === "autoIncrement" || !row.meta) return current;
  const original = originalOf(row.meta, field);
  const k = row.opAt[field];

  if (field === "name") {
    const to = String(value);
    if (k === undefined) {
      const op = columnOp(dialect, table, row.name, "name", to);
      return op ? [...ops, op] : current;
    }
    if (to === original) {
      const next = ops.filter((_, j) => j !== k);
      return rewriteFrom(next, k, table, row.name, to);
    }
    const prev = ops[k] as Extract<DdlOp, { op: "renameColumn" }>;
    return rewriteFrom(replaceAt(ops, k, { ...prev, newName: to }), k + 1, table, row.name, to);
  }

  if (k !== undefined) {
    if (value === original) return ops.filter((_, j) => j !== k);
    const prev = ops[k]!;
    if (prev.op === "renameColumn" || prev.op === "rawSql") return current;
    const op = columnOp(dialect, table, (prev as { name: string }).name, field, value);
    return op ? replaceAt(ops, k, op) : current;
  }
  const op = columnOp(dialect, table, row.name, field, value);
  return op ? [...ops, op] : current;
}

/**
 * Stages a new column `column_<n>` (MySQL: placed after `after`, '' = first). Returns the new op
 * list and the row key of the new column.
 */
export function addColumn(
  meta: TableMeta,
  ops: readonly DdlOp[],
  dialect: Dialect,
  after: string | null = null,
): { ops: DdlOp[]; key: string } {
  const view = foldStructure(meta, ops);
  const taken = namesInUse(view);
  let n = 1;
  while (taken.has(`column_${n}`)) n++;
  const count = ops.filter((o) => o.op === "addColumn" && targets(o, meta.table)).length;
  const op: DdlOp = {
    op: "addColumn",
    table: meta.table,
    column: newColumnDef(dialect, `column_${n}`),
    after: supportsColumnPosition(dialect) ? after : null,
  };
  return { ops: [...ops, op], key: `n:${count}` };
}

/**
 * Stages dropping column `rowKey`. A staged new column just disappears (with the staged indexes
 * and keys that used it); an existing column loses its other staged edits and is dropped under
 * its database name.
 */
export function dropColumn(meta: TableMeta, ops: readonly DdlOp[], rowKey: string): DdlOp[] {
  const view = foldStructure(meta, ops);
  const row = view.columns.find((c) => c.key === rowKey);
  if (!row || row.status === "dropped") return ops as DdlOp[];
  const table = meta.table;

  if (row.status === "added") {
    const remove = new Set([row.opAt.add!, ...row.refs]);
    const next = ops
      .filter((_, i) => !remove.has(i))
      .map((op) =>
        op.op === "addColumn" && targets(op, table) && op.after === row.name
          ? { ...op, after: row.after }
          : op,
      );
    return withoutNoopKeyOps(meta, withoutKeyColumn(next, table, row.name));
  }

  const origName = row.origName!;
  const remove = new Set<number>(row.refs);
  for (const f of EXISTING_FIELDS) {
    const k = row.opAt[f];
    if (k !== undefined) remove.add(k);
  }
  const renameAt = row.opAt.name;
  const next: DdlOp[] = [];
  ops.forEach((op, i) => {
    if (remove.has(i)) return;
    next.push(
      renameAt !== undefined && i > renameAt ? renameRefs(op, table, row.name, origName) : op,
    );
  });
  next.push({ op: "dropColumn", table, name: origName });
  // Staged key ops before the drop name the column by its database name again (see above).
  return withoutNoopKeyOps(meta, withoutKeyColumn(next, table, origName));
}

/**
 * `ops` with column `name` taken out of the staged `addPrimaryKey` ops of `table` (an op left
 * without columns goes), so a staged key never names a column that is being dropped or was
 * never created. A staged `dropPrimaryKey` stays: the key then goes, as it would with the column
 * (unless what is left is the database's key again: see `withoutNoopKeyOps`).
 */
function withoutKeyColumn(ops: DdlOp[], table: TableRef, name: string): DdlOp[] {
  if (!ops.some((op) => op.op === "addPrimaryKey" && targets(op, table))) return ops;
  return ops.flatMap((op): DdlOp[] => {
    if (op.op !== "addPrimaryKey" || !targets(op, table) || !op.columns.includes(name)) return [op];
    const columns = op.columns.filter((c) => c !== name);
    return columns.length > 0 ? [{ ...op, columns }] : [];
  });
}

const isKeyOp = (op: DdlOp, table: TableRef) =>
  (op.op === "addPrimaryKey" || op.op === "dropPrimaryKey") && targets(op, table);

/**
 * Whether `columns` (names in `view`, in key order) are the database's primary key: the same
 * columns by database name, in the same order. A staged rename does not change which columns
 * form the key; a staged new column is never part of it.
 */
function isDatabaseKey(meta: TableMeta, view: StructureView, columns: readonly string[]): boolean {
  return (
    columns.length === meta.primaryKey.length &&
    columns.every((name, i) => {
      const db = view.columns.find((c) => c.status !== "dropped" && c.name === name)?.origName;
      return db !== null && db !== undefined && db === meta.primaryKey[i];
    })
  );
}

/**
 * `ops` without the table's staged key ops when, applied, they leave the database's key as it
 * was: e.g. a column toggled into the key and then dropped (or a staged new key column removed
 * again) would otherwise keep a DROP + ADD PRIMARY KEY of the same key, which rebuilds it for
 * nothing (MySQL) or fails when foreign keys reference it (PostgreSQL).
 */
function withoutNoopKeyOps(meta: TableMeta, ops: DdlOp[]): DdlOp[] {
  const table = meta.table;
  if (!ops.some((op) => isKeyOp(op, table))) return ops;
  const view = foldStructure(meta, ops);
  return isDatabaseKey(meta, view, view.primaryKey) ? ops.filter((op) => !isKeyOp(op, table)) : ops;
}

/**
 * Stages "the primary key is `columns`" (current names, in key order). Earlier staged key ops of
 * the table are replaced: nothing is staged when `columns` is the database's key (same order),
 * `addPrimaryKey` when the table has none, else `dropPrimaryKey` followed by the new key (if
 * any). The two stay adjacent at the end of the list, so MySQL replaces the key in one ALTER.
 */
export function setPrimaryKey(
  meta: TableMeta,
  ops: readonly DdlOp[],
  columns: readonly string[],
): DdlOp[] {
  const table = meta.table;
  const base = ops.filter((op) => !isKeyOp(op, table));
  // Nothing staged and nothing to stage: keep the list (no undo step for a no-op).
  if (isDatabaseKey(meta, foldStructure(meta, base), columns))
    return base.length === ops.length ? (ops as DdlOp[]) : base;
  const add: DdlOp[] =
    columns.length > 0 ? [{ op: "addPrimaryKey", table, columns: [...columns] }] : [];
  if (meta.primaryKey.length === 0) return add.length > 0 ? [...base, ...add] : base;
  return [...base, { op: "dropPrimaryKey", table }, ...add];
}

/**
 * Toggles column `rowKey` in the primary key: on appends it to the current key order, off takes
 * it out. When that leaves exactly the database key's columns, they take the database's order
 * again, so toggling a key column off and back on stages nothing (rather than a reordered key).
 * Dropped columns are left alone.
 */
export function togglePrimaryKey(meta: TableMeta, ops: readonly DdlOp[], rowKey: string): DdlOp[] {
  const view = foldStructure(meta, ops);
  const row = view.columns.find((c) => c.key === rowKey);
  if (!row || row.status === "dropped") return ops as DdlOp[];
  const key = view.primaryKey;
  const next = key.includes(row.name) ? key.filter((c) => c !== row.name) : [...key, row.name];
  // The database key's columns under their current names (null when one was dropped).
  const dbKey = meta.primaryKey.map(
    (db) => view.columns.find((c) => c.status !== "dropped" && c.origName === db)?.name ?? null,
  );
  const sameColumns =
    dbKey.length === next.length && dbKey.every((name) => name !== null && next.includes(name));
  return setPrimaryKey(meta, ops, sameColumns ? (dbKey as string[]) : next);
}

/** Undoes a staged drop of an existing column. */
export function restoreColumn(meta: TableMeta, ops: readonly DdlOp[], rowKey: string): DdlOp[] {
  const row = foldStructure(meta, ops).columns.find((c) => c.key === rowKey);
  const k = row?.status === "dropped" ? row.opAt.drop : undefined;
  return k === undefined ? (ops as DdlOp[]) : ops.filter((_, i) => i !== k);
}

export function addIndex(meta: TableMeta, ops: readonly DdlOp[], index: IndexDef): DdlOp[] {
  return [...ops, { op: "addIndex", table: meta.table, index }];
}

/** Drops an existing index (not the primary key) or discards a staged one. */
export function dropIndex(meta: TableMeta, ops: readonly DdlOp[], key: string): DdlOp[] {
  const row = foldStructure(meta, ops).indexes.find((x) => x.key === key);
  if (!row || row.status === "dropped" || row.primary) return ops as DdlOp[];
  if (row.status === "added") return ops.filter((_, i) => i !== row.opIndex);
  return [...ops, { op: "dropIndex", table: meta.table, name: row.name }];
}

export function restoreIndex(meta: TableMeta, ops: readonly DdlOp[], key: string): DdlOp[] {
  const row = foldStructure(meta, ops).indexes.find((x) => x.key === key);
  if (row?.status !== "dropped") return ops as DdlOp[];
  return ops.filter((_, i) => i !== row.opIndex);
}

export function addForeignKey(meta: TableMeta, ops: readonly DdlOp[], fk: FkDef): DdlOp[] {
  return [...ops, { op: "addForeignKey", table: meta.table, fk }];
}

/** Drops an existing foreign key or discards a staged one. */
export function dropForeignKey(meta: TableMeta, ops: readonly DdlOp[], key: string): DdlOp[] {
  const row = foldStructure(meta, ops).foreignKeys.find((x) => x.key === key);
  if (!row || row.status === "dropped") return ops as DdlOp[];
  if (row.status === "added") return ops.filter((_, i) => i !== row.opIndex);
  if (!row.name) return ops as DdlOp[];
  return [...ops, { op: "dropForeignKey", table: meta.table, name: row.name }];
}

export function restoreForeignKey(meta: TableMeta, ops: readonly DdlOp[], key: string): DdlOp[] {
  const row = foldStructure(meta, ops).foreignKeys.find((x) => x.key === key);
  if (row?.status !== "dropped") return ops as DdlOp[];
  return ops.filter((_, i) => i !== row.opIndex);
}
