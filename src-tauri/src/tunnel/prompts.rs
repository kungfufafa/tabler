//! Keyboard-interactive (2FA) questions of SSH servers, forwarded to the user.
//!
//! [`super::open_tunnel`] asks a [`KeyboardInteractive`] for the answers to each round of
//! questions the server sends. In the app that is a [`ForwardedPrompts`]: it emits the round as an
//! `ssh:prompt` event, the renderer's dialog answers through the `answer_ssh_prompt` command
//! ([`PendingPrompts::answer`]), and `ssh:prompt-done` tells the renderer to drop the dialog when
//! the backend stops waiting (answered, cancelled, timed out or the connect went away).
//!
//! The time the user spends answering does not count against the connect timeout:
//! [`timeout_excluding_prompts`] with the [`PromptClock`] that a [`ClockedPrompts`] runs.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use ts_rs::TS;
use uuid::Uuid;

use crate::error::AppError;

/// How long the backend waits for the user to answer one round of questions.
pub const PROMPT_ANSWER_TIMEOUT: Duration = Duration::from_secs(300);
/// The connect-timeout check interval while a question is on screen.
const PAUSED_RECHECK: Duration = Duration::from_millis(250);

/// One question of a keyboard-interactive round.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SshPromptField {
    /// The server's text, e.g. `Verification code: `.
    pub prompt: String,
    /// Whether the answer may be shown while typed (false: a secret).
    pub echo: bool,
}

/// One round of keyboard-interactive questions, with who is signing in where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptRound {
    pub user: String,
    pub host: String,
    pub port: u16,
    pub name: String,
    pub instructions: String,
    pub prompts: Vec<SshPromptField>,
    /// `None`: the server's own questions.
    pub kind: Option<SshPromptKind>,
}

/// A question the app asks itself rather than the SSH server's keyboard-interactive round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SshPromptKind {
    /// "Ask every time": the SSH password (one hidden field).
    Password,
    /// A yes / no question with no field (e.g. downloading a large SQLite file over SSH); the
    /// answer is an empty list, cancelling says no.
    Confirm,
}

/// Answers keyboard-interactive questions.
#[async_trait]
pub trait KeyboardInteractive: Send + Sync {
    /// One answer per prompt of `round`; `Err(AppError::Cancelled)` when the user cancels (the
    /// connect is then given up).
    async fn answer(&self, round: PromptRound) -> Result<Vec<String>, AppError>;
}

/// What a connect is for, so the dialog can say "Connect" or "Test".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum SshPromptPurpose {
    Connect,
    Test,
}

/// Payload of the global `ssh:prompt` event: the SSH server asks the user something.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SshPromptRequest {
    /// Passed back to `answer_ssh_prompt`.
    pub request_id: String,
    pub profile_id: Uuid,
    /// The connection's name.
    pub label: String,
    pub purpose: SshPromptPurpose,
    pub user: String,
    pub host: String,
    pub port: u16,
    /// The server's title for the round (often empty).
    pub name: String,
    /// The server's instruction text (often empty).
    pub instructions: String,
    pub prompts: Vec<SshPromptField>,
    /// Set for the app's own questions (the SSH password, a confirmation); absent for the
    /// server's keyboard-interactive rounds.
    #[serde(default)]
    #[ts(optional)]
    pub kind: Option<SshPromptKind>,
}

/// Payload of the global `ssh:prompt-done` event: the backend no longer waits for this answer.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SshPromptDone {
    pub request_id: String,
}

type Answer = Option<Vec<String>>;

/// Questions waiting for the user's answer, by request id.
#[derive(Default)]
pub struct PendingPrompts(Mutex<HashMap<String, oneshot::Sender<Answer>>>);

impl PendingPrompts {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, oneshot::Sender<Answer>>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// A new request id and the receiver of its answer.
    pub fn register(&self) -> (String, oneshot::Receiver<Answer>) {
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.map().insert(id.clone(), tx);
        (id, rx)
    }

    /// Delivers the user's answers (`None`: cancelled). False when nobody waits for `id` any more.
    pub fn answer(&self, id: &str, answers: Answer) -> bool {
        match self.map().remove(id) {
            Some(tx) => tx.send(answers).is_ok(),
            None => false,
        }
    }

    pub fn forget(&self, id: &str) {
        self.map().remove(id);
    }

