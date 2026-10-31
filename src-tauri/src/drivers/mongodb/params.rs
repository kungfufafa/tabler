//! `MongoParams`: the MongoDB settings of a profile beyond the common fields, read from
//! `options.driver_params`, and a light reading of a connection string for the checks made
//! before connecting (`ClientOptions::parse` reads it in full when the session opens, SRV
//! included).

use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::schema::{DEFAULT_SAMPLE_SIZE, MAX_SAMPLE_SIZE};
use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, DriverParams, SslMode};

/// How the login proves who it is. The server negotiates SCRAM-SHA-256 or SCRAM-SHA-1 by
/// default; MONGODB-AWS and GSSAPI (Kerberos) are not built into this Tabler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum MongoAuth {
    /// The connection string's mechanism, else the server's choice of SCRAM.
    #[default]
    Default,
    ScramSha1,
    ScramSha256,
    /// The client certificate's subject is the user (`$external`).
    X509,
    /// LDAP through SASL PLAIN (`$external`).
    Plain,
    /// MONGODB-OIDC with an access token, which the password holds (its Command mode can print a
    /// fresh one).
    Oidc,
}

impl MongoAuth {
    /// Whether the mechanism authenticates against `$external` by default.
    pub fn external(self) -> bool {
        matches!(self, MongoAuth::X509 | MongoAuth::Plain | MongoAuth::Oidc)
    }
}

/// Which members serve reads (`readPreference`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum MongoReadPreference {
    #[default]
    Primary,
    PrimaryPreferred,
    Secondary,
    SecondaryPreferred,
    Nearest,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct MongoParams {
    /// A connection string (`mongodb://` or `mongodb+srv://`) without its password or any other
    /// secret (refused): its hosts, database, replica set and options. The profile's user,
    /// password, SSL mode and certificate files apply on top of it.
    #[ts(optional = nullable)]
    pub uri: Option<String>,
    pub auth_mechanism: MongoAuth,
    /// The database that holds the user (`authSource`); empty: the connection string's, else
    /// `admin` (`$external` for X.509, PLAIN and OIDC).
    #[ts(optional = nullable)]
    pub auth_source: Option<String>,
    #[ts(optional = nullable)]
    pub replica_set: Option<String>,
    /// Talk to the one host given rather than discover its replica set; unset: on for one host
    /// without a replica set name, as mongosh does, and always over SSH.
    #[ts(optional = nullable)]
    pub direct_connection: Option<bool>,
    pub read_preference: MongoReadPreference,
    /// Shown in the server's logs and `$currentOp`; empty: Tabler.
    #[ts(optional = nullable)]
    pub app_name: Option<String>,
    /// How long to look for a server to talk to; unset: the connect timeout.
    #[ts(optional = nullable)]
    pub server_selection_timeout_secs: Option<u32>,
    /// Documents sampled to infer a collection's columns (1–10000); unset: 100.
    #[ts(optional = nullable)]
    pub sample_size: Option<u32>,
}

pub const DEFAULT_APP_NAME: &str = "Tabler";

impl MongoParams {
    /// The params of a profile, or why they cannot be read.
    pub fn from_driver_params(params: Option<&DriverParams>) -> Result<MongoParams, AppError> {
        let Some(DriverParams(map)) = params else {
            return Ok(MongoParams::default());
        };
        serde_json::from_value(serde_json::Value::Object(map.clone()))
            .map_err(|e| AppError::InvalidInput(format!("MongoDB settings: {e}")))
    }

    /// The profile's params, checked against the rest of the profile.
    pub fn of_profile(profile: &ConnectionProfile) -> Result<MongoParams, AppError> {
        let params = MongoParams::from_driver_params(profile.options.driver_params.as_ref())?;
        params.check(profile)?;
        Ok(params)
    }

    pub fn uri(&self) -> Option<&str> {
        trimmed(&self.uri)
    }

    pub fn auth_source(&self) -> Option<&str> {
        trimmed(&self.auth_source)
    }

    pub fn replica_set(&self) -> Option<&str> {
        trimmed(&self.replica_set)
    }

    pub fn app_name(&self) -> &str {
        trimmed(&self.app_name).unwrap_or(DEFAULT_APP_NAME)
    }

