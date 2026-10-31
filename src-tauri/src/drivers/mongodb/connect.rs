//! A profile as the crate's `ClientOptions`: the hosts (the SSH tunnel's address when there is
//! one), TLS, the sign-in and the timeouts, and the [`Login`] a session keeps to sign in again
//! with a fresh Command Line password.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ::mongodb::options::oidc::{Callback, CallbackContext, IdpServerResponse};
use ::mongodb::options::{
    AuthMechanism, ClientOptions, Credential, ReadPreference, SelectionCriteria, ServerAddress,
    Tls, TlsOptions,
};
use futures::FutureExt as _;
use futures::future::BoxFuture;

use super::errors;
use super::params::{self, MongoAuth, MongoParams, MongoReadPreference, UriParts};
use crate::drivers::ConnectTarget;
use crate::error::AppError;
use crate::secrets::{self, CommandPassword};
use crate::types::driver::ExtraSecret;
use crate::types::profile::{ConnectionProfile, DEFAULT_CONNECT_TIMEOUT_SECS, SslMode};

/// What a session needs to sign in again: the options it opened with and, in Command Line mode,
/// the command that prints the password, run again when the server refuses the one it has (a
/// rotated secret). OIDC tokens are renewed by the crate itself, through the callback.
pub struct Login {
    pub options: ClientOptions,
    pub command: Option<Arc<CommandPassword>>,
}

impl Login {
    /// Whether a refused sign-in is worth a fresh password: Command Line mode, with a mechanism
    /// that sends the password (SCRAM, PLAIN).
    pub fn renewable(&self) -> bool {
        self.command.is_some()
            && self.options.credential.as_ref().is_some_and(|c| {
                c.password.is_some() && c.mechanism != Some(AuthMechanism::MongoDbOidc)
            })
    }

    /// The options with the command's current output as the password.
    pub async fn renewed(&self) -> Result<ClientOptions, AppError> {
        let Some(command) = &self.command else {
            return Ok(self.options.clone());
        };
        command.invalidate().await;
        let password = command.password().await?;
        let mut options = self.options.clone();
        if let Some(c) = options.credential.as_mut() {
            c.password = Some(password);
        }
        Ok(options)
    }
}

/// The client options of `target`'s profile. `command` is the Command Line password's command
/// (seeded with `target.password`, which it printed just now).
pub async fn client_options(
    target: &ConnectTarget<'_>,
    params: &MongoParams,
    command: Option<Arc<CommandPassword>>,
) -> Result<ClientOptions, AppError> {
    let profile = target.profile;
    let uri = params.uri().map(UriParts::parse).transpose()?;
    let mut options = match params.uri() {
        Some(text) => ClientOptions::parse(text)
            .await
            .map_err(|e| errors::from_driver(&e))?,
        None => {
            let mut o = ClientOptions::default();
            o.hosts = vec![address(&target.host(), target.port())];
            o
        }
    };
    let tunnelled = target.host_override.is_some();
    if tunnelled {
        // The tunnel reaches one node (`params::check` refused a seed list or SRV).
        options.hosts = vec![address(&target.host(), target.port())];
        options.direct_connection = Some(true);
    } else if let Some(direct) = params.direct_connection {
        options.direct_connection = Some(direct);
    }
    if let Some(name) = params.replica_set() {
        options.repl_set_name = Some(name.to_string());
    }
    // One host without a replica set name: that host, as mongosh connects (a member reached
    // from outside often advertises names only its own network resolves).
    if options.direct_connection.is_none()
        && options.hosts.len() == 1
        && options.repl_set_name.is_none()
        && !uri.as_ref().is_some_and(|u| u.srv)
        && options.load_balanced != Some(true)
    {
        options.direct_connection = Some(true);
    }
    options.app_name = Some(params.app_name().to_string());
    let connect = Duration::from_secs(
        profile
            .options
            .connect_timeout_secs
            .filter(|s| *s > 0)
            .map_or(DEFAULT_CONNECT_TIMEOUT_SECS, u64::from),
    );
    options.connect_timeout = Some(connect);
    options.server_selection_timeout = Some(
        params
            .server_selection_timeout_secs
            .map_or(connect, |s| Duration::from_secs(s.into())),
    );
    // Run as written: the commands Tabler sends are its own retry decisions.
    options.retry_writes = Some(false);
    options.tls = tls(profile, options.tls.take(), tunnelled)?;
    options.credential = credential(
        profile,
        params,
        options.credential.take(),
        target.password,
        command,
    )?;
    if params.auth_mechanism == MongoAuth::X509
        && !matches!(&options.tls, Some(Tls::Enabled(t)) if t.cert_key_file_path.is_some())
    {
        return Err(AppError::InvalidInput(
            "MongoDB X.509 sign-in needs TLS with a client certificate".into(),
        ));
    }
    Ok(options)
}

