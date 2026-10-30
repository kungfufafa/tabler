import { toggleLineComment } from "@codemirror/commands";
import { diagnosticCount, setDiagnostics } from "@codemirror/lint";
import { EditorSelection, type EditorState } from "@codemirror/state";
import type { SqlLanguage } from "sql-formatter";
import { toast } from "sonner";
import {
  ipc,
  isMock,
  toIpcError,
  type ExplainMode,
  type QueryEvent,
  type StatementResult,
} from "@/lib/ipc";
import { openExport, type ExportSourceSpec } from "@/features/export";
import { resultTable } from "@/features/export/resultTable";
import { definitionOf } from "@/features/commit/definitions";
import { commitWorkspace, workspacePendingCount } from "@/features/commit/workspace";
import { hasPendingResultEdits } from "@/features/results/commit";
import { confirmRerunDiscard } from "@/features/results/rerunConfirm";
import { discardRunEdits, pendingForRun, useResultsStore } from "@/features/results/store";
import { guardExecution } from "@/features/safety/guard";
import { allowFeature } from "@/lib/caps";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
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
import { toggleBlockCommentSmart } from "./cmExtensions";
import {
  applyToEditor,
  focusEditor,
  getEditorState,
  getEditorView,
  markSaved,
} from "./editorStates";
import {
  clampRange,
  diagnostic,
  errorRange,
  runDiagnostics,
  type DocRange,
  type Submission,
} from "./errorPos";
import { baseName, pickOpenPath, pickSavePath, readSqlFileGuarded } from "./files";
import { linkTabToFile, openFileInNewQueryTab } from "./openSql";
import {
  applyEvent,
  failRun,
  finalizeRun,
  firstError,
  newRun,
  normalizeError,
  type RunState,
} from "./runModel";
import {
  coreLanguage,
  languageOf,
  splitWith,
  statementsWith,
  type EditorLanguage,
} from "./language";
import { findParams, paramFields, substituteParams, type Replacement } from "./params";
import { askParams } from "./paramsPrompt";
import { minifySql, statementAt } from "./scanner";
import { changesSchema } from "./schemaChange";
import { activeEditorKey, splitKey } from "./split";
import { recordSubmission, unsubstituteRange } from "./submissions";

/**
 * The cap sent for "No limit": the backend clamps `maxRows` to this (`MAX_ROWS_LIMIT` in
 * `commands/query.rs`), and 0 would mean its default (10,000), not "unlimited".
 */
export const NO_LIMIT_MAX_ROWS = 1_000_000;

/** The row limits the editor bar offers (null = no limit, the default of a new tab). */
export const ROW_LIMITS: readonly (number | null)[] = [null, 100, 300, 500, 1_000, 10_000];

/** `maxRows` of a run for the tab's row limit. */
export function maxRowsFor(limit: number | null): number {
  return limit === null ? NO_LIMIT_MAX_ROWS : Math.max(1, Math.min(limit, NO_LIMIT_MAX_ROWS));
}

export interface ControllerHooks {
  /** A run / explain finished (the backend wrote history rows). */
  onHistoryChanged?: () => void;
  /**
   * A run included a statement that can change the schema (CREATE, ALTER, DROP…): cached table
   * lists, metadata, rows and completion are stale.
   */
  onSchemaChanged?: () => void;
}

function queryTab(tabId: string) {
  const tab = useTabsStore.getState().tabs.find((t) => t.id === tabId);
  if (!tab || tab.kind !== "query") return null;
  const connection = useConnectionsStore.getState().byId[tab.connectionId];
  // A core dialect, or the language of the session's lane (`languageOf`).
  const dialect = languageOf(connection?.info);
  // No editor for such a connection (`QueryTab` says so), or its lane is still loading:
  // nothing to run, format or save.
  if (!dialect) return null;
  return { tab, connectionId: tab.connectionId, dialect };
}

export type RunTarget = "auto" | "current" | Submission;

/**
 * What to submit: the selection when there is one; otherwise the whole document ("auto") or the
 * statement under the cursor ("current").
 */
