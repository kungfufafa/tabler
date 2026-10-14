import type { AssistantChatRequest } from "../bindings/AssistantChatRequest";
import type { AssistantChatResult } from "../bindings/AssistantChatResult";
import type { AssistantContextPreview } from "../bindings/AssistantContextPreview";
import type { AssistantContextRequest } from "../bindings/AssistantContextRequest";
import type { AssistantEvent } from "../bindings/AssistantEvent";
import type { AssistantProvider } from "../bindings/AssistantProvider";
import type { AssistantProviderConfig } from "../bindings/AssistantProviderConfig";
import type { AssistantTestResult } from "../bindings/AssistantTestResult";
import type { CompletionMetadata } from "../bindings/CompletionMetadata";
import type { ConnectionInfo } from "../bindings/ConnectionInfo";
import type { TableSummary } from "../bindings/TableSummary";
import { mockError, mockInvoke, registerMock } from "./transport";

/**
 * AI Assistant mock: keys in memory (an Anthropic key is preset so the chat works in
 * `pnpm dev:mock`), the structure context built from the mock schema (as the backend does:
 * names and DDL only), and a canned Markdown reply streamed in small chunks. A message
 * containing "rate limit" or "offline" demonstrates the inline errors.
 */

const keys = new Map<AssistantProvider, string>([["anthropic", "sk-ant-mock"]]);
const running = new Map<string, { cancelled: boolean }>();
/** Every chat request received (tests check what the renderer sends). */
export const mockAssistantRequests: AssistantChatRequest[] = [];

const isTest = import.meta.env.MODE === "test";
let chunkDelay = isTest ? 0 : 18;
/** Test helper: per-chunk delay of the streamed reply. */
export function setMockAssistantDelay(ms: number): void {
  chunkDelay = ms;
}

export function resetMockAssistant(): void {
  keys.clear();
  keys.set("anthropic", "sk-ant-mock");
  running.clear();
  mockAssistantRequests.length = 0;
  chunkDelay = isTest ? 0 : 18;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function contextFor(req: AssistantContextRequest): Promise<AssistantContextPreview> {
  const connections = await mockInvoke<ConnectionInfo[]>("list_connections", {});
  const info = connections.find((c) => c.connectionId === req.connectionId);
  if (!info) {
    return {
      text: "",
      summary: "no database context",
      tables: 0,
      truncated: false,
      warning: "Could not read the structure: not connected",
    };
  }
  const meta = await mockInvoke<CompletionMetadata>("get_completion_metadata", {
    connectionId: req.connectionId,
  });
  const schema =
    req.schema ??
    req.table?.schema ??
    meta.defaultSchema ??
    (info.driver === "sqlite" ? "main" : null);
  let tables: TableSummary[] = [];
  const warnings: string[] = [];
  if (schema) {
    try {
      tables = await mockInvoke<TableSummary[]>("list_tables", {
        connectionId: req.connectionId,
        schema,
      });
    } catch (err) {
      warnings.push(String((err as { message?: string }).message ?? err));
    }
  }
  let ddl: string | null = null;
  if (req.table) {
    try {
      ddl = await mockInvoke<string>("get_table_ddl", {
        connectionId: req.connectionId,
        table: req.table,
      });
    } catch (err) {
      warnings.push(String((err as { message?: string }).message ?? err));
    }
  }
  const engine =
    info.driver === "postgres" ? "PostgreSQL" : info.driver === "mysql" ? "MySQL" : "SQLite";
  const columns = (name: string) => (schema ? (meta.schemas[schema]?.[name] ?? []) : []);
  const lines = [
    "<database_context>",
    `Engine: ${engine} — server version ${info.serverVersion} (SQL dialect: ${info.driver})`,
  ];
  if (info.currentDatabase) lines.push(`Current database: ${info.currentDatabase}`);
  if (schema) lines.push(`Current schema: ${schema}`);
  if (req.table && ddl) {
    lines.push("", `The user is looking at the table ${req.table.name}:`, "```sql", ddl, "```");
  }
  if (tables.length > 0) {
    lines.push("", `Tables and views in ${schema} (${tables.length}), with their columns:`);
    for (const t of tables) {
      const kind = t.kind === "table" ? "" : ` [${t.kind}]`;
      const cols = columns(t.name);
      lines.push(`- ${t.name}${kind}${cols.length ? `: ${cols.join(", ")}` : ""}`);
    }
  }
  lines.push("</database_context>");
  return {
    text: lines.join("\n"),
    summary: req.table
      ? `${req.table.name} + ${tables.length} tables`
      : `${tables.length} tables of ${schema ?? "schema"}`,
    tables: tables.length,
    truncated: false,
    warning: warnings.length
      ? `Could not read part of the structure (${warnings.join("; ")})`
      : null,
  };
}

/** A plausible reply for the question, using the context's tables and columns. */
function cannedReply(req: AssistantChatRequest, context: AssistantContextPreview | null): string {
  const tableLines = (context?.text ?? "").split("\n").filter((l) => l.startsWith("- "));
  const pick = req.context?.table?.name ?? tableLines[0]?.slice(2).split(/[: [\]]/)[0] ?? "users";
  const line = tableLines.find((l) => l.startsWith(`- ${pick}`)) ?? "";
  const cols = line.includes(":")
    ? line
        .split(":")[1]!
        .split(",")
        .map((c) => c.trim())
    : ["id"];
  const shown = cols.slice(0, 4).join(", ");
  const agent = req.systemPrompt.includes("EXPLAIN")
    ? "optimize"
    : req.systemPrompt.includes("Explain what it does")
      ? "explain"
      : "sql";
  if (agent === "explain") {
    return [
      `This query reads **${pick}** and returns one row per record:`,
      "",
      `1. It scans \`${pick}\` (no filter, so every row is read).`,
      `2. It keeps the columns ${cols
        .slice(0, 3)
        .map((c) => `\`${c}\``)
        .join(", ")}.`,
      "3. Rows come back in no particular order — add `ORDER BY` if order matters.",
      "",
      "> Only the structure was sent, so I can't say how many rows that is.",
    ].join("\n");
  }
  if (agent === "optimize") {
    return [
      `Without an \`EXPLAIN\` plan I can only guess from the structure of **${pick}**.`,
      "",
      "Most likely fixes:",
      "",
      `- Index the column you filter on, e.g. \`${cols[1] ?? cols[0]}\`:`,
      "",
      "```sql",
      `CREATE INDEX ${pick}_${cols[1] ?? cols[0]}_idx ON ${pick} (${cols[1] ?? cols[0]});`,
      "```",
      "",
      "- Select only the columns you need instead of `*`.",
      "",
      "Run **Explain** in the SQL editor and paste the plan here for a precise answer.",
    ].join("\n");
  }
  return [
    `Here is a query for **${pick}**:`,
    "",
    "```sql",
    `SELECT ${shown}`,
    `FROM ${pick}`,
    `ORDER BY ${cols[0]} DESC`,
    "LIMIT 10;",
    "```",
    "",
    "| column | note |",
    "|---|---|",
    ...cols.slice(0, 3).map((c, i) => `| \`${c}\` | ${i === 0 ? "sort key" : "shown"} |`),
    "",
    "Use **Run** to try it in a new tab, or **Insert** to add it to the editor.",
  ].join("\n");
}

