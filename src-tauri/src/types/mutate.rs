use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::browse::RowKey;
use super::ddl::DdlOp;
use super::objects::{ObjectOp, RoutineRef};
use super::schema::TableRef;
use super::value::Value;
use crate::error::ErrorPayload;

/// The clock expressions that take a fractional-seconds precision (`NOW(6)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clock {
    Now,
    CurrentTimestamp,
    CurrentTime,
    LocalTimestamp,
}

impl Clock {
    /// The renderer's name for it (the wire form of `ServerExpr`).
    fn wire(self) -> &'static str {
        match self {
            Clock::Now => "now",
            Clock::CurrentTimestamp => "currentTimestamp",
            Clock::CurrentTime => "currentTime",
            Clock::LocalTimestamp => "localTimestamp",
        }
    }

    /// The SQL keyword.
    pub fn keyword(self) -> &'static str {
        match self {
            Clock::Now => "NOW",
            Clock::CurrentTimestamp => "CURRENT_TIMESTAMP",
            Clock::CurrentTime => "CURRENT_TIME",
            Clock::LocalTimestamp => "LOCALTIMESTAMP",
        }
    }
}

/// A server-side SQL expression a staged cell holds instead of a value: Set Value ▸ NOW() and
/// typing `NOW()` / `CURRENT_TIMESTAMP` / `NOW(6)` in a date / time cell (TablePlus). Only these
/// keywords exist, rendered per dialect by `sqlgen::mutate::server_expr_sql`: never arbitrary
/// SQL. `Fsp` carries the fractional-seconds precision (0-6 digits) of a clock typed with one:
/// MySQL's bare `CURRENT_TIMESTAMP` has 0 digits whatever the column's precision, so dropping it
/// would silently cut the microseconds.
///
/// On the wire an expression is one string: the camelCase name (`"now"`, `"currentDate"`…), with
/// its precision in parentheses for the clocks (`"now(6)"`, `"currentTimestamp(3)"`); the names
/// without a precision are the unit variants this type always had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, TS)]
#[ts(
    export,
    type = "\"now\" | \"currentTimestamp\" | \"currentDate\" | \"currentTime\" | \"localTimestamp\" | \"default\" | `now(${number})` | `currentTimestamp(${number})` | `currentTime(${number})` | `localTimestamp(${number})`"
)]
pub enum ServerExpr {
    Now,
    CurrentTimestamp,
    CurrentDate,
    CurrentTime,
    LocalTimestamp,
    Default,
    /// A clock with an explicit precision: `NOW(6)`.
    Fsp(Clock, u8),
}

impl ServerExpr {
    /// The largest fractional-seconds precision (MySQL, MariaDB and PostgreSQL stop at 6).
    pub const MAX_FSP: u8 = 6;

    /// `clock` with `fsp` fractional digits; None above `MAX_FSP`.
    pub fn with_fsp(clock: Clock, fsp: u8) -> Option<ServerExpr> {
        (fsp <= Self::MAX_FSP).then_some(ServerExpr::Fsp(clock, fsp))
    }

    /// The clock and its explicit precision, for the variants that are one.
    pub fn clock(self) -> Option<(Clock, Option<u8>)> {
        match self {
            ServerExpr::Now => Some((Clock::Now, None)),
            ServerExpr::CurrentTimestamp => Some((Clock::CurrentTimestamp, None)),
            ServerExpr::CurrentTime => Some((Clock::CurrentTime, None)),
            ServerExpr::LocalTimestamp => Some((Clock::LocalTimestamp, None)),
            ServerExpr::Fsp(c, n) => Some((c, Some(n))),
            ServerExpr::CurrentDate | ServerExpr::Default => None,
        }
    }

    /// The wire string (`"now"`, `"now(6)"`).
    pub fn wire(self) -> String {
        match self {
            ServerExpr::Now => "now".into(),
            ServerExpr::CurrentTimestamp => "currentTimestamp".into(),
            ServerExpr::CurrentDate => "currentDate".into(),
            ServerExpr::CurrentTime => "currentTime".into(),
            ServerExpr::LocalTimestamp => "localTimestamp".into(),
            ServerExpr::Default => "default".into(),
            ServerExpr::Fsp(c, n) => format!("{}({n})", c.wire()),
        }
    }
}

