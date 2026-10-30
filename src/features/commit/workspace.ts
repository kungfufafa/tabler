import type { QueryClient } from "@tanstack/react-query";
import { useMemo, useSyncExternalStore } from "react";
import { toast } from "sonner";
import { useShallow } from "zustand/react/shallow";
import {
  applyToEditor,
  getEditorState,
  markSaved,
  onDocChanged,
} from "@/features/editor/editorStates";
import {
  countOf,
  discardObjectOps,
  clearOpError,
  opTables,
  opTargets,
  pendingObjectOps,
  settleCommittedOp,
  settleFailedOp,
  usePendingObjectOps,
  usePendingOpsStore,
  type PendingObjectOp,
} from "@/features/objects/pendingOps";
import {
  failResultCommit,
  finishResultCommit,
  planResultCommit,
  settlePartialResultCommit,
  type PlannedResult,
} from "@/features/results/commit";
import { discardResultEdits, pendingForTab, useResultsStore } from "@/features/results/store";
import { guardExecution } from "@/features/safety/guard";
import { withServerExprs } from "@/features/staging/expr";
import { discardAll, dropApplied, pendingCount, setRowError } from "@/features/staging/model";
import { toStagedChanges } from "@/features/staging/toPayload";
import {
  failStructureCommit,
  finishStructureCommit,
  reloadStructure,
} from "@/features/structure/commitFinish";
import { invalidateSchema } from "@/features/structure/invalidate";
import { sameRef } from "@/features/structure/model";
import { useStructureStore } from "@/features/structure/store";
import { appliesPartially, connectionCaps } from "@/lib/caps";
import {
  ipc,
  toIpcError,
  type DdlOp,
  type ErrorPayload,
  type GroupPlan,
  type GroupResult,
  type RoutineRef,
  type TableRef,
  type TxMode,
  type WorkspaceGroup,
  type WorkspacePlan,
  type WorkspaceResult,
} from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { getPrefs } from "@/stores/prefs";
import { useTableTabsStore } from "@/stores/tableTabs";
import { isTableTab, useTabsStore, type Tab } from "@/stores/tabs";
import {
  definitionLabel,
  definitionLoadedText,
  definitionOf,
  keepPreviousVersion,
  markDefinition,
  useDefinitionStore,
} from "./definitions";
import { refreshOpenTableTabs } from "./refreshTables";
import { rowContextOf } from "./rowContext";
import {
  bumpGeneration,
  clearFailure,
  closeWorkspaceSheet,
  isCommitting,
  openWorkspaceSheet,
  setCommitting,
  setFailure,
  useCommitStore,
} from "./store";

export { closeWorkspaceSheet, openWorkspaceSheet } from "./store";

/**
 * The workspace commit (TablePlus §9): every GUI change of a workspace (one connection) is
 * pending until ⌘S commits all of them together — the staged rows of each of its table tabs,
 * the staged edits of its query results, the staged structure of its tables, the unsaved text of
 * its view / function / routine definition tabs, its pending sidebar operations — in ONE
 * transaction where the engine allows it (`apply_workspace_changes`). ⇧⌘P previews them all in
 * one sheet, grouped by tab; ⇧⌘⌫ discards them all.
 */

interface SourceBase {
  /** Stable id of the part (`rows:<tab>`, `results:<tab>`, `ddl:<tab>`, `def:<tab>`, `object:<op>`). */
  key: string;
  /** How the Preview sheet heads it. */
  title: string;
  /** Pending changes it holds (the action control's count). */
  count: number;
  group: WorkspaceGroup;
}

export type WorkspaceSource =
  | (SourceBase & { kind: "rows"; tabId: string; table: TableRef; noKey: boolean })
  | (SourceBase & { kind: "results"; tabId: string; planned: PlannedResult })
  | (SourceBase & { kind: "ddl"; tabId: string; table: TableRef; ops: DdlOp[] })
  /** A view / function / procedure written (or a routine edited) in a SQL tab: `sql` is its text. */
  | (SourceBase & { kind: "definition"; tabId: string; drop: RoutineRef | null; sql: string })
  | (SourceBase & { kind: "object"; op: PendingObjectOp });

/** A part that cannot be committed as it is (nothing is sent while there is one). */
export interface WorkspaceProblem {
  title: string;
  message: string;
  detail?: string;
}

export interface CollectedWorkspace {
  sources: WorkspaceSource[];
  problems: WorkspaceProblem[];
}

