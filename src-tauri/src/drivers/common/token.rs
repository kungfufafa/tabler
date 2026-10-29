//! The secret a token-authenticated driver sends: none, the stored one, or what the profile's
//! Command Line mode prints, reused for the driver's `command_ttl_secs` and run again once the
//! server refuses it (`invalidate`, which `HttpClient` calls on a 401).

use std::sync::Arc;
use std::time::Duration;

use crate::error::AppError;
use crate::secrets::{CommandPassword, PASSWORD_COMMAND_TTL};
use crate::types::profile::ConnectionProfile;

pub enum TokenSource {
    /// Nothing to send (no secret, or an empty one).
    None,
    Static(String),
    Command(Arc<CommandPassword>),
}

/// Secrets never show in logs or panics.
impl std::fmt::Debug for TokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TokenSource::None => "TokenSource::None",
            TokenSource::Static(_) => "TokenSource::Static(***)",
            TokenSource::Command(_) => "TokenSource::Command",
        })
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

    /// The secret to send now (`None`: nothing). A command runs when its last output is older
    /// than the TTL or was invalidated.
    pub async fn get(&self) -> Result<Option<String>, AppError> {
        match self {
            TokenSource::None => Ok(None),
            TokenSource::Static(s) => Ok(Some(s.clone())),
            TokenSource::Command(command) => {
                Ok(Some(command.password().await?).filter(|t| !t.is_empty()))
            }
        }
    }

    /// The server refused the secret: a command runs again for the next request (a stored secret
    /// stays, nothing would change it).
    pub async fn invalidate(&self) {
        if let TokenSource::Command(command) = self {
            command.invalidate().await;
        }
    }

    /// Whether [`TokenSource::invalidate`] can bring a different secret.
    pub fn refreshes(&self) -> bool {
        matches!(self, TokenSource::Command(_))
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

    #[tokio::test]
    async fn an_empty_command_output_sends_nothing() {
        let command = CommandPassword::with_fetch(Duration::from_secs(60), || {
            Box::pin(async { Ok(String::new()) })
        });
        let t = TokenSource::Command(Arc::new(command));
        assert_eq!(t.get().await.unwrap(), None);
    }
}
