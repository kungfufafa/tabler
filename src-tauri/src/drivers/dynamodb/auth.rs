//! Where a session's AWS credentials come from, by the profile's `DynamoAuth`: the access key
//! (the profile's user and secret, with the session token secret), a named profile of the shared
//! files (keys or `credential_process`), Tabler's environment, the password's Command mode
//! (`credential_process` JSON, or the secret access key alone) and DynamoDB Local's dummy keys.
//! The credentials, the shared files and `credential_process` are `drivers::common::aws`'s; what
//! only DynamoDB needs (Local's keys, the Command mode's output) stays here.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::future::BoxFuture;

use super::params::{DynamoAuth, DynamoParams};
use crate::drivers::common::aws::credentials::{
    self as aws, CredentialProvider, Credentials, Fetch, REFRESH_MARGIN,
};
use crate::error::AppError;
use crate::secrets::{self, CommandPassword};
use crate::types::driver::ExtraSecret;
use crate::types::profile::{ConnectionProfile, PasswordMode};

/// DynamoDB Local takes any key; the key id and region name its database file unless it runs
/// with `-sharedDb`, so the profile's access key id is used when it has one.
pub const LOCAL_KEY_ID: &str = "local";
pub const LOCAL_SECRET: &str = "local";

/// Reads a variable of Tabler's environment (tests pass their own).
pub type Env<'a> = &'a (dyn Fn(&str) -> Option<String> + Send + Sync);

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

/// DynamoDB Local's dummy keys (`key_id`: the profile's access key id, if any).
pub fn local(key_id: Option<&str>) -> Credentials {
    Credentials {
        access_key_id: non_empty(key_id).unwrap_or(LOCAL_KEY_ID).to_string(),
        secret_access_key: LOCAL_SECRET.into(),
        session_token: None,
        expiration: None,
    }
}

/// The output of the password's Command mode: `credential_process` JSON (from its first line that
/// starts with `{`, so a login script's greeting before it does no harm), or else the secret
/// access key on its last non-empty line, with `key_id` (the profile's access key id).
pub fn from_command_output(output: &str, key_id: Option<&str>) -> Result<Credentials, AppError> {
    let lines: Vec<&str> = output.lines().collect();
    if let Some(start) = lines.iter().position(|l| l.trim_start().starts_with('{')) {
        return Credentials::from_process_json(&lines[start..].join("\n"));
    }
    let secret = lines
        .iter()
        .map(|l| l.trim())
        .rfind(|l| !l.is_empty())
        .ok_or_else(|| AppError::Auth("the password command printed nothing".into()))?;
    let id = non_empty(key_id).ok_or_else(|| {
        AppError::InvalidInput(
            "DynamoDB: the command printed a secret key but the profile has no access key ID; \
             enter it, or make the command print credential_process JSON"
                .into(),
        )
    })?;
    Credentials::static_keys(id, secret, None)
}

/// Whether what the connect's password command printed (its last non-empty line) is all a
/// credential needs: JSON on one line, or a bare secret. The last line of JSON that spans lines
/// (`}`) is not, and the command runs again for its whole output.
fn whole_output(last_line: &str) -> bool {
    let s = last_line.trim();
    (s.starts_with('{') && s.ends_with('}')) || (!s.is_empty() && !s.contains(['{', '}', '"']))
}

/// What signs a session's requests.
pub struct Source {
    pub provider: Arc<CredentialProvider>,
    /// The Command mode's command, run again when DynamoDB refuses its credentials as expired.
    pub command: Option<Arc<CommandPassword>>,
    /// A named AWS profile's `endpoint_url` (a local or VPC endpoint), for a profile without one.
    pub endpoint: Option<String>,
}

impl Source {
    fn fixed(creds: Credentials) -> Source {
        Source {
            provider: Arc::new(CredentialProvider::fixed(creds)),
            command: None,
            endpoint: None,
        }
    }
}

