//! What a server says about itself: `INFO` sections, the keyspace (`db0:keys=12,expires=3,…`),
//! the product and version behind the Redis protocol (Valkey, Dragonfly, Garnet, … all answer
//! `redis_version` with the Redis version they match) and the gates that follow from it, and the
//! databases as the sidebar lists them. Managed services refuse `CONFIG` and parts of `INFO`
//! (ElastiCache, Upstash; TablePlus #3304): every reader here has a fallback.

use crate::sqlgen::native::redis::Gates;
use crate::types::schema::{DatabaseInfo, SchemaInfo, TableKind, TableSummary};

use super::values::{as_pairs, as_text};

type Resp = ::redis::Value;

/// How many databases the sidebar lists at most, whatever `databases` is set to.
pub const MAX_DATABASES: u32 = 4096;
/// The count when the server does not say (Redis's default).
pub const DEFAULT_DATABASES: u32 = 16;

/// An `INFO` reply: `# Section` headers and `key:value` lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    pub sections: Vec<(String, Vec<(String, String)>)>,
}

impl Info {
    pub fn parse(text: &str) -> Info {
        let mut sections: Vec<(String, Vec<(String, String)>)> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('#') {
                sections.push((name.trim().to_string(), Vec::new()));
                continue;
            }
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            if sections.is_empty() {
                sections.push((String::new(), Vec::new()));
            }
            if let Some((_, fields)) = sections.last_mut() {
                fields.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
        Info { sections }
    }

    /// From an `INFO` reply (bulk string, or RESP3 verbatim text); empty for anything else.
    pub fn from_reply(v: &Resp) -> Info {
        as_text(v).map(|t| Info::parse(&t)).unwrap_or_default()
    }

    /// A field from any section.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.sections
            .iter()
            .flat_map(|(_, f)| f)
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn section(&self, name: &str) -> Option<&[(String, String)]> {
        self.sections
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, f)| f.as_slice())
    }
}

/// One `INFO keyspace` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keyspace {
    pub db: u32,
    pub keys: i64,
    pub expires: i64,
    pub avg_ttl: i64,
}

/// The databases that hold keys (`db3:keys=12,expires=3,avg_ttl=0,subexpiry=0`).
pub fn keyspace(info: &Info) -> Vec<Keyspace> {
    let mut out: Vec<Keyspace> = info
        .sections
        .iter()
        .flat_map(|(_, f)| f)
        .filter_map(|(k, v)| {
            let db = parse_db_name(k)?;
            let mut ks = Keyspace {
                db,
                keys: 0,
                expires: 0,
                avg_ttl: 0,
            };
            for part in v.split(',') {
                let Some((name, n)) = part.split_once('=') else {
                    continue;
                };
                let n = n.trim().parse().unwrap_or(0);
                match name.trim() {
                    "keys" => ks.keys = n,
                    "expires" => ks.expires = n,
                    "avg_ttl" => ks.avg_ttl = n,
                    _ => {}
                }
            }
            Some(ks)
        })
        .collect();
    out.sort_by_key(|k| k.db);
    out.dedup_by_key(|k| k.db);
    out
}

/// A `major.minor.patch` version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// Leading numbers of `7.2.4`, `6.0.20-rc1`, `df-v1.21.2`; `None` without a major number.
    pub fn parse(text: &str) -> Option<Version> {
        let start = text.find(|c: char| c.is_ascii_digit())?;
        let mut parts = text[start..].split('.').map(|p| {
            let digits: String = p.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u32>().ok()
        });
        let major = parts.next().flatten()?;
        let minor = parts.next().flatten().unwrap_or(0);
        let patch = parts.next().flatten().unwrap_or(0);
        Some(Version {
            major,
            minor,
            patch,
        })
    }

    pub fn at_least(&self, major: u32, minor: u32) -> bool {
        (self.major, self.minor) >= (major, minor)
    }

    /// The commands a server of this (Redis-compatible) version has.
    pub fn gates(&self) -> Gates {
        Gates {
            keepttl: self.at_least(6, 0),
            unlink: self.at_least(4, 0),
            copy: self.at_least(6, 2),
            scan_type: self.at_least(6, 0),
        }
    }
}

/// The server behind the protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    /// `Redis`, `Valkey`, `Dragonfly`, … (the status pill).
    pub product: String,
    /// The product's own version text.
    pub version: String,
    /// The Redis version it is compatible with (`redis_version`), which decides the gates.
    pub compat: Option<Version>,
    /// `standalone`, `cluster` or `sentinel`.
    pub mode: String,
}

/// Products that report their own `<name>_version` next to `redis_version`.
const PRODUCTS: &[(&str, &str)] = &[
    ("valkey", "Valkey"),
    ("dragonfly", "Dragonfly"),
    ("garnet", "Garnet"),
    ("keydb", "KeyDB"),
    ("memurai", "Memurai"),
];

