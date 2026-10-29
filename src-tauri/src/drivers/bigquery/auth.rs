//! Credentials: a service-account key file (an RS256 assertion traded for an access token),
//! Application Default Credentials (`gcloud auth application-default login` writes an
//! `authorized_user` file holding a refresh token; `GOOGLE_APPLICATION_CREDENTIALS` may name a
//! service-account key instead), or a fixed token (Command mode, `gcloud auth
//! print-access-token`). Access tokens live in memory only; a new one is fetched a few minutes
//! before the old one expires, and once after the API refuses one (a 401).
//!
//! The key file is read at connect from the path in the profile's params and never stored
//! (plan A18). Read-only profiles ask a service account for the `bigquery.readonly` scope; an
//! `authorized_user` refresh keeps the scopes granted at login, so it is not narrowed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::time::Instant;

use super::jwt::{JwtSigner, token_claims};
use super::transport::{HttpRequest, Transport, body_excerpt, credentials_may_go_to};
use crate::error::AppError;

pub const SCOPE_BIGQUERY: &str = "https://www.googleapis.com/auth/bigquery";
pub const SCOPE_BIGQUERY_READONLY: &str = "https://www.googleapis.com/auth/bigquery.readonly";
pub const GOOGLE_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
const JWT_BEARER: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";
/// A credential file is a few KB; anything this big is not one.
const CREDENTIAL_FILE_LIMIT: u64 = 1 << 20;
/// A cached token is replaced this long before it expires.
const REFRESH_MARGIN: Duration = Duration::from_secs(300);
/// When the token endpoint does not say how long its token lasts.
const DEFAULT_LIFETIME: Duration = Duration::from_secs(3600);

/// The fields of a service-account key file Tabler uses.
#[derive(Clone, Deserialize)]
pub struct ServiceAccountKey {
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub private_key_id: Option<String>,
    pub private_key: String,
    pub client_email: String,
    #[serde(default)]
    pub token_uri: Option<String>,
}

impl std::fmt::Debug for ServiceAccountKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceAccountKey")
            .field("project_id", &self.project_id)
            .field("client_email", &self.client_email)
            .field("token_uri", &self.token_uri)
            .finish_non_exhaustive()
    }
}

/// The `authorized_user` file of `gcloud auth application-default login`.
#[derive(Clone, Deserialize)]
pub struct AuthorizedUser {
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
    #[serde(default)]
    pub quota_project_id: Option<String>,
    #[serde(default)]
    pub token_uri: Option<String>,
}

