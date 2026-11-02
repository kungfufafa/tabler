//! `SnowflakeParams`: the Snowflake settings of a profile beyond the common fields, read from
//! `options.driver_params`, and the account identifier in the forms users paste (TablePlus #802:
//! region formats of the identifier broke connecting).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::AppError;
use crate::sqlgen::dialect::snowflake::{APP_PARAMETERS, app_parameter_value, parameter_name};
use crate::types::profile::DriverParams;

/// How the login proves who it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SnowflakeAuth {
    /// An RSA key pair: the private key file (`privateKeyPath`, PKCS#8 PEM, encrypted or not)
    /// signs a JWT; its passphrase is the profile's key passphrase secret. Recommended, and the
    /// only way for unattended (MCP) use besides a token.
    #[default]
    KeyPair,
    /// A programmatic access token, sent in place of the password (the profile's secret).
    Pat,
    /// An OAuth access token, usually from the password's Command mode.
    Oauth,
    /// User and password; when `mfa` is set, every sign-in waits for an MFA push to be approved
    /// (no passcode reaches a connect yet). Refused unattended: nobody is there to approve it.
    Password,
}

impl SnowflakeAuth {
    /// Whether the profile's secret (password, PAT or token) is used.
    pub fn uses_secret(self) -> bool {
        !matches!(self, SnowflakeAuth::KeyPair)
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct SnowflakeParams {
    /// The account identifier: `orgname-account_name`, or the legacy locator with its region
    /// (`xy12345.us-east-2.aws`); a pasted URL or host is reduced to it.
    pub account: String,
    pub auth: SnowflakeAuth,
    /// The private key file of `KeyPair` (never stored in the app database).
    #[ts(optional = nullable)]
    pub private_key_path: Option<String>,
    /// `Password`: the account asks a second factor, approved as a push at each sign-in.
    pub mfa: bool,
    #[ts(optional = nullable)]
    pub warehouse: Option<String>,
    #[ts(optional = nullable)]
    pub role: Option<String>,
    /// The schema the session starts in (the database is the profile's).
    #[ts(optional = nullable)]
    pub schema: Option<String>,
    /// Another endpoint than `https://<account>.snowflakecomputing.com`: a PrivateLink host, a
    /// proxy, or a test server. Scheme and host only.
    #[ts(optional = nullable)]
    pub base_url: Option<String>,
    /// Session parameters set at login (`QUERY_TAG`, `TIMEZONE`, …): text, numbers or booleans.
    #[ts(type = "Record<string, string | number | boolean>")]
    pub session_parameters: BTreeMap<String, serde_json::Value>,
    /// `STATEMENT_TIMEOUT_IN_SECONDS` of the session; None: the account's.
    #[ts(optional = nullable)]
    pub statement_timeout_secs: Option<u32>,
}

impl SnowflakeParams {
    /// The params of a profile, or why they cannot be read.
    pub fn from_driver_params(params: Option<&DriverParams>) -> Result<SnowflakeParams, AppError> {
        let Some(DriverParams(map)) = params else {
            return Err(AppError::InvalidInput(
                "Snowflake: the account identifier is required".into(),
            ));
        };
        let mut parsed: SnowflakeParams =
            serde_json::from_value(serde_json::Value::Object(map.clone()))
                .map_err(|e| AppError::InvalidInput(format!("Snowflake settings: {e}")))?;
        parsed.account = normalize_account(&parsed.account)?;
        parsed.check()?;
        Ok(parsed)
    }

    /// What a profile being saved must hold beyond the common fields.
    pub fn check(&self) -> Result<(), AppError> {
        if self.auth == SnowflakeAuth::KeyPair
            && self
                .private_key_path
                .as_deref()
                .is_none_or(|p| p.trim().is_empty())
        {
            return Err(AppError::InvalidInput(
                "Snowflake: key pair sign-in needs the private key file".into(),
            ));
        }
        if self.auth == SnowflakeAuth::KeyPair
            && let Some(path) = &self.private_key_path
        {
            local_key_path(path)?;
        }
        if let Some(input) = &self.base_url {
            let url = base_url(input)?;
            // A profile from a connections file could pair the user's own key with another
            // host, which could then sign in as the user with the JWT for up to an hour.
            if self.auth == SnowflakeAuth::KeyPair && !snowflake_host(&url) {
                return Err(AppError::InvalidInput(format!(
                    "Snowflake endpoint \u{201c}{input}\u{201d}: {KEY_PAIR_HOST}"
                )));
            }
        }
        for (name, value) in &self.session_parameters {
            parameter_name(name)?;
            if !(value.is_string() || value.is_number() || value.is_boolean()) {
                return Err(AppError::InvalidInput(format!(
                    "Snowflake: session parameter {name} takes text, a number or a boolean"
                )));
            }
        }
        Ok(())
    }

    /// The session parameters sent at login: the profile's, then the statement timeout, then one
    /// statement per request. Tabler splits scripts itself, and with the account's or the
    /// profile's MULTI_STATEMENT_COUNT at 0 a `;` the classifier took for a comment or a string
    /// would run a second statement; at 1 Snowflake refuses the whole request. AUTOCOMMIT stays on
    /// whatever the profile, the user or the account says: with it off, the app's own writes
    /// would wait in a transaction nobody commits, and the editor's in one Tabler cannot see.
    /// `APP_PARAMETERS` keep their values too: with QUOTED_IDENTIFIERS_IGNORE_CASE on, the app's
    /// quoted `"Orders"` would find ORDERS, and an edit could land in that other table; with
    /// another DATE_INPUT_FORMAT (or TIME_ / TIMESTAMP_), the grid's ISO text another date.
    pub fn login_parameters(&self) -> BTreeMap<String, serde_json::Value> {
        let mut out: BTreeMap<String, serde_json::Value> = self
            .session_parameters
            .iter()
            .filter_map(|(k, v)| Some((parameter_name(k).ok()?, v.clone())))
            .collect();
        if let Some(secs) = self.statement_timeout_secs {
            out.insert("STATEMENT_TIMEOUT_IN_SECONDS".into(), secs.into());
        }
        out.insert("MULTI_STATEMENT_COUNT".into(), 1.into());
        out.insert("AUTOCOMMIT".into(), true.into());
        for (name, value) in APP_PARAMETERS {
            out.insert(name.into(), app_parameter_value(value));
        }
        out
    }
}

/// The session parameter that makes quoted identifiers match in any case.
pub const IGNORE_CASE: &str = "QUOTED_IDENTIFIERS_IGNORE_CASE";

/// Why a key pair's endpoint is refused when it is not Snowflake's or this machine's.
const KEY_PAIR_HOST: &str = "a key pair signs in only at Snowflake's own endpoint \
     (*.snowflakecomputing.com) or a test server on this machine (another host could sign in \
     as you with the token)";

/// Refuses a key file that is no full path on this machine: `C:\…` / `C:/…`, or `/…` with one
/// leading separator. Two leading separators in any mix (`\\host\share`, `/\host`,
/// `\\?\UNC\…`) name a share or a device: Windows reads `/` as `\`, and opening a share signs in
/// to its host (with the user's NTLM hash) before anything is read. A `?` is in no Windows file
/// name, only in the object manager's prefixes (`\??\UNC\…`).
pub fn local_key_path(path: &str) -> Result<(), AppError> {
    let path = path.trim();
    let sep = |c: u8| c == b'/' || c == b'\\';
    let local = match path.as_bytes() {
        [a, b, ..] if sep(*a) && sep(*b) => false,
        [drive, b':', s, ..] => drive.is_ascii_alphabetic() && sep(*s),
        [b'/', ..] => true,
        _ => false,
    };
    if local && !path.contains('?') {
        Ok(())
    } else {
        Err(AppError::InvalidInput(format!(
            "Snowflake: the key file \u{201c}{path}\u{201d} must be a full path on this machine \
             (C:\\keys\\rsa_key.p8 or /keys/rsa_key.p8), not a network share"
        )))
    }
}

/// Whether an endpoint is Snowflake's own (`*.snowflakecomputing.com` / `.cn`, PrivateLink
/// included) or this machine: where a key pair may sign in.
pub fn snowflake_host(url: &reqwest::Url) -> bool {
    let host = url.host_str().unwrap_or_default();
    host.ends_with(".snowflakecomputing.com")
        || host.ends_with(".snowflakecomputing.cn")
        || loopback(url)
}

/// The account identifier of what a user pastes: `https://xy12345.eu-west-1.snowflakecomputing.com/`,
/// `myorg-myaccount.snowflakecomputing.com`, `MYORG-MYACCOUNT`, `xy12345.us-east-2.aws`,
/// `xy12345.us-east-1.privatelink`. Lower case (host names are), no scheme, domain, port or path.
pub fn normalize_account(input: &str) -> Result<String, AppError> {
    let mut s = input.trim();
    for scheme in ["https://", "http://"] {
        // `get`, not a slice: pasted text may have no char boundary where the scheme would end.
        if s.get(..scheme.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(scheme))
        {
            s = &s[scheme.len()..];
        }
    }
    let s = s.split(['/', '?', '#']).next().unwrap_or_default();
    let s = s.split(':').next().unwrap_or_default();
    let mut account = s.to_ascii_lowercase();
    for domain in [".snowflakecomputing.com", ".snowflakecomputing.cn"] {
        if let Some(rest) = account.strip_suffix(domain) {
            account = rest.to_string();
        }
    }
    let valid = !account.is_empty()
        && account
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(account_char))
        && !account.starts_with('-');
    if valid {
        Ok(account)
    } else {
        Err(AppError::InvalidInput(format!(
            "\u{201c}{}\u{201d} is no Snowflake account identifier (orgname-account_name or \
             locator.region)",
            input.trim()
        )))
    }
}

fn account_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// The name Snowflake checks a key-pair JWT against: the identifier up to its first `.` (the
/// region and cloud of a locator, `privatelink` and `global` are left out), upper case. The
/// connector signs with it; a `DESC USER` fingerprint mismatch shows as a JWT error.
pub fn jwt_account(account: &str) -> String {
    account
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase()
}

/// A custom endpoint: `https://host[:port]`, nothing after the host. Plain `http://` only to a
/// test server on this machine (`localhost`, `127.0.0.1`, `::1`): the sign-in (key-pair JWT,
/// password, token, passcode) and the session token would cross the network in clear text.
pub fn base_url(input: &str) -> Result<reqwest::Url, AppError> {
    let bad = |why: &str| {
        AppError::InvalidInput(format!("Snowflake endpoint \u{201c}{input}\u{201d}: {why}"))
    };
    let url = reqwest::Url::parse(input.trim()).map_err(|e| bad(&e.to_string()))?;
    if !matches!(url.scheme(), "https" | "http") {
        return Err(bad("use https://"));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(bad("no host"));
    }
    if url.scheme() == "http" && !loopback(&url) {
        return Err(bad(
            "use https:// (plain http only reaches a test server on this machine)",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(bad("credentials do not belong in the URL"));
    }
    if url.query().is_some() || url.fragment().is_some() || !matches!(url.path(), "" | "/") {
        return Err(bad("give the scheme and host only"));
    }
    Ok(url)
}

/// Whether `url`'s host is this machine: `localhost` or a loopback address (the URL parser has
/// already written `127.1` as `127.0.0.1` and lower-cased the name).
fn loopback(url: &reqwest::Url) -> bool {
    let host = url.host_str().unwrap_or_default();
    host == "localhost"
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(json: serde_json::Value) -> Result<SnowflakeParams, AppError> {
        let serde_json::Value::Object(map) = json else {
            panic!("an object");
        };
        SnowflakeParams::from_driver_params(Some(&DriverParams(map)))
    }

    #[test]
    fn account_identifiers_in_every_pasted_form() {
        for (input, want) in [
            ("myorg-myaccount", "myorg-myaccount"),
            ("MYORG-MY_ACCOUNT", "myorg-my_account"),
            (
                "https://myorg-myaccount.snowflakecomputing.com/",
                "myorg-myaccount",
            ),
            (
                "HTTPS://XY12345.us-east-2.aws.snowflakecomputing.com:443/console#/",
                "xy12345.us-east-2.aws",
            ),
            (
                "xy12345.us-east-1.privatelink",
                "xy12345.us-east-1.privatelink",
            ),
            ("  xy12345  ", "xy12345"),
            (
                "xy12345.cn-north-1.snowflakecomputing.cn",
                "xy12345.cn-north-1",
            ),
        ] {
            assert_eq!(normalize_account(input).unwrap(), want, "{input}");
        }
        for bad in [
            "",
            "https://",
            "a..b",
            "a b",
            "x;DROP",
            "-x",
            ".snowflakecomputing.com",
            // No char boundary where `https://` or `http://` would end.
            "ééééé",
            "abcdefg€",
            "abcdef€",
            "https://é",
            "日本語アカウント",
        ] {
            assert!(normalize_account(bad).is_err(), "{bad:?}");
        }
        assert_eq!(jwt_account("xy12345.us-east-2.aws"), "XY12345");
        assert_eq!(jwt_account("myorg-my_account"), "MYORG-MY_ACCOUNT");
    }

    #[test]
    fn params_read_and_checked() {
        let p = params(serde_json::json!({
            "account": "https://MyOrg-Acct.snowflakecomputing.com",
            "auth": "keyPair",
            "privateKeyPath": "C:/keys/rsa_key.p8",
            "warehouse": "COMPUTE_WH",
            "role": "ANALYST",
            "schema": "PUBLIC",
            "sessionParameters": {"query_tag": "tabler", "TIMEZONE": "UTC", "WEEK_START": 1},
            "statementTimeoutSecs": 300
        }))
        .unwrap();
        assert_eq!(p.account, "myorg-acct");
        assert_eq!(p.auth, SnowflakeAuth::KeyPair);
        let login = p.login_parameters();
        assert_eq!(login["QUERY_TAG"], "tabler");
        assert_eq!(login["WEEK_START"], 1);
        assert_eq!(login["STATEMENT_TIMEOUT_IN_SECONDS"], 300);
        assert_eq!(login["MULTI_STATEMENT_COUNT"], 1);
        assert!(!p.auth.uses_secret() && SnowflakeAuth::Pat.uses_secret());
        // One statement per request, whatever the profile says.
        let p = params(serde_json::json!({"account": "a", "auth": "pat",
            "sessionParameters": {"multi_statement_count": 0}}))
        .unwrap();
        assert_eq!(p.login_parameters()["MULTI_STATEMENT_COUNT"], 1);
        // AUTOCOMMIT stays on, whatever the profile says.
        assert_eq!(login["AUTOCOMMIT"], true);
        let p = params(serde_json::json!({"account": "a", "auth": "pat",
            "sessionParameters": {"autocommit": false}}))
        .unwrap();
        assert_eq!(p.login_parameters()["AUTOCOMMIT"], true);
        // Quoted names match in their own case only, whatever the profile says.
        assert_eq!(login[IGNORE_CASE], false);
        let p = params(serde_json::json!({"account": "a", "auth": "pat",
            "sessionParameters": {"quoted_identifiers_ignore_case": true}}))
        .unwrap();
        assert_eq!(p.login_parameters()[IGNORE_CASE], false);
        // The grid's dates are read as it writes them, whatever the profile (or a link) says.
        let p = params(serde_json::json!({"account": "a", "auth": "pat",
            "sessionParameters": {"DATE_INPUT_FORMAT": "YYYY-DD-MM",
                "time_input_format": "HH24MISS", "TIMESTAMP_INPUT_FORMAT": "DD/MM/YYYY HH24:MI"}}))
        .unwrap();
        let login = p.login_parameters();
        for name in [
            "DATE_INPUT_FORMAT",
            "TIME_INPUT_FORMAT",
            "TIMESTAMP_INPUT_FORMAT",
        ] {
            assert_eq!(login[name], "AUTO", "{name}");
        }

        // Key pair needs its file; settings must be names and plain values.
        assert!(params(serde_json::json!({"account": "a"})).is_err());
        assert!(params(serde_json::json!({"account": "a", "auth": "pat"})).is_ok());
        assert!(
            params(serde_json::json!({"account": "a", "auth": "pat",
                "sessionParameters": {"X; DROP": "1"}}))
            .is_err()
        );
        assert!(
            params(serde_json::json!({"account": "a", "auth": "pat",
                "sessionParameters": {"X": {"a": 1}}}))
            .is_err()
        );
        assert!(params(serde_json::json!({"account": "", "auth": "pat"})).is_err());
        assert!(params(serde_json::json!({"account": "a", "auth": "sso"})).is_err());
        assert!(SnowflakeParams::from_driver_params(None).is_err());
    }

    #[test]
    fn custom_endpoints_are_scheme_and_host() {
        assert_eq!(
            base_url("https://xy12345.us-east-1.privatelink.snowflakecomputing.com")
                .unwrap()
                .as_str(),
            "https://xy12345.us-east-1.privatelink.snowflakecomputing.com/"
        );
        // Plain http only to this machine: the sign-in would cross the network in clear text.
        for local in [
            "http://127.0.0.1:8080/",
            "http://127.1:8080",
            "http://localhost:8080",
            "HTTP://LocalHost",
            "http://[::1]:8080",
            "https://example.com",
        ] {
            assert!(base_url(local).is_ok(), "{local}");
        }
        for bad in [
            "ftp://x",
            "https://u:p@x",
            "https://x/path",
            "https://x/?a=1",
            "not a url",
            "http://example.com",
            "http://xy12345.us-east-1.privatelink.snowflakecomputing.com",
            "http://10.0.0.5:8080",
            "http://localhost.example.com",
            "http://127.0.0.1.nip.io",
            "http://[::ffff:10.0.0.5]",
            "http://0.0.0.0:8080",
        ] {
            assert!(base_url(bad).is_err(), "{bad}");
        }
        assert!(
            params(
                serde_json::json!({"account": "a", "auth": "pat", "baseUrl": "http://example.com"})
            )
            .is_err()
        );
        assert!(
            params(serde_json::json!({"account": "a", "auth": "pat", "baseUrl": "https://x/y"}))
                .is_err()
        );
    }

    #[test]
    fn a_key_file_is_a_local_path() {
        for local in [
            "C:\\keys\\rsa_key.p8",
            "c:/keys/rsa_key.p8",
            "/Users/me/.snowflake/rsa_key.p8",
            "  /keys/k.p8  ",
        ] {
            assert!(local_key_path(local).is_ok(), "{local}");
        }
        // Shares and device paths in every mix of separators, and anything relative.
        for bad in [
            "\\\\host\\share\\k.p8",
            "//host/share/k.p8",
            "/\\host\\share\\k.p8",
            "\\/host/share/k.p8",
            "/\\?\\UNC\\host\\s\\k",
            "\\\\?\\UNC\\host\\s\\k",
            "\\\\.\\pipe\\k",
            "\\??\\UNC\\host\\s\\k",
            "/??/UNC/host/s/k",
            "\\keys\\k.p8",
            "keys/k.p8",
            "C:keys\\k.p8",
            "",
        ] {
            let err = local_key_path(bad).unwrap_err();
            assert!(
                err.to_string().contains("not a network share"),
                "{bad}: {err}"
            );
        }
        // The profile's check refuses it, as a connections file may carry one.
        let share = params(serde_json::json!({"account": "a", "auth": "keyPair",
            "privateKeyPath": "\\\\attacker\\share\\k.p8"}));
        assert!(
            share
                .unwrap_err()
                .to_string()
                .contains("not a network share")
        );
        // A path other sign-ins never open is left alone.
        assert!(
            params(serde_json::json!({"account": "a", "auth": "pat",
                "privateKeyPath": "\\\\host\\share\\k.p8"}))
            .is_ok()
        );
    }

    #[test]
    fn a_key_pair_signs_in_only_at_snowflake_or_this_machine() {
        let key = |base: &str| {
            params(serde_json::json!({"account": "a", "auth": "keyPair",
                "privateKeyPath": "C:/keys/rsa_key.p8", "baseUrl": base}))
        };
        for ok in [
            "https://xy12345.us-east-1.privatelink.snowflakecomputing.com",
            "https://myorg-acct.snowflakecomputing.cn",
            "http://127.0.0.1:8080",
            "https://localhost:8443",
        ] {
            assert!(key(ok).is_ok(), "{ok}");
        }
        for bad in [
            "https://attacker.example",
            "https://snowflakecomputing.com.attacker.example",
            "https://xsnowflakecomputing.com",
        ] {
            let err = key(bad).unwrap_err();
            assert!(
                err.to_string().contains("a key pair signs in only"),
                "{bad}: {err}"
            );
        }
        // A token or password the profile's secret holds may go to a proxy.
        assert!(
            params(serde_json::json!({"account": "a", "auth": "pat",
                "baseUrl": "https://proxy.example"}))
            .is_ok()
        );
    }
}
