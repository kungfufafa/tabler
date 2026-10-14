//! HTTP requests of each provider, as plain data (URL, headers, JSON body) so they can be tested
//! without a network. A request is built from the agent's instructions, the conversation (plain
//! text the user typed and the replies) and the rendered STRUCTURE context — there is no input
//! here that could carry a row or a cell value.

use serde_json::{Value as Json, json};

use crate::types::assistant::{
    AssistantMessage, AssistantProvider, AssistantProviderConfig, AssistantRole,
};

pub const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
pub const ANTHROPIC_MODELS_URL: &str = "https://api.anthropic.com/v1/models";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Beta of the server-side refusal fallback in its `"default"` form.
pub const ANTHROPIC_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// The default Claude model.
pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-opus-5-5";
/// Output cap of a streamed reply.
pub const ANTHROPIC_MAX_TOKENS: u32 = 64_000;

pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434";

/// What Tabler knows about a Claude model's request surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaudeModel {
    pub id: &'static str,
    /// Accepts `thinking: {type: "adaptive"}` and `output_config.effort`.
    pub adaptive: bool,
    /// Accepts `fallbacks: "default"` (server-side refusal fallback).
    pub fallbacks: bool,
}

/// The models the settings offer (the renderer's list mirrors it).
pub const CLAUDE_MODELS: &[ClaudeModel] = &[
    ClaudeModel {
        id: "claude-opus-5-5",
        adaptive: true,
        fallbacks: true,
    },
    ClaudeModel {
        id: "claude-sonnet-5-5",
        adaptive: true,
        fallbacks: true,
    },
    ClaudeModel {
        id: "claude-haiku-5-5",
        adaptive: true,
        fallbacks: false,
    },
    ClaudeModel {
        id: "claude-fable-5-1",
        adaptive: true,
        fallbacks: true,
    },
    ClaudeModel {
        id: "claude-opus-5",
        adaptive: true,
        fallbacks: true,
    },
    ClaudeModel {
        id: "claude-sonnet-5",
        adaptive: true,
        fallbacks: false,
    },
    ClaudeModel {
        id: "claude-opus-4-8",
        adaptive: true,
        fallbacks: false,
    },
    ClaudeModel {
        id: "claude-haiku-4-5",
        adaptive: false,
        fallbacks: false,
    },
];

pub fn claude_model(id: &str) -> Option<ClaudeModel> {
    CLAUDE_MODELS.iter().copied().find(|m| m.id == id)
}

/// A request ready to send.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Json,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// The system prompt: the agent's instructions, the house rules, then the database context.
pub fn system_text(agent_prompt: &str, context: Option<&str>) -> String {
    let mut out = String::new();
    let agent = agent_prompt.trim();
    if !agent.is_empty() {
        out.push_str(agent);
        out.push_str("\n\n");
    }
    out.push_str(
        "You are the assistant inside Tabler, a desktop database client. Answer about the \
         database described below. Write SQL for its dialect inside ```sql fenced code blocks, one \
         statement per block unless the user asks for a script. You only see the structure \
         (tables, columns, DDL), never the data.",
    );
    if let Some(ctx) = context.filter(|c| !c.trim().is_empty()) {
        out.push_str("\n\n");
        out.push_str(ctx);
    }
    out
}

/// The conversation without empty turns (a stopped reply may have no text), starting with a
/// user turn.
fn turns(messages: &[AssistantMessage]) -> impl Iterator<Item = &AssistantMessage> {
    messages
        .iter()
        .filter(|m| !m.content.trim().is_empty())
        .skip_while(|m| m.role != AssistantRole::User)
}

fn role(r: AssistantRole) -> &'static str {
    match r {
        AssistantRole::User => "user",
        AssistantRole::Assistant => "assistant",
    }
}

