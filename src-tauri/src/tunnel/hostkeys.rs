//! SSH host-key trust. A server key is trusted when `~/.ssh/known_hosts` or the app's
//! `ssh_trusted_hosts` table lists it for that host and port. Unknown keys are parked as a
//! [`PendingHostKey`] until the user confirms the fingerprint; changed keys are a hard error.
//!
//! Host names are compared the way OpenSSH does: lowercased (ASCII) before lookup, pinning and
//! appending, so `Bastion.Example.com` and `bastion.example.com` share one entry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::Engine as _;
use rusqlite::params;
use russh::keys::{HashAlg, PublicKey, PublicKeyBase64};
use uuid::Uuid;

use crate::appdb::AppDb;
use crate::error::AppError;

/// A host key the user has not trusted yet, kept until they accept or reject it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingHostKey {
    /// Normalized (lowercase) host name.
    pub host: String,
    pub port: u16,
    /// Key algorithm, e.g. `ssh-ed25519`.
    pub key_type: String,
    /// Base64 key blob as written in `known_hosts` (no algorithm prefix, no comment).
    pub public_key: String,
    /// `SHA256:…`, the same format `ssh-keygen -lf` prints.
    pub fingerprint: String,
}

impl PendingHostKey {
    pub fn from_key(host: &str, port: u16, key: &PublicKey) -> PendingHostKey {
        PendingHostKey {
            host: normalize_host(host),
            port,
            key_type: key_type(key),
            public_key: key.public_key_base64(),
            fingerprint: fingerprint(key),
        }
    }

    /// The error the renderer turns into a fingerprint prompt (`changed == false`) or a warning.
    pub fn to_error(&self, changed: bool) -> AppError {
        AppError::HostKey {
            host: self.host.clone(),
            port: self.port,
            key_type: self.key_type.clone(),
            fingerprint: self.fingerprint.clone(),
            changed,
        }
    }

    pub fn parse_key(&self) -> Result<PublicKey, AppError> {
        russh::keys::parse_public_key_base64(&self.public_key)
            .map_err(|e| AppError::internal(format!("invalid stored host key: {e}")))
    }
}

/// SHA-256 fingerprint in OpenSSH notation (`SHA256:<unpadded base64>`).
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

pub fn key_type(key: &PublicKey) -> String {
    key.algorithm().as_str().to_string()
}

/// The host name as OpenSSH matches it against `known_hosts`: trimmed and ASCII-lowercased.
pub fn normalize_host(host: &str) -> String {
    host.trim().to_ascii_lowercase()
}

/// Outcome of looking a server key up in a trust store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyMatch {
    /// The store lists exactly this key for the host.
    Trusted,
    /// The store knows nothing about this host and key type.
    Unknown,
    /// The store lists a different key of the same type: possible man-in-the-middle.
    Changed,
    /// `known_hosts` marks this very key `@revoked`.
    Revoked,
}

/// Where the tunnel asks whether a server key may be trusted. The app uses [`AppHostKeys`];
/// tests can inject [`MemoryHostKeys`]. `host` is already normalized ([`normalize_host`]).
pub trait HostKeyTrust: Send + Sync {
    fn lookup(&self, host: &str, port: u16, key: &PublicKey) -> Result<HostKeyMatch, AppError>;
    /// Called with an unknown key so that a later "accept" can trust exactly that key.
    fn remember_pending(&self, pending: PendingHostKey);
}

/// Accepts a trusted key; returns the `HostKey` error (after recording the pending key when it is
/// unknown) otherwise. Changed and revoked keys are never parked, so they cannot be accepted.
pub fn verify_host_key(
    trust: &dyn HostKeyTrust,
    host: &str,
    port: u16,
    key: &PublicKey,
) -> Result<(), AppError> {
    let host = normalize_host(host);
    match trust.lookup(&host, port, key)? {
        HostKeyMatch::Trusted => Ok(()),
        found @ (HostKeyMatch::Changed | HostKeyMatch::Revoked) => {
            let pending = PendingHostKey::from_key(&host, port, key);
            log::warn!(
                "ssh host key for {host}:{port} {} ({} {})",
                if found == HostKeyMatch::Revoked {
                    "is revoked in known_hosts"
                } else {
                    "changed"
                },
                pending.key_type,
                pending.fingerprint
            );
            Err(pending.to_error(true))
        }
        HostKeyMatch::Unknown => {
            let pending = PendingHostKey::from_key(&host, port, key);
            let err = pending.to_error(false);
            trust.remember_pending(pending);
            Err(err)
        }
    }
}

