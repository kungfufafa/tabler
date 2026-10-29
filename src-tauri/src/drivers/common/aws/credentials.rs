//! AWS credentials and where they come from: keys given directly (with a session token), the
//! environment, a named profile of `~/.aws/credentials` and `~/.aws/config` (keys or a
//! `credential_process`), and the JSON a `credential_process` prints. Credentials that expire
//! are fetched again five minutes before they do, or once the server refused them
//! (`CredentialProvider::invalidate`).
//!
//! Assume-role, SSO and web-identity profiles are not resolved here: their error names the
//! remedy (`credential_process = aws configure export-credentials --profile … --format process`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::future::BoxFuture;

use crate::error::AppError;

/// Credentials that expire are fetched again this long before they do.
pub const REFRESH_MARGIN: Duration = Duration::from_secs(5 * 60);

/// How long a `credential_process` may run.
pub const CREDENTIAL_PROCESS_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    /// When temporary credentials stop working (`None`: they do not).
    pub expiration: Option<DateTime<Utc>>,
}

/// Secrets never show in logs or panics.
impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"***")
            .field("session_token", &self.session_token.as_ref().map(|_| "***"))
            .field("expiration", &self.expiration)
            .finish()
    }
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl Credentials {
    /// Keys as given (trimmed); an empty key id or secret is refused, an empty token is none.
    pub fn static_keys(
        access_key_id: &str,
        secret_access_key: &str,
        session_token: Option<&str>,
    ) -> Result<Credentials, AppError> {
        let (Some(id), Some(secret)) = (
            non_empty(Some(access_key_id)),
            non_empty(Some(secret_access_key)),
        ) else {
            return Err(AppError::Auth(
                "AWS credentials need an access key ID and a secret access key".into(),
            ));
        };
        Ok(Credentials {
            access_key_id: id,
            secret_access_key: secret,
            session_token: non_empty(session_token),
            expiration: None,
        })
    }

    /// `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and `AWS_SESSION_TOKEN` of Tabler's
    /// environment.
    pub fn from_env() -> Result<Credentials, AppError> {
        Credentials::from_env_with(&|k| std::env::var(k).ok())
    }

    /// [`Credentials::from_env`] with `get` reading a variable.
    pub fn from_env_with(get: &dyn Fn(&str) -> Option<String>) -> Result<Credentials, AppError> {
        match (
            non_empty(get("AWS_ACCESS_KEY_ID").as_deref()),
            non_empty(get("AWS_SECRET_ACCESS_KEY").as_deref()),
        ) {
            (Some(id), Some(secret)) => {
                Credentials::static_keys(&id, &secret, get("AWS_SESSION_TOKEN").as_deref())
            }
            _ => Err(AppError::Auth(
                "AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY are not set in Tabler's environment"
                    .into(),
            )),
        }
    }

    /// What a `credential_process` prints (version 1): `{"Version": 1, "AccessKeyId": …,
    /// "SecretAccessKey": …, "SessionToken": …, "Expiration": "2026-10-29T12:00:00Z"}`.
    pub fn from_process_json(text: &str) -> Result<Credentials, AppError> {
        let bad = |why: &str| {
            AppError::Auth(format!(
                "the credential process printed {why}; it must print JSON with Version 1, \
                 AccessKeyId and SecretAccessKey"
            ))
        };
        let v: serde_json::Value =
            serde_json::from_str(text.trim()).map_err(|_| bad("something that is not JSON"))?;
        match v.get("Version").and_then(serde_json::Value::as_i64) {
            Some(1) => {}
            Some(n) => return Err(bad(&format!("Version {n}"))),
            None => return Err(bad("no Version")),
        }
        let field = |k: &str| v.get(k).and_then(serde_json::Value::as_str);
        let id = field("AccessKeyId").ok_or_else(|| bad("no AccessKeyId"))?;
        let secret = field("SecretAccessKey").ok_or_else(|| bad("no SecretAccessKey"))?;
        let mut creds = Credentials::static_keys(id, secret, field("SessionToken"))
            .map_err(|_| bad("an empty key"))?;
        creds.expiration = match field("Expiration") {
            Some(e) => Some(
                DateTime::parse_from_rfc3339(e.trim())
                    .map_err(|_| bad(&format!("an Expiration that is not a date ({e})")))?
                    .with_timezone(&Utc),
            ),
            None => None,
        };
        Ok(creds)
    }

    /// Profile `name` of this machine's shared files, resolved now (a `credential_process` runs
    /// once; [`CredentialProvider::from_profile`] runs it again when its output expires).
    pub async fn from_profile(name: &str) -> Result<Credentials, AppError> {
        CredentialProvider::from_profile(name)
            .await?
            .get(Utc::now())
            .await
    }

    /// Whether these credentials stop working within `margin` of `now`.
    pub fn expires_within(&self, now: DateTime<Utc>, margin: Duration) -> bool {
        let margin = chrono::Duration::from_std(margin).unwrap_or(chrono::Duration::MAX);
        self.expiration.is_some_and(|e| e <= now + margin)
    }
}

