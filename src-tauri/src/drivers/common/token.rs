//! The secret a token-authenticated driver sends: none, the stored one, what the profile's
//! Command Line mode prints (reused for the driver's `command_ttl_secs`), or what a login issues
//! for as long as it says (an OAuth token endpoint's access token). A command or a login runs
//! again once the server refuses the token (`invalidate`, which `HttpClient` calls on a 401).

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::time::Instant;

use crate::error::AppError;
use crate::secrets::{CommandPassword, PASSWORD_COMMAND_TTL};
use crate::types::profile::ConnectionProfile;

pub enum TokenSource {
    /// Nothing to send (no secret, or an empty one).
    None,
    Static(String),
    Command(Arc<CommandPassword>),
    Issued(Arc<IssuedTokens>),
}

/// Secrets never show in logs or panics.
impl std::fmt::Debug for TokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TokenSource::None => "TokenSource::None",
            TokenSource::Static(_) => "TokenSource::Static(***)",
            TokenSource::Command(_) => "TokenSource::Command",
            TokenSource::Issued(_) => "TokenSource::Issued",
        })
    }
}

/// A token a login issued and how long it lasts (`expires_in`; `None`: until a server refuses
/// it).
#[derive(Clone)]
pub struct IssuedToken {
    pub token: String,
    pub expires_in: Option<Duration>,
}

impl std::fmt::Debug for IssuedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedToken")
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

/// Asks the login for a token (a signed assertion or a refresh token sent to the token
/// endpoint).
pub type FetchToken =
    Arc<dyn Fn() -> BoxFuture<'static, Result<IssuedToken, AppError>> + Send + Sync>;

/// A token is fetched again this long before it expires, or after a quarter of its lifetime
/// when that is shorter.
pub const ISSUED_REFRESH_MARGIN: Duration = Duration::from_secs(5 * 60);

/// The tokens of a login: fetched when first needed, again shortly before they expire, and once
/// more after a server refused one. One fetch serves every request waiting for it.
pub struct IssuedTokens {
    fetch: FetchToken,
    /// The token and when to fetch the next (`None`: never, until invalidated).
    cached: tokio::sync::Mutex<Option<(String, Option<Instant>)>>,
}

impl IssuedTokens {
    pub fn new(fetch: FetchToken) -> IssuedTokens {
        IssuedTokens {
            fetch,
            cached: tokio::sync::Mutex::new(None),
        }
    }

    /// A token that should still work.
    pub async fn token(&self) -> Result<String, AppError> {
        let mut cached = self.cached.lock().await;
        if let Some((token, renew_at)) = cached.as_ref()
            && renew_at.is_none_or(|at| Instant::now() < at)
        {
            return Ok(token.clone());
        }
        let fresh = (self.fetch)().await?;
        // A lifetime past what a clock can count is none.
        let renew_at = fresh.expires_in.and_then(|lifetime| {
            let margin = ISSUED_REFRESH_MARGIN.min(lifetime / 4);
            Instant::now().checked_add(lifetime.saturating_sub(margin))
        });
        *cached = Some((fresh.token.clone(), renew_at));
        Ok(fresh.token)
    }

    /// Forgets the token a server refused.
    pub async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }
}

impl TokenSource {
    /// The profile's secret: its password command (Command Line mode, `secret` being what the
    /// command printed for this connect) reused for the driver's `command_ttl_secs`, else the
    /// stored `secret`.
    pub fn for_profile(p: &ConnectionProfile, secret: Option<&str>) -> TokenSource {
        let ttl = match p.driver.descriptor().command_ttl_secs {
            0 => PASSWORD_COMMAND_TTL,
            secs => Duration::from_secs(u64::from(secs)),
        };
        match CommandPassword::for_profile_ttl(p, secret, ttl) {
            Some(command) => TokenSource::Command(command),
            None => TokenSource::from_secret(secret),
        }
    }

    /// A stored secret; an empty one sends nothing.
    pub fn from_secret(secret: Option<&str>) -> TokenSource {
        match secret.filter(|s| !s.is_empty()) {
            Some(s) => TokenSource::Static(s.to_string()),
            None => TokenSource::None,
        }
    }

    /// The tokens `fetch` gets from a login (see [`IssuedTokens`]).
    pub fn issued(fetch: FetchToken) -> TokenSource {
        TokenSource::Issued(Arc::new(IssuedTokens::new(fetch)))
    }

    /// The secret to send now (`None`: nothing). A command runs when its last output is older
    /// than the TTL or was invalidated; a login when its token is about to expire or was
    /// invalidated.
    pub async fn get(&self) -> Result<Option<String>, AppError> {
        match self {
            TokenSource::None => Ok(None),
            TokenSource::Static(s) => Ok(Some(s.clone())),
            TokenSource::Command(command) => {
                Ok(Some(command.password().await?).filter(|t| !t.is_empty()))
            }
            TokenSource::Issued(tokens) => {
                Ok(Some(tokens.token().await?).filter(|t| !t.is_empty()))
            }
        }
    }

    /// The server refused the secret: a command or a login runs again for the next request (a
    /// stored secret stays, nothing would change it).
    pub async fn invalidate(&self) {
        match self {
            TokenSource::Command(command) => command.invalidate().await,
            TokenSource::Issued(tokens) => tokens.invalidate().await,
            TokenSource::None | TokenSource::Static(_) => {}
        }
    }

