import { isMock } from "@/lib/ipc";

/** The native Plugins menu's command items reach the focused window as this event. */
export const PLUGIN_RUN_EVENT = "plugins:run";

/** Subscribes to this window's `plugins:run` events (payload: the command's run id). */
export async function listenPluginRuns(handler: (runId: string) => void): Promise<() => void> {
  const onPayload = (payload: unknown) => {
    if (typeof payload === "string") handler(payload);
  };
  if (isMock) {
    const { mockListen } = await import("@/lib/ipc/mock/transport");
    return mockListen(PLUGIN_RUN_EVENT, onPayload);
  }
  const { getCurrentWebviewWindow } = await import("@tauri-apps/api/webviewWindow");
  return getCurrentWebviewWindow().listen<unknown>(PLUGIN_RUN_EVENT, (e) => onPayload(e.payload));
}
