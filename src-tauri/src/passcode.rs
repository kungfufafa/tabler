//! The app passcode (Settings ▸ Security ▸ Enable Passcode): only a salted PBKDF2-HMAC-SHA256
//! hash is kept, as one JSON value under `SETTINGS_KEY` in the app database's settings table. The
//! passcode itself never leaves the command that hashes or checks it.

use std::num::NonZeroU32;

use aws_lc_rs::{pbkdf2, rand};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};

use crate::appdb::AppDb;
use crate::error::AppError;

/// Settings key of the stored hash (absent = no passcode).
pub const SETTINGS_KEY: &str = "security.passcode";
/// PBKDF2 rounds for new hashes (OWASP's figure for PBKDF2-HMAC-SHA256).
pub const ITERATIONS: u32 = 600_000;
const MIN_ITERATIONS: u32 = 1_000;
const MAX_ITERATIONS: u32 = 10_000_000;
const SALT_LEN: usize = 16;
const HASH_LEN: usize = 32;
const KDF: &str = "pbkdf2-hmac-sha256";
/// Shortest accepted passcode, in characters.
pub const MIN_LENGTH: usize = 4;
/// Longest accepted passcode, in characters (PBKDF2 hashes any length; this bounds the input).
pub const MAX_LENGTH: usize = 256;

/// What is stored: the hash, its salt and how it was derived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredPasscode {
    pub kdf: String,
    pub iterations: u32,
    /// Base64.
    pub salt: String,
    /// Base64.
    pub hash: String,
}

fn rounds(iterations: u32) -> Result<NonZeroU32, AppError> {
    if !(MIN_ITERATIONS..=MAX_ITERATIONS).contains(&iterations) {
        return Err(AppError::InvalidInput(
            "invalid passcode hash settings".into(),
        ));
    }
    NonZeroU32::new(iterations)
        .ok_or_else(|| AppError::InvalidInput("invalid passcode hash settings".into()))
}

/// Refuses passcodes that are too short or too long.
pub fn check_new(passcode: &str) -> Result<(), AppError> {
    let len = passcode.chars().count();
    if len < MIN_LENGTH {
        return Err(AppError::InvalidInput(format!(
            "The passcode needs at least {MIN_LENGTH} characters."
        )));
    }
    if len > MAX_LENGTH {
        return Err(AppError::InvalidInput(format!(
            "The passcode can have at most {MAX_LENGTH} characters."
        )));
    }
    Ok(())
}

/// Hashes `passcode` with a fresh random salt.
pub fn hash(passcode: &str, iterations: u32) -> Result<StoredPasscode, AppError> {
    let rounds = rounds(iterations)?;
    let mut salt = [0u8; SALT_LEN];
    rand::fill(&mut salt).map_err(|_| AppError::Internal("no secure random numbers".into()))?;
    let mut out = [0u8; HASH_LEN];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        rounds,
        &salt,
        passcode.as_bytes(),
        &mut out,
    );
    Ok(StoredPasscode {
        kdf: KDF.into(),
        iterations,
        salt: B64.encode(salt),
        hash: B64.encode(out),
    })
}

/// Whether `passcode` matches `stored` (constant-time comparison). A malformed record never
/// matches.
pub fn verify(stored: &StoredPasscode, passcode: &str) -> bool {
    if stored.kdf != KDF {
        return false;
    }
    let Ok(rounds) = rounds(stored.iterations) else {
        return false;
    };
    let (Ok(salt), Ok(hash)) = (B64.decode(&stored.salt), B64.decode(&stored.hash)) else {
        return false;
    };
    if salt.is_empty() || hash.len() != HASH_LEN {
        return false;
    }
    pbkdf2::verify(
        pbkdf2::PBKDF2_HMAC_SHA256,
        rounds,
        &salt,
        passcode.as_bytes(),
        &hash,
    )
    .is_ok()
}

/// The stored hash, if a passcode is set. A record that does not parse counts as set (and never
/// matches): a damaged settings row must not silently turn the lock off.
pub fn load(appdb: &AppDb) -> Result<Option<StoredPasscode>, AppError> {
    let Some(value) = appdb.get_setting(SETTINGS_KEY)? else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_value(value).unwrap_or_else(|e| {
        log::warn!("passcode: stored hash is unreadable ({e}); it matches nothing");
        StoredPasscode {
            kdf: String::new(),
            iterations: 0,
            salt: String::new(),
            hash: String::new(),
        }
    })))
}

