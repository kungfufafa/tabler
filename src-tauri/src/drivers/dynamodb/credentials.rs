//! AWS credentials and where they come from: keys typed into the profile (with a session token),
//! the environment, a named profile of `~/.aws/credentials` and `~/.aws/config` (static keys or
//! `credential_process`), the password's Command mode printing `credential_process` JSON, and
//! DynamoDB Local's dummy keys. Credentials that expire are fetched again five minutes before
//! they do, or once after the server refused them (`ExpiredTokenException`).
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

/// Credentials are fetched again this long before they expire.
pub const REFRESH_MARGIN: Duration = Duration::from_secs(5 * 60);

/// DynamoDB Local takes any key; the key id and region name its database file unless it runs
/// with `-sharedDb`, so the profile's access key id is used when it has one.
pub const LOCAL_KEY_ID: &str = "local";
pub const LOCAL_SECRET: &str = "local";

#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    /// When temporary credentials stop working (`None`: they do not expire).
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
    /// Keys as given (trimmed); a key id or secret that is empty is refused.
    pub fn new(
        access_key_id: &str,
        secret_access_key: &str,
        session_token: Option<&str>,
    ) -> Result<Credentials, AppError> {
        let id = non_empty(Some(access_key_id))
            .ok_or_else(|| AppError::InvalidInput("DynamoDB: the access key ID is empty".into()))?;
        let secret = non_empty(Some(secret_access_key)).ok_or_else(|| {
            AppError::InvalidInput("DynamoDB: the secret access key is empty".into())
        })?;
        Ok(Credentials {
            access_key_id: id,
            secret_access_key: secret,
            session_token: non_empty(session_token),
            expiration: None,
        })
    }

    /// DynamoDB Local's dummy keys (`key_id`: the profile's access key id, if any).
    pub fn local(key_id: Option<&str>) -> Credentials {
        Credentials {
            access_key_id: non_empty(key_id).unwrap_or_else(|| LOCAL_KEY_ID.into()),
            secret_access_key: LOCAL_SECRET.into(),
            session_token: None,
            expiration: None,
        }
    }

    /// Whether they expire within `margin` of `now`.
    pub fn expires_within(&self, now: DateTime<Utc>, margin: Duration) -> bool {
        let margin = chrono::Duration::from_std(margin).unwrap_or(chrono::Duration::MAX);
        self.expiration.is_some_and(|e| e <= now + margin)
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
        let field = |k: &str| v.get(k).and_then(|x| x.as_str());
        match v.get("Version").and_then(serde_json::Value::as_i64) {
            Some(1) => {}
            Some(n) => return Err(bad(&format!("Version {n}"))),
            None => return Err(bad("no Version")),
        }
        let id = field("AccessKeyId").ok_or_else(|| bad("no AccessKeyId"))?;
        let secret = field("SecretAccessKey").ok_or_else(|| bad("no SecretAccessKey"))?;
        let mut creds =
            Credentials::new(id, secret, field("SessionToken")).map_err(|_| bad("an empty key"))?;
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

    /// The output of the password's Command mode: `credential_process` JSON, or else the secret
    /// access key itself, with `key_id` (the profile's access key id).
    pub fn from_command_output(
        output: &str,
        key_id: Option<&str>,
    ) -> Result<Credentials, AppError> {
        let text = output.trim();
        if text.starts_with('{') {
            return Credentials::from_process_json(text);
        }
        let id = non_empty(key_id).ok_or_else(|| {
            AppError::InvalidInput(
                "DynamoDB: the command printed a secret key but the profile has no access key ID; \
                 enter it, or make the command print credential_process JSON"
                    .into(),
            )
        })?;
        Credentials::new(&id, text, None)
    }

    /// `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and `AWS_SESSION_TOKEN` (`get` reads a
    /// variable).
    pub fn from_env(get: &dyn Fn(&str) -> Option<String>) -> Result<Credentials, AppError> {
        let id = non_empty(get("AWS_ACCESS_KEY_ID").as_deref());
        let secret = non_empty(get("AWS_SECRET_ACCESS_KEY").as_deref());
        match (id, secret) {
            (Some(id), Some(secret)) => {
                Credentials::new(&id, &secret, get("AWS_SESSION_TOKEN").as_deref())
            }
            _ => Err(AppError::Auth(
                "AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY are not set in Tabler's environment"
                    .into(),
            )),
        }
    }
}

