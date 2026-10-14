import { useMemo } from "react";
import { create } from "zustand";
import { routineItemKey, sequenceItemKey, tableItemKey } from "@/features/sidebar/groups";
import { ipc, type ObjectOp, type RoutineRef, type TableRef } from "@/lib/ipc";

/**
 * Sidebar Truncate… / Delete… (of tables and views, functions / procedures, sequences) are staged
 * like every other GUI change (TablePlus, spec §2.4 / §9): they wait here, per connection, until
 * ⌘S commits them with the workspace's other changes (`features/commit/workspace.ts`), ⇧⌘⌫
 * discards them, the tab that carries them in the unsaved-changes guard is closed with Discard,
 * or the connection goes away.
 */
export type PendingOpKind = "drop" | "truncate";

export type StagedObjectOp = Extract<
  ObjectOp,
  { op: "dropTables" | "truncateTables" | "dropRoutines" | "dropSequences" }
>;

/** One object a staged op acts on. */
export interface OpTarget {
  type: "table" | "routine" | "sequence";
  /** Its sidebar item key (`groups.ts`): `t:` tables and views, `f:` routines, `s:` sequences. */
  key: string;
  /** How the sheet and the toasts name it (a routine with its argument types). */
  name: string;
}

export interface PendingObjectOp {
  id: string;
  kind: PendingOpKind;
  op: StagedObjectOp;
  /** The SQL the op was previewed with when it was staged. */
  statements: string[];
  /** The database the connection was on (a database switch discards the op). */
  database: string | null;
  /** Why the last commit stopped at this op; null when it has not failed. */
  error: string | null;
}

interface PendingOpsState {
  byConnection: Record<string, PendingObjectOp[]>;
}

export const EMPTY_OPS: readonly PendingObjectOp[] = Object.freeze([]);

export const usePendingOpsStore = create<PendingOpsState>()(() => ({
  byConnection: {},
}));

export function kindOf(op: StagedObjectOp): PendingOpKind {
  return op.op === "truncateTables" ? "truncate" : "drop";
}

/** `slugify(value text)`; a routine without arguments by its name alone. */
export function routineName(r: RoutineRef): string {
  return r.identityArguments ? `${r.name}(${r.identityArguments})` : r.name;
}

/** What `op` acts on: one target per table / view, routine (overload) or sequence. */
export function opTargets(op: StagedObjectOp): OpTarget[] {
  switch (op.op) {
    case "dropTables":
      return op.tables.map((t) => ({
        type: "table",
        key: tableItemKey(t.table),
        name: t.table.name,
      }));
    case "truncateTables":
      return op.tables.map((t) => ({ type: "table", key: tableItemKey(t), name: t.name }));
    case "dropRoutines":
      return op.routines.map((r) => ({
        type: "routine",
        key: routineItemKey(r),
        name: routineName(r),
      }));
    case "dropSequences":
      return op.sequences.map((s) => ({ type: "sequence", key: sequenceItemKey(s), name: s.name }));
  }
}

/**
 * How the sheet names `op`: "Delete users, posts", "Truncate tags", "Drop function
 * slugify(value text)", "Drop sequences a, b".
 */
export function describeOp(op: StagedObjectOp): { verb: string; what: string } {
  const names = opTargets(op)
    .map((t) => t.name)
    .join(", ");
  switch (op.op) {
    case "dropTables":
      return { verb: "Delete", what: names };
    case "truncateTables":
      return { verb: "Truncate", what: names };
    case "dropRoutines": {
      const kinds = new Set(
        op.routines.map((r) => (r.kind === "procedure" ? "procedure" : "function")),
      );
      const noun = kinds.size === 1 ? [...kinds][0]! : "routine";
      return { verb: "Drop", what: `${op.routines.length === 1 ? noun : `${noun}s`} ${names}` };
    }
    case "dropSequences":
      return { verb: "Drop", what: `sequence${op.sequences.length === 1 ? "" : "s"} ${names}` };
  }
}

/** The tables and views of `op` (none for a drop of routines or sequences). */
export function opTables(op: StagedObjectOp): TableRef[] {
  switch (op.op) {
    case "dropTables":
      return op.tables.map((t) => t.table);
    case "truncateTables":
      return op.tables;
    default:
      return [];
  }
}