/// Validates and normalizes a base URL: http(s), no trailing slash.
pub fn base_url(raw: Option<&str>, default: &str) -> Result<String, String> {
    let raw = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(default);
    let parsed = reqwest::Url::parse(raw).map_err(|e| format!("Invalid base URL “{raw}”: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(format!(
            "The base URL must start with http:// or https:// (“{raw}”)"
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(format!(
            "The base URL cannot have a query or fragment (“{raw}”)"
        ));
    }
    Ok(raw.trim_end_matches('/').to_string())
}

fn model_of(config: &AssistantProviderConfig) -> Result<String, String> {
    let model = config.model.trim();
    if model.is_empty() {
        return Err("Choose a model in Settings ▸ AI Assistant".into());
    }
    if model.len() > 200 || model.chars().any(char::is_control) {
        return Err("The model name is not valid".into());
    }
    Ok(model.to_string())
}

/// Claude API Messages request, streamed.
pub fn anthropic(
    config: &AssistantProviderConfig,
    api_key: &str,
    system: &str,
    messages: &[AssistantMessage],
) -> Result<HttpRequest, String> {
    let model = model_of(config)?;
    let known = claude_model(&model);
    let messages: Vec<Json> = turns(messages)
        .map(|m| json!({ "role": role(m.role), "content": m.content }))
        .collect();
    let mut body = json!({
        "model": model,
        "max_tokens": ANTHROPIC_MAX_TOKENS,
        "stream": true,
        "system": system,
        "messages": messages,
        // Caches the system prompt and history between turns (no-op when too short).
        "cache_control": { "type": "ephemeral" },
    });
    let mut headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("x-api-key".to_string(), api_key.to_string()),
        (
            "anthropic-version".to_string(),
            ANTHROPIC_VERSION.to_string(),
        ),
    ];
    if let Some(m) = known {
        if m.adaptive {
            body["thinking"] = json!({ "type": "adaptive" });
            // A chat answer: moderate depth keeps replies quick.
            body["output_config"] = json!({ "effort": "medium" });
        }
        if m.fallbacks {
            body["fallbacks"] = json!("default");
            headers.push((
                "anthropic-beta".to_string(),
                ANTHROPIC_FALLBACK_BETA.to_string(),
            ));
        }
    }
    Ok(HttpRequest {
        url: ANTHROPIC_URL.to_string(),
        headers,
        body,
    })
}

/// OpenAI-compatible `/chat/completions`, streamed. The key is optional (local servers).
pub fn openai(
    config: &AssistantProviderConfig,
    api_key: Option<&str>,
    system: &str,
    messages: &[AssistantMessage],
) -> Result<HttpRequest, String> {
    let model = model_of(config)?;
    let base = base_url(config.base_url.as_deref(), DEFAULT_OPENAI_BASE_URL)?;
    let mut all = vec![json!({ "role": "system", "content": system })];
    all.extend(turns(messages).map(|m| json!({ "role": role(m.role), "content": m.content })));
    let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
    if let Some(key) = api_key.filter(|k| !k.is_empty()) {
        headers.push(("authorization".to_string(), format!("Bearer {key}")));
    }
    Ok(HttpRequest {
        url: format!("{base}/chat/completions"),
        headers,
        body: json!({ "model": model, "stream": true, "messages": all }),
    })
}

/// Ollama's native `/api/chat`, streamed as JSON lines. No key.
pub fn ollama(
    config: &AssistantProviderConfig,
    system: &str,
    messages: &[AssistantMessage],
) -> Result<HttpRequest, String> {
    let model = model_of(config)?;
    let base = base_url(config.base_url.as_deref(), DEFAULT_OLLAMA_BASE_URL)?;
    let mut all = vec![json!({ "role": "system", "content": system })];
    all.extend(turns(messages).map(|m| json!({ "role": role(m.role), "content": m.content })));
    Ok(HttpRequest {
        url: format!("{base}/api/chat"),
        headers: vec![("content-type".to_string(), "application/json".to_string())],
        body: json!({ "model": model, "stream": true, "messages": all }),
    })
}

/// The chat request of `config.provider`.
pub fn build(
    config: &AssistantProviderConfig,
    api_key: Option<&str>,
    system: &str,
    messages: &[AssistantMessage],
) -> Result<HttpRequest, String> {
    match config.provider {
        AssistantProvider::Anthropic => anthropic(config, api_key.unwrap_or(""), system, messages),
        AssistantProvider::Openai => openai(config, api_key, system, messages),
        AssistantProvider::Ollama => ollama(config, system, messages),
    }
}

