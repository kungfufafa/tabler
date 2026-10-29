//! RS256 JSON Web Tokens: the signed assertions of service-account (BigQuery) and key-pair
//! (Snowflake) logins. aws-lc-rs signs with an RSA private key from PEM (PKCS#8 or PKCS#1).

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{
    KeyPair as _, RSA_PKCS1_2048_8192_SHA256, RSA_PKCS1_SHA256, RsaKeyPair, UnparsedPublicKey,
};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rustls::pki_types::PrivateKeyDer;
use rustls::pki_types::pem::PemObject as _;
use serde_json::Value;

use crate::error::AppError;

/// An RSA private key that signs RS256 JWTs; parsed once, it signs any number of them.
pub struct Rs256Key {
    key: RsaKeyPair,
}

impl std::fmt::Debug for Rs256Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Rs256Key(***)")
    }
}

impl Rs256Key {
    /// From a `-----BEGIN PRIVATE KEY-----` (PKCS#8) or `-----BEGIN RSA PRIVATE KEY-----`
    /// (PKCS#1) block. Key files written by hand may carry `\n` escapes left unexpanded.
    pub fn from_pem(pem: &str) -> Result<Rs256Key, AppError> {
        let pem = pem.replace("\\n", "\n");
        let der = PrivateKeyDer::from_pem_slice(pem.as_bytes()).map_err(|_| {
            AppError::Auth(
                "the private key is not PEM (-----BEGIN PRIVATE KEY----- or -----BEGIN RSA \
                 PRIVATE KEY-----)"
                    .into(),
            )
        })?;
        let key = match &der {
            PrivateKeyDer::Pkcs8(k) => RsaKeyPair::from_pkcs8(k.secret_pkcs8_der()),
            PrivateKeyDer::Pkcs1(k) => RsaKeyPair::from_der(k.secret_pkcs1_der()),
            _ => {
                return Err(AppError::Auth(
                    "the private key is not an RSA key (RS256 needs one)".into(),
                ));
            }
        }
        .map_err(|e| AppError::Auth(format!("the private key is not a usable RSA key: {e}")))?;
        Ok(Rs256Key { key })
    }

    /// `header.claims.signature`, each part base64url without padding. The header gets
    /// `"alg": "RS256"` and `"typ": "JWT"` unless it has them; another `alg` is refused.
    pub fn sign(&self, header: &Value, claims: &Value) -> Result<String, AppError> {
        let mut header = match header {
            Value::Object(h) => h.clone(),
            Value::Null => serde_json::Map::new(),
            _ => return Err(AppError::internal("a JWT header must be a JSON object")),
        };
        match header.get("alg") {
            None => {
                header.insert("alg".into(), "RS256".into());
            }
            Some(Value::String(alg)) if alg == "RS256" => {}
            Some(other) => {
                return Err(AppError::internal(format!(
                    "an RS256 key cannot sign a JWT with alg {other}"
                )));
            }
        }
        header.entry("typ").or_insert_with(|| "JWT".into());
        let signing_input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(Value::Object(header).to_string()),
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
            .map_err(|_| AppError::Auth("signing the JWT failed".into()))?;
        Ok(format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature)
        ))
    }

    /// The public key (PKCS#1 `RSAPublicKey` DER) that checks the signatures.
    pub fn public_key_der(&self) -> &[u8] {
        self.key.public_key().as_ref()
    }
}

/// The `grant_type` that trades a signed assertion for an access token (RFC 7523).
pub const JWT_BEARER_GRANT: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

/// How long a token request's assertion is valid (Google's maximum).
pub const ASSERTION_LIFETIME_SECS: i64 = 3600;

/// The claims of a service account's request for an access token (RFC 7523, Google's service
/// account login): who asks (`iss`), for which scopes, at which token endpoint (`aud`), valid
/// for an hour from `now` (seconds since the epoch). The key file's `private_key_id` goes in the
/// header as `kid`.
pub fn token_request_claims(issuer: &str, scope: &str, audience: &str, now: i64) -> Value {
    serde_json::json!({
        "iss": issuer,
        "scope": scope,
        "aud": audience,
        "iat": now,
        "exp": now + ASSERTION_LIFETIME_SECS,
    })
}

/// Signs one JWT with the key `pkcs8_pem` (see [`Rs256Key`]).
pub fn sign_rs256(pkcs8_pem: &str, header: &Value, claims: &Value) -> Result<String, AppError> {
    Rs256Key::from_pem(pkcs8_pem)?.sign(header, claims)
}

