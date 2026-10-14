import { describe, expect, it } from "vitest";
import { DEFAULT_PREFS, sanitizePrefs } from "@/stores/prefs";
import {
  ANTHROPIC_MODEL_IDS,
  baseUrlError,
  configError,
  DEFAULT_ANTHROPIC_MODEL,
  isModelName,
  modelLabel,
  providerNeedsKey,
  providerUsesKey,
  resolveProvider,
  type AssistantSettings,
} from "./providers";

const SETTINGS: AssistantSettings = {
  assistantProvider: "anthropic",
  assistantAnthropicModel: "claude-opus-5-5",
  assistantOpenaiBaseUrl: "https://api.deepseek.com/v1",
  assistantOpenaiModel: "deepseek-chat",
  assistantOllamaBaseUrl: "http://localhost:11434",
  assistantOllamaModel: "",
};

describe("assistant settings validation", () => {
  it("accepts http(s) base URLs only", () => {
    expect(baseUrlError("https://api.openai.com/v1")).toBeNull();
    expect(baseUrlError(" http://127.0.0.1:1234/v1 ")).toBeNull();
    expect(baseUrlError("")).toMatch(/base URL/);
    expect(baseUrlError("localhost:11434")).not.toBeNull();
    expect(baseUrlError("ftp://host")).toMatch(/http/);
    expect(baseUrlError("https://h/v1?key=1")).toMatch(/query/);
    expect(baseUrlError(`https://h/${"a".repeat(600)}`)).toMatch(/too long/);
  });

  it("checks model names", () => {
    expect(isModelName("llama3.2:latest")).toBe(true);
    expect(isModelName("")).toBe(true);
    expect(isModelName("a\nb")).toBe(false);
    expect(isModelName(42)).toBe(false);
    expect(isModelName("x".repeat(201))).toBe(false);
  });

  it("defaults to Anthropic with the recommended Claude model", () => {
    expect(DEFAULT_PREFS.assistantEnabled).toBe(true);
    expect(DEFAULT_PREFS.assistantProvider).toBe("anthropic");
    expect(DEFAULT_PREFS.assistantAnthropicModel).toBe("claude-opus-5-5");
    expect(DEFAULT_ANTHROPIC_MODEL).toBe(ANTHROPIC_MODEL_IDS[0]);
  });

  it("sanitizes stored assistant preferences", () => {
    const prefs = sanitizePrefs({
      assistantEnabled: "yes",
      assistantProvider: "gemini",
      assistantAnthropicModel: "claude-3-opus",
      assistantOpenaiBaseUrl: "javascript:alert(1)",
      assistantOllamaModel: "qwen2.5-coder:7b",
    });
    expect(prefs.assistantEnabled).toBe(true);
    expect(prefs.assistantProvider).toBe("anthropic");
    expect(prefs.assistantAnthropicModel).toBe("claude-opus-5-5");
    expect(prefs.assistantOpenaiBaseUrl).toBe("https://api.openai.com/v1");
    expect(prefs.assistantOllamaModel).toBe("qwen2.5-coder:7b");
  });
});

describe("resolveProvider", () => {
  it("uses the settings of the chosen provider", () => {
    expect(resolveProvider(SETTINGS)).toEqual({
      provider: "anthropic",
      model: "claude-opus-5-5",
      baseUrl: null,
    });
    expect(resolveProvider({ ...SETTINGS, assistantProvider: "openai" })).toEqual({
      provider: "openai",
      model: "deepseek-chat",
      baseUrl: "https://api.deepseek.com/v1",
    });
  });

  it("lets an agent override the provider and the model", () => {
    expect(resolveProvider(SETTINGS, { provider: "ollama", model: " sqlcoder " })).toEqual({
      provider: "ollama",
      model: "sqlcoder",
      baseUrl: "http://localhost:11434",
    });
    expect(resolveProvider(SETTINGS, { provider: null, model: "" }).model).toBe("claude-opus-5-5");
  });

  it("reports incomplete settings before anything is sent", () => {
    expect(configError(resolveProvider(SETTINGS))).toBeNull();
    expect(configError(resolveProvider({ ...SETTINGS, assistantProvider: "ollama" }))).toBe(
      "Choose a model",
    );
    expect(configError({ provider: "openai", model: "m", baseUrl: "not a url" })).not.toBeNull();
  });

  it("labels models and knows which providers use keys", () => {
    expect(modelLabel(resolveProvider(SETTINGS))).toBe("Claude Opus 5.5");
    expect(modelLabel({ provider: "ollama", model: "", baseUrl: null })).toBe("Ollama (local)");
    expect(providerNeedsKey("anthropic")).toBe(true);
    expect(providerNeedsKey("openai")).toBe(false);
    expect(providerUsesKey("openai")).toBe(true);
    expect(providerUsesKey("ollama")).toBe(false);
  });
});
