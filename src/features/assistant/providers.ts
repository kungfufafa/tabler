import type { AssistantProvider, AssistantProviderConfig } from "@/lib/ipc";

/**
 * AI Assistant providers and their settings. Pure (no store, no IPC): `src/stores/prefs.ts`
 * validates its assistant keys with these helpers.
 */

export const ASSISTANT_PROVIDERS = [
  "anthropic",
  "openai",
  "ollama",
] as const satisfies readonly AssistantProvider[];

export const PROVIDER_LABELS: Record<AssistantProvider, string> = {
  anthropic: "Anthropic",
  openai: "OpenAI-compatible",
  ollama: "Ollama (local)",
};

/** Claude models offered in Settings (mirrors `CLAUDE_MODELS` in src-tauri/src/assistant/request.rs). */
export const ANTHROPIC_MODELS = [
  { id: "claude-opus-5-5", label: "Claude Opus 5.5" },
  { id: "claude-sonnet-5-5", label: "Claude Sonnet 5.5" },
  { id: "claude-haiku-5-5", label: "Claude Haiku 5.5" },
  { id: "claude-fable-5-1", label: "Claude Fable 5.1" },
  { id: "claude-opus-5", label: "Claude Opus 5" },
  { id: "claude-sonnet-5", label: "Claude Sonnet 5" },
  { id: "claude-opus-4-8", label: "Claude Opus 4.8" },
  { id: "claude-haiku-4-5", label: "Claude Haiku 4.5" },
] as const;
export type AnthropicModelId = (typeof ANTHROPIC_MODELS)[number]["id"];
export const ANTHROPIC_MODEL_IDS: readonly string[] = ANTHROPIC_MODELS.map((m) => m.id);
export const DEFAULT_ANTHROPIC_MODEL: AnthropicModelId = "claude-opus-5-5";

export const DEFAULT_OPENAI_BASE_URL = "https://api.openai.com/v1";
export const DEFAULT_OLLAMA_BASE_URL = "http://localhost:11434";

/** Presets of the OpenAI-compatible base URL field. */
export const OPENAI_BASE_URL_PRESETS = [
  { label: "OpenAI", url: DEFAULT_OPENAI_BASE_URL },
  { label: "DeepSeek", url: "https://api.deepseek.com/v1" },
  { label: "LM Studio", url: "http://localhost:1234/v1" },
  { label: "Ollama (/v1)", url: "http://localhost:11434/v1" },
] as const;

/** Providers whose API key lives in the Keychain (Ollama runs locally without one). */
export function providerUsesKey(provider: AssistantProvider): boolean {
  return provider !== "ollama";
}

/** A key is required (Anthropic) rather than optional (OpenAI-compatible local servers). */
export function providerNeedsKey(provider: AssistantProvider): boolean {
  return provider === "anthropic";
}

export const MAX_BASE_URL_LENGTH = 500;
export const MAX_MODEL_LENGTH = 200;

/** Why a base URL cannot be used, or null when it can (http/https, a host, no query). */
export function baseUrlError(raw: string): string | null {
  const url = raw.trim();
  if (!url) return "Enter the server's base URL";
  if (url.length > MAX_BASE_URL_LENGTH) return "The URL is too long";
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return "Not a valid URL (it starts with http:// or https://)";
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:")
    return "The URL must start with http:// or https://";
  if (!parsed.hostname) return "The URL needs a host";
  if (parsed.search || parsed.hash) return "The URL cannot have a query or #fragment";
  return null;
}

export function isBaseUrl(v: unknown): boolean {
  return typeof v === "string" && baseUrlError(v) === null;
}

/** A model name the settings accept (empty = not chosen yet). */
export function isModelName(v: unknown): boolean {
  return (
    typeof v === "string" &&
    v.length <= MAX_MODEL_LENGTH &&
    // eslint-disable-next-line no-control-regex
    !/[\u0000-\u001f\u007f]/.test(v)
  );
}

export function modelError(model: string): string | null {
  if (!model.trim()) return "Choose a model";
  return isModelName(model) ? null : "The model name is not valid";
}

/** The assistant settings kept in preferences. */
export interface AssistantSettings {
  assistantProvider: AssistantProvider;
  assistantAnthropicModel: string;
  assistantOpenaiBaseUrl: string;
  assistantOpenaiModel: string;
  assistantOllamaBaseUrl: string;
  assistantOllamaModel: string;
}

/** An agent's optional override of the provider and model. */
export interface ProviderOverride {
  provider?: AssistantProvider | null;
  model?: string | null;
}

/** The provider config a request uses: the settings, overridden by the agent. */
export function resolveProvider(
  settings: AssistantSettings,
  override?: ProviderOverride | null,
): AssistantProviderConfig {
  const provider = override?.provider ?? settings.assistantProvider;
  const overrideModel = override?.model?.trim() || null;
  switch (provider) {
    case "anthropic":
      return {
        provider,
        model: overrideModel ?? settings.assistantAnthropicModel,
        baseUrl: null,
      };
    case "openai":
      return {
        provider,
        model: overrideModel ?? settings.assistantOpenaiModel,
        baseUrl: settings.assistantOpenaiBaseUrl,
      };
    case "ollama":
      return {
        provider,
        model: overrideModel ?? settings.assistantOllamaModel,
        baseUrl: settings.assistantOllamaBaseUrl,
      };
  }
}

/** What is wrong with a provider config before anything is sent (null: ready). */
export function configError(config: AssistantProviderConfig): string | null {
  const model = modelError(config.model);
  if (model) return model;
  if (config.provider !== "anthropic") return baseUrlError(config.baseUrl ?? "");
  return null;
}

/** Short label of a model for the chat header. */
export function modelLabel(config: AssistantProviderConfig): string {
  if (config.provider === "anthropic") {
    return ANTHROPIC_MODELS.find((m) => m.id === config.model)?.label ?? config.model;
  }
  return config.model || PROVIDER_LABELS[config.provider];
}
