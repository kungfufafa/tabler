//! Backup / restore commands (TablePlus "Backup…" / "Restore…"). The work happens in
//! `crate::backup`; these resolve the profile, its password and its SSH tunnel.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tauri::State;
use tauri::ipc::Channel;
use uuid::Uuid;

use crate::backup::args::{self, ConnParams, Endpoint, ToolInfo};
use crate::backup::{discover, run, version};
use crate::commands::connections::{
    connect_password, connect_timeout, open_profile_tunnel, release_pinned_port,
    stored_tunnel_auth, tunnel_config,
};
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::{self, ConnectTarget};
use crate::error::AppError;
use crate::state::{AppState, Connection};
use crate::tunnel::{self, TunnelHandle};
use crate::types::backup::{
    BackupEvent, BackupJobKind, BackupJobRequest, BackupJobResult, BackupTarget, DumpBinary,
    RestoreFileInfo,
};
use crate::types::profile::{ConnStatus, ConnectionProfile, Driver, ProfileOptions};

/// Bound on closing a temporary session (MySQL waits for its pooled connections).
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

fn load_profile(state: &AppState, profile_id: Uuid) -> Result<ConnectionProfile, AppError> {
    let profile = state
        .appdb
        .get_profile(profile_id)?
        .ok_or_else(|| AppError::NotFound(format!("profile {profile_id}")))?;
    if profile.driver == Driver::Sqlite {
        return Err(AppError::Unsupported(
            "SQLite databases are backed up by copying the database file".into(),
        ));
    }
    Ok(profile)
}

/// An open connection of the profile whose session is up.
async fn live_connection(state: &AppState, profile_id: Uuid) -> Option<Arc<Connection>> {
    state
        .connections
        .read()
        .await
        .values()
        .find(|c| c.profile.id == profile_id && *c.status.borrow() == ConnStatus::Connected)
        .cloned()
}

/// Where a binary (or a temporary session) reaches the profile's server.
struct Route {
    endpoint: Endpoint,
    /// A tunnel opened for this job only; closed by [`Route::close`].
    own_tunnel: Option<TunnelHandle>,
    /// The open connection whose tunnel is reused (kept alive while the job runs).
    _shared: Option<Arc<Connection>>,
}

impl Route {
    fn host_override(&self) -> Option<(String, u16)> {
        self.endpoint
            .via_tunnel
            .then(|| (self.endpoint.host.clone(), self.endpoint.port))
    }

    async fn close(self) {
        if let Some(t) = self.own_tunnel {
            t.close().await;
        }
    }
}

/// The profile's host and port, or an SSH tunnel to them: the tunnel of an open connection of the
/// profile when one is up, else a new one for the duration of the job.
async fn route(state: &AppState, profile: &ConnectionProfile) -> Result<Route, AppError> {
    let Some(ssh) = tunnel_config(profile) else {
        let host = profile
            .host
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .unwrap_or(tunnel::LOCAL_HOST)
            .to_string();
        let port = profile
            .port
            .or_else(|| profile.driver.default_port())
            .unwrap_or(0);
        return Ok(Route {
            endpoint: Endpoint {
                host,
                port,
                via_tunnel: false,
            },
            own_tunnel: None,
            _shared: None,
        });
    };
    let shared = state
        .connections
        .read()
        .await
        .values()
        .find(|c| c.profile.id == profile.id && c.tunnel.as_ref().is_some_and(TunnelHandle::is_up))
        .cloned();
    if let Some(conn) = shared
        && let Some((host, port)) = conn.tunnel.as_ref().map(TunnelHandle::connect_addr)
    {
        return Ok(Route {
            endpoint: Endpoint {
                host,
                port,
                via_tunnel: true,
            },
            own_tunnel: None,
            _shared: Some(conn),
        });
    }
    let auth = stored_tunnel_auth(state, profile.id, ssh).await?;
    if let Some(port) = ssh.local_port {
        release_pinned_port(state, profile.id, port).await;
    }
    let handle = open_profile_tunnel(state, profile, auth)
        .await?
        .ok_or_else(|| AppError::internal("the profile has no SSH tunnel"))?;
    let (host, port) = handle.connect_addr();
    Ok(Route {
        endpoint: Endpoint {
            host,
            port,
            via_tunnel: true,
        },
        own_tunnel: Some(handle),
        _shared: None,
    })
}

/// Every pg_dump / pg_restore / psql (PostgreSQL) or mysqldump / mysql (MySQL) binary found on
/// this Mac, with its version.
#[tauri::command]
pub async fn backup_list_binaries(driver: Driver) -> Result<Vec<DumpBinary>, AppError> {
    if driver == Driver::Sqlite {
        return Ok(Vec::new());
    }
    let path = std::env::var_os("PATH");
    Ok(discover::discover(Path::new("/"), path.as_deref(), driver).await)
}

