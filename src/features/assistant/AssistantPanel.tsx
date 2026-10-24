import { useQueryClient } from "@tanstack/react-query";
import {
  ArrowUp,
  Bot,
  ChevronDown,
  Copy,
  Eraser,
  Info,
  Loader2,
  Settings2,
  ShieldCheck,
  Square,
  TriangleAlert,
} from "lucide-react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { coreDialect } from "@/features/drivers/catalog";
import { NotAvailable } from "@/features/drivers/NotAvailable";
import { formatCombo } from "@/lib/keys/combo";
import {
  ipc,
  toIpcError,
  type AssistantContextPreview,
  type AssistantContextRequest,
  type AssistantFailure,
  type Dialect,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useActiveConnection, type OpenConnection } from "@/stores/connections";
import { usePrefs } from "@/stores/prefs";
import { isTableTab, useTabsStore } from "@/stores/tabs";
import { copyText, insertIntoEditor, runInNewTab } from "./actions";
import { AgentDialog } from "./AgentDialog";
import { allAgents, loadAgents, selectedAgent, useAgentsStore, type Agent } from "./agents";
import { STRUCTURE_ONLY_NOTE } from "./AssistantSettings";
import { hasKey, refreshKeyStatus, useAssistantKeys } from "./keys";
import { Markdown, type SqlActions } from "./MarkdownView";
import { modelLabel, PROVIDER_LABELS, providerNeedsKey, resolveProvider } from "./providers";
import { openAssistantSettings } from "./settingsLink";
import {
  clearConversation,
  sendMessage,
  setDraft,
  stopNote,
  stopReply,
  takeFocusRequest,
  useAssistantStore,
  useConversation,
  type ChatMessage,
} from "./store";

const SEND_COMBO = "Mod+Enter";

/**
 * The right sidebar's Assistant tab (TablePlus b624): a chat about the active connection. Only
 * its structure is sent with each message; SQL in replies can be inserted into the editor or run
 * in a new tab.
 */
export function AssistantPanel() {
  const connection = useActiveConnection();
  if (!connection) {
    return (
      <div className="text-muted-foreground flex flex-1 items-center justify-center p-6 text-xs">
        No connection
      </div>
    );
  }
  // Its context and its SQL are a core dialect's.
  const dialect = coreDialect(connection.info.driver);
  if (!dialect) return <NotAvailable driver={connection.info.driver} what="The assistant" />;
  return (
    <AssistantChat key={connection.info.connectionId} connection={connection} dialect={dialect} />
  );
}

/** What the next message is about: the connection's schema and the active tab's table. */
function useContextRequest(connection: OpenConnection): AssistantContextRequest {
  const connectionId = connection.info.connectionId;
  const activeTab = useTabsStore((s) => s.tabs.find((t) => t.id === s.activeId) ?? null);
  const table =
    isTableTab(activeTab) && activeTab.connectionId === connectionId ? activeTab.table : null;
  const schema = connection.currentSchema ?? table?.schema ?? null;
  return useMemo(() => ({ connectionId, schema, table }), [connectionId, schema, table]);
}

function useProviderSettings(agent: Agent) {
  const assistantProvider = usePrefs((p) => p.assistantProvider);
  const assistantAnthropicModel = usePrefs((p) => p.assistantAnthropicModel);
  const assistantOpenaiBaseUrl = usePrefs((p) => p.assistantOpenaiBaseUrl);
  const assistantOpenaiModel = usePrefs((p) => p.assistantOpenaiModel);
  const assistantOllamaBaseUrl = usePrefs((p) => p.assistantOllamaBaseUrl);
  const assistantOllamaModel = usePrefs((p) => p.assistantOllamaModel);
  return useMemo(
    () =>
      resolveProvider(
        {
          assistantProvider,
          assistantAnthropicModel,
          assistantOpenaiBaseUrl,
          assistantOpenaiModel,
          assistantOllamaBaseUrl,
          assistantOllamaModel,
        },
        agent,
      ),
    [
      agent,
      assistantProvider,
      assistantAnthropicModel,
      assistantOpenaiBaseUrl,
      assistantOpenaiModel,
      assistantOllamaBaseUrl,
      assistantOllamaModel,
    ],
  );
}

