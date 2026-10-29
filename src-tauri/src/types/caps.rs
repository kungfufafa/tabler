//! What a connection can do. A session reports its `SessionCaps` (`DbSession::capabilities`);
//! the backend refuses a command whose `Feature` they lack before it sends anything
//! (`require`), and the renderer hides what they lack. The core sessions (PostgreSQL,
//! MySQL / MariaDB, SQLite) get every feature; any other session starts from `minimal` and its
//! driver adds what it implements and proves.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::AppError;
use crate::sqlgen::dialect::DialectRef;
use crate::sqlgen::{Dialect, SqlDialect};
use crate::types::browse::FilterOp;
use crate::types::driver::{Flavor, KindId, PgFlavor, QueryLanguage, ReadOnlyMode};
use crate::types::mutate::ServerExpr;
use crate::types::profile::{ConnectionProfile, Driver, MysqlVersion};

/// A feature of the app that a session may or may not offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Feature {
    Editor,
    Explain,
    ExplainAnalyze,
    CancelOnServer,
    Format,
    EditRows,
    InsertRows,
    DeleteRows,
    EditWithoutKey,
    Returning,
    ServerExpressions,
    ExactCount,
    CopyAsInsert,
    WorkspaceCommit,
    StructureView,
    StructureEdit,
    CreateTable,
    DropTable,
    TruncateTable,
    RenameTable,
    TableDdl,
    ViewDefinition,
    Routines,
    Sequences,
    Triggers,
    Diagram,
    SwitchDatabase,
    CreateDatabase,
    DropDatabase,
    RenameDatabase,
    CreateSchema,
    DropSchema,
    ProcessList,
    KillProcess,
    Users,
    ExportTables,
    ExportSql,
    ImportCsv,
    ImportJson,
    ImportSqlDump,
    Backup,
    Restore,
    Encoding,
    Timezone,
    StartupCommands,
    SessionDetails,
    Metrics,
    Assistant,
    Mcp,
    LaravelMigration,
    DummyData,
}

impl Feature {
    pub const ALL: [Feature; 51] = [
        Feature::Editor,
        Feature::Explain,
        Feature::ExplainAnalyze,
        Feature::CancelOnServer,
        Feature::Format,
        Feature::EditRows,
        Feature::InsertRows,
        Feature::DeleteRows,
        Feature::EditWithoutKey,
        Feature::Returning,
        Feature::ServerExpressions,
        Feature::ExactCount,
        Feature::CopyAsInsert,
        Feature::WorkspaceCommit,
        Feature::StructureView,
        Feature::StructureEdit,
        Feature::CreateTable,
        Feature::DropTable,
        Feature::TruncateTable,
        Feature::RenameTable,
        Feature::TableDdl,
        Feature::ViewDefinition,
        Feature::Routines,
        Feature::Sequences,
        Feature::Triggers,
        Feature::Diagram,
        Feature::SwitchDatabase,
        Feature::CreateDatabase,
        Feature::DropDatabase,
        Feature::RenameDatabase,
        Feature::CreateSchema,
        Feature::DropSchema,
        Feature::ProcessList,
        Feature::KillProcess,
        Feature::Users,
        Feature::ExportTables,
        Feature::ExportSql,
        Feature::ImportCsv,
        Feature::ImportJson,
        Feature::ImportSqlDump,
        Feature::Backup,
        Feature::Restore,
        Feature::Encoding,
        Feature::Timezone,
        Feature::StartupCommands,
        Feature::SessionDetails,
        Feature::Metrics,
        Feature::Assistant,
        Feature::Mcp,
        Feature::LaravelMigration,
        Feature::DummyData,
    ];