    pub fn sample_size(&self) -> u32 {
        self.sample_size.unwrap_or(DEFAULT_SAMPLE_SIZE)
    }

    /// What the server would refuse, or this build cannot do, said before connecting.
    pub fn check(&self, profile: &ConnectionProfile) -> Result<(), AppError> {
        // Without a URL or a host: this machine (`connect::client_options`), as mongosh.
        let uri = self.uri().map(UriParts::parse).transpose()?;
        if let Some(n) = self.sample_size
            && !(1..=MAX_SAMPLE_SIZE).contains(&n)
        {
            return Err(AppError::InvalidInput(format!(
                "MongoDB sample size: {n} is not between 1 and {MAX_SAMPLE_SIZE}"
            )));
        }
        if self.server_selection_timeout_secs == Some(0) {
            return Err(AppError::InvalidInput(
                "MongoDB server selection timeout: at least 1 second".into(),
            ));
        }
        if let Some(uri) = &uri {
            uri.check_mechanism()?;
            uri.check_secrets()?;
            if profile.ssh.is_some() {
                uri.check_tunnel_node(profile)?;
            }
            // Off is the form's first value: a URL that asks for TLS is never sent in plain
            // text (with its password) because the menu was left alone.
            if profile.ssl == SslMode::Disable && uri.tls() {
                return Err(tls_turned_off());
            }
        }
        if self.auth_mechanism == MongoAuth::X509 {
            let tls_off = profile.ssl == SslMode::Disable
                || (profile.ssl == SslMode::Prefer && !uri.as_ref().is_some_and(UriParts::tls));
            if tls_off {
                return Err(AppError::InvalidInput(
                    "MongoDB X.509 sign-in needs TLS: turn SSL on".into(),
                ));
            }
            if trimmed(&profile.options.ssl_cert_path).is_none()
                && trimmed(&profile.options.ssl_key_path).is_none()
            {
                return Err(AppError::InvalidInput(
                    "MongoDB X.509 sign-in needs a client certificate: choose its file".into(),
                ));
            }
        }
        if self.auth_mechanism == MongoAuth::Plain
            && trimmed(&profile.user).is_none()
            && uri.as_ref().is_none_or(|u| u.user.is_none())
        {
            return Err(AppError::InvalidInput(
                "MongoDB PLAIN (LDAP) sign-in needs a user".into(),
            ));
        }
        Ok(())
    }
}

fn trimmed(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// SSL Off with a connection string that turns TLS on.
pub fn tls_turned_off() -> AppError {
    AppError::InvalidInput(
        "MongoDB connection URL asks for TLS (tls=true, ssl=true or mongodb+srv://) while SSL is \
         Off: turn SSL on, or set tls=false in the URL"
            .into(),
    )
}

/// MongoDB's port where a URL names none.
const DEFAULT_PORT: u16 = 27017;

/// `host[:port]` of a URL as its host (without an IPv6 address's brackets) and port (MongoDB's
/// without one; `None`: not a port).
fn host_port(h: &str) -> (&str, Option<u16>) {
    let (host, port) = match h.strip_prefix('[') {
        Some(rest) => match rest.split_once(']') {
            Some((host, after)) => (host, after.strip_prefix(':')),
            None => (rest, None),
        },
        None => match h.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (h, None),
        },
    };
    let port = match port {
        Some(p) => p.parse().ok(),
        None => Some(DEFAULT_PORT),
    };
    (host, port)
}

/// What the checks need of a connection string: `mongodb://[user[:password]@]host[:port][,…]
/// [/database][?options]`, or `mongodb+srv://` with one host name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UriParts {
    pub srv: bool,
    pub hosts: Vec<String>,
    pub user: Option<String>,
    pub database: Option<String>,
    /// The options, names lower-cased (the server's names are case-insensitive).
    pub options: Vec<(String, String)>,
}