/// The region of the environment (`AWS_REGION`, then `AWS_DEFAULT_REGION`).
pub fn env_region(get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    non_empty(get("AWS_REGION").as_deref())
        .or_else(|| non_empty(get("AWS_DEFAULT_REGION").as_deref()))
}

// --- shared config files ---------------------------------------------------------------------

/// The sections of an AWS config or credentials file: `[name]` → `key = value`. Comment lines
/// start with `#` or `;`; indented lines continue a nested setting (`s3 =`) and are skipped.
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
                .map(|name| name.trim().to_string())
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
            .insert(key.trim().to_string(), value.trim().to_string());
    }
    sections
}

/// Where a named profile's credentials come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileCredentials {
    Static(Credentials),
    /// `credential_process`: a command that prints the JSON of [`Credentials::from_process_json`].
    Process(String),
}

/// A named profile of the shared files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwsProfile {
    pub credentials: ProfileCredentials,
    pub region: Option<String>,
    /// `endpoint_url` of the profile (a local or VPC endpoint).
    pub endpoint_url: Option<String>,
}

/// Settings of profiles Tabler does not resolve itself, with the setting that names them.
const UNSUPPORTED: &[(&str, &str)] = &[
    ("role_arn", "assume-role"),
    ("sso_session", "SSO"),
    ("sso_start_url", "SSO"),
    ("web_identity_token_file", "web identity"),
    ("login_session", "console login"),
];