    /// The feature's name in a refusal ("DuckDB: editing rows is not available").
    pub fn label(self) -> &'static str {
        match self {
            Feature::Editor => "the query editor",
            Feature::Explain => "Explain",
            Feature::ExplainAnalyze => "Explain Analyze",
            Feature::CancelOnServer => "cancelling on the server",
            Feature::Format => "Format SQL",
            Feature::EditRows => "editing rows",
            Feature::InsertRows => "inserting rows",
            Feature::DeleteRows => "deleting rows",
            Feature::EditWithoutKey => "editing rows without a primary key",
            Feature::Returning => "reading saved rows back",
            Feature::ServerExpressions => "DEFAULT and server expressions",
            Feature::ExactCount => "exact row counts",
            Feature::CopyAsInsert => "Copy as INSERT",
            Feature::WorkspaceCommit => "committing the workspace",
            Feature::StructureView => "the structure view",
            Feature::StructureEdit => "editing the structure",
            Feature::CreateTable => "creating tables",
            Feature::DropTable => "dropping tables",
            Feature::TruncateTable => "truncating tables",
            Feature::RenameTable => "renaming tables",
            Feature::TableDdl => "Show DDL",
            Feature::ViewDefinition => "view definitions",
            Feature::Routines => "functions and procedures",
            Feature::Sequences => "sequences",
            Feature::Triggers => "triggers",
            Feature::Diagram => "the diagram",
            Feature::SwitchDatabase => "switching databases",
            Feature::CreateDatabase => "creating databases",
            Feature::DropDatabase => "dropping databases",
            Feature::RenameDatabase => "renaming databases",
            Feature::CreateSchema => "creating schemas",
            Feature::DropSchema => "dropping schemas",
            Feature::ProcessList => "the process list",
            Feature::KillProcess => "killing processes",
            Feature::Users => "user management",
            Feature::ExportTables => "exporting tables",
            Feature::ExportSql => "SQL export",
            Feature::ImportCsv => "CSV import",
            Feature::ImportJson => "JSON import",
            Feature::ImportSqlDump => "SQL dump import",
            Feature::Backup => "Backup",
            Feature::Restore => "Restore",
            Feature::Encoding => "connection encodings",
            Feature::Timezone => "time zones",
            Feature::StartupCommands => "startup commands",
            Feature::SessionDetails => "session details",
            Feature::Metrics => "Metrics boards",
            Feature::Assistant => "the AI assistant",
            Feature::Mcp => "MCP",
            Feature::LaravelMigration => "Laravel migrations",
            Feature::DummyData => "Dummy Data",
        }
    }
}

/// How a server's namespaces nest under a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SchemaMode {
    /// Databases hold schemas, which hold tables (PostgreSQL).
    Schemas,
    /// A database is the schema (MySQL).
    DatabaseIsSchema,
    /// One namespace only.
    Single,
}

/// What the sidebar calls the levels of the tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NamespaceLabels {
    pub database: String,
    pub schema: String,
    pub table: String,
    pub tables: String,
}

impl Default for NamespaceLabels {
    fn default() -> NamespaceLabels {
        NamespaceLabels {
            database: "Database".into(),
            schema: "Schema".into(),
            table: "Table".into(),
            tables: "Tables".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NamespaceCaps {
    /// The server lists databases one can switch between.
    pub databases: bool,
    pub schemas: SchemaMode,
    pub default_schema: Option<String>,
    pub labels: NamespaceLabels,
}

/// How the grid asks for the next page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Paging {
    Offset,
    Cursor,
}

/// What counting a table's rows costs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CountMode {
    /// Counts are cheap enough to run on every load.
    Exact,
    /// Large tables show the planner's estimate; the exact count runs on request.
    Estimate,
    /// An exact count scans (and may bill) the whole table: only on request.
    Costly,
    None,
}

/// What the grid's raw filter field takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RawFilter {
    SqlWhere,
    Json,
    QueryString,
}

/// How staged changes are applied, and what the commit sheet says about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TxMode {
    /// All or nothing.
    Atomic,
    /// Data atomic; structure commits immediately.
    DataOnly,
    /// Sent as one batch that the server applies in full (or partly, on command errors).
    Batch,
    /// Applied one by one; the failed changes stay staged.
    Sequential,
    /// One change at a time.
    None,
}

/// Whether a write reports how many rows it affected, which the duplicate-row guard
/// (`check_affected`) needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Affected {
    Exact,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DataCaps {
    pub paging: Paging,
    pub count: CountMode,
    pub sort: bool,
    pub filter_ops: Vec<FilterOp>,
    pub raw_filter: Option<RawFilter>,
    pub transactions: TxMode,
    pub affected: Affected,
    /// A column can be set to `DEFAULT` in an UPDATE.
    pub default_keyword: bool,
    pub server_exprs: Vec<ServerExpr>,
    /// Most bind parameters one statement takes.
    pub max_params: u32,
}

