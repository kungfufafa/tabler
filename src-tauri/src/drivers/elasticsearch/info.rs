//! The cluster behind a connection, from `GET /`: Elasticsearch or OpenSearch, its version and
//! build flavor, and from those the endpoints and request features Tabler may use
//! ([`Features`]): the point-in-time API, `_shard_doc`, `if_seq_no`, the SQL endpoint, typed
//! document paths before 7.0.

use crate::error::AppError;

use super::json::Json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distribution {
    Elasticsearch,
    OpenSearch,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// `8.15.2`, `2.17.0`, `7.10.2-SNAPSHOT`, `9.0.0-beta1`; missing parts are 0.
    pub fn parse(text: &str) -> Option<Version> {
        let core = text.trim().split(['-', '+']).next()?;
        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next().map_or(Some(0), |p| p.parse().ok())?;
        let patch = parts.next().map_or(Some(0), |p| p.parse().ok())?;
        Some(Version {
            major,
            minor,
            patch,
        })
    }

    pub fn at_least(&self, major: u32, minor: u32) -> bool {
        (self.major, self.minor) >= (major, minor)
    }
}

/// What `GET /` says about the cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterInfo {
    /// The node that answered.
    pub node_name: Option<String>,
    pub cluster_name: String,
    pub cluster_uuid: Option<String>,
    pub distribution: Distribution,
    /// As the server wrote it.
    pub version_text: String,
    pub version: Version,
    /// `default`, `oss`, `serverless` (Elasticsearch).
    pub build_flavor: Option<String>,
    pub lucene: Option<String>,
}

impl ClusterInfo {
    pub fn parse(reply: &Json) -> Result<ClusterInfo, AppError> {
        let version = reply.get("version").ok_or_else(|| {
            AppError::Connect(
                "the server did not answer like Elasticsearch or OpenSearch (no version in `GET /`)"
                    .into(),
            )
        })?;
        let number = version
            .get("number")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string();
        let distribution = match version.get("distribution").and_then(Json::as_str) {
            Some(d) if d.eq_ignore_ascii_case("opensearch") => Distribution::OpenSearch,
            _ => Distribution::Elasticsearch,
        };
        let build_flavor = version
            .get("build_flavor")
            .and_then(Json::as_str)
            .map(str::to_string);
        let serverless = build_flavor.as_deref() == Some("serverless");
        // Serverless reports a fixed `8.11.0`; its API is the latest one.
        let parsed = Version::parse(&number).filter(|_| !serverless);
        let version_value = match (parsed, serverless) {
            (Some(v), _) => v,
            (None, true) => Version {
                major: u32::MAX,
                minor: 0,
                patch: 0,
            },
            (None, false) => {
                return Err(AppError::Connect(format!(
                    "unrecognised server version \u{201c}{number}\u{201d} in `GET /`"
                )));
            }
        };
        Ok(ClusterInfo {
            node_name: reply.get("name").and_then(Json::as_str).map(str::to_string),
            cluster_name: reply
                .get("cluster_name")
                .and_then(Json::as_str)
                .unwrap_or("elasticsearch")
                .to_string(),
            cluster_uuid: reply
                .get("cluster_uuid")
                .and_then(Json::as_str)
                .filter(|u| *u != "_na_")
                .map(str::to_string),
            distribution,
            version_text: number,
            version: version_value,
            build_flavor,
            lucene: version
                .get("lucene_version")
                .and_then(Json::as_str)
                .map(str::to_string),
        })
    }

    pub fn is_serverless(&self) -> bool {
        self.build_flavor.as_deref() == Some("serverless")
    }

    /// The product and version for the status pill: `Elasticsearch 8.15.2`, `OpenSearch 2.17.0`,
    /// `Elasticsearch Serverless`.
    pub fn display_version(&self) -> String {
        match (self.distribution, self.is_serverless()) {
            (Distribution::OpenSearch, _) => format!("OpenSearch {}", self.version_text),
            (Distribution::Elasticsearch, true) => "Elasticsearch Serverless".into(),
            (Distribution::Elasticsearch, false) if self.build_flavor.as_deref() == Some("oss") => {
                format!("Elasticsearch {} (OSS)", self.version_text)
            }
            (Distribution::Elasticsearch, false) => format!("Elasticsearch {}", self.version_text),
        }
    }

