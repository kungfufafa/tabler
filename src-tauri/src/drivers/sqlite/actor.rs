use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rusqlite::{Connection, InterruptHandle, OpenFlags};

use super::cipher::Cipher;
use crate::error::AppError;

type Job = Box<dyn FnOnce(&mut Connection) + Send + 'static>;

/// How long [`Actor::close`] waits for the thread to end (a statement that ignores the interrupt
/// can hold it up; the connection is then closed whenever that thread gets to it).
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// A dedicated thread owning one `rusqlite::Connection`; jobs are closures run in order.
pub struct Actor {
    tx: Mutex<Option<mpsc::Sender<Job>>>,
    interrupt: Arc<InterruptHandle>,
    /// Set by `close`: jobs still queued are answered with `NotConnected` instead of running.
    closed: Arc<AtomicBool>,
    /// The thread that owns the connection; `close` waits for it to end.
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Actor {
    /// Opens `path`; with `cipher` the key is set before any other statement (SQLCipher).
    pub fn spawn(
        path: &str,
        flags: OpenFlags,
        foreign_keys: bool,
        cipher: Option<&Cipher>,
    ) -> Result<Actor, AppError> {
        let conn = Connection::open_with_flags(path, flags)
            .map_err(|e| AppError::Connect(format!("cannot open {path}: {e}")))?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        match cipher {
            Some(cipher) => super::cipher::apply(&conn, cipher)?,
            // The bundled SQLCipher is built with `SQLITE_TEMP_STORE=2`, which makes temporary
            // tables, sorts and indexes live in memory unless told otherwise (the stock SQLite
            // build used files). A multi-gigabyte `ORDER BY`, `CREATE INDEX` or `VACUUM` of a
            // plain database must be able to spill to disk. An encrypted database keeps
            // SQLCipher's choice: its temporary files would otherwise hold plain text.
            None => conn.execute_batch("PRAGMA temp_store = FILE;")?,
        }
        conn.execute_batch(if foreign_keys {
            "PRAGMA foreign_keys = ON;"
        } else {
            "PRAGMA foreign_keys = OFF;"
        })?;
        let interrupt = Arc::new(conn.get_interrupt_handle());
        let (tx, rx) = mpsc::channel::<Job>();
        let thread = thread::Builder::new()
            .name("tabler-sqlite".into())
            .spawn(move || {
                let mut conn = conn;
                while let Ok(job) = rx.recv() {
                    job(&mut conn);
                }
                // `close` interrupts the connection to stop a statement that is running. With
                // nothing running that leaves SQLite's interrupt flag set until the next
                // statement starts, and a connection closed with the flag set aborts its
                // checkpoint: the `-wal` / `-shm` files would stay behind. A statement that does
                // nothing clears the flag.
                let _ = conn.execute_batch("SELECT 1");
                // The connection drops here, at the end of the thread: SQLite checkpoints a WAL
                // database and removes its `-wal` / `-shm` files when its last connection closes.
            })
            .map_err(|e| AppError::Internal(format!("spawn sqlite thread: {e}")))?;
        Ok(Actor {
            tx: Mutex::new(Some(tx)),
            interrupt,
            closed: Arc::new(AtomicBool::new(false)),
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Runs `f` on the actor thread and awaits its result.
    pub async fn run<R, F>(&self, f: F) -> Result<R, AppError>
    where
        R: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<R, AppError> + Send + 'static,
    {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let closed = self.closed.clone();
        let job: Job = Box::new(move |conn| {
            let result = if closed.load(Ordering::SeqCst) {
                Err(AppError::NotConnected)
            } else {
                f(conn)
            };
            let _ = reply_tx.send(result);
        });
        {
            let guard = self.tx.lock().unwrap_or_else(|p| p.into_inner());
            match guard.as_ref() {
                Some(tx) => tx.send(job).map_err(|_| AppError::NotConnected)?,
                None => return Err(AppError::NotConnected),
            }
        }
        reply_rx.await.map_err(|_| AppError::NotConnected)?
    }

    /// Interrupts the statement currently running on this connection (safe from any thread).
    pub fn interrupt(&self) {
        self.interrupt.interrupt();
    }

    pub fn interrupt_handle(&self) -> Arc<InterruptHandle> {
        self.interrupt.clone()
    }

    /// Stops the statement running now, makes queued jobs fail with `NotConnected` and drops
    /// the sender, so the thread exits (closing the connection and its file locks), then waits
    /// for that (up to 2 s): when this returns the file is closed, its WAL checkpointed. Without
    /// the wait the connection was dropped on the thread some time later, which an exiting
    /// process may never reach. Blocks the calling thread for that moment.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.interrupt.interrupt();
        {
            let mut guard = self.tx.lock().unwrap_or_else(|p| p.into_inner());
            guard.take();
        }
        self.wait_for_thread(CLOSE_WAIT);
    }

    /// Waits until the connection thread has ended, at most `limit`. A thread that is still busy
    /// stays registered for the next `close`.
    fn wait_for_thread(&self, limit: Duration) {
        let handle = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take();
        let Some(handle) = handle else {
            return;
        };
        // Called from one of this actor's own jobs: the thread cannot wait for itself.
        if handle.thread().id() == thread::current().id() {
            return;
        }
        let deadline = Instant::now() + limit;
        while !handle.is_finished() {
            if Instant::now() >= deadline {
                log::warn!("sqlite: the connection thread did not end within {limit:?}");
                *self.thread.lock().unwrap_or_else(|p| p.into_inner()) = Some(handle);
                return;
            }
            thread::sleep(Duration::from_millis(2));
        }
        let _ = handle.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const READ_WRITE: OpenFlags = OpenFlags::SQLITE_OPEN_READ_WRITE
        .union(OpenFlags::SQLITE_OPEN_CREATE)
        .union(OpenFlags::SQLITE_OPEN_NO_MUTEX);

    async fn temp_store(actor: &Actor) -> i64 {
        actor
            .run(|c| Ok(c.query_row("PRAGMA temp_store", [], |r| r.get(0))?))
            .await
            .unwrap()
    }

    /// `temp_store` 1 is FILE (0 would be the compile-time default, which the bundled SQLCipher
    /// sets to MEMORY).
    #[tokio::test]
    async fn plain_databases_keep_temporary_data_in_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.sqlite");
        let path = path.to_string_lossy().into_owned();
        let actor = Actor::spawn(&path, READ_WRITE, true, None).unwrap();
        assert_eq!(temp_store(&actor).await, 1);
        actor.close();

        // Read-only and foreign keys off: the same.
        let actor = Actor::spawn(&path, OpenFlags::SQLITE_OPEN_READ_ONLY, false, None).unwrap();
        assert_eq!(temp_store(&actor).await, 1);
        actor.close();
    }

    #[tokio::test]
    async fn keyed_databases_still_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keyed.sqlite");
        let path = path.to_string_lossy().into_owned();
        let cipher = Cipher {
            key: "k".into(),
            compatibility: None,
        };
        super::super::cipher::create_encrypted(&path, &cipher).unwrap();
        let actor = Actor::spawn(&path, READ_WRITE, true, Some(&cipher)).unwrap();
        // SQLCipher's own temp_store choice is left alone (not asserted: it is the library's).
        let tables: i64 = actor
            .run(|c| Ok(c.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?))
            .await
            .unwrap();
        assert_eq!(tables, 0);
        actor.close();
    }

    /// `close` returns once the connection is really closed: the single connection of a WAL
    /// database leaves no `-wal` / `-shm` file behind, even when a statement is still running.
    #[tokio::test]
    async fn close_waits_until_the_connection_thread_has_ended() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wal.sqlite");
        let path_str = path.to_string_lossy().into_owned();
        let actor = Arc::new(Actor::spawn(&path_str, READ_WRITE, true, None).unwrap());
        actor
            .run(|c| {
                c.execute_batch(
                    "PRAGMA journal_mode = WAL; CREATE TABLE t (id INTEGER); INSERT INTO t VALUES (1);",
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let wal = dir.path().join("wal.sqlite-wal");
        let shm = dir.path().join("wal.sqlite-shm");
        assert!(
            wal.exists() && shm.exists(),
            "an open WAL database has both files"
        );

        // A job that does not look at the interrupt keeps the thread busy for a while.
        let busy = {
            let actor = actor.clone();
            tokio::spawn(async move {
                actor
                    .run(|_| {
                        std::thread::sleep(Duration::from_millis(400));
                        Ok(())
                    })
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(80)).await;
        let started = Instant::now();
        actor.close();
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_millis(200),
            "close returned after {waited:?} while the thread was still running a job"
        );
        // The thread is gone: the connection closed, the WAL was checkpointed and removed.
        assert!(!wal.exists() && !shm.exists(), "WAL files left behind");
        assert!(actor.thread.lock().unwrap().is_none());
        let _ = busy.await;
        // Closing twice is harmless and quick.
        let again = Instant::now();
        actor.close();
        assert!(again.elapsed() < Duration::from_millis(100));
        // The row survived the checkpoint.
        let conn = Connection::open(&path).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    /// A thread that ignores the close for longer than the limit is not waited for forever; the
    /// next `close` can still wait for it.
    #[test]
    fn the_wait_is_bounded_and_the_handle_kept_for_another_try() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("slow.sqlite")
            .to_string_lossy()
            .into_owned();
        let actor = Actor::spawn(&path, READ_WRITE, true, None).unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let job: Job = Box::new(move |_| {
            started_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(500));
        });
        actor
            .tx
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .send(job)
            .unwrap();
        started_rx.recv().unwrap();
        // Closed (sender dropped) but the job is still running.
        actor.tx.lock().unwrap().take();
        let started = Instant::now();
        actor.wait_for_thread(Duration::from_millis(50));
        assert!(started.elapsed() < Duration::from_millis(400));
        assert!(
            actor.thread.lock().unwrap().is_some(),
            "kept for the next close"
        );
        actor.wait_for_thread(Duration::from_secs(5));
        assert!(actor.thread.lock().unwrap().is_none());
    }
}
