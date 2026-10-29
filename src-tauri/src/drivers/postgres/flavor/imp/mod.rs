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
}
