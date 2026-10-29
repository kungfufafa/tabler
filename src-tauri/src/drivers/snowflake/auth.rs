//! Sign-in: the connector's `AuthConfig` for each `SnowflakeAuth`, the private key file of key
//! pair sign-in, and the public key fingerprint Snowflake registers for it.
//!
//! The connector signs the key-pair JWT itself (RS256, `iss` = `ACCOUNT.USER.SHA256:<fp>`, `sub` =
//! `ACCOUNT.USER`, upper case, the account up to its first `.`, ten minutes of life); the fake
//! server of `testing` checks those claims and the signature. The fingerprint here is what the
//! connection form shows next to the key, to compare with `DESC USER`'s `RSA_PUBLIC_KEY_FP`.

use aws_lc_rs::encoding::AsDer;
use aws_lc_rs::signature::KeyPair as _;
use base64::Engine as _;
use snowflake_connector_rs::{AuthConfig, KeyPairConfig, PasswordConfig};

use super::params::SnowflakeAuth;
use crate::error::AppError;

/// A private key file larger than this is no PEM key.
const MAX_KEY_FILE: u64 = 64 * 1024;

/// The forms a private key file may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyFormat {
    /// `BEGIN PRIVATE KEY`
    Pkcs8,
    /// `BEGIN ENCRYPTED PRIVATE KEY`: needs the passphrase.
    EncryptedPkcs8,
}

/// A private key read from its file, as the connector takes it.
#[derive(Clone)]
pub struct PrivateKey {
    pub pem: String,
    pub format: KeyFormat,
}

impl std::fmt::Debug for PrivateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivateKey")
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

/// What a PEM text holds, or why the connector cannot use it.
pub fn key_format(pem: &str) -> Result<KeyFormat, AppError> {
    let begins = |label: &str| pem.contains(&format!("-----BEGIN {label}-----"));
    if begins("ENCRYPTED PRIVATE KEY") {
        Ok(KeyFormat::EncryptedPkcs8)
    } else if begins("PRIVATE KEY") {
        Ok(KeyFormat::Pkcs8)
    } else if begins("RSA PRIVATE KEY") {
        Err(AppError::InvalidInput(
            "Snowflake: the key file is PKCS#1 (BEGIN RSA PRIVATE KEY); convert it to PKCS#8 \
             with `openssl pkcs8 -topk8 -inform PEM -in rsa_key.pem -out rsa_key.p8`"
                .into(),
        ))
    } else if begins("OPENSSH PRIVATE KEY") {
        Err(AppError::InvalidInput(
            "Snowflake: the key file is an OpenSSH key; Snowflake takes a PKCS#8 PEM RSA key \
             (openssl genrsa 2048 | openssl pkcs8 -topk8 -out rsa_key.p8)"
                .into(),
        ))
    } else {
        Err(AppError::InvalidInput(
            "Snowflake: the key file holds no PEM private key (BEGIN PRIVATE KEY or BEGIN \
             ENCRYPTED PRIVATE KEY)"
                .into(),
        ))
    }
}

/// Reads the private key file of key-pair sign-in.
pub fn read_private_key(path: &str) -> Result<PrivateKey, AppError> {
    let path = path.trim();
    let meta = std::fs::metadata(path).map_err(|e| {
        AppError::InvalidInput(format!("Snowflake: cannot read the key file {path}: {e}"))
    })?;
    if meta.len() > MAX_KEY_FILE {
        return Err(AppError::InvalidInput(format!(
            "Snowflake: {path} is too large for a private key file"
        )));
    }
    let pem = std::fs::read_to_string(path).map_err(|e| {
        AppError::InvalidInput(format!("Snowflake: cannot read the key file {path}: {e}"))
    })?;
    let format = key_format(&pem)?;
    Ok(PrivateKey { pem, format })
}

/// The DER bytes of the first PEM block labelled `label`.
fn pem_der(pem: &str, label: &str) -> Option<Vec<u8>> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let start = pem.find(&begin)? + begin.len();
    let stop = start + pem[start..].find(&end)?;
    let body: String = pem[start..stop]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    base64::engine::general_purpose::STANDARD.decode(body).ok()
}