/// How Safe Mode confirms a write: the connection's password, or (for a token or command
/// secret, which nobody types) the app passcode or the connection's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SafeConfirm {
    Password,
    Passcode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SafetyCaps {
    /// What really enforces a read-only connection.
    pub read_only: ReadOnlyMode,
    /// The session runs with the engine's hardening (no file or network access from SQL).
    pub hardened: bool,
    pub confirm: SafeConfirm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LifecycleCaps {
    /// How often an idle connection is pinged; None: never (a ping would bill or wake it).
    pub keepalive_secs: Option<u32>,
    pub ping_billed: bool,
}

/// What a session can do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionCaps {
    pub driver: Driver,
    pub flavor: Option<Flavor>,
    /// None for the native stores.
    pub dialect: Option<SqlDialect>,
    pub language: QueryLanguage,
    /// Sorted, without duplicates.
    pub features: Vec<Feature>,
    pub namespaces: NamespaceCaps,
    pub data: DataCaps,
    pub safety: SafetyCaps,
    pub session: LifecycleCaps,
}

/// Every filter operator.
const FILTER_OPS: [FilterOp; 25] = [
    FilterOp::Eq,
    FilterOp::Ne,
    FilterOp::Gt,
    FilterOp::Lt,
    FilterOp::Ge,
    FilterOp::Le,
    FilterOp::Like,
    FilterOp::NotLike,
    FilterOp::In,
    FilterOp::NotIn,
    FilterOp::IsNull,
    FilterOp::IsNotNull,
    FilterOp::Between,
    FilterOp::NotBetween,
    FilterOp::Contains,
    FilterOp::NotContains,
    FilterOp::ContainsCase,
    FilterOp::NotContainsCase,
    FilterOp::HasPrefix,
    FilterOp::HasSuffix,
    FilterOp::HasPrefixCase,
    FilterOp::HasSuffixCase,
    FilterOp::Ilike,
    FilterOp::JsonContains,
    FilterOp::JsonNotContains,
];

/// The expressions the core generators write (`sqlgen::mutate::server_expr_sql`); a precision is
/// a clock's own option.
const SERVER_EXPRS: [ServerExpr; 6] = [
    ServerExpr::Now,
    ServerExpr::CurrentTimestamp,
    ServerExpr::CurrentDate,
    ServerExpr::CurrentTime,
    ServerExpr::LocalTimestamp,
    ServerExpr::Default,
];

/// How often the app pings an idle connection it may ping freely.
const KEEPALIVE_SECS: u32 = 30;

impl SessionCaps {
    /// A session of a core driver on its core dialect: every feature, and the grid, namespaces
    /// and safety as the app has always had them.
    pub fn core(driver: Driver, dialect: Dialect, flavor: Option<Flavor>) -> SessionCaps {
        let (namespaces, count, default_keyword) = match dialect {
            Dialect::Postgres => (
                NamespaceCaps {
                    databases: true,
                    schemas: SchemaMode::Schemas,
                    default_schema: Some("public".into()),
                    labels: NamespaceLabels::default(),
                },
                CountMode::Estimate,
                true,
            ),
            Dialect::Mysql => (
                NamespaceCaps {
                    databases: true,
                    schemas: SchemaMode::DatabaseIsSchema,
                    default_schema: None,
                    labels: NamespaceLabels::default(),
                },
                CountMode::Estimate,
                true,
            ),
            // No row estimates; an UPDATE cannot SET a column to DEFAULT.
            Dialect::Sqlite => (
                NamespaceCaps {
                    databases: false,
                    schemas: SchemaMode::Schemas,
                    default_schema: Some("main".into()),
                    labels: NamespaceLabels::default(),
                },
                CountMode::Exact,
                false,
            ),
        };
        // ILIKE and the JSON containment operators are PostgreSQL's.
        let filter_ops = FILTER_OPS
            .into_iter()
            .filter(|op| {
                dialect == Dialect::Postgres
                    || !matches!(
                        op,
                        FilterOp::Ilike | FilterOp::JsonContains | FilterOp::JsonNotContains
                    )
            })
            .collect();
        SessionCaps {
            driver,
            flavor,
            dialect: Some(dialect.into()),
            language: QueryLanguage::Sql,
            features: Feature::ALL.to_vec(),
            namespaces,
            data: DataCaps {
                paging: Paging::Offset,
                count,
                sort: true,
                filter_ops,
                raw_filter: Some(RawFilter::SqlWhere),
                transactions: TxMode::Atomic,
                affected: Affected::Exact,
                default_keyword,
                server_exprs: SERVER_EXPRS.to_vec(),
                max_params: crate::sqlgen::sql::max_params(dialect) as u32,
            },
            safety: SafetyCaps {
                read_only: driver.descriptor().read_only,
                hardened: false,
                confirm: SafeConfirm::Password,
            },
            session: LifecycleCaps {
                keepalive_secs: Some(KEEPALIVE_SECS),
                ping_billed: false,
            },
        }
    }