/// `$HOME/.ssh/known_hosts`, when a home directory is known.
pub fn default_known_hosts_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".ssh").join("known_hosts"))
}

/// The name a host is listed under in `known_hosts`: `host`, or `[host]:port` for non-22 ports.
fn known_hosts_name(host: &str, port: u16) -> String {
    let host = normalize_host(host);
    if port == 22 {
        host
    } else {
        format!("[{host}]:{port}")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Marker {
    None,
    CertAuthority,
    Revoked,
}

/// Checks an OpenSSH `known_hosts` file the way OpenSSH reads it: fields split on any run of
/// spaces/tabs, `[host]:port` names for non-22 ports, case-insensitive names with `*`/`?`
/// wildcards and `!` negation, hashed (`|1|salt|hash`) names, `@revoked` and `@cert-authority`
/// markers. A line that cannot be parsed is skipped without hiding the other lines.
///
/// Fails closed: an existing file that cannot be read, or an unreadable entry of the presented
/// key's type for this host with no readable entry that trusts the key, is an error (never
/// "unknown", which the user could accept). A missing file is "unknown".
pub fn check_known_hosts_file(
    path: &Path,
    host: &str,
    port: u16,
    key: &PublicKey,
) -> Result<HostKeyMatch, AppError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HostKeyMatch::Unknown),
        Err(e) => {
            return Err(AppError::Tunnel(format!(
                "cannot read {} to check the SSH host key: {e}",
                path.display()
            )));
        }
    };
    let name = known_hosts_name(host, port);
    let presented_type = key_type(key);
    let mut trusted = false;
    let mut changed = false;
    let mut revoked = false;
    let mut unreadable: Option<usize> = None;

    for (index, raw) in bytes.split(|b| *b == b'\n').enumerate() {
        let line_no = index + 1;
        let line = String::from_utf8_lossy(raw);
        let mut fields = line.split_ascii_whitespace();
        let Some(first) = fields.next() else {
            continue;
        };
        if first.starts_with('#') {
            continue;
        }
        let (marker, hosts) = match first.strip_prefix('@') {
            Some(m) => {
                let marker = match m {
                    "cert-authority" => Marker::CertAuthority,
                    "revoked" => Marker::Revoked,
                    _ => {
                        log::warn!(
                            "{}:{line_no}: skipping a line with the unknown marker @{m}",
                            path.display()
                        );
                        continue;
                    }
                };
                match fields.next() {
                    Some(h) => (marker, h),
                    None => continue,
                }
            }
            None => (Marker::None, first),
        };
        if !hosts_match(hosts, &name) {
            continue;
        }
        // CA keys sign host certificates; they never match a plain host key.
        if marker == Marker::CertAuthority {
            continue;
        }
        let line_type = fields.next();
        let parsed = match fields.next() {
            Some(blob) => russh::keys::parse_public_key_base64(blob).map_err(|e| e.to_string()),
            None => Err("no key on the line".to_string()),
        };
        match parsed {
            Ok(recorded) => {
                let same_key = recorded.key_data() == key.key_data();
                if marker == Marker::Revoked {
                    revoked |= same_key;
                } else if same_key {
                    trusted = true;
                } else if recorded.algorithm() == key.algorithm() {
                    changed = true;
                }
            }
            Err(e) => {
                log::warn!(
                    "{}:{line_no}: skipping an unreadable entry for {name}: {e}",
                    path.display()
                );
                if line_type.is_none_or(|t| t == presented_type) {
                    unreadable.get_or_insert(line_no);
                }
            }
        }
    }

    if revoked {
        return Ok(HostKeyMatch::Revoked);
    }
    if trusted {
        return Ok(HostKeyMatch::Trusted);
    }
    if changed {
        return Ok(HostKeyMatch::Changed);
    }
    if let Some(line_no) = unreadable {
        return Err(AppError::Tunnel(format!(
            "{}:{line_no} has an entry for {name} that cannot be read, so the server's \
             {presented_type} key cannot be checked; fix or remove that line and connect again",
            path.display()
        )));
    }
    Ok(HostKeyMatch::Unknown)
}

/// Whether a `known_hosts` host field matches `name` (normalized, `[host]:port` for non-22
/// ports): comma-separated patterns, case-insensitive, `*`/`?` wildcards, a matching `!pattern`
/// excludes the line, `|1|salt|hash` entries are HMAC-SHA1 hashed names.
fn hosts_match(field: &str, name: &str) -> bool {
    let mut matched = false;
    for pattern in field.split(',').filter(|p| !p.is_empty()) {
        if let Some(hashed) = pattern.strip_prefix("|1|") {
            matched |= hashed_name_matches(hashed, name);
            continue;
        }
        let (negated, pattern) = match pattern.strip_prefix('!') {
            Some(p) => (true, p),
            None => (false, pattern),
        };
        if glob_match(pattern.to_ascii_lowercase().as_bytes(), name.as_bytes()) {
            if negated {
                return false;
            }
            matched = true;
        }
    }
    matched
}