impl std::fmt::Debug for AuthorizedUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthorizedUser")
            .field("client_id", &self.client_id)
            .field("quota_project_id", &self.quota_project_id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub enum CredentialFile {
    ServiceAccount(ServiceAccountKey),
    AuthorizedUser(AuthorizedUser),
}

impl CredentialFile {
    /// The project the file names, when it names one (a key's project, a user's quota project).
    pub fn project(&self) -> Option<&str> {
        match self {
            CredentialFile::ServiceAccount(k) => k.project_id.as_deref(),
            CredentialFile::AuthorizedUser(u) => u.quota_project_id.as_deref(),
        }
        .filter(|p| !p.is_empty())
    }
}

/// Parses a credential file. Error messages name what is wrong, never a value of the file.
pub fn parse_credential_file(text: &str) -> Result<CredentialFile, AppError> {
    let doc: serde_json::Value = serde_json::from_str(text).map_err(|e| {
        AppError::Auth(format!(
            "the credential file is not JSON (line {}, column {})",
            e.line(),
            e.column()
        ))
    })?;
    fn fields<T: serde::de::DeserializeOwned>(doc: serde_json::Value) -> Result<T, AppError> {
        serde_json::from_value(doc).map_err(|e| {
            let said = e.to_string();
            // serde names a missing field; other messages may quote a value of the file.
            let why = if said.starts_with("missing field") {
                said
            } else {
                "a field has the wrong type".to_string()
            };
            AppError::Auth(format!("the credential file is not usable: {why}"))
        })
    }
    match doc.get("type").and_then(serde_json::Value::as_str) {
        Some("service_account") => Ok(CredentialFile::ServiceAccount(fields(doc)?)),
        Some("authorized_user") => Ok(CredentialFile::AuthorizedUser(fields(doc)?)),
        Some(
            kind @ ("external_account"
            | "external_account_authorized_user"
            | "impersonated_service_account"
            | "gdch_service_account"),
        ) => Err(AppError::Unsupported(format!(
            "BigQuery: credential files of type \u{201c}{kind}\u{201d} are not supported yet; use \
             a service-account key, `gcloud auth application-default login`, or an access token \
             command"
        ))),
        Some(other) => Err(AppError::Auth(format!(
            "the credential file has an unknown type \u{201c}{other}\u{201d}"
        ))),
        None => Err(AppError::Auth(
            "the credential file has no \u{201c}type\u{201d}: is it a Google credential file?"
                .into(),
        )),
    }
}

/// Reads and parses the credential file at `path`.
pub async fn read_credential_file(path: &Path) -> Result<CredentialFile, AppError> {
    let shown = path.display();
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|e| AppError::Auth(format!("cannot read the credential file {shown}: {e}")))?;
    if !meta.is_file() || meta.len() > CREDENTIAL_FILE_LIMIT {
        return Err(AppError::Auth(format!(
            "{shown} is not a credential file (a JSON file of a few KB)"
        )));
    }
    let text = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| AppError::Auth(format!("cannot read the credential file {shown}: {e}")))?;
    parse_credential_file(&text)
}

/// Where Application Default Credentials are: `GOOGLE_APPLICATION_CREDENTIALS`, else the file
/// `gcloud auth application-default login` writes in gcloud's configuration folder
/// (`CLOUDSDK_CONFIG`, `%APPDATA%\gcloud` on Windows, `~/.config/gcloud` elsewhere).
pub fn adc_path() -> Option<PathBuf> {
    adc_path_with(|k| std::env::var(k).ok(), cfg!(windows))
}

pub fn adc_path_with(var: impl Fn(&str) -> Option<String>, windows: bool) -> Option<PathBuf> {
    let set = |k: &str| var(k).filter(|v| !v.trim().is_empty());
    if let Some(p) = set("GOOGLE_APPLICATION_CREDENTIALS") {
        return Some(PathBuf::from(p));
    }
    const FILE: &str = "application_default_credentials.json";
    if let Some(dir) = set("CLOUDSDK_CONFIG") {
        return Some(PathBuf::from(dir).join(FILE));
    }
    if windows {
        set("APPDATA").map(|d| PathBuf::from(d).join("gcloud").join(FILE))
    } else {
        set("HOME").map(|h| PathBuf::from(h).join(".config").join("gcloud").join(FILE))
    }
}

/// A bearer token and when it stops working.
#[derive(Clone)]
pub struct AccessToken {
    pub token: String,
    pub expires_at: Option<Instant>,
}

impl std::fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessToken")
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// Where the client's bearer tokens come from.
#[async_trait]
pub trait TokenSource: Send + Sync {
    /// A token that should still work.
    async fn token(&self) -> Result<String, AppError>;
    /// Forgets the token the API just refused; whether the next `token` can be a new one (a
    /// fixed token cannot).
    fn invalidate(&self) -> bool;
}

/// A token given as it is (Command mode reuses it for the descriptor's TTL).
pub struct FixedToken(String);

impl FixedToken {
    pub fn new(token: impl Into<String>) -> FixedToken {
        FixedToken(token.into())
    }
}

#[async_trait]
impl TokenSource for FixedToken {
    async fn token(&self) -> Result<String, AppError> {
        if self.0.trim().is_empty() {
            return Err(AppError::Auth("the access token is empty".into()));
        }
        Ok(self.0.trim().to_string())
    }

