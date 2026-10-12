//! `.tablerconnections` files: "Export Connections ▸ …" writes saved connections (and, on request,
//! their passwords) to one; "Import Connections…" reads it back as new connections.
//!
//! The file is JSON. Without a file password it holds the connections in the clear (never any
//! secret). With a file password the whole payload (connections, groups and secrets) is sealed
//! with AES-256-GCM under a key derived from the password with PBKDF2-HMAC-SHA256; a wrong
//! password and a modified file both fail authentication.
//!
//! Pure: no Tauri, app DB or Keychain access (see `commands::profiles` for those).

use std::collections::HashSet;
use std::num::NonZeroU32;

use aws_lc_rs::{aead, pbkdf2, rand};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, ImportPreviewItem};

/// `format` of every connections file.
pub const FORMAT: &str = "tabler-connections";
/// File layout version written by this build (files from newer builds are refused).
pub const VERSION: u32 = 1;
/// The file name extension (without the dot).
pub const FILE_EXTENSION: &str = "tablerconnections";

/// PBKDF2-HMAC-SHA256 rounds for new files (OWASP's 2023 recommendation).
pub const PBKDF2_ITERATIONS: u32 = 600_000;
/// Below this, a file is refused: it was not written by Tabler (or was tampered with).
const MIN_ITERATIONS: u32 = 1_000;
/// Above this, a file is refused as corrupt: deriving its key would hang the import for minutes.
const MAX_ITERATIONS: u32 = 10_000_000;
const SALT_LEN: usize = 16;
const KEY_LEN: usize = 32;
const KDF: &str = "pbkdf2-hmac-sha256";
const CIPHER: &str = "aes-256-gcm";
/// Bound to the ciphertext, so a sealed payload cannot be passed off as another kind of file.
const AAD: &[u8] = b"tabler-connections/1";

/// Secrets of one exported connection; absent fields were not exported (or not stored).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExportedSecrets {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_passphrase: Option<String>,
}

impl ExportedSecrets {
    pub fn is_empty(&self) -> bool {
        self.password.is_none() && self.ssh_password.is_none() && self.ssh_passphrase.is_none()
    }

    pub fn has_ssh(&self) -> bool {
        self.ssh_password.is_some() || self.ssh_passphrase.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedConnection {
    pub profile: ConnectionProfile,
    #[serde(default, skip_serializing_if = "ExportedSecrets::is_empty")]
    pub secrets: ExportedSecrets,
}

/// What a file carries: the connections and the group names (empty groups included).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Payload {
    #[serde(default)]
    pub groups: Vec<String>,
    pub connections: Vec<ExportedConnection>,
}

impl Payload {
    fn has_secrets(&self) -> bool {
        self.connections.iter().any(|c| !c.secrets.is_empty())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    format: String,
    version: u32,
    #[serde(default)]
    exported_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    payload: Option<Payload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    encrypted: Option<Sealed>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Sealed {
    kdf: String,
    iterations: u32,
    salt: String,
    cipher: String,
    nonce: String,
    data: String,
}

/// Outcome of reading a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOutcome {
    /// The file is encrypted and no password was given.
    NeedsPassword,
    Opened {
        payload: Payload,
        encrypted: bool,
    },
}

fn derive_key(password: &str, salt: &[u8], iterations: u32) -> Result<aead::LessSafeKey, AppError> {
    let rounds = NonZeroU32::new(iterations)
        .ok_or_else(|| AppError::InvalidInput("invalid key derivation settings".into()))?;
    let mut key = [0u8; KEY_LEN];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        rounds,
        salt,
        password.as_bytes(),
        &mut key,
    );
    let unbound = aead::UnboundKey::new(&aead::AES_256_GCM, &key)
        .map_err(|_| AppError::Internal("cannot set up AES-256-GCM".into()))?;
    Ok(aead::LessSafeKey::new(unbound))
}

fn random<const N: usize>() -> Result<[u8; N], AppError> {
    let mut out = [0u8; N];
    rand::fill(&mut out).map_err(|_| AppError::Internal("no secure random numbers".into()))?;
    Ok(out)
}

/// Serialises `payload` as a connections file. Secrets need a `password`; with one the payload is
/// encrypted. An empty password counts as none.
pub fn write(
    payload: &Payload,
    password: Option<&str>,
    exported_at: &str,
) -> Result<Vec<u8>, AppError> {
    write_with_iterations(payload, password, exported_at, PBKDF2_ITERATIONS)
}

fn write_with_iterations(
    payload: &Payload,
    password: Option<&str>,
    exported_at: &str,
    iterations: u32,
) -> Result<Vec<u8>, AppError> {
    let password = password.filter(|p| !p.is_empty());
    let mut envelope = Envelope {
        format: FORMAT.into(),
        version: VERSION,
        exported_at: exported_at.into(),
        payload: None,
        encrypted: None,
    };
    match password {
        None if payload.has_secrets() => {
            return Err(AppError::InvalidInput(
                "Set a file password to export passwords and passphrases".into(),
            ));
        }
        None => envelope.payload = Some(payload.clone()),
        Some(password) => {
            let salt: [u8; SALT_LEN] = random()?;
            let nonce: [u8; aead::NONCE_LEN] = random()?;
            let key = derive_key(password, &salt, iterations)?;
            let mut data = serde_json::to_vec(payload)?;
            key.seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(AAD),
                &mut data,
            )
            .map_err(|_| AppError::Internal("encryption failed".into()))?;
            envelope.encrypted = Some(Sealed {
                kdf: KDF.into(),
                iterations,
                salt: B64.encode(salt),
                cipher: CIPHER.into(),
                nonce: B64.encode(nonce),
                data: B64.encode(&data),
            });
        }
    }
    let mut out = serde_json::to_vec_pretty(&envelope)?;
    out.push(b'\n');
    Ok(out)
}

