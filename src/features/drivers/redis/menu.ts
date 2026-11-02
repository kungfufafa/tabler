import { toast } from "sonner";
import { valueText } from "@/lib/values";
import type { RedisEditResult } from "@/lib/ipc/bindings/RedisEditResult";
import type { RedisServerInfo } from "@/lib/ipc/bindings/RedisServerInfo";
import type { RedisSlowEntry } from "@/lib/ipc/bindings/RedisSlowEntry";
import type { ObjectMenuContext, ObjectMenuItem, SidebarItemRef } from "../types";
import { ask, askNewKey } from "./ask";
import { dbOf } from "./keys";

/**
 * A database's items of the sidebar menu, over the session's calls (`drivers/redis/calls.rs`):
 * Server Info and Slow Log (reads, through `ctx.read`), New Key… and Flush Database… (writes,
 * through `ctx.call`: Safe Mode first; off on a read-only connection, where the backend refuses
 * them). Flush shows the command it sends before it runs.
 */

/** The first failed edit of a write call, which reports one result per edit. */
function failure(r: RedisEditResult): string | null {
  return r.results.find((x) => !x.ok)?.error ?? null;
}

/**
 * The FLUSHDB the backend sends (`sqlgen::native::redis::flush_db`): ASYNC from Redis 4, decided
 * like `Server::gates` by `redis_version`; a server that hides it is taken for a current one.
 */
export function flushCommand(db: number, info: RedisServerInfo | null | undefined): string {
  const version = info?.sections
    .flatMap((s) => s.fields)
    .find(([name]) => name === "redis_version")?.[1];
  const major = /\d+/.exec(version ?? "")?.[0];
  const async = major === undefined || Number(major) >= 4;
  return `SELECT ${db}\nFLUSHDB${async ? " ASYNC" : ""}`;
}

/** `serverInfo` as the dialog's text: the server, then every INFO section. */
export function serverInfoText(info: RedisServerInfo): string {
  const head = [
    `${info.product} ${info.version} (${info.mode}), RESP${info.protocol}`,
    `${info.databases} databases${info.readOnly ? ", read-only connection" : ""}`,
  ];
  const sections = info.sections.map(
    (s) => `# ${s.name}\n${s.fields.map(([k, v]) => `${k}: ${v}`).join("\n")}`,
  );
  return [head.join("\n"), ...sections].join("\n\n");
}

/** `slowLog` as the dialog's text, newest first, as SLOWLOG GET lists it. */
export function slowLogText(entries: readonly RedisSlowEntry[]): string {
  if (entries.length === 0) return "The slow log is empty.";
  const n = (v: RedisSlowEntry["id"]) =>
    v.t === "int" || v.t === "float" ? v.v : Number(valueText(v) ?? 0);
  return entries
    .map((e) => {
      const when = new Date(n(e.startedAt) * 1000).toISOString().replace(".000Z", "Z");
      const who = [e.client, e.clientName].filter(Boolean).join(" ");
      return `#${n(e.id)} ${when} ${(n(e.durationUs) / 1000).toFixed(1)} ms${who ? ` ${who}` : ""}\n  ${e.command}`;
    })
    .join("\n");
}

export function redisObjectMenu(item: SidebarItemRef, ctx: ObjectMenuContext): ObjectMenuItem[] {
  const db = dbOf(item.table);
  const readOnly = ctx.info.readOnly;
  const read = ctx.read ?? ctx.call;

  return [
    {
      id: "server-info",
      label: "Server Info",
      run: async () => {
        const info = await read<RedisServerInfo>("serverInfo", null);
        await ask({ title: "Server", command: serverInfoText(info), confirmLabel: "Done" });
      },
    },
    {
      id: "slow-log",
      label: "Slow Log",
      run: async () => {
        const entries = await read<RedisSlowEntry[]>("slowLog", { count: 128 });
        await ask({ title: "Slow log", command: slowLogText(entries), confirmLabel: "Done" });
      },
    },
    {
      id: "new-key",
      label: "New Key…",
      disabled: readOnly,
      run: async () => {
        const args = await askNewKey(db);
        if (!args) return;
        const r = await ctx.call<RedisEditResult>("newKey", args);
        const error = failure(r);
        if (error) {
          toast.error(error);
          return;
        }
        ctx.refresh();
        toast.success(`Created ${args.key.t === "text" ? args.key.v : "the key"} in db${db}`);
      },
    },
    {
      id: "flush",
      label: "Flush Database…",
      destructive: true,
      disabled: readOnly,
      run: async () => {
        // The server's version decides ASYNC, so the dialog shows the command that is sent.
        const info = await read<RedisServerInfo>("serverInfo", null).catch(() => null);
        const go = await ask({
          title: `Flush db${db}?`,
          description: "Every key of this database is deleted. This cannot be undone.",
          command: flushCommand(db, info),
          confirmLabel: "Flush Database",
          destructive: true,
        });
        if (go === null) return;
        const r = await ctx.call<RedisEditResult>("flushDb", { db });
        const error = failure(r);
        if (error) {
          toast.error(error);
          return;
        }
        ctx.refresh();
        toast.success(`db${db} flushed`);
      },
    },
  ];
}
