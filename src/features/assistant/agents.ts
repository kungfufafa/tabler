import { toast } from "sonner";
import { create } from "zustand";
import { ipc, type AssistantProvider } from "@/lib/ipc";
import { ASSISTANT_PROVIDERS, isModelName } from "./providers";

/**
 * Custom agents (TablePlus b742): a name and instructions, optionally with their own provider
 * and model. Three built-in agents are always there; custom ones are stored in the app DB
 * settings under `AGENTS_SETTING_KEY` together with the selected agent.
 */

export interface Agent {
  id: string;
  name: string;
  systemPrompt: string;
  /** Overrides Settings ▸ AI Assistant's provider (null: the default). */
  provider?: AssistantProvider | null;
  /** Overrides the provider's model (null / empty: the provider's model). */
  model?: string | null;
  builtIn?: boolean;
}

export type AgentInput = Omit<Agent, "id" | "builtIn">;

export const AGENTS_SETTING_KEY = "assistant.agents";
export const MAX_AGENTS = 50;
export const MAX_AGENT_NAME = 60;
export const MAX_AGENT_PROMPT = 20_000;

export const BUILT_IN_AGENTS: readonly Agent[] = Object.freeze([
  {
    id: "sql-helper",
    name: "SQL helper",
    builtIn: true,
    systemPrompt:
      "You help the user write, fix and understand SQL for their database. Use the tables and " +
      "columns of the database context; when something the user asks for does not exist there, " +
      "say so instead of guessing names. Keep explanations short and put the SQL first.",
  },
  {
    id: "explain-query",
    name: "Explain query",
    builtIn: true,
    systemPrompt:
      "The user pastes a SQL query. Explain what it does in plain language, step by step: the " +
      "tables it reads, how they are joined and filtered, what each output column is, and " +
      "anything surprising (row multiplication from joins, NULL handling, implicit casts, " +
      "non-deterministic ordering). End with one sentence summarizing the result set.",
  },
  {
    id: "optimize-query",
    name: "Optimize query",
    builtIn: true,
    systemPrompt:
      "The user pastes a slow SQL query and, ideally, the EXPLAIN (or EXPLAIN ANALYZE) output " +
      "they got by running it. Using that plan and the table structure of the database " +
      "context, find the bottlenecks (sequential scans of large tables, missing or unusable " +
      "indexes, bad row estimates, sorts or hashes spilling to disk, N+1 patterns) and propose " +
      "concrete fixes: a rewritten query and CREATE INDEX statements for this dialect, each " +
      "with the reason. If no plan was pasted, say how to get one in Tabler (Explain in the " +
      "SQL editor) and give the most likely fixes from the structure alone.",
  },
]);

export const DEFAULT_AGENT_ID = "sql-helper";

/** Why an agent cannot be saved, or null. */
export function agentError(input: AgentInput): string | null {
  const name = input.name.trim();
  if (!name) return "Give the agent a name";
  if (name.length > MAX_AGENT_NAME) return `The name is longer than ${MAX_AGENT_NAME} characters`;
  if (!input.systemPrompt.trim()) return "Write the agent's instructions";
  if (input.systemPrompt.length > MAX_AGENT_PROMPT)
    return `The instructions are longer than ${MAX_AGENT_PROMPT.toLocaleString()} characters`;
  if (input.provider != null && !ASSISTANT_PROVIDERS.includes(input.provider))
    return "Unknown provider";
  if (input.model != null && !isModelName(input.model)) return "The model name is not valid";
  return null;
}

function cleanAgent(raw: unknown): Agent | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  if (typeof r.id !== "string" || !r.id || r.id.length > 64) return null;
  if (typeof r.name !== "string" || typeof r.systemPrompt !== "string") return null;
  const provider =
    typeof r.provider === "string" &&
    (ASSISTANT_PROVIDERS as readonly string[]).includes(r.provider)
      ? (r.provider as AssistantProvider)
      : null;
  const model = typeof r.model === "string" && r.model.trim() ? r.model.trim() : null;
  const agent: Agent = {
    id: r.id,
    name: r.name.trim(),
    systemPrompt: r.systemPrompt,
    provider,
    model,
  };
  return agentError(agent) ? null : agent;
}