/// The server version and databases of a saved connection: read through its open connection, or
/// through a temporary one (with its SSH tunnel) that is closed again.
#[tauri::command]
pub async fn backup_inspect_target(
    state: State<'_, AppState>,
    profile_id: Uuid,
) -> Result<BackupTarget, AppError> {
    let profile = load_profile(&state, profile_id)?;
    if let Some(conn) = live_connection(&state, profile_id).await {
        let info = conn.info().await;
        let databases = conn.session().await.list_databases().await?;
        return Ok(BackupTarget {
            profile_id,
            driver: profile.driver,
            server: version::parse_server_version(&info.server_version),
            server_version: info.server_version,
            databases: databases.into_iter().map(|d| d.name).collect(),
            current_database: info.current_database,
            live: true,
        });
    }
    let password = connect_password(&state, &profile, None).await?;
    let route = route(&state, &profile).await?;
    let opened = tokio::time::timeout(
        connect_timeout(&profile),
        drivers::connect(ConnectTarget {
            profile: &profile,
            password: password.as_deref(),
            host_override: route.host_override(),
        }),
    )
    .await
    .map_err(|_| AppError::Timeout)
    .and_then(|r| r);
    let session = match opened {
        Ok(s) => s,
        Err(e) => {
            route.close().await;
            return Err(e);
        }
    };
    let read = async {
        let server = session.server_info().await?;
        let databases = session.list_databases().await?;
        Ok::<_, AppError>((server, databases))
    }
    .await;
    if tokio::time::timeout(CLOSE_TIMEOUT, session.close())
        .await
        .is_err()
    {
        log::warn!(
            "backup: closing the temporary session of {} timed out",
            profile.name
        );
    }
    route.close().await;
    let (server, databases) = read?;
    Ok(BackupTarget {
        profile_id,
        driver: profile.driver,
        server: version::parse_server_version(&server.version),
        server_version: server.version,
        databases: databases.into_iter().map(|d| d.name).collect(),
        current_database: server.current_database,
        live: false,
    })
}

/// What a file picked for a restore holds (by its first bytes).
#[tauri::command]
pub async fn backup_inspect_file(path: String) -> Result<RestoreFileInfo, AppError> {
    tokio::task::spawn_blocking(move || crate::backup::detect::inspect_file(Path::new(&path)))
        .await
        .map_err(|e| AppError::internal(e.to_string()))?
}

/// Checks a backup destination: absolute, no `..`; creates its folder; picks a free name.
fn backup_destination(path: &str) -> Result<PathBuf, AppError> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(AppError::InvalidInput(
            "the backup path must be absolute".into(),
        ));
    }
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(AppError::InvalidInput(
            "the backup file name cannot contain ..".into(),
        ));
    }
    if p.file_name().is_none() {
        return Err(AppError::InvalidInput(
            "the backup needs a file name".into(),
        ));
    }
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| AppError::Io(format!("cannot create {}: {e}", dir.display())))?;
    }
    Ok(run::free_path(p))
}

/// The binary a job runs: one of the known tools of the driver, executable, with its version.
async fn tool_info(driver: Driver, path: &str) -> Result<ToolInfo, AppError> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(AppError::InvalidInput("choose a binary".into()));
    }
    let tool = discover::tool_for_file(driver, &path).ok_or_else(|| {
        AppError::InvalidInput(format!(
            "{} is not a {} dump or restore binary",
            path.display(),
            if driver == Driver::Postgres {
                "PostgreSQL"
            } else {
                "MySQL"
            }
        ))
    })?;
    let output = discover::version_output(&path).await.ok_or_else(|| {
        AppError::NotFound(format!(
            "{} cannot be run; choose another version",
            path.display()
        ))
    })?;
    Ok(ToolInfo {
        path,
        tool,
        version: version::parse_tool_version(&output),
    })
}

/// A restore writes into the database: refused on a read-only connection (the renderer's guard
/// refuses it first).
fn refuse_read_only_restore(kind: BackupJobKind, options: &ProfileOptions) -> Result<(), AppError> {
    if kind == BackupJobKind::Restore && options.read_only {
        Err(AppError::InvalidInput(READ_ONLY_MESSAGE.into()))
    } else {
        Ok(())
    }
}