/// `host:port`; a blank host is this machine, as mongosh connects without one.
fn address(host: &str, port: u16) -> ServerAddress {
    let host = match host.trim() {
        "" => "127.0.0.1",
        h => h,
    };
    ServerAddress::Tcp {
        host: host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string(),
        port: (port != 0).then_some(port),
    }
}

/// The read preference of the profile's reads (the catalog, the grid, the console's reads): the
/// profile's, else the connection string's (`from_uri`, taken out of the client's options, whose
/// default would reach transactions too, which read on the primary only).
pub fn read_preference(
    params: &MongoParams,
    from_uri: Option<SelectionCriteria>,
) -> SelectionCriteria {
    SelectionCriteria::ReadPreference(match params.read_preference {
        MongoReadPreference::Primary => return from_uri.unwrap_or(ReadPreference::Primary.into()),
        MongoReadPreference::PrimaryPreferred => ReadPreference::PrimaryPreferred { options: None },
        MongoReadPreference::Secondary => ReadPreference::Secondary { options: None },
        MongoReadPreference::SecondaryPreferred => {
            ReadPreference::SecondaryPreferred { options: None }
        }
        MongoReadPreference::Nearest => ReadPreference::Nearest { options: None },
    })
}

/// TLS by the profile's SSL mode: off, on without checks (`require`), or on with the chain and the
/// host name checked (`verify-ca` and `verify-full` alike: rustls always checks the name).
/// `prefer` leaves it to the connection string (off without one). Off with a connection string
/// that asks for TLS is refused (as `params::check` does, here as the crate read the string).
/// The CA file, the client certificate with its key, and the key's password (the profile's
/// second secret) go with it, never one the connection string carries (refused, as
/// `params::check` does, here as the crate read the string).
fn tls(
    profile: &ConnectionProfile,
    from_uri: Option<Tls>,
    tunnelled: bool,
) -> Result<Option<Tls>, AppError> {
    if matches!(&from_uri, Some(Tls::Enabled(t)) if t.tls_certificate_key_file_password.is_some()) {
        return Err(params::key_password_in_url());
    }
    let mut options = match (profile.ssl, from_uri) {
        (SslMode::Disable, Some(Tls::Enabled(_))) => return Err(params::tls_turned_off()),
        (SslMode::Disable, _) | (SslMode::Prefer, None | Some(Tls::Disabled)) => {
            return Ok(Some(Tls::Disabled));
        }
        (SslMode::Prefer, Some(Tls::Enabled(t))) => t,
        (SslMode::Require, uri) => {
            let mut t = enabled(uri);
            t.allow_invalid_certificates = Some(true);
            t
        }
        (SslMode::VerifyCa | SslMode::VerifyFull, uri) => {
            let mut t = enabled(uri);
            t.allow_invalid_certificates = Some(false);
            t
        }
    };
    if tunnelled && options.allow_invalid_certificates != Some(true) {
        return Err(AppError::Unsupported(
            "MongoDB over SSH: a verified TLS certificate cannot be checked through the tunnel \
             (the driver checks it against the tunnel's local address). Use SSL mode \
             \u{201c}On\u{201d} without verification, or connect without SSH"
                .into(),
        ));
    }
    if let Some(ca) = non_blank(&profile.ssl_ca_path) {
        options.ca_file_path = Some(PathBuf::from(ca));
    }
    if let Some(file) = client_certificate(profile)? {
        options.cert_key_file_path = Some(file);
    }
    if let Some(password) = secrets::extra_secret(ExtraSecret::KeyPassphrase) {
        options.tls_certificate_key_file_password = Some(password.into_bytes());
    }
    Ok(Some(Tls::Enabled(options)))
}