/** Custom agents and the selection read from storage; invalid entries are dropped. */
export function sanitizeStoredAgents(raw: unknown): { agents: Agent[]; selectedId: string } {
  const r = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  const seen = new Set(BUILT_IN_AGENTS.map((a) => a.id));
  const agents: Agent[] = [];
  for (const item of Array.isArray(r.agents) ? r.agents : []) {
    const a = cleanAgent(item);
    if (!a || seen.has(a.id) || agents.length >= MAX_AGENTS) continue;
    seen.add(a.id);
    agents.push(a);
  }
  const selectedId =
    typeof r.selectedId === "string" && seen.has(r.selectedId) ? r.selectedId : DEFAULT_AGENT_ID;
  return { agents, selectedId };
}

interface AgentsState {
  custom: Agent[];
  selectedId: string;
  loaded: boolean;
  select: (id: string) => void;
  /** Adds (no `id`) or updates a custom agent; returns its id. Throws with the validation error. */
  save: (input: AgentInput, id?: string) => string;
  remove: (id: string) => void;
}

let loadPromise: Promise<void> | null = null;

async function persist(): Promise<void> {
  const { custom, selectedId } = useAgentsStore.getState();
  try {
    await ipc.settings.set(AGENTS_SETTING_KEY, { agents: custom, selectedId });
  } catch (err) {
    toast.error("Could not save the assistant agents", {
      id: "assistant-agents-save",
      description: err instanceof Error ? err.message : String(err),
    });
  }
}

export const useAgentsStore = create<AgentsState>()((set, get) => ({
  custom: [],
  selectedId: DEFAULT_AGENT_ID,
  loaded: false,
  select: (id) => {
    if (!allAgents(get().custom).some((a) => a.id === id) || id === get().selectedId) return;
    set({ selectedId: id });
    void persist();
  },
  save: (input, id) => {
    const error = agentError(input);
    if (error) throw new Error(error);
    const agent: Agent = {
      id: id ?? `agent-${crypto.randomUUID()}`,
      name: input.name.trim(),
      systemPrompt: input.systemPrompt,
      provider: input.provider ?? null,
      model: input.model?.trim() || null,
    };
    const custom = get().custom;
    if (id !== undefined) {
      if (!custom.some((a) => a.id === id)) throw new Error("Built-in agents cannot be changed");
      set({ custom: custom.map((a) => (a.id === id ? agent : a)) });
    } else {
      if (custom.length >= MAX_AGENTS) throw new Error(`At most ${MAX_AGENTS} agents`);
      set({ custom: [...custom, agent], selectedId: agent.id });
    }
    void persist();
    return agent.id;
  },
  remove: (id) => {
    const { custom, selectedId } = get();
    if (!custom.some((a) => a.id === id)) return;
    set({
      custom: custom.filter((a) => a.id !== id),
      selectedId: selectedId === id ? DEFAULT_AGENT_ID : selectedId,
    });
    void persist();
  },
}));

/** Reads the stored agents once (later calls share the promise). */
export function loadAgents(): Promise<void> {
  loadPromise ??= (async () => {
    try {
      const stored = (await ipc.settings.get())[AGENTS_SETTING_KEY];
      const { agents, selectedId } = sanitizeStoredAgents(stored);
      // Edits made while loading win.
      if (!useAgentsStore.getState().loaded && useAgentsStore.getState().custom.length === 0)
        useAgentsStore.setState({ custom: agents, selectedId });
    } catch (err) {
      console.warn("[assistant] could not load agents", err);
    } finally {
      useAgentsStore.setState({ loaded: true });
    }
  })();
  return loadPromise;
}

export function allAgents(custom: readonly Agent[]): Agent[] {
  return [...BUILT_IN_AGENTS, ...custom];
}

/** The selected agent (the default one when it went away). */
export function selectedAgent(state: Pick<AgentsState, "custom" | "selectedId">): Agent {
  return (
    allAgents(state.custom).find((a) => a.id === state.selectedId) ??
    BUILT_IN_AGENTS.find((a) => a.id === DEFAULT_AGENT_ID)!
  );
}

/** Test-only: back to the built-in agents, unloaded. */
export function resetAgentsForTests(): void {
  loadPromise = null;
  useAgentsStore.setState({ custom: [], selectedId: DEFAULT_AGENT_ID, loaded: false });
}