/** TablePlus's order: data edits, structure, definitions, then the sidebar's operations. */
const RANK: Record<WorkspaceSource["kind"], number> = {
  rows: 0,
  results: 1,
  ddl: 2,
  definition: 3,
  object: 4,
};

/** Whether the tab is a definition editor with an edit to commit (see `pendingDefinitionText`). */
function hasPendingDefinition(tab: Tab): boolean {
  if (tab.kind !== "query" || !tab.dirty || !definitionOf(tab.id)) return false;
  const doc = getEditorState(tab.id)?.doc;
  if (!doc) return false;
  // Anything but whitespace; stops at the first chunk with text, so typing never copies the buffer.
  for (const chunk of doc.iter()) if (chunk.trim() !== "") return true;
  return false;
}

/**
 * The text of a pending definition tab, or null when the tab holds none: a query tab marked as a
 * definition editor (`markDefinition`) whose buffer changed since it was opened or last committed
 * (a routine just opened, or an untouched template, is nothing to commit) and is not empty.
 */
export function pendingDefinitionText(tab: Tab): string | null {
  return hasPendingDefinition(tab) ? (getEditorState(tab.id)?.doc.toString() ?? null) : null;
}

function definitionSource(tab: Tab): WorkspaceSource | null {
  const def = definitionOf(tab.id);
  const sql = pendingDefinitionText(tab);
  if (!def || sql === null) return null;
  return {
    kind: "definition",
    key: `def:${tab.id}`,
    title: tab.title,
    count: 1,
    tabId: tab.id,
    drop: def.drop,
    sql,
    group: { kind: "definition", drop: def.drop, sql },
  };
}

function tabsOf(connectionId: string): Tab[] {
  return useTabsStore.getState().tabs.filter((t) => t.connectionId === connectionId);
}

function rowsSource(tab: Tab, problems: WorkspaceProblem[]): WorkspaceSource | null {
  if (!isTableTab(tab)) return null;
  const slice = useTableTabsStore.getState().slices[tab.id];
  const count = slice ? pendingCount(slice.staging) : 0;
  if (!slice || count === 0) return null;
  const ctx = rowContextOf(tab.id);
  if (!ctx) {
    problems.push({
      title: tab.title,
      message: "Show the tab once more before committing its rows",
    });
    return null;
  }
  if (slice.stagedOn !== null && slice.stagedOn !== ctx.signature) {
    problems.push({
      title: tab.title,
      message: "The table's columns changed since these edits were made",
      detail: "Discard the pending changes, then edit the rows again.",
    });
    return null;
  }
  const changes = toStagedChanges(slice.staging, ctx.columns, (k) => ctx.keys.get(k) ?? null).map(
    withServerExprs,
  );
  if (changes.length === 0) return null;
  return {
    kind: "rows",
    key: `rows:${tab.id}`,
    title: tab.title,
    count,
    tabId: tab.id,
    table: tab.table,
    noKey: ctx.identity === "none",
    group: { kind: "rows", request: { table: tab.table, changes, returning: true } },
  };
}

/** Every pending change of the connection's workspace, as the parts of one commit. */
export function collectWorkspace(connectionId: string): CollectedWorkspace {
  const sources: WorkspaceSource[] = [];
  const problems: WorkspaceProblem[] = [];
  const tabs = tabsOf(connectionId);
  for (const tab of tabs) {
    const rows = rowsSource(tab, problems);
    if (rows) sources.push(rows);
  }
  for (const tab of tabs) {
    if (tab.kind !== "query") continue;
    const plan = planResultCommit(tab.id);
    if (!plan) continue;
    if (!plan.ok) {
      problems.push({ title: tab.title, message: plan.problem, detail: plan.detail });
      continue;
    }
    sources.push({
      kind: "results",
      key: `results:${tab.id}`,
      title: tab.title,
      count: pendingForTab(useResultsStore.getState(), tab.id),
      tabId: tab.id,
      planned: plan.planned,
      group: {
        kind: "rows",
        request: { ...plan.planned.req, changes: plan.planned.req.changes.map(withServerExprs) },
      },
    });
  }
  for (const tab of tabs) {
    if (!isTableTab(tab)) continue;
    const ops = useStructureStore.getState().byTab[tab.id]?.ops ?? [];
    if (ops.length === 0) continue;
    sources.push({
      kind: "ddl",
      key: `ddl:${tab.id}`,
      title: tab.title,
      count: ops.length,
      tabId: tab.id,
      table: tab.table,
      ops,
      group: { kind: "ddl", table: tab.table, ops },
    });
  }
  for (const tab of tabs) {
    const def = definitionSource(tab);
    if (def) sources.push(def);
  }
  for (const op of pendingObjectOps(connectionId)) {
    sources.push({
      kind: "object",
      key: `object:${op.id}`,
      title: "Item operations",
      count: countOf([op]),
      op,
      group: { kind: "object", op: op.op },
    });
  }
  sources.sort((a, b) => RANK[a.kind] - RANK[b.kind]);
  if (commitsWorkspace(connectionId)) return { sources, problems };
  // Without the workspace commit, definitions and item operations have no command to go through.
  const kept: WorkspaceSource[] = [];
  for (const s of sources) {
    if (s.kind === "definition" || s.kind === "object")
      problems.push({ title: s.title, message: "This connection cannot commit it" });
    else kept.push(s);
  }
  return { sources: kept, problems };
}