/// `salt|hash` (both base64) of a hashed `known_hosts` name: `hash == HMAC-SHA1(salt, name)`.
fn hashed_name_matches(hashed: &str, name: &str) -> bool {
    let b64 = base64::engine::general_purpose::STANDARD;
    let Some((salt, hash)) = hashed.split_once('|') else {
        return false;
    };
    match (b64.decode(salt), b64.decode(hash)) {
        (Ok(salt), Ok(hash)) => hmac_sha1(&salt, name.as_bytes())[..] == hash[..],
        _ => false,
    }
}

/// OpenSSH `match_pattern`: `*` matches any run of characters, `?` exactly one.
fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some(b'*') => {
                star = Some((p, t));
                p += 1;
            }
            Some(&c) if c == b'?' || c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match star {
                Some((sp, st)) => {
                    p = sp + 1;
                    t = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p.min(pattern.len())..].iter().all(|c| *c == b'*')
}

/// SHA-1 (FIPS 180-4), only for matching hashed `known_hosts` names.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    let (blocks, _) = msg.as_chunks::<64>();
    for block in blocks {
        let mut w = [0u32; 80];
        let (words, _) = block.as_chunks::<4>();
        for (slot, word) in w.iter_mut().zip(words) {
            *slot = u32::from_be_bytes(*word);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (hv, v) in h.iter_mut().zip([a, b, c, d, e]) {
            *hv = hv.wrapping_add(v);
        }
    }
    let mut out = [0u8; 20];
    let (chunks, _) = out.as_chunks_mut::<4>();
    for (chunk, v) in chunks.iter_mut().zip(h) {
        *chunk = v.to_be_bytes();
    }
    out
}