    /// Refuses servers the driver cannot work with: Elasticsearch before 6.0 (no
    /// `seq_no` guards, several types per index).
    pub fn check_supported(&self) -> Result<(), AppError> {
        if self.distribution == Distribution::Elasticsearch && self.version.major < 6 {
            return Err(AppError::Unsupported(format!(
                "Elasticsearch {} is too old: Tabler needs 6.0 or later",
                self.version_text
            )));
        }
        Ok(())
    }

    pub fn features(&self) -> Features {
        let v = &self.version;
        match self.distribution {
            Distribution::Elasticsearch => Features {
                pit: v.at_least(7, 10).then_some(PitApi::Elastic),
                // PIT searches sort on `_shard_doc` last by themselves from 7.12.
                shard_doc: v.at_least(7, 12),
                seq_no_guards: v.at_least(6, 7),
                sql: if self.build_flavor.as_deref() == Some("oss") {
                    None
                } else if v.at_least(7, 0) {
                    Some(SqlApi::Elastic)
                } else if v.at_least(6, 3) {
                    Some(SqlApi::XPack)
                } else {
                    None
                },
                typed_paths: v.major < 7,
                case_insensitive: v.at_least(7, 10),
                data_streams: v.at_least(7, 9),
                create_without_id: v.at_least(7, 5),
                cat: true,
                total_hits_object: v.major >= 7,
            },
            Distribution::OpenSearch => Features {
                pit: v.at_least(2, 4).then_some(PitApi::OpenSearch),
                // Not documented reliably for OpenSearch: deep pages use a scroll there.
                shard_doc: false,
                seq_no_guards: true,
                sql: Some(SqlApi::OpenSearch),
                typed_paths: false,
                case_insensitive: true,
                data_streams: true,
                create_without_id: true,
                cat: true,
                total_hits_object: true,
            },
        }
    }
}

/// The point-in-time API of the cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PitApi {
    /// `POST /<index>/_pit?keep_alive=`, `{"pit": {"id": …}}`, `DELETE /_pit {"id": …}`.
    Elastic,
    /// `POST /<index>/_search/point_in_time?keep_alive=`, `{"pit": {"id": …}}`,
    /// `DELETE /_search/point_in_time {"pit_id": [...]}`.
    OpenSearch,
}

/// The SQL endpoint of the cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlApi {
    /// `/_sql` (7.0+): `{columns, rows, cursor}`.
    Elastic,
    /// `/_xpack/sql` (6.3–6.8), same replies.
    XPack,
    /// `/_plugins/_sql` (jdbc: `{schema, datarows, cursor}`).
    OpenSearch,
}

impl SqlApi {
    pub fn segments(self) -> &'static [&'static str] {
        match self {
            SqlApi::Elastic => &["_sql"],
            SqlApi::XPack => &["_xpack", "sql"],
            SqlApi::OpenSearch => &["_plugins", "_sql"],
        }
    }
}

