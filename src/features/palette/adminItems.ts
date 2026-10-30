import { toast } from "sonner";
import { currentDatabaseRefusal } from "@/features/admin/databaseForm";
import { openDropDatabase, openDropSchema, openRenameDatabase } from "@/features/admin/store";
import { writeClipboard } from "@/features/datagrid/clipboard";
import type { Driver, Feature } from "@/lib/ipc";
import type { PaletteMenuEntry } from "./items";

/** Copy Name of the ⌘K list: the palette stays open. */
export function copyName(name: string): void {
  void writeClipboard(name).then(
    () => toast.success(`Copied ${name}`),
    (e: unknown) => toast.error("Could not copy", { description: (e as Error).message }),
  );
}

function refuse(name: string, action: "rename" | "drop") {
  toast.error(`Cannot ${action} the current database`, {
    description: currentDatabaseRefusal(name, action),
  });
}

/**
 * Right-click menu of a database in ⌘K (TablePlus): Open in Current Workspace, Copy Name, then
 * Rename… (PostgreSQL only: MySQL cannot rename a database) and Drop…. The database the
 * connection uses can be neither renamed nor dropped; trying says why.
 */
export function databaseMenu(
  ctx: {
    connectionId: string;
    driver: Driver;
    open: (database: string) => unknown;
    /** What the session offers (every feature when left out). */
    features?: readonly Feature[];
  },
  database: string,
  current: boolean,
): PaletteMenuEntry[] {
  const offers = (f: Feature) => !ctx.features || ctx.features.includes(f);
  const entries: PaletteMenuEntry[] = [
    { label: "Open in Current Workspace", run: () => ctx.open(database) },
    { label: "Copy Name", run: () => copyName(database), keepOpen: true },
  ];
  if (ctx.driver === "postgres" && offers("renameDatabase"))
    entries.push({
      label: "Rename…",
      separated: true,
      run: () =>
        current ? refuse(database, "rename") : openRenameDatabase(ctx.connectionId, database),
    });
  if (ctx.driver !== "sqlite" && offers("dropDatabase"))
    entries.push({
      label: "Drop…",
      destructive: true,
      separated: ctx.driver !== "postgres",
      run: () =>
        current ? refuse(database, "drop") : openDropDatabase(ctx.connectionId, database),
    });
  return entries;
}

/** Right-click menu of a schema in ⌘K: Show in Sidebar, Copy Name, Drop Schema… (PostgreSQL). */
export function schemaMenu(
  ctx: {
    connectionId: string;
    driver: Driver;
    show: (schema: string) => void;
    /** What the session offers (every feature when left out). */
    features?: readonly Feature[];
  },
  schema: string,
): PaletteMenuEntry[] {
  const entries: PaletteMenuEntry[] = [
    { label: "Show in Sidebar", run: () => ctx.show(schema) },
    { label: "Copy Name", run: () => copyName(schema), keepOpen: true },
  ];
  if (ctx.driver === "postgres" && (!ctx.features || ctx.features.includes("dropSchema")))
    entries.push({
      label: "Drop Schema…",
      destructive: true,
      separated: true,
      run: () => openDropSchema(ctx.connectionId, schema),
    });
  return entries;
}
