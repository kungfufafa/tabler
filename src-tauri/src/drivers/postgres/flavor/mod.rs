//! PostgreSQL-compatible engines served by the PostgreSQL driver (CockroachDB, Greenplum,
//! Redshift). The engine-specific code lives in `imp`, built with the pg-flavors lane's feature.

#[cfg(feature = "drv-pg-flavors")]
pub mod imp;
