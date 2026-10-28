//! PostgreSQL-compatible engines served by the PostgreSQL driver (CockroachDB, Greenplum,
//! Redshift). The engine-specific code lives in `imp`, built with the pg-flavors lane's feature.
//!
//! Until that code takes a flavor over, a PostgreSQL session on one runs the plain PostgreSQL
//! code paths, so it offers less (`adjust_caps`), and a Redshift server, whose SQL the core
//! generators do not write, is refused (`refuse_unsupported`).

#[cfg(feature = "drv-pg-flavors")]
pub mod imp;

use crate::error::AppError;
use crate::sqlgen::SqlDialect;
use crate::types::caps::{Feature, SessionCaps};
use crate::types::driver::{Flavor, KindId, PgFlavor};

/// The PostgreSQL-compatible engine a server's `version()` names, if it is not PostgreSQL.
pub fn detect(version: &str) -> Option<PgFlavor> {
    if version.contains("CockroachDB") {
        Some(PgFlavor::Cockroachdb)
    } else if version.contains("Greenplum Database") || version.contains("Cloudberry") {
        Some(PgFlavor::Greenplum)
    } else if version.contains("Redshift") {
        Some(PgFlavor::Redshift)
    } else {
        None
    }
}

/// The session flavor of a PostgreSQL-compatible engine.
pub fn session_flavor(flavor: PgFlavor) -> Flavor {
    match flavor {
        PgFlavor::Cockroachdb => Flavor::Cockroachdb,
        PgFlavor::Greenplum => Flavor::Greenplum,
        PgFlavor::Redshift => Flavor::Redshift,
    }
}

/// What the PostgreSQL code paths get wrong on the flavors (CockroachDB runs DDL in a
/// transaction non-atomically, Greenplum 6 lacks what the structure planner writes, the dump
/// tools, roles and process ids differ), withheld until the pg-flavors lane grants them again.
pub const UNPROVEN_ON_FLAVORS: [Feature; 5] = [
    Feature::StructureEdit,
    Feature::Backup,
    Feature::Restore,
    Feature::Users,
    Feature::KillProcess,
];

/// The caps of a PostgreSQL session as the plain PostgreSQL code gives them: a session on a
/// flavor (set in the profile or found at connect) loses `UNPROVEN_ON_FLAVORS`.
pub fn adjust_caps(caps: SessionCaps) -> SessionCaps {
    if caps.flavor.is_some() {
        caps.without(&UNPROVEN_ON_FLAVORS)
    } else {
        caps
    }
}

/// Refuses a session on a Redshift server that does not speak Redshift SQL: the PostgreSQL
/// generators would run there (`::text` becomes VARCHAR(256), no RETURNING or ON CONFLICT).
pub fn refuse_unsupported(found: Option<PgFlavor>, dialect: SqlDialect) -> Result<(), AppError> {
    match found {
        Some(PgFlavor::Redshift) if dialect != SqlDialect::Redshift => {
            Err(AppError::Unsupported(format!(
                "{} is not built into this Tabler",
                KindId::Redshift.descriptor().label
            )))
        }
        Some(PgFlavor::Redshift | PgFlavor::Cockroachdb | PgFlavor::Greenplum) | None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlgen::Dialect;
    use crate::types::profile::Driver;

    #[test]
    fn detects_the_flavor_from_the_version() {
        for (version, flavor) in [
            (
                "PostgreSQL 17.2 on x86_64-pc-linux-gnu, compiled by gcc",
                None,
            ),
            (
                "CockroachDB CCL v24.2.4 (x86_64-pc-linux-gnu, built 2024/10/14, go1.22.5)",
                Some(PgFlavor::Cockroachdb),
            ),
            (
                "PostgreSQL 12.12 (Greenplum Database 7.1.0 build commit:e7c2b1f) on x86_64",
                Some(PgFlavor::Greenplum),
            ),
            (
                "PostgreSQL 14.4 (Apache Cloudberry 2.0.0-incubating build 1)",
                Some(PgFlavor::Greenplum),
            ),
            (
                "PostgreSQL 8.0.2 on i686-pc-linux-gnu, compiled by GCC gcc (GCC) 3.4.2 20041017 \
                 (Red Hat 3.4.2-6.fc3), Redshift 1.0.77467",
                Some(PgFlavor::Redshift),
            ),
        ] {
            assert_eq!(detect(version), flavor, "{version}");
        }
    }

    #[test]
    fn a_flavor_loses_what_the_postgres_code_gets_wrong() {
        let plain = SessionCaps::core(Driver::Postgres, Dialect::Postgres, None);
        assert_eq!(adjust_caps(plain.clone()), plain);
        for flavor in [PgFlavor::Cockroachdb, PgFlavor::Greenplum] {
            let caps = adjust_caps(SessionCaps::base(
                Driver::Postgres,
                SqlDialect::Postgres,
                Some(session_flavor(flavor)),
            ));
            for f in UNPROVEN_ON_FLAVORS {
                assert!(!caps.has(f), "{flavor:?} {f:?}");
            }
            assert!(caps.has(Feature::EditRows) && caps.has(Feature::ExportTables));
            let name = KindId::of(Driver::Postgres, Some(flavor), None)
                .descriptor()
                .label;
            assert_eq!(
                caps.require(Feature::Users).unwrap_err().to_string(),
                format!("unsupported: {name}: user management is not available")
            );
        }
    }

    #[test]
    fn a_redshift_server_needs_redshift_sql() {
        let refused = refuse_unsupported(Some(PgFlavor::Redshift), SqlDialect::Postgres)
            .unwrap_err()
            .to_string();
        assert_eq!(
            refused,
            "unsupported: Amazon Redshift is not built into this Tabler"
        );
        refuse_unsupported(Some(PgFlavor::Redshift), SqlDialect::Redshift).unwrap();
        for flavor in [None, Some(PgFlavor::Cockroachdb), Some(PgFlavor::Greenplum)] {
            refuse_unsupported(flavor, SqlDialect::Postgres).unwrap();
        }
    }
}
