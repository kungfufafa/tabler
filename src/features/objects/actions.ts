import { EditorSelection } from "@codemirror/state";
import { toast } from "sonner";
import { openCreateSchema } from "@/features/admin/store";
import { markDefinition } from "@/features/commit/definitions";
import { coreDialect } from "@/features/drivers/catalog";
import { applyToEditor } from "@/features/editor/editorStates";
import { openSqlInNewQueryTab } from "@/features/editor/openSql";
import { openExportTables } from "@/features/export/store";
import { openImport, type ImportKind } from "@/features/import/store";
import { actions } from "@/features/palette/actions";
import { openGroupDialog } from "@/features/sidebar/groups";
import {
  ipc,
  toIpcError,
  type Driver,
  type ObjectOp,
  type RoutineSummary,
  type SequenceSummary,
  type TableRef,
  type TableSummary,
} from "@/lib/ipc";
import { isPinned, togglePin } from "@/stores/pins";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { laravelMigrations } from "./laravel";
import type { NewTarget } from "./menu";
import { openObjectDialog, routineRef } from "./store";
import { newObjectTemplate, type NewObjectKind } from "./templates";

/** The connection and sidebar state a menu acts in. */
export interface ObjectContext {
  connectionId: string;
  driver: Driver;
  readOnly: boolean;
  /** The schema the sidebar shows (PG schema, MySQL database, SQLite attached database). */
  schema: string | null;
  /** Where pins are kept (`pinScope`); null when pinning is unavailable. */
  pinScope: string | null;
}

export const refOf = (t: Pick<TableSummary, "schema" | "name">): TableRef => ({
  schema: t.schema,
  name: t.name,
});

/** Shows the workspace (the connection manager may be in front). */
function showWorkspace(): void {
  useUiStore.getState().setView("workspace");
}

export function copyText(text: string, what = "Copied"): void {
  const clip = typeof navigator !== "undefined" ? navigator.clipboard : undefined;
  if (!clip) {
    toast.error("Could not copy");
    return;
  }
  void clip.writeText(text).then(
    () => toast.success(what, { description: text.length > 200 ? `${text.slice(0, 200)}…` : text }),
    () => toast.error("Could not copy"),
  );
}

/** Copy Name: every selected item, comma-separated. */
export function copyNames(names: readonly string[]): void {
  copyText(names.join(", "), names.length > 1 ? `Copied ${names.length} names` : "Copied");
}

/** Statements as one script, each ending in `;`. */
export function scriptOf(statements: readonly string[]): string {
  return statements.map((s) => (s.trimEnd().endsWith(";") ? s.trimEnd() : `${s};`)).join("\n");
}

function failed(what: string, e: unknown): void {
  toast.error(what, { description: toIpcError(e).message });
}

export function openTableTab(
  ctx: ObjectContext,
  table: TableSummary,
  kind: "table" | "structure" = "table",
): void {
  useTabsStore.getState().openTable(ctx.connectionId, refOf(table), kind);
  showWorkspace();
}

/** Opens a routine's definition in a new SQL tab titled with its name. */
export async function openRoutine(ctx: ObjectContext, r: RoutineSummary): Promise<void> {
  try {
    const sql = await ipc.objects.routineDefinition(ctx.connectionId, routineRef(r));
    const id = openSqlInNewQueryTab(ctx.connectionId, sql, coreDialect(ctx.driver), r.name);
    // ⌘S in this tab drops and recreates the routine (TablePlus), instead of saving a file.
    markDefinition(id, { drop: routineRef(r) });
    showWorkspace();
  } catch (e) {
    failed(`Could not open ${r.name}`, e);
  }
}

/** Opens a sequence's CREATE SEQUENCE text in a new SQL tab. */
export async function openSequence(ctx: ObjectContext, s: SequenceSummary): Promise<void> {
  try {
    const sql = await ipc.objects.sequenceDefinition(ctx.connectionId, refOf(s));
    openSqlInNewQueryTab(ctx.connectionId, sql, coreDialect(ctx.driver), s.name);
    showWorkspace();
  } catch (e) {
    failed(`Could not open ${s.name}`, e);
  }
}

/** New ▸ View… / Function… …: a SQL tab with the template, its placeholder name selected. */
export function openNewObject(ctx: ObjectContext, kind: NewObjectKind): string {
  const t = newObjectTemplate(coreDialect(ctx.driver), kind, ctx.schema);
  const id = openSqlInNewQueryTab(ctx.connectionId, t.sql, coreDialect(ctx.driver), t.title);
  applyToEditor(id, () => ({ selection: EditorSelection.range(t.from, t.to) }));
  // ⌘S in the template's tab creates the object (instead of asking to save a file).
  markDefinition(id, { drop: null });
  showWorkspace();
  return id;
}