    /// What any session of `driver` can do before its driver says more: look at structure (and
    /// run statements in the editor, for a SQL driver), page with offsets, and nothing that
    /// writes. Read-only is the classifier's alone, and the app never pings it on its own.
    pub fn minimal(driver: Driver) -> SessionCaps {
        let desc = driver.descriptor();
        let mut features = vec![Feature::StructureView];
        if desc.dialect.is_some() {
            features.push(Feature::Editor);
        }
        features.sort_unstable();
        SessionCaps {
            driver,
            flavor: None,
            dialect: desc.dialect,
            language: desc.language,
            features,
            namespaces: NamespaceCaps {
                databases: false,
                schemas: SchemaMode::Single,
                default_schema: None,
                labels: NamespaceLabels::default(),
            },
            data: DataCaps {
                paging: Paging::Offset,
                count: CountMode::None,
                sort: false,
                filter_ops: Vec::new(),
                raw_filter: None,
                transactions: TxMode::None,
                affected: Affected::Unknown,
                default_keyword: false,
                server_exprs: Vec::new(),
                max_params: 0,
            },
            safety: SafetyCaps {
                read_only: ReadOnlyMode::ClassifierOnly,
                hardened: false,
                confirm: if desc.secret_is_token {
                    SafeConfirm::Passcode
                } else {
                    SafeConfirm::Password
                },
            },
            session: LifecycleCaps {
                keepalive_secs: None,
                ping_billed: false,
            },
        }
    }

    /// Where a session's caps start: `core` for a core driver on a core dialect, `sqlite_family`
    /// for another driver on SQLite's (LibSQL, D1), otherwise `minimal` (a PostgreSQL flavor
    /// that speaks Redshift SQL is not a core session), which the driver then adjusts.
    pub fn base(driver: Driver, dialect: SqlDialect, flavor: Option<Flavor>) -> SessionCaps {
        match dialect.resolve() {
            DialectRef::Core(core) if driver.is_core() => SessionCaps::core(driver, core, flavor),
            DialectRef::Core(Dialect::Sqlite) => SessionCaps::sqlite_family(driver),
            DialectRef::Core(_) | DialectRef::Ext(_) => {
                let mut caps = SessionCaps::minimal(driver);
                caps.flavor = flavor;
                caps.dialect = Some(dialect);
                caps
            }
        }
    }

    /// LibSQL and D1 speak SQLite and their grid goes through the SQLite builders: SQLite's
    /// namespaces and data caps from the start, the features of `minimal` until the driver grants
    /// more. A server that binds fewer values says so in its descriptor (`wire`, D1: 100). D1's
    /// edits address rows by primary key only: its batch commits before the affected count can be
    /// checked, so an edit by all columns could not be undone when it matched two rows (its
    /// descriptor has no rowid, and the driver never grants `EditWithoutKey`).
    fn sqlite_family(driver: Driver) -> SessionCaps {
        let sqlite = SessionCaps::core(driver, Dialect::Sqlite, None);
        let mut caps = SessionCaps::minimal(driver);
        caps.namespaces = sqlite.namespaces;
        caps.data = sqlite.data;
        if let Some(max) = driver.descriptor().wire.max_params {
            caps.data.max_params = caps.data.max_params.min(max);
        }
        caps
    }

    /// The caps a driver's sessions start from when nothing is known about the server: `base`
    /// on its descriptor's dialect, `minimal` for a native store. (The renderer's `baseCaps`.)
    pub fn for_driver(driver: Driver) -> SessionCaps {
        match driver.descriptor().dialect {
            Some(dialect) => SessionCaps::base(driver, dialect, None),
            None => SessionCaps::minimal(driver),
        }
    }

    /// What a session of `profile` starts from before it connects: `base` on the SQL of its kind
    /// (a Redshift profile is not a core session) with the flavor it names; `minimal` for a native
    /// store.
    pub fn for_profile(profile: &ConnectionProfile) -> SessionCaps {
        let flavor = match profile.kind() {
            KindId::Cockroachdb => Some(Flavor::Cockroachdb),
            KindId::Greenplum => Some(Flavor::Greenplum),
            KindId::Redshift => Some(Flavor::Redshift),
            KindId::Mariadb => Some(Flavor::Mariadb),
            _ => None,
        };
        match profile.sql_dialect() {
            Some(dialect) => SessionCaps::base(profile.driver, dialect, flavor),
            None => SessionCaps::minimal(profile.driver),
        }
    }

