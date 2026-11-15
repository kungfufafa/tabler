import { create } from "zustand";
import { ipc, type AssistantKeyStatus, type AssistantProvider } from "@/lib/ipc";
import { broadcastEvent, listenEvent, RENDERER_ID } from "@/lib/ipc/events";

/** Which providers have an API key in the Keychain (never the keys themselves). */

interface KeysState {
  status: AssistantKeyStatus | null;
}

export const useAssistantKeys = create<KeysState>()(() => ({ status: null }));

let inflight: Promise<void> | null = null;

/** Reads the key status (concurrent calls share one request). */
export function refreshKeyStatus(): Promise<void> {
  inflight ??= ipc.assistant
    .keyStatus()
    .then((status) => useAssistantKeys.setState({ status }))
    .catch((err: unknown) => console.warn("[assistant] key status", err))
    .finally(() => {
      inflight = null;
    });
  return inflight;
}

/**
 * Stores (or with null removes) a provider's key, then refreshes the status, here and in the
 * other windows (Settings is a window of its own: the Assistant panel of a workspace window must
 * see the key at once).
 */
export async function saveKey(provider: AssistantProvider, key: string | null): Promise<void> {
  await ipc.assistant.setKey(provider, key);
  await refreshKeyStatus();
  void broadcastEvent("assistant:keys-changed", { from: RENDERER_ID }).catch((err: unknown) =>
    console.warn("[assistant] could not tell the other windows", err),
  );
}

/** Follows the keys another window stored or removed. Returns the uninstall function. */
export function installKeySync(): () => void {
  let gone = false;
  let unlisten: (() => void) | null = null;
  void listenEvent("assistant:keys-changed", ({ from }) => {
    if (from !== RENDERER_ID) void refreshKeyStatus();
  }).then((u) => {
    if (gone) u();
    else unlisten = u;
  });
  return () => {
    gone = true;
    unlisten?.();
  };
}

/** Whether `provider` has a key (Ollama never needs one); null while unknown. */
export function hasKey(
  status: AssistantKeyStatus | null,
  provider: AssistantProvider,
): boolean | null {
  if (provider === "ollama") return true;
  if (!status) return null;
  return provider === "anthropic" ? status.anthropic : status.openai;
}
