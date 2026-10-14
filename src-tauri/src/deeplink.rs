//! Opening Tabler from outside (TablePlus §1.10, §1.5): connection URLs from `open -a Tabler
//! "postgres://…"`, from links in other apps or from the command line (`postgres`, `postgresql`,
//! `mysql`, `mariadb`, `sqlite`, their `+ssh` forms and `tabler://connection/<id>`), and files
//! double-clicked in Finder (`.db`, `.sqlite`, `.sqlite3`, `.sql`, `.sql.gz`).
//!
//! macOS hands URLs and files over as `RunEvent::Opened` (registered in `Info.plist` and
//! `bundle.fileAssociations`); command-line arguments are read at start. Requests are queued
//! here until the renderer takes them (`take_pending_opens`), so what arrives while the app
//! starts is not lost; `deeplink:open` tells a running renderer to look. The renderer parses and
//! routes them (`src/features/deeplink`).
//!
//! A URL that matches no saved connection opens a temporary one: a profile saved for the
//! session only and listed under [`TEMPORARY_KEY`], so it is deleted when its connection closes,
//! or at the next start if the app quit first, unless the user saves it.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::appdb::AppDb;
use crate::error::AppError;

/// Tells the renderer that requests are waiting (`take_pending_opens`).
pub const EVENT_OPEN: &str = "deeplink:open";
/// App settings key listing the temporary profiles (URL opens not saved by the user).
pub const TEMPORARY_KEY: &str = "deeplink.temporaryProfiles";

/// The URL schemes Tabler opens (also in `Info.plist`).
pub const URL_SCHEMES: [&str; 10] = [
    "postgres",
    "postgresql",
    "mysql",
    "mariadb",
    "sqlite",
    "tabler",
    "postgres+ssh",
    "postgresql+ssh",
    "mysql+ssh",
    "mariadb+ssh",
];

/// File types Tabler opens from Finder (`.sql.gz` too).
pub const FILE_EXTENSIONS: [&str; 4] = ["db", "sqlite", "sqlite3", "sql"];

/// Something to open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export, rename_all_fields = "camelCase")]
pub enum OpenRequest {
    /// A connection URL (as given; the renderer parses it).
    Url { url: String },
    /// A file: a SQLite database or an SQL script.
    File { path: String },
}

fn scheme_of(text: &str) -> Option<String> {
    let colon = text.find(':')?;
    let scheme = &text[..colon];
    let valid = !scheme.is_empty()
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    valid.then(|| scheme.to_ascii_lowercase())
}

/// Whether Tabler opens files like `path`.
pub fn is_supported_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    name.ends_with(".sql.gz")
        || FILE_EXTENSIONS
            .iter()
            .any(|ext| name.ends_with(&format!(".{ext}")))
}

/// What a URL macOS handed over stands for: a file (`file://`), a connection URL, or nothing
/// Tabler opens.
pub fn classify(url: &str) -> Option<OpenRequest> {
    let scheme = scheme_of(url)?;
    if scheme == "file" {
        let parsed = tauri::Url::parse(url).ok()?;
        let path = parsed.to_file_path().ok()?;
        return Some(OpenRequest::File {
            path: path.to_string_lossy().into_owned(),
        });
    }
    URL_SCHEMES
        .contains(&scheme.as_str())
        .then(|| OpenRequest::Url {
            url: url.trim().to_string(),
        })
}