/**
 * New ▸ … and the "+" menu: Table… opens the create-table dialog (`createTable`, or the host's),
 * View… / Function… … a SQL tab with a template, Schema… the admin dialog, Group… the sidebar
 * group dialog, New Query a query tab.
 */
export function runNew(ctx: ObjectContext, what: NewTarget, createTable?: () => void): void {
  switch (what) {
    case "table":
      if (createTable) createTable();
      else
        openObjectDialog({
          connectionId: ctx.connectionId,
          dialect: coreDialect(ctx.driver),
          dialog: { kind: "createTable", schema: ctx.schema },
        });
      return;
    case "schema":
      openCreateSchema(ctx.connectionId);
      return;
    case "query":
      void actions.run("tab.new-query");
      return;
    case "group":
      if (ctx.pinScope !== null) openGroupDialog({ scope: ctx.pinScope });
      return;
    default:
      openNewObject(ctx, what);
  }
}

/** Pin to Top for every target; when all of them are pinned already, unpins them. */
export function togglePins(scope: string, tables: readonly TableRef[]): void {
  const allPinned = tables.every((t) => isPinned(scope, t));
  for (const t of tables) if (isPinned(scope, t) === allPinned) togglePin(scope, t);
}

export function exportTables(ctx: ObjectContext, tables: readonly TableSummary[]): void {
  openExportTables(ctx.connectionId, tables.map(refOf));
}

export function importInto(ctx: ObjectContext, kind: ImportKind, table: TableSummary | null): void {
  openImport({ connectionId: ctx.connectionId, kind, table: table ? refOf(table) : null });
}

async function copyPlan(ctx: ObjectContext, op: ObjectOp, what: string): Promise<void> {
  try {
    const plan = await ipc.objects.preview(ctx.connectionId, op);
    copyText(scriptOf(plan.statements), `Copied ${what} script`);
  } catch (e) {
    failed(`Could not build the ${what} script`, e);
  }
}

export type ScriptKind = "creation" | "drop" | "truncate";

/** Copy Script As ▸ CREATION / DROP / TRUNCATE of the clicked table or view. */
export async function copyTableScript(
  ctx: ObjectContext,
  table: TableSummary,
  kind: ScriptKind,
): Promise<void> {
  if (kind === "creation") {
    try {
      copyText(await ipc.ddl.tableDdl(ctx.connectionId, refOf(table)), "Copied CREATE script");
    } catch (e) {
      failed("Could not load the CREATE script", e);
    }
    return;
  }
  const op: ObjectOp =
    kind === "drop"
      ? {
          op: "dropTables",
          tables: [{ table: refOf(table), kind: table.kind }],
          cascade: false,
          ignoreForeignKeys: false,
        }
      : {
          op: "truncateTables",
          tables: [refOf(table)],
          cascade: false,
          restartIdentity: false,
          ignoreForeignKeys: false,
        };
  await copyPlan(ctx, op, kind === "drop" ? "DROP" : "TRUNCATE");
}

/**
 * Copy Script As ▸ Laravel Migration: one migration per table (from its metadata), separated by
 * `// ----- <table> -----` lines.
 */
export async function copyLaravelMigrations(
  ctx: ObjectContext,
  tables: readonly TableSummary[],
): Promise<void> {
  if (tables.length === 0) return;
  try {
    const metas = await Promise.all(
      tables.map((t) => ipc.schema.table(ctx.connectionId, refOf(t))),
    );
    copyText(
      laravelMigrations(metas, coreDialect(ctx.driver)),
      tables.length > 1 ? `Copied ${tables.length} Laravel migrations` : "Copied Laravel migration",
    );
  } catch (e) {
    failed("Could not build the Laravel migration", e);
  }
}

export async function copyRoutineScript(
  ctx: ObjectContext,
  r: RoutineSummary,
  kind: Exclude<ScriptKind, "truncate">,
): Promise<void> {
  if (kind === "creation") {
    try {
      copyText(
        await ipc.objects.routineDefinition(ctx.connectionId, routineRef(r)),
        "Copied CREATE script",
      );
    } catch (e) {
      failed("Could not load the CREATE script", e);
    }
    return;
  }
  await copyPlan(ctx, { op: "dropRoutines", routines: [routineRef(r)], cascade: false }, "DROP");
}

export async function copySequenceScript(
  ctx: ObjectContext,
  s: SequenceSummary,
  kind: Exclude<ScriptKind, "truncate">,
): Promise<void> {
  if (kind === "creation") {
    try {
      copyText(
        await ipc.objects.sequenceDefinition(ctx.connectionId, refOf(s)),
        "Copied CREATE script",
      );
    } catch (e) {
      failed("Could not load the CREATE script", e);
    }
    return;
  }
  await copyPlan(ctx, { op: "dropSequences", sequences: [refOf(s)], cascade: false }, "DROP");
}
