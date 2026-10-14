import { create } from "zustand";
import { ipc, type SshPromptRequest } from "@/lib/ipc";

/**
 * SSH keyboard-interactive (2FA) questions: the backend emits `ssh:prompt` while a connect or test
 * signs in to an SSH server that asks something (a verification code, a PIN…); `SshPromptHost`
 * shows them one at a time and answers with `answer_ssh_prompt`. `ssh:prompt-done` removes a
 * question the backend no longer waits for.
 */

/** Name of the single-host slot `SshPromptHost` registers (the mock checks it too). */
export const SSH_PROMPT_HOST = "ssh-keyboard-interactive-prompt";

/** Questions waiting for an answer; the first one is on screen. */
export const useSshPromptStore = create<{ queue: SshPromptRequest[] }>()(() => ({ queue: [] }));

/**
 * SSH passwords handed over for a connect (a `+ssh` URL's password: used for that session only,
 * never stored), by profile id. The connect's password question is answered with it instead of
 * being shown; each is used once.
 */
const handedOver = new Map<string, string>();

export function presetSshPassword(profileId: string, password: string): void {
  handedOver.set(profileId, password);
}

export function clearSshPassword(profileId: string): void {
  handedOver.delete(profileId);
}

/** Whether a round asks for the SSH password: the app's own question, or the server's one hidden
 * "Password:" prompt. */
export function isPasswordQuestion(r: SshPromptRequest): boolean {
  if (r.kind === "password") return true;
  return (
    r.kind === undefined &&
    r.prompts.length === 1 &&
    !r.prompts[0]!.echo &&
    /password/i.test(r.prompts[0]!.prompt)
  );
}

export function enqueueSshPrompt(request: SshPromptRequest): void {
  const preset = handedOver.get(request.profileId);
  if (preset !== undefined && isPasswordQuestion(request)) {
    handedOver.delete(request.profileId);
    answerSshPrompt(request.requestId, [preset]);
    return;
  }
  useSshPromptStore.setState((s) =>
    s.queue.some((r) => r.requestId === request.requestId) ? s : { queue: [...s.queue, request] },
  );
}

/** Removes a question from the queue (answered here, or no longer awaited by the backend). */
export function dropSshPrompt(requestId: string): void {
  useSshPromptStore.setState((s) =>
    s.queue.some((r) => r.requestId === requestId)
      ? { queue: s.queue.filter((r) => r.requestId !== requestId) }
      : s,
  );
}

/** Answers question `requestId` (null cancels the connect or test waiting for it). */
export function answerSshPrompt(requestId: string, answers: string[] | null): void {
  dropSshPrompt(requestId);
  // A late answer (the backend gave up meanwhile) is ignored by the backend.
  void ipc.connections.answerSshPrompt(requestId, answers).catch((err: unknown) => {
    console.warn("[ssh] could not answer the SSH server's question", err);
  });
}

/** Cancels every queued question (the prompt host went away). */
export function cancelSshPrompts(): void {
  const pending = useSshPromptStore.getState().queue;
  if (pending.length === 0) return;
  useSshPromptStore.setState({ queue: [] });
  pending.forEach((r) => void ipc.connections.answerSshPrompt(r.requestId, null).catch(() => {}));
}

/** The server's question without its trailing colon: `Verification code: ` → `Verification code`. */
export function promptLabel(prompt: string): string {
  const text = prompt.trim().replace(/:\s*$/, "").trim();
  return text === "" ? "Answer" : text;
}
