import { create } from "zustand";
import { hasSingleHost } from "@/features/export/singleHost";

/**
 * The password prompt of "Ask every time" connections: Connect and Test ask for the database
 * password here and pass it to the backend (which keeps it in memory for reconnects while a
 * connection of the profile is open; it is never stored). Also reused for the password of an
 * encrypted connections file.
 */

export interface PasswordPromptRequest {
  id: number;
  title: string;
  description: string;
  /** Label of the confirm button. */
  confirmLabel: string;
  /** Shown under the field (e.g. "wrong password" when asking again). */
  error: string | null;
}

export const PASSWORD_PROMPT_HOST = "connection-password-prompt";

export const usePasswordPromptStore = create<{ queue: PasswordPromptRequest[] }>()(() => ({
  queue: [],
}));

let nextId = 0;
const resolvers = new Map<number, (password: string | null) => void>();

/**
 * Asks for a password; resolves with it, or null when cancelled (or when no prompt host is
 * mounted, so callers simply do not go on).
 */
export function requestPassword(
  req: Partial<Omit<PasswordPromptRequest, "id">> & { title: string },
): Promise<string | null> {
  if (!hasSingleHost(PASSWORD_PROMPT_HOST)) return Promise.resolve(null);
  const id = ++nextId;
  const full: PasswordPromptRequest = {
    id,
    description: "",
    confirmLabel: "Connect",
    error: null,
    ...req,
  };
  return new Promise((resolve) => {
    resolvers.set(id, resolve);
    usePasswordPromptStore.setState((s) => ({ queue: [...s.queue, full] }));
  });
}

/** Answers prompt `id` (null = cancelled). Later answers for the same prompt are ignored. */
export function answerPasswordPrompt(id: number, password: string | null): void {
  const resolve = resolvers.get(id);
  resolvers.delete(id);
  usePasswordPromptStore.setState((s) =>
    s.queue.some((r) => r.id === id) ? { queue: s.queue.filter((r) => r.id !== id) } : s,
  );
  resolve?.(password);
}

/** Cancels every pending prompt (the host went away). */
export function cancelPasswordPrompts(): void {
  const pending = [...resolvers.values()];
  resolvers.clear();
  usePasswordPromptStore.setState((s) => (s.queue.length > 0 ? { queue: [] } : s));
  pending.forEach((resolve) => resolve(null));
}

/** Thrown by a connect / test the user cancelled at the password prompt: nothing to report. */
export class PasswordPromptCancelled extends Error {
  constructor() {
    super("Cancelled");
    this.name = "PasswordPromptCancelled";
  }
}

export function isPromptCancelled(err: unknown): boolean {
  return err instanceof PasswordPromptCancelled;
}

/** Asks for the database password of connection `name` (Connect / Test). */
export async function askConnectionPassword(
  name: string,
  purpose: "connect" | "test",
): Promise<string> {
  const password = await requestPassword({
    title: `Password for ${name || "this connection"}`,
    description:
      "This connection asks for its password every time. It is kept in memory while the connection is open, so reconnecting does not ask again.",
    confirmLabel: purpose === "test" ? "Test" : "Connect",
  });
  if (password === null) throw new PasswordPromptCancelled();
  return password;
}

/**
 * Passwords typed in the connection form's prompt (Test) for the Connect that follows, so the
 * form does not ask twice. One use each, by profile id.
 */
const handedOver = new Map<string, string>();

export function handOverPassword(profileId: string, password: string): void {
  handedOver.set(profileId, password);
}

/** Forgets a handed-over password the Connect did not use (the profile was already open). */
export function dropHandedOverPassword(profileId: string): void {
  handedOver.delete(profileId);
}

export function takeHandedOverPassword(profileId: string): string | undefined {
  const password = handedOver.get(profileId);
  handedOver.delete(profileId);
  return password;
}
