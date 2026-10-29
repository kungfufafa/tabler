import { toast } from "sonner";
import { create } from "zustand";
import { hasSingleHost } from "@/features/export/singleHost";
import type { ConnectionTag, Driver, SafeModeLevel } from "@/lib/ipc";

/** The connection a prompt is about, as the dialog shows it. */
export interface PromptTarget {
  profileId: string;
  name: string;
  driver: Driver;
  tag: ConnectionTag | null;
  level: SafeModeLevel;
}

interface PromptBase {
  id: number;
  target: PromptTarget;
  title: string;
  /** Extra line under the title (operations, level changes). */
  description: string | null;
  /** The statements about to be sent; empty for operations. */
  statements: string[];
  /** Label of the confirming button: Send, Commit, Continue. */
  confirmLabel: string;
}

/** Alert Mode: a confirmation. */
export interface AlertPrompt extends PromptBase {
  kind: "alert";
}

/**
 * What a Safe Mode prompt asks for: the connection's DB password (checked by the backend), the
 * app passcode for a connection whose secret is a token (checked by the backend too), or, when
 * neither can be checked, the connection's name (see `passwordSecret` in `guard.ts`).
 */
export type SafeSecret = "password" | "passcode" | "name";

/** Safe Mode: asks for `secret`. */
export interface PasswordPrompt extends PromptBase {
  kind: "password";
  secret: SafeSecret;
}

export type SafetyPrompt = AlertPrompt | PasswordPrompt;
export type SafetyPromptRequest = Omit<AlertPrompt, "id"> | Omit<PasswordPrompt, "id">;

/** Prompts waiting for an answer; the first one is on screen (see `SafetyHost`). */
export const useSafetyPromptStore = create<{ queue: SafetyPrompt[] }>()(() => ({ queue: [] }));

/** Name of the single-host slot `SafetyHost` registers. */
export const SAFETY_HOST = "safe-mode-prompt";

let nextId = 0;
const resolvers = new Map<number, (ok: boolean) => void>();

/**
 * Queues a safe-mode prompt (one dialog at a time, in call order) and resolves with the answer:
 * true = go ahead. Without a mounted `SafetyHost` nobody could answer, so it resolves false.
 */
export function requestSafetyPrompt(request: SafetyPromptRequest): Promise<boolean> {
  if (!hasSingleHost(SAFETY_HOST)) {
    toast.error("Safe mode could not ask for confirmation", {
      id: "safe-mode-no-host",
      description: "Nothing was sent to the server.",
    });
    return Promise.resolve(false);
  }
  const id = ++nextId;
  return new Promise((resolve) => {
    resolvers.set(id, resolve);
    useSafetyPromptStore.setState((s) => ({
      queue: [...s.queue, { ...request, id } as SafetyPrompt],
    }));
  });
}

/** Answers prompt `id` (idempotent: later answers for the same prompt are ignored). */
export function answerSafetyPrompt(id: number, ok: boolean): void {
  const resolve = resolvers.get(id);
  resolvers.delete(id);
  useSafetyPromptStore.setState((s) =>
    s.queue.some((p) => p.id === id) ? { queue: s.queue.filter((p) => p.id !== id) } : s,
  );
  resolve?.(ok);
}

/** Declines every pending prompt (their host went away): nothing is sent. */
export function abandonSafetyPrompts(): void {
  const pending = [...resolvers.values()];
  resolvers.clear();
  useSafetyPromptStore.setState((s) => (s.queue.length > 0 ? { queue: [] } : s));
  pending.forEach((resolve) => resolve(false));
}
