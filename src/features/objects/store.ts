import { create } from "zustand";
import { openToolTab } from "@/features/tabs/toolTabs";
import type {
  Dialect,
  ObjectTarget,
  RoutineRef,
  RoutineSummary,
  TableRef,
  TableSummary,
} from "@/lib/ipc";

/** The sidebar dialogs `ObjectsHost` shows (one at a time). */
export type ObjectDialog =
  | {
      kind: "dropTables";
      tables: ObjectTarget[];
      /** Preset by ⇧⌫ (PostgreSQL CASCADE) / ⌘⌫ (MySQL without foreign-key checks). */
      cascade?: boolean;
      ignoreForeignKeys?: boolean;
    }
  | { kind: "truncateTables"; tables: TableRef[] }
  | { kind: "dropRoutines"; routines: RoutineRef[]; names: string[] }
  | { kind: "dropSequences"; sequences: TableRef[] }
  | { kind: "clone"; table: TableSummary }
  | { kind: "createTable"; schema: string | null }
  | { kind: "renameTable"; table: TableSummary };

export interface ObjectDialogRequest {
  connectionId: string;
  dialect: Dialect;
  dialog: ObjectDialog;
}

export interface OverviewRequest {
  connectionId: string;
  dialect: Dialect;
  schema: string;
}

interface ObjectsState {
  dialog: ObjectDialogRequest | null;
  overview: OverviewRequest | null;
}

export const useObjectsStore = create<ObjectsState>()(() => ({ dialog: null, overview: null }));

export function openObjectDialog(request: ObjectDialogRequest): void {
  useObjectsStore.setState({ dialog: request });
}

export function closeObjectDialog(): void {
  useObjectsStore.setState({ dialog: null });
}

/** The Item Overview's tool tab (`registerToolTab`, ItemOverview.tsx). */
export const OVERVIEW_TOOL = "overview";

export interface OverviewParams {
  schema: string;
}

export const overviewTitle = (params: OverviewParams): string => `Overview: ${params.schema}`;

/** Saved / handed-over params of an overview tab (null when they are not usable). */
export function overviewParams(raw: unknown): OverviewParams | null {
  if (raw === null || typeof raw !== "object") return null;
  const schema = (raw as { schema?: unknown }).schema;
  return typeof schema === "string" && schema !== "" ? { schema } : null;
}

/**
 * Item Overview (spec §11.2) of `schema`: opens as a tab of the connection's workspace (the
 * schema's open overview tab is shown again).
 */
export function openItemOverview(request: OverviewRequest): void {
  const params: OverviewParams = { schema: request.schema };
  openToolTab(request.connectionId, OVERVIEW_TOOL, params, { title: overviewTitle(params) });
}

export function closeItemOverview(): void {
  useObjectsStore.setState({ overview: null });
}

export function routineRef(r: RoutineSummary): RoutineRef {
  return {
    schema: r.schema,
    name: r.name,
    kind: r.kind,
    identityArguments: r.identityArguments,
  };
}
