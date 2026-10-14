import { CheckCircle2, Loader2, ShieldCheck, XCircle } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { SettingGroup, SettingRow } from "@/features/settings/controls";
import { ipc, toIpcError, type AssistantProvider, type AssistantTestResult } from "@/lib/ipc";
import { setPref, usePrefs, type Prefs } from "@/stores/prefs";
import { refreshKeyStatus, saveKey, useAssistantKeys } from "./keys";
import {
  ANTHROPIC_MODELS,
  ASSISTANT_PROVIDERS,
  baseUrlError,
  isModelName,
  OPENAI_BASE_URL_PRESETS,
  PROVIDER_LABELS,
  providerNeedsKey,
  providerUsesKey,
  resolveProvider,
  type AssistantSettings as Settings,
} from "./providers";
import { ASSISTANT_SETTINGS_ID } from "./settingsLink";

export const STRUCTURE_ONLY_NOTE =
  "Only the structure of the current connection (table and column names, types, DDL) is sent to the provider — never row data or cell values.";

type TextKey =
  | "assistantOpenaiBaseUrl"
  | "assistantOpenaiModel"
  | "assistantOllamaBaseUrl"
  | "assistantOllamaModel";

/** A text preference committed on Enter / blur / unmount when valid; invalid text shows why. */
function TextPrefField({
  id,
  prefKey,
  validate,
  placeholder,
  list,
  className,
}: {
  id: string;
  prefKey: TextKey;
  validate: (v: string) => string | null;
  placeholder?: string;
  list?: string;
  className?: string;
}) {
  const value = usePrefs((p) => p[prefKey]);
  const [draft, setDraftState] = useState<string | null>(null);
  const draftRef = useRef<string | null>(null);
  const setDraft = (v: string | null) => {
    draftRef.current = v;
    setDraftState(v);
  };
  const error = draft !== null ? validate(draft) : null;
  const commit = () => {
    const next = draftRef.current;
    if (next === null) return;
    if (validate(next) === null) {
      setPref(prefKey, next.trim());
      setDraft(null);
    }
  };
  useEffect(
    () => () => {
      const next = draftRef.current;
      if (next !== null && validate(next) === null) setPref(prefKey, next.trim());
    },
    // Unmount only: commits what was typed when the dialog closes with focus in the field.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );
  return (
    <div className="flex flex-col items-end gap-0.5">
      <Input
        id={id}
        value={draft ?? value}
        placeholder={placeholder}
        list={list}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        aria-invalid={error ? true : undefined}
        aria-labelledby={`${id}-label`}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit();
          } else if (e.key === "Escape" && draftRef.current !== null) {
            e.preventDefault();
            e.stopPropagation();
            setDraft(null);
          }
        }}
        className={className ?? "h-7 w-64 rounded-md px-2 font-mono text-xs"}
      />
      {error && <span className="text-destructive text-[11px]">{error}</span>}
    </div>
  );
}

/** The provider's API key: stored in the Keychain, replaced or removed here (never shown). */
function ApiKeyRow({ provider }: { provider: Exclude<AssistantProvider, "ollama"> }) {
  const id = useId();
  const status = useAssistantKeys((s) => s.status);
  const stored = status ? status[provider] : null;
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const run = async (key: string | null) => {
    setBusy(true);
    try {
      await saveKey(provider, key);
      setDraft("");
      toast.success(key ? "API key saved in the Keychain" : "API key removed");
    } catch (err) {
      toast.error("Could not save the API key", { description: toIpcError(err).message });
    } finally {
      setBusy(false);
    }
  };
  return (
    <SettingRow
      id={id}
      label="API key"
      description={
        stored
          ? "Stored in the macOS Keychain. Type a new key to replace it."
          : providerNeedsKey(provider)
            ? "Required. Kept in the macOS Keychain, never in preferences."
            : "Optional (local servers such as LM Studio need none). Kept in the macOS Keychain."
      }
    >
      <form
        className="flex items-center gap-1.5"
        onSubmit={(e) => {
          e.preventDefault();
          if (draft.trim()) void run(draft.trim());
        }}
      >
        <Input
          id={id}
          type="password"
          autoComplete="off"
          value={draft}
          placeholder={stored ? "•••••••• (stored)" : "Paste the key"}
          onChange={(e) => setDraft(e.target.value)}
          className="h-7 w-44 rounded-md px-2 font-mono text-xs"
        />
        <Button type="submit" size="sm" variant="outline" disabled={busy || !draft.trim()}>
          Save
        </Button>
        {stored && (
          <Button
            type="button"
            size="sm"
            variant="ghost"
            disabled={busy}
            onClick={() => void run(null)}
          >
            Remove
          </Button>
        )}
      </form>
    </SettingRow>
  );
}