    pub fn has(&self, f: Feature) -> bool {
        self.features.binary_search(&f).is_ok()
    }

    /// Refuses `f` when the session lacks it: "<Engine>: <feature> is not available".
    pub fn require(&self, f: Feature) -> Result<(), AppError> {
        if self.has(f) {
            Ok(())
        } else {
            Err(AppError::Unsupported(format!(
                "{}: {} is not available",
                self.engine(),
                f.label()
            )))
        }
    }

    /// `require` for each of `features`.
    pub fn require_all(&self, features: &[Feature]) -> Result<(), AppError> {
        features.iter().try_for_each(|f| self.require(*f))
    }

    pub fn with(mut self, features: &[Feature]) -> SessionCaps {
        self.features.extend_from_slice(features);
        self.features.sort_unstable();
        self.features.dedup();
        self
    }

    pub fn without(mut self, features: &[Feature]) -> SessionCaps {
        self.features.retain(|f| !features.contains(f));
        self
    }

    /// The engine's name: the flavor's when the session found one ("Amazon Redshift").
    pub fn engine(&self) -> &'static str {
        let pg_flavor = match self.flavor {
            Some(Flavor::Cockroachdb) => Some(PgFlavor::Cockroachdb),
            Some(Flavor::Greenplum) => Some(PgFlavor::Greenplum),
            Some(Flavor::Redshift) => Some(PgFlavor::Redshift),
            Some(Flavor::Mariadb) | None => None,
        };
        let mysql = (self.flavor == Some(Flavor::Mariadb)).then_some(MysqlVersion::Mariadb);
        KindId::of(self.driver, pg_flavor, mysql).descriptor().label
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unsupported(r: Result<(), AppError>) -> String {
        match r {
            Err(AppError::Unsupported(m)) => m,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn features_are_listed_once_in_order() {
        let mut sorted = Feature::ALL.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, Feature::ALL);
        for f in Feature::ALL {
            assert!(!f.label().is_empty(), "{f:?}");
        }
    }

    /// What the core sessions report, recorded in `tests/fixtures/caps/core.json`. The renderer
    /// does not read their data and namespace caps in v1, but a change here changes what
    /// PostgreSQL, MySQL and SQLite are said to be: after an intended one,
    /// `TABLER_BLESS=1 cargo test --lib types::caps` rewrites the file (review the diff).
    #[test]
    fn core_caps_snapshot() {
        let caps = [
            SessionCaps::core(Driver::Postgres, Dialect::Postgres, None),
            SessionCaps::core(Driver::Mysql, Dialect::Mysql, Some(Flavor::Mariadb)),
            SessionCaps::core(Driver::Sqlite, Dialect::Sqlite, None),
        ];
        for c in &caps {
            assert_eq!(c.features, Feature::ALL, "{:?}: every feature", c.driver);
            assert!(Feature::ALL.iter().all(|f| c.require(*f).is_ok()));
        }
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/caps/core.json");
        let got = serde_json::to_value(&caps).unwrap();
        if std::env::var("TABLER_BLESS").as_deref() == Ok("1") {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let text = serde_json::to_string_pretty(&got).unwrap() + "\n";
            std::fs::write(&path, text).unwrap();
            return;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e} (TABLER_BLESS=1 writes it)", path.display()));
        let want: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            got, want,
            "the core caps changed; if that is intended: TABLER_BLESS=1 cargo test --lib \
             types::caps, then review the diff"
        );
        // They read back as they were written (`ConnectionInfo` carries them).
        let back: Vec<SessionCaps> = serde_json::from_value(got).unwrap();
        assert_eq!(back, caps);
    }

