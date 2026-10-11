use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use rusqlite::{Connection, InterruptHandle, OpenFlags};

use crate::error::AppError;

type Job = Box<dyn FnOnce(&mut Connection) + Send + 'static>;

/// A dedicated thread owning one `rusqlite::Connection`; jobs are closures run in order.
pub struct Actor {
    tx: Mutex<Option<mpsc::Sender<Job>>>,
    interrupt: Arc<InterruptHandle>,
    /// Set by `close`: jobs still queued are answered with `NotConnected` instead of running.
    closed: Arc<AtomicBool>,
}

impl Actor {
    pub fn spawn(path: &str, flags: OpenFlags, foreign_keys: bool) -> Result<Actor, AppError> {
        let conn = Connection::open_with_flags(path, flags)
            .map_err(|e| AppError::Connect(format!("cannot open {path}: {e}")))?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        conn.execute_batch(if foreign_keys {
            "PRAGMA foreign_keys = ON;"
        } else {
            "PRAGMA foreign_keys = OFF;"
        })?;
        let interrupt = Arc::new(conn.get_interrupt_handle());
        let (tx, rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("tabler-sqlite".into())
            .spawn(move || {
                let mut conn = conn;
                while let Ok(job) = rx.recv() {
                    job(&mut conn);
                }
            })
            .map_err(|e| AppError::Internal(format!("spawn sqlite thread: {e}")))?;
        Ok(Actor {
            tx: Mutex::new(Some(tx)),
            interrupt,
            closed: Arc::new(AtomicBool::new(false)),
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
    /// the sender, so the thread exits (closing the connection and its file locks) right away.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.interrupt.interrupt();
        let mut guard = self.tx.lock().unwrap_or_else(|p| p.into_inner());
        guard.take();
    }
}