impl Serialize for ServerExpr {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.wire())
    }
}

impl<'de> Deserialize<'de> for ServerExpr {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<ServerExpr, D::Error> {
        let s = String::deserialize(deserializer)?;
        ServerExpr::try_from(s).map_err(serde::de::Error::custom)
    }
}

impl TryFrom<String> for ServerExpr {
    type Error = String;

    fn try_from(s: String) -> Result<ServerExpr, String> {
        let unknown = || format!("unknown server expression `{s}`");
        let (name, fsp) = match s.strip_suffix(')').and_then(|r| r.split_once('(')) {
            Some((name, digits)) => {
                let fsp = match digits.as_bytes() {
                    [d @ b'0'..=b'9'] => d - b'0',
                    _ => return Err(unknown()),
                };
                (name, Some(fsp))
            }
            None => (s.as_str(), None),
        };
        let clock = match name {
            "now" => Some(Clock::Now),
            "currentTimestamp" => Some(Clock::CurrentTimestamp),
            "currentTime" => Some(Clock::CurrentTime),
            "localTimestamp" => Some(Clock::LocalTimestamp),
            _ => None,
        };
        match (clock, fsp, name) {
            (Some(c), Some(n), _) => ServerExpr::with_fsp(c, n).ok_or_else(|| {
                format!(
                    "the precision of `{s}` is at most {} digits",
                    ServerExpr::MAX_FSP
                )
            }),
            (Some(Clock::Now), None, _) => Ok(ServerExpr::Now),
            (Some(Clock::CurrentTimestamp), None, _) => Ok(ServerExpr::CurrentTimestamp),
            (Some(Clock::CurrentTime), None, _) => Ok(ServerExpr::CurrentTime),
            (Some(Clock::LocalTimestamp), None, _) => Ok(ServerExpr::LocalTimestamp),
            (None, None, "currentDate") => Ok(ServerExpr::CurrentDate),
            (None, None, "default") => Ok(ServerExpr::Default),
            _ => Err(unknown()),
        }
    }
}

/// One staged edit from the grid. `row_id` is the renderer's own key, echoed back in results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export, rename_all_fields = "camelCase")]
pub enum StagedChange {
    Update {
        row_id: String,
        key: RowKey,
        changes: BTreeMap<String, Value>,
        /// Columns set to a server expression; one named here too in `changes` takes this.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        exprs: Option<BTreeMap<String, ServerExpr>>,
    },
    Insert {
        row_id: String,
        values: BTreeMap<String, Value>,
        /// Columns given a server expression; one named here too in `values` takes this.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        exprs: Option<BTreeMap<String, ServerExpr>>,
    },
    Delete {
        row_id: String,
        key: RowKey,
    },
}

impl StagedChange {
    pub fn row_id(&self) -> &str {
        match self {
            StagedChange::Update { row_id, .. }
            | StagedChange::Insert { row_id, .. }
            | StagedChange::Delete { row_id, .. } => row_id,
        }
    }

