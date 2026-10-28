//! What the client answers an Authentication request with: the password hashes of vertica-python's
//! `Password` message (MD5 with md-5, SHA-512 with aws-lc-rs), the cleartext password (over TLS
//! only), or an OAuth access token.

use aws_lc_rs::digest::{SHA512, digest};
use md5::{Digest as _, Md5};

use super::wire::AuthRequest;
use crate::error::AppError;

/// What the login proves itself with.
#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    Password(String),
    /// An OAuth access token (also sent in the startup message, which the server checks).
    OAuthToken(String),
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the secret itself, also not in a log line.
        match self {
            Credential::Password(_) => f.write_str("Password(…)"),
            Credential::OAuthToken(_) => f.write_str("OAuthToken(…)"),
        }
    }
}

/// `md5` + md5hex(md5hex(password + user) + salt), PostgreSQL's MD5 and Vertica's HASH_MD5.
pub fn md5_response(password: &[u8], user: &[u8], salt: &[u8; 4]) -> String {
    let inner = hex::encode(
        Md5::new()
            .chain_update(password)
            .chain_update(user)
            .finalize(),
    );
    let outer = hex::encode(
        Md5::new()
            .chain_update(inner.as_bytes())
            .chain_update(salt)
            .finalize(),
    );
    format!("md5{outer}")
}

/// `sha512` + sha512hex(sha512hex(password + user salt) + salt), Vertica's HASH and HASH_SHA512.
pub fn sha512_response(password: &[u8], user_salt: &[u8; 16], salt: &[u8; 4]) -> String {
    let inner = hex::encode(digest(&SHA512, &[password, user_salt].concat()));
    let outer = hex::encode(digest(&SHA512, &[inner.as_bytes(), salt].concat()));
    format!("sha512{outer}")
}

/// The body of the password message for `request`, or None when the request needs no answer
/// (`Ok`, a grace-period warning). `tls`: the connection is encrypted, which a cleartext password
/// requires.
pub fn response(
    request: &AuthRequest,
    user: &str,
    credential: &Credential,
    tls: bool,
) -> Result<Option<Vec<u8>>, AppError> {
    let password = match credential {
        Credential::Password(p) => p.as_bytes(),
        Credential::OAuthToken(t) => {
            return match request {
                AuthRequest::OAuth => Ok(Some(t.as_bytes().to_vec())),
                AuthRequest::Ok | AuthRequest::PasswordGrace => Ok(None),
                other => Err(refused(other, "an OAuth token")),
            };
        }
    };
    match request {
        AuthRequest::Ok | AuthRequest::PasswordGrace => Ok(None),
        AuthRequest::Md5 { salt } => Ok(Some(
            md5_response(password, user.as_bytes(), salt).into_bytes(),
        )),
        AuthRequest::Sha512 { salt, user_salt } => Ok(Some(
            sha512_response(password, user_salt, salt).into_bytes(),
        )),
        AuthRequest::Cleartext if tls => Ok(Some(password.to_vec())),
        AuthRequest::Cleartext => Err(AppError::Auth(
            "the server asks for the password in clear text (LDAP), which Tabler sends only over \
             TLS: set SSL mode to Require or stricter"
                .into(),
        )),
        AuthRequest::OAuth => Err(AppError::Auth(
            "the server asks for an OAuth access token: choose OAuth token as the authentication"
                .into(),
        )),
        other => Err(refused(other, "a password")),
    }
}

fn refused(request: &AuthRequest, offered: &str) -> AppError {
    AppError::Auth(match request {
        AuthRequest::ChangePassword => {
            "the password has expired: change it with vsql or ALTER USER, then connect again".into()
        }
        AuthRequest::Totp => {
            "the server asks for a TOTP code (multi-factor authentication), which Tabler does not \
             support yet"
                .into()
        }
        AuthRequest::OAuth => "the server asks for an OAuth access token".into(),
        AuthRequest::Unsupported(code) => format!(
            "the server asks for an authentication method Tabler does not support (code {code}: \
             Kerberos, GSS or crypt)"
        ),
        AuthRequest::Ok | AuthRequest::PasswordGrace => "unexpected request".into(),
        AuthRequest::Cleartext | AuthRequest::Md5 { .. } | AuthRequest::Sha512 { .. } => {
            format!("the server asks for a password, and the profile offers {offered}")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Computed outside Rust by the algorithm of vertica-python's `Password.encoded_password`
    // (Node's crypto: md5 / sha512 hex digests, password + key, twice).
    const SALT: [u8; 4] = [1, 2, 3, 4];
    const USER_SALT: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

    #[test]
    fn hash_vectors() {
        assert_eq!(
            md5_response(b"secret", b"dbadmin", &SALT),
            "md5eb4ab686c356bb27c27fd00833d5a193"
        );
        assert_eq!(
            md5_response(b"", b"u", &[0xde, 0xad, 0xbe, 0xef]),
            "md5df53786c226354a8ef1e7d1d9c7b9a46"
        );
        assert_eq!(
            sha512_response(b"secret", &USER_SALT, &SALT),
            "sha512152e5b4a1f6f1e0631ee4f4ebb763d3dc9f94f03b1a14cb6551bad74b0b21e61fe9e1e7f2c4724\
             79a4592206db44b00301f2937c02bcd84368757e4716767b7a"
        );
        let user_salt: [u8; 16] = [
            0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22,
            0x11, 0x00,
        ];
        assert_eq!(
            sha512_response(
                "pässwörd \u{1F600}".as_bytes(),
                &user_salt,
                &[0xde, 0xad, 0xbe, 0xef]
            ),
            "sha5122384dfea375bcebcd9201311f5fb9e1bc8e27c103dee1e3e7d4e0447ed02709cff53d2d5c0850f\
             2c0c6bca795cc55349452786ab5188243174de6c24410815a5"
        );
    }

    #[test]
    fn answers_per_request() {
        let pw = Credential::Password("secret".into());
        let md5 = AuthRequest::Md5 { salt: SALT };
        assert_eq!(
            response(&md5, "dbadmin", &pw, false).unwrap().unwrap(),
            b"md5eb4ab686c356bb27c27fd00833d5a193"
        );
        assert_eq!(response(&AuthRequest::Ok, "u", &pw, false).unwrap(), None);
        assert_eq!(
            response(&AuthRequest::PasswordGrace, "u", &pw, false).unwrap(),
            None
        );
        // Cleartext only over TLS.
        assert_eq!(
            response(&AuthRequest::Cleartext, "u", &pw, true).unwrap(),
            Some(b"secret".to_vec())
        );
        let err = response(&AuthRequest::Cleartext, "u", &pw, false).unwrap_err();
        assert!(
            matches!(err, AppError::Auth(ref m) if m.contains("TLS")),
            "{err}"
        );
        // A token answers OAuth only, and a password never answers OAuth.
        let token = Credential::OAuthToken("eyJ.tok".into());
        assert_eq!(
            response(&AuthRequest::OAuth, "", &token, false).unwrap(),
            Some(b"eyJ.tok".to_vec())
        );
        assert!(response(&md5, "u", &token, true).is_err());
        assert!(response(&AuthRequest::OAuth, "u", &pw, true).is_err());
        for r in [
            AuthRequest::Totp,
            AuthRequest::ChangePassword,
            AuthRequest::Unsupported(2),
        ] {
            assert!(matches!(
                response(&r, "u", &pw, true),
                Err(AppError::Auth(_))
            ));
        }
        assert_eq!(format!("{pw:?}"), "Password(…)");
    }
}
