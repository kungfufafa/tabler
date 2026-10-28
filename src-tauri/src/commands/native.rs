//! The drivers' own calls, for what a driver offers beyond its SQL or console: `session_call` on
//! an open connection, and `driver_util` from the connection form before any session exists
//! (DuckDB's "New…", the D1 and BigQuery pickers, the Snowflake probe). Each driver says which of
//! its calls only read (`CallKind`); a read-only connection makes no other.

use serde_json::Value as Json;
use tauri::State;
use uuid::Uuid;

use crate::commands::profiles::resolve_password;
use crate::commands::safety::refuse_read_only_call;
use crate::drivers::native::unknown_call;
use crate::drivers::registry::{self, UtilInput};
use crate::error::AppError;
use crate::state::{AppState, Connection};
use crate::types::profile::ProfileInput;

/// Calls `method` of the connection's driver with `args`.
#[tauri::command]
pub async fn session_call(
    state: State<'_, AppState>,
    connection_id: Uuid,
    method: String,
    args: Json,
) -> Result<Json, AppError> {
    let conn = state.connection(connection_id).await?;
    session_call_on(&conn, &method, args).await
}

/// The body of [`session_call`] on an open connection.
pub(crate) async fn session_call_on(
    conn: &Connection,
    method: &str,
    args: Json,
) -> Result<Json, AppError> {
    let session = conn.any_session().await;
    refuse_read_only_call(conn.profile.options.read_only, session.call_kind(method))?;
    log::info!("session_call {method} on {}", conn.profile.name);
    session.call(method, args).await
}

/// Calls `method` of the driver of the profile being edited, with the secret the dialog would
/// connect with.
#[tauri::command]
pub async fn driver_util(
    state: State<'_, AppState>,
    input: ProfileInput,
    method: String,
    args: Json,
) -> Result<Json, AppError> {
    driver_util_on(&state, &input, &method, args).await
}

/// The body of [`driver_util`], callable without a Tauri runtime.
pub(crate) async fn driver_util_on(
    state: &AppState,
    input: &ProfileInput,
    method: &str,
    args: Json,
) -> Result<Json, AppError> {
    let profile = &input.profile;
    registry::ensure_built(profile)?;
    // The core drivers have none: no password command runs for nothing.
    if profile.driver.is_core() {
        return Err(unknown_call(profile.driver, method));
    }
    refuse_read_only_call(
        profile.options.read_only,
        registry::util_kind(profile.driver, method),
    )?;
    let secret = resolve_password(state, input).await?;
    log::info!("driver_util {method} for {}", profile.name);
    registry::util(
        UtilInput {
            profile,
            secret: secret.as_deref(),
        },
        method,
        args,
    )
    .await
}
