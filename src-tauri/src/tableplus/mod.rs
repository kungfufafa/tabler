//! TablePlus's connection exports (`.tableplusconnection`, "Export Connections ▸ …" in TablePlus
//! for macOS and for Windows), read as a connections [`Payload`] so "Import Connections…" (and
//! "Import from TablePlus…") preview and import them like a Tabler file.
//!
//! The file is RNCryptor v3 (see [`rncryptor`]) under the password the user set on export (an
//! empty one when they set none). Inside is UTF-8 JSON: an array whose items are connections
//! (TablePlus's own connection record, see [`convert`]) or groups, `{ Name, IsExpanded, image,
//! connections: […], groups: […] }`, nested to any depth; `image` is the group picture as hex.
//! Older exports, and TablePlus's own `Connections.plist` / `ConnectionGroups.plist` model, keep
//! groups apart: `{ ConnectionGroups: [{ ID, Name, GroupID }], Connections: [{ …, GroupID }] }`;
//! that shape reads too.
//!
//! Connections carry no id in the file, so each gets one derived from its place and content:
//! the preview and the import, which read the file twice, agree on it.
//!
//! Pure: no Tauri, app DB or Keychain access.

mod convert;
pub mod rncryptor;

use std::collections::HashMap;

use aws_lc_rs::digest;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::error::AppError;
use crate::profiles_io::{self, GROUP_SEPARATOR, GroupRecord, MAX_IMAGE_LEN, Payload, ReadOutcome};
use crate::types::profile::SkippedConnection;

pub use convert::Converted;

/// Deepest group nesting followed (a file nesting deeper is cut off there, not overflowing the
/// stack).
const MAX_DEPTH: usize = 32;

/// Whether `bytes` are a TablePlus connections export (by content: password-based RNCryptor).
pub fn is_export(bytes: &[u8]) -> bool {
    rncryptor::looks_like(bytes)
}

fn not_an_export() -> AppError {
    AppError::InvalidInput("This is not a TablePlus connections export".into())
}

fn wrong_password() -> AppError {
    AppError::InvalidInput("The file password is wrong, or the file was modified".into())
}

/// Reads a TablePlus export. Without a `password` (or with an empty one) the empty password is
/// tried, which opens a file exported without one; otherwise the outcome asks for it.
pub fn read(bytes: &[u8], password: Option<&str>) -> Result<ReadOutcome, AppError> {
    let plain = match password.filter(|p| !p.is_empty()) {
        Some(password) => rncryptor::decrypt(bytes, password).map_err(|e| match e {
            rncryptor::Error::BadPassword => wrong_password(),
            rncryptor::Error::NotRncryptor => not_an_export(),
        })?,
        None => match rncryptor::decrypt(bytes, "") {
            Ok(plain) => plain,
            Err(rncryptor::Error::BadPassword) => return Ok(ReadOutcome::NeedsPassword),
            Err(rncryptor::Error::NotRncryptor) => return Err(not_an_export()),
        },
    };
    let json: Value = serde_json::from_slice(&plain).map_err(|_| not_an_export())?;
    Ok(ReadOutcome::Opened {
        payload: parse(&json)?,
        encrypted: true,
    })
}

/// The connections and groups of a decrypted export.
pub fn parse(json: &Value) -> Result<Payload, AppError> {
    let mut out = Payload::default();
    match json {
        Value::Array(items) => walk(items, "", "", 0, &mut out),
        Value::Object(o) if o.get("Connections").is_some_and(Value::is_array) => flat(o, &mut out),
        _ => return Err(not_an_export()),
    }
    Ok(out)
}

/// A group record of the export (`{ Name, image, connections, groups }`): TablePlus tells it
/// from a connection by its `connections` key.
fn is_group(o: &Map<String, Value>) -> bool {
    o.contains_key("connections")
}

fn text<'a>(o: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    o.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// `parent / name`, in canonical form ("" when both are blank).
fn child_path(parent: &str, name: &str) -> String {
    profiles_io::normalize_group_path(&format!("{parent}{GROUP_SEPARATOR}{name}"))
}

/// Walks the export's tree: `items` are the connections and groups inside `group` ("" at the
/// top). `place` locates them in the file (for the derived ids).
fn walk(items: &[Value], group: &str, place: &str, depth: usize, out: &mut Payload) {
    for (i, item) in items.iter().enumerate() {
        let Value::Object(o) = item else { continue };
        let here = format!("{place}/{i}");
        if is_group(o) {
            if depth >= MAX_DEPTH {
                continue;
            }
            let path = child_path(group, text(o, "Name").unwrap_or("Group"));
            add_group(out, &path, image(o.get("image")));
            for key in ["connections", "groups"] {
                if let Some(Value::Array(inner)) = o.get(key) {
                    walk(inner, &path, &format!("{here}{key}"), depth + 1, out);
                }
            }
        } else {
            add_connection(o, group, &here, out);
        }
    }
}