export function pendingObjectOps(connectionId: string | null): readonly PendingObjectOp[] {
  if (connectionId === null) return EMPTY_OPS;
  return usePendingOpsStore.getState().byConnection[connectionId] ?? EMPTY_OPS;
}

/**
 * One change per table, routine or sequence, like the statements (one per object) TablePlus
 * generates.
 */
export function countOf(ops: readonly PendingObjectOp[]): number {
  return ops.reduce((n, o) => n + opTargets(o.op).length, 0);
}

export function pendingObjectCount(connectionId: string | null): number {
  return countOf(pendingObjectOps(connectionId));
}

function setOps(connectionId: string, update: (ops: PendingObjectOp[]) => PendingObjectOp[]) {
  usePendingOpsStore.setState((s) => {
    const current = s.byConnection[connectionId] ?? [];
    const next = update(current);
    if (next === current) return s;
    const byConnection = { ...s.byConnection };
    if (next.length === 0) delete byConnection[connectionId];
    else byConnection[connectionId] = next;
    return { byConnection };
  });
}

/** Adds an op to the connection's pending list (committed after the ones already there). */
export function stageObjectOp(
  connectionId: string,
  op: StagedObjectOp,
  statements: readonly string[],
  database: string | null,
): PendingObjectOp {
  const pending: PendingObjectOp = {
    id: crypto.randomUUID(),
    kind: kindOf(op),
    op,
    statements: [...statements],
    database,
    error: null,
  };
  setOps(connectionId, (ops) => [...ops, pending]);
  return pending;
}

/** Takes one op out of the pending list (the sheet's ×). */
export function unstageObjectOp(connectionId: string, id: string): void {
  setOps(connectionId, (ops) =>
    ops.some((o) => o.id === id) ? ops.filter((o) => o.id !== id) : ops,
  );
}

/** Drops every pending op of the connection; returns how many changes were dropped. */
export function discardObjectOps(connectionId: string): number {
  const n = pendingObjectCount(connectionId);
  if (n > 0) setOps(connectionId, () => []);
  return n;
}

/** `op` restricted to the targets whose key (`OpTarget.key`) `keep` accepts (same options). */
export function withTargets(op: StagedObjectOp, keep: (key: string) => boolean): StagedObjectOp {
  switch (op.op) {
    case "dropTables":
      return { ...op, tables: op.tables.filter((t) => keep(tableItemKey(t.table))) };
    case "truncateTables":
      return { ...op, tables: op.tables.filter((t) => keep(tableItemKey(t))) };
    case "dropRoutines":
      return { ...op, routines: op.routines.filter((r) => keep(routineItemKey(r))) };
    case "dropSequences":
      return { ...op, sequences: op.sequences.filter((s) => keep(sequenceItemKey(s))) };
  }
}

/** Moves the pending ops of a dead connection to the one a reconnect opened in its place. */
export function retargetObjectOps(oldId: string, newId: string): void {
  if (oldId === newId) return;
  usePendingOpsStore.setState((s) => {
    const ops = s.byConnection[oldId];
    if (!ops) return s;
    const byConnection = { ...s.byConnection };
    delete byConnection[oldId];
    byConnection[newId] = [...(byConnection[newId] ?? []), ...ops];
    return { byConnection };
  });
}

/** The pending ops of a connection (a stable reference until they change). */
export function usePendingObjectOps(connectionId: string | null): readonly PendingObjectOp[] {
  return usePendingOpsStore(
    (s) => (connectionId !== null ? s.byConnection[connectionId] : undefined) ?? EMPTY_OPS,
  );
}

/**
 * Sidebar item key (`tableItemKey`, `routineItemKey`, `sequenceItemKey`) → what is pending for
 * that object; a pending drop wins over a truncate.
 */
export function pendingMarks(ops: readonly PendingObjectOp[]): ReadonlyMap<string, PendingOpKind> {
  const marks = new Map<string, PendingOpKind>();
  for (const o of ops)
    for (const { key } of opTargets(o.op)) if (marks.get(key) !== "drop") marks.set(key, o.kind);
  return marks;
}

export function usePendingMarks(connectionId: string | null): ReadonlyMap<string, PendingOpKind> {
  const ops = usePendingObjectOps(connectionId);
  return useMemo(() => pendingMarks(ops), [ops]);
}