    fn invalidate(&self) -> bool {
        false
    }
}

enum Grant {
    ServiceAccount {
        signer: JwtSigner,
        client_email: String,
        token_uri: String,
        scope: String,
    },
    AuthorizedUser {
        user: AuthorizedUser,
        token_uri: String,
    },
}

/// Tokens from Google's OAuth endpoint, cached until shortly before they expire.
pub struct OAuthTokens {
    transport: Arc<dyn Transport>,
    grant: Grant,
    cache: tokio::sync::Mutex<Option<AccessToken>>,
    /// Set by `invalidate`, read under the cache's lock.
    stale: AtomicBool,
}

impl OAuthTokens {
    /// For a credential file: a service account asks for `scope`, a user's refresh token keeps
    /// the scopes of its login.
    pub fn from_file(
        transport: Arc<dyn Transport>,
        file: &CredentialFile,
        scope: &str,
    ) -> Result<OAuthTokens, AppError> {
        let grant = match file {
            CredentialFile::ServiceAccount(key) => {
                let token_uri = token_uri(key.token_uri.as_deref())?;
                Grant::ServiceAccount {
                    signer: JwtSigner::from_pem(&key.private_key, key.private_key_id.as_deref())?,
                    client_email: key.client_email.clone(),
                    token_uri,
                    scope: scope.to_string(),
                }
            }
            CredentialFile::AuthorizedUser(user) => Grant::AuthorizedUser {
                token_uri: token_uri(user.token_uri.as_deref())?,
                user: user.clone(),
            },
        };
        Ok(OAuthTokens {
            transport,
            grant,
            cache: tokio::sync::Mutex::new(None),
            stale: AtomicBool::new(false),
        })
    }

    /// The token request: a signed assertion, or the refresh token.
    fn request(&self) -> Result<HttpRequest, AppError> {
        Ok(match &self.grant {
            Grant::ServiceAccount {
                signer,
                client_email,
                token_uri,
                scope,
            } => {
                let now = chrono::Utc::now().timestamp();
                let assertion = signer.sign(&token_claims(client_email, scope, token_uri, now))?;
                HttpRequest::post_form(
                    token_uri.clone(),
                    &[("grant_type", JWT_BEARER), ("assertion", &assertion)],
                )
            }
            Grant::AuthorizedUser { user, token_uri } => HttpRequest::post_form(
                token_uri.clone(),
                &[
                    ("grant_type", "refresh_token"),
                    ("client_id", &user.client_id),
                    ("client_secret", &user.client_secret),
                    ("refresh_token", &user.refresh_token),
                ],
            ),
        })
    }

    async fn fetch(&self) -> Result<AccessToken, AppError> {
        let reply = self.transport.send(self.request()?).await?;
        parse_token_reply(reply.status, &reply.body, reply.content_type.as_deref())
    }
}

#[async_trait]
impl TokenSource for OAuthTokens {
    async fn token(&self) -> Result<String, AppError> {
        let mut cache = self.cache.lock().await;
        if self.stale.swap(false, Ordering::SeqCst) {
            *cache = None;
        }
        if let Some(t) = cache.as_ref()
            && t.expires_at
                .is_none_or(|at| Instant::now() + REFRESH_MARGIN < at)
        {
            return Ok(t.token.clone());
        }
        let fresh = self.fetch().await?;
        let token = fresh.token.clone();
        *cache = Some(fresh);
        Ok(token)
    }

    fn invalidate(&self) -> bool {
        self.stale.store(true, Ordering::SeqCst);
        true
    }
}

/// The token endpoint of a credential file: Google's by default. Another one must be HTTPS
/// (plain HTTP only to this machine, for tests and emulators), since the signed assertion or
/// the refresh token and client secret go to it.
fn token_uri(uri: Option<&str>) -> Result<String, AppError> {
    let uri = uri
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .unwrap_or(GOOGLE_TOKEN_URI);
    if credentials_may_go_to(uri) {
        Ok(uri.to_string())
    } else {
        Err(AppError::Auth(
            "the credential file's token_uri must be an https URL (plain http only to this \
             machine)"
                .into(),
        ))
    }
}