export function resolveTarget(
  state: EditorState,
  target: RunTarget,
  dialect: EditorLanguage,
): Submission | null {
  if (typeof target === "object") return target;
  const sel = state.selection.main;
  if (!sel.empty) return { sql: state.sliceDoc(sel.from, sel.to), baseOffset: sel.from };
  const text = state.doc.toString();
  if (target === "auto") return { sql: text, baseOffset: 0 };
  const st = statementAt(splitWith(dialect, text), text, sel.head, true);
  return st ? { sql: text.slice(st.start, st.end), baseOffset: st.start } : null;
}

/** Clears the error squiggles of the tab's editors (both panes when it is split). */
function clearDiagnostics(tabId: string) {
  for (const key of [tabId, splitKey(tabId)])
    applyToEditor(key, (s) => (diagnosticCount(s) > 0 ? setDiagnostics(s, []) : null));
}

function busyNotice(): void {
  toast.message("A query is already running", {
    description: `Stop it first with ${formatCombo(shortcutFor("query.cancel") ?? "Mod+.")}.`,
  });
}

/** SQL about to run, after its query parameters were filled in. */
export interface PreparedSql extends Submission {
  /** The variables replaced (empty when there were none); offsets of the submitted text. */
  replacements: Replacement[];
}

/**
 * Fills in the query parameters of `sub` when they are enabled and it has any: asks for their
 * values (null when the user cancels) and substitutes them as SQL text, as TablePlus does.
 */
export async function prepareSql(
  tabId: string,
  sub: Submission,
  language: EditorLanguage,
): Promise<PreparedSql | null> {
  const prefs = getPrefs();
  // Parameters are read with the core lexers: a lane's text has none.
  const dialect = coreLanguage(language);
  if (!prefs.editorQueryParams || dialect === null) return { ...sub, replacements: [] };
  const occurrences = findParams(sub.sql, dialect, {
    syntax: prefs.editorQueryParamsRegex,
    inQuotes: prefs.editorQueryParamsInQuotes,
  });
  if (occurrences.length === 0) return { ...sub, replacements: [] };
  const values = await askParams(tabId, paramFields(occurrences));
  if (!values) return null;
  const { sql, replacements } = substituteParams(sub.sql, occurrences, values);
  return { sql, baseOffset: sub.baseOffset, replacements };
}

