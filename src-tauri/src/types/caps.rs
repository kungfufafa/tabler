//! What a connection can do. For now only the list of features, as data (descriptors name the
//! core-only ones a driver may reuse); the session capabilities that use it come later.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

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