// --- sessions without a workspace commit -----------------------------------------------------------

/**
 * Whether the connection commits its whole workspace in one call (`apply_workspace_changes`).
 * Without `Feature::WorkspaceCommit` each tab's part goes through its own command
 * (`apply_changes`, `apply_ddl`), one after another, stopping at the first that fails.
 */
export function commitsWorkspace(connectionId: string): boolean {
  const caps = connectionCaps(connectionId);
  return caps === null || caps.features.includes("workspaceCommit");
}

/** How the connection's session applies staged rows (atomic while it is unknown). */
export function txModeOf(connectionId: string): TxMode {
  return connectionCaps(connectionId)?.data.transactions ?? "atomic";
}

/** Whether a commit of `sources` on its own commands runs as one transaction. */
function perTabAtomic(mode: TxMode, sources: readonly WorkspaceSource[]): boolean {
  return sources.length === 1 && sources[0]!.group.kind === "rows" && !appliesPartially(mode);
}

const commitError = (message: string): ErrorPayload => ({
  code: "UNSUPPORTED",
  message,
  detail: null,
  hint: null,
  sqlstate: null,
  vendorCode: null,
  position: null,
  statementIndex: null,
  data: null,
});

/**
 * What committing `sources` would run (the Preview sheet's statements): one
 * `preview_workspace_changes`, or each part's own preview on a session without the workspace
 * commit.
 */
export async function previewWorkspace(
  connectionId: string,
  sources: readonly WorkspaceSource[],
): Promise<WorkspacePlan> {
  if (commitsWorkspace(connectionId))
    return ipc.workspace.preview(
      connectionId,
      sources.map((s) => s.group),
    );
  const mode = txModeOf(connectionId);
  const groups = await Promise.all(
    sources.map(async ({ group }): Promise<GroupPlan> => {
      try {
        if (group.kind === "rows") {
          const statements = await ipc.mutate.preview(connectionId, group.request);
          return { statements, warnings: [], transactional: !appliesPartially(mode), error: null };
        }
        if (group.kind === "ddl")
          return { ...(await ipc.ddl.preview(connectionId, group.table, group.ops)), error: null };
        return {
          statements: [],
          warnings: [],
          transactional: false,
          error: commitError("This connection cannot commit it"),
        };
      } catch (e) {
        const error = commitError(toIpcError(e).message);
        return { statements: [], warnings: [], transactional: false, error };
      }
    }),
  );
  return { groups, atomic: perTabAtomic(mode, sources) };
}

const NOT_RUN: GroupResult = {
  status: "notRun",
  sql: [],
  executed: [],
  failedIndex: null,
  rows: [],
  routine: null,
  error: null,
};

/**
 * Commits each part on its own command, in order, stopping at the first that fails, and answers
 * as `apply_workspace_changes` would. A part of a session that applies changes one by one (or in
 * a batch that can stop partway) answers a result per change, applied or not.
 */