    #[test]
    fn a_core_driver_on_an_extension_dialect_is_not_a_core_session() {
        // Redshift is the PostgreSQL driver speaking Redshift SQL: it starts from minimal.
        let redshift = SessionCaps::base(
            Driver::Postgres,
            SqlDialect::Redshift,
            Some(Flavor::Redshift),
        );
        assert_eq!(redshift.features, [Feature::Editor, Feature::StructureView]);
        assert_eq!(redshift.dialect, Some(SqlDialect::Redshift));
        assert_eq!(redshift.flavor, Some(Flavor::Redshift));
        assert_eq!(
            unsupported(redshift.require(Feature::StructureEdit)),
            "Amazon Redshift: editing the structure is not available"
        );
        // A non-core driver on a core dialect (LibSQL speaks SQLite) is not one either.
        let libsql = SessionCaps::base(Driver::Libsql, SqlDialect::Sqlite, None);
        assert_eq!(libsql.features, [Feature::Editor, Feature::StructureView]);
        assert_eq!(libsql.dialect, Some(SqlDialect::Sqlite));
        assert_eq!(
            unsupported(libsql.require(Feature::Backup)),
            "LibSQL: Backup is not available"
        );
        // The core three on their own dialects are.
        assert_eq!(
            SessionCaps::base(Driver::Sqlite, SqlDialect::Sqlite, None),
            SessionCaps::core(Driver::Sqlite, Dialect::Sqlite, None)
        );
        let maria = SessionCaps::base(Driver::Mysql, SqlDialect::Mysql, Some(Flavor::Mariadb));
        assert!(maria.has(Feature::Users));
        assert_eq!(maria.engine(), "MariaDB");
    }

    #[test]
    fn minimal_caps_offer_no_writes() {
        for d in Driver::ALL {
            let caps = SessionCaps::minimal(d);
            let sql = d.descriptor().dialect.is_some();
            let mut want = vec![Feature::StructureView];
            if sql {
                want.insert(0, Feature::Editor);
            }
            assert_eq!(caps.features, want, "{d:?}");
            assert_eq!(caps.dialect, d.descriptor().dialect);
            assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);
            assert_eq!(caps.data.affected, Affected::Unknown);
            assert_eq!(caps.session.keepalive_secs, None);
            // A token secret is confirmed with the passcode, not by typing it.
            assert_eq!(
                caps.safety.confirm == SafeConfirm::Passcode,
                d.descriptor().secret_is_token,
                "{d:?}"
            );
            for f in [Feature::EditRows, Feature::Mcp, Feature::Backup] {
                assert!(caps.require(f).is_err(), "{d:?} {f:?}");
            }
            // What the renderer starts from for a driver.
            let base = SessionCaps::for_driver(d);
            if d.is_core() {
                assert_eq!(base.features, Feature::ALL, "{d:?}");
            } else {
                assert_eq!(base.features, want, "{d:?}");
            }
        }
        assert_eq!(
            unsupported(SessionCaps::minimal(Driver::Redis).require(Feature::Editor)),
            "Redis: the query editor is not available"
        );
    }

    #[test]
    fn libsql_and_d1_start_as_the_sqlite_family() {
        let sqlite = SessionCaps::core(Driver::Sqlite, Dialect::Sqlite, None);
        for d in [Driver::Libsql, Driver::D1] {
            let caps = SessionCaps::for_driver(d);
            // The grid of the SQLite builders, none of the core-only features.
            assert_eq!(caps.features, [Feature::Editor, Feature::StructureView]);
            assert_eq!(caps.namespaces, sqlite.namespaces, "{d:?}");
            assert_eq!(caps.data.filter_ops, sqlite.data.filter_ops, "{d:?}");
            assert!(caps.data.sort && caps.data.count == CountMode::Exact);
            assert!(!caps.has(Feature::EditWithoutKey));
            assert_eq!(caps.safety, SessionCaps::minimal(d).safety);
            assert_eq!(caps.session, SessionCaps::minimal(d).session);
        }
        assert_eq!(
            SessionCaps::for_driver(Driver::Libsql).data,
            sqlite.data,
            "LibSQL binds as many parameters as SQLite"
        );
        assert_eq!(SessionCaps::for_driver(Driver::D1).data.max_params, 100);
        // Rows without a primary key: LibSQL's by rowid, D1's not at all.
        assert!(Driver::Libsql.descriptor().rowid);
        assert!(!Driver::D1.descriptor().rowid);
    }

    #[test]
    fn with_and_without_keep_the_list_sorted() {
        let caps = SessionCaps::minimal(Driver::Duckdb)
            .with(&[Feature::Mcp, Feature::EditRows, Feature::Editor])
            .without(&[Feature::StructureView]);
        assert_eq!(
            caps.features,
            [Feature::Editor, Feature::EditRows, Feature::Mcp]
        );
        assert!(caps.has(Feature::Mcp) && !caps.has(Feature::StructureView));
        assert!(caps.require_all(&[Feature::Editor, Feature::Mcp]).is_ok());
        assert_eq!(
            unsupported(caps.require_all(&[Feature::Editor, Feature::DeleteRows])),
            "DuckDB: deleting rows is not available"
        );
    }
}
