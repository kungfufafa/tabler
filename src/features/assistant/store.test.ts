import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ipc, type AssistantProviderConfig } from "@/lib/ipc";
import {
  mockAssistantRequests,
  resetMockAssistant,
  setMockAssistantDelay,
} from "@/lib/ipc/mock/assistant";
import { toggleRowDetail, useInspectorStore } from "@/stores/inspector";
import { useUiStore } from "@/stores/ui";
import { BUILT_IN_AGENTS } from "./agents";
import {
  clearConversation,
  conversationOf,
  historyForRequest,
  resetAssistantForTests,
  sendMessage,
  showAssistant,
  stopReply,
  useAssistantStore,
  type ChatMessage,
} from "./store";

const agent = BUILT_IN_AGENTS[0]!;
const CLAUDE: AssistantProviderConfig = { provider: "anthropic", model: "claude-opus-5-5" };

async function connect(): Promise<string> {
  const info = await ipc.connections.connect("p-local-pg");
  return info.connectionId;
}

beforeEach(() => {
  resetMockAssistant();
  resetAssistantForTests();
});
afterEach(() => setMockAssistantDelay(0));

describe("assistant conversations", () => {
  it("streams a reply with the structure context into the connection's conversation", async () => {
    const id = await connect();
    await sendMessage(id, "  Latest posts?  ", {
      agent,
      provider: CLAUDE,
      context: { connectionId: id, schema: "public", table: { schema: "public", name: "posts" } },
    });
    const [user, reply] = conversationOf(id).messages;
    expect(user).toMatchObject({ role: "user", content: "Latest posts?", status: "done" });
    expect(reply!.status).toBe("done");
    expect(reply!.stopReason).toBe("end_turn");
    expect(reply!.agent).toBe("SQL helper · Claude Opus 5.5");
    expect(reply!.content).toContain("```sql\nSELECT");
    expect(reply!.content).toContain("FROM posts");
    expect(reply!.context?.summary).toMatch(/^posts \+ \d+ tables$/);
    expect(reply!.context?.text).toContain("CREATE TABLE");
    expect(conversationOf(id).requestId).toBeNull();

    // What the renderer sent: the agent, the typed conversation and the context's ids only.
    const sent = mockAssistantRequests[0]!;
    expect(sent.systemPrompt).toBe(agent.systemPrompt);
    expect(sent.messages).toEqual([{ role: "user", content: "Latest posts?" }]);
    expect(sent.context).toEqual({
      connectionId: id,
      schema: "public",
      table: { schema: "public", name: "posts" },
    });

    // The next turn carries the history; conversations are per connection.
    await sendMessage(id, "And by author?", { agent, provider: CLAUDE, context: null });
    expect(mockAssistantRequests[1]!.messages.map((m) => m.role)).toEqual([
      "user",
      "assistant",
      "user",
    ]);
    expect(conversationOf("other").messages).toEqual([]);
  });

  it("shows provider failures inline without retrying", async () => {
    const id = await connect();
    await sendMessage(id, "hit the rate limit", { agent, provider: CLAUDE, context: null });
    const reply = conversationOf(id).messages[1]!;
    expect(reply.status).toBe("error");
    expect(reply.error).toMatchObject({ kind: "rateLimit", status: 429 });
    expect(mockAssistantRequests).toHaveLength(1);

    await ipc.assistant.setKey("anthropic", null);
    await sendMessage(id, "hello", { agent, provider: CLAUDE, context: null });
    expect(conversationOf(id).messages[3]!.error?.kind).toBe("missingKey");

    // Incomplete settings never reach the backend.
    await sendMessage(id, "hello", {
      agent,
      provider: { provider: "ollama", model: "", baseUrl: "http://localhost:11434" },
      context: null,
    });
    expect(conversationOf(id).messages[5]!.error).toEqual({
      kind: "config",
      message: "Choose a model",
    });
    expect(mockAssistantRequests).toHaveLength(2);
  });

  it("stops a streaming reply and keeps its text", async () => {
    const id = await connect();
    setMockAssistantDelay(5);
    const done = sendMessage(id, "Write a query", { agent, provider: CLAUDE, context: null });
    await new Promise((r) => setTimeout(r, 200));
    expect(conversationOf(id).requestId).not.toBeNull();
    await stopReply(id);
    await done;
    const reply = conversationOf(id).messages[1]!;
    expect(reply.status).toBe("stopped");
    expect(reply.content.length).toBeGreaterThan(0);
    expect(reply.content).not.toContain("Use **Run**");
    // A second send while one runs is ignored; a cleared conversation keeps no message.
    clearConversation(id);
    expect(conversationOf(id).messages).toEqual([]);
  });

  it("sends finished turns only", () => {
    const m = (role: ChatMessage["role"], content: string, status: ChatMessage["status"]) =>
      ({ id: content, role, content, status }) as ChatMessage;
    expect(
      historyForRequest([
        m("user", "a", "done"),
        m("assistant", "partial", "error"),
        m("user", "b", "done"),
        m("assistant", "kept", "stopped"),
        m("assistant", "  ", "done"),
      ]),
    ).toEqual([
      { role: "user", content: "a" },
      { role: "user", content: "b" },
      { role: "assistant", content: "kept" },
    ]);
  });

  it("shows the Assistant tab, and Space's row detail leaves it", async () => {
    useUiStore.setState({ inspectorOpen: false });
    showAssistant();
    expect(useAssistantStore.getState().active).toBe(true);
    expect(useUiStore.getState().inspectorOpen).toBe(true);
    // ⇧⌘I closes and reopens on the Assistant.
    useUiStore.getState().toggleInspector();
    useUiStore.getState().toggleInspector();
    expect(useAssistantStore.getState().active).toBe(true);
    // The grid publishing (no) cell while the sidebar is closed changes nothing.
    useUiStore.setState({ inspectorOpen: false });
    useInspectorStore.getState().set(null);
    await Promise.resolve();
    useUiStore.getState().toggleInspector();
    expect(useAssistantStore.getState().active).toBe(true);
    // Space in a grid (toggleRowDetail) opens on Details.
    useUiStore.setState({ inspectorOpen: false });
    toggleRowDetail();
    expect(useUiStore.getState().inspectorOpen).toBe(true);
    expect(useAssistantStore.getState().active).toBe(false);
  });
});
