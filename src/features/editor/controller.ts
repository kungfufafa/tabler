import { setDiagnostics } from "@codemirror/lint";
import { EditorSelection, type EditorState } from "@codemirror/state";
import { toast } from "sonner";
import {
  ipc,
  isMock,
  toIpcError,
  type Driver,
  type ExplainMode,
  type QueryEvent,
  type StatementResult,
} from "@/lib/ipc";
import { openExport, type ExportSourceSpec } from "@/features/export";
import { resultTable } from "@/features/export/resultTable";
import { useConnectionsStore } from "@/stores/connections";
import { getPrefs } from "@/stores/prefs";
import {
  getQueryTab,
  isBusy,
  updateQueryTab,
  useQueryTabsStore,
  type QueryTabState,
} from "@/stores/queryTabs";
import { dropResults, flushResultVersions, resultData } from "@/stores/resultData";
import { useTabsStore } from "@/stores/tabs";
import { anchorSubmission, toCurrentDoc } from "./docAnchors";
import { applyToEditor, focusEditor, getEditorState, markSaved, seedEditor } from "./editorStates";
import { clampRange, diagnostic, errorRange, runDiagnostics, type Submission } from "./errorPos";
import { baseName, pickOpenPath, pickSavePath } from "./files";
import {
  applyEvent,
  failRun,
  finalizeRun,
  firstError,
  newRun,
  normalizeError,
  type RunState,
} from "./runModel";
import { splitStatements, statementAt, type SqlDialect } from "./scanner";
import { changesSchema } from "./schemaChange";

/** Row cap per statement (matches the backend default). */
export const MAX_ROWS = 10_000;

export interface ControllerHooks {
  /** A run / explain finished (the backend wrote history rows). */
  onHistoryChanged?: () => void;
  /**
   * A run included a statement that can change the schema (CREATE, ALTER, DROP…): cached table
   * lists, metadata, rows and completion are stale.
   */
  onSchemaChanged?: () => void;
}

export function dialectOf(driver: Driver | undefined): SqlDialect {
  return driver ?? "postgres";
}

function queryTab(tabId: string) {
  const tab = useTabsStore.getState().tabs.find((t) => t.id === tabId);
  if (!tab || tab.kind !== "query") return null;
  const connection = useConnectionsStore.getState().byId[tab.connectionId];
  return { tab, connectionId: tab.connectionId, dialect: dialectOf(connection?.info.driver) };
}

export type RunTarget = "auto" | "current" | Submission;

/**
 * What to submit: the selection when there is one; otherwise the whole document ("auto") or the
 * statement under the cursor ("current").
 */
export function resolveTarget(
  state: EditorState,
  target: RunTarget,
  dialect: SqlDialect,
): Submission | null {
  if (typeof target === "object") return target;
  const sel = state.selection.main;
  if (!sel.empty) return { sql: state.sliceDoc(sel.from, sel.to), baseOffset: sel.from };
  const text = state.doc.toString();
  if (target === "auto") return { sql: text, baseOffset: 0 };
  const st = statementAt(splitStatements(text, dialect), text, sel.head, true);
  return st ? { sql: text.slice(st.start, st.end), baseOffset: st.start } : null;
}

function clearDiagnostics(tabId: string) {
  applyToEditor(tabId, (s) => setDiagnostics(s, []));
}

function busyNotice(): void {
  toast.message("A query is already running", { description: "Stop it first with ⌘." });
}

const tick = (ms: number) => new Promise((r) => setTimeout(r, ms));

const nextFrame: (fn: () => void) => void =
  typeof requestAnimationFrame === "function"
    ? (fn) => void requestAnimationFrame(() => fn())
    : (fn) => void setTimeout(fn, 16);

/**
 * A run in flight. Streamed events fold into `state` here and reach the store at most once per
 * animation frame: a script of thousands of statements sends two events per statement, and
 * committing each one would re-render the results pane every time.
 */
interface LiveRun {
  tabId: string;
  state: RunState;
  scheduled: boolean;
}

const liveRuns = new Map<string, LiveRun>();

function commitLive(live: LiveRun): void {
  live.scheduled = false;
  const stored = getQueryTab(live.tabId).run;
  // The tab closed, or another run replaced this one.
  if (stored?.id !== live.state.id || stored === live.state) return;
  const patch: Partial<QueryTabState> = { run: live.state };
  // Show the first result set as soon as it exists.
  if (stored.results.length === 0 && live.state.results.length > 0)
    patch.pane = live.state.results[0]!.id;
  updateQueryTab(live.tabId, patch);
}