async function applyPerTab(
  connectionId: string,
  sources: readonly WorkspaceSource[],
): Promise<WorkspaceResult> {
  const groups: GroupResult[] = [];
  let failedGroup: number | null = null;
  let error: ErrorPayload | null = null;
  let durationMs = 0;
  for (const [i, { group }] of sources.entries()) {
    if (failedGroup !== null) {
      groups.push(NOT_RUN);
      continue;
    }
    let g: GroupResult;
    if (group.kind === "rows") {
      const r = await ipc.mutate.apply(connectionId, group.request);
      durationMs += r.durationMs;
      g = {
        ...NOT_RUN,
        status: r.success ? "applied" : "failed",
        sql: r.sql,
        executed: r.success ? r.sql : [],
        rows: r.results,
        error: r.error,
      };
    } else if (group.kind === "ddl") {
      const r = await ipc.ddl.apply(connectionId, group.table, group.ops);
      durationMs += r.durationMs;
      g = {
        ...NOT_RUN,
        status: r.error ? "failed" : "applied",
        sql: r.executed,
        executed: r.executed,
        failedIndex: r.failedIndex,
        error: r.error,
      };
    } else {
      g = { ...NOT_RUN, status: "failed", error: commitError("This connection cannot commit it") };
    }
    groups.push(g);
    if (g.status === "failed") {
      failedGroup = i;
      error = g.error;
    }
  }
  return {
    success: failedGroup === null,
    atomic: perTabAtomic(txModeOf(connectionId), sources),
    groups,
    failedGroup,
    error,
    durationMs,
  };
}

/**
 * The workspace ⌘S / ⇧⌘P / ⇧⌘⌫ and the action control act on: the connection of the tab in front,
 * else the active connection (a workspace with no tab open).
 */
export function useActiveWorkspaceId(): string | null {
  const active = useConnectionsStore((s) => s.activeId);
  const ofTab = useTabsStore((s) => s.tabs.find((t) => t.id === s.activeId)?.connectionId ?? null);
  return ofTab ?? active;
}

/** Pending changes of the connection's workspace (read outside React). */
export function workspacePendingCount(connectionId: string | null): number {
  if (connectionId === null) return 0;
  let n = countOf(pendingObjectOps(connectionId));
  const slices = useTableTabsStore.getState().slices;
  const structure = useStructureStore.getState().byTab;
  const results = useResultsStore.getState();
  for (const tab of tabsOf(connectionId)) {
    const slice = slices[tab.id];
    if (slice) n += pendingCount(slice.staging);
    n += structure[tab.id]?.ops.length ?? 0;
    if (tab.kind === "query") n += pendingForTab(results, tab.id);
    if (hasPendingDefinition(tab)) n += 1;
  }
  return n;
}

// --- the definition tabs' buffers -----------------------------------------------------------------
//
// Whether a definition tab has something to commit depends on its TEXT (an emptied buffer has
// nothing to create), and typing does not touch the tabs store once the tab is dirty (its flag
// does not flip again), so the hooks below listen to the editors' document changes too.

const definitionDocListeners = new Set<() => void>();
let definitionDocVersion = 0;
let stopWatchingDocs: (() => void) | null = null;

function subscribeDefinitionDocs(notify: () => void): () => void {
  definitionDocListeners.add(notify);
  stopWatchingDocs ??= onDocChanged((tabId) => {
    if (!definitionOf(tabId)) return;
    definitionDocVersion += 1;
    for (const fn of [...definitionDocListeners]) fn();
  });
  return () => {
    definitionDocListeners.delete(notify);
    if (definitionDocListeners.size === 0) {
      stopWatchingDocs?.();
      stopWatchingDocs = null;
    }
  };
}

/** Changes whenever the document of a definition tab does. */
function useDefinitionDocVersion(): number {
  return useSyncExternalStore(subscribeDefinitionDocs, () => definitionDocVersion);
}

/** How many of the connection's definition tabs hold an edit; follows the typing in them. */
function useDefinitionCount(connectionId: string | null): number {
  const subscribe = useMemo(
    () => (notify: () => void) => {
      const stopDocs = subscribeDefinitionDocs(notify);
      const stopTabs = useTabsStore.subscribe(notify);
      const stopMarks = useDefinitionStore.subscribe(notify);
      return () => {
        stopDocs();
        stopTabs();
        stopMarks();
      };
    },
    [],
  );
  return useSyncExternalStore(subscribe, () =>
    connectionId === null ? 0 : tabsOf(connectionId).filter(hasPendingDefinition).length,
  );
}