/** Maps a document range of the substituted text back to the variables it came from. */
function unsubstitute(range: DocRange, prepared: PreparedSql): DocRange {
  return unsubstituteRange(range, prepared.baseOffset, prepared.replacements);
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
  // The pane focused last, when the editor is split.
  const key = activeEditorKey(tabId);
  const state = getEditorState(key);
  if (!info || !state) return;
  if (isBusy(getQueryTab(tabId))) return busyNotice();
  const target0 = resolveTarget(state, target, info.dialect);
  if (!target0 || splitWith(info.dialect, target0.sql).length === 0) {
    toast.message(target === "current" ? "No statement under the cursor" : "Nothing to run");
    return;
  }
  // The results this run replaces hold staged edits: ask before they go. A run started by
  // something else (the Re-run button after a commit) drops them with a toast instead.
  const replaced = getQueryTab(tabId).run?.id ?? null;
  let discardAgreed = false;
  if (replaced !== null && typeof target !== "object") {
    const lost = pendingForRun(useResultsStore.getState(), tabId, replaced);
    if (lost > 0) {
      if (!(await confirmRerunDiscard(tabId, lost))) return;
      discardAgreed = true;
    }
  }
  const sub = await prepareSql(tabId, target0, info.dialect);
  if (!sub) return;
  // The connection's safe mode may ask first; declining is not an error.
  const statements = statementsWith(info.dialect, sub.sql);
  if (!(await guardExecution(info.connectionId, { kind: "query", statements }))) return;
  const before = getQueryTab(tabId);
  // Another run may have started while the dialogs were open.
  if (isBusy(before)) return busyNotice();

  clearDiagnostics(tabId);
  // Agreed to above: dropped silently (the results lifecycle would toast about them).
  if (discardAgreed && replaced !== null) discardRunEdits(replaced);
  const previous = before.run;
  if (previous) dropResults((id) => id.startsWith(`${previous.id}:`));
  const queryId = crypto.randomUUID();
  // Error positions and spans refer to the document as it is now; edits made later are mapped.
  anchorSubmission(key, "run", queryId);
  recordSubmission(tabId, "run", queryId, {
    key,
    baseOffset: sub.baseOffset,
    replacements: sub.replacements,
  });
  const live: LiveRun = {
    tabId,
    state: newRun(queryId, sub.sql, sub.baseOffset),
    scheduled: false,
  };
  liveRuns.set(tabId, live);
  // A collapsed result pane (⌃`) opens again for the new run.
  updateQueryTab(tabId, { run: live.state, pane: "messages", resultsCollapsed: false });

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
      // "Return on Error" off: the statements after a failing one still run.
      { maxRows: maxRowsFor(before.rowLimit), stopOnError: getPrefs().editorStopOnError },
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
    applyToEditor(key, (s) => {
      const diagnostics = runDiagnostics(run, s.doc.length, (r) =>
        toCurrentDoc(key, run.id, unsubstitute(r, sub), { unlessEdited: true }),
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
  const key = activeEditorKey(tabId);
  const state = getEditorState(key);
  if (!info || !state) return;
  if (isBusy(getQueryTab(tabId))) return busyNotice();
  if (!allowFeature(info.connectionId, mode === "analyze" ? "explainAnalyze" : "explain")) return;
  const target = resolveTarget(state, "current", info.dialect);
  if (!target || target.sql.trim() === "") {
    toast.message("No statement under the cursor");
    return;
  }
  const sub = await prepareSql(tabId, target, info.dialect);
  if (!sub) return;
  const wrapped = `${mode === "analyze" ? "EXPLAIN ANALYZE" : "EXPLAIN"} ${sub.sql}`;
  if (!(await guardExecution(info.connectionId, { kind: "query", statements: [wrapped] }))) return;
  if (isBusy(getQueryTab(tabId))) return busyNotice();
  clearDiagnostics(tabId);
  const id = crypto.randomUUID();
  anchorSubmission(key, "plan", id);
  recordSubmission(tabId, "plan", id, {
    key,
    baseOffset: sub.baseOffset,
    replacements: sub.replacements,
  });
  const startedAt = Date.now();
  updateQueryTab(tabId, {
    pane: "plan",
    resultsCollapsed: false,
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
    const at = range && toCurrentDoc(key, id, unsubstitute(range, sub), { unlessEdited: true });
    if (at)
      applyToEditor(key, (s) =>
        setDiagnostics(s, [diagnostic(clampRange(at, s.doc.length), error)]),
      );
  }
  hooks.onHistoryChanged?.();
}

/**
 * Edit ▸ Toggle Line Comment (⌘/) / Toggle Block Comment (⌥⌘/) on the tab's focused editor pane:
 * the native menu's items (the keys themselves are the editor's own keymap).
 */
export function toggleComment(tabId: string, kind: "line" | "block"): boolean {
  const key = activeEditorKey(tabId);
  const view = getEditorView(key);
  if (!view) return false;
  const done = kind === "line" ? toggleLineComment(view) : toggleBlockCommentSmart(view);
  focusEditor(key);
  return done;
}

/** `toggleComment` on the active tab, when it is a query tab. */
export function toggleCommentInActiveTab(kind: "line" | "block"): boolean {
  const { tabs, activeId } = useTabsStore.getState();
  const tab = tabs.find((t) => t.id === activeId);
  return tab?.kind === "query" ? toggleComment(tab.id, kind) : false;
}

const FORMAT_LANGUAGE = { postgres: "postgresql", mysql: "mysql", sqlite: "sqlite" } as const;

/** Where the cursor lands when `from..to` is replaced by `length` characters. */
function cursorAfterReplace(head: number, from: number, to: number, length: number): number {
  if (head <= from) return head;
  if (head >= to) return head + length - (to - from);
  return from + Math.min(head - from, length);
}

/**
 * Beautify / Uglify: rewrites the selection, or else the statement under the cursor (terminator
 * excluded), like TablePlus. Nothing happens when the document changed while `rewrite` ran.
 */
async function rewriteQuery(
  tabId: string,
  rewrite: (sql: string, dialect: EditorLanguage) => string | Promise<string>,
): Promise<void> {
  const info = queryTab(tabId);
  const key = activeEditorKey(tabId);
  const state = getEditorState(key);
  if (!info || !state) return;
  if (!allowFeature(info.connectionId, "format")) return;
  const sub = resolveTarget(state, "current", info.dialect);
  if (!sub || sub.sql.trim() === "") return;
  const out = await rewrite(sub.sql, info.dialect);
  if (out === sub.sql) return;
  const from = sub.baseOffset;
  const to = from + sub.sql.length;
  const applied = applyToEditor(key, (s) => {
    if (s.doc !== state.doc) return null;
    const sel = s.selection.main;
    return {
      changes: { from, to, insert: out },
      selection: sel.empty
        ? EditorSelection.cursor(cursorAfterReplace(sel.head, from, to, out.length))
        : EditorSelection.range(from, from + out.length),
      scrollIntoView: true,
      userEvent: "input.format",
    };
  });
  if (applied) focusEditor(key);
}

/** "Beautify SQL": formats the selection (or the statement under the cursor) with sql-formatter. */
export async function formatQuery(tabId: string): Promise<void> {
  const prefs = getPrefs();
  try {
    await rewriteQuery(tabId, async (sql, dialect) => {
      const language = typeof dialect === "string" ? FORMAT_LANGUAGE[dialect] : dialect.formatter;
      // A lane's language without a formatter is left as it is.
      if (language === null) return sql;
      const { format } = await import("sql-formatter");
      return format(sql, {
        language: language as SqlLanguage,
        keywordCase: prefs.editorUppercaseKeywords ? "upper" : "lower",
        tabWidth: prefs.editorTabSize,
        useTabs: prefs.editorIndentWithTabs,
      });
    });
  } catch (e) {
    toast.error("Could not format SQL", {
      description: e instanceof Error ? e.message.split("\n")[0] : String(e),
    });
  }
}

/** "Uglify SQL": puts the selection (or the statement under the cursor) on one line. */
export function minifyQuery(tabId: string): Promise<void> {
  // The core scanner finds the strings and comments to keep; a lane's text is left as it is.
  return rewriteQuery(tabId, (sql, dialect) => {
    const core = coreLanguage(dialect);
    return core === null ? sql : minifySql(sql, core);
  });
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
  const key = activeEditorKey(tabId);
  applyToEditor(key, (s) => {
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
  focusEditor(key);
  if (opts.run && inserted) await runQuery(tabId, inserted, opts);
}

/** Saves the buffer to its file (asking for a path the first time, or always with `saveAs`). */
export async function saveQuery(tabId: string, opts: { saveAs?: boolean } = {}): Promise<boolean> {
  // Results package: with staged edits in an editable result, ⌘S commits them instead of saving
  // the file (TablePlus) — with every other pending change of the workspace; Save As still saves.
  if (!opts.saveAs && hasPendingResultEdits(tabId)) {
    const connectionId = queryTab(tabId)?.connectionId;
    if (connectionId) return commitWorkspace(connectionId);
  }
  const info = queryTab(tabId);
  const state = getEditorState(tabId);
  if (!info || !state) return false;
  // A routine opened from the sidebar, or a New ▸ View / Function / Procedure template: its text
  // is a pending change of the workspace (not a file), so ⌘S commits it - with every other
  // pending change, through the Preview SQL and the safe mode; Save As still saves a file.
  if (!opts.saveAs && definitionOf(tabId)) {
    if (workspacePendingCount(info.connectionId) === 0) {
      toast.message("Nothing to commit", {
        description: "Edit the definition first. Save SQL as… saves it to a file instead.",
      });
      return false;
    }
    return commitWorkspace(info.connectionId);
  }
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
    // A `.sql.gz` path is written compressed.
    await ipc.sqlFiles.write(path, text);
  } catch (e) {
    toast.error("Could not save the file", { description: toIpcError(e).message });
    return false;
  }
  markSaved(tabId, text);
  // Saved: the tab now follows its own file (the head of a big file is a copy no more).
  updateQueryTab(tabId, { filePath: path, headNotice: null });
  useTabsStore.getState().rename(tabId, baseName(path));
  toast.success(`Saved ${baseName(path)}`);
  return true;
}

/** The selected text (every selection range, one per line), or "" when nothing is selected. */
function selectedText(state: EditorState): string {
  return state.selection.ranges
    .filter((r) => !r.empty)
    .map((r) => state.sliceDoc(r.from, r.to))
    .join("\n");
}

/**
 * The SQL a tab would save as a query ("Add to Queries…"): the selection, else the statement
 * under the cursor, else the whole document.
 */
export function sqlToKeep(tabId: string): string {
  const info = queryTab(tabId);
  const state = getEditorState(activeEditorKey(tabId));
  if (!info || !state) return "";
  const selected = selectedText(state);
  if (selected.trim()) return selected.trim();
  const current = resolveTarget(state, "current", info.dialect);
  return (current?.sql ?? state.doc.toString()).trim();
}

/**
 * "Save Selected SQL Statements As…" (⇧⌘S): writes the selection, or the statement under the
 * cursor, to a new file. The tab keeps its own file and stays as it is.
 */
export async function saveSelectionAs(tabId: string): Promise<boolean> {
  const info = queryTab(tabId);
  const state = getEditorState(activeEditorKey(tabId));
  if (!info || !state) return false;
  const selected = selectedText(state);
  const text = selected.trim()
    ? selected
    : (resolveTarget(state, "current", info.dialect)?.sql ?? "");
  if (!text.trim()) {
    toast.message("Nothing to save", {
      description: "Select statements, or put the cursor in one.",
    });
    return false;
  }
  const title = info.tab.title.replace(/\.sql$/i, "").replace(/[^\w.-]+/g, "-");
  const path = await pickSavePath(`${title}-selection.sql`);
  if (!path) return false;
  try {
    await ipc.sqlFiles.write(path, text.endsWith("\n") ? text : `${text}\n`);
  } catch (e) {
    toast.error("Could not save the file", { description: toIpcError(e).message });
    return false;
  }
  toast.success(`Saved ${baseName(path)}`);
  return true;
}

/**
 * Opens a .sql file: into this tab when it is empty and clean, otherwise into a new query tab (or
 * the tab that already shows that file). A big file asks first (`readSqlFileGuarded`): its head
 * opens in a tab that is NOT linked to the file, so saving can never truncate the original.
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
  let file: Awaited<ReturnType<typeof readSqlFileGuarded>>;
  try {
    // `.sql.gz` files open decompressed; a file that is not UTF-8 opens too.
    file = await readSqlFileGuarded(path, { connectionId: info.connectionId });
  } catch (e) {
    toast.error("Could not open the file", { description: toIpcError(e).message });
    return;
  }
  // Cancelled, or sent to the SQL import wizard instead.
  if (!file) return;
  const text = file.text;
  const state = getEditorState(tabId);
  const reuse = state !== null && state.doc.length === 0 && !info.tab.dirty;
  if (!reuse) {
    openFileInNewQueryTab(info.connectionId, path, file, coreLanguage(info.dialect));
    return;
  }
  applyToEditor(tabId, (s) => ({
    changes: { from: 0, to: s.doc.length, insert: text },
    selection: EditorSelection.cursor(0),
    scrollIntoView: true,
  }));
  markSaved(tabId, getEditorState(tabId)?.doc ?? text);
  linkTabToFile(tabId, path, file);
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
  const state = getEditorState(activeEditorKey(tabId));
  if (!state) return;
  const sub = resolveTarget(state, "current", info.dialect);
  const statements = sub ? splitWith(info.dialect, sub.sql) : [];
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

/** Collapses / expands the result pane below the editor (⌃`); `show` forces one way. */
export function toggleResults(tabId: string, show?: boolean): void {
  updateQueryTab(tabId, (s) => ({
    resultsCollapsed: show === undefined ? !s.resultsCollapsed : !show,
  }));
}

/** Opens / closes the history sheet. */
export function toggleHistory(tabId: string, open?: boolean): void {
  updateQueryTab(tabId, (s) => ({ historyOpen: open ?? !s.historyOpen }));
}