fn enabled(from_uri: Option<Tls>) -> TlsOptions {
    match from_uri {
        Some(Tls::Enabled(t)) => t,
        _ => TlsOptions::default(),
    }
}

fn non_blank(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// The client certificate file: MongoDB reads the certificate and its key from one PEM file
/// (`tlsCertificateKeyFile`), so the certificate and key fields must name the same file, or only
/// one of them be set.
fn client_certificate(profile: &ConnectionProfile) -> Result<Option<PathBuf>, AppError> {
    let cert = non_blank(&profile.options.ssl_cert_path);
    let key = non_blank(&profile.options.ssl_key_path);
    match (cert, key) {
        (Some(c), Some(k)) if c != k => Err(AppError::InvalidInput(
            "MongoDB reads the client certificate and its key from one PEM file: choose the \
             same file for both, or put the key in the certificate's file"
                .into(),
        )),
        (Some(file), _) | (None, Some(file)) => Ok(Some(PathBuf::from(file))),
        (None, None) => Ok(None),
    }
}

/// The sign-in: the profile's user (else the connection string's) and password, the mechanism and
/// its database. No user and a SCRAM mechanism: no sign-in at all.
fn credential(
    profile: &ConnectionProfile,
    params: &MongoParams,
    from_uri: Option<Credential>,
    password: Option<&str>,
    command: Option<Arc<CommandPassword>>,
) -> Result<Option<Credential>, AppError> {
    let mut c = from_uri.unwrap_or_default();
    if let Some(user) = non_blank(&profile.user) {
        c.username = Some(user.to_string());
    }
    c.mechanism = match params.auth_mechanism {
        MongoAuth::Default => c.mechanism.take(),
        MongoAuth::ScramSha1 => Some(AuthMechanism::ScramSha1),
        MongoAuth::ScramSha256 => Some(AuthMechanism::ScramSha256),
        MongoAuth::X509 => Some(AuthMechanism::MongoDbX509),
        MongoAuth::Plain => Some(AuthMechanism::Plain),
        MongoAuth::Oidc => Some(AuthMechanism::MongoDbOidc),
    };
    let external = matches!(
        c.mechanism,
        Some(AuthMechanism::MongoDbX509 | AuthMechanism::Plain | AuthMechanism::MongoDbOidc)
    );
    if let Some(source) = params.auth_source() {
        c.source = Some(source.to_string());
    } else if c.source.is_none() {
        c.source = Some(if external { "$external" } else { "admin" }.to_string());
    }
    match c.mechanism {
        Some(AuthMechanism::MongoDbX509) => {
            // The certificate's subject is the user; a password is never sent.
            c.password = None;
        }
        Some(AuthMechanism::MongoDbOidc) => {
            let token = password
                .map(str::to_string)
                .filter(|t| !t.trim().is_empty());
            if token.is_none() && command.is_none() {
                return Err(AppError::InvalidInput(
                    "MongoDB OIDC sign-in needs an access token: put it in the password field \
                     (Command Line mode can print a fresh one)"
                        .into(),
                ));
            }
            c.password = None;
            c.oidc_callback = oidc_callback(TokenFeed::new(token, command));
        }
        _ => {
            if c.username.is_none() {
                // Nobody to sign in as (a server without access control).
                return Ok(None);
            }
            if let Some(p) = password {
                c.password = Some(p.to_string());
            }
        }
    }
    Ok(Some(c))
}

/// Where the OIDC callback takes its tokens: the password field's token, or the Command Line
/// password's command. The crate keeps a token until the server refuses it and calls back only
/// then, so every call after the first one runs the command again.
#[derive(Clone)]
pub struct TokenFeed {
    token: Option<String>,
    command: Option<Arc<CommandPassword>>,
    called: Arc<AtomicBool>,
}

impl TokenFeed {
    pub fn new(token: Option<String>, command: Option<Arc<CommandPassword>>) -> TokenFeed {
        TokenFeed {
            token,
            command,
            called: Arc::default(),
        }
    }

    pub async fn next(&self) -> Result<String, AppError> {
        let again = self.called.swap(true, Ordering::SeqCst);
        match (&self.command, &self.token) {
            (Some(command), _) => {
                if again {
                    command.invalidate().await;
                }
                command.password().await
            }
            (None, Some(token)) => Ok(token.clone()),
            (None, None) => Err(AppError::Auth("no OIDC access token".into())),
        }
    }
}

fn oidc_callback(feed: TokenFeed) -> Callback {
    Callback::machine(
        move |_: CallbackContext| -> BoxFuture<'static, ::mongodb::error::Result<IdpServerResponse>> {
            let feed = feed.clone();
            async move {
                let token = feed.next().await.map_err(::mongodb::error::Error::custom)?;
                Ok(IdpServerResponse::builder().access_token(token).build())
            }
            .boxed()
        },
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::secrets::ExtraSecrets;

    fn profile(extra: serde_json::Value) -> ConnectionProfile {
        super::super::params::tests::profile(extra)
    }

    async fn options_of(
        profile: &ConnectionProfile,
        password: Option<&str>,
        tunnel: Option<(&str, u16)>,
    ) -> Result<ClientOptions, AppError> {
        let params = MongoParams::of_profile(profile)?;
        let target = ConnectTarget {
            profile,
            password,
            host_override: tunnel.map(|(h, p)| (h.to_string(), p)),
        };
        client_options(&target, &params, None).await
    }

    fn tls_of(o: &ClientOptions) -> Option<&TlsOptions> {
        match &o.tls {
            Some(Tls::Enabled(t)) => Some(t),
            _ => None,
        }
    }

    #[tokio::test]
    async fn a_form_profile_connects_to_its_host_directly() {
        let p = profile(json!({ "port": 27018, "user": "ann", "database": "shop",
            "options": { "connectTimeoutSecs": 7 } }));
        let o = options_of(&p, Some("pw"), None).await.unwrap();
        assert_eq!(
            o.hosts,
            [ServerAddress::Tcp {
                host: "db.example.test".into(),
                port: Some(27018)
            }]
        );
        assert_eq!(o.direct_connection, Some(true));
        assert_eq!(o.app_name.as_deref(), Some("Tabler"));
        assert_eq!(o.connect_timeout, Some(Duration::from_secs(7)));
        assert_eq!(o.server_selection_timeout, Some(Duration::from_secs(7)));
        assert_eq!(o.retry_writes, Some(false));
        assert_eq!(o.tls, Some(Tls::Disabled));
        let c = o.credential.unwrap();
        assert_eq!(c.username.as_deref(), Some("ann"));
        assert_eq!(c.password.as_deref(), Some("pw"));
        assert_eq!(c.source.as_deref(), Some("admin"));
        assert_eq!(c.mechanism, None);
        // No user: no sign-in.
        let o = options_of(&profile(json!({})), None, None).await.unwrap();
        assert!(o.credential.is_none());
        // No host: this machine, on the profile's port or the server's default.
        for (host, port, want) in [
            (json!(null), json!(null), Some(27017)),
            (json!("  "), json!(27019), Some(27019)),
        ] {
            let p = profile(json!({ "host": host, "port": port }));
            let o = options_of(&p, None, None).await.unwrap();
            assert_eq!(
                o.hosts,
                [ServerAddress::Tcp {
                    host: "127.0.0.1".into(),
                    port: want
                }]
            );
        }
        // A replica set name: discovery, not one node.
        let p = profile(json!({ "options": { "driverParams": { "replicaSet": "rs0",
            "serverSelectionTimeoutSecs": 4, "appName": "ops" } } }));
        let o = options_of(&p, None, None).await.unwrap();
        assert_eq!(
            (o.direct_connection, o.repl_set_name.as_deref()),
            (None, Some("rs0"))
        );
        assert_eq!(o.server_selection_timeout, Some(Duration::from_secs(4)));
        assert_eq!(o.app_name.as_deref(), Some("ops"));
    }

    #[tokio::test]
    async fn a_connection_string_gives_hosts_and_options_and_the_profile_signs_in() {
        let p = profile(
            json!({ "host": null, "user": "bob", "options": { "driverParams": {
            "uri": "mongodb://alice@a.example.net:27017,b.example.net:27018/shop?replicaSet=rs0&authSource=users&readPreference=secondary"
        } } }),
        );
        let o = options_of(&p, Some("s3cret"), None).await.unwrap();
        assert_eq!(o.hosts.len(), 2);
        assert_eq!(o.repl_set_name.as_deref(), Some("rs0"));
        assert_eq!(o.direct_connection, None);
        assert_eq!(o.default_database.as_deref(), Some("shop"));
        let c = o.credential.unwrap();
        // The profile's user wins over the string's; the string's authSource stays.
        assert_eq!(c.username.as_deref(), Some("bob"));
        assert_eq!(c.source.as_deref(), Some("users"));
        assert_eq!(c.password.as_deref(), Some("s3cret"));
        // The string's read preference serves the reads unless the profile names one.
        let params = MongoParams::of_profile(&p).unwrap();
        let reads = read_preference(&params, o.selection_criteria.clone());
        assert!(matches!(
            reads,
            SelectionCriteria::ReadPreference(ReadPreference::Secondary { .. })
        ));
        let nearest = MongoParams {
            read_preference: MongoReadPreference::Nearest,
            ..params
        };
        assert!(matches!(
            read_preference(&nearest, o.selection_criteria),
            SelectionCriteria::ReadPreference(ReadPreference::Nearest { .. })
        ));
        assert!(matches!(
            read_preference(&MongoParams::default(), None),
            SelectionCriteria::ReadPreference(ReadPreference::Primary)
        ));
        // SRV over no tunnel is left to the crate (it resolves the record at connect); its TLS
        // default is on under `prefer`.
    }

    #[tokio::test]
    async fn ssl_modes_map_to_tls_options() {
        let o = options_of(&profile(json!({ "ssl": "prefer" })), None, None)
            .await
            .unwrap();
        assert_eq!(o.tls, Some(Tls::Disabled));
        let p = profile(json!({ "ssl": "require", "sslCaPath": "C:/ca.pem",
            "options": { "sslCertPath": "C:/client.pem", "sslKeyPath": "C:/client.pem" } }));
        let o = options_of(&p, None, None).await.unwrap();
        let t = tls_of(&o).unwrap();
        assert_eq!(t.allow_invalid_certificates, Some(true));
        assert_eq!(t.ca_file_path, Some(PathBuf::from("C:/ca.pem")));
        assert_eq!(t.cert_key_file_path, Some(PathBuf::from("C:/client.pem")));
        assert_eq!(t.tls_certificate_key_file_password, None);
        for mode in ["verify-ca", "verify-full"] {
            let o = options_of(&profile(json!({ "ssl": mode })), None, None)
                .await
                .unwrap();
            assert_eq!(tls_of(&o).unwrap().allow_invalid_certificates, Some(false));
        }
        // `prefer` keeps the string's TLS.
        let p = profile(json!({ "ssl": "prefer", "options": { "sslKeyPath": "k.pem",
            "driverParams": { "uri": "mongodb://h/?tls=true" } } }));
        let o = options_of(&p, None, None).await.unwrap();
        assert_eq!(
            tls_of(&o).unwrap().cert_key_file_path,
            Some(PathBuf::from("k.pem"))
        );
        // Off, the form's first value, never drops the TLS a URL asks for (an LDAP password
        // would go in plain text): refused before connecting and as the crate read the URL.
        let p = profile(json!({ "user": "alice", "options": { "driverParams": {
            "uri": "mongodb://ldap.example.net/?tls=true", "authMechanism": "plain" } } }));
        let err = options_of(&p, Some("pw"), None).await.unwrap_err();
        assert_eq!(err.to_string(), params::tls_turned_off().to_string());
        let off = profile(json!({}));
        let err = tls(&off, Some(Tls::Enabled(TlsOptions::default())), false).unwrap_err();
        assert_eq!(err.to_string(), params::tls_turned_off().to_string());
        assert_eq!(
            tls(&off, Some(Tls::Disabled), false).unwrap(),
            Some(Tls::Disabled)
        );
        // Two different files cannot be one tlsCertificateKeyFile.
        let p = profile(json!({ "ssl": "require",
            "options": { "sslCertPath": "c.pem", "sslKeyPath": "k.pem" } }));
        let err = options_of(&p, None, None).await.unwrap_err();
        assert!(err.to_string().contains("from one PEM file"), "{err}");
        // The client key's password is the profile's second secret.
        let mut extras = ExtraSecrets::default();
        extras.set(ExtraSecret::KeyPassphrase, "kp".into());
        let p = profile(json!({ "ssl": "require", "options": { "sslCertPath": "c.pem" } }));
        let o = extras.scope(options_of(&p, None, None)).await.unwrap();
        assert_eq!(
            tls_of(&o).unwrap().tls_certificate_key_file_password,
            Some(b"kp".to_vec())
        );
        // Never the URL's own key password, in any spelling the crate reads (U+212A is a `k`
        // to it), even past `params::check`.
        let keyed = MongoParams {
            uri: Some(
                "mongodb://h.example.net/?tls=true&tlsCertificate\u{212A}eyFilePassword=hunter2"
                    .into(),
            ),
            ..MongoParams::default()
        };
        for ssl in ["prefer", "require", "verify-full"] {
            let p = profile(json!({ "ssl": ssl }));
            let target = ConnectTarget {
                profile: &p,
                password: None,
                host_override: None,
            };
            let err = client_options(&target, &keyed, None).await.unwrap_err();
            assert_eq!(err.to_string(), params::key_password_in_url().to_string());
        }
    }

    #[tokio::test]
    async fn ssh_pins_one_node_and_refuses_a_verified_certificate() {
        let ssh = json!({ "host": "bastion", "port": 22, "user": "u", "auth": "agent",
            "keyPath": null, "localPort": null, "keepaliveSecs": null });
        let p =
            profile(json!({ "ssh": ssh, "options": { "driverParams": { "replicaSet": "rs0" } } }));
        let o = options_of(&p, None, Some(("127.0.0.1", 40001)))
            .await
            .unwrap();
        assert_eq!(
            o.hosts,
            [ServerAddress::Tcp {
                host: "127.0.0.1".into(),
                port: Some(40001)
            }]
        );
        // One node, still checked to be a member of the named set.
        assert_eq!(o.direct_connection, Some(true));
        assert_eq!(o.repl_set_name.as_deref(), Some("rs0"));
        let p = profile(json!({ "ssh": ssh, "ssl": "require" }));
        let o = options_of(&p, None, Some(("127.0.0.1", 40001)))
            .await
            .unwrap();
        assert_eq!(tls_of(&o).unwrap().allow_invalid_certificates, Some(true));
        let p = profile(json!({ "ssh": ssh, "ssl": "verify-full" }));
        let err = options_of(&p, None, Some(("127.0.0.1", 40001)))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("through the tunnel"), "{err}");
    }

    #[tokio::test]
    async fn mechanisms_pick_their_database_and_secret() {
        let with = |mech: &str, extra: serde_json::Value| {
            let mut p = json!({ "user": "u", "ssl": "require",
                "options": { "sslCertPath": "c.pem", "driverParams": { "authMechanism": mech } } });
            if let (Some(p), Some(extra)) = (p.as_object_mut(), extra.as_object()) {
                for (k, v) in extra {
                    p.insert(k.clone(), v.clone());
                }
            }
            profile(p)
        };
        let o = options_of(&with("x509", json!({})), Some("ignored"), None)
            .await
            .unwrap();
        let c = o.credential.unwrap();
        assert_eq!(c.mechanism, Some(AuthMechanism::MongoDbX509));
        assert_eq!(
            (c.source.as_deref(), c.password.as_deref()),
            (Some("$external"), None)
        );
        let o = options_of(&with("plain", json!({})), Some("pw"), None)
            .await
            .unwrap();
        let c = o.credential.unwrap();
        assert_eq!(c.mechanism, Some(AuthMechanism::Plain));
        assert_eq!(
            (c.source.as_deref(), c.password.as_deref()),
            (Some("$external"), Some("pw"))
        );
        let o = options_of(&with("scramSha1", json!({})), Some("pw"), None)
            .await
            .unwrap();
        assert_eq!(
            o.credential.unwrap().mechanism,
            Some(AuthMechanism::ScramSha1)
        );
        let o = options_of(&with("oidc", json!({})), Some("tok"), None)
            .await
            .unwrap();
        let c = o.credential.unwrap();
        assert_eq!(c.mechanism, Some(AuthMechanism::MongoDbOidc));
        assert_eq!(c.password, None);
        let err = options_of(&with("oidc", json!({})), None, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("needs an access token"), "{err}");
    }

    #[tokio::test]
    async fn the_token_feed_runs_the_command_again_after_the_first_token() {
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = runs.clone();
        let command = CommandPassword::with_fetch(Duration::from_secs(600), move || {
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move { Ok(format!("token-{n}")) })
        })
        .seeded(Some("token-0"));
        let feed = TokenFeed::new(Some("token-0".into()), Some(Arc::new(command)));
        // The first call is the token the connect just printed.
        assert_eq!(feed.next().await.unwrap(), "token-0");
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        // The crate calls again only after the server refused it: a fresh one.
        assert_eq!(feed.next().await.unwrap(), "token-1");
        assert_eq!(feed.next().await.unwrap(), "token-2");
        let fixed = TokenFeed::new(Some("t".into()), None);
        assert_eq!(fixed.next().await.unwrap(), "t");
        assert_eq!(fixed.next().await.unwrap(), "t");
    }

    #[tokio::test]
    async fn a_login_renews_only_a_command_password_it_sends() {
        let command = Arc::new(
            CommandPassword::with_fetch(Duration::from_secs(600), || {
                Box::pin(async { Ok("fresh".to_string()) })
            })
            .seeded(Some("old")),
        );
        let p = profile(json!({ "user": "u" }));
        let params = MongoParams::of_profile(&p).unwrap();
        let target = ConnectTarget {
            profile: &p,
            password: Some("old"),
            host_override: None,
        };
        let options = client_options(&target, &params, Some(command.clone()))
            .await
            .unwrap();
        let login = Login {
            options,
            command: Some(command),
        };
        assert!(login.renewable());
        let renewed = login.renewed().await.unwrap();
        assert_eq!(
            renewed.credential.unwrap().password.as_deref(),
            Some("fresh")
        );
        let no_command = Login {
            options: login.options.clone(),
            command: None,
        };
        assert!(!no_command.renewable());
    }
}
