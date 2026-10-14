//! Opening Tabler from outside (see `crate::deeplink`): the renderer takes the queued URLs and
//! files, and records which profiles are temporary (opened from a URL, not saved).

use tauri::State;
use uuid::Uuid;

use crate::deeplink::{self, OpenRequest};
use crate::error::AppError;
use crate::state::AppState;

/// The URLs and files waiting to be opened (oldest first); each is handed out once.
#[tauri::command]
pub async fn take_pending_opens() -> Result<Vec<OpenRequest>, AppError> {
    Ok(deeplink::take())
}

/// Marks a profile as temporary (deleted at the next start unless saved) or as saved.
#[tauri::command]
pub async fn mark_temporary_profile(
    state: State<'_, AppState>,
    profile_id: Uuid,
    temporary: bool,
) -> Result<(), AppError> {
    deeplink::mark_temporary(&state.appdb, profile_id, temporary)
}

/// The profiles currently marked temporary.
#[tauri::command]
pub async fn list_temporary_profiles(state: State<'_, AppState>) -> Result<Vec<Uuid>, AppError> {
    Ok(deeplink::temporary_profiles(&state.appdb))
}
