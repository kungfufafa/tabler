//! Checks and parsing of driver options of a connection profile (the form's Other options,
//! extra parameters and encoding), shared by the drivers and the profile commands (which refuse
//! options that cannot work when a connection is saved or tested).

use crate::error::AppError;
use crate::sqlgen::conn_options as opt_sql;
use crate::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode, SslNegotiation};

/// MySQL's connection encoding when the profile sets none.
pub const MYSQL_DEFAULT_CHARSET: &str = "utf8mb4";

/// The profile's MySQL connection encoding (`SET NAMES`), checked.
pub fn mysql_charset(profile: &ConnectionProfile) -> Result<&str, AppError> {
    match profile.options.encoding() {
        None => Ok(MYSQL_DEFAULT_CHARSET),
        Some(enc) if opt_sql::valid_encoding(enc) => Ok(enc),
        Some(enc) => Err(AppError::InvalidInput(format!(
            "\u{201c}{enc}\u{201d} is not a character set name"
        ))),
    }
}

/// Whether `name` (the connection encoding of a profile; `None` or blank: the driver's default,
/// UTF-8) is a UTF-8 encoding. Tabler reads and writes text as UTF-8 only, so a session with another encoding
/// (latin1, sjis, gbk…) would show non-ASCII text as bytes or U+FFFD and store edits mis-encoded.
pub fn is_utf8_encoding(name: Option<&str>) -> bool {
    match name.map(str::trim) {
        None | Some("") => true,
        Some(name) => ["utf8", "utf-8", "utf8mb3", "utf8mb4", "unicode"]
            .iter()
            .any(|utf8| name.eq_ignore_ascii_case(utf8)),
    }
}

/// Whether the session of `profile` is read-only: the profile says so, or the connection uses an
/// encoding that is not UTF-8 (MySQL / PostgreSQL; SQLite text is always UTF-8). The drivers turn
/// it into a read-only server session and the renderer's guards refuse writes up front.
pub fn effective_read_only(profile: &ConnectionProfile) -> bool {
    profile.options.read_only
        || (profile.driver.descriptor().encoding && !is_utf8_encoding(profile.options.encoding()))
}

/// SQLCipher compatibility values (`PRAGMA cipher_compatibility`).
pub const CIPHER_COMPATIBILITY: std::ops::RangeInclusive<u8> = 1..=4;

/// The options of `profile`'s driver that cannot work, refused before connecting.
pub fn check(profile: &ConnectionProfile) -> Result<(), AppError> {
    match profile.driver {
        Driver::Postgres => check_postgres(profile),
        Driver::Mysql => mysql_charset(profile).map(|_| ()),
        Driver::Sqlite => match profile.options.sqlite_cipher_compatibility {
            Some(v) if !CIPHER_COMPATIBILITY.contains(&v) => Err(AppError::InvalidInput(
                "SQLCipher compatibility is 1, 2, 3 or 4".into(),
            )),
            _ => Ok(()),
        },
        // The other drivers check their own options (`drivers::registry::validate`).
        _ => Ok(()),
    }
}

/// Connection parameters that the form sets itself, refused in the extra parameters.
const FORM_PARAMS: [(&str, &str); 14] = [
    ("host", "Host"),
    ("hostaddr", "Host"),
    ("port", "Port"),
    ("user", "User"),
    ("password", "Password"),
    ("dbname", "Database"),
    ("sslmode", "SSL mode"),
    ("sslrootcert", "CA Cert…"),
    ("sslcert", "Cert…"),
    ("sslkey", "Key…"),
    ("sslnegotiation", "SSL negotiation"),
    ("channel_binding", "Other options ▸ Disable channel binding"),
    ("connect_timeout", "Advanced ▸ Connect timeout"),
    ("client_encoding", "Encoding"),
];

/// A profile's extra connection parameters, sorted out: what goes into the startup message and
/// the server settings applied after login.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ExtraParams {
    pub application_name: Option<String>,
    pub options: Option<String>,
    pub target_session_attrs: Option<tokio_postgres::config::TargetSessionAttrs>,
    pub settings: Vec<(String, String)>,
}