/// HMAC-SHA1 (RFC 2104).
fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    let mut block = [0u8; 64];
    if key.len() > block.len() {
        block[..20].copy_from_slice(&sha1(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = block.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(data);
    let mut outer: Vec<u8> = block.iter().map(|b| b ^ 0x5c).collect();
    outer.extend_from_slice(&sha1(&inner));
    sha1(&outer)
}

/// Looks the key up in the app's `ssh_trusted_hosts` table (host compared case-insensitively,
/// so rows saved before names were normalized still count).
pub fn lookup_trusted_host(
    appdb: &AppDb,
    host: &str,
    port: u16,
    key: &PublicKey,
) -> Result<HostKeyMatch, AppError> {
    let host = normalize_host(host);
    let kind = key_type(key);
    let stored: Vec<String> = appdb.with_conn(|c| {
        let mut stmt = c.prepare(
            "SELECT public_key FROM ssh_trusted_hosts
             WHERE lower(host) = ?1 AND port = ?2 AND key_type = ?3",
        )?;
        let rows = stmt.query_map(params![host, port, kind], |r| r.get(0))?;
        rows.collect()
    })?;
    if stored.is_empty() {
        return Ok(HostKeyMatch::Unknown);
    }
    let matches = stored.iter().any(|blob| {
        russh::keys::parse_public_key_base64(blob).is_ok_and(|k| k.key_data() == key.key_data())
    });
    Ok(if matches {
        HostKeyMatch::Trusted
    } else {
        HostKeyMatch::Changed
    })
}

/// Appends `host key` to an OpenSSH `known_hosts` file (`[host]:port` for non-22 ports, host
/// lowercased), creating the file (and a private `~/.ssh`) when needed.
pub fn append_known_host(path: &Path, entry: &PendingHostKey) -> Result<(), AppError> {
    use std::io::{Read, Seek, SeekFrom, Write};

    let io_err = |e: std::io::Error| AppError::Io(format!("cannot update {}: {e}", path.display()));
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
        && !parent.exists()
    {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(parent).map_err(io_err)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(path)
        .map_err(io_err)?;
    let mut needs_newline = false;
    if file.metadata().map_err(io_err)?.len() > 0 {
        let mut last = [0u8; 1];
        file.seek(SeekFrom::End(-1)).map_err(io_err)?;
        file.read_exact(&mut last).map_err(io_err)?;
        needs_newline = last[0] != b'\n';
    }
    let line = format!(
        "{}{} {} {}\n",
        if needs_newline { "\n" } else { "" },
        known_hosts_name(&entry.host, entry.port),
        entry.key_type,
        entry.public_key
    );
    file.write_all(line.as_bytes()).map_err(io_err)
}

/// Stores a key in `ssh_trusted_hosts` under the normalized host (replacing an older key of the
/// same type, whatever case its host was saved with).
pub fn trust_host_key(appdb: &AppDb, entry: &PendingHostKey) -> Result<(), AppError> {
    let host = normalize_host(&entry.host);
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    appdb.with_conn(|c| {
        let tx = c.transaction()?;
        tx.execute(
            "DELETE FROM ssh_trusted_hosts
             WHERE lower(host) = ?1 AND host <> ?1 AND port = ?2 AND key_type = ?3",
            params![host, entry.port, entry.key_type],
        )?;
        tx.execute(
            "INSERT INTO ssh_trusted_hosts (host, port, key_type, public_key, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(host, port, key_type) DO UPDATE SET
               public_key = excluded.public_key, added_at = excluded.added_at",
            params![host, entry.port, entry.key_type, entry.public_key, now],
        )?;
        tx.commit()
    })?;
    Ok(())
}

/// The app's trust policy: `known_hosts` first, then `ssh_trusted_hosts`; unknown keys are parked
/// in `pending` under the profile id until `accept_pending_host_key`.
pub struct AppHostKeys<'a> {
    appdb: &'a AppDb,
    pending: &'a Mutex<HashMap<Uuid, PendingHostKey>>,
    profile_id: Uuid,
    known_hosts: Option<PathBuf>,
}

impl<'a> AppHostKeys<'a> {
    pub fn new(
        appdb: &'a AppDb,
        pending: &'a Mutex<HashMap<Uuid, PendingHostKey>>,
        profile_id: Uuid,
    ) -> AppHostKeys<'a> {
        AppHostKeys {
            appdb,
            pending,
            profile_id,
            known_hosts: default_known_hosts_path(),
        }
    }

    pub fn for_state(state: &'a crate::state::AppState, profile_id: Uuid) -> AppHostKeys<'a> {
        AppHostKeys::new(&state.appdb, &state.pending_host_keys, profile_id)
    }

    /// Reads another `known_hosts` file (or none); tests use this to stay away from `~/.ssh`.
    pub fn with_known_hosts(mut self, path: Option<PathBuf>) -> AppHostKeys<'a> {
        self.known_hosts = path;
        self
    }
}

impl HostKeyTrust for AppHostKeys<'_> {
    fn lookup(&self, host: &str, port: u16, key: &PublicKey) -> Result<HostKeyMatch, AppError> {
        if let Some(path) = &self.known_hosts {
            match check_known_hosts_file(path, host, port, key)? {
                HostKeyMatch::Unknown => {}
                found => return Ok(found),
            }
        }
        lookup_trusted_host(self.appdb, host, port, key)
    }

    fn remember_pending(&self, pending: PendingHostKey) {
        lock(self.pending).insert(self.profile_id, pending);
    }
}

/// Trusts the key parked for `profile_id` after the user confirmed `fingerprint`: writes it to
/// `ssh_trusted_hosts` and, when `known_hosts` is given, appends it to that file too.
pub fn accept_pending_host_key(
    appdb: &AppDb,
    pending: &Mutex<HashMap<Uuid, PendingHostKey>>,
    profile_id: Uuid,
    fingerprint: &str,
    known_hosts: Option<&Path>,
) -> Result<PendingHostKey, AppError> {
    let entry = lock(pending).get(&profile_id).cloned().ok_or_else(|| {
        AppError::NotFound("no SSH host key is waiting for confirmation; connect again".into())
    })?;
    if entry.fingerprint != fingerprint.trim() {
        return Err(AppError::InvalidInput(format!(
            "fingerprint {} does not match the server's key {}; connect again",
            fingerprint.trim(),
            entry.fingerprint
        )));
    }
    if let Some(path) = known_hosts {
        append_known_host(path, &entry)?;
    }
    trust_host_key(appdb, &entry)?;
    let mut map = lock(pending);
    if map.get(&profile_id) == Some(&entry) {
        map.remove(&profile_id);
    }
    log::info!(
        "trusted ssh host key {} {} for {}:{}",
        entry.key_type,
        entry.fingerprint,
        entry.host,
        entry.port
    );
    Ok(entry)
}

/// In-memory trust store for tests and previews.
#[derive(Debug, Default)]
pub struct MemoryHostKeys {
    trusted: Mutex<Vec<PendingHostKey>>,
    pending: Mutex<Option<PendingHostKey>>,
}

impl MemoryHostKeys {
    pub fn new() -> MemoryHostKeys {
        MemoryHostKeys::default()
    }