fn not_a_file() -> AppError {
    AppError::InvalidInput("This is not a Tabler connections file".into())
}

fn wrong_password() -> AppError {
    AppError::InvalidInput("The file password is wrong, or the file was modified".into())
}

/// Reads a connections file. An encrypted file needs `password` (an empty one counts as none).
pub fn read(bytes: &[u8], password: Option<&str>) -> Result<ReadOutcome, AppError> {
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| not_a_file())?;
    if envelope.format != FORMAT {
        return Err(not_a_file());
    }
    if envelope.version > VERSION {
        return Err(AppError::InvalidInput(
            "This connections file was made by a newer version of Tabler".into(),
        ));
    }
    let sealed = match (envelope.payload, envelope.encrypted) {
        (Some(payload), None) => {
            return Ok(ReadOutcome::Opened {
                payload,
                encrypted: false,
            });
        }
        (None, Some(sealed)) => sealed,
        _ => return Err(not_a_file()),
    };
    let Some(password) = password.filter(|p| !p.is_empty()) else {
        return Ok(ReadOutcome::NeedsPassword);
    };
    if sealed.kdf != KDF || sealed.cipher != CIPHER || sealed.iterations < MIN_ITERATIONS {
        return Err(AppError::InvalidInput(
            "This connections file uses an unsupported encryption".into(),
        ));
    }
    if sealed.iterations > MAX_ITERATIONS {
        return Err(not_a_file());
    }
    let salt = B64.decode(&sealed.salt).map_err(|_| not_a_file())?;
    let nonce: [u8; aead::NONCE_LEN] = B64
        .decode(&sealed.nonce)
        .ok()
        .and_then(|n| n.try_into().ok())
        .ok_or_else(not_a_file)?;
    let mut data = B64.decode(&sealed.data).map_err(|_| not_a_file())?;
    let key = derive_key(password, &salt, sealed.iterations)?;
    let plain = key
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(AAD),
            &mut data,
        )
        .map_err(|_| wrong_password())?;
    let payload: Payload = serde_json::from_slice(plain).map_err(|_| not_a_file())?;
    Ok(ReadOutcome::Opened {
        payload,
        encrypted: true,
    })
}

/// The import preview: every connection of the file, with a note when its name is taken.
pub fn preview(payload: &Payload, existing: &[ConnectionProfile]) -> Vec<ImportPreviewItem> {
    let names: HashSet<String> = existing.iter().map(|p| name_key(&p.name)).collect();
    payload
        .connections
        .iter()
        .map(|c| ImportPreviewItem {
            profile: c.profile.clone(),
            duplicate_name: names.contains(&name_key(&c.profile.name)),
            has_password: c.secrets.password.is_some(),
            has_ssh_secrets: c.secrets.has_ssh(),
        })
        .collect()
}

fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// The connections chosen for import (by their id in the file), as new profiles: fresh ids,
/// appended to the list (sort order 0 lets the app DB put them last), never connected. Secrets
/// the profile cannot use (an SSH secret without SSH) are dropped.
pub fn plan_import(
    payload: &Payload,
    selected: &[Uuid],
) -> Vec<(ConnectionProfile, ExportedSecrets)> {
    let wanted: HashSet<Uuid> = selected.iter().copied().collect();
    payload
        .connections
        .iter()
        .filter(|c| wanted.contains(&c.profile.id))
        .map(|c| {
            let mut profile = c.profile.clone();
            profile.id = Uuid::new_v4();
            profile.sort_order = 0;
            profile.created_at = String::new();
            profile.updated_at = String::new();
            profile.last_connected_at = None;
            let mut secrets = c.secrets.clone();
            if profile.ssh.is_none() {
                secrets.ssh_password = None;
                secrets.ssh_passphrase = None;
            }
            (profile, secrets)
        })
        .collect()
}

