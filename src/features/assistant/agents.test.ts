import { beforeEach, describe, expect, it } from "vitest";
import { ipc } from "@/lib/ipc";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
import {
  AGENTS_SETTING_KEY,
  agentError,
  allAgents,
  BUILT_IN_AGENTS,
  DEFAULT_AGENT_ID,
  loadAgents,
  resetAgentsForTests,
  sanitizeStoredAgents,
  selectedAgent,
  useAgentsStore,
} from "./agents";

const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  resetMockSettings();
  resetAgentsForTests();
});

describe("agents", () => {
  it("ships SQL helper, Explain query and Optimize query", () => {
    expect(BUILT_IN_AGENTS.map((a) => a.name)).toEqual([
      "SQL helper",
      "Explain query",
      "Optimize query",
    ]);
    expect(BUILT_IN_AGENTS.find((a) => a.id === "optimize-query")!.systemPrompt).toMatch(/EXPLAIN/);
    expect(selectedAgent(useAgentsStore.getState()).id).toBe(DEFAULT_AGENT_ID);
  });

  it("validates names, instructions and overrides", () => {
    const ok = {
      name: "Reviewer",
      systemPrompt: "Review migrations.",
      provider: null,
      model: null,
    };
    expect(agentError(ok)).toBeNull();
    expect(agentError({ ...ok, name: "  " })).toMatch(/name/);
    expect(agentError({ ...ok, name: "x".repeat(61) })).toMatch(/60/);
    expect(agentError({ ...ok, systemPrompt: "" })).toMatch(/instructions/);
    expect(agentError({ ...ok, provider: "gemini" as never })).toMatch(/provider/);
    expect(agentError({ ...ok, model: "a\nb" })).toMatch(/model/);
  });

  it("adds, edits, selects and deletes custom agents, and persists them", async () => {
    const store = useAgentsStore.getState();
    const id = store.save({
      name: "  Migration reviewer ",
      systemPrompt: "Point out locking risks.",
      provider: "ollama",
      model: " sqlcoder ",
    });
    let state = useAgentsStore.getState();
    expect(state.selectedId).toBe(id);
    expect(state.custom[0]).toMatchObject({
      name: "Migration reviewer",
      provider: "ollama",
      model: "sqlcoder",
    });
    state.save({ name: "Reviewer", systemPrompt: "Shorter.", provider: null, model: "" }, id);
    state = useAgentsStore.getState();
    expect(state.custom).toHaveLength(1);
    expect(state.custom[0]).toMatchObject({ name: "Reviewer", provider: null, model: null });
    expect(() => state.save({ name: "x", systemPrompt: "y" }, "sql-helper")).toThrow(/Built-in/);
    expect(() => state.save({ name: "", systemPrompt: "y" })).toThrow(/name/);

    await flush();
    const stored = (await ipc.settings.get())[AGENTS_SETTING_KEY] as {
      agents: unknown[];
      selectedId: string;
    };
    expect(stored.selectedId).toBe(id);
    expect(stored.agents).toHaveLength(1);

    state.select("explain-query");
    expect(useAgentsStore.getState().selectedId).toBe("explain-query");
    state.select("missing");
    expect(useAgentsStore.getState().selectedId).toBe("explain-query");

    useAgentsStore.getState().select(id);
    useAgentsStore.getState().remove(id);
    expect(useAgentsStore.getState().custom).toEqual([]);
    expect(useAgentsStore.getState().selectedId).toBe(DEFAULT_AGENT_ID);
    // Built-ins cannot be removed.
    useAgentsStore.getState().remove("sql-helper");
    expect(allAgents(useAgentsStore.getState().custom)).toHaveLength(3);
  });

  it("loads stored agents, dropping invalid entries", async () => {
    await ipc.settings.set(AGENTS_SETTING_KEY, {
      agents: [
        { id: "a1", name: "Ok", systemPrompt: "Do it.", provider: "openai", model: "m" },
        { id: "a1", name: "Duplicate id", systemPrompt: "x" },
        { id: "sql-helper", name: "Shadows a built-in", systemPrompt: "x" },
        { id: "a2", name: "", systemPrompt: "no name" },
        { id: "a3", name: "Bad provider", systemPrompt: "x", provider: "gemini" },
        "junk",
      ],
      selectedId: "a1",
    });
    await loadAgents();
    const state = useAgentsStore.getState();
    expect(state.loaded).toBe(true);
    expect(state.custom.map((a) => a.id)).toEqual(["a1", "a3"]);
    expect(state.custom[1]!.provider).toBeNull();
    expect(selectedAgent(state).name).toBe("Ok");
    expect(sanitizeStoredAgents({ agents: [], selectedId: "gone" }).selectedId).toBe(
      DEFAULT_AGENT_ID,
    );
    expect(sanitizeStoredAgents(null)).toEqual({ agents: [], selectedId: DEFAULT_AGENT_ID });
  });
});