    pub fn trust(&self, entry: PendingHostKey) {
        let mut trusted = lock(&self.trusted);
        trusted.retain(|t| {
            !(t.host.eq_ignore_ascii_case(&entry.host)
                && t.port == entry.port
                && t.key_type == entry.key_type)
        });
        trusted.push(entry);
    }

    pub fn pending(&self) -> Option<PendingHostKey> {
        lock(&self.pending).clone()
    }

    /// Trusts the pending key, if any, and returns it.
    pub fn accept_pending(&self) -> Option<PendingHostKey> {
        let entry = lock(&self.pending).take()?;
        self.trust(entry.clone());
        Some(entry)
    }
}

impl HostKeyTrust for MemoryHostKeys {
    fn lookup(&self, host: &str, port: u16, key: &PublicKey) -> Result<HostKeyMatch, AppError> {
        let kind = key_type(key);
        let blob = key.public_key_base64();
        let trusted = lock(&self.trusted);
        Ok(
            match trusted.iter().find(|t| {
                t.host.eq_ignore_ascii_case(host.trim()) && t.port == port && t.key_type == kind
            }) {
                None => HostKeyMatch::Unknown,
                Some(t) if t.public_key == blob => HostKeyMatch::Trusted,
                Some(_) => HostKeyMatch::Changed,
            },
        )
    }

    fn remember_pending(&self, pending: PendingHostKey) {
        *lock(&self.pending) = Some(pending);
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> PublicKey {
        let pair = russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[seed; 32]);
        russh::keys::PrivateKey::from(pair).public_key().clone()
    }

    fn line(host: &str, k: &PublicKey) -> String {
        format!("{host} {} {}\n", key_type(k), k.public_key_base64())
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn app_store_flags_unknown_then_trusts_after_accept() {
        let appdb = AppDb::open_in_memory().unwrap();
        let pending = Mutex::new(HashMap::new());
        let profile = Uuid::new_v4();
        let dir = tempfile::tempdir().unwrap();
        let kh = dir.path().join("known_hosts");
        let trust = AppHostKeys::new(&appdb, &pending, profile).with_known_hosts(Some(kh.clone()));
        let k = key(1);

        let err = verify_host_key(&trust, "db.example", 2222, &k).unwrap_err();
        let AppError::HostKey {
            fingerprint: fp,
            changed,
            key_type,
            ..
        } = err
        else {
            panic!("expected HostKey, got {err:?}");
        };
        assert!(!changed);
        assert_eq!(key_type, "ssh-ed25519");
        assert!(fp.starts_with("SHA256:"), "{fp}");
        assert_eq!(pending.lock().unwrap()[&profile].fingerprint, fp);

        let wrong = accept_pending_host_key(&appdb, &pending, profile, "SHA256:nope", None);
        assert!(matches!(wrong, Err(AppError::InvalidInput(_))));

        accept_pending_host_key(&appdb, &pending, profile, &fp, Some(&kh)).unwrap();
        assert!(pending.lock().unwrap().is_empty());
        verify_host_key(&trust, "db.example", 2222, &k).unwrap();
        let text = std::fs::read_to_string(&kh).unwrap();
        assert!(text.starts_with("[db.example]:2222 ssh-ed25519 "), "{text}");

        // known_hosts alone is enough; another key of the same type is "changed" in both stores.
        let other_db = AppDb::open_in_memory().unwrap();
        let kh_only =
            AppHostKeys::new(&other_db, &pending, profile).with_known_hosts(Some(kh.clone()));
        verify_host_key(&kh_only, "db.example", 2222, &k).unwrap();
        let changed_kh = verify_host_key(&kh_only, "db.example", 2222, &key(2)).unwrap_err();
        assert!(matches!(
            changed_kh,
            AppError::HostKey { changed: true, .. }
        ));
        let db_only = AppHostKeys::new(&appdb, &pending, profile).with_known_hosts(None);
        let changed_db = verify_host_key(&db_only, "db.example", 2222, &key(2)).unwrap_err();
        assert!(matches!(
            changed_db,
            AppError::HostKey { changed: true, .. }
        ));
        // Same host on another port is a different entry.
        let other_port = verify_host_key(&db_only, "db.example", 22, &k).unwrap_err();
        assert!(matches!(
            other_port,
            AppError::HostKey { changed: false, .. }
        ));
    }

    #[test]
    fn appends_and_reads_known_hosts_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ssh").join("known_hosts");
        let k = key(5);
        append_known_host(&path, &PendingHostKey::from_key("example.org", 22, &k)).unwrap();
        // A hand-edited file without a trailing newline.
        let other = PendingHostKey::from_key("other.host", 22, &key(6));
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str(&format!(
            "other.host {} {}",
            other.key_type, other.public_key
        ));
        std::fs::write(&path, text).unwrap();
        append_known_host(&path, &PendingHostKey::from_key("Example.ORG", 2200, &k)).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(lines[0].starts_with("example.org ssh-ed25519 "));
        assert!(lines[2].starts_with("[example.org]:2200 ssh-ed25519 "));
        let check = |host: &str, port: u16| check_known_hosts_file(&path, host, port, &k).unwrap();
        assert_eq!(check("example.org", 22), HostKeyMatch::Trusted);
        assert_eq!(check("example.org", 2200), HostKeyMatch::Trusted);
        assert_eq!(check("other.host", 22), HostKeyMatch::Changed);
        assert_eq!(check("nobody.example", 22), HostKeyMatch::Unknown);
        let missing = dir.path().join("nope");
        assert_eq!(
            check_known_hosts_file(&missing, "example.org", 22, &k).unwrap(),
            HostKeyMatch::Unknown
        );
    }

