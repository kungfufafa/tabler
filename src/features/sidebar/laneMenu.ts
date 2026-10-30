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
 * (`ipc.sessionCall`) through Safe Mode first, as writes: the app cannot tell which of a
 * driver's calls only read (the backend refuses a writing call on a read-only connection).
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
  const ctx: ObjectMenuContext = {
    connectionId,
    info,
    caps: capsOf(info),
    call: async <T>(method: string, args?: unknown): Promise<T> => {
      const go = await guardExecution(connectionId, {
        kind: "operation",
        label: method,
        write: true,
      });
      if (!go) throw new Error("Cancelled");
      return ipc.sessionCall<T>(connectionId, method, args ?? null);
    },
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
