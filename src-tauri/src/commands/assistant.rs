//! The AI Assistant tab (right sidebar): chat, cancel, the structure context preview, API keys
//! and the provider test. Logic lives in `crate::assistant`.

use tauri::State;
use tauri::ipc::Channel;

use crate::assistant::{self, keys::KeyStore};
use crate::error::AppError;
use crate::state::AppState;
use crate::types::assistant::{
    AssistantChatRequest, AssistantChatResult, AssistantContextPreview, AssistantContextRequest,
    AssistantEvent, AssistantKeyStatus, AssistantProvider, AssistantProviderConfig,
    AssistantTestResult,
};
use crate::types::caps::Feature;

fn key_store(state: &AppState) -> KeyStore {
    KeyStore::new(state.secrets.backend(), state.appdb.clone())
}

/// Sends the conversation with the structure of `req.context` and streams the reply on
/// `on_event`. Provider failures come back in the result (shown inline), not as errors.
#[tauri::command]
pub async fn assistant_chat(
    state: State<'_, AppState>,
    req: AssistantChatRequest,
    on_event: Channel<AssistantEvent>,
) -> Result<AssistantChatResult, AppError> {
    assistant_chat_on(&state, req, on_event).await
}

/// Refuses the structure context of a connection whose session does not offer the assistant
/// (and so a chat that would send it). A connection that is not open gives an empty context
/// with a warning, as before.
async fn require_assistant(
    state: &AppState,
    context: &AssistantContextRequest,
) -> Result<(), AppError> {
    match state.connection(context.connection_id).await {
        Ok(conn) => conn.caps().await.require(Feature::Assistant),
        Err(_) => Ok(()),
    }
}

/// The body of [`assistant_chat`], callable without a Tauri runtime.
pub(crate) async fn assistant_chat_on(
    state: &AppState,
    req: AssistantChatRequest,
    on_event: Channel<AssistantEvent>,
) -> Result<AssistantChatResult, AppError> {
    if let Some(context) = &req.context {
        require_assistant(state, context).await?;
    }
    let running = assistant::register(&req.request_id)?;
    let emit = move |event: AssistantEvent| {
        if let Err(e) = on_event.send(event) {
            log::debug!("assistant event not delivered: {e}");
        }
    };
    let cancelled = AssistantChatResult {
        stop_reason: None,
        cancelled: true,
        error: None,
    };
    let context = match &req.context {
        Some(c) => tokio::select! {
            biased;
            _ = running.token.cancelled() => return Ok(cancelled),
            preview = assistant::context_for(state, c) => Some(preview),
        },
        None => None,
    };
    if let Some(preview) = &context {
        emit(AssistantEvent::Context {
            context: preview.clone(),
        });
    }
    let http = match assistant::prepare(&key_store(state), &req, context.as_ref()).await {
        Ok(r) => r,
        Err(failure) => {
            return Ok(AssistantChatResult {
                stop_reason: None,
                cancelled: false,
                error: Some(failure),
            });
        }
    };
    log::info!(
        "assistant: {:?} {} ({} messages, context {} chars)",
        req.provider.provider,
        req.provider.model,
        req.messages.len(),
        context.as_ref().map_or(0, |c| c.text.len())
    );
    Ok(assistant::stream_reply(req.provider.provider, http, &emit, &running.token).await)
}

/// Stops a running chat request; false when it already ended.
#[tauri::command]
pub fn assistant_cancel(request_id: String) -> bool {
    assistant::cancel(&request_id)
}

/// The structure that a request with this context would send ("What is sent").
#[tauri::command]
pub async fn assistant_context(
    state: State<'_, AppState>,
    context: AssistantContextRequest,
) -> Result<AssistantContextPreview, AppError> {
    assistant_context_on(&state, &context).await
}

/// The body of [`assistant_context`], callable without a Tauri runtime.
pub(crate) async fn assistant_context_on(
    state: &AppState,
    context: &AssistantContextRequest,
) -> Result<AssistantContextPreview, AppError> {
    require_assistant(state, context).await?;
    Ok(assistant::context_for(state, context).await)
}

#[tauri::command]
pub async fn assistant_key_status(
    state: State<'_, AppState>,
) -> Result<AssistantKeyStatus, AppError> {
    let keys = key_store(&state);
    Ok(AssistantKeyStatus {
        anthropic: keys.get(AssistantProvider::Anthropic).await?.is_some(),
        openai: keys.get(AssistantProvider::Openai).await?.is_some(),
    })
}

/// Stores a provider's API key in the Keychain; `None` / blank removes it.
#[tauri::command]
pub async fn assistant_set_key(
    state: State<'_, AppState>,
    provider: AssistantProvider,
    key: Option<String>,
) -> Result<(), AppError> {
    if provider == AssistantProvider::Ollama {
        return Err(AppError::InvalidInput(
            "Ollama does not use an API key".into(),
        ));
    }
    key_store(&state).set(provider, key.as_deref()).await
}

#[tauri::command]
pub async fn assistant_test(
    state: State<'_, AppState>,
    config: AssistantProviderConfig,
) -> Result<AssistantTestResult, AppError> {
    Ok(assistant::test_provider(&key_store(&state), &config).await)
}
