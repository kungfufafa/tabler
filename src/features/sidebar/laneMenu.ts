import { toast } from "sonner";
import type { DriverUi, ObjectMenuContext, SidebarItemRef } from "@/features/drivers/types";
import type { MenuEntry } from "@/features/objects/menu";
import { guardExecution } from "@/features/safety/guard";
import { capsOf } from "@/lib/caps";
import { errorWithHint } from "@/lib/errorText";
import { ipc, type ConnectionInfo } from "@/lib/ipc";

/**
 * A lane driver's items of a sidebar item's menu (`DriverUi.objectMenu`: Collection Stats, Flush
 * Index…), after the app's own, as menu entries. Their calls go to the session
 * (`ipc.sessionCall`) through Safe Mode first: `ctx.call` as a write, `ctx.read` as a read, as
 * the lane knows its driver's `call_kind` (the backend refuses a writing call on a read-only
 * connection either way).
 */
export function laneMenuEntries(
  ui: DriverUi | null | undefined,
  item: SidebarItemRef,
  info: ConnectionInfo,
  refresh: () => void,
): MenuEntry[] {
  const build = ui?.objectMenu;
  if (!build) return [];
  const { connectionId } = info;
  const send = async <T>(method: string, args: unknown, write: boolean): Promise<T> => {
    const go = await guardExecution(connectionId, { kind: "operation", label: method, write });
    if (!go) throw new Error("Cancelled");
    return ipc.sessionCall<T>(connectionId, method, args ?? null);
  };
  const ctx: ObjectMenuContext = {
    connectionId,
    info,
    caps: capsOf(info),
    call: (method, args) => send(method, args, true),
    read: (method, args) => send(method, args, false),
    refresh,
  };
  const items = build(item, ctx);
  if (items.length === 0) return [];
  return [
    { kind: "separator", id: "lane-sep" },
    ...items.map((i): MenuEntry => ({
      kind: "item",
      id: `lane-${i.id}`,
      label: i.label,
      destructive: i.destructive,
      disabled: i.disabled,
      onSelect: () =>
        void Promise.resolve()
          .then(i.run)
          .catch((e: unknown) => {
            if (e instanceof Error && e.message === "Cancelled") return;
            toast.error(`${i.label.replace(/…$/, "")} failed`, { description: errorWithHint(e) });
          }),
    })),
  ];
}