impl Server {
    pub fn identify(info: &Info) -> Server {
        let redis = info.get("redis_version");
        let own = PRODUCTS.iter().find_map(|(key, label)| {
            info.get(&format!("{key}_version"))
                .map(|v| (label.to_string(), v.to_string()))
        });
        let named = info
            .get("server_name")
            .and_then(|n| PRODUCTS.iter().find(|(k, _)| n.eq_ignore_ascii_case(k)))
            .map(|(_, label)| label.to_string());
        let (product, version) = match (own, named) {
            (Some((label, v)), _) => (label, v),
            (None, Some(label)) => (label, redis.unwrap_or("").to_string()),
            (None, None) => ("Redis".to_string(), redis.unwrap_or("").to_string()),
        };
        Server {
            product,
            version,
            compat: redis.and_then(Version::parse),
            mode: info.get("redis_mode").unwrap_or("standalone").to_string(),
        }
    }

    /// `Valkey 8.0.1`, or the product alone when the server hides its version.
    pub fn label(&self) -> String {
        if self.version.is_empty() {
            self.product.clone()
        } else {
            format!("{} {}", self.product, self.version)
        }
    }

    /// The gates of its compatible version; a server that hides it (managed services restrict
    /// `INFO`) is taken for a current one.
    pub fn gates(&self) -> Gates {
        self.compat.map(|v| v.gates()).unwrap_or_default()
    }
}

/// The `databases` setting from `CONFIG GET databases` (RESP2 pairs or a RESP3 map); `None`
/// when refused or unreadable.
pub fn config_databases(reply: &Resp) -> Option<u32> {
    let pairs = as_pairs(reply)?;
    pairs
        .iter()
        .find(|(k, _)| as_text(k).is_some_and(|k| k.eq_ignore_ascii_case("databases")))
        .and_then(|(_, v)| as_text(v)?.trim().parse::<u32>().ok())
        .filter(|n| *n > 0)
}

/// How many databases to list: the setting, else 16, and at least one past the highest that
/// holds keys; at most `MAX_DATABASES`.
pub fn database_count(config: Option<u32>, keyspace: &[Keyspace]) -> u32 {
    let used = keyspace
        .iter()
        .map(|k| k.db.saturating_add(1))
        .max()
        .unwrap_or(0);
    config
        .unwrap_or(DEFAULT_DATABASES)
        .max(used)
        .clamp(1, MAX_DATABASES)
}

pub fn db_name(index: u32) -> String {
    format!("db{index}")
}

/// `db3` → 3 (the sidebar's names, and `INFO keyspace`'s).
pub fn parse_db_name(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("db")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) || digits.len() > 9 {
        return None;
    }
    digits.parse().ok()
}

/// The databases of the sidebar.
pub fn databases(count: u32, current: u32) -> Vec<DatabaseInfo> {
    (0..count)
        .map(|i| DatabaseInfo {
            name: db_name(i),
            owner: None,
            encoding: None,
            is_current: i == current,
        })
        .collect()
}

/// The one schema of a connection: its current database.
pub fn schemas(current: u32) -> Vec<SchemaInfo> {
    vec![SchemaInfo {
        name: db_name(current),
        is_default: true,
    }]
}