/// What the cluster's version allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Features {
    pub pit: Option<PitApi>,
    /// Point-in-time searches sort on `_shard_doc` last, so `search_after` pages are exact.
    pub shard_doc: bool,
    /// `seq_no_primary_term` in searches and `if_seq_no` / `if_primary_term` on writes (6.7+).
    pub seq_no_guards: bool,
    pub sql: Option<SqlApi>,
    /// Document paths name the mapping type (`/<index>/<type>/<id>`, before 7.0).
    pub typed_paths: bool,
    /// `case_insensitive` on `term`, `prefix` and `wildcard` (7.10+).
    pub case_insensitive: bool,
    pub data_streams: bool,
    /// A bulk `create` without `_id` (7.5+); before, `index` without `_id`.
    pub create_without_id: bool,
    pub cat: bool,
    /// `hits.total` is `{value, relation}` (7.0+), a number before.
    pub total_hits_object: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::elasticsearch::json::parse;

    fn info(text: &str) -> ClusterInfo {
        ClusterInfo::parse(&parse(text).unwrap()).unwrap()
    }

    #[test]
    fn elasticsearch_8() {
        let i = info(
            r#"{"name":"node-1","cluster_name":"docker-cluster","cluster_uuid":"Xq3","version":{"number":"8.15.2","build_flavor":"default","build_type":"docker","lucene_version":"9.11.1","minimum_wire_compatibility_version":"7.17.0"},"tagline":"You Know, for Search"}"#,
        );
        assert_eq!(i.distribution, Distribution::Elasticsearch);
        assert_eq!(i.display_version(), "Elasticsearch 8.15.2");
        assert_eq!(i.cluster_name, "docker-cluster");
        let f = i.features();
        assert_eq!(f.pit, Some(PitApi::Elastic));
        assert!(f.shard_doc && f.seq_no_guards && !f.typed_paths);
        assert_eq!(f.sql, Some(SqlApi::Elastic));
        assert!(i.check_supported().is_ok());
    }

    #[test]
    fn elasticsearch_9_and_serverless() {
        let i =
            info(r#"{"cluster_name":"c","version":{"number":"9.1.0","build_flavor":"default"}}"#);
        assert!(i.features().shard_doc);
        let s = info(
            r#"{"name":"serverless","cluster_name":"abc","cluster_uuid":"_na_","version":{"number":"8.11.0","build_flavor":"serverless","build_type":"docker"}}"#,
        );
        assert!(s.is_serverless());
        assert_eq!(s.cluster_uuid, None);
        assert_eq!(s.display_version(), "Elasticsearch Serverless");
        assert_eq!(s.features().pit, Some(PitApi::Elastic));
    }

    #[test]
    fn older_elasticsearch() {
        let i =
            info(r#"{"cluster_name":"c","version":{"number":"7.9.3","build_flavor":"default"}}"#);
        let f = i.features();
        assert_eq!(f.pit, None);
        assert!(f.data_streams && !f.case_insensitive);
        let six =
            info(r#"{"cluster_name":"c","version":{"number":"6.8.23","build_flavor":"default"}}"#);
        let f = six.features();
        assert!(f.typed_paths && f.seq_no_guards && !f.total_hits_object);
        assert_eq!(f.sql, Some(SqlApi::XPack));
        let oss =
            info(r#"{"cluster_name":"c","version":{"number":"7.10.2","build_flavor":"oss"}}"#);
        assert_eq!(oss.features().sql, None);
        assert_eq!(oss.display_version(), "Elasticsearch 7.10.2 (OSS)");
        let five = info(r#"{"cluster_name":"c","version":{"number":"5.6.16"}}"#);
        assert!(matches!(
            five.check_supported(),
            Err(AppError::Unsupported(_))
        ));
    }

    #[test]
    fn opensearch() {
        let i = info(
            r#"{"name":"os-1","cluster_name":"opensearch","cluster_uuid":"u","version":{"distribution":"opensearch","number":"2.17.1","build_type":"tar","lucene_version":"9.11.1","minimum_wire_compatibility_version":"7.10.0"},"tagline":"The OpenSearch Project: https://opensearch.org/"}"#,
        );
        assert_eq!(i.distribution, Distribution::OpenSearch);
        assert_eq!(i.display_version(), "OpenSearch 2.17.1");
        let f = i.features();
        assert_eq!(f.pit, Some(PitApi::OpenSearch));
        assert!(!f.shard_doc);
        assert_eq!(f.sql, Some(SqlApi::OpenSearch));
        let old = info(
            r#"{"cluster_name":"c","version":{"distribution":"opensearch","number":"1.3.19"}}"#,
        );
        assert_eq!(old.features().pit, None);
        let three = info(
            r#"{"cluster_name":"c","version":{"distribution":"opensearch","number":"3.0.0"}}"#,
        );
        assert!(three.features().pit.is_some());
    }

    #[test]
    fn versions_parse_leniently_and_fail_cleanly() {
        assert_eq!(
            Version::parse("9.0.0-beta1"),
            Some(Version {
                major: 9,
                minor: 0,
                patch: 0
            })
        );
        assert_eq!(Version::parse("7").map(|v| v.minor), Some(0));
        assert_eq!(Version::parse("x.y"), None);
        assert!(ClusterInfo::parse(&parse(r#"{"ok":true}"#).unwrap()).is_err());
        assert!(ClusterInfo::parse(&parse(r#"{"version":{"number":"abc"}}"#).unwrap()).is_err());
    }
}