/// The GET that checks a provider's settings (Settings ▸ AI Assistant ▸ Test): the model's
/// entry (Anthropic) or the server's model list (OpenAI-compatible, Ollama).
pub fn test_request(
    config: &AssistantProviderConfig,
    api_key: Option<&str>,
) -> Result<HttpRequest, String> {
    match config.provider {
        AssistantProvider::Anthropic => {
            let model = model_of(config)?;
            Ok(HttpRequest {
                url: format!("{ANTHROPIC_MODELS_URL}/{model}"),
                headers: vec![
                    ("x-api-key".to_string(), api_key.unwrap_or("").to_string()),
                    (
                        "anthropic-version".to_string(),
                        ANTHROPIC_VERSION.to_string(),
                    ),
                ],
                body: Json::Null,
            })
        }
        AssistantProvider::Openai => {
            let base = base_url(config.base_url.as_deref(), DEFAULT_OPENAI_BASE_URL)?;
            let mut headers = Vec::new();
            if let Some(key) = api_key.filter(|k| !k.is_empty()) {
                headers.push(("authorization".to_string(), format!("Bearer {key}")));
            }
            Ok(HttpRequest {
                url: format!("{base}/models"),
                headers,
                body: Json::Null,
            })
        }
        AssistantProvider::Ollama => {
            let base = base_url(config.base_url.as_deref(), DEFAULT_OLLAMA_BASE_URL)?;
            Ok(HttpRequest {
                url: format!("{base}/api/tags"),
                headers: Vec::new(),
                body: Json::Null,
            })
        }
    }
}

