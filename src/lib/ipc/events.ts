import type { ConnectionStatusEvent } from "./bindings/ConnectionStatusEvent";
import type { TunnelStatusEvent } from "./bindings/TunnelStatusEvent";
import { isMock } from "./transport";

export type Unlisten = () => void;

type EventMap = {
  "connection:status": ConnectionStatusEvent;
  "tunnel:status": TunnelStatusEvent;
};

/** Subscribes to a global backend event (Tauri `listen`, or the mock emitter in the browser). */
export async function listenEvent<K extends keyof EventMap>(
  name: K,
  handler: (payload: EventMap[K]) => void,
): Promise<Unlisten> {
  if (isMock) {
    const { mockListen } = await import("./mock/transport");
    return mockListen(name, handler as (payload: unknown) => void);
  }
  const { listen } = await import("@tauri-apps/api/event");
  const unlisten = await listen<EventMap[K]>(name, (e) => handler(e.payload));
  return unlisten;
}