/** `workspacePendingCount` as a hook. */
export function useWorkspacePendingCount(connectionId: string | null): number {
  const tabIds = useTabsStore(
    useShallow((s) =>
      connectionId === null
        ? []
        : s.tabs.filter((t) => t.connectionId === connectionId).map((t) => t.id),
    ),
  );
  const rows = useTableTabsStore((s) =>
    tabIds.reduce((n, id) => n + (s.slices[id] ? pendingCount(s.slices[id].staging) : 0), 0),
  );
  const ddl = useStructureStore((s) =>
    tabIds.reduce((n, id) => n + (s.byTab[id]?.ops.length ?? 0), 0),
  );
  const results = useResultsStore((s) => tabIds.reduce((n, id) => n + pendingForTab(s, id), 0));
  const ops = countOf(usePendingObjectOps(connectionId));
  // Definition tabs: marked, edited since they were opened or last committed, not empty.
  const definitions = useDefinitionCount(connectionId);
  return rows + ddl + results + ops + definitions;
}

/**
 * The connection's parts, recollected whenever something they come from changes. The tab slices
 * and structure entries are watched through the references the parts are built from (a page load
 * updating a slice leaves them alone), so the Preview sheet does not reload its statements for it.
 */
export function useWorkspaceSources(connectionId: string | null): CollectedWorkspace {
  const tabs = useTabsStore((s) => s.tabs);
  const staged = useTableTabsStore(
    useShallow((s) =>
      Object.entries(s.slices).flatMap(([id, slice]) => [id, slice.staging, slice.stagedOn]),
    ),
  );
  const structure = useStructureStore(
    useShallow((s) => Object.entries(s.byTab).flatMap(([id, slice]) => [id, slice.ops])),
  );
  const edits = useResultsStore((s) => s.edits);
  const editability = useResultsStore((s) => s.editability);
  const ops = usePendingOpsStore((s) => s.byConnection);
  const definitions = useDefinitionStore((s) => s.byTab);
  const definitionDocs = useDefinitionDocVersion();
  return useMemo(
    () => (connectionId === null ? { sources: [], problems: [] } : collectWorkspace(connectionId)),
    // The stores are read through `collectWorkspace`; these say when to read them again.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [connectionId, tabs, staged, structure, edits, editability, ops, definitions, definitionDocs],
  );
}

// --- the query client the commit refreshes ---------------------------------------------------------

let queryClient: QueryClient | null = null;

/** Set once by the host (`usePendingOpActions`). */
export function setCommitQueryClient(qc: QueryClient): void {
  queryClient = qc;
}

export function getCommitQueryClient(): QueryClient | null {
  return queryClient;
}

/** Closes the tabs of dropped tables and forgets their cached metadata. */
export function forgetTables(
  qc: QueryClient,
  connectionId: string,
  tables: readonly TableRef[],
): void {
  const ids = useTabsStore
    .getState()
    .tabs.filter(
      (t) =>
        isTableTab(t) && t.connectionId === connectionId && tables.some((x) => sameRef(t.table, x)),
    )
    .map((t) => t.id);
  if (ids.length > 0) {
    useStructureStore.getState().remove(ids);
    useTabsStore.getState().forceClose(ids);
  }
  for (const t of tables)
    qc.removeQueries({ queryKey: ["table", connectionId, t.schema ?? "", t.name] });
}

// --- commit --------------------------------------------------------------------------------------

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

function reportProblems(problems: readonly WorkspaceProblem[]): void {
  const first = problems[0];
  if (!first) return;
  toast.error(`${first.title}: ${first.message}`, {
    id: "workspace-commit-problem",
    description: first.detail,
  });
}