function AssistantChat({ connection, dialect }: { connection: OpenConnection; dialect: Dialect }) {
  const connectionId = connection.info.connectionId;
  const conversation = useConversation(connectionId);
  const custom = useAgentsStore((s) => s.custom);
  const selectedId = useAgentsStore((s) => s.selectedId);
  const agent = useMemo(() => selectedAgent({ custom, selectedId }), [custom, selectedId]);
  const provider = useProviderSettings(agent);
  const keyStatus = useAssistantKeys((s) => s.status);
  const missingKey =
    providerNeedsKey(provider.provider) && hasKey(keyStatus, provider.provider) === false;
  const context = useContextRequest(connection);
  const qc = useQueryClient();
  const sql = useMemo<SqlActions>(
    () => ({
      insert: (text) => insertIntoEditor(connection, text),
      run: (text) => runInNewTab(qc, connection, text),
    }),
    [connection, qc],
  );
  const streaming = conversation.requestId !== null;

  useEffect(() => {
    void loadAgents();
    void refreshKeyStatus();
  }, []);

  const send = (text = conversation.draft) => {
    if (streaming || !text.trim()) return;
    void sendMessage(connectionId, text, { agent, provider, context });
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-scope="sidebar">
      <ChatHeader
        agent={agent}
        model={modelLabel(provider)}
        providerLabel={PROVIDER_LABELS[provider.provider]}
        canClear={conversation.messages.length > 0}
        onClear={() => clearConversation(connectionId)}
      />
      <MessageList
        messages={conversation.messages}
        dialect={dialect}
        sql={sql}
        empty={
          <EmptyChat
            connection={connection}
            table={context.table?.name ?? null}
            onPick={(text) => setDraft(connectionId, text)}
          />
        }
      />
      {missingKey && (
        <div className="border-border flex items-start gap-1.5 border-t px-2 py-1.5 text-[11px]">
          <TriangleAlert className="mt-px size-3 shrink-0 text-amber-600 dark:text-amber-400" />
          <span className="min-w-0 flex-1">
            Add your {PROVIDER_LABELS[provider.provider]} API key to use the assistant.{" "}
            <button
              type="button"
              className="text-primary hover:underline"
              onClick={openAssistantSettings}
            >
              Open Settings
            </button>
          </span>
        </div>
      )}
      <Composer
        draft={conversation.draft}
        streaming={streaming}
        context={context}
        onDraft={(text) => setDraft(connectionId, text)}
        onSend={() => send()}
        onStop={() => void stopReply(connectionId)}
      />
    </div>
  );
}