/// `SHA256:<base64>` of the public key's SubjectPublicKeyInfo DER, as `DESC USER` shows it in
/// `RSA_PUBLIC_KEY_FP`. Only for an unencrypted key (an encrypted one needs its passphrase, and
/// decrypting PKCS#8 is the connector's).
pub fn public_key_fingerprint(pem: &str) -> Result<String, AppError> {
    let bad = |why: &str| AppError::InvalidInput(format!("Snowflake: the private key {why}"));
    match key_format(pem)? {
        KeyFormat::EncryptedPkcs8 => {
            return Err(bad("is encrypted; its fingerprint shows once it signs in"));
        }
        KeyFormat::Pkcs8 => {}
    }
    let der = pem_der(pem, "PRIVATE KEY").ok_or_else(|| bad("is no valid PEM"))?;
    let pair = aws_lc_rs::rsa::KeyPair::from_pkcs8(&der)
        .map_err(|e| bad(&format!("is no RSA key of 2048 bits or more ({e})")))?;
    let spki = pair
        .public_key()
        .as_der()
        .map_err(|_| bad("has no public key to show"))?;
    let digest =
        aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, AsRef::<[u8]>::as_ref(&*spki));
    Ok(format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD.encode(digest.as_ref())
    ))
}

/// The secrets and answers a sign-in may use.
#[derive(Default, Clone)]
pub struct AuthInputs {
    /// The profile's secret: the password, the PAT, or the OAuth token (Command mode included).
    pub secret: Option<String>,
    /// The private key, read from `privateKeyPath`.
    pub key: Option<PrivateKey>,
    /// The key passphrase secret.
    pub passphrase: Option<String>,
    /// An MFA passcode typed at connect.
    pub passcode: Option<String>,
    /// The password account asks a second factor.
    pub mfa: bool,
    /// No one can answer a prompt (the MCP process).
    pub unattended: bool,
}

impl std::fmt::Debug for AuthInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthInputs")
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .field("key", &self.key)
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "<redacted>"),
            )
            .field("passcode", &self.passcode.as_ref().map(|_| "<redacted>"))
            .field("mfa", &self.mfa)
            .field("unattended", &self.unattended)
            .finish()
    }
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|s| !s.is_empty())
}