/// Resolves profile `name` from the texts of the credentials file and the config file, in the
/// order the AWS CLI uses: assume-role and SSO (refused), the credentials file's keys,
/// `credential_process`, the config file's keys.
pub fn resolve_profile(
    name: &str,
    credentials_file: Option<&str>,
    config_file: Option<&str>,
) -> Result<AwsProfile, AppError> {
    let name = name.trim();
    let name = if name.is_empty() { "default" } else { name };
    let creds = credentials_file.map(parse_ini).unwrap_or_default();
    let config = config_file.map(parse_ini).unwrap_or_default();
    let from_creds = creds.get(name);
    let from_config = config.get(&format!("profile {name}")).or_else(|| {
        if name == "default" {
            config.get("default")
        } else {
            None
        }
    });
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
        let id = section.get("aws_access_key_id")?;
        let secret = section.get("aws_secret_access_key")?;
        Credentials::new(
            id,
            secret,
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
pub fn home_dir(get: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
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

/// Reads profile `name` from the shared files of this machine.
pub async fn load_profile(name: &str) -> Result<AwsProfile, AppError> {
    let get = |k: &str| std::env::var(k).ok();
    let (creds_path, config_path) = shared_files(&get);
    async fn read(path: Option<PathBuf>) -> Option<String> {
        tokio::fs::read_to_string(path?).await.ok()
    }
    let creds = read(creds_path).await;
    let config = read(config_path).await;
    resolve_profile(name, creds.as_deref(), config.as_deref())
}

// --- the provider a client signs with ---------------------------------------------------------

/// Runs a command and hands over everything it printed (a `credential_process`, the Command
/// mode).
pub type RunCommand =
    Arc<dyn Fn(String) -> BoxFuture<'static, Result<String, AppError>> + Send + Sync>;

/// Fetches fresh credentials.
pub type Fetch = Arc<dyn Fn() -> BoxFuture<'static, Result<Credentials, AppError>> + Send + Sync>;

/// The credentials a client signs with: fixed ones, or ones fetched again before they expire.
pub struct CredentialProvider {
    fetch: Option<Fetch>,
    cached: tokio::sync::Mutex<Option<Credentials>>,
}

impl CredentialProvider {
    pub fn fixed(creds: Credentials) -> CredentialProvider {
        CredentialProvider {
            fetch: None,
            cached: tokio::sync::Mutex::new(Some(creds)),
        }
    }

    /// Credentials from `fetch`, kept until five minutes before they expire (for good when they
    /// do not), or until [`CredentialProvider::invalidate`]. `first` is what a connect fetched.
    pub fn fetching(fetch: Fetch, first: Option<Credentials>) -> CredentialProvider {
        CredentialProvider {
            fetch: Some(fetch),
            cached: tokio::sync::Mutex::new(first),
        }
    }

    /// `credential_process` of a profile, run through `run`.
    pub fn process(command: String, run: RunCommand) -> CredentialProvider {
        let fetch: Fetch = Arc::new(move || {
            let run = run.clone();
            let command = command.clone();
            Box::pin(async move { Credentials::from_process_json(&run(command).await?) })
        });
        CredentialProvider::fetching(fetch, None)
    }

    /// Whether a refused token can be replaced by fetching again.
    pub fn refreshable(&self) -> bool {
        self.fetch.is_some()
    }

    /// The credentials to sign with now.
    pub async fn get(&self, now: DateTime<Utc>) -> Result<Credentials, AppError> {
        let mut cached = self.cached.lock().await;
        if let Some(c) = cached.as_ref()
            && (self.fetch.is_none() || !c.expires_within(now, REFRESH_MARGIN))
        {
            return Ok(c.clone());
        }
        let Some(fetch) = &self.fetch else {
            return Err(AppError::Auth("the AWS credentials expired".into()));
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

    /// Forgets fetched credentials so the next [`CredentialProvider::get`] fetches again (fixed
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

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
    }

    #[test]
    fn keys_are_trimmed_and_secrets_hidden() {
        let c = Credentials::new(" AKID ", " s3cret ", Some("  ")).unwrap();
        assert_eq!(c.access_key_id, "AKID");
        assert_eq!(c.secret_access_key, "s3cret");
        assert_eq!(c.session_token, None);
        let shown = format!(
            "{:?}",
            Credentials::new("AKID", "s3cret", Some("T0KVALUE")).unwrap()
        );
        assert!(
            !shown.contains("s3cret") && !shown.contains("T0KVALUE"),
            "{shown}"
        );
        assert!(Credentials::new("", "x", None).is_err());
        assert!(Credentials::new("x", " ", None).is_err());
        assert_eq!(Credentials::local(None).access_key_id, "local");
        assert_eq!(
            Credentials::local(Some("fakeMyKeyId")).access_key_id,
            "fakeMyKeyId"
        );
    }

    #[test]
    fn process_json() {
        let c = Credentials::from_process_json(
            r#"{
              "Version": 1,
              "AccessKeyId": "ASIAX",
              "SecretAccessKey": "sk",
              "SessionToken": "tok",
              "Expiration": "2026-10-29T12:00:00Z"
            }"#,
        )
        .unwrap();
        assert_eq!(c.session_token.as_deref(), Some("tok"));
        assert_eq!(
            c.expiration.unwrap().to_rfc3339(),
            "2026-10-29T12:00:00+00:00"
        );
        let plain = Credentials::from_process_json(
            r#"{"Version":1,"AccessKeyId":"A","SecretAccessKey":"S"}"#,
        )
        .unwrap();
        assert_eq!(plain.expiration, None);
        for bad in [
            "not json",
            r#"{"Version":2,"AccessKeyId":"A","SecretAccessKey":"S"}"#,
            r#"{"AccessKeyId":"A","SecretAccessKey":"S"}"#,
            r#"{"Version":1,"SecretAccessKey":"S"}"#,
            r#"{"Version":1,"AccessKeyId":"A","SecretAccessKey":"S","Expiration":"soon"}"#,
            r#"{"Version":1,"AccessKeyId":"","SecretAccessKey":"S"}"#,
        ] {
            assert!(
                matches!(Credentials::from_process_json(bad), Err(AppError::Auth(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn command_output_is_json_or_the_secret() {
        let json = Credentials::from_command_output(
            r#"{"Version":1,"AccessKeyId":"A","SecretAccessKey":"S"}"#,
            None,
        )
        .unwrap();
        assert_eq!(json.access_key_id, "A");
        let secret = Credentials::from_command_output("  sk\n", Some("AKID")).unwrap();
        assert_eq!(
            (
                secret.access_key_id.as_str(),
                secret.secret_access_key.as_str()
            ),
            ("AKID", "sk")
        );
        assert!(Credentials::from_command_output("sk", None).is_err());
    }

    #[test]
    fn environment() {
        let c = Credentials::from_env(&env(&[
            ("AWS_ACCESS_KEY_ID", "A"),
            ("AWS_SECRET_ACCESS_KEY", "S"),
            ("AWS_SESSION_TOKEN", "T"),
        ]))
        .unwrap();
        assert_eq!(c.session_token.as_deref(), Some("T"));
        assert!(Credentials::from_env(&env(&[("AWS_ACCESS_KEY_ID", "A")])).is_err());
        assert_eq!(
            env_region(&env(&[("AWS_DEFAULT_REGION", "eu-west-1")])).as_deref(),
            Some("eu-west-1")
        );
        assert_eq!(
            env_region(&env(&[
                ("AWS_REGION", "us-east-2"),
                ("AWS_DEFAULT_REGION", "x")
            ]))
            .as_deref(),
            Some("us-east-2")
        );
    }

    const CREDS: &str = "\
# keys
[default]
aws_access_key_id = AKIDDEFAULT
aws_secret_access_key = skdefault

[dev]
aws_access_key_id=AKIDDEV
aws_secret_access_key=skdev
aws_session_token=tokdev
";

    const CONFIG: &str = "\
[default]
region = us-east-1

[profile dev]
region = eu-west-1
s3 =
  max_concurrent_requests = 20

[profile proc]
credential_process = aws configure export-credentials --profile sso --format process
region = ap-southeast-1
endpoint_url = http://localhost:8000

[profile cfgkeys]
aws_access_key_id = AKIDCFG
aws_secret_access_key = skcfg

[profile role]
role_arn = arn:aws:iam::123:role/r
source_profile = default

[profile sso]
sso_session = corp

[profile empty]
region = us-west-2
";

    #[test]
    fn ini_sections_and_nested_settings() {
        let s = parse_ini(CONFIG);
        assert_eq!(s["profile dev"]["region"], "eu-west-1");
        assert!(!s["profile dev"].contains_key("max_concurrent_requests"));
        assert_eq!(s["profile dev"]["s3"], "");
        assert_eq!(parse_ini("[a]\r\nk = v\r\n")["a"]["k"], "v");
        assert!(parse_ini("k = orphan\n[]\n").is_empty());
    }

    #[test]
    fn profiles_resolve_like_the_cli() {
        let p = resolve_profile("default", Some(CREDS), Some(CONFIG)).unwrap();
        assert_eq!(p.region.as_deref(), Some("us-east-1"));
        assert!(
            matches!(p.credentials, ProfileCredentials::Static(ref c) if c.access_key_id == "AKIDDEFAULT")
        );
        assert_eq!(
            resolve_profile("  ", Some(CREDS), None).unwrap().region,
            None
        );
        let dev = resolve_profile("dev", Some(CREDS), Some(CONFIG)).unwrap();
        assert_eq!(dev.region.as_deref(), Some("eu-west-1"));
        match dev.credentials {
            ProfileCredentials::Static(c) => assert_eq!(c.session_token.as_deref(), Some("tokdev")),
            other => panic!("{other:?}"),
        }
        let proc = resolve_profile("proc", Some(CREDS), Some(CONFIG)).unwrap();
        assert_eq!(
            proc.credentials,
            ProfileCredentials::Process(
                "aws configure export-credentials --profile sso --format process".into()
            )
        );
        assert_eq!(proc.endpoint_url.as_deref(), Some("http://localhost:8000"));
        assert!(matches!(
            resolve_profile("cfgkeys", None, Some(CONFIG))
                .unwrap()
                .credentials,
            ProfileCredentials::Static(_)
        ));
        for (name, word) in [("role", "assume-role"), ("sso", "SSO")] {
            match resolve_profile(name, Some(CREDS), Some(CONFIG)) {
                Err(AppError::Unsupported(m)) => {
                    assert!(m.contains(word) && m.contains("credential_process"), "{m}")
                }
                other => panic!("{other:?}"),
            }
        }
        assert!(matches!(
            resolve_profile("empty", Some(CREDS), Some(CONFIG)),
            Err(AppError::Auth(_))
        ));
        assert!(matches!(
            resolve_profile("nope", Some(CREDS), Some(CONFIG)),
            Err(AppError::Auth(_))
        ));
    }

    #[test]
    fn shared_file_locations() {
        let home = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let (c, f) = shared_files(&env(&[(home, "/home/u")]));
        assert_eq!(
            c.unwrap(),
            PathBuf::from("/home/u").join(".aws").join("credentials")
        );
        assert_eq!(
            f.unwrap(),
            PathBuf::from("/home/u").join(".aws").join("config")
        );
        let (c, f) = shared_files(&env(&[
            (home, "/home/u"),
            ("AWS_SHARED_CREDENTIALS_FILE", "~/keys"),
            ("AWS_CONFIG_FILE", "/etc/aws"),
        ]));
        assert_eq!(c.unwrap(), PathBuf::from("/home/u").join("keys"));
        assert_eq!(f.unwrap(), PathBuf::from("/etc/aws"));
        assert_eq!(shared_files(&env(&[])), (None, None));
    }

    #[tokio::test]
    async fn fetched_credentials_refresh_before_they_expire() {
        let now = Utc::now();
        let runs = Arc::new(AtomicUsize::new(0));
        let counter = runs.clone();
        let fetch: Fetch = Arc::new(move || {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                let mut c = Credentials::new(&format!("A{n}"), "S", Some("T")).unwrap();
                c.expiration = Some(now + chrono::Duration::minutes(if n == 0 { 3 } else { 60 }));
                Ok(c)
            })
        });
        let p = CredentialProvider::fetching(fetch, None);
        assert!(p.refreshable());
        // The first expires within the margin, so the next get fetches again.
        assert_eq!(p.get(now).await.unwrap().access_key_id, "A0");
        assert_eq!(p.get(now).await.unwrap().access_key_id, "A1");
        assert_eq!(p.get(now).await.unwrap().access_key_id, "A1");
        p.invalidate().await;
        assert_eq!(p.get(now).await.unwrap().access_key_id, "A2");
        assert_eq!(runs.load(Ordering::SeqCst), 3);

        let fixed = CredentialProvider::fixed(Credentials::local(None));
        fixed.invalidate().await;
        assert_eq!(fixed.get(now).await.unwrap().access_key_id, "local");
        assert!(!fixed.refreshable());
    }

    #[tokio::test]
    async fn processes_run_through_the_runner() {
        let run: RunCommand = Arc::new(|command: String| {
            Box::pin(async move {
                assert_eq!(command, "get-creds");
                Ok(r#"{"Version":1,"AccessKeyId":"P","SecretAccessKey":"S","Expiration":"2000-01-01T00:00:00Z"}"#.to_string())
            })
        });
        let p = CredentialProvider::process("get-creds".into(), run);
        match p.get(Utc::now()).await {
            Err(AppError::Auth(m)) => assert!(m.contains("already expired"), "{m}"),
            other => panic!("{other:?}"),
        }
    }
}