impl UriParts {
    pub fn parse(uri: &str) -> Result<UriParts, AppError> {
        let bad = |why: &str| AppError::InvalidInput(format!("MongoDB connection URL: {why}"));
        let (scheme, rest) = uri
            .split_once("://")
            .ok_or_else(|| bad("it starts with mongodb:// or mongodb+srv://"))?;
        let srv = match scheme.to_ascii_lowercase().as_str() {
            "mongodb" => false,
            "mongodb+srv" => true,
            _ => return Err(bad("it starts with mongodb:// or mongodb+srv://")),
        };
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(end);
        let (userinfo, hosts) = match authority.rsplit_once('@') {
            Some((u, h)) => (Some(u), h),
            None => (None, authority),
        };
        let user = match userinfo {
            Some(u) => {
                let name = u.split_once(':').map_or(u, |(name, _)| name);
                Some(decode(name)).filter(|n| !n.is_empty())
            }
            None => None,
        };
        let hosts: Vec<String> = hosts
            .split(',')
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .map(str::to_string)
            .collect();
        if hosts.is_empty() {
            return Err(bad("it names no host"));
        }
        if srv && (hosts.len() > 1 || hosts[0].contains(':') && !hosts[0].starts_with('[')) {
            return Err(bad("mongodb+srv:// takes one host name, without a port"));
        }
        let (path, query) = match tail.split_once('?') {
            Some((p, q)) => (p, q),
            None => (tail, ""),
        };
        let database = Some(decode(path.trim_start_matches('/'))).filter(|d| !d.is_empty());
        let options = query
            .split(['&', ';'])
            .filter(|kv| !kv.is_empty())
            .map(|kv| {
                let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                // Unicode lower case, as the crate reads the names (U+212A KELVIN SIGN is a `k`
                // to it); decoded too, which the crate does not, to refuse either spelling.
                (decode(k).to_lowercase(), decode(v))
            })
            .collect();
        Ok(UriParts {
            srv,
            hosts,
            user,
            database,
            options,
        })
    }

