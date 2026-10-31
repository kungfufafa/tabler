//! LibSQL addresses: the URL forms people paste (`libsql://…`, `wss://…`, `https://…`,
//! `http://127.0.0.1:8080` for `sqld`) become the HTTP base the Hrana endpoints hang off. A token
//! or basic credentials pasted in the URL come out of it, so they never end up in the host field
//! or a deep link.

use percent_encoding::percent_decode_str;
use reqwest::Url;

use crate::error::AppError;

/// A parsed LibSQL URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibsqlUrl {
    /// `http(s)://host[:port][/path]`, no trailing slash.
    pub base: String,
    /// `?authToken=…`.
    pub token: Option<String>,
    /// `user:password@` (a `sqld` behind basic auth), also with only one of the two.
    pub basic: Option<(String, String)>,
    /// A token key (any casing, even empty or repeated) or a user or password was in the URL: a
    /// stored URL that says so is refused, since the params are kept as plain text.
    pub has_secret: bool,
    pub host: String,
    pub port: Option<u16>,
    pub tls: bool,
}

/// Parses a pasted URL. Without a scheme it is HTTPS, like Turso's own clients. `?tls=0` turns
/// a `libsql://` URL into plain HTTP.
pub fn parse_url(input: &str) -> Result<LibsqlUrl, AppError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AppError::InvalidInput("the LibSQL URL is empty".into()));
    }
    let (scheme, rest) = match input.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => ("libsql".to_string(), input),
    };
    let tls = match scheme.as_str() {
        "libsql" | "https" | "wss" => true,
        "http" | "ws" => false,
        other => {
            return Err(AppError::InvalidInput(format!(
                "a LibSQL URL starts with libsql://, https://, http://, wss:// or ws://, not {other}://"
            )));
        }
    };
    let parse = |scheme: &str| {
        Url::parse(&format!("{scheme}://{rest}"))
            .map_err(|e| AppError::InvalidInput(format!("invalid LibSQL URL: {e}")))
    };
    let mut token = None;
    let mut has_secret = false;
    let mut tls = tls;
    let mut kept: Vec<(String, String)> = Vec::new();
    for (k, v) in parse("https")?.query_pairs() {
        match k.to_ascii_lowercase().as_str() {
            // A later empty one does not erase the token.
            "authtoken" | "auth_token" => {
                has_secret = true;
                if !v.is_empty() {
                    token = Some(v.into_owned());
                }
            }
            // Only `libsql://` reads `tls`; the scheme says it for the others.
            "tls" if scheme == "libsql" => tls = !matches!(v.as_ref(), "0" | "false"),
            "tls" => {}
            _ => kept.push((k.into_owned(), v.into_owned())),
        }
    }
    // Parsed again with the real scheme, so its default port is the one dropped.
    let mut url = parse(if tls { "https" } else { "http" })?;
    url.set_query(None);
    if !kept.is_empty() {
        url.query_pairs_mut().extend_pairs(kept);
    }
    url.set_fragment(None);
    let basic = (!url.username().is_empty() || url.password().is_some()).then(|| {
        let decode = |s: &str| percent_decode_str(s).decode_utf8_lossy().into_owned();
        (
            decode(url.username()),
            decode(url.password().unwrap_or_default()),
        )
    });
    has_secret |= basic.is_some();
    // Neither fails on an http(s) URL with a host.
    let _ = url.set_username("");
    let _ = url.set_password(None);
    let host = url
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| AppError::InvalidInput("the LibSQL URL has no host".into()))?
        .to_string();
    let port = url.port();
    let path = url.path().trim_end_matches('/').to_string();
    let query = url.query().map(|q| format!("?{q}")).unwrap_or_default();
    let base = format!(
        "{}://{}{}{path}{query}",
        if tls { "https" } else { "http" },
        host_for_url(&host),
        port.map(|p| format!(":{p}")).unwrap_or_default(),
    );
    Ok(LibsqlUrl {
        base,
        token,
        basic,
        has_secret,
        host: host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string(),
        port,
        tls,
    })
}

/// The base of a profile stored as host, port and TLS on or off. A port that is the scheme's
/// default is left out.
pub fn base_from_parts(host: &str, port: Option<u16>, tls: bool) -> Result<String, AppError> {
    let host = host.trim();
    if host.is_empty() || host.contains(['/', '?', '#', '@', ' ']) {
        return Err(AppError::InvalidInput(format!(
            "\u{201c}{host}\u{201d} is not a LibSQL host name"
        )));
    }
    let port = port.filter(|p| *p != if tls { 443 } else { 80 });
    Ok(format!(
        "{}://{}{}",
        if tls { "https" } else { "http" },
        host_for_url(host.trim_start_matches('[').trim_end_matches(']')),
        port.map(|p| format!(":{p}")).unwrap_or_default()
    ))
}

