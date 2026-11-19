//! RNCryptor data format v3 (and v2, the same layout), password-based: what TablePlus seals its
//! connection exports with, on macOS and on Windows.
//!
//! ```text
//! version (1) = 3 | options (1) = 1 (password) | encryption salt (8) | HMAC salt (8) | IV (16)
//! | AES-256-CBC + PKCS#7 ciphertext | HMAC-SHA256 (32) over everything before it
//! ```
//!
//! The encryption key and the HMAC key are PBKDF2-HMAC-SHA1 of the password (UTF-8) with their
//! salt, 10 000 rounds, 32 bytes each. The HMAC is checked before anything is decrypted: a wrong
//! password and a modified file both fail there. Pure: no I/O.

use std::num::NonZeroU32;

use aws_lc_rs::cipher::{AES_256, DecryptionContext, PaddedBlockDecryptingKey, UnboundCipherKey};
use aws_lc_rs::iv::FixedLength;
use aws_lc_rs::{hmac, pbkdf2};

const OPTIONS_PASSWORD: u8 = 1;
const SALT_LEN: usize = 8;
const IV_LEN: usize = 16;
const KEY_LEN: usize = 32;
const HMAC_LEN: usize = 32;
const HEADER_LEN: usize = 2 + SALT_LEN + SALT_LEN + IV_LEN;
const ROUNDS: u32 = 10_000;
/// AES block: the ciphertext is a whole number of them (at least one, for the padding).
const BLOCK: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Not password-based RNCryptor v2 / v3 data (or cut short).
    NotRncryptor,
    /// The HMAC does not match: the password is wrong or the data was modified.
    BadPassword,
}

/// Whether `bytes` look like password-based RNCryptor v2 / v3 data (the header and room for one
/// block and the HMAC). Says nothing about the password.
pub fn looks_like(bytes: &[u8]) -> bool {
    bytes.len() >= HEADER_LEN + BLOCK + HMAC_LEN
        && matches!(bytes[0], 2 | 3)
        && bytes[1] == OPTIONS_PASSWORD
        && (bytes.len() - HEADER_LEN - HMAC_LEN).is_multiple_of(BLOCK)
}

fn key(password: &str, salt: &[u8]) -> [u8; KEY_LEN] {
    let mut out = [0u8; KEY_LEN];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA1,
        NonZeroU32::new(ROUNDS).expect("non-zero"),
        salt,
        password.as_bytes(),
        &mut out,
    );
    out
}

