import { create } from "zustand";
import { ipc, type AssistantKeyStatus, type AssistantProvider } from "@/lib/ipc";

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

/** Stores (or with null removes) a provider's key, then refreshes the status. */
export async function saveKey(provider: AssistantProvider, key: string | null): Promise<void> {
  await ipc.assistant.setKey(provider, key);
  await refreshKeyStatus();
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
