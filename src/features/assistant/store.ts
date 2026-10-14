import { create } from "zustand";
import {
  ipc,
  toIpcError,
  type AssistantContextPreview,
  type AssistantContextRequest,
  type AssistantEvent,
  type AssistantFailure,
  type AssistantMessage,
  type AssistantProviderConfig,
} from "@/lib/ipc";
import { useInspectorStore } from "@/stores/inspector";
import { useUiStore } from "@/stores/ui";
import type { Agent } from "./agents";
import { configError, modelLabel } from "./providers";

/**
 * The assistant's conversations, one per connection, kept in memory only (Clear empties one),
 * and whether the right sidebar shows the Assistant tab.
 */

export type ChatStatus = "streaming" | "done" | "stopped" | "error";

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  status: ChatStatus;
  /** The model is reasoning and no text has arrived since. */
  thinking?: boolean;
  error?: AssistantFailure | null;
  notices?: string[];
  /** Assistant replies: the structure that was sent with the request. */
  context?: AssistantContextPreview | null;
  /** Assistant replies: who answered ("SQL helper · Claude Opus 5.5"). */
  agent?: string;
  stopReason?: string | null;
}

export interface Conversation {
  messages: ChatMessage[];
  /** The request in flight. */
  requestId: string | null;
  /** Unsent text of the composer. */
  draft: string;
}

const EMPTY: Conversation = Object.freeze({
  messages: [],
  requestId: null,
  draft: "",
}) as Conversation;

interface AssistantState {
  byConnection: Record<string, Conversation>;
  /** The right sidebar shows the Assistant tab (instead of Details / Cell). */
  active: boolean;
  /** Bumped by "Ask Assistant…": the composer takes the focus (see `takeFocusRequest`). */
  focusRequest: number;
}

export const useAssistantStore = create<AssistantState>()(() => ({
  byConnection: {},
  active: false,
  focusRequest: 0,
}));

export function conversationOf(connectionId: string | null | undefined): Conversation {
  return (connectionId && useAssistantStore.getState().byConnection[connectionId]) || EMPTY;
}

export function useConversation(connectionId: string | null | undefined): Conversation {
  return useAssistantStore((s) => (connectionId && s.byConnection[connectionId]) || EMPTY);
}

function update(connectionId: string, fn: (c: Conversation) => Conversation): void {
  useAssistantStore.setState((s) => ({
    byConnection: {
      ...s.byConnection,
      [connectionId]: fn(s.byConnection[connectionId] ?? EMPTY),
    },
  }));
}

function updateMessage(
  connectionId: string,
  messageId: string,
  fn: (m: ChatMessage) => ChatMessage,
): void {
  update(connectionId, (c) => ({
    ...c,
    messages: c.messages.map((m) => (m.id === messageId ? fn(m) : m)),
  }));
}

export function setDraft(connectionId: string, draft: string): void {
  update(connectionId, (c) => (c.draft === draft ? c : { ...c, draft }));
}

/** Shows the Assistant tab in the right sidebar, opening the sidebar if needed. */
export function showAssistant(): void {
  useAssistantStore.setState({ active: true });
  const ui = useUiStore.getState();
  if (!ui.inspectorOpen) ui.toggleInspector();
}

/**
 * "Ask Assistant…": shows the Assistant tab (opening the right sidebar) and asks its composer to
 * take the focus, whether the tab was already showing or mounts only now.
 */
export function askAssistant(): void {
  showAssistant();
  useAssistantStore.setState((s) => ({ focusRequest: s.focusRequest + 1 }));
}

let handledFocusRequest = 0;

/** Whether a focus request is waiting for the composer (taken once; call it when it mounts). */
export function takeFocusRequest(): boolean {
  const pending = useAssistantStore.getState().focusRequest;
  if (pending === handledFocusRequest) return false;
  handledFocusRequest = pending;
  return true;
}

export function hideAssistant(): void {
  useAssistantStore.setState({ active: false });
}

// Space in a grid (`toggleRowDetail`) opens the sidebar on Details: while the sidebar is closed it
// sets the mode to "row" (the inspected cell unchanged) and opens the sidebar in the same tick —
// leave the Assistant then. ⇧⌘I and the toolbar reopen the sidebar on the Assistant.
let rowDetailRequested = false;
useInspectorStore.subscribe((s, prev) => {
  if (s.cell === prev.cell && s.mode === "row" && !useUiStore.getState().inspectorOpen) {
    rowDetailRequested = true;
    queueMicrotask(() => {
      rowDetailRequested = false;
    });
  }
});
useUiStore.subscribe((s, prev) => {
  if (s.inspectorOpen && !prev.inspectorOpen && rowDetailRequested) {
    rowDetailRequested = false;
    if (useAssistantStore.getState().active) hideAssistant();
  }
});