function updateLive(live: LiveRun, next: RunState, immediately = false): void {
  if (next !== live.state) live.state = next;
  if (immediately) return commitLive(live);
  if (live.scheduled || getQueryTab(live.tabId).run === live.state) return;
  live.scheduled = true;
  nextFrame(() => {
    if (live.scheduled) commitLive(live);
  });
}

/** Runs SQL in the tab: selection / whole document ("auto"), current statement, or given text. */
export async function runQuery(
  tabId: string,
  target: RunTarget = "auto",
  hooks: ControllerHooks = {},
): Promise<void> {
  const info = queryTab(tabId);
  const state = getEditorState(tabId);
  if (!info || !state) return;
  const before = getQueryTab(tabId);
  if (isBusy(before)) return busyNotice();
  const sub = resolveTarget(state, target, info.dialect);
  if (!sub || splitStatements(sub.sql, info.dialect).length === 0) {
    toast.message(target === "current" ? "No statement under the cursor" : "Nothing to run");
    return;
  }

  clearDiagnostics(tabId);
  const previous = before.run;
  if (previous) dropResults((id) => id.startsWith(`${previous.id}:`));
  const queryId = crypto.randomUUID();
  // Error positions and spans refer to the document as it is now; edits made later are mapped.
  anchorSubmission(tabId, "run", queryId);
  const live: LiveRun = {
    tabId,
    state: newRun(queryId, sub.sql, sub.baseOffset),
    scheduled: false,
  };
  liveRuns.set(tabId, live);
  updateQueryTab(tabId, { run: live.state, pane: "messages" });

  const alive = () => getQueryTab(tabId).run?.id === queryId;
  const onEvent = (e: QueryEvent) => {
    if (!alive()) return;
    // `done` ends the run: show it at once. Everything else is batched per frame.
    updateLive(live, applyEvent(live.state, e, resultData), e.event === "done");
  };

  // The statements that ran (or started): their DDL makes cached schema data stale.
  let ran: readonly { sql: string }[];
  try {
    const results: StatementResult[] = await ipc.query.run(
      info.connectionId,
      queryId,
      sub.sql,
      { maxRows: MAX_ROWS },
      onEvent,
    );
    ran = results;
    // Channel messages can trail the command response in Tauri: give them a moment.
    if (!isMock && live.state.status === "running") await tick(80);
    if (alive()) updateLive(live, finalizeRun(live.state, results, resultData), true);
  } catch (e) {
    ran = live.state.statements;
    if (alive()) updateLive(live, failRun(live.state, normalizeError(toIpcError(e).payload)), true);
  } finally {
    if (liveRuns.get(tabId) === live) liveRuns.delete(tabId);
  }
  if (ran.some((r) => changesSchema(r.sql))) hooks.onSchemaChanged?.();

  flushResultVersions();
  if (alive()) {
    commitLive(live);
    const run = live.state;
    applyToEditor(tabId, (s) => {
      const diagnostics = runDiagnostics(run, s.doc.length, (r) =>
        toCurrentDoc(tabId, run.id, r, { unlessEdited: true }),
      );
      return diagnostics.length > 0 ? setDiagnostics(s, diagnostics) : null;
    });
    if (firstError(run) || run.results.length === 0) updateQueryTab(tabId, { pane: "messages" });
  }
  hooks.onHistoryChanged?.();
}

/** Stops the running statement (or explain) of the tab. */
export async function cancelQuery(tabId: string): Promise<void> {
  const info = queryTab(tabId);
  if (!info) return;
  const s = getQueryTab(tabId);
  const id =
    s.run?.status === "running" ? s.run.id : s.plan?.status === "running" ? s.plan.id : null;
  if (!id) return;
  if (s.run?.status === "running" && s.run.id === id) {
    const live = liveRuns.get(tabId);
    if (live?.state.id === id) updateLive(live, { ...live.state, cancelling: true }, true);
    else updateQueryTab(tabId, { run: { ...s.run, cancelling: true } });
  }
  try {
    await ipc.query.cancel(info.connectionId, id);
  } catch (e) {
    toast.error("Could not cancel the query", { description: toIpcError(e).message });
  }
}