/// Group names in first-seen order without duplicates or blanks: `known` first, then `extra`.
pub fn merge_groups(known: &[String], extra: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    known
        .iter()
        .chain(extra)
        .map(|g| g.trim().to_string())
        .filter(|g| !g.is_empty() && seen.insert(g.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::profile::{
        ConnectionTag, Driver, ProfileOptions, SafeModeLevel, SshAuth, SshConfig, SslMode,
    };

    const FAST: u32 = MIN_ITERATIONS;

    fn profile(name: &str, ssh: bool) -> ConnectionProfile {
        ConnectionProfile {
            id: Uuid::new_v4(),
            name: name.into(),
            color: Some("purple".into()),
            group: Some("Staging".into()),
            driver: Driver::Postgres,
            host: Some("10.0.1.20".into()),
            port: Some(5432),
            user: Some("app".into()),
            database: Some("app_staging".into()),
            ssl: SslMode::VerifyFull,
            ssl_ca_path: Some("/etc/ca.pem".into()),
            sqlite_path: None,
            ssh: ssh.then(|| SshConfig {
                host: "bastion.example.com".into(),
                port: 22,
                user: "deploy".into(),
                auth: SshAuth::Password,
                key_path: None,
                local_port: Some(15432),
                keepalive_secs: Some(20),
            }),
            options: ProfileOptions {
                tag: Some(ConnectionTag::Staging),
                safe_mode: Some(SafeModeLevel::AlertNonSelect),
                startup_sql: Some("SET search_path = app".into()),
                ..ProfileOptions::default()
            },
            sort_order: 7,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-02T00:00:00Z".into(),
            last_connected_at: Some("2026-01-03T00:00:00Z".into()),
        }
    }

    fn payload() -> Payload {
        Payload {
            groups: vec!["Staging".into(), "Empty".into()],
            connections: vec![
                ExportedConnection {
                    profile: profile("Staging", true),
                    secrets: ExportedSecrets {
                        password: Some("db-pw".into()),
                        ssh_password: Some("ssh-pw".into()),
                        ssh_passphrase: None,
                    },
                },
                ExportedConnection {
                    profile: profile("Reporting", false),
                    secrets: ExportedSecrets::default(),
                },
            ],
        }
    }

    #[test]
    fn encrypted_round_trip_keeps_every_field_and_secret() {
        let p = payload();
        let bytes = write_with_iterations(&p, Some("file pw"), "now", FAST).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        // Nothing readable leaks: no secret, no host, no name.
        for needle in ["db-pw", "ssh-pw", "10.0.1.20", "Reporting", "bastion"] {
            assert!(!text.contains(needle), "{needle} leaked");
        }
        assert!(text.contains("\"format\": \"tabler-connections\""));
        assert_eq!(read(&bytes, None).unwrap(), ReadOutcome::NeedsPassword);
        assert_eq!(read(&bytes, Some("")).unwrap(), ReadOutcome::NeedsPassword);
        assert_eq!(
            read(&bytes, Some("file pw")).unwrap(),
            ReadOutcome::Opened {
                payload: p,
                encrypted: true
            }
        );
    }

    #[test]
    fn plain_files_never_carry_secrets() {
        let mut p = payload();
        let err = write_with_iterations(&p, None, "now", FAST).unwrap_err();
        assert_eq!(err.code(), "INVALID_INPUT");
        assert!(err.to_string().contains("file password"), "{err}");
        for c in &mut p.connections {
            c.secrets = ExportedSecrets::default();
        }
        let bytes = write_with_iterations(&p, Some(""), "now", FAST).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.contains("Reporting") && !text.contains("secrets"));
        assert_eq!(
            read(&bytes, Some("ignored")).unwrap(),
            ReadOutcome::Opened {
                payload: p,
                encrypted: false
            }
        );
    }

    #[test]
    fn wrong_password_is_refused() {
        let bytes = write_with_iterations(&payload(), Some("right"), "now", FAST).unwrap();
        let err = read(&bytes, Some("wrong")).unwrap_err();
        assert_eq!(err.code(), "INVALID_INPUT");
        assert!(err.to_string().contains("password is wrong"), "{err}");
    }

    #[test]
    fn tampered_files_are_refused() {
        let bytes = write_with_iterations(&payload(), Some("pw"), "now", FAST).unwrap();
        let mut envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let sealed = envelope["encrypted"].clone();

        // One flipped bit in the ciphertext.
        let mut data = B64.decode(sealed["data"].as_str().unwrap()).unwrap();
        data[3] ^= 0x01;
        envelope["encrypted"]["data"] = B64.encode(&data).into();
        let tampered = serde_json::to_vec(&envelope).unwrap();
        assert!(
            read(&tampered, Some("pw"))
                .unwrap_err()
                .to_string()
                .contains("modified")
        );

        // A swapped nonce or salt fails the same way.
        let mut envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        envelope["encrypted"]["nonce"] = B64.encode([7u8; aead::NONCE_LEN]).into();
        assert!(read(&serde_json::to_vec(&envelope).unwrap(), Some("pw")).is_err());
        let mut envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        envelope["encrypted"]["salt"] = B64.encode([9u8; SALT_LEN]).into();
        assert!(read(&serde_json::to_vec(&envelope).unwrap(), Some("pw")).is_err());

        // Lowered key-derivation cost is refused before decrypting.
        let mut envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        envelope["encrypted"]["iterations"] = 1.into();
        let err = read(&serde_json::to_vec(&envelope).unwrap(), Some("pw")).unwrap_err();
        assert!(err.to_string().contains("unsupported"), "{err}");

        // So is a cost that would hang the import (u32::MAX rounds): refused without deriving.
        envelope["encrypted"]["iterations"] = u32::MAX.into();
        let started = std::time::Instant::now();
        let err = read(&serde_json::to_vec(&envelope).unwrap(), Some("pw")).unwrap_err();
        assert!(
            err.to_string().contains("not a Tabler connections file"),
            "{err}"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        envelope["encrypted"]["iterations"] = (MAX_ITERATIONS + 1).into();
        assert!(read(&serde_json::to_vec(&envelope).unwrap(), Some("pw")).is_err());
    }

    #[test]
    fn other_files_are_refused() {
        for bytes in [
            &b"not json"[..],
            br#"{"format":"tableplus","version":1,"payload":{"connections":[]}}"#,
            br#"{"format":"tabler-connections","version":1}"#,
        ] {
            assert!(
                read(bytes, None)
                    .unwrap_err()
                    .to_string()
                    .contains("not a Tabler connections file")
            );
        }
        let newer = br#"{"format":"tabler-connections","version":99,"payload":{"connections":[]}}"#;
        assert!(read(newer, None).unwrap_err().to_string().contains("newer"));
    }

    #[test]
    fn the_production_cost_round_trips() {
        let p = payload();
        let bytes = write(&p, Some("pw"), "now").unwrap();
        let envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(envelope["encrypted"]["iterations"], PBKDF2_ITERATIONS);
        assert_eq!(envelope["encrypted"]["cipher"], CIPHER);
        assert!(matches!(
            read(&bytes, Some("pw")).unwrap(),
            ReadOutcome::Opened { .. }
        ));
    }

    #[test]
    fn preview_flags_duplicates_and_secrets() {
        let p = payload();
        let mut existing = profile("  staging ", false);
        existing.id = Uuid::new_v4();
        let items = preview(&p, &[existing]);
        assert_eq!(items.len(), 2);
        assert!(items[0].duplicate_name && items[0].has_password && items[0].has_ssh_secrets);
        assert!(!items[1].duplicate_name && !items[1].has_password && !items[1].has_ssh_secrets);
    }

    #[test]
    fn import_plan_assigns_new_ids_and_keeps_settings() {
        let mut p = payload();
        // An SSH secret on a profile without SSH is dropped.
        p.connections[1].secrets.ssh_passphrase = Some("orphan".into());
        let chosen = [p.connections[0].profile.id, p.connections[1].profile.id];
        let planned = plan_import(&p, &chosen);
        assert_eq!(planned.len(), 2);
        let (first, secrets) = &planned[0];
        let original = &p.connections[0].profile;
        assert_ne!(first.id, original.id);
        assert_eq!(first.sort_order, 0);
        assert_eq!(first.last_connected_at, None);
        assert_eq!(first.created_at, "");
        assert_eq!(first.name, original.name);
        assert_eq!(first.group, original.group);
        assert_eq!(first.color, original.color);
        assert_eq!(first.ssh, original.ssh);
        assert_eq!(first.options, original.options);
        assert_eq!(first.options.safe_mode, Some(SafeModeLevel::AlertNonSelect));
        assert_eq!(secrets.password.as_deref(), Some("db-pw"));
        assert_eq!(planned[1].1, ExportedSecrets::default());

        assert_eq!(plan_import(&p, &[chosen[1]]).len(), 1);
        assert!(plan_import(&p, &[]).is_empty());
    }

    #[test]
    fn groups_merge_without_duplicates() {
        let known = vec!["Local".to_string(), "Staging".to_string()];
        let extra = vec![" Staging ".to_string(), "".to_string(), "Prod".to_string()];
        assert_eq!(merge_groups(&known, &extra), ["Local", "Staging", "Prod"]);
    }
}