    pub fn op_name(&self) -> &'static str {
        match self {
            StagedChange::Update { .. } => "update",
            StagedChange::Insert { .. } => "insert",
            StagedChange::Delete { .. } => "delete",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApplyRequest {
    pub table: TableRef,
    pub changes: Vec<StagedChange>,
    /// Ask for the stored row back (PostgreSQL/SQLite `RETURNING`).
    #[serde(default)]
    pub returning: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RowResult {
    pub row_id: String,
    pub op: String,
    pub affected: u64,
    pub returned: Option<BTreeMap<String, Value>>,
    pub error: Option<ErrorPayload>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApplyResult {
    pub success: bool,
    pub results: Vec<RowResult>,
    /// Statements that were (or would have been) executed, with literals inlined.
    pub sql: Vec<String>,
    pub error: Option<ErrorPayload>,
    pub duration_ms: u64,
}

/// One part of a workspace commit (`apply_workspace_changes`, TablePlus: every pending change
/// of the workspace in one transaction): the staged changes of one tab, a pending sidebar
/// operation, or a definition saved from its SQL tab.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export, rename_all_fields = "camelCase")]
pub enum WorkspaceGroup {
    /// Staged row edits of a table tab or of a query tab's result.
    Rows { request: ApplyRequest },
    /// Staged structure changes of a table (its structure view).
    Ddl {
        table: Option<TableRef>,
        ops: Vec<DdlOp>,
    },
    /// A pending sidebar operation (Truncate… / Delete…).
    Object { op: ObjectOp },
    /// A view / function / procedure saved from its SQL tab (⌘S): `drop` first when given (the
    /// routine the tab was opened from, `DROP … IF EXISTS`), then `sql` as ONE statement, sent
    /// as written (no `DELIMITER`).
    Definition {
        drop: Option<RoutineRef>,
        sql: String,
    },
}

/// What one group would run (`preview_workspace_changes`).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GroupPlan {
    /// Statements with literals inlined, in execution order.
    pub statements: Vec<String>,
    pub warnings: Vec<String>,
    /// The statements can run inside the workspace's transaction.
    pub transactional: bool,
    /// The group could not be planned (nothing runs while one cannot).
    pub error: Option<ErrorPayload>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkspacePlan {
    /// One per requested group, in request order.
    pub groups: Vec<GroupPlan>,
    /// Everything runs in ONE transaction (PostgreSQL / SQLite while every plan is
    /// transactional): a failure changes nothing. Otherwise the data edits run first in a
    /// transaction of their own, then each structure change / operation (MySQL DDL commits).
    pub atomic: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum GroupStatus {
    /// Ran and stays applied.
    Applied,
    /// Its statement failed (`GroupResult.failed_index`); `executed` lists what persists.
    Failed,
    /// Ran, then the failure of another group rolled it back.
    RolledBack,
    /// Never started (a failure came first).
    NotRun,
}

/// What became of the old version of a routine whose replacement failed. MySQL commits the
/// `DROP` before the `CREATE` runs, so the old routine is read first (a routine that cannot be
/// read is not dropped) and created again when the new text fails (best effort).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DefinitionRestore {
    /// The old version's `CREATE` text as `SHOW CREATE` printed it (its name qualified with the
    /// schema it lived in), read before the DROP.
    pub previous: String,
    /// The old version is back on the server.
    pub restored: bool,
    /// It was created again without its `DEFINER` (the account may not assume it): the connected
    /// account owns it now.
    pub without_definer: bool,
    /// Why it could not be put back.
    pub error: Option<ErrorPayload>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GroupResult {
    pub status: GroupStatus,
    /// The group's statements, literals inlined (as previewed).
    pub sql: Vec<String>,
    /// The statements whose effects persist.
    pub executed: Vec<String>,
    /// Index into `sql` of the failing statement, when known.
    pub failed_index: Option<usize>,
    /// Row groups: one result per change (with the stored row on PostgreSQL / SQLite).
    pub rows: Vec<RowResult>,
    /// A definition that ran: the routine its text creates, named for a later `DROP`.
    pub routine: Option<RoutineRef>,
    /// A MySQL definition whose CREATE failed after the old routine was dropped: what became of
    /// the old version (restored, or its text so it can be opened in an editor tab).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub restore: Option<DefinitionRestore>,
    pub error: Option<ErrorPayload>,
}

/// Outcome of `apply_workspace_changes`. Failures are reported here, never as `Err`.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkspaceResult {
    pub success: bool,
    /// Ran as one transaction (see `WorkspacePlan::atomic`).
    pub atomic: bool,
    /// One per requested group, in request order.
    pub groups: Vec<GroupResult>,
    /// The group whose statement failed, when known.
    pub failed_group: Option<usize>,
    pub error: Option<ErrorPayload>,
    pub duration_ms: u64,
}
