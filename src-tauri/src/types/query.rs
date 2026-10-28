use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::schema::{TableRef, ValueKind};
use super::value::Value;
use crate::error::ErrorPayload;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResultColumn {
    pub name: String,
    /// Type name as reported by the server for this result column.
    pub data_type: String,
    pub kind: ValueKind,
    pub nullable: Option<bool>,
    /// Source table when the server reports it (used to detect editable results). MySQL reports
    /// the name as written in the query (an alias, a derived table's name); PostgreSQL and SQLite
    /// the base table's name.
    pub table: Option<String>,
    /// The base-table column this result column reads, when the server can tell (a plain column
    /// reference, also through a join or an alias): PostgreSQL's RowDescription table OID and
    /// attribute number resolved through the catalog, MySQL's `org_table` / `org_name`, SQLite's
    /// `sqlite3_column_table_name` / `origin_name` (dropped for compound statements and
    /// statements that read a view, where SQLite names another row's table). `None` for computed
    /// columns.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub origin: Option<ColumnOrigin>,
}

/// Where a result column comes from: `column` of the base table `table`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ColumnOrigin {
    /// Schema-qualified (the PostgreSQL schema, the MySQL database, the SQLite database name).
    pub table: TableRef,
    /// The column's name in that table (the result column may be aliased).
    pub column: String,
    /// Which reference to `table` in the query the column was read through, as the server names
    /// it (MySQL: the alias, or the table name when it has none). The two sides of a self-join
    /// (`employees e JOIN employees m`) differ here, so an edit never mixes them up. `None` when
    /// the server does not tell (PostgreSQL, SQLite): the renderer then refuses to edit a table
    /// the query names more than once.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub instance: Option<String>,
}

impl ColumnOrigin {
    /// An origin from the names a server reports; `None` when the table or column is unknown
    /// (empty), which is how servers describe computed columns.
    pub fn from_names(schema: &str, table: &str, column: &str) -> Option<ColumnOrigin> {
        if table.is_empty() || column.is_empty() {
            return None;
        }
        Some(ColumnOrigin {
            table: TableRef::new((!schema.is_empty()).then_some(schema), table),
            column: column.to_string(),
            instance: None,
        })
    }

    /// The origin read through the table reference `instance` (ignored when empty).
    pub fn with_instance(mut self, instance: &str) -> ColumnOrigin {
        self.instance = (!instance.is_empty()).then(|| instance.to_string());
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResultSet {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Value>>,
    pub truncated: bool,
}

/// Outcome of a statement run through `DbSession::execute`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExecOutcome {
    pub affected: u64,
    pub last_insert_id: Option<String>,
    /// Rows produced by `RETURNING` (or a SELECT run through execute).
    pub rows: Option<ResultSet>,
    /// The server did not say how many rows the statement touched (`affected` is then only a
    /// guess): the affected-rows guard refuses such an outcome (`check_affected`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub affected_unknown: bool,
}

// ---------------------------------------------------------------------------------------------
// SQL editor (run_sql / explain)
// ---------------------------------------------------------------------------------------------

/// `Plan` only plans the statement; `Analyze` executes it and reports actual timings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ExplainMode {
    Plan,
    Analyze,
}

/// What a statement produced: result rows, an affected-row count, nothing (DDL, `SET`…), or a
/// query plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum StatementKind {
    Rows,
    Affected,
    #[default]
    Empty,
    Explain,
}

/// Where a statement sits in the submitted editor text: `[start, end)` character offsets (not
/// bytes) of the trimmed statement, terminator excluded, plus its 1-based first line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StatementSpan {
    pub start: u32,
    pub end: u32,
    pub line: u32,
}

/// Options of one `run_sql` call. Every field has a default, so `{}` is valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct RunOptions {
    /// Row cap per statement; further rows are not read and the result is marked `truncated`.
    pub max_rows: u32,
    /// Stop at the first failing statement (otherwise run the rest and report each error).
    pub stop_on_error: bool,
    /// Wrap every statement in the dialect's EXPLAIN instead of running it.
    pub explain: Option<ExplainMode>,
    /// Cancels a statement still running after this many seconds and reports it as timed out
    /// (Preferences ▸ Connections ▸ Query time out); 0 = no limit.
    pub timeout_secs: u32,
}

pub const DEFAULT_MAX_ROWS: u32 = 10_000;

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            max_rows: DEFAULT_MAX_ROWS,
            stop_on_error: true,
            explain: None,
            timeout_secs: 0,
        }
    }
}

