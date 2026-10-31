import { toast } from "sonner";
import { ipc, type TableMeta } from "@/lib/ipc";
import type { ObjectMenuContext, ObjectMenuItem, SidebarItemRef } from "../types";
import type { IndexChoice, TableInfo } from "./info";
import { openIndexBrowser, openTableInfo } from "./dialogs";

/**
 * A table's items of the sidebar menu, over the session's calls (`drivers/dynamodb/session.rs`),
 * both reads: Table Info… (`tableInfo`: billing, counts, stream, TTL, point-in-time recovery,
 * protection) and Browse Index… (`browseIndex`: pages of a global or local secondary index).
 * Reads are sent as they are (no Safe Mode prompt), so they work on a read-only connection.
 */

/** The secondary indexes of a table's structure. */
export function indexChoices(meta: TableMeta): IndexChoice[] {
  return meta.indexes
    .filter((i) => i.method === "GSI" || i.method === "LSI")
    .map((i) => ({ name: i.name, global: i.method === "GSI", keys: i.columns }));
}

export function dynamoObjectMenu(item: SidebarItemRef, ctx: ObjectMenuContext): ObjectMenuItem[] {
  if (item.kind !== "table") return [];
  const { table } = item;
  return [
    {
      id: "table-info",
      label: "Table Info…",
      run: async () => {
        const info = await ipc.sessionCall<TableInfo>(ctx.connectionId, "tableInfo", {
          table: table.name,
        });
        openTableInfo(table.name, info);
      },
    },
    {
      id: "browse-index",
      label: "Browse Index…",
      run: async () => {
        const indexes = indexChoices(await ipc.schema.table(ctx.connectionId, table));
        if (indexes.length === 0) {
          toast.info(`${table.name} has no secondary index`);
          return;
        }
        openIndexBrowser(ctx.connectionId, table, indexes);
      },
    },
  ];
}