/// The region of the environment (`AWS_REGION`, then `AWS_DEFAULT_REGION`).
pub fn env_region(get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    non_empty(get("AWS_REGION").as_deref())
        .or_else(|| non_empty(get("AWS_DEFAULT_REGION").as_deref()))
}

// --- the shared files -------------------------------------------------------------------------

/// The sections of an AWS config or credentials file: `[name]` → `key = value`. Comment lines
/// start with `#` or `;`; an indented line belongs to a nested setting (`s3 =`) and is skipped.
/// A section name's inner spaces are collapsed (`[profile  dev]` is `profile dev`).
pub fn parse_ini(text: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut sections: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('[') {
            current = rest
                .strip_suffix(']')
                .map(|name| name.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|n| !n.is_empty());
            if let Some(name) = &current {
                sections.entry(name.clone()).or_default();
            }
            continue;
        }
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let (Some(section), Some((key, value))) = (&current, trimmed.split_once('=')) else {
            continue;
        };
        sections
            .entry(section.clone())
            .or_default()
            .insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    sections
}

/// Where a named profile's credentials come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileCredentials {
    Static(Credentials),
    /// `credential_process`: a command printing [`Credentials::from_process_json`]'s JSON.
    Process(String),
}

/// A named profile of the shared files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwsProfile {
    pub credentials: ProfileCredentials,
    pub region: Option<String>,
    /// The profile's `endpoint_url` (a local or VPC endpoint).
    pub endpoint_url: Option<String>,
}

/// Settings of profiles not resolved here, and the kind of credentials they name.
const UNSUPPORTED: &[(&str, &str)] = &[
    ("role_arn", "assume-role"),
    ("sso_session", "SSO"),
    ("sso_start_url", "SSO"),
    ("web_identity_token_file", "web identity"),
    ("login_session", "console login"),
];

/// Resolves profile `name` (empty: `default`) from the texts of the credentials and config files,
/// in the AWS CLI's order: assume-role and SSO (refused), the credentials file's keys,
/// `credential_process` (either file, the credentials file's first), the config file's keys.
pub fn resolve_profile(
    name: &str,
    credentials_file: Option<&str>,
    config_file: Option<&str>,
) -> Result<AwsProfile, AppError> {
    let name = match name.trim() {
        "" => "default",
        n => n,
    };
    let creds = credentials_file.map(parse_ini).unwrap_or_default();
    let config = config_file.map(parse_ini).unwrap_or_default();
    let from_creds = creds.get(name);
    let from_config = config
        .get(&format!("profile {name}"))
        .or_else(|| (name == "default").then(|| config.get("default")).flatten());
    if from_creds.is_none() && from_config.is_none() {
        return Err(AppError::Auth(format!(
            "the AWS profile \u{201c}{name}\u{201d} is not in ~/.aws/credentials or ~/.aws/config"
        )));
    }
    let get = |key: &str| {
        from_creds
            .and_then(|s| s.get(key))
            .or_else(|| from_config.and_then(|s| s.get(key)))
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    };
    if let Some((_, what)) = UNSUPPORTED.iter().find(|(key, _)| get(key).is_some()) {
        return Err(AppError::Unsupported(format!(
            "the AWS profile \u{201c}{name}\u{201d} uses {what} credentials, which Tabler does not \
             resolve itself: add `credential_process = aws configure export-credentials --profile \
             {name} --format process` to a profile and use that one"
        )));
    }
    let keys = |section: Option<&BTreeMap<String, String>>| {
        let section = section?;
        Credentials::static_keys(
            section.get("aws_access_key_id")?,
            section.get("aws_secret_access_key")?,
            section.get("aws_session_token").map(String::as_str),
        )
        .ok()
    };
    let credentials = if let Some(c) = keys(from_creds) {
        ProfileCredentials::Static(c)
    } else if let Some(command) = get("credential_process") {
        ProfileCredentials::Process(command.to_string())
    } else if let Some(c) = keys(from_config) {
        ProfileCredentials::Static(c)
    } else {
        return Err(AppError::Auth(format!(
            "the AWS profile \u{201c}{name}\u{201d} has no aws_access_key_id and \
             aws_secret_access_key, and no credential_process"
        )));
    };
    Ok(AwsProfile {
        credentials,
        region: get("region").map(str::to_string),
        endpoint_url: get("endpoint_url").map(str::to_string),
    })
}