function ChatHeader({
  agent,
  model,
  providerLabel,
  canClear,
  onClear,
}: {
  agent: Agent;
  model: string;
  providerLabel: string;
  canClear: boolean;
  onClear: () => void;
}) {
  const custom = useAgentsStore((s) => s.custom);
  const select = useAgentsStore((s) => s.select);
  const remove = useAgentsStore((s) => s.remove);
  const [dialog, setDialog] = useState<{ agent: Agent | null; n: number } | null>(null);
  const agents = useMemo(() => allAgents(custom), [custom]);
  return (
    <div className="border-border flex h-7 shrink-0 items-center gap-1 border-b px-1">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="xs"
            className="h-5 max-w-[55%] min-w-0 gap-0.5 px-1.5 text-[11px]"
            aria-label="Agent"
          >
            <Bot className="size-3" />
            <span className="truncate">{agent.name}</span>
            <ChevronDown className="size-3 opacity-60" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="min-w-48">
          <DropdownMenuRadioGroup value={agent.id} onValueChange={select}>
            {agents.map((a) => (
              <DropdownMenuRadioItem key={a.id} value={a.id} className="text-xs">
                {a.name}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
          <DropdownMenuSeparator />
          <DropdownMenuItem
            className="text-xs"
            onSelect={() => setDialog({ agent: null, n: Date.now() })}
          >
            New Agent…
          </DropdownMenuItem>
          <DropdownMenuItem
            className="text-xs"
            onSelect={() => setDialog({ agent, n: Date.now() })}
          >
            {agent.builtIn ? `Duplicate “${agent.name}”…` : `Edit “${agent.name}”…`}
          </DropdownMenuItem>
          {!agent.builtIn && (
            <DropdownMenuItem
              variant="destructive"
              className="text-xs"
              onSelect={() => remove(agent.id)}
            >
              Delete “{agent.name}”
            </DropdownMenuItem>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
      <span
        className="text-muted-foreground min-w-0 flex-1 truncate text-[10px]"
        title={`${providerLabel} · ${model}`}
      >
        {model}
      </span>
      <Button
        variant="ghost"
        size="icon-xs"
        className="size-5"
        aria-label="Clear conversation"
        title="Clear conversation"
        disabled={!canClear}
        onClick={onClear}
      >
        <Eraser />
      </Button>
      <Button
        variant="ghost"
        size="icon-xs"
        className="size-5"
        aria-label="AI Assistant settings"
        title="AI Assistant settings"
        onClick={openAssistantSettings}
      >
        <Settings2 />
      </Button>
      {dialog && (
        <AgentDialog
          key={dialog.n}
          agent={dialog.agent}
          open
          onOpenChange={(open) => !open && setDialog(null)}
        />
      )}
    </div>
  );
}

function EmptyChat({
  connection,
  table,
  onPick,
}: {
  connection: OpenConnection;
  table: string | null;
  onPick: (text: string) => void;
}) {
  const ideas = table
    ? [
        `Explain the structure of ${table}`,
        `Write a query that joins ${table} with the tables it references`,
        `Suggest indexes for ${table}`,
      ]
    : [
        "Which tables hold customer data?",
        "Write a query for the 10 most recent rows of each table",
        "Draw the relationships between the tables",
      ];
  return (
    <div className="flex flex-col items-center gap-2 px-4 py-6 text-center">
      <Bot className="text-muted-foreground/60 size-6" />
      <p className="text-xs font-medium">Ask about {connection.info.name}</p>
      <p className="text-muted-foreground flex max-w-64 gap-1 text-left text-[11px] leading-4">
        <ShieldCheck className="mt-px size-3 shrink-0" />
        {STRUCTURE_ONLY_NOTE}
      </p>
      <div className="mt-1 flex w-full flex-col gap-1">
        {ideas.map((idea) => (
          <button
            key={idea}
            type="button"
            onClick={() => onPick(idea)}
            className="border-border hover:bg-muted rounded-md border px-2 py-1 text-left text-[11px]"
          >
            {idea}
          </button>
        ))}
      </div>
    </div>
  );
}

/** Keeps the list scrolled to the end while it grows, unless the user scrolled up. */
function MessageList({
  messages,
  dialect,
  sql,
  empty,
}: {
  messages: ChatMessage[];
  dialect: Dialect;
  sql: SqlActions;
  empty: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const last = messages[messages.length - 1];
  useLayoutEffect(() => {
    const el = ref.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [messages.length, last?.content, last?.status, last?.thinking]);
  return (
    <div
      ref={ref}
      role="log"
      aria-label="Assistant conversation"
      aria-live="polite"
      onScroll={(e) => {
        const el = e.currentTarget;
        stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
      }}
      className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-2 py-2 text-[12px] leading-5"
    >
      {messages.length === 0
        ? empty
        : messages.map((m) => <MessageView key={m.id} message={m} dialect={dialect} sql={sql} />)}
    </div>
  );
}

const FAILURE_TITLES: Record<AssistantFailure["kind"], string> = {
  missingKey: "API key needed",
  auth: "API key rejected",
  rateLimit: "Rate limited",
  network: "Network error",
  server: "Provider error",
  badRequest: "Request rejected",
  refusal: "Declined",
  config: "Check the AI Assistant settings",
  other: "Error",
};

const SETTINGS_FAILURES = new Set<AssistantFailure["kind"]>([
  "missingKey",
  "auth",
  "config",
  "badRequest",
]);

function FailureBox({ error }: { error: AssistantFailure }) {
  return (
    <div
      role="alert"
      className="border-destructive/30 bg-destructive/5 text-foreground flex flex-col gap-0.5 rounded-md border px-2 py-1.5 text-[11px]"
    >
      <span className="text-destructive flex items-center gap-1 font-medium">
        <TriangleAlert className="size-3" />
        {FAILURE_TITLES[error.kind]}
        {error.status ? ` (${error.status})` : ""}
      </span>
      <span className="selectable break-words">{error.message}</span>
      {SETTINGS_FAILURES.has(error.kind) && (
        <button
          type="button"
          className="text-primary self-start hover:underline"
          onClick={openAssistantSettings}
        >
          Open Settings
        </button>
      )}
    </div>
  );
}

function MessageView({
  message: m,
  dialect,
  sql,
}: {
  message: ChatMessage;
  dialect: Dialect;
  sql: SqlActions;
}) {
  if (m.role === "user") {
    return (
      <div className="bg-surface-2 dark:bg-accent/60 selectable self-end rounded-md px-2 py-1 break-words whitespace-pre-wrap">
        {m.content}
      </div>
    );
  }
  const streaming = m.status === "streaming";
  const note = stopNote(m.stopReason);
  return (
    <div className="group/msg flex min-w-0 flex-col gap-1.5">
      <div className="text-muted-foreground flex h-4 items-center gap-1 text-[10px]">
        <Bot className="size-3 shrink-0" />
        <span className="min-w-0 truncate">{m.agent}</span>
        {m.context && (
          <span className="min-w-0 truncate" title={`Sent with: ${m.context.summary}`}>
            · {m.context.summary}
          </span>
        )}
        <span className="flex-1" />
        {m.content && !streaming && (
          <Button
            variant="ghost"
            size="icon-xs"
            className="size-4 opacity-0 group-hover/msg:opacity-100 focus-visible:opacity-100"
            aria-label="Copy reply"
            title="Copy reply"
            onClick={() => copyText(m.content, "reply")}
          >
            <Copy className="size-2.5" />
          </Button>
        )}
      </div>
      {m.content && <Markdown text={m.content} dialect={dialect} streaming={streaming} sql={sql} />}
      {streaming && (m.thinking || !m.content) && (
        <span className="text-muted-foreground flex items-center gap-1 text-[11px]">
          <Loader2 className="size-3 animate-spin" />
          {m.thinking ? "Thinking…" : "Waiting for the reply…"}
        </span>
      )}
      {m.notices?.map((n, i) => (
        <p key={i} className="text-muted-foreground text-[11px]">
          {n}
        </p>
      ))}
      {m.context?.warning && (
        <p className="flex gap-1 text-[11px] text-amber-700 dark:text-amber-400">
          <TriangleAlert className="mt-0.5 size-3 shrink-0" />
          {m.context.warning}
        </p>
      )}
      {m.status === "stopped" && <p className="text-muted-foreground text-[11px]">Stopped.</p>}
      {note && <p className="text-muted-foreground text-[11px]">{note}</p>}
      {m.status === "error" && m.error && <FailureBox error={m.error} />}
    </div>
  );
}

/** "What is sent": the exact structure context of the next message. */
function ContextButton({ context }: { context: AssistantContextRequest }) {
  const [state, setState] = useState<
    | { kind: "loading" }
    | { kind: "ready"; preview: AssistantContextPreview }
    | { kind: "error"; message: string }
    | null
  >(null);
  const label = context.table
    ? context.table.name
    : context.schema
      ? `schema ${context.schema}`
      : "database";
  return (
    <Popover
      onOpenChange={(open) => {
        if (!open) return;
        setState({ kind: "loading" });
        ipc.assistant
          .context(context)
          .then((preview) => setState({ kind: "ready", preview }))
          .catch((err: unknown) => setState({ kind: "error", message: toIpcError(err).message }));
      }}
    >
      <PopoverTrigger asChild>
        <button
          type="button"
          className="text-muted-foreground hover:text-foreground flex min-w-0 items-center gap-1 text-[10px]"
          title="What is sent with your message"
        >
          <ShieldCheck className="size-3 shrink-0" />
          <span className="truncate">Structure of {label}</span>
          <Info className="size-2.5 shrink-0 opacity-60" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        side="top"
        align="start"
        className="flex w-96 max-w-[90vw] flex-col gap-2 p-3"
      >
        <p className="text-xs font-medium">Sent with each message</p>
        <p className="text-muted-foreground text-[11px] leading-4">{STRUCTURE_ONLY_NOTE}</p>
        {state?.kind === "loading" && (
          <span className="text-muted-foreground flex items-center gap-1 text-[11px]">
            <Loader2 className="size-3 animate-spin" /> Reading the structure…
          </span>
        )}
        {state?.kind === "error" && <p className="text-destructive text-[11px]">{state.message}</p>}
        {state?.kind === "ready" && (
          <>
            {state.preview.warning && (
              <p className="text-[11px] text-amber-700 dark:text-amber-400">
                {state.preview.warning}
              </p>
            )}
            <pre className="bg-surface-2 border-border selectable max-h-80 overflow-auto rounded-md border p-2 font-mono text-[10.5px] leading-4 whitespace-pre-wrap">
              {state.preview.text || "(nothing)"}
            </pre>
            <p className="text-muted-foreground text-[10px]">
              {state.preview.tables} tables listed · {state.preview.text.length.toLocaleString()}{" "}
              characters{state.preview.truncated ? " · cut to the size limit" : ""}
            </p>
          </>
        )}
      </PopoverContent>
    </Popover>
  );
}

function Composer({
  draft,
  streaming,
  context,
  onDraft,
  onSend,
  onStop,
}: {
  draft: string;
  streaming: boolean;
  context: AssistantContextRequest;
  onDraft: (text: string) => void;
  onSend: () => void;
  onStop: () => void;
}) {
  const sendKeys = formatCombo(SEND_COMBO);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  // "Ask Assistant…" (command palette) puts the caret here, also when the panel mounts for it.
  const focusRequest = useAssistantStore((s) => s.focusRequest);
  useEffect(() => {
    if (takeFocusRequest()) inputRef.current?.focus();
  }, [focusRequest]);
  return (
    <div className="border-border flex shrink-0 flex-col gap-1 border-t p-1.5">
      <textarea
        ref={inputRef}
        aria-label="Message the assistant"
        value={draft}
        rows={3}
        placeholder={`Ask about this database… (${sendKeys} to send)`}
        onChange={(e) => onDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !e.nativeEvent.isComposing) {
            e.preventDefault();
            onSend();
          }
        }}
        className={cn(
          "border-input bg-background dark:bg-input/30 placeholder:text-muted-foreground [field-sizing:content] max-h-48 min-h-14 w-full resize-none rounded-md border px-2 py-1.5 text-[12px] leading-5 outline-none",
          "focus-visible:border-ring focus-visible:ring-ring/50 focus-visible:ring-2",
        )}
      />
      <div className="flex items-center gap-1">
        <ContextButton context={context} />
        <span className="flex-1" />
        {streaming ? (
          <Button
            size="xs"
            variant="outline"
            className="h-6 px-2 text-[11px]"
            onClick={onStop}
            aria-label="Stop"
            title="Stop the reply"
          >
            <Square className="size-2.5 fill-current" />
            Stop
          </Button>
        ) : (
          <Button
            size="xs"
            className="h-6 px-2 text-[11px]"
            disabled={!draft.trim()}
            onClick={onSend}
            aria-label="Send"
            title={`Send (${sendKeys})`}
          >
            <ArrowUp className="size-3" />
            Send
          </Button>
        )}
      </div>
    </div>
  );
}