/// Whether the profile's credentials come from its password command.
pub fn uses_command(profile: &ConnectionProfile, params: &DynamoParams) -> bool {
    matches!(params.auth, DynamoAuth::AccessKey | DynamoAuth::Command)
        && profile.options.password_mode() == PasswordMode::Command
}

/// The credentials of `profile` (its secret: `password`), read from this machine.
pub async fn source(
    profile: &ConnectionProfile,
    params: &DynamoParams,
    password: Option<&str>,
) -> Result<Source, AppError> {
    source_with(profile, params, password, &|k| std::env::var(k).ok()).await
}

/// [`source`] with `env` reading the environment (the keys, and where the shared files are).
pub async fn source_with(
    profile: &ConnectionProfile,
    params: &DynamoParams,
    password: Option<&str>,
    env: Env<'_>,
) -> Result<Source, AppError> {
    let key_id = non_empty(profile.user.as_deref());
    if uses_command(profile, params) {
        let command = profile.options.password_command().ok_or_else(|| {
            AppError::InvalidInput(
                "Enter the command that prints the credentials (Command Line mode)".into(),
            )
        })?;
        let ttl = Duration::from_secs(profile.driver.descriptor().command_ttl_secs.into());
        let cmd = Arc::new(
            CommandPassword::with_fetch(ttl, run_whole(command.to_string()))
                .seeded(password.filter(|p| whole_output(p))),
        );
        return Ok(Source {
            provider: command_provider(cmd.clone(), key_id.map(str::to_string)),
            command: Some(cmd),
            endpoint: None,
        });
    }
    match params.auth {
        DynamoAuth::AccessKey => {
            let token = secrets::extra_secret(ExtraSecret::SessionToken);
            Ok(Source::fixed(Credentials::static_keys(
                key_id.unwrap_or_default(),
                password.unwrap_or_default(),
                token.as_deref(),
            )?))
        }
        DynamoAuth::Command => Err(AppError::InvalidInput(
            "DynamoDB: the Command source reads the password's Command Line mode; choose it \
             for the secret access key"
                .into(),
        )),
        DynamoAuth::Profile => {
            let (creds_file, config_file) = aws::shared_files(env);
            let profile = aws::resolve_profile(
                params.profile(),
                read(creds_file).await.as_deref(),
                read(config_file).await.as_deref(),
            )?;
            Ok(Source {
                provider: Arc::new(CredentialProvider::for_profile(&profile)),
                command: None,
                endpoint: profile.endpoint_url,
            })
        }
        DynamoAuth::Environment => Ok(Source::fixed(Credentials::from_env_with(env)?)),
        DynamoAuth::Local => Ok(Source::fixed(local(key_id))),
    }
}

async fn read(path: Option<PathBuf>) -> Option<String> {
    tokio::fs::read_to_string(path?).await.ok()
}

/// Runs the Command mode's command for all it prints (`credential_process` JSON spans lines,
/// where a password command's last line is enough).
fn run_whole(
    command: String,
) -> impl Fn() -> BoxFuture<'static, Result<String, AppError>> + Send + Sync + 'static {
    move || {
        let command = command.clone();
        Box::pin(async move {
            secrets::run_command_output(
                &command,
                secrets::PASSWORD_COMMAND_TIMEOUT,
                "password command",
            )
            .await
        })
    }
}

/// The credentials the command prints, kept as long as they are good; its output is reused for
/// the command's time to live (the descriptor's `command_ttl_secs`) unless it is about to expire.
pub fn command_provider(
    cmd: Arc<CommandPassword>,
    key_id: Option<String>,
) -> Arc<CredentialProvider> {
    let fetch: Fetch = Arc::new(move || {
        let (cmd, key_id) = (cmd.clone(), key_id.clone());
        Box::pin(async move {
            let creds = from_command_output(&cmd.password().await?, key_id.as_deref())?;
            if !creds.expires_within(Utc::now(), REFRESH_MARGIN) {
                return Ok(creds);
            }
            cmd.invalidate().await;
            from_command_output(&cmd.password().await?, key_id.as_deref())
        })
    });
    Arc::new(CredentialProvider::fetching(fetch, None))
}

