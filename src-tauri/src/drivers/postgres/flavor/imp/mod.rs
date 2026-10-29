//! CockroachDB, Greenplum and Redshift specifics (the pg-flavors lane).

use super::PgFlavorHooks;
use crate::types::driver::PgFlavor;

/// The hooks of `flavor` (`super::hooks`); `None` leaves the engine on the PostgreSQL code paths
/// (`super::VANILLA`).
pub(crate) fn hooks(_flavor: PgFlavor) -> Option<&'static dyn PgFlavorHooks> {
    None
}