    /// Whether [`TokenSource::invalidate`] can bring a different secret.
    pub fn refreshes(&self) -> bool {
        matches!(self, TokenSource::Command(_) | TokenSource::Issued(_))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::types::profile::{Driver, PasswordMode, ProfileOptions, SslMode};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A profile of `driver` for `db.example.test`, in Command Line mode with `command`.
    pub(crate) fn profile(driver: Driver, command: Option<&str>) -> ConnectionProfile {
        let mut options = ProfileOptions::default();
        if let Some(command) = command {
            options.password_mode = Some(PasswordMode::Command);
            options.password_command = Some(command.into());
        }
        ConnectionProfile {
            id: uuid::Uuid::nil(),
            name: "t".into(),
            color: None,
            group: None,
            driver,
            host: Some("db.example.test".into()),
            port: None,
            user: None,
            database: None,
            ssl: SslMode::Require,
            ssl_ca_path: None,
            sqlite_path: None,
            ssh: None,
            options,
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        }
    }

    /// A Command token whose runs are counted: run n prints `tok-n`.
    pub(crate) fn counted(ttl: Duration) -> (TokenSource, Arc<AtomicUsize>) {
        let runs = Arc::new(AtomicUsize::new(0));
        let counter = runs.clone();
        let command = CommandPassword::with_fetch(ttl, move || {
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move { Ok(format!("tok-{n}")) })
        });
        (TokenSource::Command(Arc::new(command)), runs)
    }

    #[tokio::test]
    async fn stored_secrets_are_sent_and_empty_ones_are_not() {
        let p = profile(Driver::Clickhouse, None);
        let t = TokenSource::for_profile(&p, Some("s3cret"));
        assert_eq!(t.get().await.unwrap().as_deref(), Some("s3cret"));
        assert!(!t.refreshes());
        t.invalidate().await;
        assert_eq!(t.get().await.unwrap().as_deref(), Some("s3cret"));
        for secret in [None, Some("")] {
            let t = TokenSource::for_profile(&p, secret);
            assert!(matches!(t, TokenSource::None));
            assert_eq!(t.get().await.unwrap(), None);
        }
        assert_eq!(format!("{t:?}"), "TokenSource::Static(***)");
    }

    #[tokio::test]
    async fn command_tokens_are_reused_until_invalidated() {
        let p = profile(Driver::Bigquery, Some("print-token"));
        // What the command printed at connect serves first.
        let t = TokenSource::for_profile(&p, Some("from-connect"));
        assert!(t.refreshes());
        assert_eq!(t.get().await.unwrap().as_deref(), Some("from-connect"));

        let (t, runs) = counted(Duration::from_secs(600));
        assert_eq!(t.get().await.unwrap().as_deref(), Some("tok-1"));
        assert_eq!(t.get().await.unwrap().as_deref(), Some("tok-1"));
        t.invalidate().await;
        assert_eq!(t.get().await.unwrap().as_deref(), Some("tok-2"));
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    /// A login whose fetches are counted: fetch n issues `tok-n` lasting `lifetime`.
    pub(crate) fn issued(lifetime: Option<Duration>) -> (TokenSource, Arc<AtomicUsize>) {
        let fetches = Arc::new(AtomicUsize::new(0));
        let counter = fetches.clone();
        let fetch: FetchToken = Arc::new(move || {
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move {
                Ok(IssuedToken {
                    token: format!("tok-{n}"),
                    expires_in: lifetime,
                })
            })
        });
        (TokenSource::issued(fetch), fetches)
    }

    #[tokio::test(start_paused = true)]
    async fn issued_tokens_last_until_shortly_before_they_expire() {
        let (t, fetches) = issued(Some(Duration::from_secs(3600)));
        assert!(t.refreshes());
        assert_eq!(t.get().await.unwrap().as_deref(), Some("tok-1"));
        // Five minutes before the hour is up, the next request gets a new one.
        tokio::time::advance(Duration::from_secs(3600 - 301)).await;
        assert_eq!(t.get().await.unwrap().as_deref(), Some("tok-1"));
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(t.get().await.unwrap().as_deref(), Some("tok-2"));
        // A refused token is replaced at once.
        t.invalidate().await;
        assert_eq!(t.get().await.unwrap().as_deref(), Some("tok-3"));
        assert_eq!(fetches.load(Ordering::SeqCst), 3);
        assert_eq!(format!("{t:?}"), "TokenSource::Issued");

        // A short-lived token is kept for three quarters of its life, not fetched every time.
        let (t, fetches) = issued(Some(Duration::from_secs(60)));
        t.get().await.unwrap();
        tokio::time::advance(Duration::from_secs(44)).await;
        t.get().await.unwrap();
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(t.get().await.unwrap().as_deref(), Some("tok-2"));

        // Without a lifetime it stays until refused.
        let (t, fetches) = issued(None);
        t.get().await.unwrap();
        tokio::time::advance(Duration::from_secs(86_400)).await;
        t.get().await.unwrap();
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_failed_login_is_the_caller_s_error_and_is_tried_again() {
        let fails = Arc::new(AtomicUsize::new(0));
        let counter = fails.clone();
        let t = TokenSource::issued(Arc::new(move || {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                match n {
                    0 => Err(AppError::Auth("the login was refused".into())),
                    _ => Ok(IssuedToken {
                        token: "ok".into(),
                        expires_in: None,
                    }),
                }
            })
        }));
        assert_eq!(t.get().await.unwrap_err().code(), "AUTH_FAILED");
        assert_eq!(t.get().await.unwrap().as_deref(), Some("ok"));
        let shown = format!(
            "{:?}",
            IssuedToken {
                token: "secret".into(),
                expires_in: None
            }
        );
        assert!(!shown.contains("secret"), "{shown}");
    }

    #[tokio::test]
    async fn an_empty_command_output_sends_nothing() {
        let command = CommandPassword::with_fetch(Duration::from_secs(60), || {
            Box::pin(async { Ok(String::new()) })
        });
        let t = TokenSource::Command(Arc::new(command));
        assert_eq!(t.get().await.unwrap(), None);
    }
}