/// The names of the profiles in this machine's shared files, for the form's picker.
pub async fn profile_names(env: Env<'_>) -> Vec<String> {
    let (creds_file, config_file) = aws::shared_files(env);
    let mut names: Vec<String> = Vec::new();
    if let Some(text) = read(creds_file).await {
        names.extend(aws::parse_ini(&text).into_keys());
    }
    if let Some(text) = read(config_file).await {
        for section in aws::parse_ini(&text).into_keys() {
            match section.strip_prefix("profile ") {
                Some(name) => names.push(name.trim().to_string()),
                None if section == "default" => names.push(section),
                // `sso-session …`, `services …`: not profiles.
                None => {}
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::secrets::ExtraSecrets;
    use crate::types::profile::Driver;

    fn profile(user: Option<&str>, mode: PasswordMode, command: Option<&str>) -> ConnectionProfile {
        let mut p: ConnectionProfile = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(),
            "name": "dynamo",
            "driver": "dynamodb",
            "database": "us-east-1",
            "user": user,
        }))
        .unwrap();
        p.options.password_mode = Some(mode);
        p.options.password_command = command.map(str::to_string);
        assert_eq!(p.driver, Driver::Dynamodb);
        p
    }

    fn params(auth: DynamoAuth) -> DynamoParams {
        DynamoParams {
            auth,
            ..DynamoParams::default()
        }
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn local_keys_keep_the_profile_s_key_id() {
        assert_eq!(local(None).access_key_id, LOCAL_KEY_ID);
        assert_eq!(local(Some("  ")).access_key_id, LOCAL_KEY_ID);
        let mine = local(Some(" me "));
        assert_eq!(
            (mine.access_key_id.as_str(), mine.secret_access_key.as_str()),
            ("me", LOCAL_SECRET)
        );
    }

    #[test]
    fn command_output_is_json_or_the_secret() {
        let json = from_command_output(
            r#"{"Version":1,"AccessKeyId":"A","SecretAccessKey":"S","SessionToken":"T"}"#,
            None,
        )
        .unwrap();
        assert_eq!(
            (json.access_key_id.as_str(), json.session_token.as_deref()),
            ("A", Some("T"))
        );
        // `aws configure export-credentials --format process` prints it over several lines,
        // after whatever a login script printed.
        let spread = "Welcome!\n{\n  \"Version\": 1,\n  \"AccessKeyId\": \"B\",\n  \
                      \"SecretAccessKey\": \"S\",\n  \"Expiration\": \"2099-01-01T00:00:00Z\"\n}\n";
        let spread = from_command_output(spread, None).unwrap();
        assert_eq!(spread.access_key_id, "B");
        assert!(spread.expiration.is_some());
        let secret = from_command_output("motd line\r\n  sk \r\n\n", Some("AKID")).unwrap();
        assert_eq!(
            (
                secret.access_key_id.as_str(),
                secret.secret_access_key.as_str()
            ),
            ("AKID", "sk")
        );
        assert!(matches!(
            from_command_output("sk", None),
            Err(AppError::InvalidInput(_))
        ));
        assert!(matches!(
            from_command_output(" \n", Some("A")),
            Err(AppError::Auth(_))
        ));
        assert!(from_command_output("{\"Version\": 2}", None).is_err());
    }

    #[test]
    fn a_last_line_is_whole_output_unless_it_ends_json() {
        assert!(whole_output("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY"));
        assert!(whole_output(
            r#"{"Version":1,"AccessKeyId":"A","SecretAccessKey":"S"}"#
        ));
        for partial in ["}", "  }", "\"Expiration\": \"x\"", ""] {
            assert!(!whole_output(partial), "{partial}");
        }
    }

    #[tokio::test]
    async fn access_keys_take_the_session_token_secret() {
        let p = profile(Some("AKID"), PasswordMode::Keychain, None);
        let mut extras = ExtraSecrets::default();
        extras.set(ExtraSecret::SessionToken, "tok".into());
        let source = extras
            .scope(source_with(
                &p,
                &params(DynamoAuth::AccessKey),
                Some("sk"),
                &no_env,
            ))
            .await
            .unwrap();
        let creds = source.provider.get(Utc::now()).await.unwrap();
        assert_eq!(
            (
                creds.access_key_id.as_str(),
                creds.secret_access_key.as_str(),
                creds.session_token.as_deref()
            ),
            ("AKID", "sk", Some("tok"))
        );
        assert!(source.command.is_none());
        // No secret: refused before anything is sent.
        assert!(matches!(
            source_with(&p, &params(DynamoAuth::AccessKey), None, &no_env).await,
            Err(AppError::Auth(_))
        ));
    }

    #[tokio::test]
    async fn the_environment_and_local_keys() {
        let p = profile(None, PasswordMode::None, None);
        let env = |k: &str| match k {
            "AWS_ACCESS_KEY_ID" => Some("ENVID".to_string()),
            "AWS_SECRET_ACCESS_KEY" => Some("envsecret".to_string()),
            _ => None,
        };
        let source = source_with(&p, &params(DynamoAuth::Environment), None, &env)
            .await
            .unwrap();
        assert_eq!(
            source.provider.get(Utc::now()).await.unwrap().access_key_id,
            "ENVID"
        );
        assert!(
            source_with(&p, &params(DynamoAuth::Environment), None, &no_env)
                .await
                .is_err()
        );
        let local = source_with(&p, &params(DynamoAuth::Local), Some("ignored"), &no_env)
            .await
            .unwrap();
        assert_eq!(
            local.provider.get(Utc::now()).await.unwrap().access_key_id,
            LOCAL_KEY_ID
        );
    }

    #[tokio::test]
    async fn named_profiles_come_from_the_shared_files() {
        let dir = tempfile::tempdir().unwrap();
        let creds = dir.path().join("credentials");
        let config = dir.path().join("config");
        std::fs::write(
            &creds,
            "[default]\naws_access_key_id = DEF\naws_secret_access_key = s\n\n\
             [dev]\naws_access_key_id = DEV\naws_secret_access_key = s\n",
        )
        .unwrap();
        std::fs::write(
            &config,
            "[profile dev]\nendpoint_url = http://localhost:8000\n\n\
             [profile sso]\nsso_session = corp\n\n[sso-session corp]\nsso_region = x\n",
        )
        .unwrap();
        let (c, f) = (
            creds.to_string_lossy().into_owned(),
            config.to_string_lossy().into_owned(),
        );
        let env = move |k: &str| match k {
            "AWS_SHARED_CREDENTIALS_FILE" => Some(c.clone()),
            "AWS_CONFIG_FILE" => Some(f.clone()),
            _ => None,
        };
        let p = profile(None, PasswordMode::None, None);
        let mut dev = params(DynamoAuth::Profile);
        dev.profile_name = Some("dev".into());
        let source = source_with(&p, &dev, None, &env).await.unwrap();
        assert_eq!(
            source.provider.get(Utc::now()).await.unwrap().access_key_id,
            "DEV"
        );
        assert_eq!(source.endpoint.as_deref(), Some("http://localhost:8000"));
        let default = source_with(&p, &params(DynamoAuth::Profile), None, &env)
            .await
            .unwrap();
        assert_eq!(
            default
                .provider
                .get(Utc::now())
                .await
                .unwrap()
                .access_key_id,
            "DEF"
        );
        let mut sso = params(DynamoAuth::Profile);
        sso.profile_name = Some("sso".into());
        match source_with(&p, &sso, None, &env).await {
            Err(AppError::Unsupported(m)) => assert!(m.contains("export-credentials"), "{m}"),
            other => panic!("{:?}", other.err()),
        }
        assert_eq!(profile_names(&env).await, ["default", "dev", "sso"]);
    }

    /// A command that counts its runs and prints `outputs` in turn (the last one again).
    fn counting(outputs: Vec<String>) -> (Arc<CommandPassword>, Arc<AtomicUsize>) {
        let runs = Arc::new(AtomicUsize::new(0));
        let seen = runs.clone();
        let cmd = CommandPassword::with_fetch(Duration::from_secs(600), move || {
            let n = seen.fetch_add(1, Ordering::SeqCst);
            let out = outputs[n.min(outputs.len() - 1)].clone();
            Box::pin(async move { Ok(out) })
        });
        (Arc::new(cmd), runs)
    }

    #[tokio::test]
    async fn command_output_is_reused_until_it_expires_or_is_refused() {
        let json = |id: &str, expires: chrono::DateTime<Utc>| {
            format!(
                "{{\"Version\":1,\"AccessKeyId\":\"{id}\",\"SecretAccessKey\":\"s\",\
                 \"Expiration\":\"{}\"}}",
                expires.to_rfc3339()
            )
        };
        let later = Utc::now() + chrono::Duration::hours(1);
        let (cmd, runs) = counting(vec![json("A", later), json("B", later)]);
        let provider = command_provider(cmd.clone(), None);
        assert_eq!(provider.get(Utc::now()).await.unwrap().access_key_id, "A");
        assert_eq!(provider.get(Utc::now()).await.unwrap().access_key_id, "A");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        // Refused as expired: the client forgets both, and the command runs again.
        provider.invalidate().await;
        cmd.invalidate().await;
        assert_eq!(provider.get(Utc::now()).await.unwrap().access_key_id, "B");
        assert_eq!(runs.load(Ordering::SeqCst), 2);

        // Output about to expire within the command's TTL runs the command again at once.
        let soon = Utc::now() + chrono::Duration::minutes(1);
        let (cmd, runs) = counting(vec![json("C", soon), json("D", later)]);
        let provider = command_provider(cmd, None);
        assert_eq!(provider.get(Utc::now()).await.unwrap().access_key_id, "D");
        assert_eq!(runs.load(Ordering::SeqCst), 2);

        // A connect's output seeds it: nothing runs.
        let (cmd, runs) = counting(vec!["never".into()]);
        let seeded = Arc::new(
            Arc::into_inner(cmd)
                .unwrap()
                .seeded(Some("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY")),
        );
        let provider = command_provider(seeded, Some("AKID".into()));
        assert_eq!(
            provider.get(Utc::now()).await.unwrap().access_key_id,
            "AKID"
        );
        assert_eq!(runs.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn the_command_source_needs_the_command_mode() {
        let keychain = profile(Some("AKID"), PasswordMode::Keychain, None);
        assert!(matches!(
            source_with(&keychain, &params(DynamoAuth::Command), Some("x"), &no_env).await,
            Err(AppError::InvalidInput(_))
        ));
        // The access key with a Command Line password reads the command too; a connect's last
        // line seeds it when it is the whole output.
        let command = profile(Some("AKID"), PasswordMode::Command, Some("print-secret"));
        let source = source_with(
            &command,
            &params(DynamoAuth::AccessKey),
            Some("sk"),
            &no_env,
        )
        .await
        .unwrap();
        assert!(source.command.is_some());
        let creds = source.provider.get(Utc::now()).await.unwrap();
        assert_eq!(
            (
                creds.access_key_id.as_str(),
                creds.secret_access_key.as_str()
            ),
            ("AKID", "sk")
        );
        assert!(uses_command(&command, &params(DynamoAuth::Command)));
        assert!(!uses_command(&command, &params(DynamoAuth::Local)));
    }
}