/// The home folder (`USERPROFILE` on Windows, `HOME` elsewhere).
fn home_dir(get: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let vars: &[&str] = if cfg!(windows) {
        &["USERPROFILE", "HOME"]
    } else {
        &["HOME"]
    };
    vars.iter()
        .find_map(|v| non_empty(get(v).as_deref()))
        .map(PathBuf::from)
}

/// The credentials and config files: `AWS_SHARED_CREDENTIALS_FILE` and `AWS_CONFIG_FILE`, else
/// `~/.aws/credentials` and `~/.aws/config` (`~/` in a variable is the home folder too).
pub fn shared_files(get: &dyn Fn(&str) -> Option<String>) -> (Option<PathBuf>, Option<PathBuf>) {
    let home = home_dir(get);
    let file = |var: &str, default: &str| match non_empty(get(var).as_deref()) {
        Some(p) => match p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
            Some(rest) => home.as_ref().map(|h| h.join(rest)),
            None => Some(PathBuf::from(p)),
        },
        None => home.as_ref().map(|h| h.join(".aws").join(default)),
    };
    (
        file("AWS_SHARED_CREDENTIALS_FILE", "credentials"),
        file("AWS_CONFIG_FILE", "config"),
    )
}

/// Profile `name` of this machine's shared files.
pub async fn load_profile(name: &str) -> Result<AwsProfile, AppError> {
    let (creds, config) = shared_files(&|k| std::env::var(k).ok());
    async fn read(path: Option<PathBuf>) -> Option<String> {
        tokio::fs::read_to_string(path?).await.ok()
    }
    let creds = read(creds).await;
    let config = read(config).await;
    resolve_profile(name, creds.as_deref(), config.as_deref())
}

// --- the provider a signer uses -----------------------------------------------------------------

/// Runs a command and returns everything it printed.
pub type RunCommand =
    Arc<dyn Fn(String) -> BoxFuture<'static, Result<String, AppError>> + Send + Sync>;

/// Fetches fresh credentials.
pub type Fetch = Arc<dyn Fn() -> BoxFuture<'static, Result<Credentials, AppError>> + Send + Sync>;

/// Runs a `credential_process` through the user's login shell, like a password command (see
/// `secrets::run_password_command`), and returns all it printed.
pub fn credential_process_runner() -> RunCommand {
    Arc::new(|command: String| {
        Box::pin(async move {
            crate::secrets::run_command_output(
                &command,
                CREDENTIAL_PROCESS_TIMEOUT,
                "credential_process command",
            )
            .await
        })
    })
}

/// The credentials a signer uses: fixed ones, or ones fetched again before they expire.
pub struct CredentialProvider {
    fetch: Option<Fetch>,
    cached: tokio::sync::Mutex<Option<Credentials>>,
}

impl std::fmt::Debug for CredentialProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialProvider")
            .field("refreshable", &self.refreshable())
            .finish_non_exhaustive()
    }
}

impl CredentialProvider {
    pub fn fixed(creds: Credentials) -> CredentialProvider {
        CredentialProvider {
            fetch: None,
            cached: tokio::sync::Mutex::new(Some(creds)),
        }
    }

    /// Credentials from `fetch`, kept until five minutes before they expire (for good when they
    /// do not) or until [`CredentialProvider::invalidate`]; `first` is what a connect fetched.
    pub fn fetching(fetch: Fetch, first: Option<Credentials>) -> CredentialProvider {
        CredentialProvider {
            fetch: Some(fetch),
            cached: tokio::sync::Mutex::new(first),
        }
    }

    /// The JSON `command` prints, run through `run`.
    pub fn process(command: String, run: RunCommand) -> CredentialProvider {
        let fetch: Fetch = Arc::new(move || {
            let (run, command) = (run.clone(), command.clone());
            Box::pin(async move { Credentials::from_process_json(&run(command).await?) })
        });
        CredentialProvider::fetching(fetch, None)
    }

