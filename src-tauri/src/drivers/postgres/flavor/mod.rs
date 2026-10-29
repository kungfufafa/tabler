//! PostgreSQL-compatible engines served by the PostgreSQL driver (CockroachDB, Greenplum,
//! Redshift). The engine-specific code lives in `imp`, built with the pg-flavors lane's feature,
//! and reaches the driver only through `PgFlavorHooks`.
//!
//! Until that code takes a flavor over, a PostgreSQL session on one runs the plain PostgreSQL
//! code paths (`VANILLA`), so it offers less (`adjust_caps`), and a Redshift server, whose SQL the
//! core generators do not write, is refused (`refuse_unsupported`).

#[cfg(feature = "drv-pg-flavors")]
pub mod imp;

use async_trait::async_trait;

use super::introspect::Traced;
use crate::error::AppError;
use crate::sqlgen::SqlDialect;
use crate::sqlgen::dialect::spec::ExplainForm;
use crate::types::admin::{KillOutcome, KillScope, ProcessRow};
use crate::types::caps::{Feature, SessionCaps};
use crate::types::driver::{Flavor, KindId, PgFlavor};
use crate::types::profile::ConnectionProfile;
use crate::types::query::ExplainMode;
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};

/// What a statement of the session init sets. An engine's hooks pick statements by it
/// (`PgFlavorHooks::init_sql`), never by their text, which the profile's own startup commands
/// may share (`SET client_encoding …` among them is the user's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitKind {
    DateStyle,
    IntervalStyle,
    FloatDigits,
    Encoding,
    TimeZone,
    ReadOnly,
    /// A server setting of the profile's extra parameters.
    Setting,
    /// The profile's startup commands.
    Startup,
}

/// A statement of the session init and what it sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitStatement {
    pub kind: InitKind,
    pub sql: String,
}

impl InitStatement {
    pub fn new(kind: InitKind, sql: impl Into<String>) -> InitStatement {
        InitStatement {
            kind,
            sql: sql.into(),
        }
    }
}

/// What the PostgreSQL driver does differently on one engine. Every hook defaults to plain
/// PostgreSQL: a catalog or admin hook answering `None` leaves the work to the driver's own
/// queries. The driver picks the hooks once, at connect, for the engine the server names
/// (`hooks`).
#[async_trait]
pub(crate) trait PgFlavorHooks: Send + Sync {
    /// The engine; `None` for plain PostgreSQL (`VANILLA`), whose admin hooks the session then
    /// leaves to the callers' SQL without taking a connection for them.
    fn flavor(&self) -> Option<PgFlavor>;

