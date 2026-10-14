import { toast } from "sonner";
import { useConsoleStore } from "@/features/console/store";
import { getPrefs } from "@/stores/prefs";
import { useQueryTabsStore } from "@/stores/queryTabs";
import { useTabsStore } from "@/stores/tabs";
import type { RunNotice, RunState } from "./runModel";

/**
 * PostgreSQL `RAISE NOTICE` / `WARNING` as notifications (TablePlus b174, ⚙ ▸ "Enable Raise Notice
 * Notifications"): while a run is in progress, each notice its statements raise shows a toast;
 * the Messages tab lists them under their statement either way. Notices raised outside the run's
 * statements (bootstrap SQL…) go to the console log instead.
 */

/** Severities that get a toast (`LOG` / `DEBUG` only reach the client when asked for). */
const TOASTED = new Set(["NOTICE", "WARNING", "INFO"]);
/** Toasts per run; the rest are summed up in one "N more notices" toast. */
export const MAX_NOTICE_TOASTS = 5;
/** Longest message a toast shows. */
const TOAST_MAX_CHARS = 200;

/** `NOTICE: hi` (the first line of what psql prints). */
export function noticeText(n: RunNotice): string {
  return `${n.severity}: ${n.message}`;
}

/** The lines below the message: `DETAIL: …`, `HINT: …`. */
export function noticeDetails(n: RunNotice): string[] {
  const out: string[] = [];
  if (n.detail) out.push(`DETAIL: ${n.detail}`);
  if (n.hint) out.push(`HINT: ${n.hint}`);
  return out;
}

export const isWarning = (n: RunNotice): boolean => n.severity.toUpperCase() === "WARNING";

/** How many notices of a run were already reported, per statement and outside them. */
export interface NoticeCursor {
  statements: Map<number, number>;
  outside: number;
  toasts: number;
  /** Notices past `MAX_NOTICE_TOASTS` that got no toast of their own. */
  untoasted: number;
}

export const newCursor = (): NoticeCursor => ({
  statements: new Map(),
  outside: 0,
  toasts: 0,
  untoasted: 0,
});

/** The notices that arrived since `cursor` (which moves past them). */
export function takeNewNotices(
  run: RunState,
  cursor: NoticeCursor,
): { statement: RunNotice[]; outside: RunNotice[] } {
  const statement: RunNotice[] = [];
  for (const st of run.statements) {
    const list = st.notices ?? [];
    const seen = cursor.statements.get(st.index) ?? 0;
    if (list.length <= seen) continue;
    statement.push(...list.slice(seen));
    cursor.statements.set(st.index, list.length);
  }
  const all = run.outsideNotices ?? [];
  const outside = all.slice(cursor.outside);
  cursor.outside = all.length;
  return { statement, outside };
}

const clip = (text: string) =>
  text.length > TOAST_MAX_CHARS ? `${text.slice(0, TOAST_MAX_CHARS - 1)}…` : text;

/** Shows the toasts of `notices` (up to `MAX_NOTICE_TOASTS` per run, then a running count). */
export function toastNotices(runId: string, notices: readonly RunNotice[], cursor: NoticeCursor) {
  for (const n of notices) {
    if (!TOASTED.has(n.severity.toUpperCase())) continue;
    if (cursor.toasts >= MAX_NOTICE_TOASTS) {
      cursor.untoasted++;
      continue;
    }
    cursor.toasts++;
    const details = noticeDetails(n);
    const options = details.length > 0 ? { description: clip(details.join("\n")) } : {};
    if (isWarning(n)) toast.warning(clip(noticeText(n)), options);
    else toast.message(clip(noticeText(n)), options);
  }
  if (cursor.untoasted > 0)
    toast.message(
      `${cursor.untoasted} more notice${cursor.untoasted === 1 ? "" : "s"} in Messages`,
      { id: `notices-${runId}` },
    );
}

/** Notices raised outside a run's statements, as console log entries of the tab's connection. */
function logOutside(tabId: string, notices: readonly RunNotice[]) {
  if (notices.length === 0) return;
  const tab = useTabsStore.getState().tabs.find((t) => t.id === tabId);
  if (tab?.kind !== "query") return;
  const { log } = useConsoleStore.getState();
  for (const n of notices)
    log({
      connectionId: tab.connectionId,
      category: "query",
      sql: [noticeText(n), ...noticeDetails(n)].map((line) => `-- ${line}`).join("\n"),
      meta: true,
      label: n.severity.toUpperCase(),
    });
}

let installed = false;
/** Runs seen running, by id. */
const cursors = new Map<string, NoticeCursor>();

/** Watches the query tabs' runs for new notices (once, app-wide). */
export function installNoticeNotifications(): void {
  if (installed) return;
  installed = true;
  useQueryTabsStore.subscribe((s, prev) => {
    if (s.slices === prev.slices) return;
    for (const [tabId, slice] of Object.entries(s.slices)) {
      const run = slice.run;
      if (!run || run === prev.slices[tabId]?.run) continue;
      let cursor = cursors.get(run.id);
      if (!cursor) {
        // Only runs in progress notify (not one restored or shown after it ended).
        if (run.status !== "running") continue;
        cursor = newCursor();
        cursors.set(run.id, cursor);
      }
      const { statement, outside } = takeNewNotices(run, cursor);
      if (getPrefs().editorRaiseNotices) toastNotices(run.id, statement, cursor);
      logOutside(tabId, outside);
      if (run.status !== "running") cursors.delete(run.id);
    }
    // Runs replaced (or whose tab closed) before they ended.
    if (cursors.size > 16) {
      const live = new Set(Object.values(s.slices).map((x) => x.run?.id));
      for (const id of cursors.keys()) if (!live.has(id)) cursors.delete(id);
    }
  });
}