/// Splits libpq-style `key=value` pairs, separated by whitespace, newlines or `&` (a pasted URL
/// query). A value may be single-quoted (`options='-c geqo=off'`, `\'` and `\\` escaped); the
/// pairs of a URL query are percent-decoded.
pub fn split_params(text: &str) -> Result<Vec<(String, String)>, AppError> {
    let bad = |m: String| AppError::InvalidInput(format!("Extra parameters: {m}"));
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars
            .peek()
            .is_some_and(|c| c.is_whitespace() || *c == '&' || *c == '?')
        {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' || c.is_whitespace() || c == '&' {
                break;
            }
            key.push(c);
            chars.next();
        }
        while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
            chars.next();
        }
        if chars.next() != Some('=') {
            return Err(bad(format!(
                "\u{201c}{key}\u{201d} needs a value (key=value)"
            )));
        }
        while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
            chars.next();
        }
        let mut value = String::new();
        if chars.peek() == Some(&'\'') {
            chars.next();
            let mut closed = false;
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        if let Some(next) = chars.next() {
                            value.push(next);
                        }
                    }
                    '\'' => {
                        closed = true;
                        break;
                    }
                    c => value.push(c),
                }
            }
            if !closed {
                return Err(bad(format!(
                    "the value of \u{201c}{key}\u{201d} lacks its closing quote"
                )));
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || c == '&' {
                    break;
                }
                value.push(c);
                chars.next();
            }
            if value.contains('%') {
                value = percent_decode(&value);
            }
        }
        out.push((key.to_ascii_lowercase(), value));
    }
    Ok(out)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (
            bytes[i],
            bytes.get(i + 1).copied().and_then(hex),
            bytes.get(i + 2).copied().and_then(hex),
        ) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (b'+', _, _) => {
                out.push(b' ');
                i += 1;
            }
            (b, _, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Sorts the profile's extra parameters (see [`ExtraParams`]); an error names what is wrong.
pub fn extra_params(options: &ProfileOptions) -> Result<ExtraParams, AppError> {
    let mut out = ExtraParams::default();
    let Some(text) = options
        .pg_params
        .as_deref()
        .filter(|t| !t.trim().is_empty())
    else {
        return Ok(out);
    };
    let bad = |m: String| AppError::InvalidInput(format!("Extra parameters: {m}"));
    for (key, value) in split_params(text)? {
        if let Some((_, field)) = FORM_PARAMS.iter().find(|(k, _)| *k == key) {
            return Err(bad(format!("set \u{201c}{key}\u{201d} with {field}")));
        }
        match key.as_str() {
            "application_name" | "fallback_application_name" => {
                if key == "application_name" || out.application_name.is_none() {
                    out.application_name = Some(value);
                }
            }
            "options" => out.options = Some(value),
            "target_session_attrs" => {
                out.target_session_attrs = Some(match value.as_str() {
                    "any" => tokio_postgres::config::TargetSessionAttrs::Any,
                    "read-write" => tokio_postgres::config::TargetSessionAttrs::ReadWrite,
                    "read-only" => tokio_postgres::config::TargetSessionAttrs::ReadOnly,
                    _ => {
                        return Err(bad(format!(
                            "target_session_attrs is any, read-write or read-only, not \u{201c}{value}\u{201d}"
                        )));
                    }
                })
            }
            name => {
                let valid = name
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
                if !valid {
                    return Err(bad(format!("\u{201c}{name}\u{201d} is not a setting name")));
                }
                out.settings.push((name.to_string(), value));
            }
        }
    }
    Ok(out)
}

/// Refuses PostgreSQL options that cannot work, before connecting: DIRECT SSL negotiation with a
/// weak SSL mode (as libpq does), a malformed encoding, unreadable extra parameters.
pub fn check_postgres(profile: &ConnectionProfile) -> Result<(), AppError> {
    let options = &profile.options;
    if options.pg_ssl_negotiation == Some(SslNegotiation::Direct)
        && matches!(profile.ssl, SslMode::Disable | SslMode::Prefer)
    {
        return Err(AppError::InvalidInput(
            "SSL negotiation DIRECT needs the SSL mode require, verify-ca or verify-full".into(),
        ));
    }
    if let Some(enc) = options.encoding()
        && !opt_sql::valid_encoding(enc)
    {
        return Err(AppError::InvalidInput(format!(
            "\u{201c}{enc}\u{201d} is not an encoding name"
        )));
    }
    extra_params(options).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_postgres::config::TargetSessionAttrs;

    fn options(params: &str) -> ProfileOptions {
        ProfileOptions {
            pg_params: Some(params.into()),
            ..ProfileOptions::default()
        }
    }

    #[test]
    fn splits_libpq_style_parameters() {
        assert_eq!(
            split_params("a=1  b = 2\nc='x y \\' z'&d=%2Fp+q ?e=").unwrap(),
            [
                ("a".into(), "1".into()),
                ("b".into(), "2".into()),
                ("c".into(), "x y ' z".into()),
                ("d".into(), "/p q".into()),
                ("e".into(), String::new()),
            ]
        );
        assert!(split_params("lonely").is_err());
        assert!(split_params("x='open").is_err());
        assert!(split_params("   ").unwrap().is_empty());
    }

    #[test]
    fn sorts_startup_parameters_from_server_settings() {
        let extra = extra_params(&options(
            "application_name=reports options='-c geqo=off' target_session_attrs=read-write \
             statement_timeout=5000 search_path='app, public' jit=off myapp.tenant=7",
        ))
        .unwrap();
        assert_eq!(extra.application_name.as_deref(), Some("reports"));
        assert_eq!(extra.options.as_deref(), Some("-c geqo=off"));
        assert_eq!(
            extra.target_session_attrs,
            Some(TargetSessionAttrs::ReadWrite)
        );
        assert_eq!(
            extra.settings,
            [
                ("statement_timeout".to_string(), "5000".to_string()),
                ("search_path".into(), "app, public".into()),
                ("jit".into(), "off".into()),
                ("myapp.tenant".into(), "7".into()),
            ]
        );
        // What the form sets is refused, naming the field to use.
        let err = extra_params(&options("sslmode=require")).unwrap_err();
        assert!(err.to_string().contains("SSL mode"), "{err}");
        let err = extra_params(&options("client_encoding=LATIN1")).unwrap_err();
        assert!(err.to_string().contains("Encoding"), "{err}");
        assert!(extra_params(&options("bad-name=1")).is_err());
        assert!(extra_params(&options("target_session_attrs=primary")).is_err());
        assert_eq!(
            extra_params(&ProfileOptions::default()).unwrap(),
            ExtraParams::default()
        );
    }

    fn profile(driver: Driver) -> ConnectionProfile {
        ConnectionProfile {
            id: uuid::Uuid::nil(),
            name: "t".into(),
            color: None,
            group: None,
            driver,
            host: Some("h".into()),
            port: None,
            user: Some("u".into()),
            database: None,
            ssl: SslMode::Prefer,
            ssl_ca_path: None,
            sqlite_path: Some("/x.db".into()),
            ssh: None,
            options: ProfileOptions::default(),
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        }
    }

    #[test]
    fn checks_driver_options() {
        let mut pg = profile(Driver::Postgres);
        check(&pg).unwrap();
        pg.options.pg_ssl_negotiation = Some(SslNegotiation::Direct);
        assert!(check(&pg).unwrap_err().to_string().contains("DIRECT"));
        pg.ssl = SslMode::Require;
        check(&pg).unwrap();
        pg.options.encoding = Some("LATIN1'".into());
        assert_eq!(check(&pg).unwrap_err().code(), "INVALID_INPUT");

        let mut my = profile(Driver::Mysql);
        my.options.encoding = Some("latin1".into());
        assert_eq!(mysql_charset(&my).unwrap(), "latin1");
        my.options.encoding = Some("lat in1".into());
        assert!(check(&my).is_err());

        let mut lite = profile(Driver::Sqlite);
        lite.options.sqlite_cipher_compatibility = Some(3);
        check(&lite).unwrap();
        lite.options.sqlite_cipher_compatibility = Some(5);
        assert!(check(&lite).is_err());
    }

    #[test]
    fn only_utf8_encodings_are_utf8() {
        for utf8 in [
            None,
            Some("utf8mb4"),
            Some("UTF8"),
            Some("utf-8"),
            Some("utf8mb3"),
            Some("Unicode"),
            Some(" utf8mb4 "),
            Some(" "),
        ] {
            assert!(is_utf8_encoding(utf8), "{utf8:?}");
        }
        for other in [
            "latin1", "LATIN1", "sjis", "gbk", "WIN1252", "EUC_JP", "utf16", "utf8mb5",
        ] {
            assert!(!is_utf8_encoding(Some(other)), "{other}");
        }
    }

    #[test]
    fn a_non_utf8_encoding_makes_the_session_read_only() {
        for driver in [Driver::Mysql, Driver::Postgres] {
            let mut p = profile(driver);
            assert!(!effective_read_only(&p), "no encoding");
            for utf8 in ["utf8mb4", "UTF8", "utf-8", "  "] {
                p.options.encoding = Some(utf8.into());
                assert!(!effective_read_only(&p), "{driver:?} {utf8}");
            }
            for other in ["latin1", "LATIN1", "sjis", "gbk", "WIN1252"] {
                p.options.encoding = Some(other.into());
                assert!(effective_read_only(&p), "{driver:?} {other}");
            }
            // The profile's own read-only option always wins.
            p.options.encoding = None;
            p.options.read_only = true;
            assert!(effective_read_only(&p));
        }
        // SQLite text is always UTF-8: an encoding there changes nothing.
        let mut lite = profile(Driver::Sqlite);
        lite.options.encoding = Some("latin1".into());
        assert!(!effective_read_only(&lite));
        lite.options.read_only = true;
        assert!(effective_read_only(&lite));
    }
}