/// IPv6 addresses go in brackets.
fn host_for_url(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

/// The base a server's `base_url` names for the rest of a stream, resolved against the base the
/// request went to. `None` (keep using that one) when it is on another scheme, host or port: the
/// requests carry the token, and an SSH tunnel maps only the profile's host.
pub fn follow_base_url(current: &str, next: &str) -> Option<String> {
    let current = Url::parse(current).ok()?;
    let mut next = current.join(next.trim()).ok()?;
    if next.origin() != current.origin() {
        return None;
    }
    next.set_username("").ok()?;
    next.set_password(None).ok()?;
    next.set_fragment(None);
    Some(next.to_string())
}

/// An endpoint below a base (a profile's, or a stream's `base_url`).
pub fn endpoint(base: &str, path: &str) -> String {
    let (base, query) = match base.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (base, None),
    };
    let mut url = format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    if let Some(q) = query {
        url.push('?');
        url.push_str(q);
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(input: &str) -> String {
        parse_url(input).unwrap().base
    }

    #[test]
    fn schemes_map_to_http() {
        assert_eq!(
            base("libsql://db-org.aws-eu-west-1.turso.io"),
            "https://db-org.aws-eu-west-1.turso.io"
        );
        assert_eq!(base("wss://db.turso.io/"), "https://db.turso.io");
        assert_eq!(base("ws://127.0.0.1:8080"), "http://127.0.0.1:8080");
        assert_eq!(base("HTTP://localhost:8080/"), "http://localhost:8080");
        assert_eq!(base("db.turso.io"), "https://db.turso.io");
        assert_eq!(
            base("libsql://127.0.0.1:8080?tls=0"),
            "http://127.0.0.1:8080"
        );
        assert_eq!(base("https://h/ns/db/"), "https://h/ns/db");
        assert_eq!(base("http://[::1]:8080"), "http://[::1]:8080");
        assert_eq!(base("http://h:443"), "http://h:443");
        assert_eq!(base("https://h:443"), "https://h");
        assert_eq!(base("libsql://h:80?tls=0"), "http://h");
        assert!(parse_url("postgres://h").is_err());
        assert!(parse_url("").is_err());
        assert!(parse_url("libsql://").is_err());
    }

    #[test]
    fn tokens_and_credentials_leave_the_url() {
        let u = parse_url("libsql://db.turso.io?authToken=eyJ.abc&tls=1").unwrap();
        assert_eq!(u.base, "https://db.turso.io");
        assert_eq!(u.token.as_deref(), Some("eyJ.abc"));
        assert!(u.tls);
        assert_eq!(u.host, "db.turso.io");
        let empty = parse_url("libsql://db.turso.io?authToken=").unwrap();
        assert_eq!(empty.token, None);
        let basic = parse_url("http://ad%40min:p%3Ass@127.0.0.1:8080/?x=1#frag").unwrap();
        assert_eq!(basic.base, "http://127.0.0.1:8080?x=1");
        assert_eq!(basic.basic, Some(("ad@min".into(), "p:ss".into())));
        assert_eq!(basic.port, Some(8080));
        // `tls` in an https URL changes nothing.
        assert_eq!(base("https://h?tls=0"), "https://h");
        let v6 = parse_url("http://[::1]:8080").unwrap();
        assert_eq!(v6.host, "::1");
        assert!(!v6.has_secret);
    }

    #[test]
    fn any_token_key_or_userinfo_is_a_secret() {
        // A password without a user.
        let pw = parse_url("https://:pw@h/db").unwrap();
        assert_eq!(pw.basic, Some((String::new(), "pw".into())));
        assert!(pw.has_secret);
        assert_eq!(pw.base, "https://h/db");
        assert!(parse_url("https://u@h").unwrap().has_secret);
        // A later empty key neither erases the token nor hides it.
        let twice = parse_url("https://h/db?authToken=SECRET&authToken=").unwrap();
        assert_eq!(twice.token.as_deref(), Some("SECRET"));
        assert!(twice.has_secret);
        assert!(parse_url("https://h?auth_token=").unwrap().has_secret);
        // Any casing is the token, and it never stays in the base sent to the server.
        let cased = parse_url("https://h?AuthToken=S&x=1").unwrap();
        assert_eq!(cased.token.as_deref(), Some("S"));
        assert_eq!(cased.base, "https://h?x=1");
        assert!(!parse_url("https://h/db?x=1").unwrap().has_secret);
        assert!(!parse_url("https://@h").unwrap().has_secret);
    }

    #[test]
    fn parts_and_endpoints() {
        assert_eq!(
            base_from_parts("db.turso.io", Some(443), true).unwrap(),
            "https://db.turso.io"
        );
        assert_eq!(
            base_from_parts("127.0.0.1", Some(8080), false).unwrap(),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            base_from_parts("::1", Some(8080), false).unwrap(),
            "http://[::1]:8080"
        );
        assert!(base_from_parts("h/x", None, true).is_err());
        assert!(base_from_parts(" ", None, true).is_err());
        assert_eq!(
            endpoint("http://h:8080", "v2/pipeline"),
            "http://h:8080/v2/pipeline"
        );
        assert_eq!(endpoint("https://h/db/", "/v3"), "https://h/db/v3");
        assert_eq!(endpoint("https://h?x=1", "v3"), "https://h/v3?x=1");
    }

    #[test]
    fn base_urls_are_followed_on_the_same_origin_only() {
        let follow = |next: &str| follow_base_url("http://h:8080/db", next);
        assert_eq!(
            follow("http://h:8080/replica/db").as_deref(),
            Some("http://h:8080/replica/db")
        );
        // Relative ones resolve against the base the request went to.
        assert_eq!(
            follow("/other/db").as_deref(),
            Some("http://h:8080/other/db")
        );
        assert_eq!(
            follow("http://u:p@h:8080/x#f").as_deref(),
            Some("http://h:8080/x")
        );
        assert_eq!(
            follow_base_url("https://h", "https://H:443/s").as_deref(),
            Some("https://h/s")
        );
        for elsewhere in [
            "http://evil.test:8080/db",
            "https://h:8080/db",
            "http://h:8081/db",
            "//evil.test/db",
            "file:///etc/passwd",
            "javascript:alert(1)",
        ] {
            assert_eq!(follow(elsewhere), None, "{elsewhere}");
        }
        assert_eq!(follow_base_url("not a url", "/db"), None);
    }
}