/// The requests in the command line (`tabler "postgres://…" ~/app.sqlite`): connection URLs and
/// existing files of a supported type, relative paths resolved against `cwd`. Flags (`-…`, such
/// as macOS's `-psn_…`) are skipped.
pub fn from_args<I: IntoIterator<Item = String>>(args: I, cwd: &Path) -> Vec<OpenRequest> {
    let mut out = Vec::new();
    for arg in args {
        let arg = arg.trim();
        if arg.is_empty() || arg.starts_with('-') {
            continue;
        }
        if let Some(scheme) = scheme_of(arg)
            && arg.len() > scheme.len() + 1
            && (URL_SCHEMES.contains(&scheme.as_str()) || scheme == "file")
        {
            out.extend(classify(arg));
            continue;
        }
        let path = if arg == "~" || arg.starts_with("~/") {
            crate::tunnel::expand_tilde(arg)
        } else {
            let p = PathBuf::from(arg);
            if p.is_absolute() { p } else { cwd.join(p) }
        };
        if path.is_file() && is_supported_file(&path) {
            out.push(OpenRequest::File {
                path: path.to_string_lossy().into_owned(),
            });
        }
    }
    out
}

fn pending() -> &'static Mutex<Vec<OpenRequest>> {
    static PENDING: Mutex<Vec<OpenRequest>> = Mutex::new(Vec::new());
    &PENDING
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Queues requests; true when there was something to queue.
pub fn enqueue(requests: Vec<OpenRequest>) -> bool {
    if requests.is_empty() {
        return false;
    }
    for r in &requests {
        match r {
            OpenRequest::Url { url } => log::info!("open request: {}", redact(url)),
            OpenRequest::File { path } => log::info!("open request: {path}"),
        }
    }
    lock(pending()).extend(requests);
    true
}

/// Takes the queued requests (oldest first).
pub fn take() -> Vec<OpenRequest> {
    std::mem::take(&mut *lock(pending()))
}

/// A URL for the log: only what cannot hold a secret, `scheme://host[:port]/...` of its first
/// authority. The userinfo (a password may hold `@`, `:` or `/`), the path, the query
/// (`?sslpassword=…`, the wrapper form `tabler://open?url=<percent-encoded URL>`) and the second
/// authority of a `+ssh` URL are all dropped; a host that does not look like one (the start of a
/// password that held a `/`) is dropped too.
pub fn redact(url: &str) -> String {
    let Some(scheme) = scheme_of(url) else {
        return "...".to_string();
    };
    let Some(rest) = url[scheme.len()..].strip_prefix("://") else {
        return format!("{scheme}:...");
    };
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if plain_host(host) {
        format!("{scheme}://{host}/...")
    } else {
        format!("{scheme}://...")
    }
}

/// `name`, `name:port`, `[v6]` or `[v6]:port`: a host as it is written in a URL.
fn plain_host(host: &str) -> bool {
    let name = |n: &str| {
        n.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    };
    let port = |p: &str| p.chars().all(|c| c.is_ascii_digit());
    match host.strip_prefix('[') {
        Some(v6) => v6.split_once(']').is_some_and(|(addr, tail)| {
            addr.chars()
                .all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '.' | '%'))
                && (tail.is_empty() || tail.strip_prefix(':').is_some_and(port))
        }),
        None => match host.split_once(':') {
            Some((n, p)) => name(n) && port(p),
            None => name(host),
        },
    }
}

/// `RunEvent::Opened` (macOS): queues what Tabler opens and tells the renderer.
pub fn on_opened(app: &tauri::AppHandle, urls: &[tauri::Url]) {
    let requests: Vec<OpenRequest> = urls.iter().filter_map(|u| classify(u.as_str())).collect();
    if enqueue(requests) {
        crate::windows::emit_to_target(app, EVENT_OPEN, ());
    }
}

/// The advisory lock of the running instance, held for the life of the process (the OS releases
/// it when the process ends, however it ends).
static INSTANCE_LOCK: OnceLock<File> = OnceLock::new();

/// Takes an exclusive advisory lock on the file `path` (created if missing): `None` when another
/// process (or another open handle of this one) holds it. The lock lasts as long as the returned
/// file is open.
fn lock_at(path: &Path) -> io::Result<Option<File>> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