    /// The credentials of profile `name` of this machine's shared files.
    pub async fn from_profile(name: &str) -> Result<CredentialProvider, AppError> {
        Ok(CredentialProvider::for_profile(&load_profile(name).await?))
    }

    pub fn for_profile(profile: &AwsProfile) -> CredentialProvider {
        match &profile.credentials {
            ProfileCredentials::Static(c) => CredentialProvider::fixed(c.clone()),
            ProfileCredentials::Process(command) => {
                CredentialProvider::process(command.clone(), credential_process_runner())
            }
        }
    }

    /// Whether credentials the server refused can be replaced by fetching again.
    pub fn refreshable(&self) -> bool {
        self.fetch.is_some()
    }

    /// The credentials to sign with at `now`.
    pub async fn get(&self, now: DateTime<Utc>) -> Result<Credentials, AppError> {
        let mut cached = self.cached.lock().await;
        if let Some(c) = cached.as_ref()
            && (self.fetch.is_none() || !c.expires_within(now, REFRESH_MARGIN))
        {
            return Ok(c.clone());
        }
        let Some(fetch) = &self.fetch else {
            return Err(AppError::Auth("no AWS credentials".into()));
        };
        let fresh = fetch().await?;
        if fresh.expires_within(now, Duration::ZERO) {
            return Err(AppError::Auth(
                "the credential process printed credentials that have already expired".into(),
            ));
        }
        *cached = Some(fresh.clone());
        Ok(fresh)
    }

