//! RS256 JWTs for the service-account login (RFC 7523): the assertion the token endpoint trades
//! for an access token. Signed with aws-lc-rs from the key file's PKCS#8 PEM. The shared
//! `drivers::common::jwt` (F8a) is still empty; this can move there or be swapped for it.

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};

use crate::error::AppError;

/// An RSA private key that signs RS256 JWTs.
pub struct JwtSigner {
    key: RsaKeyPair,
    /// The key file's `private_key_id`, sent as `kid`.
    key_id: Option<String>,
}

impl std::fmt::Debug for JwtSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtSigner")
            .field("key_id", &self.key_id)
            .finish_non_exhaustive()
    }
}

impl JwtSigner {
    /// From a `-----BEGIN PRIVATE KEY-----` (PKCS#8) or `-----BEGIN RSA PRIVATE KEY-----`
    /// (PKCS#1) block.
    pub fn from_pem(pem: &str, key_id: Option<&str>) -> Result<JwtSigner, AppError> {
        let (label, der) = pem_der(pem)?;
        let key = match label.as_str() {
            "PRIVATE KEY" => RsaKeyPair::from_pkcs8(&der),
            "RSA PRIVATE KEY" => RsaKeyPair::from_der(&der),
            other => {
                return Err(AppError::Auth(format!(
                    "the key file holds a \u{201c}{other}\u{201d} block, not an RSA private key"
                )));
            }
        }
        .map_err(|e| AppError::Auth(format!("the key file's private key is not usable: {e}")))?;
        Ok(JwtSigner {
            key,
            key_id: key_id.filter(|k| !k.is_empty()).map(str::to_string),
        })
    }

    /// `header.claims.signature`, each part base64url without padding.
    pub fn sign(&self, claims: &serde_json::Value) -> Result<String, AppError> {
        let mut header = serde_json::json!({"alg": "RS256", "typ": "JWT"});
        if let Some(kid) = &self.key_id {
            header["kid"] = serde_json::Value::String(kid.clone());
        }
        let signing_input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let mut signature = vec![0u8; self.key.public_modulus_len()];
        self.key
            .sign(
                &RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                signing_input.as_bytes(),
                &mut signature,
            )
            .map_err(|_| AppError::Auth("signing the login assertion failed".into()))?;
        Ok(format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature)
        ))
    }
}

/// The claims of a service account's token request: who asks (`iss`), for which scopes, at
/// which token endpoint (`aud`), valid for an hour from `now` (seconds since the epoch).
pub fn token_claims(
    client_email: &str,
    scope: &str,
    token_uri: &str,
    now: i64,
) -> serde_json::Value {
    serde_json::json!({
        "iss": client_email,
        "scope": scope,
        "aud": token_uri,
        "iat": now,
        "exp": now + 3600,
    })
}

/// The label and the decoded bytes of the first PEM block of `pem`.
fn pem_der(pem: &str) -> Result<(String, Vec<u8>), AppError> {
    let bad = |why: &str| AppError::Auth(format!("the key file's private key is not PEM: {why}"));
    // Key files written by hand or by other tools may carry `\n` escapes left unexpanded.
    let pem = pem.replace("\\n", "\n");
    let begin = pem
        .find("-----BEGIN ")
        .ok_or_else(|| bad("no BEGIN line"))?;
    let rest = &pem[begin + "-----BEGIN ".len()..];
    let label_end = rest
        .find("-----")
        .ok_or_else(|| bad("a BEGIN line without its end"))?;
    let label = rest[..label_end].trim().to_string();
    let body_and_end = &rest[label_end + 5..];
    let end_marker = format!("-----END {label}-----");
    let end = body_and_end
        .find(&end_marker)
        .ok_or_else(|| bad("no END line"))?;
    let body: String = body_and_end[..end]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let der = STANDARD
        .decode(body.as_bytes())
        .map_err(|e| bad(&format!("bad base64 ({e})")))?;
    Ok((label, der))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::bigquery::testing::{TestKey, test_key};
    use aws_lc_rs::signature::{RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};

    fn parts(jwt: &str) -> Vec<Vec<u8>> {
        jwt.split('.')
            .map(|p| URL_SAFE_NO_PAD.decode(p).expect("base64url"))
            .collect()
    }

    #[test]
    fn signs_rs256_jwts_that_verify() {
        let TestKey {
            pem, public_key, ..
        } = test_key();
        let signer = JwtSigner::from_pem(pem, Some("kid-1")).unwrap();
        let claims = token_claims(
            "sa@p.iam.gserviceaccount.com",
            "https://www.googleapis.com/auth/bigquery.readonly",
            "https://oauth2.googleapis.com/token",
            1_700_000_000,
        );
        let jwt = signer.sign(&claims).unwrap();
        let p = parts(&jwt);
        assert_eq!(p.len(), 3);
        let header: serde_json::Value = serde_json::from_slice(&p[0]).unwrap();
        assert_eq!(
            header,
            serde_json::json!({"alg": "RS256", "typ": "JWT", "kid": "kid-1"})
        );
        let body: serde_json::Value = serde_json::from_slice(&p[1]).unwrap();
        assert_eq!(body["iss"], "sa@p.iam.gserviceaccount.com");
        assert_eq!(body["aud"], "https://oauth2.googleapis.com/token");
        assert_eq!(body["exp"], 1_700_003_600);
        assert_eq!(
            body["scope"],
            "https://www.googleapis.com/auth/bigquery.readonly"
        );
        let signed = jwt.rsplit_once('.').unwrap().0;
        UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, public_key)
            .verify(signed.as_bytes(), &p[2])
            .expect("the signature verifies with the public key");
        // A different message does not.
        assert!(
            UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, public_key)
                .verify(b"x.y", &p[2])
                .is_err()
        );
    }

    #[test]
    fn reads_escaped_newlines_and_refuses_what_is_no_rsa_key() {
        let TestKey { pem, .. } = test_key();
        let escaped = pem.replace('\n', "\\n");
        assert!(JwtSigner::from_pem(&escaped, None).is_ok());
        for bad in [
            "",
            "not a key",
            "-----BEGIN PRIVATE KEY-----\n!!!\n-----END PRIVATE KEY-----\n",
            "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n",
            "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
            "-----BEGIN PRIVATE KEY-----\nAAAA\n",
        ] {
            let e = JwtSigner::from_pem(bad, None).unwrap_err();
            assert!(matches!(e, AppError::Auth(_)), "{bad:?}: {e:?}");
        }
    }
}
