import type { ConnectionStatusEvent } from "./bindings/ConnectionStatusEvent";
import type { SqliteSshEvent } from "./bindings/SqliteSshEvent";
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
  /** URLs or files to open are waiting (`ipc.deeplink.takePending`); sent to the focused window. */
  "deeplink:open": null;
  /** SQLite over SSH: changes uploaded, a remote change found (conflict), an upload failed. */
  "sqlite-ssh:status": SqliteSshEvent;
  /** The window that kept the session saved closed: this one does from now on. */
  "window:session-owner": null;
  /** Another window saved the preferences (`changed`: the keys it changed, with their values). */
  "prefs:changed": { from: string; changed: Record<string, unknown> };
  /** Another window set, changed or turned off the passcode. */
  "lock:changed": { from: string; enabled: boolean };
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

/** Identifies this window's renderer in the events it broadcasts (it ignores its own). */
export const RENDERER_ID: string =
  globalThis.crypto?.randomUUID?.() ?? `r-${Date.now()}-${Math.random().toString(36).slice(2)}`;

/**
 * Sends an event to every window (this one included: compare `from` with `RENDERER_ID`). Tauri
 * `emit`, or the mock emitter in the browser.
 */
export async function broadcastEvent<K extends keyof EventMap>(
  name: K,
  payload: EventMap[K],
): Promise<void> {
  if (isMock) {
    const { mockEmit } = await import("./mock/transport");
    mockEmit(name, payload);
    return;
  }
  const { emit } = await import("@tauri-apps/api/event");
  await emit(name, payload);
}

/**
 * Subscribes to a backend event sent to this window only (`emit_to` its label: native menu
 * actions go to the focused window). A global `listenEvent` would also hear the other windows'.
 */
export async function listenWindowEvent<K extends keyof EventMap>(
  name: K,
  handler: (payload: EventMap[K]) => void,
): Promise<Unlisten> {
  if (isMock) return listenEvent(name, handler);
  const { getCurrentWebviewWindow } = await import("@tauri-apps/api/webviewWindow");
  return getCurrentWebviewWindow().listen<EventMap[K]>(name, (e) => handler(e.payload));
}