    /// Forgets fetched credentials, so the next [`CredentialProvider::get`] fetches again (fixed
    /// ones stay).
    pub async fn invalidate(&self) {
        if self.fetch.is_some() {
            *self.cached.lock().await = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
    }

    #[test]
    fn keys_are_trimmed_and_secrets_hidden() {
        let c = Credentials::static_keys(" AKID ", " secret\n", Some("  ")).unwrap();
        assert_eq!(c.access_key_id, "AKID");
        assert_eq!(c.secret_access_key, "secret");
        assert_eq!(c.session_token, None);
        let c = Credentials::static_keys("AKID", "secret", Some("tok")).unwrap();
        let shown = format!("{c:?}");
        assert!(shown.contains("\"AKID\""), "{shown}");
        assert!(
            !shown.contains("\"secret\"") && !shown.contains("\"tok\""),
            "{shown}"
        );
        assert_eq!(
            Credentials::static_keys("", "s", None).unwrap_err().code(),
            "AUTH_FAILED"
        );
    }

    #[test]
    fn credential_process_json_parses() {
        let c = Credentials::from_process_json(
            r#"{
  "Version": 1,
  "AccessKeyId": "ASIAEXAMPLE",
  "SecretAccessKey": "s3cret",
  "SessionToken": "tok",
  "Expiration": "2026-10-29T12:00:00Z"
}"#,
        )
        .unwrap();
        assert_eq!(c.access_key_id, "ASIAEXAMPLE");
        assert_eq!(c.session_token.as_deref(), Some("tok"));
        assert_eq!(
            c.expiration.unwrap().to_rfc3339(),
            "2026-10-29T12:00:00+00:00"
        );
        let forever = Credentials::from_process_json(
            r#"{"Version":1,"AccessKeyId":"a","SecretAccessKey":"b"}"#,
        )
        .unwrap();
        assert_eq!(forever.expiration, None);
        for (bad, why) in [
            ("not json", "not JSON"),
            (r#"{"AccessKeyId":"a","SecretAccessKey":"b"}"#, "no Version"),
            (
                r#"{"Version":2,"AccessKeyId":"a","SecretAccessKey":"b"}"#,
                "Version 2",
            ),
            (r#"{"Version":1,"SecretAccessKey":"b"}"#, "no AccessKeyId"),
            (r#"{"Version":1,"AccessKeyId":"a"}"#, "no SecretAccessKey"),
            (
                r#"{"Version":1,"AccessKeyId":"a","SecretAccessKey":"b","Expiration":"soon"}"#,
                "not a date",
            ),
        ] {
            let err = Credentials::from_process_json(bad).unwrap_err();
            assert_eq!(err.code(), "AUTH_FAILED");
            assert!(err.to_string().contains(why), "{err}");
        }
    }

    #[test]
    fn the_environment_holds_keys_and_a_region() {
        let get = env(&[
            ("AWS_ACCESS_KEY_ID", "AKID"),
            ("AWS_SECRET_ACCESS_KEY", "s"),
            ("AWS_SESSION_TOKEN", "t"),
            ("AWS_DEFAULT_REGION", "eu-west-1"),
        ]);
        let c = Credentials::from_env_with(&get).unwrap();
        assert_eq!(
            (c.access_key_id.as_str(), c.session_token.as_deref()),
            ("AKID", Some("t"))
        );
        assert_eq!(env_region(&get).as_deref(), Some("eu-west-1"));
        let get = env(&[
            ("AWS_REGION", "us-east-2"),
            ("AWS_DEFAULT_REGION", "eu-west-1"),
        ]);
        assert_eq!(env_region(&get).as_deref(), Some("us-east-2"));
        assert_eq!(
            Credentials::from_env_with(&get).unwrap_err().code(),
            "AUTH_FAILED"
        );
    }

    const CREDENTIALS_FILE: &str = "\
# keys of the CLI
[default]
aws_access_key_id = AKIDDEFAULT
aws_secret_access_key = default-secret

[dev]
aws_access_key_id=AKIDDEV
aws_secret_access_key=dev-secret
aws_session_token=dev-token
region = ap-south-1
";

    const CONFIG_FILE: &str = "\
[default]
region = us-west-2

[profile dev]
region = eu-central-1
s3 =
  max_concurrent_requests = 20

[profile  tools]
credential_process = aws-vault export --format=json tools
region=us-east-1
endpoint_url = http://localhost:8000

; config-only keys
[profile ci]
aws_access_key_id = AKIDCI
aws_secret_access_key = ci-secret

[profile sso]
sso_session = corp
sso_account_id = 1

[profile both]
credential_process = never-run
";

    #[test]
    fn shared_files_parse_like_the_cli() {
        let ini = parse_ini(CONFIG_FILE);
        assert_eq!(ini["profile dev"]["region"], "eu-central-1");
        // A nested setting's lines are not keys of the profile.
        assert!(!ini["profile dev"].contains_key("max_concurrent_requests"));
        assert!(ini.contains_key("profile tools"));

        let resolve = |name| resolve_profile(name, Some(CREDENTIALS_FILE), Some(CONFIG_FILE));
        let default = resolve("").unwrap();
        assert_eq!(
            default.credentials,
            ProfileCredentials::Static(
                Credentials::static_keys("AKIDDEFAULT", "default-secret", None).unwrap()
            )
        );
        assert_eq!(default.region.as_deref(), Some("us-west-2"));
        // The credentials file's settings win over the config file's.
        let dev = resolve("dev").unwrap();
        assert_eq!(dev.region.as_deref(), Some("ap-south-1"));
        let ProfileCredentials::Static(c) = dev.credentials else {
            panic!("static keys")
        };
        assert_eq!(c.session_token.as_deref(), Some("dev-token"));
        let tools = resolve("tools").unwrap();
        assert_eq!(
            tools.credentials,
            ProfileCredentials::Process("aws-vault export --format=json tools".into())
        );
        assert_eq!(tools.endpoint_url.as_deref(), Some("http://localhost:8000"));
        assert!(matches!(
            resolve("ci").unwrap().credentials,
            ProfileCredentials::Static(_)
        ));
        let sso = resolve("sso").unwrap_err();
        assert_eq!(sso.code(), "UNSUPPORTED");
        assert!(sso.to_string().contains("credential_process"), "{sso}");
        assert_eq!(resolve("missing").unwrap_err().code(), "AUTH_FAILED");
        // Keys in the credentials file come before a credential_process.
        let both = resolve_profile(
            "both",
            Some("[both]\naws_access_key_id=A\naws_secret_access_key=B\n"),
            Some(CONFIG_FILE),
        )
        .unwrap();
        assert!(matches!(both.credentials, ProfileCredentials::Static(_)));
        // Windows line ends.
        let crlf = CREDENTIALS_FILE.replace('\n', "\r\n");
        assert!(resolve_profile("dev", Some(&crlf), None).is_ok());
    }

    #[test]
    fn shared_file_locations_follow_the_environment() {
        let home = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let (creds, config) = shared_files(&env(&[(home, "/home/ann")]));
        assert_eq!(
            creds,
            Some(PathBuf::from("/home/ann").join(".aws").join("credentials"))
        );
        assert_eq!(
            config,
            Some(PathBuf::from("/home/ann").join(".aws").join("config"))
        );
        let (creds, config) = shared_files(&env(&[
            (home, "/home/ann"),
            ("AWS_SHARED_CREDENTIALS_FILE", "~/keys"),
            ("AWS_CONFIG_FILE", "/etc/aws.conf"),
        ]));
        assert_eq!(creds, Some(PathBuf::from("/home/ann").join("keys")));
        assert_eq!(config, Some(PathBuf::from("/etc/aws.conf")));
        assert_eq!(shared_files(&env(&[])), (None, None));
    }

    fn counting(outputs: Vec<String>) -> (RunCommand, Arc<AtomicUsize>) {
        let runs = Arc::new(AtomicUsize::new(0));
        let counter = runs.clone();
        let run: RunCommand = Arc::new(move |command: String| {
            assert_eq!(command, "creds");
            let n = counter.fetch_add(1, Ordering::SeqCst);
            let out = outputs.get(n).cloned().unwrap_or_default();
            Box::pin(async move { Ok(out) })
        });
        (run, runs)
    }

    #[tokio::test]
    async fn processes_run_again_before_their_credentials_expire() {
        let now = Utc::now();
        let soon = (now + chrono::Duration::minutes(3)).to_rfc3339();
        let later = (now + chrono::Duration::hours(1)).to_rfc3339();
        let first = format!(
            r#"{{"Version":1,"AccessKeyId":"A1","SecretAccessKey":"s","Expiration":"{later}"}}"#
        );
        let second = format!(
            r#"{{"Version":1,"AccessKeyId":"A2","SecretAccessKey":"s","Expiration":"{later}"}}"#
        );
        let expiring = format!(
            r#"{{"Version":1,"AccessKeyId":"A0","SecretAccessKey":"s","Expiration":"{soon}"}}"#
        );
        let (run, runs) = counting(vec![expiring, first, second]);
        let p = CredentialProvider::process("creds".into(), run);
        assert!(p.refreshable());
        // Within five minutes of its end: fetched again at once.
        assert_eq!(p.get(now).await.unwrap().access_key_id, "A0");
        assert_eq!(p.get(now).await.unwrap().access_key_id, "A1");
        assert_eq!(p.get(now).await.unwrap().access_key_id, "A1");
        assert_eq!(runs.load(Ordering::SeqCst), 2);
        p.invalidate().await;
        assert_eq!(p.get(now).await.unwrap().access_key_id, "A2");
        assert_eq!(runs.load(Ordering::SeqCst), 3);
        // Already expired output is refused.
        let past = (now - chrono::Duration::minutes(1)).to_rfc3339();
        let dead = format!(
            r#"{{"Version":1,"AccessKeyId":"A","SecretAccessKey":"s","Expiration":"{past}"}}"#
        );
        let (run, _) = counting(vec![dead]);
        let p = CredentialProvider::process("creds".into(), run);
        assert_eq!(p.get(now).await.unwrap_err().code(), "AUTH_FAILED");
    }

    #[tokio::test]
    async fn fixed_credentials_stay() {
        let c = Credentials::static_keys("A", "s", None).unwrap();
        let p = CredentialProvider::fixed(c.clone());
        assert!(!p.refreshable());
        p.invalidate().await;
        assert_eq!(p.get(Utc::now()).await.unwrap(), c);
        let profile = resolve_profile("dev", Some(CREDENTIALS_FILE), None).unwrap();
        let p = CredentialProvider::for_profile(&profile);
        assert_eq!(p.get(Utc::now()).await.unwrap().access_key_id, "AKIDDEV");
    }

    /// The real runner hands over every line a command printed (JSON spans lines).
    #[tokio::test]
    async fn the_runner_keeps_every_line() {
        use crate::secrets::{LoginShell, login_shell};
        let command = match login_shell() {
            LoginShell::Posix(_) => {
                "printf '{\"Version\": 1,\\n\"AccessKeyId\": \"A\", \"SecretAccessKey\": \"s\"}\\n'"
            }
            #[cfg(windows)]
            LoginShell::Cmd(_) => {
                "echo {\"Version\": 1,& echo \"AccessKeyId\": \"A\", \"SecretAccessKey\": \"s\"}"
            }
        };
        let out = credential_process_runner()(command.into()).await.unwrap();
        assert_eq!(out.lines().count(), 2, "{out:?}");
        let c = Credentials::from_process_json(&out).unwrap();
        assert_eq!(c.access_key_id, "A");
        let err = credential_process_runner()("".into()).await.unwrap_err();
        assert_eq!(err.code(), "INVALID_INPUT");
    }
}
