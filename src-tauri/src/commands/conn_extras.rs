//! Connection ▸ View Using Encoding… (§11.7, b170): reads and changes the encoding of an open
//! connection's session (MySQL `SET NAMES`, PostgreSQL `client_encoding`). The session is
//! reopened with the new encoding (every pooled connection must get it), on the database it is
//! on now and through its SSH tunnel; the saved profile is not changed, so a reconnect goes back
//! to the profile's encoding. Switch Database keeps the chosen encoding (see
//! [`with_live_encoding`]).
//!
//! Tabler reads and writes text as UTF-8 only: a session with another encoding is read-only (the
//! server refuses writes, `ConnectionInfo::read_only` makes the renderer's guards refuse them up
//! front), so edits are never stored mis-encoded.

use std::time::{Duration, Instant};

use tauri::State;
use uuid::Uuid;

use crate::commands::connections::{connect_password, connect_timeout};
use crate::conn_options::{MYSQL_DEFAULT_CHARSET, effective_read_only};
use crate::drivers::{self, ConnectTarget, DbSession};
use crate::error::AppError;
use crate::sqlgen::conn_options as sql;
use crate::state::AppState;
use crate::types::caps::Feature;
use crate::types::profile::{ConnectionInfo, ConnectionProfile, Driver};
use crate::types::value::Value;

fn no_sqlite() -> AppError {
    AppError::InvalidInput("SQLite text is always UTF-8: there is no connection encoding".into())
}

/// The error for a driver without a connection encoding (`descriptor().encoding` is false).
fn no_encoding(driver: Driver) -> AppError {
    match driver {
        Driver::Sqlite => no_sqlite(),
        other => AppError::InvalidInput(format!(
            "{} connections have no connection encoding",
            other.label()
        )),
    }
}

/// Bound on reading the encoding of a session that is about to be replaced.
const LIVE_ENCODING_TIMEOUT: Duration = Duration::from_secs(5);

/// Reads the encoding `session` uses now (`UTF8`, `utf8mb4`, `latin1`…).
async fn read_encoding(session: &dyn DbSession) -> Result<String, AppError> {
    let query = match session.driver() {
        Driver::Postgres => sql::PG_SHOW_CLIENT_ENCODING,
        Driver::Mysql => sql::MYSQL_SHOW_CHARSET,
        other => return Err(no_encoding(other)),
    };
    let rows = session.query(query, &[]).await?;
    match rows.rows.first().and_then(|r| r.first()) {
        Some(Value::Text(s)) => Ok(s.clone()),
        other => Err(AppError::internal(format!(
            "unexpected encoding value {other:?}"
        ))),
    }
}

/// The encoding `session` uses now, or `None` when it cannot be read (SQLite has none; a dead
/// session does not answer) so the caller keeps what it had.
pub(crate) async fn live_encoding(session: &dyn DbSession) -> Option<String> {
    match tokio::time::timeout(LIVE_ENCODING_TIMEOUT, read_encoding(session)).await {
        Ok(Ok(encoding)) => Some(encoding),
        Ok(Err(e)) => {
            log::debug!("the session's encoding is unknown: {e}");
            None
        }
        Err(_) => {
            log::debug!("reading the session's encoding timed out");
            None
        }
    }
}

/// `profile` with the encoding its connection's session uses now (`live`, see [`live_encoding`]):
/// View Using Encoding… changes the session only, so a session opened from the profile again
/// (Switch Database…) would silently go back to the profile's own encoding. Unchanged when `live`
/// is unknown, is the profile's encoding (`None`: the driver's default, compared case-insensitively)
/// or is not a name a session can be set to; SQLite has no encoding.
pub(crate) fn with_live_encoding(
    mut profile: ConnectionProfile,
    live: Option<String>,
) -> ConnectionProfile {
    let own_default = match profile.driver {
        Driver::Mysql => MYSQL_DEFAULT_CHARSET,
        Driver::Postgres => "UTF8",
        // SQLite and the other drivers have no encoding to keep.
        _ => return profile,
    };
    let Some(live) = live
        .map(|l| l.trim().to_string())
        .filter(|l| sql::valid_encoding(l))
    else {
        return profile;
    };
    if !live.eq_ignore_ascii_case(profile.options.encoding().unwrap_or(own_default)) {
        profile.options.encoding = Some(live);
    }
    profile
}