    pub fn len(&self) -> usize {
        self.map().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// What [`ForwardedPrompts`] tells the renderer.
#[derive(Debug, Clone)]
pub enum PromptEvent {
    Ask(SshPromptRequest),
    Done(SshPromptDone),
}

/// Forwards each round to the user (`emit`) and waits for the answer given to `pending`.
pub struct ForwardedPrompts {
    pending: Arc<PendingPrompts>,
    emit: Box<dyn Fn(PromptEvent) + Send + Sync>,
    profile_id: Uuid,
    label: String,
    purpose: SshPromptPurpose,
    timeout: Duration,
}

impl ForwardedPrompts {
    pub fn new(
        pending: Arc<PendingPrompts>,
        emit: impl Fn(PromptEvent) + Send + Sync + 'static,
        profile_id: Uuid,
        label: String,
        purpose: SshPromptPurpose,
    ) -> ForwardedPrompts {
        ForwardedPrompts {
            pending,
            emit: Box::new(emit),
            profile_id,
            label,
            purpose,
            timeout: PROMPT_ANSWER_TIMEOUT,
        }
    }

    /// Waits at most `timeout` for each answer (tests).
    pub fn with_timeout(mut self, timeout: Duration) -> ForwardedPrompts {
        self.timeout = timeout;
        self
    }
}

/// Forgets the pending question and tells the renderer, however the wait ends.
struct DoneGuard<'a> {
    owner: &'a ForwardedPrompts,
    request_id: String,
}

impl Drop for DoneGuard<'_> {
    fn drop(&mut self) {
        self.owner.pending.forget(&self.request_id);
        (self.owner.emit)(PromptEvent::Done(SshPromptDone {
            request_id: self.request_id.clone(),
        }));
    }
}

#[async_trait]
impl KeyboardInteractive for ForwardedPrompts {
    async fn answer(&self, round: PromptRound) -> Result<Vec<String>, AppError> {
        let (request_id, answer) = self.pending.register();
        let _done = DoneGuard {
            owner: self,
            request_id: request_id.clone(),
        };
        let expected = round.prompts.len();
        (self.emit)(PromptEvent::Ask(SshPromptRequest {
            request_id,
            profile_id: self.profile_id,
            label: self.label.clone(),
            purpose: self.purpose,
            user: round.user,
            host: round.host,
            port: round.port,
            name: round.name,
            instructions: round.instructions,
            prompts: round.prompts,
            kind: round.kind,
        }));
        match tokio::time::timeout(self.timeout, answer).await {
            Ok(Ok(Some(answers))) if answers.len() == expected => Ok(answers),
            Ok(Ok(Some(answers))) => Err(AppError::InvalidInput(format!(
                "the SSH server asked {expected} question(s), {} answer(s) were given",
                answers.len()
            ))),
            // Cancelled, or the registry went away.
            Ok(Ok(None)) | Ok(Err(_)) => Err(AppError::Cancelled),
            Err(_) => Err(AppError::Tunnel(format!(
                "nobody answered the SSH server's question within {} minutes",
                self.timeout.as_secs() / 60
            ))),
        }
    }
}

/// Time spent waiting for the user's answers.
#[derive(Debug, Default)]
pub struct PromptClock(Mutex<ClockState>);

#[derive(Debug, Default)]
struct ClockState {
    /// Questions on screen right now (nested waits count once).
    open: usize,
    since: Option<Instant>,
    total: Duration,
}

impl PromptClock {
    fn state(&self) -> std::sync::MutexGuard<'_, ClockState> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn begin(&self) {
        let mut s = self.state();
        if s.open == 0 {
            s.since = Some(Instant::now());
        }
        s.open += 1;
    }

    fn end(&self) {
        let mut s = self.state();
        s.open = s.open.saturating_sub(1);
        if s.open == 0
            && let Some(since) = s.since.take()
        {
            s.total += since.elapsed();
        }
    }

    /// Whether a question is on screen right now.
    pub fn waiting(&self) -> bool {
        self.state().open > 0
    }

    /// Total time spent waiting for answers, the current wait included.
    pub fn waited(&self) -> Duration {
        let s = self.state();
        s.total + s.since.map(|t| t.elapsed()).unwrap_or_default()
    }
}

/// A [`KeyboardInteractive`] whose waits are timed by `clock`.
pub struct ClockedPrompts {
    inner: Arc<dyn KeyboardInteractive>,
    clock: Arc<PromptClock>,
}

impl ClockedPrompts {
    pub fn new(inner: Arc<dyn KeyboardInteractive>, clock: Arc<PromptClock>) -> ClockedPrompts {
        ClockedPrompts { inner, clock }
    }
}

struct ClockGuard<'a>(&'a PromptClock);

impl Drop for ClockGuard<'_> {
    fn drop(&mut self) {
        self.0.end();
    }
}

#[async_trait]
impl KeyboardInteractive for ClockedPrompts {
    async fn answer(&self, round: PromptRound) -> Result<Vec<String>, AppError> {
        self.clock.begin();
        let _end = ClockGuard(&self.clock);
        self.inner.answer(round).await
    }
}

