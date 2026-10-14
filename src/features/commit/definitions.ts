import { create } from "zustand";
import { getEditorState, markSaved } from "@/features/editor/editorStates";
import { openSqlInNewQueryTab } from "@/features/editor/openSql";
import { type DefinitionRestore, type RoutineRef } from "@/lib/ipc";
import { useTabsStore } from "@/stores/tabs";

/**
 * A routine (or a new view / function / procedure) edited in a SQL tab (TablePlus §4.5). Its
 * unsaved text is a pending change of the workspace (`features/commit/workspace.ts`, the
 * `definition` source): counted by the action control, previewed with ⇧⌘P, committed together
 * with every other pending change by ⌘S, discarded (the buffer reverts to what the tab was
 * opened with) by ⇧⌘⌫. `drop` is the routine the tab was opened from (dropped first,
 * `DROP … IF EXISTS`; PostgreSQL replaces it in place when the text is a `CREATE OR REPLACE` with
 * the same signature); null for a template, which only creates. MySQL commits the DROP before the
 * CREATE runs, so when the new text fails the backend creates the previous version again
 * (`DefinitionRestore`), and the Preview sheet says how that went.
 */
export interface DefinitionTab {
  drop: RoutineRef | null;
}

interface DefinitionState {
  byTab: Record<string, DefinitionTab>;
  /**
   * What each definition tab holds as saved: the text it was opened with (a routine's
   * definition, a template), then the text of its last successful commit. Discard puts it back.
   */
  loaded: Record<string, string>;
}

export const useDefinitionStore = create<DefinitionState>()(() => ({ byTab: {}, loaded: {} }));

/**
 * Marks the query tab as a definition editor (see `DefinitionTab`). `loaded` is the text the tab
 * holds as saved; by default the buffer as it is now (the caller just seeded it).
 */
export function markDefinition(tabId: string, def: DefinitionTab, loaded?: string): void {
  const text = loaded ?? getEditorState(tabId)?.doc.toString() ?? "";
  useDefinitionStore.setState((s) => ({
    byTab: { ...s.byTab, [tabId]: def },
    loaded: { ...s.loaded, [tabId]: text },
  }));
}

export function definitionOf(tabId: string): DefinitionTab | null {
  return useDefinitionStore.getState().byTab[tabId] ?? null;
}

/** The text a definition tab was opened with / last committed (what Discard puts back). */
export function definitionLoadedText(tabId: string): string {
  return useDefinitionStore.getState().loaded[tabId] ?? "";
}

/** "function slugify", "procedure refresh", "view": how the toasts name the object. */
export function definitionLabel(sql: string, routine: RoutineRef | null): string {
  if (routine) return `${routine.kind === "procedure" ? "procedure" : "function"} ${routine.name}`;
  if (
    /^\s*(?:--[^\n]*\n|\/\*[\s\S]*?\*\/|\s)*create\s+(?:or\s+replace\s+)?materialized\s+view\b/i.test(
      sql,
    )
  )
    return "materialized view";
  if (
    /^\s*(?:--[^\n]*\n|\/\*[\s\S]*?\*\/|\s)*create\s+(?:or\s+replace\s+)?(?:temp\w*\s+)?view\b/i.test(
      sql,
    )
  )
    return "view";
  return "definition";
}

/**
 * What became of the previous version of a routine whose replacement failed, in a sentence for
 * the sheet: put back (maybe without its DEFINER), or lost (its text is kept in a tab).
 */
export function restoreNote(r: DefinitionRestore): string {
  if (r.restored)
    return r.withoutDefiner
      ? "The previous version was put back without its DEFINER (this account may not assume it), so the connected account owns it now."
      : "The previous version was put back; nothing changed on the server.";
  const why = r.error ? ` (${r.error.message})` : "";
  return `The previous version could not be put back${why}. Its text is shown below and was opened in a new editor tab: saving there creates it again.`;
}

/**
 * Keeps the text of a routine that could not be put back: a new SQL tab (behind the one being
 * saved, which stays active) where ⌘S creates it again.
 */
export function keepPreviousVersion(
  connectionId: string,
  routine: RoutineRef | null,
  text: string,
): void {
  const active = useTabsStore.getState().activeId;
  const id = openSqlInNewQueryTab(
    connectionId,
    text.endsWith(";") ? text : `${text};\n`,
    "mysql",
    `${routine?.name ?? "routine"} (previous)`,
  );
  // Nothing exists on the server for this text any more: it is a pending creation (counted,
  // committed by ⌘S), and closing the tab asks before the only copy goes.
  markSaved(id, "");
  markDefinition(id, { drop: null }, "");
  if (active) useTabsStore.getState().activate(active);
}

/** Test helper. */
export function resetDefinitions(): void {
  useDefinitionStore.setState({ byTab: {}, loaded: {} });
}
