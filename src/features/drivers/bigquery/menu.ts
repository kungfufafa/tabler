import { toast } from "sonner";
import { ipc } from "@/lib/ipc";
import type { ObjectMenuContext, ObjectMenuItem, SidebarItemRef } from "../types";

/**
 * A table's item of the sidebar menu, over the session's one call (`dryRun`,
 * `drivers/bigquery/session.rs`): Estimate Full Scan…, what reading the whole table would process
 * and bill. A dry run starts no job and bills nothing, and the backend calls it a read (it runs
 * nothing, whatever the statement), so it is sent as it is: through `ObjectMenuContext.call` Safe
 * Mode would ask for the passcode and a read-only connection would refuse it, where an estimate
 * matters most.
 */

/** What `dryRun` answers: the bytes as BigQuery's int64 string, and the result's columns. */
export interface DryRun {
  bytesProcessed: string | null;
  columns: { name: string; dataType: string }[];
}

/** `1.5 MB`: bytes as the editor's cost notice writes them (`session::bytes_text`). */
export function bytesText(n: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let unit = 0;
  while (v >= 1024 && unit + 1 < units.length) {
    v /= 1024;
    unit += 1;
  }
  return unit === 0 ? `${n} B` : `${v.toFixed(1)} ${units[unit]}`;
}

/**
 * `dryRun`'s arguments for a full scan of the item: the table and its project, from which the
 * backend builds the statement (`dialect::full_scan_query`), so no SQL is written here.
 */
export function fullScanArgs(item: SidebarItemRef): {
  table: { schema: string | null; name: string };
  project?: string;
} {
  const table = { schema: item.table.schema, name: item.table.name };
  return item.database ? { table, project: item.database } : { table };
}

/** The toast's words for a dry run's answer. */
export function estimateText(name: string, dry: DryRun): string {
  const bytes = dry.bytesProcessed === null ? null : Number(dry.bytesProcessed);
  if (bytes === null || !Number.isFinite(bytes))
    return `BigQuery did not say what reading ${name} would process`;
  return bytes === 0
    ? `Reading all of ${name} would process nothing (metadata or cached)`
    : `Reading all of ${name} would process ${bytesText(bytes)} (${dry.columns.length} columns)`;
}

export function bigqueryObjectMenu(item: SidebarItemRef, ctx: ObjectMenuContext): ObjectMenuItem[] {
  if (item.kind !== "table" && item.kind !== "view" && item.kind !== "materializedView") return [];
  return [
    {
      id: "estimate-scan",
      label: "Estimate Full Scan…",
      run: async () => {
        const dry = await ipc.sessionCall<DryRun>(ctx.connectionId, "dryRun", fullScanArgs(item));
        toast.info(estimateText(item.table.name, dry), {
          description: "A dry run: no job ran and nothing was billed",
        });
      },
    },
  ];
}