/// The `{ ConnectionGroups, Connections }` shape: groups point at their parent by `GroupID`,
/// connections at their group.
fn flat(o: &Map<String, Value>, out: &mut Payload) {
    let groups: Vec<&Map<String, Value>> = o
        .get("ConnectionGroups")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default();
    let by_id: HashMap<&str, &Map<String, Value>> = groups
        .iter()
        .filter_map(|g| Some((text(g, "ID")?, *g)))
        .collect();
    let path_of = |id: Option<&str>| -> String {
        let mut names: Vec<&str> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        let mut next = id.map(str::to_string);
        // A parent cycle in a damaged file ends at the first group met again.
        while let Some(gid) = next.take() {
            let Some(g) = by_id.get(gid.as_str()) else {
                break;
            };
            if seen.contains(&gid) || names.len() >= MAX_DEPTH {
                break;
            }
            names.push(text(g, "Name").unwrap_or("Group"));
            next = text(g, "GroupID").map(str::to_string);
            seen.push(gid);
        }
        names
            .iter()
            .rev()
            .fold(String::new(), |p, n| child_path(&p, n))
    };
    for g in &groups {
        add_group(out, &path_of(text(g, "ID")), image(g.get("image")));
    }
    for (i, c) in o["Connections"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        if let Value::Object(c) = c {
            add_connection(c, &path_of(text(c, "GroupID")), &format!("/flat/{i}"), out);
        }
    }
}

fn add_group(out: &mut Payload, path: &str, image: Option<String>) {
    if path.is_empty() || out.groups.iter().any(|g| g.path == path) {
        return;
    }
    out.groups.push(GroupRecord {
        path: path.to_string(),
        icon: None,
        image,
    });
}

/// A group picture: TablePlus writes its bytes as hex. Kept as a `data:` URI when it is a PNG,
/// JPEG or GIF that fits [`MAX_IMAGE_LEN`]; anything else is dropped (the group keeps the
/// plain folder icon).
fn image(raw: Option<&Value>) -> Option<String> {
    let hex = raw?.as_str()?.trim();
    // Base64 is 4/3 of the bytes, hex twice them: a hex string past this cannot fit.
    if hex.is_empty() || hex.len() % 2 != 0 || hex.len() > MAX_IMAGE_LEN * 3 / 2 {
        return None;
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else {
        return None;
    };
    profiles_io::image_uri(Some(&format!("data:{mime};base64,{}", B64.encode(&bytes))))
}

/// The id of a connection of the file: derived from where it is and what it holds, the same on
/// every read of the same file.
fn derived_id(place: &str, o: &Map<String, Value>) -> Uuid {
    let mut ctx = digest::Context::new(&digest::SHA256);
    ctx.update(b"tableplus-connection\0");
    ctx.update(place.as_bytes());
    ctx.update(b"\0");
    ctx.update(serde_json::to_string(o).unwrap_or_default().as_bytes());
    let hash = ctx.finish();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash.as_ref()[..16]);
    uuid::Builder::from_random_bytes(bytes).into_uuid()
}