/** EXPLAIN (or EXPLAIN ANALYZE) of the selection / statement under the cursor. */
export async function explainQuery(
  tabId: string,
  mode: ExplainMode = "plan",
  hooks: ControllerHooks = {},
): Promise<void> {
  const info = queryTab(tabId);
  const state = getEditorState(tabId);
  if (!info || !state) return;
  if (isBusy(getQueryTab(tabId))) return busyNotice();
  const sub = resolveTarget(state, "current", info.dialect);
  if (!sub || sub.sql.trim() === "") {
    toast.message("No statement under the cursor");
    return;
  }
  clearDiagnostics(tabId);
  const id = crypto.randomUUID();
  anchorSubmission(tabId, "plan", id);
  const startedAt = Date.now();
  updateQueryTab(tabId, {
    pane: "plan",
    plan: {
      id,
      mode,
      sql: sub.sql,
      baseOffset: sub.baseOffset,
      status: "running",
      result: null,
      error: null,
      startedAt,
      finishedAt: null,
    },
  });
  const settle = (patch: Partial<NonNullable<QueryTabState["plan"]>>) =>
    useQueryTabsStore
      .getState()
      .update(tabId, (s) =>
        s.plan?.id === id ? { plan: { ...s.plan, ...patch, finishedAt: Date.now() } } : {},
      );
  try {
    const result = await ipc.query.explain(info.connectionId, sub.sql, mode, id);
    settle({ status: "done", result });
  } catch (e) {
    const error = normalizeError(toIpcError(e).payload);
    settle({ status: error.code === "CANCELLED" ? "cancelled" : "error", error });
    const range = error.code === "CANCELLED" ? null : errorRange(error, sub);
    const at = range && toCurrentDoc(tabId, id, range, { unlessEdited: true });
    if (at)
      applyToEditor(tabId, (s) =>
        setDiagnostics(s, [diagnostic(clampRange(at, s.doc.length), error)]),
      );
  }
  hooks.onHistoryChanged?.();
}

const FORMAT_LANGUAGE = { postgres: "postgresql", mysql: "mysql", sqlite: "sqlite" } as const;

/** Formats the selection (or the whole document) with sql-formatter. */
export async function formatQuery(tabId: string): Promise<void> {
  const info = queryTab(tabId);
  const state = getEditorState(tabId);
  if (!info || !state) return;
  const sel = state.selection.main;
  const from = sel.empty ? 0 : sel.from;
  const to = sel.empty ? state.doc.length : sel.to;
  const text = state.sliceDoc(from, to);
  if (text.trim() === "") return;
  const prefs = getPrefs();
  let out: string;
  try {
    const { format } = await import("sql-formatter");
    out = format(text, {
      language: FORMAT_LANGUAGE[info.dialect],
      keywordCase: prefs.editorUppercaseKeywords ? "upper" : "lower",
      tabWidth: prefs.editorTabSize,
    });
  } catch (e) {
    toast.error("Could not format SQL", {
      description: e instanceof Error ? e.message.split("\n")[0] : String(e),
    });
    return;
  }
  if (out === text) return;
  applyToEditor(tabId, () => ({
    changes: { from, to, insert: out },
    selection: sel.empty
      ? EditorSelection.cursor(Math.min(sel.head, out.length))
      : EditorSelection.range(from, from + out.length),
    scrollIntoView: true,
    userEvent: "input.format",
  }));
  focusEditor(tabId);
}

/** Inserts `sql` at the cursor on its own line; with `run`, selects it and runs it. */
export async function insertSql(
  tabId: string,
  sql: string,
  opts: { run?: boolean } & ControllerHooks = {},
): Promise<void> {
  let text = sql.trim();
  if (!text) return;
  if (!/;\s*$/.test(text)) text += ";";
  let inserted: Submission | null = null;
  applyToEditor(tabId, (s) => {
    const sel = s.selection.main;
    const line = s.doc.lineAt(sel.from);
    const prefix = s.sliceDoc(line.from, sel.from).trim() ? "\n" : "";
    const after = s.doc.lineAt(sel.to);
    const suffix = s.sliceDoc(sel.to, after.to).trim() ? "\n" : "";
    const start = sel.from + prefix.length;
    inserted = { sql: text, baseOffset: start };
    return {
      changes: { from: sel.from, to: sel.to, insert: prefix + text + suffix },
      selection: opts.run
        ? EditorSelection.range(start, start + text.length)
        : EditorSelection.cursor(start + text.length),
      scrollIntoView: true,
      userEvent: "input.paste",
    };
  });
  focusEditor(tabId);
  if (opts.run && inserted) await runQuery(tabId, inserted, opts);
}

/** Saves the buffer to its file (asking for a path the first time, or always with `saveAs`). */
export async function saveQuery(tabId: string, opts: { saveAs?: boolean } = {}): Promise<boolean> {
  const info = queryTab(tabId);
  const state = getEditorState(tabId);
  if (!info || !state) return false;
  let path = getQueryTab(tabId).filePath;
  if (!path || opts.saveAs) {
    // A title that already names a .sql file ("users.sql" from "Open in SQL") keeps its extension.
    const title = info.tab.title.replace(/[^\w.-]+/g, "-");
    const suggested = path ? baseName(path) : /\.sql$/i.test(title) ? title : `${title}.sql`;
    path = await pickSavePath(suggested);
  }
  if (!path) return false;
  const text = getEditorState(tabId)?.doc.toString() ?? state.doc.toString();
  try {
    await ipc.files.writeText(path, text);
  } catch (e) {
    toast.error("Could not save the file", { description: toIpcError(e).message });
    return false;
  }
  markSaved(tabId, text);
  updateQueryTab(tabId, { filePath: path });
  useTabsStore.getState().rename(tabId, baseName(path));
  toast.success(`Saved ${baseName(path)}`);
  return true;
}

