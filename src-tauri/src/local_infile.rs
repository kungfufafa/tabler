//! MySQL "Enable LOAD DATA LOCAL INFILE" (Other options): the server may ask for a local file
//! (`LOAD DATA LOCAL INFILE '/path/file.csv' INTO TABLE t`). Tabler sends only the files the
//! user allowed: each path is confirmed once per connection and app session (a native "Allow" /
//! "Deny" alert, installed at startup by [`install_dialog_approver`]); without an approver every
//! request is refused. An approval belongs to the connection that was asked: another connection
//! that asks for the same file is asked again. Profiles without the option get no handler at all,
//! so the driver refuses the request.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use futures::FutureExt as _;
use futures::future::BoxFuture;
use mysql_async::prelude::GlobalHandler;
use mysql_async::{InfileData, LocalInfileError, WhiteListFsHandler};

/// A server's request for a local file, shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfileRequest {
    pub connection: String,
    pub path: PathBuf,
}

type Approver = Arc<dyn Fn(InfileRequest) -> BoxFuture<'static, bool> + Send + Sync>;

static APPROVER: OnceLock<Approver> = OnceLock::new();

/// (connection, path) pairs allowed during this app session (canonical paths).
fn allowed() -> &'static Mutex<HashSet<(String, PathBuf)>> {
    static ALLOWED: OnceLock<Mutex<HashSet<(String, PathBuf)>>> = OnceLock::new();
    ALLOWED.get_or_init(Default::default)
}

/// Sets who decides about paths not allowed yet (once; later calls are ignored).
pub fn set_approver(approver: Approver) {
    let _ = APPROVER.set(approver);
}

/// Allows `paths` to `connection` for the rest of the app session (no question asked for them).
/// Other connections are still asked.
pub fn allow(connection: &str, paths: impl IntoIterator<Item = PathBuf>) {
    let mut set = allowed().lock().unwrap_or_else(|p| p.into_inner());
    for p in paths {
        set.insert((
            connection.to_string(),
            std::fs::canonicalize(&p).unwrap_or(p),
        ));
    }
}

fn is_allowed(connection: &str, path: &Path) -> bool {
    allowed()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .contains(&(connection.to_string(), path.to_path_buf()))
}

/// Asks with a native alert, on the main thread (tauri-plugin-dialog).
pub fn install_dialog_approver<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    use tauri_plugin_dialog::{DialogExt as _, MessageDialogButtons, MessageDialogKind};
    set_approver(Arc::new(move |req: InfileRequest| {
        let (tx, rx) = tokio::sync::oneshot::channel();
        app.dialog()
            .message(format!(
                "\u{201c}{}\u{201d} asks to read {} (LOAD DATA LOCAL INFILE). Send this file to \
                 the server?",
                req.connection,
                req.path.display()
            ))
            .title("Send a local file?")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Allow".into(),
                "Deny".into(),
            ))
            .show(move |ok| {
                let _ = tx.send(ok);
            });
        async move { rx.await.unwrap_or(false) }.boxed()
    }));
}

/// The LOCAL INFILE handler of one connection profile.
#[derive(Clone)]
pub struct InfileGate {
    connection: String,
}

impl InfileGate {
    pub fn new(connection: impl Into<String>) -> InfileGate {
        InfileGate {
            connection: connection.into(),
        }
    }
}

fn refused(path: &str, why: &str) -> LocalInfileError {
    LocalInfileError::OtherError(format!("LOAD DATA LOCAL INFILE {path}: {why}").into())
}

/// Decides about one request: an absolute path, allowed to this connection earlier or now by the
/// approver.
async fn decide(connection: String, requested: String) -> Result<(), LocalInfileError> {
    let path = PathBuf::from(&requested);
    if !path.is_absolute() {
        return Err(refused(&requested, "use the absolute path of the file"));
    }
    let canonical = std::fs::canonicalize(&path)
        .map_err(|e| refused(&requested, &format!("cannot read the file ({e})")))?;
    if is_allowed(&connection, &canonical) {
        return Ok(());
    }
    let Some(approver) = APPROVER.get().cloned() else {
        return Err(refused(&requested, "not allowed"));
    };
    if approver(InfileRequest {
        connection: connection.clone(),
        path: canonical.clone(),
    })
    .await
    {
        allow(&connection, [canonical]);
        Ok(())
    } else {
        Err(refused(&requested, "the file was not allowed"))
    }
}

impl GlobalHandler for InfileGate {
    fn handle(&self, file_name: &[u8]) -> BoxFuture<'static, Result<InfileData, LocalInfileError>> {
        let requested = String::from_utf8_lossy(file_name).into_owned();
        let connection = self.connection.clone();
        async move {
            decide(connection, requested.clone()).await?;
            // The driver's own reader, limited to exactly this file.
            WhiteListFsHandler::new([requested.clone()])
                .handle(requested.as_bytes())
                .await
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_allowed_absolute_paths_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("rows.csv");
        std::fs::write(&file, "1,a\n").unwrap();
        let file = std::fs::canonicalize(&file).unwrap();
        let other = dir.path().join("secret.txt");
        std::fs::write(&other, "x").unwrap();
        // The approver of the tests allows `rows.csv` to every connection but "conn3b", and
        // records what it was asked.
        let asked: Arc<Mutex<Vec<InfileRequest>>> = Arc::default();
        let log = asked.clone();
        let ok = file.clone();
        set_approver(Arc::new(move |req: InfileRequest| {
            let yes = req.path == ok && req.connection != "conn3b";
            log.lock().unwrap().push(req);
            async move { yes }.boxed()
        }));
        let gate = InfileGate::new("conn3a");
        assert!(
            gate.handle(b"rows.csv").await.is_err(),
            "relative paths are refused"
        );
        let path = file.to_string_lossy().into_owned();
        assert!(gate.handle(path.as_bytes()).await.is_ok());
        // Asked once: the path is remembered for the session.
        assert!(gate.handle(path.as_bytes()).await.is_ok());
        let other = std::fs::canonicalize(&other).unwrap();
        let err = match gate.handle(other.to_string_lossy().as_bytes()).await {
            Ok(_) => panic!("a denied file was read"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("not allowed"), "{err}");
        assert_eq!(asked.lock().unwrap().len(), 2);
        assert_eq!(asked.lock().unwrap()[0].connection, "conn3a");

        // The approval belongs to the connection that was asked: another connection asking for
        // the same file is asked again, and refused when the approver says no.
        let gate_b = InfileGate::new("conn3b");
        let err = match gate_b.handle(path.as_bytes()).await {
            Ok(_) => panic!("an approval of another connection was reused"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("not allowed"), "{err}");
        {
            let asked = asked.lock().unwrap();
            assert_eq!(asked.len(), 3, "the other connection is asked");
            assert_eq!(asked[2].connection, "conn3b");
            assert_eq!(asked[2].path, file);
        }
        // Still allowed to the first connection, without a new question.
        assert!(gate.handle(path.as_bytes()).await.is_ok());
        assert_eq!(asked.lock().unwrap().len(), 3);
        // A third connection that the approver accepts is asked once and then remembered.
        let gate_c = InfileGate::new("conn3c");
        assert!(gate_c.handle(path.as_bytes()).await.is_ok());
        assert!(gate_c.handle(path.as_bytes()).await.is_ok());
        assert_eq!(asked.lock().unwrap().len(), 4);
        // `allow` is per connection too.
        assert!(is_allowed("conn3a", &file) && !is_allowed("conn3d", &file));
        allow("conn3d", [file.clone()]);
        assert!(is_allowed("conn3d", &file));
    }
}