/// Decrypts password-based RNCryptor v2 / v3 data.
pub fn decrypt(bytes: &[u8], password: &str) -> Result<Vec<u8>, Error> {
    if !looks_like(bytes) {
        return Err(Error::NotRncryptor);
    }
    let enc_salt = &bytes[2..2 + SALT_LEN];
    let hmac_salt = &bytes[2 + SALT_LEN..2 + 2 * SALT_LEN];
    let iv: [u8; IV_LEN] = bytes[2 + 2 * SALT_LEN..HEADER_LEN]
        .try_into()
        .map_err(|_| Error::NotRncryptor)?;
    let (signed, tag) = bytes.split_at(bytes.len() - HMAC_LEN);

    let hmac_key = hmac::Key::new(hmac::HMAC_SHA256, &key(password, hmac_salt));
    hmac::verify(&hmac_key, signed, tag).map_err(|_| Error::BadPassword)?;

    let cipher_key = UnboundCipherKey::new(&AES_256, &key(password, enc_salt))
        .map_err(|_| Error::NotRncryptor)?;
    let decrypting =
        PaddedBlockDecryptingKey::cbc_pkcs7(cipher_key).map_err(|_| Error::NotRncryptor)?;
    let mut data = signed[HEADER_LEN..].to_vec();
    let plain_len = decrypting
        .decrypt(&mut data, DecryptionContext::Iv128(FixedLength::from(iv)))
        // Authenticated but badly padded: not data RNCryptor wrote.
        .map_err(|_| Error::NotRncryptor)?
        .len();
    data.truncate(plain_len);
    Ok(data)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use aws_lc_rs::cipher::{EncryptionContext, PaddedBlockEncryptingKey};

    /// Seals `plain` the way RNCryptor v3 does, with fixed salts and IV (tests only).
    pub(crate) fn encrypt(plain: &[u8], password: &str) -> Vec<u8> {
        let enc_salt = [1u8; SALT_LEN];
        let hmac_salt = [2u8; SALT_LEN];
        let iv = [3u8; IV_LEN];
        let mut out = vec![3, OPTIONS_PASSWORD];
        out.extend_from_slice(&enc_salt);
        out.extend_from_slice(&hmac_salt);
        out.extend_from_slice(&iv);
        let cipher_key = UnboundCipherKey::new(&AES_256, &key(password, &enc_salt)).unwrap();
        let encrypting = PaddedBlockEncryptingKey::cbc_pkcs7(cipher_key).unwrap();
        let mut data = plain.to_vec();
        encrypting
            .less_safe_encrypt(&mut data, EncryptionContext::Iv128(FixedLength::from(iv)))
            .unwrap();
        out.extend_from_slice(&data);
        let hmac_key = hmac::Key::new(hmac::HMAC_SHA256, &key(password, &hmac_salt));
        let tag = hmac::sign(&hmac_key, &out);
        out.extend_from_slice(tag.as_ref());
        out
    }

    /// Written by TablePlus for Windows's own RNCryptor (`Encryptor.Encrypt`), password
    /// `tabler-test`; see `tests/fixtures/tableplus/README.md`.
    const WINDOWS_EXPORT: &[u8] =
        include_bytes!("../../tests/fixtures/tableplus/windows-export.tableplusconnection");

    #[test]
    fn opens_a_file_tableplus_wrote() {
        assert!(looks_like(WINDOWS_EXPORT));
        let plain = decrypt(WINDOWS_EXPORT, "tabler-test").unwrap();
        let json: serde_json::Value = serde_json::from_slice(&plain).unwrap();
        assert_eq!(json[1]["ConnectionName"], "Shop");
    }

    #[test]
    fn a_wrong_password_fails_the_hmac() {
        assert_eq!(decrypt(WINDOWS_EXPORT, "nope"), Err(Error::BadPassword));
        assert_eq!(decrypt(WINDOWS_EXPORT, ""), Err(Error::BadPassword));
    }

    #[test]
    fn a_modified_file_fails_the_hmac() {
        let mut bytes = WINDOWS_EXPORT.to_vec();
        bytes[HEADER_LEN + 3] ^= 0x40;
        assert_eq!(decrypt(&bytes, "tabler-test"), Err(Error::BadPassword));
    }

    #[test]
    fn round_trips_and_handles_an_empty_password() {
        for (plain, password) in [
            (&b"[]"[..], ""),
            (&b"0123456789abcdef"[..], "p\u{e4}ss \u{1f511}"),
            (&[7u8; 1000][..], "x"),
        ] {
            let sealed = encrypt(plain, password);
            assert_eq!(decrypt(&sealed, password).unwrap(), plain);
        }
    }

    #[test]
    fn rejects_what_is_not_rncryptor() {
        for bytes in [
            &b""[..],
            &b"{\"format\":\"tabler-connections\"}"[..],
            &[3u8, 0, 0, 0][..],
            &[3u8; 200][..],
        ] {
            assert!(!looks_like(bytes));
            assert_eq!(decrypt(bytes, "x"), Err(Error::NotRncryptor));
        }
        // Version 1 (an old, different key derivation) is not read.
        let mut v1 = encrypt(b"[]", "x");
        v1[0] = 1;
        assert_eq!(decrypt(&v1, "x"), Err(Error::NotRncryptor));
    }
}
