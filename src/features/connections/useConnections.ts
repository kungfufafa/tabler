import { useMutation, useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useCallback } from "react";
import { toast } from "sonner";
import { create } from "zustand";
import { hasSingleHost } from "@/features/export/singleHost";
import { ipc, toIpcError, type ConnectionProfile, type ProfileInput } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { closeConnectionTabs } from "./closeGuard";

export const profileKeys = {
  all: ["profiles"] as const,
  secret: (id: string) => ["profiles", id, "secret"] as const,
};

export function useProfiles() {
  return useQuery({ queryKey: profileKeys.all, queryFn: ipc.profiles.list, staleTime: 60_000 });
}

export function useSecretStatus(id: string | null) {
  return useQuery({
    queryKey: profileKeys.secret(id ?? ""),
    queryFn: () => ipc.profiles.secretStatus(id!),
    enabled: id !== null,
  });
}

export function useSaveProfile() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (input: ProfileInput) => ipc.profiles.save(input),
    onSuccess: (profile) => {
      void qc.invalidateQueries({ queryKey: profileKeys.all });
      void qc.invalidateQueries({ queryKey: profileKeys.secret(profile.id) });
    },
    onError: (e) => toast.error("Could not save connection", { description: e.message }),
  });
}

export function useDeleteProfile() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => ipc.profiles.delete(id),
    onSuccess: () => void qc.invalidateQueries({ queryKey: profileKeys.all }),
    onError: (e) => toast.error("Could not delete connection", { description: e.message }),
  });
}

export function useDuplicateProfile() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => ipc.profiles.duplicate(id),
    onSuccess: () => void qc.invalidateQueries({ queryKey: profileKeys.all }),
    onError: (e) => toast.error("Could not duplicate connection", { description: e.message }),
  });
}

// ---- SSH host keys ---------------------------------------------------------------------------

/** `payload.data` of SSH_HOST_KEY_UNKNOWN / SSH_HOST_KEY_CHANGED errors. */
export interface HostKeyInfo {
  host: string;
  port: number;
  /** OpenSSH algorithm name, e.g. `ssh-ed25519`. */
  keyType: string;
  /** `SHA256:<base64>`. */
  fingerprint: string;
  /** True when the server presents a different key than the one trusted before. */
  changed: boolean;
}

/** The host key a connect / test was refused over, or null for any other error. */
export function hostKeyFromError(err: unknown): HostKeyInfo | null {
  const e = toIpcError(err);
  if (e.code !== "SSH_HOST_KEY_UNKNOWN" && e.code !== "SSH_HOST_KEY_CHANGED") return null;
  const d = (
    typeof e.payload.data === "object" && e.payload.data !== null ? e.payload.data : {}
  ) as Partial<Record<keyof HostKeyInfo, unknown>>;
  return {
    host: typeof d.host === "string" ? d.host : "unknown host",
    port: typeof d.port === "number" ? d.port : 22,
    keyType: typeof d.keyType === "string" ? d.keyType : "",
    fingerprint: typeof d.fingerprint === "string" ? d.fingerprint : "",
    changed: e.code === "SSH_HOST_KEY_CHANGED",
  };
}

/** `ssh-ed25519` → `ED25519`, `ecdsa-sha2-nistp256` → `ECDSA`, `rsa-sha2-512` → `RSA`. */
export function keyTypeLabel(keyType: string): string {
  const k = keyType.toLowerCase();
  if (k.includes("ed25519")) return "ED25519";
  if (k.includes("ed448")) return "ED448";
  if (k.startsWith("ecdsa") || k.includes("nistp")) return "ECDSA";
  if (k.includes("rsa")) return "RSA";
  if (k.includes("dss") || k.includes("dsa")) return "DSA";
  return keyType ? keyType.toUpperCase() : "Host";
}

export type HostKeyPurpose = "connect" | "test";

export interface HostKeyPromptRequest extends HostKeyInfo {
  id: number;
  profileId: string;
  purpose: HostKeyPurpose;
  /** Connection name, for context ("Connecting to …"). */
  label: string | null;
}

export interface HostKeyDecision {
  /** Also append the key to ~/.ssh/known_hosts. */
  persist: boolean;
}

/**
 * How a prompt ended: trusted (`HostKeyDecision`), declined (null), or "abandoned" when the prompt
 * went away before anyone answered it (its host unmounted).
 */
export type HostKeyAnswer = HostKeyDecision | null | "abandoned";

/** Prompts waiting for an answer; the first one is on screen (see `HostKeyPromptHost`). */
export const useHostKeyPromptStore = create<{ queue: HostKeyPromptRequest[] }>()(() => ({
  queue: [],
}));

/** Name of the single-host slot `HostKeyPromptHost` registers. */
export const HOST_KEY_PROMPT_HOST = "ssh-host-key-prompt";

let nextPromptId = 0;
const resolvers = new Map<number, (answer: HostKeyAnswer) => void>();

/**
 * Shows the host-key prompt and resolves with the user's answer (null = cancelled / closed,
 * "abandoned" = the prompt host went away first). Returns null right away when no prompt host is
 * mounted, so callers fall back to the plain error.
 */