/// Checks an RS256 JWT against the public key (PKCS#1 `RSAPublicKey` DER) and returns its header
/// and claims; it does not look at `exp` or any other claim.
pub fn verify_rs256(public_key_der: &[u8], token: &str) -> Result<(Value, Value), AppError> {
    let bad = |why: &str| AppError::Auth(format!("invalid JWT: {why}"));
    let mut parts = token.split('.');
    let (Some(header), Some(claims), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(bad("it does not have three parts"));
    };
    let decode = |part: &str| {
        URL_SAFE_NO_PAD
            .decode(part)
            .map_err(|_| bad("not base64url"))
    };
    let signature = decode(signature)?;
    UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, public_key_der)
        .verify(format!("{header}.{claims}").as_bytes(), &signature)
        .map_err(|_| bad("the signature does not match"))?;
    let json = |part: &str| -> Result<Value, AppError> {
        serde_json::from_slice(&decode(part)?).map_err(|_| bad("a part is not JSON"))
    };
    let header = json(header)?;
    if header.get("alg").and_then(Value::as_str) != Some("RS256") {
        return Err(bad("its alg is not RS256"));
    }
    Ok((header, json(claims)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::jwt_key::{CLAIMS, HEADER, PKCS1_PEM, PKCS8_PEM, TOKEN};
    use serde_json::json;

    fn claims() -> Value {
        json!({
            "aud": "https://oauth2.example.test/token",
            "exp": 1_700_003_600,
            "iat": 1_700_000_000,
            "iss": "tabler@example.test",
        })
    }

    #[test]
    fn signs_what_openssl_signs() {
        // Written as serde_json writes them, whichever map order it keeps.
        let header: Value = serde_json::from_str(HEADER).unwrap();
        assert_eq!(serde_json::to_string(&header).unwrap(), HEADER);
        assert_eq!(serde_json::to_string(&claims()).unwrap(), CLAIMS);
        assert_eq!(sign_rs256(PKCS8_PEM, &header, &claims()).unwrap(), TOKEN);
        // PKCS#1, and a header without alg and typ.
        let key = Rs256Key::from_pem(PKCS1_PEM).unwrap();
        assert_eq!(key.sign(&json!({}), &claims()).unwrap(), TOKEN);
        assert_eq!(key.sign(&Value::Null, &claims()).unwrap(), TOKEN);
    }

    #[test]
    fn tokens_verify_with_the_public_key() {
        let key = Rs256Key::from_pem(PKCS8_PEM).unwrap();
        let token = key
            .sign(&json!({"kid": "k1"}), &json!({"sub": "me"}))
            .unwrap();
        let (header, claims) = verify_rs256(key.public_key_der(), &token).unwrap();
        assert_eq!(header, json!({"alg": "RS256", "kid": "k1", "typ": "JWT"}));
        assert_eq!(claims, json!({"sub": "me"}));
        assert_eq!(
            verify_rs256(key.public_key_der(), TOKEN).unwrap().1,
            claims_of_token()
        );
        // One changed character of the claims breaks the signature.
        let mut parts: Vec<String> = token.split('.').map(str::to_string).collect();
        parts[1] = URL_SAFE_NO_PAD.encode(r#"{"sub":"you"}"#);
        let forged = parts.join(".");
        assert_eq!(
            verify_rs256(key.public_key_der(), &forged)
                .unwrap_err()
                .code(),
            "AUTH_FAILED"
        );
        assert!(verify_rs256(key.public_key_der(), "a.b").is_err());
    }

    fn claims_of_token() -> Value {
        serde_json::from_str(CLAIMS).unwrap()
    }

    #[test]
    fn a_service_account_assertion_carries_its_key_id_and_an_hour() {
        let key = Rs256Key::from_pem(PKCS8_PEM).unwrap();
        let claims = token_request_claims(
            "sa@p.iam.gserviceaccount.com",
            "https://www.googleapis.com/auth/bigquery.readonly",
            "https://oauth2.googleapis.com/token",
            1_700_000_000,
        );
        let token = key.sign(&json!({"kid": "kid-1"}), &claims).unwrap();
        let (header, got) = verify_rs256(key.public_key_der(), &token).unwrap();
        assert_eq!(
            header,
            json!({"alg": "RS256", "kid": "kid-1", "typ": "JWT"})
        );
        assert_eq!(
            got,
            json!({
                "iss": "sa@p.iam.gserviceaccount.com",
                "scope": "https://www.googleapis.com/auth/bigquery.readonly",
                "aud": "https://oauth2.googleapis.com/token",
                "iat": 1_700_000_000,
                "exp": 1_700_003_600,
            })
        );
        assert_eq!(
            JWT_BEARER_GRANT,
            "urn:ietf:params:oauth:grant-type:jwt-bearer"
        );
    }

    #[test]
    fn bad_keys_and_headers_are_refused() {
        for pem in [
            "",
            "not a key",
            "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n",
            "-----BEGIN PRIVATE KEY-----\n!!!\n-----END PRIVATE KEY-----\n",
            "-----BEGIN PRIVATE KEY-----\nAAAA\n",
            crate::testkit::tls::CA_PEM,
        ] {
            assert_eq!(
                Rs256Key::from_pem(pem).unwrap_err().code(),
                "AUTH_FAILED",
                "{pem}"
            );
        }
        // A P-256 key is not an RSA key.
        let ec = crate::testkit::tls::SERVER_KEY_PEM;
        assert!(
            Rs256Key::from_pem(ec)
                .unwrap_err()
                .to_string()
                .contains("RSA")
        );
        // Escaped line breaks, as some JSON key files hold them.
        let escaped = PKCS8_PEM.replace('\n', "\\n");
        Rs256Key::from_pem(&escaped).unwrap();
        let key = Rs256Key::from_pem(PKCS8_PEM).unwrap();
        assert!(key.sign(&json!({"alg": "HS256"}), &claims()).is_err());
        assert!(key.sign(&json!([1]), &claims()).is_err());
        assert_eq!(format!("{key:?}"), "Rs256Key(***)");
    }
}