/** "Deleted users", "Deleted 2 routines, truncated tags": what committed sidebar ops did. */
function opsSummary(ops: readonly PendingObjectOp[]): string | undefined {
  const of = (kind: PendingObjectOp["kind"]) =>
    ops.filter((o) => o.kind === kind).flatMap((o) => opTargets(o.op));
  const names = (list: ReturnType<typeof opTargets>) => {
    if (list.length === 1) return list[0]!.name;
    const types = new Set(list.map((t) => t.type));
    return plural(list.length, types.size === 1 ? [...types][0]! : "item");
  };
  const parts: string[] = [];
  const dropped = of("drop");
  const truncated = of("truncate");
  if (dropped.length > 0) parts.push(`deleted ${names(dropped)}`);
  if (truncated.length > 0) parts.push(`truncated ${names(truncated)}`);
  if (parts.length === 0) return undefined;
  const text = parts.join(", ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** After `o` (or the part of it that ran) was committed: what it dropped / truncated reloads. */
function afterObjectOp(qc: QueryClient | null, connectionId: string, o: PendingObjectOp): void {
  if (!qc) return;
  if (o.op.op === "dropTables") forgetTables(qc, connectionId, opTables(o.op));
  else if (o.op.op === "truncateTables")
    for (const t of o.op.tables) invalidateSchema(qc, connectionId, t);
}

/** A table whose rows were committed from the tab `tabId` (it reloaded itself). */
interface Written {
  table: TableRef;
  tabId: string;
}

async function finishSource(
  connectionId: string,
  s: WorkspaceSource,
  g: GroupResult,
  committedOps: PendingObjectOp[],
  savedDefinitions: string[],
  writtenTables: Written[],
): Promise<void> {
  const qc = queryClient;
  switch (s.kind) {
    case "rows":
      useTableTabsStore.getState().update(s.tabId, (slice) => ({
        staging: discardAll(),
        refreshToken: slice.refreshToken + 1,
      }));
      bumpGeneration(s.tabId);
      writtenTables.push({ table: s.table, tabId: s.tabId });
      return;
    case "results":
      finishResultCommit(s.planned, g.rows);
      return;
    case "definition": {
      // The tab goes on saving the routine its text now names, and is clean until edited again.
      const routine = g.routine ?? s.drop;
      markDefinition(s.tabId, { drop: g.routine ?? s.drop }, s.sql);
      markSaved(s.tabId, s.sql);
      savedDefinitions.push(definitionLabel(s.sql, routine));
      return;
    }
    case "ddl":
      if (qc) await finishStructureCommit(qc, connectionId, s.tabId, s.table, s.ops);
      else useStructureStore.getState().discard(s.tabId);
      bumpGeneration(s.tabId);
      return;
    case "object":
      settleCommittedOp(connectionId, s.op);
      committedOps.push(s.op);
      afterObjectOp(qc, connectionId, s.op);
      return;
  }
}

/**
 * Settles a part that failed; answers how many of its changes the server applied all the same
 * (`partial`: a session whose commit can stop partway, critique D2), which leave the staging.
 */
async function failSource(
  connectionId: string,
  s: WorkspaceSource,
  g: GroupResult,
  committedOps: PendingObjectOp[],
  partial: boolean,
  writtenTables: Written[],
): Promise<number> {
  const message = g.error?.message ?? "Failed";
  // A change the server answered without an error was applied; one it never reached has no answer.
  const applied = partial
    ? new Set(g.rows.filter((r) => r.error === null).map((r) => r.rowId))
    : new Set<string>();
  switch (s.kind) {
    case "rows":
      if (applied.size > 0) {
        const errors = new Map(
          g.rows.flatMap((r) => (r.error ? [[r.rowId, r.error.message]] : [])),
        );
        useTableTabsStore.getState().update(s.tabId, (slice) => ({
          staging: dropApplied(slice.staging, applied, errors),
          // The applied rows read back as the server holds them now.
          refreshToken: slice.refreshToken + 1,
        }));
        bumpGeneration(s.tabId);
        writtenTables.push({ table: s.table, tabId: s.tabId });
        return applied.size;
      }
      useTableTabsStore.getState().update(s.tabId, (slice) => ({
        staging: g.rows.reduce(
          (acc, r) => (r.error ? setRowError(acc, r.rowId, r.error.message) : acc),
          slice.staging,
        ),
      }));
      return 0;
    case "results":
      if (applied.size > 0) {
        settlePartialResultCommit(s.planned, g.rows, applied);
        return applied.size;
      }
      failResultCommit(s.planned, g.rows);
      return 0;
    case "ddl":
      failStructureCommit(s.tabId, g.executed);
      return 0;
    case "definition": {
      // A routine MySQL dropped and could not create again lives on only in its text: keep it
      // in a tab.
      const restore = g.restore ?? null;
      if (restore && !restore.restored) keepPreviousVersion(connectionId, s.drop, restore.previous);
      return 0;
    }
    case "object": {
      const done = await settleFailedOp(connectionId, s.op, g.executed, message);
      if (done) {
        committedOps.push(done);
        afterObjectOp(queryClient, connectionId, done);
      }
      return 0;
    }
  }
}

/**
 * Settles every part after `apply_workspace_changes` (or the parts' own commands); true when it
 * all went through. `partial`: a failed part may have applied some of its changes.
 */
async function settle(
  connectionId: string,
  sources: readonly WorkspaceSource[],
  result: WorkspaceResult,
  partial = false,
): Promise<boolean> {
  const committedOps: PendingObjectOp[] = [];
  const savedDefinitions: string[] = [];
  const writtenTables: Written[] = [];
  let changed = 0;
  // Rows first: a structure reload then finds no held edits on its table.
  for (const [i, s] of sources.entries()) {
    const g = result.groups[i];
    if (!g) continue;
    if (g.status === "applied") {
      changed += s.count;
      await finishSource(connectionId, s, g, committedOps, savedDefinitions, writtenTables);
    } else if (g.status === "failed") {
      changed += await failSource(connectionId, s, g, committedOps, partial, writtenTables);
    }
  }
  const qc = queryClient;
  // Rows were written: every other open tab of those tables reloads as well (a split view, a
  // second tab), once all the tabs of this commit have settled (they reloaded themselves).
  const committedTabs = writtenTables.map((w) => w.tabId);
  const distinct = new Map(
    writtenTables.map((w) => [`${w.table.schema ?? ""}.${w.table.name}`, w.table]),
  );
  for (const table of distinct.values()) refreshOpenTableTabs(connectionId, table, committedTabs);
  // The sidebar's lists follow what the server holds now. A definition that failed without
  // touching anything (a syntax error before any DROP ran) leaves them as they are.
  const touched = (s: WorkspaceSource, g: GroupResult | undefined) =>
    s.kind === "object" ||
    s.kind === "ddl" ||
    (s.kind === "definition" &&
      g !== undefined &&
      (g.status === "applied" || g.executed.length > 0 || g.restore != null));
  if (qc && sources.some((s, i) => touched(s, result.groups[i])))
    invalidateSchema(qc, connectionId);
  if (result.success) {
    clearFailure(connectionId);
    const saved = savedDefinitions.map((label) => `Saved ${label}`);
    toast.success(`Committed ${plural(changed, "change")} · ${result.durationMs} ms`, {
      description: [opsSummary(committedOps), ...saved].filter(Boolean).join(" · ") || undefined,
    });
    const sheet = useCommitStore.getState().sheet;
    if (sheet?.connectionId === connectionId) {
      closeWorkspaceSheet();
      sheet.onCommitted?.();
    }
    return true;
  }
  const index = result.failedGroup;
  const failed = index !== null ? sources[index] : undefined;
  const g = index !== null ? result.groups[index] : undefined;
  const error = g?.error ?? result.error;
  const message = error?.message ?? "The commit failed";
  setFailure({
    connectionId,
    key: failed?.key ?? null,
    message,
    detail: [error?.detail, error?.hint].filter(Boolean).join("\n") || null,
    statementIndex: g?.failedIndex ?? null,
    sql: g?.sql ?? [],
    atomic: result.atomic,
    restore: g?.restore ?? null,
  });
  const where = failed ? `${failed.title}: ` : "";
  if (result.atomic) {
    toast.error("All changes were reverted", { description: `${where}${message}` });
  } else {
    const kept = changed + committedOps.length;
    toast.error(kept > 0 ? "The commit stopped partway" : "Could not commit the changes", {
      description: `${where}${message}${
        kept > 0 ? ` — ${plural(changed, "change")} before it stay committed.` : ""
      }`,
    });
  }
  // The sheet shows where it stopped (without the follow-up of a guarded page change).
  openWorkspaceSheet(connectionId);
  return false;
}

/**
 * Commits every pending change of the connection's workspace: plans them again, asks the
 * connection's safe mode once about all the statements, then runs them (`apply_workspace_changes`)
 * and settles each part. True when everything went through.
 */
export async function applyWorkspace(connectionId: string): Promise<boolean> {
  if (isCommitting(connectionId)) return false;
  const { sources, problems } = collectWorkspace(connectionId);
  if (problems.length > 0) {
    reportProblems(problems);
    return false;
  }
  if (sources.length === 0) {
    if (useCommitStore.getState().sheet?.connectionId === connectionId) closeWorkspaceSheet();
    return false;
  }
  setCommitting(connectionId, true);
  try {
    for (const s of sources) if (s.kind === "object") clearOpError(connectionId, s.op.id);
    const groups = sources.map((s) => s.group);
    const perTab = !commitsWorkspace(connectionId);
    const plan = await previewWorkspace(connectionId, sources);
    const bad = plan.groups.findIndex((g) => g.error);
    if (bad >= 0) {
      const err = plan.groups[bad]!.error!;
      const s = sources[bad]!;
      setFailure({
        connectionId,
        key: s.key,
        message: err.message,
        detail: [err.detail, err.hint].filter(Boolean).join("\n") || null,
        statementIndex: null,
        sql: [],
        atomic: true,
        restore: null,
      });
      toast.error(`Could not commit ${s.title}`, { description: err.message });
      openWorkspaceSheet(connectionId);
      return false;
    }
    const statements = plan.groups.flatMap((g) => g.statements);
    if (!(await guardExecution(connectionId, { kind: "commit", statements }))) return false;
    const result = perTab
      ? await applyPerTab(connectionId, sources)
      : await ipc.workspace.apply(connectionId, groups);
    return await settle(
      connectionId,
      sources,
      result,
      perTab && appliesPartially(txModeOf(connectionId)),
    );
  } catch (e) {
    toast.error("Could not commit the changes", { description: toIpcError(e).message });
    return false;
  } finally {
    setCommitting(connectionId, false);
  }
}

/**
 * ⌘S (and the action control's Commit): commits the workspace right away — staged structure
 * included, ⇧⌘P is the way to see the SQL first — or opens its Preview sheet first with "Confirm
 * before commit", for a table without a key (its UPDATE / DELETE match every original value), for
 * a view / function / routine definition (it drops and creates the object, and the sheet is where
 * the outcome of a failed replacement is told), or when `onCommitted` must follow (a page change
 * the unsaved-changes prompt interrupted). True when it committed.
 */
export async function commitWorkspace(
  connectionId: string,
  opts: { onCommitted?: () => void } = {},
): Promise<boolean> {
  if (isCommitting(connectionId)) return false;
  const { sources, problems } = collectWorkspace(connectionId);
  if (problems.length > 0) {
    reportProblems(problems);
    return false;
  }
  if (sources.length === 0) return false;
  // Staged structure follows the same rule as rows (⌘S commits, "Confirm before commit" asks).
  // A table without a key always previews first (its UPDATE / DELETE match every original value),
  // so does a definition (it drops and recreates an object).
  const review =
    getPrefs().confirmCommit ||
    opts.onCommitted !== undefined ||
    sources.some((s) => s.kind === "definition" || (s.kind === "rows" && s.noKey));
  if (review) {
    openWorkspaceSheet(connectionId, opts.onCommitted ?? null);
    return false;
  }
  return applyWorkspace(connectionId);
}

/**
 * ⇧⌘⌫ (TablePlus Discard): drops every pending change of the connection's workspace — staged
 * rows, result edits and structure of all its tabs, the edited text of its definition tabs (put
 * back to what they were opened with), its pending sidebar operations. Returns how many changes
 * were dropped.
 */
export function discardWorkspace(connectionId: string): number {
  if (isCommitting(connectionId)) {
    toast.message("The changes are being committed", {
      description: "Wait for the commit to finish.",
    });
    return 0;
  }
  let n = 0;
  const structure = useStructureStore.getState();
  for (const tab of tabsOf(connectionId)) {
    const slice = useTableTabsStore.getState().slices[tab.id];
    const rows = slice ? pendingCount(slice.staging) : 0;
    if (rows > 0) {
      n += rows;
      useTableTabsStore.getState().update(tab.id, { staging: discardAll() });
    }
    const staged = structure.byTab[tab.id];
    if (isTableTab(tab) && staged && staged.ops.length > 0) {
      n += staged.ops.length;
      // After a partial apply the metadata is stale too: it reloads along with the discard.
      if ((staged.partial ?? 0) > 0 && queryClient)
        void reloadStructure(queryClient, connectionId, tab.id, tab.table, true);
      else structure.discard(tab.id);
      bumpGeneration(tab.id);
    }
    if (tab.kind === "query") {
      const st = useResultsStore.getState();
      n += pendingForTab(st, tab.id);
      for (const [id, e] of Object.entries(st.edits))
        if (e.tabId === tab.id && pendingCount(e.staging) > 0) discardResultEdits(id);
      // A definition tab goes back to the text it was opened with (an edit: ⌘Z brings it back).
      if (pendingDefinitionText(tab) !== null) {
        const loaded = definitionLoadedText(tab.id);
        applyToEditor(tab.id, (state) => ({
          changes: { from: 0, to: state.doc.length, insert: loaded },
          selection: { anchor: 0 },
        }));
        n += 1;
      }
    }
  }
  n += discardObjectOps(connectionId);
  clearFailure(connectionId);
  if (useCommitStore.getState().sheet?.connectionId === connectionId) closeWorkspaceSheet();
  if (n > 0) toast.message(`Discarded ${plural(n, "change")}`);
  return n;
}