/**
 * Opens a .sql file: into this tab when it is empty and clean, otherwise into a new query tab (or
 * the tab that already shows that file).
 */
export async function openQueryFile(tabId: string): Promise<void> {
  const info = queryTab(tabId);
  if (!info) return;
  const path = await pickOpenPath();
  if (!path) return;
  const already = Object.entries(useQueryTabsStore.getState().slices).find(
    ([id, s]) => s.filePath === path && useTabsStore.getState().tabs.some((t) => t.id === id),
  );
  if (already) {
    useTabsStore.getState().activate(already[0]);
    return;
  }
  let text: string;
  try {
    text = await ipc.files.readText(path);
  } catch (e) {
    toast.error("Could not open the file", { description: toIpcError(e).message });
    return;
  }
  const state = getEditorState(tabId);
  const tabs = useTabsStore.getState();
  const reuse = state !== null && state.doc.length === 0 && !info.tab.dirty;
  let target = tabId;
  if (reuse) {
    applyToEditor(tabId, (s) => ({
      changes: { from: 0, to: s.doc.length, insert: text },
      selection: EditorSelection.cursor(0),
      scrollIntoView: true,
    }));
    markSaved(tabId, getEditorState(tabId)?.doc ?? text);
  } else {
    target = tabs.newQuery(info.connectionId);
    seedEditor(target, text, { dialect: info.dialect });
  }
  updateQueryTab(target, { filePath: path });
  useTabsStore.getState().rename(target, baseName(path));
}

/**
 * The finished result set the tab shows, as an export source of the rows it holds (copied, so
 * a later run cannot change what is exported); null when the pane shows no finished result.
 * A result whose statement was cancelled or failed after some rows arrived is marked `stopped`:
 * those rows are not the whole result.
 */
export function shownResultSource(tabId: string): ExportSourceSpec | null {
  const { run, pane } = getQueryTab(tabId);
  const index = run ? run.results.findIndex((r) => r.id === pane) : -1;
  const info = run?.results[index];
  if (!run || !info?.done) return null;
  const buffer = resultData.get(info.id);
  if (!buffer) return null;
  const columns = buffer.columns.length > 0 ? buffer.columns : info.columns;
  const statement = run.statements.find((s) => s.index === info.statementIndex);
  // Without its statement (the command itself failed), the run's outcome says how it ended.
  const status = statement?.status ?? (run.status === "done" ? "ok" : run.status);
  return {
    kind: "rows",
    columns: columns.map((c) => ({ ...c })),
    rows: buffer.rows.map((row) => [...row]),
    label: `Result ${index + 1}`,
    table: resultTable(columns, statement?.sql ?? null),
    // Running the statement again exports its first result set only.
    sql: info.set === 0 ? (statement?.sql ?? null) : null,
    truncated: info.truncated,
    stopped: status === "ok" ? null : status === "cancelled" ? "cancelled" : "error",
  };
}

/**
 * "Export results…" (⌘⇧E in a query tab): exports the result set the results pane shows, as it
 * is. Without one, opens the export dialog on the selection, or else the statement under the
 * cursor. An export streams exactly one statement that returns rows, so a selection spanning
 * several statements is refused here rather than by the backend.
 */
export function exportQuery(tabId: string): void {
  const info = queryTab(tabId);
  if (!info) return;
  const shown = shownResultSource(tabId);
  if (shown) {
    openExport({ connectionId: info.connectionId, source: shown });
    return;
  }
  const state = getEditorState(tabId);
  if (!state) return;
  const sub = resolveTarget(state, "current", info.dialect);
  const statements = sub ? splitStatements(sub.sql, info.dialect) : [];
  const only = statements.length === 1 ? statements[0]! : null;
  if (!sub || !only) {
    toast.message(
      statements.length > 1 ? "Select a single statement to export" : "Nothing to export",
      {
        description:
          statements.length > 1
            ? "An export runs one query that returns rows."
            : "Put the cursor in a query, or select one.",
      },
    );
    return;
  }
  openExport({
    connectionId: info.connectionId,
    source: { kind: "sql", sql: sub.sql.slice(only.start, only.end) },
  });
}

/** Opens / closes the history sheet. */
export function toggleHistory(tabId: string, open?: boolean): void {
  updateQueryTab(tabId, (s) => ({ historyOpen: open ?? !s.historyOpen }));
}