/// The encoding the open connection's session uses now (`UTF8`, `utf8mb4`, `latin1`…).
#[tauri::command]
pub async fn get_connection_encoding(
    state: State<'_, AppState>,
    connection_id: Uuid,
) -> Result<String, AppError> {
    connection_encoding_on(&state, connection_id).await
}

/// The body of [`get_connection_encoding`], callable without a Tauri runtime.
pub(crate) async fn connection_encoding_on(
    state: &AppState,
    connection_id: Uuid,
) -> Result<String, AppError> {
    let session = state.session_for(connection_id, Feature::Encoding).await?;
    read_encoding(&*session).await
}

/// Reopens the connection's session with `encoding` (`None`: the profile's own). Returns the
/// connection's info; its schema caches are dropped, so the renderer reloads what it shows.
#[tauri::command]
pub async fn set_connection_encoding(
    state: State<'_, AppState>,
    connection_id: Uuid,
    encoding: Option<String>,
) -> Result<ConnectionInfo, AppError> {
    set_connection_encoding_on(&state, connection_id, encoding).await
}

/// The body of [`set_connection_encoding`], callable without a Tauri runtime.
pub(crate) async fn set_connection_encoding_on(
    state: &AppState,
    connection_id: Uuid,
    encoding: Option<String>,
) -> Result<ConnectionInfo, AppError> {
    let conn = state.connection(connection_id).await?;
    // The new session opens before the old one closes: a driver whose engine opens a file once
    // per process grants this only when that second session shares the instance
    // (`drivers::sharing`).
    conn.caps().await.require(Feature::Encoding)?;
    if !conn.profile.driver.descriptor().encoding {
        return Err(no_encoding(conn.profile.driver));
    }
    // A new session would drop the SQL editor's open transaction.
    if conn.session().await?.in_transaction() == Some(true) {
        return Err(AppError::InvalidInput(
            "The SQL editor has an open transaction: commit or roll it back first".into(),
        ));
    }
    let mut profile = conn.profile.clone();
    if let Some(enc) = encoding
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
    {
        profile.options.encoding = Some(enc);
    }
    crate::conn_options::check(&profile)?;
    // Stay on the database the connection is on now (Switch Database…).
    if let Some(db) = conn.info().await.current_database {
        profile.database = Some(db);
    }
    let host_override = match &conn.tunnel {
        Some(t) if !t.is_up() => {
            return Err(AppError::Tunnel(format!(
                "{}; reconnect to open a new tunnel",
                t.state()
                    .message
                    .unwrap_or_else(|| "the SSH session ended".into())
            )));
        }
        Some(t) => Some(t.connect_addr()),
        None => None,
    };
    let password = connect_password(state, &profile, None).await?;
    let extras = state.secrets.extras(&profile).await?;
    let attempt = Instant::now();
    let opened = async {
        let session = tokio::time::timeout(
            connect_timeout(&profile),
            extras.scope(drivers::connect(ConnectTarget {
                profile: &profile,
                password: password.as_deref(),
                host_override,
            })),
        )
        .await
        .map_err(|_| AppError::Timeout)??;
        let info = session.server_info().await?;
        Ok::<_, AppError>((session, info))
    }
    .await;
    let (session, server) = opened.map_err(|e| match &conn.tunnel {
        Some(t) => t.explain_error(e, attempt),
        None => e,
    })?;
    let capabilities = session.capabilities();
    let old = std::mem::replace(&mut *conn.session.write().await, session.into());
    old.close().await;
    conn.invalidate(None).await;
    log::info!(
        "connection {connection_id} now uses the encoding {}",
        profile.options.encoding().unwrap_or("of its profile")
    );
    let mut info = conn.info.write().await;
    info.server_version = server.version;
    info.current_database = server.current_database;
    info.current_user = server.current_user;
    // The profile clone carries the new encoding (or the profile's own after a reset): a session
    // that is not UTF-8 is read-only, the profile's own setting otherwise.
    info.read_only = effective_read_only(&profile);
    info.capabilities = Some(capabilities);
    Ok(info.clone())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::types::profile::{ProfileOptions, SslMode};

    fn profile(driver: Driver, encoding: Option<&str>) -> ConnectionProfile {
        ConnectionProfile {
            id: Uuid::nil(),
            name: "t".into(),
            color: None,
            group: None,
            driver,
            host: Some("h".into()),
            port: None,
            user: Some("u".into()),
            database: Some("one".into()),
            ssl: SslMode::Prefer,
            ssl_ca_path: None,
            sqlite_path: Some("/x.db".into()),
            ssh: None,
            options: ProfileOptions {
                encoding: encoding.map(str::to_string),
                ..ProfileOptions::default()
            },
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        }
    }

    fn live(driver: Driver, own: Option<&str>, live: Option<&str>) -> Option<String> {
        with_live_encoding(profile(driver, own), live.map(str::to_string))
            .options
            .encoding
    }

    #[test]
    fn a_switched_session_keeps_the_encoding_chosen_with_view_using_encoding() {
        // Different: the live encoding is applied (and makes the new session read-only).
        let p = with_live_encoding(profile(Driver::Mysql, None), Some("latin1".into()));
        assert_eq!(p.options.encoding.as_deref(), Some("latin1"));
        assert!(effective_read_only(&p));
        assert_eq!(p.database.as_deref(), Some("one"), "nothing else changes");
        assert_eq!(
            live(Driver::Postgres, None, Some("LATIN1")).as_deref(),
            Some("LATIN1")
        );
        assert_eq!(
            live(Driver::Mysql, Some("latin1"), Some("utf8mb4")).as_deref(),
            Some("utf8mb4")
        );
        assert_eq!(
            live(Driver::Postgres, Some("WIN1252"), Some("UTF8")).as_deref(),
            Some("UTF8")
        );
        // Equal: unchanged, whatever the case and whether the profile names it or not.
        assert_eq!(live(Driver::Mysql, None, Some("utf8mb4")), None);
        assert_eq!(live(Driver::Postgres, None, Some("UTF8")), None);
        assert_eq!(
            live(Driver::Mysql, Some("LATIN1"), Some("latin1")).as_deref(),
            Some("LATIN1")
        );
        assert_eq!(
            live(Driver::Postgres, Some("utf8"), Some("UTF8")).as_deref(),
            Some("utf8")
        );
        // Unknown (the session did not answer) or unusable: the profile's own encoding stays.
        assert_eq!(
            live(Driver::Mysql, Some("latin1"), None).as_deref(),
            Some("latin1")
        );
        assert_eq!(live(Driver::Mysql, None, None), None);
        assert_eq!(live(Driver::Mysql, None, Some("")), None);
        assert_eq!(live(Driver::Mysql, None, Some("la tin1'; x")), None);
        // SQLite has no encoding.
        assert_eq!(live(Driver::Sqlite, None, Some("latin1")), None);
    }

    /// A profile on `TABLER_TEST_PG_URL` / `TABLER_TEST_MYSQL_URL` (`scheme://user[:pass]@host:port/db`),
    /// with its password; `None` (the test passes) when the variable is unset.
    fn server_profile(driver: Driver) -> Option<(ConnectionProfile, Option<String>)> {
        let var = match driver {
            Driver::Postgres => "TABLER_TEST_PG_URL",
            _ => "TABLER_TEST_MYSQL_URL",
        };
        let Ok(url) = std::env::var(var) else {
            println!("skipped: {var} unset");
            return None;
        };
        let rest = url.split_once("://").expect("a URL").1;
        let (creds, hostdb) = rest.rsplit_once('@').unwrap_or(("", rest));
        let (user, password) = creds
            .split_once(':')
            .map_or((creds, None), |(u, p)| (u, Some(p.to_string())));
        let (hostport, db) = hostdb.split_once('/').unwrap_or((hostdb, ""));
        let (host, port) = hostport.rsplit_once(':').unwrap_or((hostport, ""));
        let mut p = profile(driver, None);
        p.host = Some(host.into());
        p.port = port.parse().ok();
        p.user = Some(user.into()).filter(|u: &String| !u.is_empty());
        p.database =
            Some(db.split('?').next().unwrap_or("").into()).filter(|d: &String| !d.is_empty());
        p.ssl = SslMode::Disable;
        Some((p, password))
    }

    async fn open(p: &ConnectionProfile, password: Option<&str>) -> Arc<dyn DbSession> {
        drivers::connect(ConnectTarget {
            profile: p,
            password,
            host_override: None,
        })
        .await
        .expect("connect")
    }

    async fn scalar(session: &Arc<dyn DbSession>, sql: &str) -> String {
        let rows = session.query(sql, &[]).await.expect(sql);
        match &rows.rows[0][0] {
            Value::Text(s) => s.clone(),
            Value::Int(i) => i.to_string(),
            other => format!("{other:?}"),
        }
    }

    /// Switch Database reopens the session from the profile: with the live encoding merged in,
    /// the new session still reads the encoding chosen with View Using Encoding (and, not being
    /// UTF-8, is read-only); without it the session silently went back to the profile's own.
    async fn switch_keeps_the_encoding(driver: Driver, chosen: &str, read_only_sql: &str) {
        let Some((profile, password)) = server_profile(driver) else {
            return;
        };
        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .ok();
        let password = password.as_deref();
        // The open session: the profile's own encoding, then another one (what
        // `set_connection_encoding` opens).
        let own = open(&profile, password).await;
        let default = live_encoding(&*own)
            .await
            .expect("the encoding is readable");
        assert_eq!(
            with_live_encoding(profile.clone(), Some(default))
                .options
                .encoding,
            None,
            "the profile's own encoding stays as it is"
        );
        own.close().await;
        let mut chosen_profile = profile.clone();
        chosen_profile.options.encoding = Some(chosen.into());
        let session = open(&chosen_profile, password).await;
        let live = live_encoding(&*session)
            .await
            .expect("the encoding is readable");
        assert!(live.eq_ignore_ascii_case(chosen), "{live}");
        session.close().await;

        // What `switch_database` opens: the profile (`conn.profile`, which never changed) with the
        // live encoding of the session it replaces.
        let switched = with_live_encoding(profile.clone(), Some(live.clone()));
        assert_eq!(switched.options.encoding.as_deref(), Some(live.as_str()));
        assert!(effective_read_only(&switched));
        let after = open(&switched, password).await;
        let read = live_encoding(&*after)
            .await
            .expect("the encoding is readable");
        assert!(
            read.eq_ignore_ascii_case(chosen),
            "the new session reads {read}"
        );
        assert_eq!(scalar(&after, read_only_sql).await, read_only_value(driver));
        after.close().await;
        // Without the merge the new session is back on the profile's own encoding.
        let plain = open(&profile, password).await;
        let back = live_encoding(&*plain)
            .await
            .expect("the encoding is readable");
        assert!(!back.eq_ignore_ascii_case(chosen), "{back}");
        plain.close().await;
    }

    fn read_only_value(driver: Driver) -> String {
        if driver == Driver::Postgres {
            "on"
        } else {
            "1"
        }
        .into()
    }

    #[tokio::test]
    async fn a_switched_postgres_session_keeps_the_chosen_encoding() {
        switch_keeps_the_encoding(
            Driver::Postgres,
            "LATIN1",
            "SHOW default_transaction_read_only",
        )
        .await;
    }

    #[tokio::test]
    async fn a_switched_mysql_session_keeps_the_chosen_encoding() {
        switch_keeps_the_encoding(
            Driver::Mysql,
            "latin1",
            "SELECT @@session.transaction_read_only",
        )
        .await;
    }
}