    /// Regression: one unparsable matching line used to turn the whole lookup into "unknown",
    /// so a changed key was offered for acceptance instead of being refused.
    #[test]
    fn malformed_known_hosts_lines_do_not_hide_a_changed_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let real = key(1);
        let attacker = key(2);
        let blob = real.public_key_base64();
        let text = format!(
            "# comment\n\n{}db.example.com  ssh-ed25519 {blob}\n\
             db.example.com\tssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQC\n\
             db.example.com ssh-ed25519\n\
             @bogus db.example.com ssh-ed25519 {blob}\n",
            line("db.example.com", &real)
        );
        std::fs::write(&path, text).unwrap();
        let check = |k: &PublicKey| check_known_hosts_file(&path, "db.example.com", 22, k);
        assert_eq!(check(&attacker).unwrap(), HostKeyMatch::Changed);
        assert_eq!(check(&real).unwrap(), HostKeyMatch::Trusted);

        // Through the app store: a hard "changed" error, nothing parked for acceptance.
        let appdb = AppDb::open_in_memory().unwrap();
        let pending = Mutex::new(HashMap::new());
        let profile = Uuid::new_v4();
        let trust = AppHostKeys::new(&appdb, &pending, profile).with_known_hosts(Some(path));
        let err = verify_host_key(&trust, "db.example.com", 22, &attacker).unwrap_err();
        assert!(
            matches!(err, AppError::HostKey { changed: true, .. }),
            "{err:?}"
        );
        assert!(pending.lock().unwrap().is_empty());
        verify_host_key(&trust, "db.example.com", 22, &real).unwrap();
    }

    #[test]
    fn unreadable_entry_of_the_presented_type_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let k = key(3);
        std::fs::write(
            &path,
            "db.example.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3a\n\
             db.example.com ssh-rsa not-base64!\n",
        )
        .unwrap();
        // Same type, cannot be read: refuse rather than call the key unknown.
        let err = check_known_hosts_file(&path, "db.example.com", 22, &k).unwrap_err();
        assert!(
            matches!(&err, AppError::Tunnel(m) if m.contains(":1 ") && m.contains("db.example.com")),
            "{err:?}"
        );
        let appdb = AppDb::open_in_memory().unwrap();
        let pending = Mutex::new(HashMap::new());
        let trust =
            AppHostKeys::new(&appdb, &pending, Uuid::new_v4()).with_known_hosts(Some(path.clone()));
        assert!(matches!(
            verify_host_key(&trust, "db.example.com", 22, &k),
            Err(AppError::Tunnel(_))
        ));
        assert!(pending.lock().unwrap().is_empty());
        // Another host is unaffected, and a bad line of another type does not block this one.
        assert_eq!(
            check_known_hosts_file(&path, "other.example.com", 22, &k).unwrap(),
            HostKeyMatch::Unknown
        );
        std::fs::write(&path, "db.example.com ssh-rsa not-base64!\n").unwrap();
        assert_eq!(
            check_known_hosts_file(&path, "db.example.com", 22, &k).unwrap(),
            HostKeyMatch::Unknown
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_known_hosts_file_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        // A directory where the file should be: exists but cannot be read as a file.
        let path = dir.path().join("known_hosts");
        std::fs::create_dir(&path).unwrap();
        assert!(matches!(
            check_known_hosts_file(&path, "db.example.com", 22, &key(1)),
            Err(AppError::Tunnel(_))
        ));
    }

    /// Regression: host names were compared case-sensitively, so an OpenSSH pin for
    /// `bastion.example.com` did not apply to `Bastion.Example.com`.
    #[test]
    fn host_names_are_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let real = key(1);
        let attacker = key(2);
        std::fs::write(&path, line("[bastion.example.com]:2222", &real)).unwrap();
        let check =
            |host: &str, k: &PublicKey| check_known_hosts_file(&path, host, 2222, k).unwrap();
        assert_eq!(
            check("Bastion.Example.com", &attacker),
            HostKeyMatch::Changed
        );
        assert_eq!(check("BASTION.EXAMPLE.COM", &real), HostKeyMatch::Trusted);
        // Patterns written in upper case match too (OpenSSH lowercases both sides).
        std::fs::write(&path, line("[Bastion.Example.COM]:2222", &real)).unwrap();
        assert_eq!(
            check("bastion.example.com", &attacker),
            HostKeyMatch::Changed
        );

        let appdb = AppDb::open_in_memory().unwrap();
        let pending = Mutex::new(HashMap::new());
        let profile = Uuid::new_v4();
        let trust = AppHostKeys::new(&appdb, &pending, profile).with_known_hosts(Some(path));
        let err = verify_host_key(&trust, "Bastion.Example.com", 2222, &attacker).unwrap_err();
        assert!(
            matches!(err, AppError::HostKey { changed: true, .. }),
            "{err:?}"
        );
        assert!(pending.lock().unwrap().is_empty());

        // The app table: pins are stored lowercase and looked up case-insensitively, including
        // rows saved with another case before names were normalized.
        let db_trust = AppHostKeys::new(&appdb, &pending, profile).with_known_hosts(None);
        trust_host_key(&appdb, &PendingHostKey::from_key("DB.Example", 22, &real)).unwrap();
        let stored: String = appdb
            .with_conn(|c| c.query_row("SELECT host FROM ssh_trusted_hosts", [], |r| r.get(0)))
            .unwrap();
        assert_eq!(stored, "db.example");
        verify_host_key(&db_trust, "db.EXAMPLE", 22, &real).unwrap();
        assert!(matches!(
            verify_host_key(&db_trust, "DB.example", 22, &attacker),
            Err(AppError::HostKey { changed: true, .. })
        ));
        appdb
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO ssh_trusted_hosts VALUES ('Legacy.Host', 22, ?1, ?2, '')",
                    params![key_type(&real), real.public_key_base64()],
                )
            })
            .unwrap();
        assert!(matches!(
            verify_host_key(&db_trust, "legacy.host", 22, &attacker),
            Err(AppError::HostKey { changed: true, .. })
        ));
        // Re-trusting replaces the legacy-case row instead of adding a second one.
        trust_host_key(
            &appdb,
            &PendingHostKey::from_key("legacy.host", 22, &attacker),
        )
        .unwrap();
        let hosts: Vec<String> = appdb
            .with_conn(|c| {
                let mut s = c.prepare(
                    "SELECT host FROM ssh_trusted_hosts WHERE lower(host) = 'legacy.host'",
                )?;
                s.query_map([], |r| r.get(0))?.collect()
            })
            .unwrap();
        assert_eq!(hosts, vec!["legacy.host".to_string()]);
        verify_host_key(&db_trust, "Legacy.Host", 22, &attacker).unwrap();

        // The pending key carries the normalized host, so accepting it pins the lowercase name.
        let unknown = verify_host_key(&db_trust, "New.Host", 22, &real).unwrap_err();
        assert!(
            matches!(&unknown, AppError::HostKey { host, changed: false, .. } if host == "new.host")
        );
    }

    #[test]
    fn known_hosts_patterns_hashes_and_markers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let real = key(1);
        let other = key(2);
        let check = |text: &str, host: &str, port: u16, k: &PublicKey| {
            std::fs::write(&path, text).unwrap();
            check_known_hosts_file(&path, host, port, k).unwrap()
        };
        // Comma lists and wildcards.
        let text = line("a.example,*.db.example,[g?.example]:2200", &real);
        assert_eq!(check(&text, "a.example", 22, &real), HostKeyMatch::Trusted);
        assert_eq!(
            check(&text, "x.y.DB.example", 22, &other),
            HostKeyMatch::Changed
        );
        assert_eq!(
            check(&text, "g1.example", 2200, &real),
            HostKeyMatch::Trusted
        );
        assert_eq!(
            check(&text, "g12.example", 2200, &real),
            HostKeyMatch::Unknown
        );
        assert_eq!(
            check(&text, "a.example", 2200, &real),
            HostKeyMatch::Unknown
        );
        // Negation excludes the whole line.
        let text = line("*.example,!secret.example", &real);
        assert_eq!(
            check(&text, "secret.example", 22, &other),
            HostKeyMatch::Unknown
        );
        assert_eq!(
            check(&text, "public.example", 22, &other),
            HostKeyMatch::Changed
        );
        // Hashed names (`ssh-keygen -H`): russh's own test vector, and one computed with Python's
        // hmac module for a non-22 port.
        let text = line(
            "|1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak=",
            &real,
        );
        assert_eq!(
            check(&text, "Example.com", 22, &other),
            HostKeyMatch::Changed
        );
        assert_eq!(
            check(&text, "example.org", 22, &other),
            HostKeyMatch::Unknown
        );
        let text = line(
            "|1|AQIDBAUGBwgJCgsMDQ4PEBESExQ=|wiRo5QMg73h4zgxj6KF+j+riWXE=",
            &real,
        );
        assert_eq!(
            check(&text, "bastion.example.com", 2222, &real),
            HostKeyMatch::Trusted
        );
        assert_eq!(
            check(&text, "BASTION.example.com", 2222, &other),
            HostKeyMatch::Changed
        );
        // @revoked refuses that key even when another line trusts it; @cert-authority is not a pin.
        let text = format!(
            "{}@revoked * {} {}\n",
            line("h.example", &real),
            key_type(&real),
            real.public_key_base64()
        );
        assert_eq!(check(&text, "h.example", 22, &real), HostKeyMatch::Revoked);
        let text = format!(
            "@cert-authority h.example {} {}\n",
            key_type(&real),
            real.public_key_base64()
        );
        assert_eq!(check(&text, "h.example", 22, &other), HostKeyMatch::Unknown);
        assert_eq!(check(&text, "h.example", 22, &real), HostKeyMatch::Unknown);
        // A revoked key is a hard error, never parked.
        let text = format!(
            "@revoked h.example {} {}\n",
            key_type(&real),
            real.public_key_base64()
        );
        std::fs::write(&path, text).unwrap();
        let appdb = AppDb::open_in_memory().unwrap();
        let pending = Mutex::new(HashMap::new());
        let trust =
            AppHostKeys::new(&appdb, &pending, Uuid::new_v4()).with_known_hosts(Some(path.clone()));
        assert!(matches!(
            verify_host_key(&trust, "h.example", 22, &real),
            Err(AppError::HostKey { changed: true, .. })
        ));
        assert!(pending.lock().unwrap().is_empty());
    }

    #[test]
    fn glob_matching() {
        let m = |p: &str, t: &str| glob_match(p.as_bytes(), t.as_bytes());
        assert!(m("*", ""));
        assert!(m("*", "anything"));
        assert!(m("a*b*c", "aXXbYYc"));
        assert!(m("a*b*c", "abc"));
        assert!(!m("a*b*c", "abX"));
        assert!(m("?.x", "a.x"));
        assert!(!m("?.x", ".x"));
        assert!(m("**.x", "a.b.x"));
        assert!(!m("", "a"));
        assert!(m("", ""));
    }

    #[test]
    fn sha1_and_hmac_vectors() {
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        // RFC 2202 test cases 1, 2 and 6.
        assert_eq!(
            hex(&hmac_sha1(&[0x0b; 20], b"Hi There")),
            "b617318655057264e28bc0b6fb378c8ef146be00"
        );
        assert_eq!(
            hex(&hmac_sha1(b"Jefe", b"what do ya want for nothing?")),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
        );
        assert_eq!(
            hex(&hmac_sha1(
                &[0xaa; 80],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "aa4ae5e15272d00e95705637ce8a3b55ed402112"
        );
    }

    #[test]
    fn accept_without_pending_key_is_not_found() {
        let appdb = AppDb::open_in_memory().unwrap();
        let pending = Mutex::new(HashMap::new());
        let r = accept_pending_host_key(&appdb, &pending, Uuid::new_v4(), "SHA256:x", None);
        assert!(matches!(r, Err(AppError::NotFound(_))));
    }

    #[test]
    fn memory_store_round_trip() {
        let trust = MemoryHostKeys::new();
        let k = key(3);
        assert!(verify_host_key(&trust, "h", 22, &k).is_err());
        let accepted = trust.accept_pending().unwrap();
        assert_eq!(accepted.fingerprint, fingerprint(&k));
        verify_host_key(&trust, "h", 22, &k).unwrap();
        verify_host_key(&trust, "H", 22, &k).unwrap();
        assert!(matches!(
            verify_host_key(&trust, "h", 22, &key(4)),
            Err(AppError::HostKey { changed: true, .. })
        ));
    }
}