function TestRow({ settings }: { settings: Settings }) {
  const [state, setState] = useState<
    { kind: "idle" } | { kind: "busy" } | { kind: "done"; result: AssistantTestResult }
  >({ kind: "idle" });
  const provider = settings.assistantProvider;
  // A new provider or server invalidates the last result.
  const signature = JSON.stringify(resolveProvider(settings));
  const [shownFor, setShownFor] = useState(signature);
  if (shownFor !== signature) {
    setShownFor(signature);
    setState({ kind: "idle" });
  }
  const test = async () => {
    setState({ kind: "busy" });
    try {
      const result = await ipc.assistant.test(resolveProvider(settings));
      setState({ kind: "done", result });
    } catch (err) {
      setState({
        kind: "done",
        result: { ok: false, message: toIpcError(err).message, models: [] },
      });
    }
  };
  const result = state.kind === "done" ? state.result : null;
  return (
    <>
      <SettingRow
        label="Check the connection"
        description={
          result ? (
            <span
              className={result.ok ? "text-foreground flex gap-1" : "text-destructive flex gap-1"}
              role="status"
            >
              {result.ok ? (
                <CheckCircle2 className="mt-px size-3 shrink-0 text-emerald-600 dark:text-emerald-400" />
              ) : (
                <XCircle className="mt-px size-3 shrink-0" />
              )}
              {result.message}
            </span>
          ) : provider === "anthropic" ? (
            "Checks the key and that the model is available. Nothing is sent about your database."
          ) : (
            "Checks the server and lists its models. Nothing is sent about your database."
          )
        }
      >
        <Button size="sm" variant="outline" disabled={state.kind === "busy"} onClick={test}>
          {state.kind === "busy" && <Loader2 className="animate-spin" />}
          Test
        </Button>
      </SettingRow>
      {result && result.models.length > 0 && (
        <datalist id={`assistant-models-${provider}`}>
          {result.models.map((m) => (
            <option key={m} value={m} />
          ))}
        </datalist>
      )}
    </>
  );
}

/**
 * Settings ▸ AI Assistant: the TablePlus toggle "Enable Assistant tab on the right
 * sidebar", then the provider (Anthropic, OpenAI-compatible, Ollama), its model / server and key.
 */
