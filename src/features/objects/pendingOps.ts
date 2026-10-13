import { useMemo } from "react";
import { create } from "zustand";
import { guardExecution } from "@/features/safety/guard";
import { ipc, tableKey, toIpcError, type ObjectOp, type TableRef } from "@/lib/ipc";
import type { Tab } from "@/stores/tabs";

/**
 * Sidebar Truncate… / Delete… are staged like every other GUI change (TablePlus, spec §2.4 / §9):
 * they wait here, per connection, until ⌘S commits them (after the active tab's own changes),
 * ⇧⌘⌫ discards them, the tab that carries them in the unsaved-changes guard is closed with
 * Discard, or the connection goes away.
 */
export type PendingOpKind = "drop" | "truncate";

export type StagedObjectOp = Extract<ObjectOp, { op: "dropTables" } | { op: "truncateTables" }>;

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
  /** Connections whose ops are being committed right now. */
  committing: Record<string, true>;
}

export const EMPTY_OPS: readonly PendingObjectOp[] = Object.freeze([]);

export const usePendingOpsStore = create<PendingOpsState>()(() => ({
  byConnection: {},
  committing: {},
}));

export function kindOf(op: StagedObjectOp): PendingOpKind {
  return op.op === "dropTables" ? "drop" : "truncate";
}

export function opTables(op: StagedObjectOp): TableRef[] {
  return op.op === "dropTables" ? op.tables.map((t) => t.table) : op.tables;
}

export function pendingObjectOps(connectionId: string | null): readonly PendingObjectOp[] {
  if (connectionId === null) return EMPTY_OPS;
  return usePendingOpsStore.getState().byConnection[connectionId] ?? EMPTY_OPS;
}