/// Runs `fut` with a `timeout` that does not count the time `clock` spent waiting for the user
/// (it never expires while a question is on screen). `None` when it expired.
pub async fn timeout_excluding_prompts<F: Future>(
    timeout: Duration,
    clock: &PromptClock,
    fut: F,
) -> Option<F::Output> {
    tokio::pin!(fut);
    let started = Instant::now();
    loop {
        let now = Instant::now();
        let deadline = started + timeout + clock.waited();
        if deadline <= now && !clock.waiting() {
            return None;
        }
        let wake = if clock.waiting() {
            deadline.max(now + PAUSED_RECHECK)
        } else {
            deadline
        };
        tokio::select! {
            out = &mut fut => return Some(out),
            _ = tokio::time::sleep_until(wake.into()) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(prompts: &[(&str, bool)]) -> PromptRound {
        PromptRound {
            user: "deploy".into(),
            host: "bastion".into(),
            port: 22,
            name: String::new(),
            instructions: "Two-factor".into(),
            prompts: prompts
                .iter()
                .map(|(p, echo)| SshPromptField {
                    prompt: (*p).into(),
                    echo: *echo,
                })
                .collect(),
            kind: None,
        }
    }

    /// A `ForwardedPrompts` whose "renderer" answers with `reply` (by request id).
    fn forwarded(
        reply: impl Fn(&SshPromptRequest) -> Option<Vec<String>> + Send + Sync + 'static,
    ) -> (
        ForwardedPrompts,
        Arc<PendingPrompts>,
        Arc<Mutex<Vec<String>>>,
    ) {
        let pending = Arc::new(PendingPrompts::default());
        let log = Arc::new(Mutex::new(Vec::new()));
        let (p, l) = (pending.clone(), log.clone());
        let prompts = ForwardedPrompts::new(
            pending.clone(),
            move |event| match event {
                PromptEvent::Ask(req) => {
                    l.lock().unwrap().push(format!("ask {}", req.prompts.len()));
                    if let Some(answer) = reply(&req) {
                        let (p, id) = (p.clone(), req.request_id.clone());
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_millis(20)).await;
                            p.answer(&id, Some(answer));
                        });
                    }
                }
                PromptEvent::Done(_) => l.lock().unwrap().push("done".into()),
            },
            Uuid::nil(),
            "Staging".into(),
            SshPromptPurpose::Connect,
        );
        (prompts, pending, log)
    }

    #[tokio::test]
    async fn answers_come_back_and_the_renderer_is_told_when_done() {
        let (prompts, pending, log) = forwarded(|req| {
            assert_eq!(req.label, "Staging");
            assert_eq!(req.prompts[0].prompt, "Verification code: ");
            Some(vec!["123456".into()])
        });
        let answers = prompts
            .answer(round(&[("Verification code: ", false)]))
            .await
            .unwrap();
        assert_eq!(answers, ["123456"]);
        assert!(pending.is_empty());
        assert_eq!(*log.lock().unwrap(), ["ask 1", "done"]);
    }

    #[tokio::test]
    async fn cancel_wrong_count_and_timeout() {
        // Cancelled by the user.
        let pending = Arc::new(PendingPrompts::default());
        let p = pending.clone();
        let cancel = ForwardedPrompts::new(
            pending.clone(),
            move |event| {
                if let PromptEvent::Ask(req) = event {
                    assert!(p.answer(&req.request_id, None));
                }
            },
            Uuid::nil(),
            "x".into(),
            SshPromptPurpose::Test,
        );
        let err = cancel
            .answer(round(&[("Code: ", false)]))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Cancelled), "{err:?}");

        let (wrong, _, _) = forwarded(|_| Some(vec!["a".into(), "b".into()]));
        let err = wrong.answer(round(&[("Code: ", false)])).await.unwrap_err();
        assert_eq!(err.code(), "INVALID_INPUT");

        let (silent, pending, log) = forwarded(|_| None);
        let silent = silent.with_timeout(Duration::from_millis(50));
        let err = silent
            .answer(round(&[("Code: ", false)]))
            .await
            .unwrap_err();
        assert_eq!(err.code(), "SSH_TUNNEL");
        // The late answer finds nobody waiting.
        assert!(pending.is_empty());
        assert!(!pending.answer("whatever", Some(vec![])));
        assert_eq!(*log.lock().unwrap(), ["ask 1", "done"]);
    }

    /// Answers after `delay`, like a user typing a code.
    struct SlowUser(Duration);

    #[async_trait]
    impl KeyboardInteractive for SlowUser {
        async fn answer(&self, round: PromptRound) -> Result<Vec<String>, AppError> {
            tokio::time::sleep(self.0).await;
            Ok(round.prompts.iter().map(|_| "1".to_string()).collect())
        }
    }

    #[tokio::test]
    async fn the_connect_timeout_leaves_out_the_time_spent_answering() {
        let clock = Arc::new(PromptClock::default());
        let prompts = ClockedPrompts::new(
            Arc::new(SlowUser(Duration::from_millis(400))),
            clock.clone(),
        );
        // 400 ms answering + 2 × 60 ms of work under a 150 ms timeout: still in time.
        let work = async {
            tokio::time::sleep(Duration::from_millis(60)).await;
            let answers = prompts.answer(round(&[("Code: ", false)])).await;
            tokio::time::sleep(Duration::from_millis(60)).await;
            answers
        };
        let out = timeout_excluding_prompts(Duration::from_millis(150), &clock, work).await;
        assert_eq!(out.unwrap().unwrap(), ["1"]);
        assert!(clock.waited() >= Duration::from_millis(400));
        assert!(!clock.waiting());

        // Without questions it is a plain timeout.
        let idle = PromptClock::default();
        let slow = tokio::time::sleep(Duration::from_secs(5));
        let started = Instant::now();
        assert!(
            timeout_excluding_prompts(Duration::from_millis(100), &idle, slow)
                .await
                .is_none()
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
