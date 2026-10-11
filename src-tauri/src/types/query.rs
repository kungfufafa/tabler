use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::schema::ValueKind;
use super::value::Value;
use crate::error::ErrorPayload;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResultColumn {
    pub name: String,
    /// Type name as reported by the server for this result column.
    pub data_type: String,
    pub kind: ValueKind,
    pub nullable: Option<bool>,
    /// Source table when the server reports it (used to detect editable results).
    pub table: Option<String>,
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
}

pub const DEFAULT_MAX_ROWS: u32 = 10_000;

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            max_rows: DEFAULT_MAX_ROWS,
            stop_on_error: true,
            explain: None,
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

/// Streamed to the renderer while `run_sql` executes, in this order per statement:
/// `started`, then (`columns`, `rows`*)* per result set, then `finished` or `failed`; `done`
/// closes the run.
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
    Done {
        total_ms: u64,
    },
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
}
