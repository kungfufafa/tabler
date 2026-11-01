//! CockroachDB, Greenplum and Redshift specifics (the pg-flavors lane): each engine's
//! `PgFlavorHooks`, which `super::hooks` picks for the engine the server names at connect.
//!
//! - CockroachDB (`cockroach`): the PostgreSQL catalog with its gaps filled from
//!   `crdb_internal` and `information_schema` (hidden `rowid` keys, type modifiers, row counts),
//!   sessions and kills by `CANCEL`, `SHOW CREATE`'s DDL, and transactions run again on 40001.
//! - Greenplum and its forks (`greenplum`): catalog SQL chosen by `server_version_num` (Greenplum
//!   6 is PostgreSQL 9.4, 7 is 12, Apache Cloudberry 14), legacy partition children hidden,
//!   distribution and storage shown read-only and kept in the DDL.
//! - Redshift (`redshift`): leader-node catalog SQL without what PostgreSQL 8.0.2 lacks, the
//!   `svv_` views queried apart from `pg_catalog`, late-binding views, SUPER / GEOMETRY /
//!   HLLSKETCH columns, `SHOW TABLE`'s DDL, provisioned and Serverless sessions, and Redshift SQL.

mod catalog;
mod cockroach;
mod greenplum;
mod redshift;
#[cfg(test)]
mod testkit;

use super::PgFlavorHooks;
use crate::types::driver::PgFlavor;

/// The hooks of `flavor` (`super::hooks`).
pub(crate) fn hooks(flavor: PgFlavor) -> Option<&'static dyn PgFlavorHooks> {
    Some(match flavor {
        PgFlavor::Cockroachdb => &cockroach::COCKROACH,
        PgFlavor::Greenplum => &greenplum::GREENPLUM,
        PgFlavor::Redshift => &redshift::REDSHIFT,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::postgres::flavor;

    #[test]
    fn every_engine_has_its_hooks() {
        for f in [
            PgFlavor::Cockroachdb,
            PgFlavor::Greenplum,
            PgFlavor::Redshift,
        ] {
            assert_eq!(hooks(f).and_then(|h| h.flavor()), Some(f));
            assert_eq!(flavor::hooks(Some(f)).flavor(), Some(f));
        }
        assert_eq!(flavor::hooks(None).flavor(), None);
    }

    /// The caps a session gets on each engine and version, as `PgSession::capabilities` builds
    /// them, kept in `tests/fixtures/pg_flavors/caps.json`: the renderer's mock tests read the
    /// same file, so the mock's caps cannot drift from these. `TABLER_BLESS=1` rewrites it.
    #[test]
    fn caps_match_the_recorded_fixture() {
        use crate::types::caps::SessionCaps;
        use crate::types::profile::Driver;

        let engines = [
            ("cockroachdb", PgFlavor::Cockroachdb, 130_000),
            ("greenplum6", PgFlavor::Greenplum, 90_426),
            ("greenplum7", PgFlavor::Greenplum, 120_012),
            ("cloudberry", PgFlavor::Greenplum, 140_004),
            ("redshift", PgFlavor::Redshift, 80_002),
        ];
        let mut all = serde_json::Map::new();
        for (name, engine, version) in engines {
            let hooks = flavor::hooks(Some(engine));
            let mut caps = flavor::adjust_caps(SessionCaps::base(
                Driver::Postgres,
                hooks.sql_dialect(),
                Some(flavor::session_flavor(engine)),
            ));
            hooks.adjust_caps(&mut caps, version);
            all.insert(name.into(), serde_json::to_value(&caps).unwrap());
        }
        let got = serde_json::Value::Object(all);
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/pg_flavors/caps.json");
        if std::env::var("TABLER_BLESS").as_deref() == Ok("1") {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let text = serde_json::to_string_pretty(&got).unwrap();
            std::fs::write(&path, text + "\n").unwrap();
            return;
        }
        let want: serde_json::Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        assert!(
            got == want,
            "{} differs (TABLER_BLESS=1 rewrites it); now:\n{}",
            path.display(),
            serde_json::to_string_pretty(&got).unwrap()
        );
    }
}
