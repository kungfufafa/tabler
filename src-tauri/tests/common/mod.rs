//! Helpers shared by the integration tests (`mod common;`): a profile from a test URL, and a
//! session that is only opened when the test's `TABLER_TEST_*_URL` variable is set, so a run
//! without servers passes with "skipped" printed.

// Each test crate uses only some of these.
#![allow(dead_code)]

use std::sync::Arc;

use tabler_lib::drivers::{ConnectTarget, DbSession, connect};
use tabler_lib::types::profile::ConnectionProfile;

/// Installs aws-lc-rs as rustls's process-wide provider, as the app does at startup.
pub fn install_crypto() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
}

/// `scheme://[user[:password]@]host[:port][/database][?…]` as a profile of the scheme's driver
/// (`postgresql` is `postgres`), plus the password. User and password are percent-decoded, an
/// IPv6 host is written `[::1]`, and the query is ignored. `sqlite://<path>` is that database
/// file (`sqlite:///tmp/x.db`, `sqlite://C:/x.db`). Other fields keep their defaults, with a
/// 10 s connect timeout.
pub fn profile_from_url(url: &str) -> (ConnectionProfile, Option<String>) {
    let (scheme, rest) = url
        .split_once("://")
        .unwrap_or_else(|| panic!("{url}: not a scheme://… URL"));
    let driver = match scheme {
        "postgresql" => "postgres",
        other => other,
    };
    if driver == "sqlite" {
        let fields = serde_json::json!({ "sqlitePath": rest });
        return (profile(url, driver, fields), None);
    }
    let (auth, hostdb) = match rest.rsplit_once('@') {
        Some((a, h)) => (Some(a), h),
        None => (None, rest),
    };
    let (user, password) = match auth.map(|a| a.split_once(':').unwrap_or((a, ""))) {
        Some((u, p)) => (Some(pct_decode(u)), Some(pct_decode(p))),
        None => (None, None),
    };
    let hostdb = hostdb.split('?').next().unwrap_or_default();
    let (hostport, database) = hostdb.split_once('/').unwrap_or((hostdb, ""));
    let (host, port) = match hostport.rsplit_once(':') {
        // Not the last group of a bracketed IPv6 address.
        Some((h, p)) if !p.contains(']') => {
            let port = p
                .parse::<u16>()
                .unwrap_or_else(|e| panic!("{url}: port: {e}"));
            (h, Some(port))
        }
        _ => (hostport, None),
    };
    let fields = serde_json::json!({
        "host": host.trim_start_matches('[').trim_end_matches(']'),
        "port": port,
        "user": user,
        "database": Some(database).filter(|d| !d.is_empty()),
    });
    (
        profile(url, driver, fields),
        password.filter(|p| !p.is_empty()),
    )
}

/// A profile of `driver` with `fields` set: only what a session needs, so new profile fields
/// keep their defaults.
fn profile(url: &str, driver: &str, fields: serde_json::Value) -> ConnectionProfile {
    let mut json = serde_json::json!({
        "id": uuid::Uuid::new_v4(),
        "name": format!("{driver}-test"),
        "driver": driver,
        "options": { "connectTimeoutSecs": 10 },
    });
    for (k, v) in fields.as_object().expect("fields") {
        json[k] = v.clone();
    }
    serde_json::from_value(json).unwrap_or_else(|e| panic!("{url}: {e}"))
}

/// `%XX` escapes decoded (a password with `@` or `:` is written `%40`, `%3A`).
fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The URL in the environment variable `var`, or `None` (printing that the test is skipped)
/// when it is unset or empty.
pub fn url(var: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            println!("skipped: {var} unset");
            None
        }
    }
}

/// A session for the URL in the environment variable `var`, with its profile; `None` when the
/// variable is unset (see [`url`]). Panics when the server cannot be reached.
pub async fn open(var: &str) -> Option<(Arc<dyn DbSession>, ConnectionProfile)> {
    let url = url(var)?;
    install_crypto();
    let (profile, password) = profile_from_url(&url);
    let session = connect(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .unwrap_or_else(|e| panic!("{var}: connect failed: {e}"));
    Some((session, profile))
}