/// Every database as a table of keys (TablePlus lists them without a reconnect), with its key
/// count and how many of them expire.
pub fn tables(count: u32, keyspace: &[Keyspace], schema: &str) -> Vec<TableSummary> {
    (0..count)
        .map(|i| {
            let ks = keyspace.iter().find(|k| k.db == i);
            TableSummary {
                name: db_name(i),
                schema: Some(schema.to_string()),
                kind: TableKind::Table,
                row_estimate: Some(ks.map_or(0, |k| k.keys)),
                size_bytes: None,
                comment: ks
                    .filter(|k| k.expires > 0)
                    .map(|k| format!("{} keys with a TTL", k.expires)),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::redis::parse_redis_value;

    const SERVER: &str = "# Server\r\nredis_version:7.2.4\r\nserver_name:valkey\r\nvalkey_version:8.0.1\r\nredis_mode:standalone\r\nos:Linux 6.1\r\n\r\n# Keyspace\r\ndb0:keys=12,expires=3,avg_ttl=0,subexpiry=0\r\ndb5:keys=1,expires=0,avg_ttl=0\r\n";

    #[test]
    fn info_sections_and_fields() {
        let info = Info::parse(SERVER);
        assert_eq!(info.sections.len(), 2);
        assert_eq!(info.get("os"), Some("Linux 6.1"));
        assert_eq!(info.section("keyspace").map(<[_]>::len), Some(2));
        assert_eq!(info.get("nope"), None);
        // Lines before any header still count; junk lines are skipped.
        let loose = Info::parse("redis_version:5.0.7\nnot a field\n");
        assert_eq!(loose.get("redis_version"), Some("5.0.7"));
        let bulk = parse_redis_value(b"$25\r\n# Server\r\nredis_version:6\r\n").unwrap();
        assert_eq!(Info::from_reply(&bulk).get("redis_version"), Some("6"));
        assert_eq!(Info::from_reply(&::redis::Value::Nil), Info::default());
    }

    #[test]
    fn keyspace_lines() {
        let ks = keyspace(&Info::parse(SERVER));
        assert_eq!(
            ks,
            [
                Keyspace {
                    db: 0,
                    keys: 12,
                    expires: 3,
                    avg_ttl: 0
                },
                Keyspace {
                    db: 5,
                    keys: 1,
                    expires: 0,
                    avg_ttl: 0
                }
            ]
        );
        let odd = keyspace(&Info::parse("db2:keys=x,expires\ndbx:keys=1\ndb:keys=1\n"));
        assert_eq!(odd.len(), 1);
        assert_eq!(odd[0].keys, 0);
    }

    #[test]
    fn products_and_versions() {
        let s = Server::identify(&Info::parse(SERVER));
        assert_eq!(s.label(), "Valkey 8.0.1");
        assert_eq!(s.compat, Version::parse("7.2.4"));
        assert_eq!(s.mode, "standalone");
        assert_eq!(s.gates(), Gates::default());
        let redis5 = Server::identify(&Info::parse("redis_version:5.0.14\nredis_mode:standalone"));
        assert_eq!(redis5.label(), "Redis 5.0.14");
        let g = redis5.gates();
        assert!(!g.keepttl && !g.copy && !g.scan_type && g.unlink);
        let redis62 = Version::parse("6.2.0").unwrap().gates();
        assert!(redis62.keepttl && redis62.copy);
        assert!(!Version::parse("3.2").unwrap().gates().unlink);
        let df = Server::identify(&Info::parse(
            "redis_version:7.4.0\ndragonfly_version:df-v1.21.2",
        ));
        assert_eq!(df.label(), "Dragonfly df-v1.21.2");
        let named = Server::identify(&Info::parse("redis_version:7.2.4\nserver_name:KeyDB"));
        assert_eq!(named.label(), "KeyDB 7.2.4");
        let hidden = Server::identify(&Info::default());
        assert_eq!(hidden.label(), "Redis");
        assert_eq!(hidden.gates(), Gates::default());
        assert_eq!(
            Version::parse("6.0.20-rc1"),
            Some(Version {
                major: 6,
                minor: 0,
                patch: 20
            })
        );
        assert_eq!(Version::parse("unstable"), None);
        assert_eq!(Version::parse("7").map(|v| v.minor), Some(0));
    }

    #[test]
    fn database_counts() {
        let resp2 = parse_redis_value(b"*2\r\n$9\r\ndatabases\r\n$2\r\n32\r\n").unwrap();
        let resp3 = parse_redis_value(b"%1\r\n$9\r\ndatabases\r\n$2\r\n32\r\n").unwrap();
        assert_eq!(config_databases(&resp2), Some(32));
        assert_eq!(config_databases(&resp3), Some(32));
        assert_eq!(
            config_databases(&parse_redis_value(b"*0\r\n").unwrap()),
            None
        );
        assert_eq!(
            config_databases(&parse_redis_value(b"-ERR unknown command 'CONFIG'\r\n").unwrap()),
            None
        );
        assert_eq!(
            config_databases(&parse_redis_value(b"*2\r\n$9\r\ndatabases\r\n$1\r\n0\r\n").unwrap()),
            None
        );
        let ks = keyspace(&Info::parse(SERVER));
        assert_eq!(database_count(Some(32), &ks), 32);
        assert_eq!(database_count(None, &ks), 16);
        assert_eq!(
            database_count(None, &keyspace(&Info::parse("db40:keys=1"))),
            41
        );
        assert_eq!(database_count(Some(u32::MAX), &[]), MAX_DATABASES);
        assert_eq!(database_count(Some(1), &[]), 1);
    }

    #[test]
    fn sidebar() {
        assert_eq!(parse_db_name("db12"), Some(12));
        for bad in ["db", "dbx", "db-1", "x1", "db1234567890"] {
            assert_eq!(parse_db_name(bad), None, "{bad}");
        }
        let dbs = databases(3, 1);
        assert_eq!(
            dbs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["db0", "db1", "db2"]
        );
        assert!(dbs[1].is_current && !dbs[0].is_current);
        assert_eq!(schemas(4)[0].name, "db4");
        let ks = keyspace(&Info::parse(SERVER));
        let t = tables(6, &ks, "db0");
        assert_eq!(t.len(), 6);
        assert_eq!(t[0].row_estimate, Some(12));
        assert_eq!(t[0].comment.as_deref(), Some("3 keys with a TTL"));
        assert_eq!(t[1].row_estimate, Some(0));
        assert_eq!(t[5].comment, None);
        assert_eq!(t[5].schema.as_deref(), Some("db0"));
    }
}