/** The conversation as sent to the provider: finished turns with text. */
export function historyForRequest(messages: readonly ChatMessage[]): AssistantMessage[] {
  return messages
    .filter((m) => m.content.trim() !== "" && (m.role === "user" || m.status !== "error"))
    .map((m) => ({ role: m.role, content: m.content }));
}

export interface SendOptions {
  agent: Agent;
  provider: AssistantProviderConfig;
  context: AssistantContextRequest | null;
}

/** Reply text arriving faster than this is shown in batches. */
const FLUSH_MS = 30;

const newId = () => crypto.randomUUID();

/**
 * Sends `text` in the connection's conversation and streams the reply into it. Resolves when
 * the reply ended (done, stopped or failed — failures are shown inline, never retried).
 */
export async function sendMessage(
  connectionId: string,
  text: string,
  { agent, provider, context }: SendOptions,
): Promise<void> {
  const content = text.trim();
  const current = conversationOf(connectionId);
  if (!content || current.requestId) return;
  const requestId = newId();
  const user: ChatMessage = { id: newId(), role: "user", content, status: "done" };
  const reply: ChatMessage = {
    id: newId(),
    role: "assistant",
    content: "",
    status: "streaming",
    agent: `${agent.name} · ${modelLabel(provider)}`,
  };
  const history = historyForRequest([...current.messages, user]);
  update(connectionId, (c) => ({
    ...c,
    draft: "",
    requestId,
    messages: [...c.messages, user, reply],
  }));

  let pending = "";
  let timer: ReturnType<typeof setTimeout> | null = null;
  const flush = () => {
    if (timer !== null) clearTimeout(timer);
    timer = null;
    if (!pending) return;
    const add = pending;
    pending = "";
    updateMessage(connectionId, reply.id, (m) => ({
      ...m,
      content: m.content + add,
      thinking: false,
    }));
  };
  const finish = (patch: Partial<ChatMessage>) => {
    flush();
    update(connectionId, (c) => ({
      ...c,
      requestId: c.requestId === requestId ? null : c.requestId,
      messages: c.messages.map((m) =>
        m.id === reply.id ? { ...m, thinking: false, ...patch } : m,
      ),
    }));
  };

  const invalid = configError(provider);
  if (invalid) {
    finish({ status: "error", error: { kind: "config", message: invalid } });
    return;
  }

  const onEvent = (event: AssistantEvent) => {
    switch (event.type) {
      case "delta":
        pending += event.text;
        timer ??= setTimeout(flush, FLUSH_MS);
        break;
      case "thinking":
        flush();
        updateMessage(connectionId, reply.id, (m) => ({ ...m, thinking: true }));
        break;
      case "context":
        updateMessage(connectionId, reply.id, (m) => ({ ...m, context: event.context }));
        break;
      case "notice":
        flush();
        updateMessage(connectionId, reply.id, (m) => ({
          ...m,
          notices: [...(m.notices ?? []), event.message],
        }));
        break;
    }
  };

  try {
    const result = await ipc.assistant.chat(
      { requestId, provider, systemPrompt: agent.systemPrompt, messages: history, context },
      onEvent,
    );
    if (result.error)
      finish({ status: "error", error: result.error, stopReason: result.stopReason });
    else if (result.cancelled) finish({ status: "stopped" });
    else finish({ status: "done", stopReason: result.stopReason ?? null });
  } catch (err) {
    finish({ status: "error", error: { kind: "other", message: toIpcError(err).message } });
  }
}

/** Stops the connection's reply in flight (its text so far stays). */
export async function stopReply(connectionId: string): Promise<void> {
  const requestId = conversationOf(connectionId).requestId;
  if (!requestId) return;
  try {
    await ipc.assistant.cancel(requestId);
  } catch (err) {
    console.warn("[assistant] cancel failed", err);
  }
}

/** Empties the connection's conversation (stopping a reply in flight). */
export function clearConversation(connectionId: string): void {
  void stopReply(connectionId);
  useAssistantStore.setState((s) => {
    const byConnection = { ...s.byConnection };
    const draft = byConnection[connectionId]?.draft ?? "";
    byConnection[connectionId] = { messages: [], requestId: null, draft };
    return { byConnection };
  });
}

/** A note for the reply's stop reason when it did not end normally (e.g. the length cap). */
export function stopNote(reason: string | null | undefined): string | null {
  if (reason === "max_tokens" || reason === "length") return "The reply reached its length limit.";
  return null;
}

/** Test-only. */
export function resetAssistantForTests(): void {
  useAssistantStore.setState({ byConnection: {}, active: false });
}