/// Whether this process is the only running Tabler, so that what is left in the shared
/// places (the app database's temporary connections, the SQLite-over-SSH working copies) is
/// from an earlier run and not from an instance that is using it. A second launch (`tabler
/// <url>` from a terminal, `open -n`) must not purge a running instance's data. When the lock
/// cannot be taken for another reason the answer is yes, as before the lock existed.
fn claim_instance() -> bool {
    match lock_at(&std::env::temp_dir().join("tabler-instance.lock")) {
        Ok(Some(file)) => {
            let _ = INSTANCE_LOCK.set(file);
            true
        }
        Ok(None) => {
            log::info!("another Tabler instance is running: its temporary data is left alone");
            false
        }
        Err(e) => {
            log::warn!("could not take the instance lock: {e}");
            true
        }
    }
}

/// At start: the command line's URLs and files are queued, and what an earlier run left behind
/// (temporary connections, SQLite over SSH copies) is removed, unless another instance is
/// running (see [`claim_instance`]).
pub fn startup(app: &tauri::AppHandle) {
    let cwd = std::env::current_dir().unwrap_or_default();
    enqueue(from_args(std::env::args().skip(1), &cwd));
    if !claim_instance() {
        return;
    }
    crate::tunnel::sqlite_remote::purge_stale_copies();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        use tauri::Manager;
        if let Some(state) = app.try_state::<crate::state::AppState>() {
            purge_temporary(&state).await;
        }
    });
}

// ----- temporary profiles -----------------------------------------------------------------------

/// A temporary profile as marked: its `updated_at` then. Saving it in the connection form (or any
/// other change the user makes) moves `updated_at`, and the profile is kept from then on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Marked {
    id: Uuid,
    updated_at: String,
}

fn marked(appdb: &AppDb) -> Vec<Marked> {
    appdb
        .get_setting(TEMPORARY_KEY)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_value::<Vec<Marked>>(v).ok())
        .unwrap_or_default()
}

fn store_marked(appdb: &AppDb, list: &[Marked]) -> Result<(), AppError> {
    appdb.set_setting(TEMPORARY_KEY, &serde_json::to_value(list)?)
}

/// The profiles opened from a URL and not saved by the user. Profiles that were changed since
/// (saved in the connection form) or deleted are dropped from the list.
pub fn temporary_profiles(appdb: &AppDb) -> Vec<Uuid> {
    let list = marked(appdb);
    let (keep, gone): (Vec<Marked>, Vec<Marked>) = list.into_iter().partition(
        |m| matches!(appdb.get_profile(m.id), Ok(Some(p)) if p.updated_at == m.updated_at),
    );
    if !gone.is_empty() {
        let _ = store_marked(appdb, &keep);
    }
    keep.into_iter().map(|m| m.id).collect()
}

/// Adds (`temporary`) or removes a profile from the temporary list.
pub fn mark_temporary(appdb: &AppDb, id: Uuid, temporary: bool) -> Result<(), AppError> {
    let mut list = marked(appdb);
    list.retain(|m| m.id != id);
    if temporary {
        let profile = appdb
            .get_profile(id)?
            .ok_or_else(|| AppError::NotFound(format!("profile {id}")))?;
        list.push(Marked {
            id,
            updated_at: profile.updated_at,
        });
    }
    store_marked(appdb, &list)
}