/// Runs a backup (pg_dump / mysqldump into `req.path`) or a restore (`req.path` through psql,
/// pg_restore or mysql), streaming its output on `on_event`. The renderer runs the connection's
/// safe-mode check first.
#[tauri::command]
pub async fn backup_run(
    state: State<'_, AppState>,
    req: BackupJobRequest,
    on_event: Channel<BackupEvent>,
) -> Result<BackupJobResult, AppError> {
    let (_job, cancel) = crate::backup::register_job(&req.job_id)?;
    let profile = load_profile(&state, req.profile_id)?;
    refuse_read_only_restore(req.kind, &profile.options)?;
    let tool = tool_info(profile.driver, &req.binary_path).await?;
    let (output, restore_file) = match req.kind {
        BackupJobKind::Backup => (Some(backup_destination(&req.path)?), None),
        BackupJobKind::Restore => {
            let path = req.path.clone();
            let info = tokio::task::spawn_blocking(move || {
                crate::backup::detect::inspect_file(Path::new(&path))
            })
            .await
            .map_err(|e| AppError::internal(e.to_string()))??;
            (None, Some(info))
        }
    };
    let password = connect_password(&state, &profile, None).await?;
    let route = route(&state, &profile).await?;
    let conn = ConnParams {
        driver: profile.driver,
        endpoint: route.endpoint.clone(),
        tls_host: profile.host.clone(),
        user: profile.user.clone(),
        password,
        database: req.database.clone(),
        ssl: profile.ssl,
        ssl_ca: profile.ssl_ca_path.clone(),
        connect_timeout_secs: profile.options.connect_timeout_secs.unwrap_or(15),
        system_ca: args::system_ca_bundle(),
    };
    let built = match &restore_file {
        None => args::build_backup(&tool, &conn, &req.flags, req.gzip),
        Some(file) => args::build_restore(&tool, &conn, &req.flags, file),
    };
    let inv = match built {
        Ok(inv) => inv,
        Err(e) => {
            route.close().await;
            return Err(e);
        }
    };
    log::info!(
        "backup: {:?} of {} / {} with {}",
        req.kind,
        profile.name,
        req.database,
        inv.label
    );
    let emit: run::Emit = Arc::new(move |event| {
        if let Err(e) = on_event.send(event) {
            log::debug!("backup event not delivered: {e}");
        }
    });
    let result = run::run(inv, output, emit, cancel).await;
    route.close().await;
    let out = result?;
    Ok(BackupJobResult {
        path: out.path.to_string_lossy().into_owned(),
        bytes: out.bytes,
        duration_ms: u64::try_from(out.duration.as_millis()).unwrap_or(u64::MAX),
        errors: out.errors,
    })
}

/// Stops a running job (its binary is killed; a partial backup file is removed). Unknown ids are
/// ignored.
#[tauri::command]
pub async fn backup_cancel(job_id: String) -> Result<(), AppError> {
    crate::backup::cancel_job(&job_id);
    Ok(())
}

/// SQLite: copies the profile's database file to `path` (a consistent copy, WAL included).
/// Returns the size of the copy.
#[tauri::command]
pub async fn backup_copy_sqlite(
    state: State<'_, AppState>,
    profile_id: Uuid,
    path: String,
) -> Result<u64, AppError> {
    let profile = state
        .appdb
        .get_profile(profile_id)?
        .ok_or_else(|| AppError::NotFound(format!("profile {profile_id}")))?;
    let src = match (profile.driver, profile.sqlite_path.as_deref()) {
        (Driver::Sqlite, Some(p)) if !p.trim().is_empty() => tunnel::expand_tilde(p.trim()),
        (Driver::Sqlite, _) => {
            return Err(AppError::InvalidInput(
                "the connection has no database file".into(),
            ));
        }
        _ => {
            return Err(AppError::InvalidInput(
                "only SQLite databases are copied as a file".into(),
            ));
        }
    };
    tokio::task::spawn_blocking(move || {
        crate::backup::sqlite::copy_database(&src, Path::new(&path))
    })
    .await
    .map_err(|e| AppError::internal(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_backup_destinations() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("Local PG/tabler_dev/2026-10-12.sql");
        let p = backup_destination(&nested.to_string_lossy()).unwrap();
        assert_eq!(p, nested);
        assert!(nested.parent().unwrap().is_dir());
        std::fs::write(&nested, "").unwrap();
        assert_eq!(
            backup_destination(&nested.to_string_lossy()).unwrap(),
            nested.with_file_name("2026-10-12 (2).sql")
        );
        assert!(backup_destination("relative.sql").is_err());
        assert!(backup_destination(&dir.path().join("a/../b.sql").to_string_lossy()).is_err());
        assert!(backup_destination("/").is_err());
    }

    #[test]
    fn read_only_connections_refuse_restores_only() {
        let read_only = ProfileOptions {
            read_only: true,
            ..ProfileOptions::default()
        };
        let err = refuse_read_only_restore(BackupJobKind::Restore, &read_only).unwrap_err();
        assert_eq!(err.to_string(), READ_ONLY_MESSAGE);
        assert!(refuse_read_only_restore(BackupJobKind::Backup, &read_only).is_ok());
        let writable = ProfileOptions::default();
        assert!(refuse_read_only_restore(BackupJobKind::Restore, &writable).is_ok());
    }

    #[tokio::test]
    async fn only_known_tools_run() {
        assert!(tool_info(Driver::Postgres, "/bin/sh").await.is_err());
        assert!(tool_info(Driver::Postgres, "pg_dump").await.is_err());
        let missing = tool_info(Driver::Mysql, "/nonexistent/mysqldump").await;
        assert!(matches!(missing, Err(AppError::NotFound(_))));
    }
}