/** One change per table, like the statements (one per table) TablePlus generates. */
export function countOf(ops: readonly PendingObjectOp[]): number {
  return ops.reduce((n, o) => n + opTables(o.op).length, 0);
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

function setCommitting(connectionId: string, on: boolean): void {
  usePendingOpsStore.setState((s) => {
    if ((s.committing[connectionId] === true) === on) return s;
    const committing = { ...s.committing };
    if (on) committing[connectionId] = true;
    else delete committing[connectionId];
    return { committing };
  });
}

/** Whether the connection's pending ops are being committed (× and discard wait meanwhile). */
export function isCommittingObjectOps(connectionId: string | null): boolean {
  return connectionId !== null && usePendingOpsStore.getState().committing[connectionId] === true;
}

export function useObjectOpsCommitting(connectionId: string | null): boolean {
  return usePendingOpsStore((s) => connectionId !== null && s.committing[connectionId] === true);
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

/** `op` restricted to the tables `keep` accepts (same options). */
export function withTables(op: StagedObjectOp, keep: (t: TableRef) => boolean): StagedObjectOp {
  return op.op === "dropTables"
    ? { ...op, tables: op.tables.filter((t) => keep(t.table)) }
    : { ...op, tables: op.tables.filter(keep) };
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

/** `tableKey` → what is pending for it; a pending drop wins over a truncate. */
export function pendingMarks(ops: readonly PendingObjectOp[]): ReadonlyMap<string, PendingOpKind> {
  const marks = new Map<string, PendingOpKind>();
  for (const o of ops)
    for (const t of opTables(o.op))
      if (marks.get(tableKey(t)) !== "drop") marks.set(tableKey(t), o.kind);
  return marks;
}

export function usePendingMarks(connectionId: string | null): ReadonlyMap<string, PendingOpKind> {
  const ops = usePendingObjectOps(connectionId);
  return useMemo(() => pendingMarks(ops), [ops]);
}

// --- commit --------------------------------------------------------------------------------------

export type CommitOutcome =
  | { status: "empty" }
  | { status: "busy" }
  | { status: "cancelled" }
  | { status: "done"; committed: PendingObjectOp[] }
  | { status: "failed"; committed: PendingObjectOp[]; failed: PendingObjectOp; error: string };

function markFailed(connectionId: string, id: string, error: string): void {
  setOps(connectionId, (ops) => ops.map((o) => (o.id === id ? { ...o, error } : o)));
}

function replaceOp(connectionId: string, op: PendingObjectOp): void {
  setOps(connectionId, (ops) => ops.map((o) => (o.id === op.id ? op : o)));
}

function isStaged(connectionId: string, id: string): boolean {
  return pendingObjectOps(connectionId).some((o) => o.id === id);
}

/**
 * The parts of `op` whose statements each run on their own: one per table for a truncate, one per
 * kind of object for a drop (`DROP VIEW a, b` / `DROP TABLE c, d`). Without the foreign-key
 * wrapper, so their plans hold just those statements.
 */
function partsOf(op: StagedObjectOp): StagedObjectOp[] {
  if (op.op === "truncateTables")
    return op.tables.map((t) => ({ ...op, tables: [t], ignoreForeignKeys: false }));
  const kinds = [...new Set(op.tables.map((t) => t.kind))];
  return kinds.map((k) => ({
    ...op,
    tables: op.tables.filter((t) => t.kind === k),
    ignoreForeignKeys: false,
  }));
}

/**
 * The tables an op that failed partway is done with: a plan that is not transactional (MySQL DDL
 * commits statement by statement) keeps the statements that ran before the failing one. A part
 * whose own statements all ran is done; anything unknown stays pending.
 */
async function doneTables(
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
        for (const t of opTables(part)) done.add(tableKey(t));
    } catch {
      // Not known to have run: stays pending.
    }
  }
  return done;
}

/**
 * Commits the connection's pending ops in order: plans them again (the SQL the safe mode is
 * shown), asks the safe mode once for all of them (`guardExecution` kind "commit"), then runs one
 * backend call per op that is still staged when its turn comes (one removed or discarded meanwhile
 * is skipped). The first failure stops the run: that op (marked with the error, minus the tables
 * a non-transactional plan already dropped or truncated) and the ones after it stay pending; what
 * ran leaves the list.
 */
export async function commitObjectOps(connectionId: string): Promise<CommitOutcome> {
  const ops = pendingObjectOps(connectionId);
  if (ops.length === 0) return { status: "empty" };
  if (isCommittingObjectOps(connectionId)) return { status: "busy" };
  setCommitting(connectionId, true);
  const committed: PendingObjectOp[] = [];
  try {
    const plans: string[][] = [];
    for (const o of ops) {
      try {
        plans.push((await ipc.objects.preview(connectionId, o.op)).statements);
      } catch (e) {
        const error = toIpcError(e).message;
        markFailed(connectionId, o.id, error);
        return { status: "failed", committed, failed: o, error };
      }
    }
    if (!(await guardExecution(connectionId, { kind: "commit", statements: plans.flat() })))
      return { status: "cancelled" };
    for (const o of ops) {
      if (!isStaged(connectionId, o.id)) continue;
      let error: string | null;
      let executed: string[] = [];
      try {
        const r = await ipc.objects.apply(connectionId, o.op);
        error = r.error ? r.error.message : null;
        executed = r.executed;
      } catch (e) {
        error = toIpcError(e).message;
      }
      if (error === null) {
        committed.push(o);
        unstageObjectOp(connectionId, o.id);
        continue;
      }
      const done = await doneTables(connectionId, o.op, executed);
      if (done.size === 0) {
        markFailed(connectionId, o.id, error);
        return { status: "failed", committed, failed: { ...o, error }, error };
      }
      // Partly applied: what ran is committed; the rest stays pending with the error.
      committed.push({
        ...o,
        op: withTables(o.op, (t) => done.has(tableKey(t))),
        statements: executed,
      });
      const rest = withTables(o.op, (t) => !done.has(tableKey(t)));
      if (opTables(rest).length === 0) {
        unstageObjectOp(connectionId, o.id);
        continue;
      }
      let statements = o.statements;
      try {
        statements = (await ipc.objects.preview(connectionId, rest)).statements;
      } catch {
        // Keeps the SQL it was staged with.
      }
      const failed: PendingObjectOp = { ...o, op: rest, statements, error };
      if (isStaged(connectionId, o.id)) replaceOp(connectionId, failed);
      return { status: "failed", committed, failed, error };
    }
    return { status: "done", committed };
  } finally {
    setCommitting(connectionId, false);
  }
}

// --- the unsaved-changes guard ---------------------------------------------------------------------

/**
 * The tab of `connectionId` that carries its pending ops in `unsavedCount` (so disconnect and quit
 * ask first): a regular table tab, else a preview tab, else a query tab (an auto-saved query tab
 * is skipped by the quit guard); the active one among equals, else the first. A preview tab that
 * held the count would no longer be replaced by the next single click, hence the order.
 */
export function hostTabId(
  tabs: readonly Tab[],
  activeId: string | null,
  connectionId: string,
): string | null {
  let best: Tab | null = null;
  let bestRank = Infinity;
  for (const t of tabs) {
    if (t.connectionId !== connectionId) continue;
    const rank = (t.kind === "query" ? 2 : t.preview ? 1 : 0) * 2 + (t.id === activeId ? 0 : 1);
    if (rank < bestRank) {
      best = t;
      bestRank = rank;
    }
  }
  return best?.id ?? null;
}

/** Test helper: forget every pending op. */
export function resetPendingObjectOps(): void {
  usePendingOpsStore.setState({ byConnection: {}, committing: {} });
}
