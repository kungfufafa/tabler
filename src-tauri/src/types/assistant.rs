//! The AI Assistant tab of the right sidebar: provider settings, chat requests, the stream of
//! events a reply arrives in, and how a request failed. Only the STRUCTURE of the active
//! connection (names, types, DDL) is ever sent to a provider — never rows or cell values.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use super::schema::TableRef;

/// Where chat requests go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum AssistantProvider {
    /// The Claude API (`api.anthropic.com/v1/messages`).
    Anthropic,
    /// Any OpenAI-compatible `/chat/completions` endpoint: OpenAI, DeepSeek, LM Studio,
    /// Ollama's `/v1`.
    Openai,
    /// A local Ollama server's native `/api/chat` (no API key).
    Ollama,
}

impl AssistantProvider {
    /// Keychain account of the provider's API key (service `tabler`).
    pub fn key_account(self) -> &'static str {
        match self {
            AssistantProvider::Anthropic => "assistant:anthropic",
            AssistantProvider::Openai => "assistant:openai",
            AssistantProvider::Ollama => "assistant:ollama",
        }
    }
}

/// Provider, model and (OpenAI-compatible / Ollama) server of a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AssistantProviderConfig {
    pub provider: AssistantProvider,
    pub model: String,
    /// OpenAI-compatible: the API root (`https://api.openai.com/v1`); Ollama: the server
    /// (`http://localhost:11434`). Ignored for Anthropic.
    #[ts(optional = nullable)]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum AssistantRole {
    User,
    Assistant,
}

/// One turn of the conversation, as plain text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AssistantMessage {
    pub role: AssistantRole,
    pub content: String,
}

/// What the conversation is about: the backend reads the structure (never the rows) of
/// `schema` and `table` on the open connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AssistantContextRequest {
    pub connection_id: Uuid,
    /// PG schema / MySQL database / SQLite database; `None` = the connection's default.
    #[ts(optional = nullable)]
    pub schema: Option<String>,
    /// The table or view of the active tab, whose full DDL is sent.
    #[ts(optional = nullable)]
    pub table: Option<TableRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AssistantChatRequest {
    /// Renderer-chosen id; `assistant_cancel` stops the request with it.
    pub request_id: String,
    pub provider: AssistantProviderConfig,
    /// The agent's instructions (the database context is appended by the backend).
    pub system_prompt: String,
    /// The conversation so far, oldest first, ending with the user's new message.
    pub messages: Vec<AssistantMessage>,
    /// `None`: no database context.
    #[ts(optional = nullable)]
    pub context: Option<AssistantContextRequest>,
}

/// The structure that is (or would be) sent along with a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AssistantContextPreview {
    /// The exact context text appended to the system prompt.
    pub text: String,
    /// One-line summary for the chat ("public.orders + 42 tables").
    pub summary: String,
    /// Tables listed (after the cap).
    pub tables: u32,
    /// The table list or a DDL was cut to stay under the size cap.
    pub truncated: bool,
    /// Why the structure could not be read (the request goes without it).
    #[ts(optional = nullable)]
    pub warning: Option<String>,
}

/// Streamed while a reply arrives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export, rename_all_fields = "camelCase")]
pub enum AssistantEvent {
    /// The structure sent with the request (before the provider is called).
    Context { context: AssistantContextPreview },
    /// The model is reasoning (no text yet).
    Thinking,
    /// More reply text.
    Delta { text: String },
    /// A short notice to show with the reply (e.g. another model continued after a decline).
    Notice { message: String },
}

/// Why a request failed; shown inline in the chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AssistantFailureKind {
    /// No API key stored for a provider that needs one (the UI links to Settings).
    MissingKey,
    /// The key was rejected (401 / 403).
    Auth,
    /// 429: too many requests or tokens.
    RateLimit,
    /// The server could not be reached, or the stream broke.
    Network,
    /// 5xx / overloaded.
    Server,
    /// 400 / 404 / 422: bad model, bad request.
    BadRequest,
    /// The model declined the request.
    Refusal,
    /// Settings are incomplete or invalid (base URL, model).
    Config,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AssistantFailure {
    pub kind: AssistantFailureKind,
    pub message: String,
    /// HTTP status, when the server answered.
    #[ts(optional = nullable)]
    pub status: Option<u16>,
}

impl AssistantFailure {
    pub fn new(kind: AssistantFailureKind, message: impl Into<String>) -> AssistantFailure {
        AssistantFailure {
            kind,
            message: message.into(),
            status: None,
        }
    }
}

/// How a chat request ended. Failures are reported here (not as a command error) so the chat
/// can show them inline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AssistantChatResult {
    /// The provider's stop reason (`end_turn`, `max_tokens`, `stop`, `length`…).
    #[ts(optional = nullable)]
    pub stop_reason: Option<String>,
    /// Stopped by `assistant_cancel`.
    pub cancelled: bool,
    #[ts(optional = nullable)]
    pub error: Option<AssistantFailure>,
}

/// Which providers have an API key stored (the keys themselves never reach the renderer).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AssistantKeyStatus {
    pub anthropic: bool,
    pub openai: bool,
}

/// Settings ▸ AI Assistant ▸ Test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AssistantTestResult {
    pub ok: bool,
    pub message: String,
    /// Models the server offers (OpenAI-compatible and Ollama), for the model field.
    pub models: Vec<String>,
}