fn add_connection(o: &Map<String, Value>, group: &str, place: &str, out: &mut Payload) {
    let id = derived_id(place, o);
    match convert::connection(o, id, group) {
        Ok(Converted {
            connection,
            notes,
            ssh_key,
        }) => {
            if !notes.is_empty() {
                out.notes.insert(id, notes);
            }
            if let Some(key) = ssh_key {
                out.ssh_keys.insert(id, key);
            }
            out.connections.push(connection);
        }
        Err(reason) => out.skipped.push(SkippedConnection {
            name: text(o, "ConnectionName").unwrap_or_default().to_string(),
            reason,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::profile::{Driver, SslMode};

    /// Written by TablePlus for Windows's own code (see `tests/fixtures/tableplus/README.md`).
    const EXPORT: &[u8] =
        include_bytes!("../../tests/fixtures/tableplus/windows-export.tableplusconnection");
    const NO_PASSWORD: &[u8] = include_bytes!(
        "../../tests/fixtures/tableplus/windows-export-empty-password.tableplusconnection"
    );

    fn opened(outcome: ReadOutcome) -> Payload {
        match outcome {
            ReadOutcome::Opened { payload, encrypted } => {
                assert!(encrypted);
                payload
            }
            ReadOutcome::NeedsPassword => panic!("still locked"),
        }
    }

    #[test]
    fn reads_a_tableplus_export() {
        assert!(is_export(EXPORT));
        assert_eq!(read(EXPORT, None).unwrap(), ReadOutcome::NeedsPassword);
        assert_eq!(read(EXPORT, Some("")).unwrap(), ReadOutcome::NeedsPassword);
        let err = read(EXPORT, Some("wrong")).unwrap_err();
        assert!(err.to_string().contains("password is wrong"), "{err}");

        let p = opened(read(EXPORT, Some("tabler-test")).unwrap());
        let paths: Vec<&str> = p.groups.iter().map(|g| g.path.as_str()).collect();
        assert_eq!(paths, ["Clients", "Clients / Reports"]);
        // The 29-byte "image" is a PNG signature and header: kept as a data URI.
        assert!(
            p.groups[0]
                .image
                .as_deref()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
        assert_eq!(p.groups[1].image, None);

        let by_name = |n: &str| {
            p.connections
                .iter()
                .find(|c| c.profile.name == n)
                .unwrap_or_else(|| panic!("{n}"))
        };
        let pg = by_name("Orders (prod)");
        assert_eq!(pg.profile.driver, Driver::Postgres);
        assert_eq!(pg.profile.group.as_deref(), Some("Clients"));
        assert_eq!(pg.profile.ssl, SslMode::Require);
        assert_eq!(pg.profile.ssh.as_ref().unwrap().port, 2222);
        assert_eq!(pg.secrets.password.as_deref(), Some("pg-s3cret"));
        assert_eq!(pg.secrets.ssh_password.as_deref(), Some("ssh-s3cret"));
        let lite = by_name("Sample file");
        assert_eq!(lite.profile.group.as_deref(), Some("Clients / Reports"));
        assert_eq!(
            lite.profile.sqlite_path.as_deref(),
            Some(r"C:\data\sample.sqlite")
        );
        let shop = by_name("Shop");
        assert_eq!(
            (shop.profile.group.as_deref(), shop.profile.driver),
            (None, Driver::Mysql)
        );
        assert!(p.skipped.is_empty());

        // The preview and the import read the file apart: the ids must agree.
        let again = opened(read(EXPORT, Some("tabler-test")).unwrap());
        let ids = |p: &Payload| {
            p.connections
                .iter()
                .map(|c| c.profile.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&p), ids(&again));
        let unique: std::collections::HashSet<_> = ids(&p).into_iter().collect();
        assert_eq!(unique.len(), 3);
    }

    #[test]
    fn an_export_without_a_password_opens_at_once() {
        let p = opened(read(NO_PASSWORD, None).unwrap());
        assert_eq!(p.connections.len(), 3);
    }

    #[test]
    fn tabler_files_are_not_tableplus_exports() {
        assert!(!is_export(
            b"{\"format\":\"tabler-connections\",\"version\":2}"
        ));
    }

    #[test]
    fn unsupported_and_flat_shapes() {
        let json = serde_json::json!({
            "ConnectionGroups": [
                {"ID": "a", "Name": "Prod"},
                {"ID": "b", "Name": "EU", "GroupID": "a"},
                {"ID": "c", "Name": "Loop", "GroupID": "c"}
            ],
            "Connections": [
                {"ConnectionName": "eu-db", "Driver": "PostgreSQL", "GroupID": "b"},
                {"ConnectionName": "erp", "Driver": "Oracle"}
            ]
        });
        let p = parse(&json).unwrap();
        assert_eq!(p.connections[0].profile.group.as_deref(), Some("Prod / EU"));
        assert_eq!(p.skipped.len(), 1);
        assert_eq!(p.skipped[0].name, "erp");
        assert!(parse(&serde_json::json!("nope")).is_err());
    }

    #[test]
    fn group_images() {
        assert_eq!(image(Some(&Value::from("zz"))), None);
        assert_eq!(image(Some(&Value::from("0102"))), None, "not a picture");
        assert!(
            image(Some(&Value::from("FFD8FF00")))
                .unwrap()
                .starts_with("data:image/jpeg")
        );
    }

    #[test]
    fn deep_nesting_is_cut_off() {
        let mut v = serde_json::json!({"Driver": "MySQL", "ConnectionName": "deep"});
        for _ in 0..100 {
            v = serde_json::json!({"Name": "g", "connections": [], "groups": [v]});
        }
        let p = parse(&Value::Array(vec![v])).unwrap();
        assert!(p.connections.is_empty());
    }
}