/// The connector's sign-in for `auth`, or why it cannot sign in that way.
pub fn auth_config(auth: SnowflakeAuth, inputs: &AuthInputs) -> Result<AuthConfig, AppError> {
    match auth {
        SnowflakeAuth::KeyPair => {
            let key = inputs.key.as_ref().ok_or_else(|| {
                AppError::InvalidInput("Snowflake: key pair sign-in needs the private key".into())
            })?;
            let config = match key.format {
                KeyFormat::EncryptedPkcs8 => {
                    let passphrase = nonempty(&inputs.passphrase).ok_or_else(|| {
                        AppError::Auth(
                            "the Snowflake private key is encrypted: enter its passphrase".into(),
                        )
                    })?;
                    KeyPairConfig::from_encrypted_pem(key.pem.clone(), passphrase.as_bytes())
                }
                KeyFormat::Pkcs8 => KeyPairConfig::from_pem(key.pem.clone()),
            };
            Ok(AuthConfig::key_pair(config))
        }
        SnowflakeAuth::Pat => {
            let token = nonempty(&inputs.secret).ok_or_else(|| {
                AppError::Auth("enter the Snowflake programmatic access token".into())
            })?;
            Ok(AuthConfig::password(token.to_string()))
        }
        SnowflakeAuth::Oauth => {
            let token = nonempty(&inputs.secret).ok_or_else(|| {
                AppError::Auth("enter the Snowflake OAuth access token (or its command)".into())
            })?;
            Ok(AuthConfig::oauth(token.trim().to_string()))
        }
        SnowflakeAuth::Password => {
            if inputs.mfa && inputs.unattended {
                return Err(AppError::Auth(
                    "Snowflake password sign-in asks for MFA, which an unattended connection \
                     cannot answer: use a key pair or a programmatic access token"
                        .into(),
                ));
            }
            let password = nonempty(&inputs.secret)
                .ok_or_else(|| AppError::Auth("enter the Snowflake password".into()))?;
            let config = PasswordConfig::new(password.to_string());
            // Without a passcode an MFA account sends a push and waits for it.
            let config = match nonempty(&inputs.passcode) {
                Some(code) if inputs.mfa => config.with_passcode(code.trim().to_string()),
                _ => config,
            };
            Ok(AuthConfig::password(config))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::snowflake::testing::{
        ENCRYPTED_KEY, KEY, KEY_FINGERPRINT, KEY_PASSPHRASE, PKCS1_KEY, fixture_path,
    };

    #[test]
    fn key_files_by_their_form() {
        assert_eq!(key_format(KEY).unwrap(), KeyFormat::Pkcs8);
        assert_eq!(
            key_format(ENCRYPTED_KEY).unwrap(),
            KeyFormat::EncryptedPkcs8
        );
        let pkcs1 = key_format(PKCS1_KEY).unwrap_err().to_string();
        assert!(
            pkcs1.contains("PKCS#1") && pkcs1.contains("pkcs8"),
            "{pkcs1}"
        );
        assert!(key_format("-----BEGIN OPENSSH PRIVATE KEY-----").is_err());
        assert!(key_format("hello").is_err());

        let key = read_private_key(&fixture_path("rsa_key_encrypted.p8")).unwrap();
        assert_eq!(key.format, KeyFormat::EncryptedPkcs8);
        assert!(!format!("{key:?}").contains("BEGIN"));
        assert!(read_private_key(&fixture_path("missing.p8")).is_err());
        assert!(read_private_key(&fixture_path("rsa_key_pkcs1.pem")).is_err());
    }

    #[test]
    fn the_fingerprint_is_the_one_snowflake_shows() {
        // KEY_FINGERPRINT comes from Snowflake's documented openssl pipeline (fixtures README).
        assert_eq!(public_key_fingerprint(KEY).unwrap(), KEY_FINGERPRINT);
        assert!(public_key_fingerprint(ENCRYPTED_KEY).is_err());
        assert!(
            public_key_fingerprint("-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----")
                .is_err()
        );
        assert!(
            public_key_fingerprint("-----BEGIN PRIVATE KEY-----\n!!\n-----END PRIVATE KEY-----")
                .is_err()
        );
    }

    #[test]
    fn each_method_takes_what_it_needs() {
        let key = PrivateKey {
            pem: ENCRYPTED_KEY.into(),
            format: KeyFormat::EncryptedPkcs8,
        };
        let mut inputs = AuthInputs {
            key: Some(key),
            ..AuthInputs::default()
        };
        // An encrypted key without its passphrase is an Auth error before anything is sent.
        assert!(matches!(
            auth_config(SnowflakeAuth::KeyPair, &inputs),
            Err(AppError::Auth(_))
        ));
        inputs.passphrase = Some(KEY_PASSPHRASE.into());
        let cfg = auth_config(SnowflakeAuth::KeyPair, &inputs).unwrap();
        assert!(!format!("{cfg:?}").contains(KEY_PASSPHRASE));
        assert!(auth_config(SnowflakeAuth::KeyPair, &AuthInputs::default()).is_err());

        for auth in [
            SnowflakeAuth::Pat,
            SnowflakeAuth::Oauth,
            SnowflakeAuth::Password,
        ] {
            assert!(matches!(
                auth_config(auth, &AuthInputs::default()),
                Err(AppError::Auth(_))
            ));
            let with = AuthInputs {
                secret: Some("s3cret".into()),
                ..AuthInputs::default()
            };
            let cfg = auth_config(auth, &with).unwrap();
            assert!(!format!("{cfg:?}").contains("s3cret"));
        }

        // MFA cannot be answered unattended; attended, a passcode goes with the password.
        let mfa = AuthInputs {
            secret: Some("pw".into()),
            passcode: Some("123456".into()),
            mfa: true,
            unattended: true,
            ..AuthInputs::default()
        };
        let err = auth_config(SnowflakeAuth::Password, &mfa)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unattended"), "{err}");
        let attended = AuthInputs {
            unattended: false,
            ..mfa
        };
        assert!(auth_config(SnowflakeAuth::Password, &attended).is_ok());
        // A plain password account (no MFA) may still connect unattended.
        let plain = AuthInputs {
            secret: Some("pw".into()),
            unattended: true,
            ..AuthInputs::default()
        };
        assert!(auth_config(SnowflakeAuth::Password, &plain).is_ok());
        assert!(!format!("{plain:?}").contains("pw\""));
    }
}