export function requestHostKeyDecision(
  req: Omit<HostKeyPromptRequest, "id">,
): Promise<HostKeyAnswer> | null {
  if (!hasSingleHost(HOST_KEY_PROMPT_HOST)) return null;
  const id = ++nextPromptId;
  return new Promise((resolve) => {
    resolvers.set(id, resolve);
    useHostKeyPromptStore.setState((s) => ({ queue: [...s.queue, { ...req, id }] }));
  });
}

/** Answers prompt `id` (idempotent: later answers for the same prompt are ignored). */
export function answerHostKeyPrompt(id: number, decision: HostKeyDecision | null): void {
  const resolve = resolvers.get(id);
  resolvers.delete(id);
  useHostKeyPromptStore.setState((s) =>
    s.queue.some((r) => r.id === id) ? { queue: s.queue.filter((r) => r.id !== id) } : s,
  );
  resolve?.(decision);
}

/**
 * Ends every pending prompt as "abandoned", so the connects / tests waiting on them report their
 * original error instead of hanging. Called when the last prompt host unmounts.
 */
export function abandonHostKeyPrompts(): void {
  const pending = [...resolvers.values()];
  resolvers.clear();
  useHostKeyPromptStore.setState((s) => (s.queue.length > 0 ? { queue: [] } : s));
  pending.forEach((resolve) => resolve("abandoned"));
}

const declined = new WeakSet<object>();

/** True for a host-key error the user already saw in the prompt and did not trust: no extra toast. */
export function hostKeyDeclined(err: unknown): boolean {
  return typeof err === "object" && err !== null && declined.has(err);
}

/**
 * Runs `run` (a connect or a connection test); when the SSH server's key is unknown, asks the user
 * to trust it, records the answer with `accept_host_key` (the backend parks pending keys by profile
 * id) and retries. A changed key only shows the warning. Rethrows the original error when the user
 * declines (`hostKeyDeclined` is then true for it), or when no prompt can be shown or it goes away
 * unanswered.
 */
export async function withHostKeyTrust<T>(
  profileId: string,
  purpose: HostKeyPurpose,
  label: string | null,
  run: () => Promise<T>,
): Promise<T> {
  let accepted: string | null = null;
  for (;;) {
    try {
      return await run();
    } catch (err) {
      const key = hostKeyFromError(err);
      // Refused again for the key just trusted: report it instead of asking forever.
      if (!key || key.fingerprint === accepted) throw err;
      const asking = requestHostKeyDecision({ ...key, profileId, purpose, label });
      if (!asking) throw err;
      const decision = await asking;
      // Nobody answered (the prompt was unmounted): report the error as if there were no prompt.
      if (decision === "abandoned") throw err;
      if (!decision || key.changed) {
        if (typeof err === "object" && err !== null) declined.add(err);
        throw err;
      }
      await ipc.connections.acceptHostKey(profileId, key.fingerprint, decision.persist);
      accepted = key.fingerprint;
    }
  }
}

function profileName(qc: QueryClient, profileId: string): string | null {
  return (
    qc
      .getQueryData<Pick<ConnectionProfile, "id" | "name">[]>(profileKeys.all)
      ?.find((p) => p.id === profileId)?.name ?? null
  );
}

/** Tests an unsaved profile; SSH host keys are confirmed under the form's (unsaved) profile id. */
export function useTestConnection() {
  return useMutation({
    mutationFn: (input: ProfileInput) =>
      withHostKeyTrust(input.profile.id, "test", input.profile.name.trim() || null, () =>
        ipc.profiles.test(input),
      ),
  });
}

/** Opens a connection, registers it in the store and switches to the workspace. */
export function useConnect() {
  const qc = useQueryClient();
  const add = useConnectionsStore((s) => s.add);
  const setView = useUiStore((s) => s.setView);
  return useMutation({
    mutationFn: (profileId: string) =>
      withHostKeyTrust(profileId, "connect", profileName(qc, profileId), () =>
        ipc.connections.connect(profileId),
      ),
    onSuccess: (info) => {
      add(info);
      setView("workspace");
      void qc.invalidateQueries({ queryKey: profileKeys.all });
    },
    onError: (e, profileId) => {
      // The host-key prompt already told the user why the connection was not opened.
      if (hostKeyDeclined(e)) return;
      const name = profileName(qc, profileId);
      toast.error(`Could not connect${name ? ` to ${name}` : ""}`, { description: e.message });
    },
  });
}

/**
 * Raw disconnect: drops the connection's tabs without asking. UI entry points go through
 * `useGuardedDisconnect` so unsaved work gets the "Discard changes?" prompt first.
 */
export function useDisconnect() {
  const remove = useConnectionsStore((s) => s.remove);
  const closeForConnection = useTabsStore((s) => s.closeForConnection);
  const setView = useUiStore((s) => s.setView);
  return useMutation({
    mutationFn: (connectionId: string) => ipc.connections.disconnect(connectionId),
    onSettled: (_data, _err, connectionId) => {
      closeForConnection(connectionId);
      remove(connectionId);
      if (useConnectionsStore.getState().order.length === 0) setView("launcher");
    },
  });
}

/**
 * Disconnect behind the unsaved-changes guard: the connection's tabs are closed first (asking when
 * any hold staged edits or an unsaved query). Resolves false when the user kept them.
 */
export function useGuardedDisconnect(): (connectionId: string) => Promise<boolean> {
  const { mutate } = useDisconnect();
  return useCallback(
    (connectionId: string) => closeConnectionTabs(connectionId, () => mutate(connectionId)),
    [mutate],
  );
}
