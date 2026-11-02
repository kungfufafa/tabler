//! How a session signs in, kept so it can sign in again: a second session for the app's own
//! statements, a new session after Snowflake expired one, the connection form's pickers. What
//! the connect read once (the private key file, the key passphrase secret, a typed password)
//! is kept; a Command password (an OAuth token) is run again once its TTL is over, and once more
//! when Snowflake refuses the token it printed.

use std::sync::Arc;
use std::time::Duration;

use super::auth::{AuthInputs, PrivateKey, auth_config, read_private_key};
use super::conn::{self, ConnectSpec, SnowflakeConn};
use super::params::{SnowflakeAuth, SnowflakeParams, base_url};
use crate::drivers::ConnectTarget;
use crate::drivers::registry::{UtilInput, unattended};
use crate::error::AppError;
use crate::secrets::{CommandPassword, extra_secret};
use crate::types::driver::ExtraSecret;
use crate::types::profile::{ConnectionProfile, Driver};

/// How long a sign-in may take unless the profile says otherwise.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a sign-in waits for an MFA push to be approved on the phone.
const MFA_PUSH_TIMEOUT: Duration = Duration::from_secs(120);

/// Where the profile's secret (password, PAT, OAuth token) comes from.
enum Secret {
    Typed(Option<String>),
    Command(Arc<CommandPassword>),
}

pub struct SignIn {
    pub params: SnowflakeParams,
    user: String,
    database: Option<String>,
    key: Option<PrivateKey>,
    passphrase: Option<String>,
    secret: Secret,
    unattended: bool,
    timeout: Duration,
    pub response_timeout: Duration,
}

impl std::fmt::Debug for SignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignIn")
            .field("account", &self.params.account)
            .field("auth", &self.params.auth)
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

fn named(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl SignIn {
    /// The sign-in of a connect: the profile's settings, the password it was given (a Command
    /// password's output included), and the second secrets of the connect's scope.
    pub fn for_target(target: &ConnectTarget<'_>) -> Result<SignIn, AppError> {
        let profile = target.profile;
        let ttl = Duration::from_secs(u64::from(Driver::Snowflake.descriptor().command_ttl_secs));
        let secret = match CommandPassword::for_profile_ttl(profile, target.password, ttl) {
            Some(command) => Secret::Command(command),
            None => Secret::Typed(target.password.map(str::to_string)),
        };
        SignIn::new(profile, secret)
    }

    /// The sign-in of a call of the connection form, with the secret it was given.
    pub fn for_util(input: &UtilInput<'_>) -> Result<SignIn, AppError> {
        SignIn::new(
            input.profile,
            Secret::Typed(input.secret.map(str::to_string)),
        )
    }

    fn new(profile: &ConnectionProfile, secret: Secret) -> Result<SignIn, AppError> {
        let params = SnowflakeParams::from_driver_params(profile.options.driver_params.as_ref())?;
        let user = named(profile.user.as_deref())
            .ok_or_else(|| AppError::InvalidInput("Snowflake: the user is required".into()))?;
        let key = match (params.auth, params.private_key_path.as_deref()) {
            (SnowflakeAuth::KeyPair, Some(path)) => Some(read_private_key(path)?),
            _ => None,
        };
        let timeout = profile
            .options
            .connect_timeout_secs
            .filter(|s| *s > 0)
            .map_or(DEFAULT_CONNECT_TIMEOUT, |s| {
                Duration::from_secs(u64::from(s))
            });
        let timeout = if params.auth == SnowflakeAuth::Password && params.mfa {
            timeout.max(MFA_PUSH_TIMEOUT)
        } else {
            timeout
        };
        Ok(SignIn {
            database: named(profile.database.as_deref()),
            user,
            key,
            passphrase: extra_secret(ExtraSecret::KeyPassphrase),
            secret,
            unattended: unattended(),
            timeout,
            response_timeout: conn::DEFAULT_RESPONSE_TIMEOUT,
            params,
        })
    }

    /// Whether every sign-in asks a second factor (a push the user approves): the session then
    /// signs in once, and the app's statements share the editor's session.
    pub fn asks_mfa(&self) -> bool {
        self.params.auth == SnowflakeAuth::Password && self.params.mfa
    }

    /// Whether the endpoint is reached over TLS (plain http only reaches a test server on this
    /// machine).
    pub fn tls(&self) -> bool {
        self.params
            .base_url
            .as_deref()
            .is_none_or(|u| base_url(u).is_ok_and(|u| u.scheme() == "https"))
    }

    async fn spec(&self) -> Result<ConnectSpec, AppError> {
        let secret = match &self.secret {
            Secret::Typed(s) => s.clone(),
            Secret::Command(command) => Some(command.password().await?),
        };
        let inputs = AuthInputs {
            secret,
            key: self.key.clone(),
            passphrase: self.passphrase.clone(),
            // No field carries a one-time passcode into a connect: an MFA account approves a
            // push instead.
            passcode: None,
            mfa: self.params.mfa,
            unattended: self.unattended,
        };
        Ok(ConnectSpec {
            account: self.params.account.clone(),
            user: self.user.clone(),
            auth: auth_config(self.params.auth, &inputs)?,
            warehouse: self.params.warehouse.clone(),
            role: self.params.role.clone(),
            database: self.database.clone(),
            schema: self.params.schema.clone(),
            base_url: match &self.params.base_url {
                Some(url) => Some(base_url(url)?),
                None => None,
            },
            session_parameters: self.params.login_parameters(),
            response_timeout: self.response_timeout,
        })
    }

    /// Signs in once, within the sign-in time limit.
    async fn attempt(&self) -> Result<SnowflakeConn, AppError> {
        let spec = self.spec().await?;
        tokio::time::timeout(self.timeout, conn::connect(&spec))
            .await
            .map_err(|_| {
                AppError::Connect(format!(
                    "Snowflake: signing in took longer than {} s",
                    self.timeout.as_secs()
                ))
            })?
    }

    /// Signs in. A token a Command printed that Snowflake refuses is fetched once more.
    pub async fn connect(&self) -> Result<SnowflakeConn, AppError> {
        match self.attempt().await {
            Err(AppError::Auth(_)) if matches!(self.secret, Secret::Command(_)) => {
                if let Secret::Command(command) = &self.secret {
                    command.invalidate().await;
                }
                self.attempt().await
            }
            other => other,
        }
    }
}
