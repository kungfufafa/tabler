import { toast } from "sonner";
import { ipc } from "@/lib/ipc";
import type { ObjectMenuContext, ObjectMenuItem, SidebarItemRef } from "../types";
import { ask } from "./ask";
import { parseStrict } from "./json";
import { consoleText, type EsRequest } from "./request";

/**
 * An index's items of the sidebar menu, over the session's calls (`drivers/elasticsearch/session.rs`
 * `call`): Index Settings (a read, sent as is), and the writes, which go through `ctx.call` (Safe
 * Mode first; refused on a read-only connection, where they are off): Refresh, Flush, Force Merge,
 * Clear Cache, Open, Close, New Index…, Truncate… and Delete Index…. The destructive ones show
 * the request the backend sends (its builders' text, `sqlgen/native/elastic.rs`) before it runs.
 */

/** What a write call answers: the request it sent and the cluster's reply. */
interface WriteReply {
  request: string;
  reply: string;
}

/** The index actions of the `indexAction` call, as the backend names them. */
const ACTIONS: readonly (readonly [string, string, string])[] = [
  ["refresh", "Refresh", "refreshed"],
  ["flush", "Flush", "flushed"],
  ["forcemerge", "Force Merge", "merged"],
  ["clear_cache", "Clear Cache", "caches cleared"],
  ["open", "Open Index", "opened"],
  ["close", "Close Index", "closed"],
];

/** The requests the destructive calls send (`truncate`, `delete_index`). */
export function truncateRequest(index: string): EsRequest {
  return {
    method: "POST",
    segments: [index, "_delete_by_query"],
    query: [
      ["conflicts", "proceed"],
      ["refresh", "true"],
    ],
    body: {
      kind: "json",
      value: { k: "obj", v: [["query", { k: "obj", v: [["match_all", { k: "obj", v: [] }]] }]] },
    },
  };
}

export function deleteRequest(index: string): EsRequest {
  return { method: "DELETE", segments: [index], query: [], body: null };
}

/** Whether a name is one index the operations take (`check_index`: no pattern, list or remote). */
export const singleIndex = (name: string): boolean => !/[*,:]/.test(name);

export function esObjectMenu(item: SidebarItemRef, ctx: ObjectMenuContext): ObjectMenuItem[] {
  const index = item.table.name;
  const readOnly = ctx.info.readOnly;

  const settings: ObjectMenuItem = {
    id: "settings",
    label: "Index Settings",
    run: async () => {
      // A read: sent as is, so it works on a read-only connection too.
      const text = await ipc.sessionCall<string>(ctx.connectionId, "indexSettings", { index });
      await ask({
        title: `Settings of ${index}`,
        command: text,
        confirmLabel: "Done",
      });
    },
  };
  if (item.kind === "systemView") return [settings];

  /** A write: Safe Mode, the call, a refreshed sidebar and a word of what was done. */
  const write = async (method: string, args: object, done: string) => {
    await ctx.call<WriteReply>(method, args);
    ctx.refresh();
    toast.success(done);
  };
  /** A destructive write, confirmed with the request it sends. */
  const confirmed = async (method: string, title: string, req: EsRequest, done: string) => {
    const go = await ask({
      title,
      description: "This cannot be undone. Tabler will send:",
      command: consoleText(req),
      confirmLabel: title.replace(/ “.*”$/, ""),
      destructive: true,
    });
    if (go !== null) await write(method, { index }, done);
  };

  const items: ObjectMenuItem[] = [settings];
  // Data streams and aliases take the reads and a refresh; the rest needs one index.
  const one = item.kind === "table" && singleIndex(index);
  for (const [action, label, done] of ACTIONS) {
    if (!one && action !== "refresh") continue;
    items.push({
      id: action,
      label,
      disabled: readOnly,
      run: () => write("indexAction", { index, action }, `${index} ${done}`),
    });
  }
  items.push({
    id: "create",
    label: "New Index…",
    disabled: readOnly,
    run: async () => {
      const name = await ask({
        title: "New index",
        input: { label: "Name", placeholder: "logs-2026" },
        confirmLabel: "Next",
      });
      if (name === null) return;
      const body = await ask({
        title: `Settings and mappings of ${name}`,
        description: "The body of PUT /<index> (empty: the cluster's defaults).",
        input: {
          label: "Body",
          initial: '{\n  "settings": {},\n  "mappings": {"properties": {}}\n}',
          multiline: true,
        },
        confirmLabel: "Create",
      });
      if (body === null) return;
      // Strict JSON, as the backend reads it: say so here rather than after Safe Mode.
      if (body.trim() !== "") parseStrict(body);
      await write("createIndex", { name, body }, `Index ${name} created`);
    },
  });
  if (one)
    items.push(
      {
        id: "truncate",
        label: "Truncate…",
        destructive: true,
        disabled: readOnly,
        run: () =>
          confirmed("truncate", `Truncate “${index}”`, truncateRequest(index), `${index} emptied`),
      },
      {
        id: "delete",
        label: "Delete Index…",
        destructive: true,
        disabled: readOnly,
        run: () =>
          confirmed(
            "deleteIndex",
            `Delete Index “${index}”`,
            deleteRequest(index),
            `${index} deleted`,
          ),
      },
    );
  return items;
}