/// Summary of one executed statement. Rows themselves travel through `QueryEvent::Rows`.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StatementResult {
    pub index: usize,
    pub sql: String,
    pub span: StatementSpan,
    pub kind: StatementKind,
    /// Columns of the first result set (empty when the statement returned no rows).
    pub columns: Vec<ResultColumn>,
    /// Number of result sets (`CALL` on MySQL can return several).
    pub result_sets: u32,
    /// Rows delivered (over all result sets).
    pub row_count: u64,
    /// The row cap was hit; further rows were not read.
    pub truncated: bool,
    pub affected: Option<u64>,
    pub last_insert_id: Option<String>,
    /// RFC 3339 timestamp of when the statement started.
    pub started_at: String,
    pub duration_ms: u64,
    pub error: Option<ErrorPayload>,
    /// Server notices / warnings raised while the statement ran.
    pub notices: Vec<String>,
}

/// A message the server sent the editor session besides results (PostgreSQL `RAISE NOTICE`,
/// `RAISE WARNING`, "table … does not exist, skipping"…).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerNotice {
    /// As the server names it: `NOTICE`, `WARNING`, `INFO`, `LOG`, `DEBUG`.
    pub severity: String,
    pub message: String,
    pub detail: Option<String>,
    pub hint: Option<String>,
    /// Where it was raised (PostgreSQL's `CONTEXT`, e.g. "PL/pgSQL function … line 3 at RAISE").
    pub context: Option<String>,
}

impl ServerNotice {
    /// The notice as psql prints it: `NOTICE: …`, then `DETAIL: …` and `HINT: …` lines.
    pub fn to_text(&self) -> String {
        let mut text = format!("{}: {}", self.severity, self.message);
        if let Some(detail) = &self.detail {
            text.push_str("\nDETAIL: ");
            text.push_str(detail);
        }
        if let Some(hint) = &self.hint {
            text.push_str("\nHINT: ");
            text.push_str(hint);
        }
        text
    }
}

/// Streamed to the renderer while `run_sql` executes, in this order per statement:
/// `started`, then (`columns`, `rows`*)* per result set, then `finished` or `failed`; `done`
/// closes the run. `notice` events arrive between `started` and `finished` / `failed`, as the
/// server sends them.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(
    tag = "event",
    content = "data",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export, rename_all_fields = "camelCase")]
pub enum QueryEvent {
    Started {
        index: usize,
        sql: String,
        span: StatementSpan,
    },
    Columns {
        index: usize,
        set: u32,
        columns: Vec<ResultColumn>,
    },
    Rows {
        index: usize,
        set: u32,
        rows: Vec<Vec<Value>>,
    },
    Finished {
        index: usize,
        result: StatementResult,
    },
    Failed {
        index: usize,
        error: ErrorPayload,
    },
    /// A server notice (`ServerNotice`). `index` is the statement that raised it; `None` for one
    /// raised outside the run's statements (bootstrap SQL, an earlier session's activity), which
    /// only arrives with the next run.
    Notice {
        index: Option<usize>,
        severity: String,
        message: String,
        detail: Option<String>,
        hint: Option<String>,
        context: Option<String>,
    },
    Done {
        total_ms: u64,
    },
}

impl QueryEvent {
    /// The `notice` event of `notice`, raised by statement `index` (`None`: outside the run).
    pub fn notice(index: Option<usize>, notice: &ServerNotice) -> QueryEvent {
        QueryEvent::Notice {
            index,
            severity: notice.severity.clone(),
            message: notice.message.clone(),
            detail: notice.detail.clone(),
            hint: notice.hint.clone(),
            context: notice.context.clone(),
        }
    }
}

