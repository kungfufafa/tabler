import type { ConnectionStatusEvent } from "./bindings/ConnectionStatusEvent";
import type { SshPromptDone } from "./bindings/SshPromptDone";
import type { SshPromptRequest } from "./bindings/SshPromptRequest";
import type { TunnelStatusEvent } from "./bindings/TunnelStatusEvent";
import { isMock } from "./transport";

export type Unlisten = () => void;

type EventMap = {
  "connection:status": ConnectionStatusEvent;
  "tunnel:status": TunnelStatusEvent;
  /** An SSH server asks a keyboard-interactive (2FA) question; answer with `answerSshPrompt`. */
  "ssh:prompt": SshPromptRequest;
  /** The backend no longer waits for the answer to that question (drop its dialog). */
  "ssh:prompt-done": SshPromptDone;
  /** A native menu item was chosen; the payload is its action id (see `useNativeMenu`). */
  "menu:action": string;
  /**
   * Quit was asked for (the app menu's Quit item, Dock Quit / logout / shutdown with unsaved work,
   * or an exit request while the window is open). The renderer acknowledges it
   * (`ipc.app.setUnsaved`) and answers with `ipc.app.quit()` once nothing unsaved is left.
   */
  "app:quit-requested": null;
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