// --- settling a commit ----------------------------------------------------------------------------

function markFailed(connectionId: string, id: string, error: string): void {
  setOps(connectionId, (ops) => ops.map((o) => (o.id === id ? { ...o, error } : o)));
}

function replaceOp(connectionId: string, op: PendingObjectOp): void {
  setOps(connectionId, (ops) => ops.map((o) => (o.id === op.id ? op : o)));
}

function isStaged(connectionId: string, id: string): boolean {
  return pendingObjectOps(connectionId).some((o) => o.id === id);
}

/** Clears the error an op kept from an earlier commit that stopped at it. */
export function clearOpError(connectionId: string, id: string): void {
  setOps(connectionId, (ops) =>
    ops.some((o) => o.id === id && o.error !== null)
      ? ops.map((o) => (o.id === id ? { ...o, error: null } : o))
      : ops,
  );
}

/**
 * The parts of `op` whose statements each run on their own: one per table for a truncate, one per
 * kind of object for a drop of tables (`DROP VIEW a, b` / `DROP TABLE c, d`), one per routine or
 * sequence. Without the foreign-key wrapper, so their plans hold just those statements.
 */
function partsOf(op: StagedObjectOp): StagedObjectOp[] {
  switch (op.op) {
    case "truncateTables":
      return op.tables.map((t) => ({ ...op, tables: [t], ignoreForeignKeys: false }));
    case "dropTables": {
      const kinds = [...new Set(op.tables.map((t) => t.kind))];
      return kinds.map((k) => ({
        ...op,
        tables: op.tables.filter((t) => t.kind === k),
        ignoreForeignKeys: false,
      }));
    }
    case "dropRoutines":
      return op.routines.map((r) => ({ ...op, routines: [r] }));
    case "dropSequences":
      return op.sequences.map((s) => ({ ...op, sequences: [s] }));
  }
}

/**
 * The targets (`OpTarget.key`) an op that failed partway is done with: a plan that is not
 * transactional (MySQL DDL commits statement by statement) keeps the statements that ran before
 * the failing one. A part whose own statements all ran is done; anything unknown stays pending.
 */
async function doneTargets(
  connectionId: string,
  op: StagedObjectOp,
  executed: readonly string[],
): Promise<Set<string>> {
  const done = new Set<string>();
  if (executed.length === 0) return done;
  const ran = new Set(executed);
  for (const part of partsOf(op)) {
    try {
      const { statements } = await ipc.objects.preview(connectionId, part);
      if (statements.length > 0 && statements.every((s) => ran.has(s)))
        for (const { key } of opTargets(part)) done.add(key);
    } catch {
      // Not known to have run: stays pending.
    }
  }
  return done;
}

/** After the workspace commit applied `o`: it leaves the pending list. */
export function settleCommittedOp(connectionId: string, o: PendingObjectOp): void {
  unstageObjectOp(connectionId, o.id);
}

/**
 * After the workspace commit stopped at `o` with `error`: the objects a non-transactional plan
 * already dropped or truncated (`executed`) leave it and are returned as committed; the rest
 * stays pending, marked with the error.
 */
export async function settleFailedOp(
  connectionId: string,
  o: PendingObjectOp,
  executed: readonly string[],
  error: string,
): Promise<PendingObjectOp | null> {
  const done = await doneTargets(connectionId, o.op, executed);
  if (done.size === 0) {
    markFailed(connectionId, o.id, error);
    return null;
  }
  const committed: PendingObjectOp = {
    ...o,
    op: withTargets(o.op, (key) => done.has(key)),
    statements: [...executed],
  };
  const rest = withTargets(o.op, (key) => !done.has(key));
  if (opTargets(rest).length === 0) {
    unstageObjectOp(connectionId, o.id);
    return committed;
  }
  let statements = o.statements;
  try {
    statements = (await ipc.objects.preview(connectionId, rest)).statements;
  } catch {
    // Keeps the SQL it was staged with.
  }
  if (isStaged(connectionId, o.id)) replaceOp(connectionId, { ...o, op: rest, statements, error });
  return committed;
}

/** Test helper: forget every pending op. */
export function resetPendingObjectOps(): void {
  usePendingOpsStore.setState({ byConnection: {} });
}