/// Result of the `explain` command.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExplainResult {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Value>>,
    /// The plan as plain text when the dialect produces a textual plan (PG text format, MySQL
    /// tree / MariaDB JSON, SQLite query-plan tree).
    pub text: Option<String>,
    pub duration_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_options_default_from_empty_object() {
        let opts: RunOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(opts, RunOptions::default());
        assert_eq!(opts.max_rows, 10_000);
        assert!(opts.stop_on_error);
        let opts: RunOptions =
            serde_json::from_str(r#"{"maxRows":5,"stopOnError":false,"explain":"analyze"}"#)
                .unwrap();
        assert_eq!(opts.max_rows, 5);
        assert!(!opts.stop_on_error);
        assert_eq!(opts.explain, Some(ExplainMode::Analyze));
        assert_eq!(opts.timeout_secs, 0);
        let opts: RunOptions = serde_json::from_str(r#"{"timeoutSecs":300}"#).unwrap();
        assert_eq!(opts.timeout_secs, 300);
    }

    #[test]
    fn column_origins() {
        assert_eq!(ColumnOrigin::from_names("db", "", "id"), None);
        assert_eq!(ColumnOrigin::from_names("db", "users", ""), None);
        let o = ColumnOrigin::from_names("", "users", "id").unwrap();
        assert_eq!(o.table, TableRef::new(None, "users"));
        let o = ColumnOrigin::from_names("main", "users", "id").unwrap();
        assert_eq!(o.table.schema.as_deref(), Some("main"));
        assert_eq!(o.column, "id");
        assert_eq!(o.instance, None);
        assert_eq!(o.clone().with_instance("").instance, None);
        assert_eq!(o.clone().with_instance("u").instance.as_deref(), Some("u"));

        // Serialized for the renderer: `origin` is always present (null without one).
        let col = ResultColumn {
            name: "user_id".into(),
            data_type: "int4".into(),
            kind: ValueKind::Int,
            nullable: None,
            table: Some("users".into()),
            origin: Some(o),
        };
        let json = serde_json::to_value(&col).unwrap();
        assert_eq!(
            json["origin"],
            serde_json::json!({
                "table": {"schema": "main", "name": "users"}, "column": "id", "instance": null
            })
        );
        // Older payloads without an instance still read.
        let back: ColumnOrigin = serde_json::from_value(serde_json::json!({
            "table": {"schema": null, "name": "t"}, "column": "a"
        }))
        .unwrap();
        assert_eq!(back.instance, None);
        let back: ResultColumn = serde_json::from_value(serde_json::json!({
            "name": "a", "dataType": "int", "kind": "int", "nullable": null, "table": null
        }))
        .unwrap();
        assert_eq!(back.origin, None);
    }

    #[test]
    fn query_event_json_shape() {
        let ev = QueryEvent::Rows {
            index: 1,
            set: 0,
            rows: vec![vec![Value::Int(1), Value::Null]],
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "event": "rows",
                "data": {"index": 1, "set": 0, "rows": [[{"t": "int", "v": 1}, {"t": "null"}]]}
            })
        );

        let ev = QueryEvent::Done { total_ms: 12 };
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            serde_json::json!({"event": "done", "data": {"totalMs": 12}})
        );

        let ev = QueryEvent::Started {
            index: 0,
            sql: "select 1".into(),
            span: StatementSpan {
                start: 0,
                end: 8,
                line: 1,
            },
        };
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            serde_json::json!({
                "event": "started",
                "data": {"index": 0, "sql": "select 1", "span": {"start": 0, "end": 8, "line": 1}}
            })
        );

        let result = StatementResult {
            index: 0,
            sql: "update t set a = 1".into(),
            span: StatementSpan::default(),
            kind: StatementKind::Affected,
            columns: vec![],
            result_sets: 0,
            row_count: 0,
            truncated: false,
            affected: Some(3),
            last_insert_id: None,
            started_at: "2026-01-01T00:00:00.000Z".into(),
            duration_ms: 4,
            error: None,
            notices: vec![],
        };
        let json = serde_json::to_value(QueryEvent::Finished { index: 0, result }).unwrap();
        assert_eq!(json["event"], "finished");
        assert_eq!(json["data"]["result"]["kind"], "affected");
        assert_eq!(json["data"]["result"]["resultSets"], 0);
        assert_eq!(json["data"]["result"]["durationMs"], 4);
        assert_eq!(
            json["data"]["result"]["lastInsertId"],
            serde_json::Value::Null
        );

        let err = crate::error::AppError::Cancelled.to_payload();
        let json = serde_json::to_value(QueryEvent::Failed {
            index: 2,
            error: err,
        })
        .unwrap();
        assert_eq!(json["event"], "failed");
        assert_eq!(json["data"]["error"]["code"], "CANCELLED");
    }

    #[test]
    fn notice_events_and_text() {
        let notice = ServerNotice {
            severity: "WARNING".into(),
            message: "careful".into(),
            detail: Some("d".into()),
            hint: Some("h".into()),
            context: Some("PL/pgSQL function inline_code_block line 1 at RAISE".into()),
        };
        assert_eq!(notice.to_text(), "WARNING: careful\nDETAIL: d\nHINT: h");
        let plain = ServerNotice {
            severity: "NOTICE".into(),
            message: "hi".into(),
            ..ServerNotice::default()
        };
        assert_eq!(plain.to_text(), "NOTICE: hi");

        let json = serde_json::to_value(QueryEvent::notice(Some(2), &notice)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "event": "notice",
                "data": {
                    "index": 2,
                    "severity": "WARNING",
                    "message": "careful",
                    "detail": "d",
                    "hint": "h",
                    "context": "PL/pgSQL function inline_code_block line 1 at RAISE"
                }
            })
        );
        let json = serde_json::to_value(QueryEvent::notice(None, &plain)).unwrap();
        assert_eq!(json["data"]["index"], serde_json::Value::Null);
        assert_eq!(json["data"]["detail"], serde_json::Value::Null);
    }
}