registerMock("assistant_chat", async ({ req, onEvent }): Promise<AssistantChatResult> => {
  const request = req as AssistantChatRequest;
  const channel = onEvent as { send?: (e: AssistantEvent) => void } | undefined;
  const emit = (e: AssistantEvent) => channel?.send?.(e);
  if (running.has(request.requestId)) mockError("INVALID_INPUT", "request already running");
  mockAssistantRequests.push(structuredClone(request));
  const state = { cancelled: false };
  running.set(request.requestId, state);
  try {
    const context = request.context ? await contextFor(request.context) : null;
    if (context) emit({ type: "context", context });
    const provider = request.provider.provider;
    if (provider === "anthropic" && !keys.has("anthropic")) {
      return {
        cancelled: false,
        error: {
          kind: "missingKey",
          message: "Add your Anthropic API key in Settings ▸ AI Assistant",
        },
      };
    }
    const question = (request.messages[request.messages.length - 1]?.content ?? "").toLowerCase();
    if (question.includes("rate limit")) {
      return {
        cancelled: false,
        error: {
          kind: "rateLimit",
          message: "Rate limited: Number of requests exceeded. Try again in 12 s.",
          status: 429,
        },
      };
    }
    if (question.includes("offline")) {
      return {
        cancelled: false,
        error: { kind: "network", message: "Could not connect to api.anthropic.com." },
      };
    }
    emit({ type: "thinking" });
    await sleep(chunkDelay * 15);
    const reply = cannedReply(request, context);
    for (let i = 0; i < reply.length;) {
      if (state.cancelled) return { cancelled: true };
      const n = 3 + ((i * 7) % 6);
      emit({ type: "delta", text: reply.slice(i, i + n) });
      i += n;
      if (chunkDelay > 0) await sleep(chunkDelay);
      else await Promise.resolve();
    }
    return { cancelled: false, stopReason: provider === "anthropic" ? "end_turn" : "stop" };
  } finally {
    running.delete(request.requestId);
  }
});

registerMock("assistant_cancel", ({ requestId }) => {
  const state = running.get(requestId as string);
  if (!state) return false;
  state.cancelled = true;
  return true;
});

registerMock("assistant_context", ({ context }) => contextFor(context as AssistantContextRequest));

registerMock("assistant_key_status", () => ({
  anthropic: keys.has("anthropic"),
  openai: keys.has("openai"),
}));

registerMock("assistant_set_key", ({ provider, key }) => {
  const p = provider as AssistantProvider;
  if (p === "ollama") mockError("INVALID_INPUT", "Ollama does not use an API key");
  const k = typeof key === "string" ? key.trim() : "";
  if (k) keys.set(p, k);
  else keys.delete(p);
  return null;
});

const MOCK_MODELS: Record<"openai" | "ollama", string[]> = {
  openai: ["deepseek-chat", "deepseek-reasoner", "gpt-oss-20b"],
  ollama: ["llama3.2:latest", "qwen2.5-coder:7b", "sqlcoder:15b"],
};

registerMock("assistant_test", ({ config }): AssistantTestResult => {
  const c = config as AssistantProviderConfig;
  if (c.provider === "anthropic") {
    if (!keys.has("anthropic")) return { ok: false, message: "No API key stored", models: [] };
    return { ok: true, message: `Connected — ${c.model} is available`, models: [] };
  }
  const models = MOCK_MODELS[c.provider];
  const model = c.model.trim();
  return {
    ok: model !== "",
    message: !model
      ? `Connected — ${models.length} models; choose one`
      : models.includes(model)
        ? `Connected — ${model} is available`
        : `Connected, but ${model} is not in the server's model list`,
    models,
  };
});