/// Model ids in a `/models` (OpenAI: `data[].id`) or `/api/tags` (Ollama: `models[].name`)
/// answer.
pub fn model_list(provider: AssistantProvider, body: &Json) -> Vec<String> {
    let (list, field) = match provider {
        AssistantProvider::Ollama => (body.get("models"), "name"),
        _ => (body.get("data"), "id"),
    };
    let mut out: Vec<String> = list
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|m| m.get(field).and_then(Json::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(
        provider: AssistantProvider,
        model: &str,
        base: Option<&str>,
    ) -> AssistantProviderConfig {
        AssistantProviderConfig {
            provider,
            model: model.into(),
            base_url: base.map(str::to_string),
        }
    }

    fn convo() -> Vec<AssistantMessage> {
        vec![
            AssistantMessage {
                role: AssistantRole::Assistant,
                content: "Hi! (greeting before any user turn is dropped)".into(),
            },
            AssistantMessage {
                role: AssistantRole::User,
                content: "Top customers?".into(),
            },
            AssistantMessage {
                role: AssistantRole::Assistant,
                content: "   ".into(),
            },
            AssistantMessage {
                role: AssistantRole::User,
                content: "By revenue please".into(),
            },
        ]
    }

    #[test]
    fn anthropic_request_shape() {
        let req = anthropic(
            &cfg(AssistantProvider::Anthropic, "claude-opus-5-5", None),
            "sk-ant-test",
            "SYSTEM",
            &convo(),
        )
        .unwrap();
        assert_eq!(req.url, "https://api.anthropic.com/v1/messages");
        assert_eq!(req.header("x-api-key"), Some("sk-ant-test"));
        assert_eq!(req.header("anthropic-version"), Some("2023-06-01"));
        assert_eq!(
            req.header("anthropic-beta"),
            Some("server-side-fallback-2026-07-01")
        );
        assert_eq!(req.header("authorization"), None);
        let b = &req.body;
        assert_eq!(b["model"], "claude-opus-5-5");
        assert_eq!(b["stream"], true);
        assert_eq!(b["max_tokens"], 64_000);
        assert_eq!(b["system"], "SYSTEM");
        assert_eq!(b["thinking"]["type"], "adaptive");
        assert_eq!(b["output_config"]["effort"], "medium");
        assert_eq!(b["fallbacks"], "default");
        let msgs = b["messages"].as_array().unwrap();
        assert_eq!(
            msgs.len(),
            2,
            "leading assistant and blank turns are dropped"
        );
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["content"], "By revenue please");
        assert!(b.get("temperature").is_none());
    }

    #[test]
    fn anthropic_model_capabilities() {
        let haiku45 = anthropic(
            &cfg(AssistantProvider::Anthropic, "claude-haiku-4-5", None),
            "k",
            "s",
            &convo(),
        )
        .unwrap();
        assert!(haiku45.body.get("thinking").is_none());
        assert!(haiku45.body.get("output_config").is_none());
        assert!(haiku45.body.get("fallbacks").is_none());
        assert_eq!(haiku45.header("anthropic-beta"), None);

        let haiku55 = anthropic(
            &cfg(AssistantProvider::Anthropic, "claude-haiku-5-5", None),
            "k",
            "s",
            &convo(),
        )
        .unwrap();
        assert_eq!(haiku55.body["thinking"]["type"], "adaptive");
        assert!(
            haiku55.body.get("fallbacks").is_none(),
            "no fallback for Haiku 5.5"
        );

        let sonnet = anthropic(
            &cfg(AssistantProvider::Anthropic, "claude-sonnet-5-5", None),
            "k",
            "s",
            &convo(),
        )
        .unwrap();
        assert_eq!(sonnet.body["fallbacks"], "default");

        // An unknown id is sent as typed, without model-specific options.
        let other = anthropic(
            &cfg(AssistantProvider::Anthropic, "claude-future", None),
            "k",
            "s",
            &convo(),
        )
        .unwrap();
        assert_eq!(other.body["model"], "claude-future");
        assert!(other.body.get("thinking").is_none());
        assert!(other.body.get("fallbacks").is_none());

        assert!(
            anthropic(
                &cfg(AssistantProvider::Anthropic, " ", None),
                "k",
                "s",
                &convo()
            )
            .is_err()
        );
        assert_eq!(
            claude_model(DEFAULT_ANTHROPIC_MODEL).map(|m| m.id),
            Some("claude-opus-5-5")
        );
    }

    #[test]
    fn openai_request_shape() {
        let req = openai(
            &cfg(
                AssistantProvider::Openai,
                "deepseek-chat",
                Some("https://api.deepseek.com/v1/"),
            ),
            Some("sk-test"),
            "SYSTEM",
            &convo(),
        )
        .unwrap();
        assert_eq!(req.url, "https://api.deepseek.com/v1/chat/completions");
        assert_eq!(req.header("authorization"), Some("Bearer sk-test"));
        assert_eq!(req.header("x-api-key"), None);
        let msgs = req.body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "SYSTEM");
        assert_eq!(msgs.len(), 3);
        assert_eq!(req.body["stream"], true);
        assert_eq!(req.body["model"], "deepseek-chat");

        // LM Studio: no key, default base when empty.
        let local = openai(
            &cfg(
                AssistantProvider::Openai,
                "qwen",
                Some("http://localhost:1234/v1"),
            ),
            None,
            "s",
            &convo(),
        )
        .unwrap();
        assert_eq!(local.header("authorization"), None);
        assert_eq!(local.url, "http://localhost:1234/v1/chat/completions");
        let default = openai(&cfg(AssistantProvider::Openai, "m", None), None, "s", &[]).unwrap();
        assert_eq!(default.url, "https://api.openai.com/v1/chat/completions");
    }

    #[test]
    fn ollama_request_shape() {
        let req = ollama(
            &cfg(AssistantProvider::Ollama, "llama3.2", None),
            "SYSTEM",
            &convo(),
        )
        .unwrap();
        assert_eq!(req.url, "http://localhost:11434/api/chat");
        assert_eq!(req.header("authorization"), None);
        assert_eq!(req.body["messages"][0]["role"], "system");
        assert_eq!(req.body["model"], "llama3.2");
    }

    #[test]
    fn base_urls_are_validated() {
        assert_eq!(
            base_url(Some(" http://127.0.0.1:11434/ "), "x").unwrap(),
            "http://127.0.0.1:11434"
        );
        assert!(base_url(Some("ftp://host"), "x").is_err());
        assert!(base_url(Some("localhost:11434"), "x").is_err());
        assert!(base_url(Some("https://h/v1?x=1"), "x").is_err());
        assert_eq!(
            base_url(None, DEFAULT_OPENAI_BASE_URL).unwrap(),
            DEFAULT_OPENAI_BASE_URL
        );
    }

    #[test]
    fn test_requests_and_model_lists() {
        let a = test_request(
            &cfg(AssistantProvider::Anthropic, "claude-sonnet-5-5", None),
            Some("k"),
        )
        .unwrap();
        assert_eq!(
            a.url,
            "https://api.anthropic.com/v1/models/claude-sonnet-5-5"
        );
        assert_eq!(a.header("anthropic-version"), Some("2023-06-01"));
        let o = test_request(&cfg(AssistantProvider::Ollama, "", None), None).unwrap();
        assert_eq!(o.url, "http://localhost:11434/api/tags");

        let openai_models = json!({ "data": [{ "id": "b" }, { "id": "a" }, { "id": "a" }] });
        assert_eq!(
            model_list(AssistantProvider::Openai, &openai_models),
            ["a", "b"]
        );
        let tags = json!({ "models": [{ "name": "llama3.2:latest" }] });
        assert_eq!(
            model_list(AssistantProvider::Ollama, &tags),
            ["llama3.2:latest"]
        );
        assert!(model_list(AssistantProvider::Ollama, &json!({})).is_empty());
    }

    #[test]
    fn system_text_puts_the_context_last() {
        let s = system_text("Be brief.", Some("<database_context>x</database_context>"));
        assert!(s.starts_with("Be brief.\n\n"));
        assert!(s.ends_with("<database_context>x</database_context>"));
        assert!(!system_text("", None).contains("database_context"));
    }
}
