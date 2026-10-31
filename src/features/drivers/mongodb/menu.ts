import { toast } from "sonner";
import { ipc } from "@/lib/ipc";
import type { ObjectMenuContext, ObjectMenuItem, SidebarItemRef } from "../types";
import { ask } from "./ask";
import { Lexer } from "./shell";

/**
 * A collection's items of the sidebar menu, over the session's calls (`drivers/mongodb/calls.rs`):
 * Collection Stats (a read, sent as is), and the writes, which go through `ctx.call` (Safe Mode
 * first; refused on a read-only connection, where they are off): Create Index…, Drop Index…,
 * Edit Validator…, Rename…, Truncate… and Drop…. The destructive ones show the command the
 * backend will run (its `preview` call) before they run it.
 */

interface Stats {
  count?: number;
  storageStats?: { size?: number; storageSize?: number; totalIndexSize?: number; count?: number };
}

const bytes = (n: number | undefined): string => {
  if (n === undefined) return "?";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u += 1;
  }
  return `${u === 0 ? v : v.toFixed(1)} ${units[u]}`;
};

/** `{keys}, {options}` as the two texts the `createIndex` call takes. */
export function splitIndexText(text: string): { keys: string; options: string } {
  const lx = new Lexer(text);
  try {
    lx.value();
  } catch {
    return { keys: text, options: "" };
  }
  const keys = text.slice(0, lx.pos).trim();
  const rest = text.slice(lx.pos).trim().replace(/^,/, "").trim();
  return { keys, options: rest };
}

export function mongoObjectMenu(item: SidebarItemRef, ctx: ObjectMenuContext): ObjectMenuItem[] {
  const database = item.database ?? item.table.schema ?? ctx.info.currentDatabase ?? "";
  const collection = item.table.name;
  const at = { database, collection };
  const readOnly = ctx.info.readOnly;
  const label = item.kind === "view" ? "View" : "Collection";

  const stats: ObjectMenuItem = {
    id: "stats",
    label: "Collection Stats",
    disabled: item.kind === "view",
    run: async () => {
      // A read: sent as is, so it works on a read-only connection too.
      const s = await ipc.sessionCall<Stats | null>(ctx.connectionId, "collectionStats", at);
      const st = s?.storageStats;
      toast.info(`${collection}: ${(st?.count ?? s?.count ?? 0).toLocaleString()} documents`, {
        description: `Data ${bytes(st?.size)}, on disk ${bytes(st?.storageSize)}, indexes ${bytes(st?.totalIndexSize)}`,
      });
    },
  };
  if (item.kind === "systemView") return [stats];

  /** A write: Safe Mode, the call, a refreshed sidebar and a word of what was done. */
  const write = async (method: string, args: object, done: string) => {
    await ctx.call(method, { ...at, ...args });
    ctx.refresh();
    toast.success(done);
  };
  /** A destructive write, confirmed with the command it runs. */
  const confirmed = async (method: string, title: string, done: string) => {
    const preview = await ipc.sessionCall<{ command: string }>(ctx.connectionId, "preview", {
      method,
      args: at,
    });
    const go = await ask({
      title,
      description: `This cannot be undone. Tabler will run on ${database}:`,
      command: preview.command,
      confirmLabel: title.replace(/…$/, "").replace(/ “.*”$/, ""),
      destructive: true,
    });
    if (go !== null) await write(method, {}, done);
  };

  const items: ObjectMenuItem[] = [];
  if (item.kind === "table")
    items.push(
      stats,
      {
        id: "create-index",
        label: "Create Index…",
        disabled: readOnly,
        run: async () => {
          const text = await ask({
            title: `New index on ${collection}`,
            description: "The keys, then any options, in shell syntax.",
            input: { label: "Index", placeholder: "{email: 1}, {unique: true}" },
            confirmLabel: "Create",
          });
          if (text === null) return;
          const { keys, options } = splitIndexText(text);
          await write("createIndex", { keys, options }, `Index created on ${collection}`);
        },
      },
      {
        id: "drop-index",
        label: "Drop Index…",
        destructive: true,
        disabled: readOnly,
        run: async () => {
          const name = await ask({
            title: `Drop an index of ${collection}`,
            input: { label: "Index name", placeholder: "email_1" },
            confirmLabel: "Drop Index",
            destructive: true,
          });
          if (name !== null) await write("dropIndex", { name }, `Index ${name} dropped`);
        },
      },
      {
        id: "validator",
        label: "Edit Validator…",
        disabled: readOnly,
        run: async () => {
          const meta = await ipc.schema.table(ctx.connectionId, item.table);
          const current = meta.checks.find((c) => c.name === "validator")?.expression ?? "";
          const validator = await ask({
            title: `Validator of ${collection}`,
            description: "A query document, such as {$jsonSchema: {…}}; {} removes it.",
            input: { label: "Validator", initial: current || "{}", multiline: true },
            confirmLabel: "Save",
          });
          if (validator !== null)
            await write("setValidator", { validator }, `Validator of ${collection} saved`);
        },
      },
      {
        id: "rename",
        label: "Rename…",
        disabled: readOnly,
        run: async () => {
          const to = await ask({
            title: `Rename ${collection}`,
            input: { label: "New name", initial: collection },
            confirmLabel: "Rename",
          });
          if (to !== null && to !== collection)
            await write("renameCollection", { to }, `${collection} renamed to ${to}`);
        },
      },
      {
        id: "truncate",
        label: "Truncate…",
        destructive: true,
        disabled: readOnly,
        run: () =>
          confirmed("truncateCollection", `Truncate “${collection}”`, `${collection} emptied`),
      },
    );
  items.push({
    id: "drop",
    label: `Drop ${label}…`,
    destructive: true,
    disabled: readOnly,
    run: () =>
      confirmed("dropCollection", `Drop ${label} “${collection}”`, `${collection} dropped`),
  });
  return items;
}