#[derive(Deserialize)]
struct TokenReply {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    expires_in: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

/// The token endpoint's answer: the token and its lifetime, or why it refused.
pub fn parse_token_reply(
    status: u16,
    body: &[u8],
    content_type: Option<&str>,
) -> Result<AccessToken, AppError> {
    let reply: Option<TokenReply> = serde_json::from_slice(body).ok();
    let ok = (200..300).contains(&status);
    match reply {
        Some(TokenReply {
            access_token: Some(token),
            expires_in,
            ..
        }) if ok && !token.is_empty() => {
            let secs = expires_in.as_ref().and_then(|v| match v {
                serde_json::Value::Number(n) => n.as_u64(),
                serde_json::Value::String(s) => s.trim().parse().ok(),
                _ => None,
            });
            let lifetime = secs.map_or(DEFAULT_LIFETIME, Duration::from_secs);
            Ok(AccessToken {
                token,
                expires_at: Instant::now().checked_add(lifetime),
            })
        }
        Some(TokenReply {
            error: Some(error),
            error_description,
            ..
        }) if !ok && status < 500 => {
            let said = error_description
                .filter(|d| !d.is_empty())
                .map(|d| format!(": {d}"))
                .unwrap_or_default();
            Err(AppError::Auth(format!(
                "Google refused the login ({error}{said})"
            )))
        }
        _ if status >= 500 => Err(AppError::Connect(format!(
            "Google's token endpoint is unavailable (HTTP {status})"
        ))),
        // An error page that is no JSON object (a proxy's) is quoted. A JSON body or a success
        // is not: the token endpoint's replies may hold tokens.
        None if !ok => Err(AppError::Auth(format!(
            "Google refused the login (HTTP {status}): {}",
            body_excerpt(body, content_type)
        ))),
        _ if !ok => Err(AppError::Auth(format!(
            "Google refused the login (HTTP {status})"
        ))),
        _ => Err(AppError::Auth(format!(
            "Google's token endpoint answered without an access token (HTTP {status})"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::bigquery::testing::{FakeBigquery, sa_key_json, test_key, user_json};
    use crate::drivers::bigquery::transport::HttpResponse;

    #[test]
    fn parses_the_credential_files_and_names_what_is_wrong() {
        let sa =
            parse_credential_file(&sa_key_json("https://oauth2.googleapis.com/token")).unwrap();
        match &sa {
            CredentialFile::ServiceAccount(k) => {
                assert_eq!(k.client_email, "tabler@demo-proj.iam.gserviceaccount.com");
                assert_eq!(k.private_key_id.as_deref(), Some("k1"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(sa.project(), Some("demo-proj"));
        // The key never shows in debug output.
        assert!(!format!("{sa:?}").contains("PRIVATE KEY"));
        let user =
            parse_credential_file(&user_json("https://oauth2.googleapis.com/token")).unwrap();
        assert!(matches!(user, CredentialFile::AuthorizedUser(_)));
        assert_eq!(user.project(), Some("quota-proj"));
        assert!(!format!("{user:?}").contains("refresh-secret"));

        let e = parse_credential_file("{not json").unwrap_err();
        assert!(matches!(e, AppError::Auth(_)), "{e:?}");
        let e =
            parse_credential_file(r#"{"type":"service_account","private_key":"x"}"#).unwrap_err();
        assert!(
            e.to_string().contains("missing field `client_email`"),
            "{e}"
        );
        let e = parse_credential_file(
            r#"{"type":"service_account","private_key":7,"client_email":"secret-value"}"#,
        )
        .unwrap_err();
        assert!(!e.to_string().contains("secret-value"), "{e}");
        let e = parse_credential_file(r#"{"type":"external_account"}"#).unwrap_err();
        assert!(matches!(e, AppError::Unsupported(_)), "{e:?}");
        let e = parse_credential_file(r#"{"client_id":"x"}"#).unwrap_err();
        assert!(e.to_string().contains("no \u{201c}type\u{201d}"), "{e}");
    }

    #[tokio::test]
    async fn reads_credential_files_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.json");
        std::fs::write(&path, sa_key_json(GOOGLE_TOKEN_URI)).unwrap();
        assert!(matches!(
            read_credential_file(&path).await.unwrap(),
            CredentialFile::ServiceAccount(_)
        ));
        let missing = read_credential_file(&dir.path().join("none.json"))
            .await
            .unwrap_err();
        assert!(matches!(missing, AppError::Auth(_)), "{missing:?}");
        let folder = read_credential_file(dir.path()).await.unwrap_err();
        assert!(
            folder.to_string().contains("not a credential file"),
            "{folder}"
        );
    }

    #[test]
    fn finds_application_default_credentials() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(
            adc_path_with(
                env(&[
                    ("GOOGLE_APPLICATION_CREDENTIALS", "C:\\k.json"),
                    ("APPDATA", "A")
                ]),
                true
            ),
            Some(PathBuf::from("C:\\k.json"))
        );
        assert_eq!(
            adc_path_with(env(&[("APPDATA", "C:\\Users\\u\\AppData\\Roaming")]), true),
            Some(
                PathBuf::from("C:\\Users\\u\\AppData\\Roaming")
                    .join("gcloud")
                    .join("application_default_credentials.json")
            )
        );
        assert_eq!(
            adc_path_with(env(&[("HOME", "/home/u")]), false),
            Some(PathBuf::from(
                "/home/u/.config/gcloud/application_default_credentials.json"
            ))
        );
        assert_eq!(
            adc_path_with(env(&[("CLOUDSDK_CONFIG", "/cfg"), ("HOME", "/h")]), false),
            Some(PathBuf::from("/cfg/application_default_credentials.json"))
        );
        assert_eq!(
            adc_path_with(env(&[("GOOGLE_APPLICATION_CREDENTIALS", " ")]), false),
            None
        );
    }

    #[test]
    fn token_replies_and_refusals() {
        let ok = parse_token_reply(
            200,
            br#"{"access_token":"ya29.a","expires_in":3599,"token_type":"Bearer"}"#,
            None,
        )
        .unwrap();
        assert_eq!(ok.token, "ya29.a");
        assert!(ok.expires_at.is_some());
        assert!(!format!("{ok:?}").contains("ya29"));
        let refused = parse_token_reply(
            400,
            br#"{"error":"invalid_grant","error_description":"Invalid JWT Signature."}"#,
            None,
        )
        .unwrap_err();
        assert!(matches!(&refused, AppError::Auth(m) if m.contains("Invalid JWT Signature")));
        assert!(matches!(
            parse_token_reply(503, b"<html>down</html>", Some("text/html")).unwrap_err(),
            AppError::Connect(_)
        ));
        assert!(matches!(
            parse_token_reply(200, b"{}", None).unwrap_err(),
            AppError::Auth(_)
        ));
        assert!(matches!(
            parse_token_reply(403, b"nope", None).unwrap_err(),
            AppError::Auth(m) if m.contains("nope")
        ));
        // A reply that may hold a token is never quoted.
        for (status, body) in [
            (200, r#"{"access_token":"","refresh_token":"1//secret"}"#),
            (200, "ya29.secret"),
            (200, "access_token=ya29.secret&expires_in=3599"),
            (401, r#"{"access_token":"ya29.secret"}"#),
            (400, r#"{"detail":"secret"}"#),
        ] {
            let e = parse_token_reply(status, body.as_bytes(), None).unwrap_err();
            assert!(
                matches!(&e, AppError::Auth(m) if !m.contains("secret")),
                "{e}"
            );
        }
    }

    #[test]
    fn token_endpoints_must_be_https() {
        assert_eq!(token_uri(None).unwrap(), GOOGLE_TOKEN_URI);
        assert!(token_uri(Some("http://127.0.0.1:9/token")).is_ok());
        assert!(token_uri(Some("http://localhost:9/token")).is_ok());
        assert!(token_uri(Some("https://oauth2.example/token")).is_ok());
        for bad in [
            "http://evil.example/token",
            "http://localhost.example.com/token",
            "http://127.0.0.1.nip.io/token",
            "http://127.0.0.1:80@evil.example/token",
            "file:///etc/passwd",
        ] {
            let e = token_uri(Some(bad)).unwrap_err();
            // The file's values stay out of the message.
            assert!(
                matches!(&e, AppError::Auth(m) if !m.contains("example") && !m.contains("nip")),
                "{bad}: {e}"
            );
        }
    }

    #[tokio::test]
    async fn a_service_account_signs_once_and_reuses_its_token() {
        let fake = FakeBigquery::new();
        fake.on(
            "POST",
            "/token",
            HttpResponse::json(
                200,
                &serde_json::json!({"access_token": "tok-1", "expires_in": 3600}),
            ),
        );
        let file = parse_credential_file(&sa_key_json("https://oauth2.example/token")).unwrap();
        let tokens =
            OAuthTokens::from_file(fake.transport(), &file, SCOPE_BIGQUERY_READONLY).unwrap();
        assert_eq!(tokens.token().await.unwrap(), "tok-1");
        assert_eq!(tokens.token().await.unwrap(), "tok-1");
        let sent = fake.requests();
        assert_eq!(sent.len(), 1, "the token is cached");
        let form = sent[0].form();
        assert_eq!(form[0], ("grant_type".into(), JWT_BEARER.into()));
        assert_eq!(form[1].0, "assertion");
        // The assertion asks for the read-only scope at the file's token endpoint.
        let claims = form[1].1.split('.').nth(1).unwrap();
        use base64::Engine as _;
        let claims: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(claims)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(claims["scope"], SCOPE_BIGQUERY_READONLY);
        assert_eq!(claims["aud"], "https://oauth2.example/token");
        // After a refusal the next call fetches a new one.
        assert!(tokens.invalidate());
        assert_eq!(tokens.token().await.unwrap(), "tok-1");
        assert_eq!(fake.requests().len(), 2);
        let _ = test_key();
    }

    #[tokio::test(start_paused = true)]
    async fn tokens_are_replaced_before_they_expire() {
        let fake = FakeBigquery::new();
        fake.on_seq(
            "POST",
            "/token",
            vec![
                HttpResponse::json(
                    200,
                    &serde_json::json!({"access_token": "a", "expires_in": 600}),
                ),
                HttpResponse::json(
                    200,
                    &serde_json::json!({"access_token": "b", "expires_in": 600}),
                ),
            ],
        );
        let file = parse_credential_file(&user_json("https://oauth2.example/token")).unwrap();
        let tokens = OAuthTokens::from_file(fake.transport(), &file, SCOPE_BIGQUERY).unwrap();
        assert_eq!(tokens.token().await.unwrap(), "a");
        tokio::time::advance(Duration::from_secs(200)).await;
        assert_eq!(tokens.token().await.unwrap(), "a");
        // Within five minutes of the end: a new one.
        tokio::time::advance(Duration::from_secs(150)).await;
        assert_eq!(tokens.token().await.unwrap(), "b");
        let form = fake.requests()[0].form();
        assert_eq!(form[0], ("grant_type".into(), "refresh_token".into()));
        assert!(form.contains(&("refresh_token".into(), "refresh-secret".into())));
    }

    #[tokio::test]
    async fn fixed_tokens_cannot_be_refreshed() {
        let t = FixedToken::new(" ya29.x \n");
        assert_eq!(t.token().await.unwrap(), "ya29.x");
        assert!(!t.invalidate());
        assert!(FixedToken::new("  ").token().await.is_err());
    }
}
