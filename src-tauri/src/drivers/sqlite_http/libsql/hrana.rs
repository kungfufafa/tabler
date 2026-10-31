//! Hrana over HTTP, versions 2 and 3, JSON encoding (libsql `docs/HRANA_3_SPEC.md`,
//! `docs/HTTP_V2_SPEC.md`): the bodies of `POST v2|v3/pipeline` and `POST v3/cursor`.
//!
//! Requests serialize as the spec writes them. Responses deserialize leniently where servers
//! differ (an integer sent as a JSON number, a float sent as `null`, an unknown response or
//! entry type) and fail with an error, never a panic, on anything else.

use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// A value as Hrana carries it. Integers travel as decimal strings (64 bits), blobs as base64.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    try_from = "serde_json::Value"
)]
pub enum HranaValue {
    Null,
    Integer {
        value: String,
    },
    /// `None`: a non-finite float, which a JSON server can only send as `null`.
    Float {
        value: Option<f64>,
    },
    Text {
        value: String,
    },
    Blob {
        base64: String,
    },
}

impl TryFrom<serde_json::Value> for HranaValue {
    type Error = String;

    fn try_from(v: serde_json::Value) -> Result<HranaValue, String> {
        use serde_json::Value as J;
        let field = |name: &str| v.get(name).cloned().unwrap_or(J::Null);
        let ty = v.get("type").and_then(J::as_str).unwrap_or_default();
        Ok(match ty {
            "null" => HranaValue::Null,
            "integer" => HranaValue::Integer {
                value: match field("value") {
                    J::String(s) => s,
                    J::Number(n) if n.is_i64() || n.is_u64() => n.to_string(),
                    other => return Err(format!("integer value {other}")),
                },
            },
            "float" => HranaValue::Float {
                value: match field("value") {
                    J::Number(n) => n.as_f64(),
                    J::Null => None,
                    J::String(s) => {
                        Some(s.trim().parse::<f64>().map_err(|_| format!("float {s}"))?)
                    }
                    other => return Err(format!("float value {other}")),
                },
            },
            "text" => HranaValue::Text {
                value: match field("value") {
                    J::String(s) => s,
                    other => return Err(format!("text value {other}")),
                },
            },
            "blob" => HranaValue::Blob {
                base64: match field("base64") {
                    J::String(s) => s,
                    other => return Err(format!("blob value {other}")),
                },
            },
            other => return Err(format!("unknown value type {other:?}")),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stmt {
    pub sql: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<HranaValue>,
    pub want_rows: bool,
}

impl Stmt {
    pub fn new(sql: impl Into<String>, args: Vec<HranaValue>) -> Stmt {
        Stmt {
            sql: sql.into(),
            args,
            want_rows: true,
        }
    }

    /// A statement whose rows nobody reads (BEGIN, a PRAGMA of the prelude).
    pub fn no_rows(sql: impl Into<String>) -> Stmt {
        Stmt {
            want_rows: false,
            ..Stmt::new(sql, Vec::new())
        }
    }
}

/// When a batch step runs, from the outcome of earlier steps (0-based). Only the conditions the
/// client builds; Hrana also has `error` and `or`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BatchCond {
    Ok { step: u32 },
    Not { cond: Box<BatchCond> },
    And { conds: Vec<BatchCond> },
    IsAutocommit,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BatchStep {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<BatchCond>,
    pub stmt: Stmt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct Batch {
    pub steps: Vec<BatchStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamRequest {
    Close,
    Batch {
        batch: Batch,
    },
    /// Hrana 3 only.
    GetAutocommit,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PipelineRequest {
    pub baton: Option<String>,
    pub requests: Vec<StreamRequest>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CursorRequest {
    pub baton: Option<String>,
    pub batch: Batch,
}

/// An error the server reports for a request or a step. `code` is server-specific.
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct HranaError {
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Col {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub decltype: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
pub struct StmtResult {
    pub cols: Vec<Col>,
    pub rows: Vec<Vec<HranaValue>>,
    #[serde(deserialize_with = "lenient_u64")]
    pub affected_row_count: u64,
    #[serde(deserialize_with = "lenient_opt_string")]
    pub last_insert_rowid: Option<String>,
    pub rows_read: Option<u64>,
    pub rows_written: Option<u64>,
    pub query_duration_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
pub struct BatchResult {
    pub step_results: Vec<Option<StmtResult>>,
    pub step_errors: Vec<Option<HranaError>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamResponse {
    Close,
    Batch {
        result: BatchResult,
    },
    GetAutocommit {
        is_autocommit: bool,
    },
    /// A response to a request the client does not send, or one a newer server added.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamResult {
    Ok {
        response: StreamResponse,
    },
    Error {
        error: HranaError,
    },
    /// Some servers answer a request that was not run (after an earlier failure) with `none`.
    #[serde(other)]
    None,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PipelineResponse {
    #[serde(default)]
    pub baton: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub results: Vec<StreamResult>,
}

/// The first line of a cursor response.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CursorHead {
    #[serde(default)]
    pub baton: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

/// Each later line of a cursor response.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CursorEntry {
    StepBegin {
        step: u32,
        #[serde(default)]
        cols: Vec<Col>,
    },
    StepEnd {
        #[serde(default, deserialize_with = "lenient_u64")]
        affected_row_count: u64,
        #[serde(default, deserialize_with = "lenient_opt_string")]
        last_insert_rowid: Option<String>,
    },
    StepError {
        step: u32,
        error: HranaError,
    },
    Row {
        row: Vec<HranaValue>,
    },
    Error {
        error: HranaError,
    },
    #[serde(other)]
    Unknown,
}

/// A count sent as a number or as a decimal string.
fn lenient_u64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    use serde::de::Error as _;
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::Null => Ok(0),
        serde_json::Value::Number(n) => n
            .as_u64()
            .ok_or_else(|| D::Error::custom(format!("count {n}"))),
        serde_json::Value::String(s) => s
            .trim()
            .parse()
            .map_err(|_| D::Error::custom(format!("count {s:?}"))),
        other => Err(D::Error::custom(format!("count {other}"))),
    }
}

/// A rowid sent as a string (the spec) or a number.
fn lenient_opt_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    use serde::de::Error as _;
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) => Ok(Some(s)),
        serde_json::Value::Number(n) => Ok(Some(n.to_string())),
        other => Err(D::Error::custom(format!("rowid {other}"))),
    }
}

/// Splits a cursor body into lines as its pieces arrive. Lines are read from an offset, and the
/// read part is dropped only once it is at least half the buffer, so a body that arrives in one
/// piece (or in many) is split in linear time.
#[derive(Debug, Default)]
pub struct LineBuffer {
    buf: Vec<u8>,
    /// Where the unread part of `buf` starts.
    start: usize,
    /// Up to here the unread part has no line break (the search resumes there).
    scanned: usize,
}

impl LineBuffer {
    pub fn push(&mut self, piece: &[u8]) {
        if self.start > 0 && self.start * 2 >= self.buf.len() {
            self.buf.drain(..self.start);
            self.scanned -= self.start;
            self.start = 0;
        }
        self.buf.extend_from_slice(piece);
    }

    /// The next complete line, without its line break; blank lines are skipped.
    pub fn next_line(&mut self) -> Option<Vec<u8>> {
        loop {
            let from = self.scanned.max(self.start);
            let Some(at) = self.buf[from..].iter().position(|b| *b == b'\n') else {
                self.scanned = self.buf.len();
                return None;
            };
            let begin = self.start;
            let mut end = from + at;
            self.start = end + 1;
            self.scanned = self.start;
            if end > begin && self.buf[end - 1] == b'\r' {
                end -= 1;
            }
            let line = &self.buf[begin..end];
            if !line.iter().all(u8::is_ascii_whitespace) {
                return Some(line.to_vec());
            }
        }
    }

    /// Bytes pushed and not yet read as lines: once `next_line` finds none, one unfinished line.
    pub fn unread(&self) -> usize {
        self.buf.len().saturating_sub(self.start)
    }

    /// What is left once the body has ended: a last line without a line break.
    pub fn finish(&mut self) -> Option<Vec<u8>> {
        let mut rest = std::mem::take(&mut self.buf);
        rest.drain(..self.start.min(rest.len()));
        self.start = 0;
        self.scanned = 0;
        (!rest.iter().all(u8::is_ascii_whitespace)).then_some(rest)
    }
}

/// Parses one cursor line.
pub fn parse_line<T: for<'de> Deserialize<'de>>(line: &[u8], what: &str) -> Result<T, AppError> {
    serde_json::from_slice(line).map_err(|e| {
        AppError::db(format!(
            "unexpected {what} from the LibSQL server: {e} ({})",
            super::super::transport::body_excerpt(line, None)
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_serialize_as_the_spec_writes_them() {
        let req = PipelineRequest {
            baton: None,
            requests: vec![
                StreamRequest::Batch {
                    batch: Batch {
                        steps: vec![
                            BatchStep {
                                condition: None,
                                stmt: Stmt::new(
                                    "SELECT ?1",
                                    vec![
                                        HranaValue::Integer { value: "1".into() },
                                        HranaValue::Null,
                                    ],
                                ),
                            },
                            BatchStep {
                                condition: None,
                                stmt: Stmt::no_rows("BEGIN"),
                            },
                            BatchStep {
                                condition: Some(BatchCond::Not {
                                    cond: Box::new(BatchCond::Ok { step: 0 }),
                                }),
                                stmt: Stmt::no_rows("ROLLBACK"),
                            },
                        ],
                    },
                },
                StreamRequest::GetAutocommit,
                StreamRequest::Close,
            ],
        };
        assert_eq!(
            serde_json::to_value(&req).unwrap(),
            json!({
                "baton": null,
                "requests": [
                    {"type": "batch", "batch": {"steps": [
                        {"stmt": {"sql": "SELECT ?1", "args": [
                            {"type": "integer", "value": "1"}, {"type": "null"}], "want_rows": true}},
                        {"stmt": {"sql": "BEGIN", "want_rows": false}},
                        {"condition": {"type": "not", "cond": {"type": "ok", "step": 0}},
                         "stmt": {"sql": "ROLLBACK", "want_rows": false}}]}},
                    {"type": "get_autocommit"},
                    {"type": "close"}
                ]
            })
        );
    }

    #[test]
    fn responses_parse_leniently() {
        let body = json!({
            "baton": "b1",
            "base_url": null,
            "results": [
                {"type": "ok", "response": {"type": "batch", "result": {
                    "step_results": [{
                        "cols": [{"name": "a", "decltype": "INTEGER"}, {"name": null}],
                        "rows": [[{"type": "integer", "value": "9007199254740993"},
                                  {"type": "float", "value": 1.5}],
                                 [{"type": "integer", "value": 3},
                                  {"type": "float", "value": null}],
                                 [{"type": "text", "value": "x"},
                                  {"type": "blob", "base64": "AQI="}]],
                        "affected_row_count": 0,
                        "last_insert_rowid": null,
                        "rows_read": 1, "rows_written": 0, "query_duration_ms": 0.1}],
                    "step_errors": [null]}}},
                {"type": "error", "error": {"message": "no such table: t", "code": "SQLITE_ERROR"}},
                {"type": "ok", "response": {"type": "get_autocommit", "is_autocommit": false}},
                {"type": "ok", "response": {"type": "batch", "result": {
                    "step_results": [{"cols": [], "rows": [], "affected_row_count": "2",
                                      "last_insert_rowid": 5}, null],
                    "step_errors": [null, {"message": "boom"}]}}},
                {"type": "ok", "response": {"type": "describe", "result": {"params": []}}},
                {"type": "none"},
                {"type": "ok", "response": {"type": "close"}}
            ]
        });
        let resp: PipelineResponse = serde_json::from_value(body).unwrap();
        assert_eq!(resp.baton.as_deref(), Some("b1"));
        let StreamResult::Ok {
            response: StreamResponse::Batch { result },
        } = &resp.results[0]
        else {
            panic!("{:?}", resp.results[0]);
        };
        let result = result.step_results[0].as_ref().unwrap();
        assert_eq!(result.cols[1].name, None);
        assert_eq!(
            result.rows[0][0],
            HranaValue::Integer {
                value: "9007199254740993".into()
            }
        );
        assert_eq!(result.rows[1][0], HranaValue::Integer { value: "3".into() });
        assert_eq!(result.rows[1][1], HranaValue::Float { value: None });
        assert!(
            matches!(&resp.results[1], StreamResult::Error { error } if error.code.as_deref() == Some("SQLITE_ERROR"))
        );
        assert!(matches!(
            resp.results[2],
            StreamResult::Ok {
                response: StreamResponse::GetAutocommit {
                    is_autocommit: false
                }
            }
        ));
        let StreamResult::Ok {
            response: StreamResponse::Batch { result },
        } = &resp.results[3]
        else {
            panic!();
        };
        let first = result.step_results[0].as_ref().unwrap();
        assert_eq!(first.affected_row_count, 2);
        assert_eq!(first.last_insert_rowid.as_deref(), Some("5"));
        assert_eq!(result.step_errors[1].as_ref().unwrap().message, "boom");
        assert!(matches!(
            resp.results[4],
            StreamResult::Ok {
                response: StreamResponse::Unknown
            }
        ));
        assert_eq!(resp.results[5], StreamResult::None);
    }

    #[test]
    fn malformed_values_are_errors_not_panics() {
        for bad in [
            json!({"type": "integer", "value": 1.5}),
            json!({"type": "integer"}),
            json!({"type": "text", "value": 3}),
            json!({"type": "blob"}),
            json!({"type": "date", "value": "x"}),
            json!("text"),
            json!({"type": "float", "value": "nope"}),
        ] {
            assert!(
                serde_json::from_value::<HranaValue>(bad.clone()).is_err(),
                "{bad}"
            );
        }
        assert!(serde_json::from_value::<PipelineResponse>(json!({"results": 3})).is_err());
        assert!(serde_json::from_value::<StmtResult>(json!({"affected_row_count": -1})).is_err());
    }

    #[test]
    fn cursor_lines_split_across_pieces() {
        let body = concat!(
            "{\"baton\":\"b2\",\"base_url\":null}\n",
            "{\"type\":\"step_begin\",\"step\":0,\"cols\":[{\"name\":\"a\",\"decltype\":null}]}\n",
            "{\"type\":\"row\",\"row\":[{\"type\":\"integer\",\"value\":\"1\"}]}\r\n",
            "\n",
            "{\"type\":\"step_end\",\"affected_row_count\":0,\"last_insert_rowid\":null}\n",
            "{\"type\":\"step_error\",\"step\":1,\"error\":{\"message\":\"x\"}}\n",
            "{\"type\":\"future\"}\n",
            "{\"type\":\"error\",\"error\":{\"message\":\"gone\",\"code\":\"STREAM_EXPIRED\"}}"
        );
        let mut buf = LineBuffer::default();
        let mut lines = Vec::new();
        for piece in body.as_bytes().chunks(7) {
            buf.push(piece);
            while let Some(line) = buf.next_line() {
                lines.push(line);
            }
        }
        lines.extend(buf.finish());
        assert_eq!(lines.len(), 7);
        let head: CursorHead = parse_line(&lines[0], "cursor").unwrap();
        assert_eq!(head.baton.as_deref(), Some("b2"));
        let entries: Vec<CursorEntry> = lines[1..]
            .iter()
            .map(|l| parse_line(l, "cursor entry").unwrap())
            .collect();
        assert!(matches!(&entries[0], CursorEntry::StepBegin { step: 0, cols } if cols.len() == 1));
        assert!(matches!(&entries[1], CursorEntry::Row { row } if row.len() == 1));
        assert!(matches!(
            entries[2],
            CursorEntry::StepEnd {
                affected_row_count: 0,
                last_insert_rowid: None
            }
        ));
        assert!(matches!(
            &entries[3],
            CursorEntry::StepError { step: 1, .. }
        ));
        assert_eq!(entries[4], CursorEntry::Unknown);
        assert!(
            matches!(&entries[5], CursorEntry::Error { error } if error.code.as_deref() == Some("STREAM_EXPIRED"))
        );
        assert!(parse_line::<CursorEntry>(b"{not json", "cursor entry").is_err());
        assert_eq!(LineBuffer::default().finish(), None);
    }

    #[test]
    fn line_buffers_read_large_bodies_in_one_pass() {
        // 200,000 lines in one piece (what `send_streaming` hands over by default), then the
        // same body in uneven pieces with a line split across them.
        let line = |i: usize| format!("{{\"type\":\"row\",\"row\":[{i}]}}");
        let body: String = (0..200_000).map(|i| line(i) + "\n").collect();
        let mut buf = LineBuffer::default();
        buf.push(body.as_bytes());
        let mut n = 0;
        while let Some(l) = buf.next_line() {
            assert_eq!(l, line(n).as_bytes());
            n += 1;
        }
        assert_eq!(n, 200_000);
        assert_eq!(buf.finish(), None);

        let body = format!("{}\r\n\n{}\n{}", line(0), line(1), line(2));
        for size in [1, 3, 64] {
            let mut buf = LineBuffer::default();
            let mut lines = Vec::new();
            for piece in body.as_bytes().chunks(size) {
                buf.push(piece);
                while let Some(l) = buf.next_line() {
                    lines.push(l);
                }
            }
            lines.extend(buf.finish());
            assert_eq!(
                lines,
                [line(0), line(1), line(2)].map(String::into_bytes),
                "pieces of {size}"
            );
        }
    }
}