    /// The SQL the session speaks (Redshift: `SqlDialect::Redshift`).
    fn sql_dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }

    /// The database a profile without one connects to.
    fn default_database(&self) -> &'static str {
        "postgres"
    }

    /// The statements every new server connection runs, pool and editor alike. `base` is plain
    /// PostgreSQL's (`connect::session_init_statements`), each with what it sets (`InitKind`):
    /// an engine picks by that, never by the text, which the profile's startup commands may share.
    /// `read_only`: the session must refuse writes (the profile says so, or its client encoding is
    /// not UTF-8).
    fn init_sql(
        &self,
        _profile: &ConnectionProfile,
        _read_only: bool,
        base: Vec<InitStatement>,
    ) -> Vec<InitStatement> {
        base
    }

    /// Sends the init statements one at a time instead of as one batch.
    fn init_one_by_one(&self) -> bool {
        false
    }

    async fn list_databases(&self, _c: &Traced<'_>) -> Option<Result<Vec<DatabaseInfo>, AppError>> {
        None
    }

    async fn list_schemas(&self, _c: &Traced<'_>) -> Option<Result<Vec<SchemaInfo>, AppError>> {
        None
    }

    async fn list_tables(
        &self,
        _c: &Traced<'_>,
        _schema: &str,
    ) -> Option<Result<Vec<TableSummary>, AppError>> {
        None
    }

    async fn table_meta(
        &self,
        _c: &Traced<'_>,
        _table: &TableRef,
    ) -> Option<Result<TableMeta, AppError>> {
        None
    }

    async fn view_definition(
        &self,
        _c: &Traced<'_>,
        _table: &TableRef,
    ) -> Option<Result<Option<String>, AppError>> {
        None
    }

    async fn completion_metadata(
        &self,
        _c: &Traced<'_>,
    ) -> Option<Result<CompletionMetadata, AppError>> {
        None
    }

    /// The server's sessions (`DbSession::list_processes`).
    async fn list_processes(&self, _c: &Traced<'_>) -> Option<Result<Vec<ProcessRow>, AppError>> {
        None
    }

    /// Kills processes of `list_processes` (`DbSession::kill_processes`).
    async fn kill(
        &self,
        _c: &Traced<'_>,
        _ids: &[String],
        _scope: KillScope,
    ) -> Option<Vec<KillOutcome>> {
        None
    }

    /// The server's own DDL of a table (`DbSession::table_ddl`).
    async fn table_ddl(
        &self,
        _c: &Traced<'_>,
        _table: &TableRef,
    ) -> Option<Result<String, AppError>> {
        None
    }

    fn explain_form(&self, _mode: ExplainMode) -> Option<ExplainForm> {
        None
    }

    /// Grants back what the engine is proven to do: `caps` comes without `UNPROVEN_ON_FLAVORS`
    /// (`adjust_caps`). `server_version_num` as libpq computes it (`server_version_num`).
    fn adjust_caps(&self, _caps: &mut SessionCaps, _server_version_num: i32) {}

    /// Runs a transaction (`DbSession::transaction`) the server aborted with a serialization
    /// failure (40001) again, as CockroachDB asks its clients to.
    fn retry_serialization(&self) -> bool {
        false
    }

    /// A read the editor may stop at the row cap inside a transaction block runs under a
    /// savepoint, so the cancel rolls back to it and the transaction goes on
    /// (`PgSession::run_on_editor`). An engine without savepoints (Redshift, where a SAVEPOINT
    /// aborts the open transaction) answers false: such a read is then drained, not cancelled.
    fn row_cap_savepoint(&self) -> bool {
        true
    }
}

/// Plain PostgreSQL: every hook as the driver does it.
pub(crate) struct Vanilla;

#[async_trait]
impl PgFlavorHooks for Vanilla {
    fn flavor(&self) -> Option<PgFlavor> {
        None
    }
}

pub(crate) static VANILLA: Vanilla = Vanilla;

/// The hooks of the engine `flavor` (`None`: plain PostgreSQL): the pg-flavors lane's, when this
/// build has them, otherwise `VANILLA`.
pub(crate) fn hooks(flavor: Option<PgFlavor>) -> &'static dyn PgFlavorHooks {
    #[cfg(feature = "drv-pg-flavors")]
    if let Some(hooks) = flavor.and_then(imp::hooks) {
        return hooks;
    }
    #[cfg(not(feature = "drv-pg-flavors"))]
    let _ = flavor;
    &VANILLA
}