    pub fn option(&self, name: &str) -> Option<&str> {
        self.options
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Whether the string turns TLS on (`tls` or `ssl`, on by default for SRV).
    pub fn tls(&self) -> bool {
        match self.option("tls").or_else(|| self.option("ssl")) {
            Some(v) => v.eq_ignore_ascii_case("true"),
            None => self.srv,
        }
    }

    /// Refuses a mechanism this build has no code for.
    fn check_mechanism(&self) -> Result<(), AppError> {
        match self.option("authmechanism").map(str::to_ascii_uppercase) {
            Some(m) if m == "MONGODB-AWS" || m == "GSSAPI" => Err(AppError::Unsupported(format!(
                "MongoDB {m} sign-in is not built into this Tabler; use SCRAM, X.509, PLAIN or \
                 MONGODB-OIDC"
            ))),
            _ => Ok(()),
        }
    }

    /// Refuses a secret among the options: the URL is kept in plain text with the profile.
    fn check_secrets(&self) -> Result<(), AppError> {
        for (name, value) in &self.options {
            match name.as_str() {
                "tlscertificatekeyfilepassword" => {
                    return Err(key_password_in_url());
                }
                "proxypassword" => return Err(in_url("proxyPassword", "")),
                "authmechanismproperties"
                    if value.split(',').any(|p| {
                        p.split_once(':').is_some_and(|(k, _)| {
                            k.trim().eq_ignore_ascii_case("AWS_SESSION_TOKEN")
                        })
                    }) =>
                {
                    return Err(in_url("AWS_SESSION_TOKEN", ""));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Over SSH the tunnel reaches one node, the profile's host and port as the SSH server sees
    /// them (`commands::connections::forward_target`; no host: its 127.0.0.1): no SRV record,
    /// no seed list, and no other host, which the session would not reach while it signed in
    /// to whatever listens at the tunnel's end.
    fn check_tunnel_node(&self, profile: &ConnectionProfile) -> Result<(), AppError> {
        let [node] = self.hosts.as_slice() else {
            return Err(one_node());
        };
        if self.srv {
            return Err(one_node());
        }
        let host = trimmed(&profile.host).unwrap_or(crate::tunnel::LOCAL_HOST);
        let port = profile.port.or_else(|| profile.default_port());
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        let (uri_host, uri_port) = host_port(node);
        if !uri_host.eq_ignore_ascii_case(bare) || uri_port.is_none() || uri_port != port {
            let port = port.map_or_else(String::new, |p| format!(":{p}"));
            return Err(AppError::InvalidInput(format!(
                "MongoDB over SSH: the tunnel goes to {host}{port} as the SSH server sees it (the \
                 Host and Port fields), not to {node} of the connection URL: give the URL's host \
                 and port in those fields too"
            )));
        }
        Ok(())
    }
}

fn in_url(what: &str, instead: &str) -> AppError {
    AppError::InvalidInput(format!(
        "MongoDB connection URL: {what} would be saved in plain text with the profile: take it \
         out of the URL{instead}"
    ))
}

/// The client key's password among the URL's options (`connect` refuses it too, as the crate
/// read the URL).
pub(crate) fn key_password_in_url() -> AppError {
    in_url(
        "tlsCertificateKeyFilePassword",
        " and give the client key's password as the Key Passphrase",
    )
}

fn one_node() -> AppError {
    AppError::Unsupported(
        "MongoDB over SSH connects to one node: give one host (mongodb://host:port), not a \
         mongodb+srv:// name or a list of hosts"
            .into(),
    )
}

fn decode(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;

    use super::*;

    /// A MongoDB profile of `db.example.test` without TLS, with `extra`'s fields over it.
    pub(crate) fn profile(extra: serde_json::Value) -> ConnectionProfile {
        let mut base = json!({
            "id": "00000000-0000-0000-0000-000000000001", "name": "m", "driver": "mongodb",
            "host": "db.example.test", "ssl": "disable", "options": {}, "sortOrder": 0,
            "createdAt": "", "updatedAt": "",
        });
        if let (Some(b), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
            for (k, v) in extra {
                b.insert(k.clone(), v.clone());
            }
        }
        serde_json::from_value(base).unwrap()
    }

    fn check(extra: serde_json::Value) -> Result<MongoParams, String> {
        MongoParams::of_profile(&profile(extra)).map_err(|e| e.to_string())
    }

    #[test]
    fn params_read_camel_case_with_defaults() {
        let p = check(json!({ "options": { "driverParams": {
            "authMechanism": "scramSha256", "authSource": " users ", "readPreference": "secondaryPreferred",
            "sampleSize": 250, "appName": "", "replicaSet": "rs0",
        } } }))
        .unwrap();
        assert_eq!(p.auth_mechanism, MongoAuth::ScramSha256);
        assert_eq!(p.auth_source(), Some("users"));
        assert_eq!(p.read_preference, MongoReadPreference::SecondaryPreferred);
        assert_eq!((p.sample_size(), p.app_name()), (250, "Tabler"));
        assert_eq!(p.replica_set(), Some("rs0"));
        assert_eq!(check(json!({})).unwrap(), MongoParams::default());
        assert_eq!(MongoParams::default().sample_size(), 100);
        let err = check(json!({ "options": { "driverParams": { "authMechanism": "kerberos" } } }));
        assert!(
            err.unwrap_err()
                .starts_with("MongoDB settings: unknown variant")
        );
    }

    #[test]
    fn without_a_host_or_a_url_this_machine_is_used() {
        // `connect` tests the address (127.0.0.1, as mongosh).
        check(json!({ "host": " " })).unwrap();
        check(json!({ "host": null })).unwrap();
        check(
            json!({ "host": null, "ssl": "require", "options": { "driverParams": {
            "uri": "mongodb+srv://cluster0.example.net/shop?retryWrites=true"
        } } }),
        )
        .unwrap();
    }

    #[test]
    fn values_out_of_range_are_refused() {
        for (params, msg) in [
            (
                json!({ "sampleSize": 0 }),
                "MongoDB sample size: 0 is not between 1 and 10000",
            ),
            (
                json!({ "sampleSize": 10001 }),
                "MongoDB sample size: 10001 is not between 1 and 10000",
            ),
            (
                json!({ "serverSelectionTimeoutSecs": 0 }),
                "MongoDB server selection timeout: at least 1 second",
            ),
            (
                json!({ "uri": "postgres://x" }),
                "MongoDB connection URL: it starts with mongodb:// or mongodb+srv://",
            ),
            (
                json!({ "uri": "mongodb://" }),
                "MongoDB connection URL: it names no host",
            ),
            (
                json!({ "uri": "mongodb+srv://a.example.net:27017" }),
                "MongoDB connection URL: mongodb+srv:// takes one host name, without a port",
            ),
            (
                json!({ "uri": "mongodb://h/?authMechanism=MONGODB-AWS" }),
                "unsupported: MongoDB MONGODB-AWS sign-in is not built into this Tabler; use \
                 SCRAM, X.509, PLAIN or MONGODB-OIDC",
            ),
        ] {
            let err = check(json!({ "options": { "driverParams": params } })).unwrap_err();
            assert_eq!(err, msg, "{params}");
        }
    }

    #[test]
    fn ssh_reaches_one_node() {
        let ssh = json!({ "host": "bastion", "port": 22, "user": "u", "auth": "agent",
            "keyPath": null, "localPort": null, "keepaliveSecs": null });
        for uri in [
            "mongodb+srv://cluster0.example.net",
            "mongodb://a:27017,b:27017/?replicaSet=rs0",
        ] {
            let err = check(json!({ "ssh": ssh, "options": { "driverParams": { "uri": uri } } }));
            assert!(
                err.unwrap_err().contains("over SSH connects to one node"),
                "{uri}"
            );
        }
        // The URL names the node the tunnel goes to: the profile's host and port.
        for (host, port, uri) in [
            (json!("a"), json!(null), "mongodb://a:27017"),
            (
                json!("A"),
                json!(27018),
                "mongodb://a:27018/shop?authSource=admin",
            ),
            (json!(null), json!(null), "mongodb://127.0.0.1"),
            (json!("::1"), json!(null), "mongodb://[::1]:27017"),
        ] {
            check(json!({ "ssh": ssh, "host": host, "port": port,
                "options": { "driverParams": { "uri": uri } } }))
            .unwrap_or_else(|e| panic!("{uri}: {e}"));
        }
        // Another host or port would be left unreached without a word (the tunnel goes to the
        // profile's, here db.example.test or the SSH server's 127.0.0.1).
        for (host, uri) in [
            (json!(null), "mongodb://db.internal.example:27017/"),
            (json!("db.example.test"), "mongodb://db.internal.example"),
            (json!("db.example.test"), "mongodb://db.example.test:27018"),
            (json!("db.example.test"), "mongodb://db.example.test:x"),
        ] {
            let err = check(json!({ "ssh": ssh, "host": host,
                "options": { "driverParams": { "uri": uri } } }));
            assert!(
                err.as_ref().unwrap_err().contains("the tunnel goes to"),
                "{uri}: {err:?}"
            );
        }
        let err = check(json!({ "ssh": ssh, "host": null,
            "options": { "driverParams": { "uri": "mongodb://db.internal.example:27017/" } } }));
        assert_eq!(
            err.unwrap_err(),
            "MongoDB over SSH: the tunnel goes to 127.0.0.1:27017 as the SSH server sees it (the \
             Host and Port fields), not to db.internal.example:27017 of the connection URL: give \
             the URL's host and port in those fields too"
        );
    }

    #[test]
    fn a_url_that_asks_for_tls_is_not_sent_in_plain_text() {
        for uri in [
            "mongodb://ldap.example.net/?tls=true",
            "mongodb://ldap.example.net/?ssl=TRUE",
            "mongodb+srv://cluster0.example.net",
        ] {
            let params = json!({ "uri": uri, "authMechanism": "plain" });
            let err = check(json!({ "user": "alice", "options": { "driverParams": params } }));
            assert_eq!(err.unwrap_err(), tls_turned_off().to_string(), "{uri}");
            check(
                json!({ "user": "alice", "ssl": "require", "options": { "driverParams": params } }),
            )
            .unwrap();
        }
        // TLS left off by the URL too: plain text, as asked.
        for uri in [
            "mongodb://h/?tls=false",
            "mongodb+srv://c.example.net/?tls=false",
        ] {
            check(json!({ "options": { "driverParams": { "uri": uri } } })).unwrap();
        }
    }

    #[test]
    fn secrets_are_refused_in_the_url() {
        for (uri, what) in [
            (
                "mongodb://h.example.net/?tls=true&tlsCertificateKeyFilePassword=hunter2",
                "tlsCertificateKeyFilePassword",
            ),
            (
                "mongodb://h/?TLSCERTIFICATEKEYFILEPASSWORD=x",
                "tlsCertificateKeyFilePassword",
            ),
            (
                "mongodb://h/?tls%43ertificateKeyFilePassword=x",
                "tlsCertificateKeyFilePassword",
            ),
            // KELVIN SIGN, which Unicode (and the crate) lower-cases to `k`.
            (
                "mongodb://h.example.net/?tls=true&tlsCertificate\u{212A}eyFilePassword=hunter2",
                "tlsCertificateKeyFilePassword",
            ),
            ("mongodb://h/?proxyHost=p&proxyPassword=x", "proxyPassword"),
            (
                "mongodb://h/?authMechanismProperties=SERVICE_NAME:m,aws_session_token:x",
                "AWS_SESSION_TOKEN",
            ),
        ] {
            let err =
                check(json!({ "ssl": "require", "options": { "driverParams": { "uri": uri } } }));
            let err = err.unwrap_err();
            assert!(
                err.starts_with(&format!(
                    "MongoDB connection URL: {what} would be saved in plain text with the profile"
                )),
                "{uri}: {err}"
            );
        }
        let err = check(json!({ "ssl": "require", "options": { "driverParams": {
            "uri": "mongodb://h/?tlsCertificateKeyFilePassword=x" } } }));
        assert!(err.unwrap_err().ends_with("as the Key Passphrase"));
        check(json!({ "options": { "driverParams": {
            "uri": "mongodb://h/?authMechanismProperties=SERVICE_NAME:mongodb" } } }))
        .unwrap();
    }

    #[test]
    fn x509_needs_tls_and_a_certificate_and_plain_a_user() {
        let x509 = json!({ "authMechanism": "x509" });
        let err = check(json!({ "options": { "driverParams": x509 } })).unwrap_err();
        assert_eq!(err, "MongoDB X.509 sign-in needs TLS: turn SSL on");
        let err = check(json!({ "ssl": "require", "options": { "driverParams": x509 } }));
        assert_eq!(
            err.unwrap_err(),
            "MongoDB X.509 sign-in needs a client certificate: choose its file"
        );
        check(json!({ "ssl": "verify-full", "options": {
            "sslCertPath": "C:/certs/client.pem", "driverParams": x509 } }))
        .unwrap();
        // TLS from the connection string counts when the SSL mode leaves it to the string.
        check(
            json!({ "ssl": "prefer", "options": { "sslCertPath": "c.pem", "driverParams": {
            "authMechanism": "x509", "uri": "mongodb://h/?tls=true" } } }),
        )
        .unwrap();
        let err = check(json!({ "options": { "driverParams": { "authMechanism": "plain" } } }));
        assert_eq!(
            err.unwrap_err(),
            "MongoDB PLAIN (LDAP) sign-in needs a user"
        );
        check(json!({ "user": "ldap-user", "options": { "driverParams": { "authMechanism": "plain" } } }))
            .unwrap();
    }

    #[test]
    fn connection_strings_are_read_lightly() {
        let u = UriParts::parse(
            "mongodb://al%40ice:pw@a.example.net:27017,b.example.net/sh%2Fop?replicaSet=rs0&TLS=true&authSource=admin",
        )
        .unwrap();
        assert!(!u.srv);
        assert_eq!(u.hosts, ["a.example.net:27017", "b.example.net"]);
        assert_eq!(u.user.as_deref(), Some("al@ice"));
        assert_eq!(u.database.as_deref(), Some("sh/op"));
        assert_eq!(u.option("replicaset"), Some("rs0"));
        assert!(u.tls());
        let srv = UriParts::parse("MONGODB+SRV://cluster0.example.net/?ssl=false").unwrap();
        assert!(srv.srv && !srv.tls());
        assert!(
            UriParts::parse("mongodb+srv://c.example.net")
                .unwrap()
                .tls()
        );
        let v6 = UriParts::parse("mongodb://[::1]:27017").unwrap();
        assert_eq!(v6.hosts, ["[::1]:27017"]);
        assert!(v6.user.is_none() && v6.database.is_none() && !v6.tls());
    }
}