/// Deletes the temporary profiles left by an earlier run (the app quit while they were open).
pub async fn purge_temporary(state: &crate::state::AppState) {
    let ids = temporary_profiles(&state.appdb);
    if ids.is_empty() {
        return;
    }
    for id in &ids {
        if let Err(e) = state.appdb.delete_profile(*id) {
            log::warn!("could not delete the temporary connection {id}: {e}");
        }
        let _ = state.secrets.delete_all(*id).await;
    }
    let _ = store_marked(&state.appdb, &[]);
    log::info!(
        "removed {} temporary connection(s) of the last run",
        ids.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_opened_urls() {
        assert_eq!(
            classify("postgres://u:p@h/db?table=users"),
            Some(OpenRequest::Url {
                url: "postgres://u:p@h/db?table=users".into()
            })
        );
        assert!(matches!(
            classify("postgresql+ssh://a@b/c:d@127.0.0.1/db"),
            Some(OpenRequest::Url { .. })
        ));
        assert!(matches!(
            classify("TABLER://connection/00000000-0000-0000-0000-000000000001"),
            Some(OpenRequest::Url { .. })
        ));
        assert_eq!(
            classify("file:///Users/me/My%20Data/app.sqlite"),
            Some(OpenRequest::File {
                path: "/Users/me/My Data/app.sqlite".into()
            })
        );
        assert_eq!(classify("https://example.com"), None);
        assert_eq!(classify("not a url"), None);
    }

    #[test]
    fn reads_the_command_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("app.sqlite"), b"").unwrap();
        std::fs::write(dir.path().join("dump.sql.gz"), b"").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"").unwrap();
        let args = [
            "-psn_0_12345",
            "mysql://root@127.0.0.1/shop",
            "app.sqlite",
            "dump.sql.gz",
            "notes.txt",
            "missing.db",
            "https://example.com",
        ]
        .map(String::from);
        let got = from_args(args, dir.path());
        assert_eq!(
            got,
            vec![
                OpenRequest::Url {
                    url: "mysql://root@127.0.0.1/shop".into()
                },
                OpenRequest::File {
                    path: dir.path().join("app.sqlite").to_string_lossy().into_owned()
                },
                OpenRequest::File {
                    path: dir
                        .path()
                        .join("dump.sql.gz")
                        .to_string_lossy()
                        .into_owned()
                },
            ]
        );
    }

    #[test]
    fn supported_files() {
        for name in ["a.db", "A.SQLITE", "x.sqlite3", "q.sql", "dump.SQL.gz"] {
            assert!(is_supported_file(Path::new(name)), "{name}");
        }
        for name in ["a.txt", "a.gz", "sql", "a.dbx"] {
            assert!(!is_supported_file(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn queue_and_redaction() {
        take();
        assert!(!enqueue(Vec::new()));
        assert!(enqueue(vec![OpenRequest::File {
            path: "/a.db".into()
        }]));
        assert_eq!(
            take(),
            vec![OpenRequest::File {
                path: "/a.db".into()
            }]
        );
        assert!(take().is_empty());
    }

    #[test]
    fn the_log_gets_the_scheme_and_host_only() {
        for (url, logged) in [
            (
                "postgres://user:s3cret@host:5432/db",
                "postgres://host:5432/...",
            ),
            (
                "postgresql+ssh://ssh:pw1@bastion/db:pw2@127.0.0.1/app",
                "postgresql+ssh://bastion/...",
            ),
            ("mysql://root@h/db", "mysql://h/..."),
            ("mysql://h", "mysql://h/..."),
            ("sqlite:///Users/me/app.db", "sqlite:///..."),
            ("postgres://u:pw@[::1]:5432/db", "postgres://[::1]:5432/..."),
            // The wrapper form carries the whole URL, percent-encoded, in the query.
            (
                "tabler://open?url=postgres%3A%2F%2Fu%3Asecretpw%40host%2Fdb",
                "tabler://open/...",
            ),
            // A password with `@` (the delimiter is the last one of the authority).
            ("postgres://user:p@ss@host/db", "postgres://host/..."),
            // Secrets in the query.
            (
                "postgres://host/db?sslpassword=hunter2&sshPassword=hunter3&password=x",
                "postgres://host/...",
            ),
            (
                "tabler://connection/00000000-0000-0000-0000-000000000001",
                "tabler://connection/...",
            ),
            // No authority to name.
            ("postgres:user:pw@host/db", "postgres:..."),
            ("not a url", "..."),
            // A password with `/`: what is before it is no host.
            ("postgres://u:pa/ss@host/db", "postgres://..."),
            ("postgres://sec%72et@host/db?x=1", "postgres://host/..."),
        ] {
            assert_eq!(redact(url), logged, "{url}");
        }
        for (url, secret) in [
            ("postgres://user:p@ss@host/db", "ss"),
            ("postgres://user:s3cret@host/db", "s3cret"),
            (
                "tabler://open?url=postgres%3A%2F%2Fu%3Asecretpw%40host%2Fdb",
                "secretpw",
            ),
            ("postgres://host/db?sslpassword=hunter2", "hunter2"),
            (
                "postgresql+ssh://ssh:pw1@bastion/db:pw2@h/app?sshPassword=pw3",
                "pw",
            ),
        ] {
            let logged = redact(url);
            assert!(!logged.contains(secret), "{url} -> {logged}");
        }
    }

    #[test]
    fn the_instance_lock_is_exclusive_while_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tabler-instance.lock");
        let first = lock_at(&path)
            .unwrap()
            .expect("the first caller gets the lock");
        // Another holder (another process; here another handle) is refused, not an error.
        assert!(lock_at(&path).unwrap().is_none());
        assert!(lock_at(&path).unwrap().is_none());
        drop(first);
        let again = lock_at(&path).unwrap();
        assert!(again.is_some(), "the lock is free once its holder is gone");
        assert!(lock_at(&path).unwrap().is_none());
        // Another lock file does not interfere.
        assert!(lock_at(&dir.path().join("other.lock")).unwrap().is_some());
        // A path that cannot be opened is an error: the caller keeps the old behaviour.
        assert!(lock_at(&dir.path().join("missing").join("x.lock")).is_err());
    }

    #[test]
    fn open_requests_serialize_tagged() {
        let json = serde_json::to_value(OpenRequest::Url { url: "x".into() }).unwrap();
        assert_eq!(json, serde_json::json!({"kind": "url", "url": "x"}));
    }

    /// The bundle registers what this module opens (Info.plist schemes, file associations).
    #[test]
    fn the_bundle_registers_every_scheme_and_file_type() {
        let plist = include_str!("../Info.plist");
        for scheme in URL_SCHEMES {
            assert!(
                plist.contains(&format!("<string>{scheme}</string>")),
                "{scheme}"
            );
        }
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let exts: Vec<String> = conf["bundle"]["fileAssociations"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|a| a["ext"].as_array().unwrap().iter())
            .map(|e| e.as_str().unwrap().to_string())
            .collect();
        for ext in FILE_EXTENSIONS {
            assert!(exts.contains(&ext.to_string()), "{ext}");
        }
    }

    #[test]
    fn temporary_profile_list() {
        let dir = tempfile::tempdir().unwrap();
        let appdb = AppDb::open(&dir.path().join("app.sqlite")).unwrap();
        let profile = |name: &str| -> crate::types::profile::ConnectionProfile {
            serde_json::from_value(serde_json::json!({
                "id": Uuid::new_v4(), "name": name, "driver": "sqlite", "sqlitePath": "/tmp/x.db",
            }))
            .unwrap()
        };
        let a = appdb.upsert_profile(&profile("a")).unwrap();
        let b = appdb.upsert_profile(&profile("b")).unwrap();
        mark_temporary(&appdb, a.id, true).unwrap();
        mark_temporary(&appdb, b.id, true).unwrap();
        mark_temporary(&appdb, a.id, true).unwrap();
        assert_eq!(temporary_profiles(&appdb), vec![b.id, a.id]);
        mark_temporary(&appdb, b.id, false).unwrap();
        assert_eq!(temporary_profiles(&appdb), vec![a.id]);
        assert!(mark_temporary(&appdb, Uuid::new_v4(), true).is_err());
        // Saved again by the user (its updated_at moves): no longer temporary.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut edited = a.clone();
        edited.name = "kept".into();
        let edited = appdb.upsert_profile(&edited).unwrap();
        assert_ne!(edited.updated_at, a.updated_at);
        assert!(temporary_profiles(&appdb).is_empty());
        // A deleted one drops out too.
        mark_temporary(&appdb, b.id, true).unwrap();
        appdb.delete_profile(b.id).unwrap();
        assert!(temporary_profiles(&appdb).is_empty());
    }
}