/// Saves (`Some`) or removes (`None`) the stored hash.
pub fn store(appdb: &AppDb, stored: Option<&StoredPasscode>) -> Result<(), AppError> {
    let value = match stored {
        Some(s) => serde_json::to_value(s).map_err(|e| AppError::internal(e.to_string()))?,
        None => serde_json::Value::Null,
    };
    appdb.set_setting(SETTINGS_KEY, &value)
}

/// Sets, changes or removes the passcode. With a passcode already set, `current` must match it.
/// `next: None` turns the passcode off.
pub fn change(
    appdb: &AppDb,
    current: Option<&str>,
    next: Option<&str>,
    iterations: u32,
) -> Result<(), AppError> {
    if let Some(stored) = load(appdb)? {
        let ok = current.is_some_and(|c| verify(&stored, c));
        if !ok {
            return Err(AppError::Auth("wrong passcode".into()));
        }
    }
    match next {
        Some(passcode) => {
            check_new(passcode)?;
            store(appdb, Some(&hash(passcode, iterations)?))
        }
        None => store(appdb, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: u32 = MIN_ITERATIONS;

    #[test]
    fn hashes_with_a_fresh_salt_and_verifies() {
        let a = hash("1234", FAST).unwrap();
        let b = hash("1234", FAST).unwrap();
        assert_eq!(a.kdf, KDF);
        assert_eq!(a.iterations, FAST);
        assert_ne!(a.salt, b.salt, "every hash gets its own salt");
        assert_ne!(a.hash, b.hash);
        assert!(!a.hash.contains("1234"));
        assert!(verify(&a, "1234"));
        assert!(verify(&b, "1234"));
        assert!(!verify(&a, "1235"));
        assert!(!verify(&a, ""));
        assert!(!verify(&a, "12345"));
    }

    #[test]
    fn rejects_malformed_records() {
        let good = hash("secret", FAST).unwrap();
        let bad_kdf = StoredPasscode {
            kdf: "md5".into(),
            ..good.clone()
        };
        assert!(!verify(&bad_kdf, "secret"));
        let bad_rounds = StoredPasscode {
            iterations: 1,
            ..good.clone()
        };
        assert!(!verify(&bad_rounds, "secret"));
        let bad_salt = StoredPasscode {
            salt: "%%%".into(),
            ..good.clone()
        };
        assert!(!verify(&bad_salt, "secret"));
        let short = StoredPasscode {
            hash: B64.encode([0u8; 4]),
            ..good
        };
        assert!(!verify(&short, "secret"));
        assert!(hash("x", 10).is_err());
    }

    #[test]
    fn checks_the_passcode_length() {
        assert!(check_new("123").is_err());
        assert!(check_new("1234").is_ok());
        assert!(check_new(&"x".repeat(MAX_LENGTH)).is_ok());
        assert!(check_new(&"x".repeat(MAX_LENGTH + 1)).is_err());
    }

    #[test]
    fn sets_changes_and_removes_the_passcode() {
        let db = AppDb::open_in_memory().unwrap();
        assert_eq!(load(&db).unwrap(), None);
        change(&db, None, Some("1234"), FAST).unwrap();
        let stored = load(&db).unwrap().expect("set");
        assert!(verify(&stored, "1234"));
        // Only the hash is in the settings row.
        let raw = db.get_setting(SETTINGS_KEY).unwrap().unwrap().to_string();
        assert!(!raw.contains("1234"));

        // Changing needs the current passcode.
        let err = change(&db, None, Some("5678"), FAST).unwrap_err();
        assert_eq!(err.code(), "AUTH_FAILED");
        let err = change(&db, Some("0000"), Some("5678"), FAST).unwrap_err();
        assert_eq!(err.code(), "AUTH_FAILED");
        change(&db, Some("1234"), Some("5678"), FAST).unwrap();
        assert!(verify(&load(&db).unwrap().unwrap(), "5678"));
        // Too short: refused, the old one stays.
        assert!(change(&db, Some("5678"), Some("12"), FAST).is_err());
        assert!(verify(&load(&db).unwrap().unwrap(), "5678"));

        // Turning it off needs it too.
        assert!(change(&db, Some("1234"), None, FAST).is_err());
        change(&db, Some("5678"), None, FAST).unwrap();
        assert_eq!(load(&db).unwrap(), None);
    }

    #[test]
    fn an_unreadable_record_stays_locked() {
        let db = AppDb::open_in_memory().unwrap();
        db.set_setting(SETTINGS_KEY, &serde_json::json!({ "nope": true }))
            .unwrap();
        let stored = load(&db).unwrap().expect("counts as set");
        assert!(!verify(&stored, ""));
        assert!(change(&db, None, None, FAST).is_err());
    }
}