export function AssistantSettingsGroup() {
  const enabled = usePrefs((p) => p.assistantEnabled);
  const provider = usePrefs((p) => p.assistantProvider);
  const anthropicModel = usePrefs((p) => p.assistantAnthropicModel);
  const openaiBaseUrl = usePrefs((p) => p.assistantOpenaiBaseUrl);
  const openaiModel = usePrefs((p) => p.assistantOpenaiModel);
  const ollamaBaseUrl = usePrefs((p) => p.assistantOllamaBaseUrl);
  const ollamaModel = usePrefs((p) => p.assistantOllamaModel);
  const settings: Settings = {
    assistantProvider: provider,
    assistantAnthropicModel: anthropicModel,
    assistantOpenaiBaseUrl: openaiBaseUrl,
    assistantOpenaiModel: openaiModel,
    assistantOllamaBaseUrl: ollamaBaseUrl,
    assistantOllamaModel: ollamaModel,
  };
  useEffect(() => {
    void refreshKeyStatus();
  }, []);
  const modelValidate = (v: string) =>
    !v.trim() ? "Enter a model" : isModelName(v) ? null : "Not a valid model name";

  return (
    <div id={ASSISTANT_SETTINGS_ID} className="scroll-mt-4">
      <SettingGroup title="AI Assistant">
        <SettingRow
          id="pref-assistant-enabled"
          label="Enable Assistant tab on the right sidebar"
          description={
            <span className="flex gap-1">
              <ShieldCheck className="mt-px size-3 shrink-0" />
              {STRUCTURE_ONLY_NOTE}
            </span>
          }
        >
          <Switch
            id="pref-assistant-enabled"
            checked={enabled}
            onCheckedChange={(v) => setPref("assistantEnabled", v)}
          />
        </SettingRow>
        <SettingRow id="pref-assistant-provider" label="Provider">
          <Select
            value={provider}
            onValueChange={(v) => setPref("assistantProvider", v as Prefs["assistantProvider"])}
          >
            <SelectTrigger
              id="pref-assistant-provider"
              size="sm"
              className="h-7 w-44 rounded-md text-xs"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent data-scope="dialog">
              {ASSISTANT_PROVIDERS.map((p) => (
                <SelectItem key={p} value={p} className="text-xs">
                  {PROVIDER_LABELS[p]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </SettingRow>

        {provider === "anthropic" && (
          <SettingRow id="pref-assistant-anthropic-model" label="Model">
            <Select
              value={anthropicModel}
              onValueChange={(v) => setPref("assistantAnthropicModel", v)}
            >
              <SelectTrigger
                id="pref-assistant-anthropic-model"
                size="sm"
                className="h-7 w-44 rounded-md text-xs"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent data-scope="dialog">
                {ANTHROPIC_MODELS.map((m) => (
                  <SelectItem key={m.id} value={m.id} className="text-xs">
                    {m.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </SettingRow>
        )}

        {provider === "openai" && (
          <>
            <SettingRow
              id="pref-assistant-openai-url"
              label="Base URL"
              description={
                <span className="flex flex-wrap gap-x-2">
                  {OPENAI_BASE_URL_PRESETS.map((p) => (
                    <button
                      key={p.url}
                      type="button"
                      className="text-primary hover:underline"
                      onClick={() => setPref("assistantOpenaiBaseUrl", p.url)}
                    >
                      {p.label}
                    </button>
                  ))}
                </span>
              }
            >
              <TextPrefField
                id="pref-assistant-openai-url"
                prefKey="assistantOpenaiBaseUrl"
                validate={baseUrlError}
                placeholder="https://api.openai.com/v1"
              />
            </SettingRow>
            <SettingRow
              id="pref-assistant-openai-model"
              label="Model"
              description="As the server names it, e.g. deepseek-chat. Test lists the server's models."
            >
              <TextPrefField
                id="pref-assistant-openai-model"
                prefKey="assistantOpenaiModel"
                validate={modelValidate}
                placeholder="model id"
                list="assistant-models-openai"
              />
            </SettingRow>
          </>
        )}

        {provider === "ollama" && (
          <>
            <SettingRow
              id="pref-assistant-ollama-url"
              label="Server"
              description="A local Ollama server; no API key is used."
            >
              <TextPrefField
                id="pref-assistant-ollama-url"
                prefKey="assistantOllamaBaseUrl"
                validate={baseUrlError}
                placeholder="http://localhost:11434"
              />
            </SettingRow>
            <SettingRow
              id="pref-assistant-ollama-model"
              label="Model"
              description="A pulled model, e.g. llama3.2. Test lists the installed ones."
            >
              <TextPrefField
                id="pref-assistant-ollama-model"
                prefKey="assistantOllamaModel"
                validate={modelValidate}
                placeholder="model name"
                list="assistant-models-ollama"
              />
            </SettingRow>
          </>
        )}

        {providerUsesKey(provider) && (
          <ApiKeyRow key={provider} provider={provider as Exclude<AssistantProvider, "ollama">} />
        )}
        <TestRow settings={settings} />
      </SettingGroup>
    </div>
  );
}