/// `server_version_num` as libpq computes it from the `server_version` a server reports at
/// startup (`17.2` → 170002, `9.4.26` → 90426; CockroachDB reports `13.0.0`, Redshift `8.0.2`),
/// without asking the server, which Redshift could not answer; 0 when it is not a version.
pub fn server_version_num(server_version: &str) -> i32 {
    let mut parts = [0i32; 3];
    let mut count = 0;
    let mut rest = server_version;
    while count < parts.len() {
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        // Longer parts are no version (and could overflow below).
        if digits == 0 || digits > 4 {
            break;
        }
        parts[count] = rest[..digits].parse().unwrap_or(0);
        count += 1;
        rest = &rest[digits..];
        match rest.strip_prefix('.') {
            Some(after) => rest = after,
            None => break,
        }
    }
    let [major, minor, patch] = parts;
    match count {
        3 => (100 * major + minor) * 100 + patch,
        2 if major >= 10 => 10_000 * major + minor,
        2 => (100 * major + minor) * 100,
        1 => 10_000 * major,
        _ => 0,
    }
}

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
    fn reads_the_server_version_as_libpq_does() {
        for (version, num) in [
            ("17.2", 170_002),
            ("17.2 (Debian 17.2-1.pgdg120+1)", 170_002),
            ("16beta1", 160_000),
            ("9.6.24", 90_624),
            ("9.6devel", 90_600),
            ("9.4.26", 90_426),
            ("12.12", 120_012),
            ("14.4", 140_004),
            ("13.0.0", 130_000),
            ("8.0.2", 80_002),
            ("", 0),
            ("devel", 0),
            ("123456.1", 0),
        ] {
            assert_eq!(server_version_num(version), num, "{version}");
        }
    }

    /// Without the lane every engine gets `VANILLA`, and `VANILLA` changes nothing: no SQL of its
    /// own, the init it is given, the caps it is given.
    #[tokio::test]
    async fn vanilla_is_plain_postgres() {
        use crate::drivers::SqlTrace;
        use crate::drivers::postgres::testing::FakePg;
        use crate::types::profile::ProfileOptions;

        assert_eq!(hooks(None).flavor(), None);
        for flavor in [
            PgFlavor::Cockroachdb,
            PgFlavor::Greenplum,
            PgFlavor::Redshift,
        ] {
            let picked = hooks(Some(flavor)).flavor();
            // With the lane: its hooks, or VANILLA while it has none for the engine.
            let lane = cfg!(feature = "drv-pg-flavors") && picked == Some(flavor);
            assert!(lane || picked.is_none(), "{flavor:?}: {picked:?}");
        }
        let hooks: &dyn PgFlavorHooks = &VANILLA;
        assert_eq!(hooks.sql_dialect(), SqlDialect::Postgres);
        assert_eq!(hooks.default_database(), "postgres");
        assert!(!hooks.init_one_by_one() && !hooks.retry_serialization());
        assert!(hooks.row_cap_savepoint());
        let base = vec![
            InitStatement::new(InitKind::DateStyle, "SET a = 1;"),
            InitStatement::new(InitKind::Startup, "SET b = 2;"),
        ];
        let profile = crate::types::profile::ConnectionProfile {
            id: uuid::Uuid::nil(),
            name: "p".into(),
            color: None,
            group: None,
            driver: Driver::Postgres,
            host: None,
            port: None,
            user: None,
            database: None,
            ssl: crate::types::profile::SslMode::Disable,
            ssl_ca_path: None,
            sqlite_path: None,
            ssh: None,
            options: ProfileOptions::default(),
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        };
        for read_only in [false, true] {
            assert_eq!(hooks.init_sql(&profile, read_only, base.clone()), base);
        }
        for mode in [ExplainMode::Plan, ExplainMode::Analyze] {
            assert_eq!(hooks.explain_form(mode), None);
        }
        let flavored = adjust_caps(SessionCaps::base(
            Driver::Postgres,
            SqlDialect::Postgres,
            Some(Flavor::Greenplum),
        ));
        let mut caps = flavored.clone();
        hooks.adjust_caps(&mut caps, 90_426);
        assert_eq!(caps, flavored);

        crate::testkit::install_crypto();
        let pg = FakePg::new().start().await;
        let mut config = tokio_postgres::Config::new();
        config
            .host("127.0.0.1")
            .port(pg.port())
            .user("u")
            .password("pw")
            .dbname("d");
        let (client, connection) = config.connect(tokio_postgres::NoTls).await.unwrap();
        tokio::spawn(connection);
        let trace = SqlTrace::default();
        let c = Traced::new(&client, &trace, 170_002);
        let table = TableRef::new(Some("public"), "t");
        assert!(hooks.list_databases(&c).await.is_none());
        assert!(hooks.list_schemas(&c).await.is_none());
        assert!(hooks.list_tables(&c, "public").await.is_none());
        assert!(hooks.table_meta(&c, &table).await.is_none());
        assert!(hooks.view_definition(&c, &table).await.is_none());
        assert!(hooks.completion_metadata(&c).await.is_none());
        assert!(hooks.list_processes(&c).await.is_none());
        let ids = ["7".to_string()];
        assert!(hooks.kill(&c, &ids, KillScope::Query).await.is_none());
        assert!(hooks.table_ddl(&c, &table).await.is_none());
        assert!(pg.queries().is_empty(), "{:?}", pg.queries());
        assert!(trace.drain().is_empty());
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
