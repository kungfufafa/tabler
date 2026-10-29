//! Query jobs: the bodies of `jobs.insert` and `jobs.query`, and the replies of those,
//! `jobs.getQueryResults`, `jobs.get` and `jobs.list`.
//!
//! The editor's statements run as inserted jobs with an id Tabler picks (`tabler_<uuid>`), so a
//! cancel can name the job before BigQuery has answered; `jobs.query` (one round trip) serves
//! the dry runs and the driver's own small reads. Every query is GoogleSQL with named
//! parameters, carries the profile's cost guard (`maximumBytesBilled`) and asks for int64
//! timestamps.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::decode::{TableSchema, lenient_i64, lenient_u64};
use super::errors::ErrorProto;

/// Where a dataset lives: the default of unqualified names.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DatasetReference {
    pub project_id: String,
    pub dataset_id: String,
}

/// What every query of a session carries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QueryOptions {
    pub location: Option<String>,
    pub default_dataset: Option<DatasetReference>,
    /// `None`: no cost guard.
    pub maximum_bytes_billed: Option<u64>,
    pub use_query_cache: bool,
    pub job_timeout_ms: Option<u64>,
    pub labels: BTreeMap<String, String>,
    /// A BigQuery session the statement joins (temp tables, variables and transactions live
    /// there), or `create_session` to start one.
    pub session_id: Option<String>,
    pub create_session: bool,
    pub dry_run: bool,
}

/// The `JobConfigurationQuery` part both calls share.
fn query_config(sql: &str, params: &[serde_json::Value], opts: &QueryOptions) -> serde_json::Value {
    let mut q = serde_json::json!({
        "query": sql,
        "useLegacySql": false,
        "useQueryCache": opts.use_query_cache,
    });
    if !params.is_empty() {
        q["parameterMode"] = "NAMED".into();
        q["queryParameters"] = serde_json::Value::Array(params.to_vec());
    }
    if let Some(d) = &opts.default_dataset {
        q["defaultDataset"] = serde_json::json!({
            "projectId": d.project_id,
            "datasetId": d.dataset_id,
        });
    }
    if let Some(b) = opts.maximum_bytes_billed {
        // int64 fields are strings in the API.
        q["maximumBytesBilled"] = b.to_string().into();
    }
    if opts.create_session {
        q["createSession"] = true.into();
    }
    if let Some(s) = &opts.session_id {
        q["connectionProperties"] = serde_json::json!([{ "key": "session_id", "value": s }]);
    }
    q
}

/// The body of `jobs.insert` for a query job with the id `job_id`.
pub fn insert_body(
    project: &str,
    job_id: &str,
    sql: &str,
    params: &[serde_json::Value],
    opts: &QueryOptions,
) -> serde_json::Value {
    let mut job_ref = serde_json::json!({ "projectId": project, "jobId": job_id });
    if let Some(l) = &opts.location {
        job_ref["location"] = l.clone().into();
    }
    let mut config = serde_json::json!({ "query": query_config(sql, params, opts) });
    if opts.dry_run {
        config["dryRun"] = true.into();
    }
    if let Some(t) = opts.job_timeout_ms {
        config["jobTimeoutMs"] = t.to_string().into();
    }
    if !opts.labels.is_empty() {
        config["labels"] = serde_json::json!(opts.labels);
    }
    serde_json::json!({ "jobReference": job_ref, "configuration": config })
}

/// The body of `jobs.query`: waits up to `timeout_ms` and returns the first `max_results` rows.
pub fn query_body(
    sql: &str,
    params: &[serde_json::Value],
    opts: &QueryOptions,
    max_results: Option<u32>,
    timeout_ms: Option<u32>,
) -> serde_json::Value {
    let mut body = query_config(sql, params, opts);
    body["formatOptions"] = serde_json::json!({ "useInt64Timestamp": true });
    if let Some(l) = &opts.location {
        body["location"] = l.clone().into();
    }
    if opts.dry_run {
        body["dryRun"] = true.into();
    }
    if let Some(m) = max_results {
        body["maxResults"] = m.into();
    }
    if let Some(t) = timeout_ms {
        body["timeoutMs"] = t.into();
    }
    if let Some(t) = opts.job_timeout_ms {
        body["jobTimeoutMs"] = t.to_string().into();
    }
    if !opts.labels.is_empty() {
        body["labels"] = serde_json::json!(opts.labels);
    }
    body
}

/// A fresh job id: `tabler_` and a UUID (letters, digits, `_` and `-` only, as BigQuery wants).
pub fn new_job_id() -> String {
    format!("tabler_{}", uuid::Uuid::new_v4().simple())
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JobReference {
    pub project_id: String,
    pub job_id: String,
    pub location: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionInfo {
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DmlStats {
    #[serde(deserialize_with = "lenient_u64")]
    pub inserted_row_count: Option<u64>,
    #[serde(deserialize_with = "lenient_u64")]
    pub deleted_row_count: Option<u64>,
    #[serde(deserialize_with = "lenient_u64")]
    pub updated_row_count: Option<u64>,
}

/// The reply of `jobs.query` and `jobs.getQueryResults`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QueryResponse {
    pub schema: Option<TableSchema>,
    pub job_reference: Option<JobReference>,
    pub query_id: Option<String>,
    #[serde(deserialize_with = "lenient_u64")]
    pub total_rows: Option<u64>,
    pub page_token: Option<String>,
    pub rows: Vec<serde_json::Value>,
    #[serde(deserialize_with = "lenient_u64")]
    pub total_bytes_processed: Option<u64>,
    #[serde(deserialize_with = "lenient_u64")]
    pub total_bytes_billed: Option<u64>,
    /// `false`: the job still runs; ask again.
    pub job_complete: Option<bool>,
    pub errors: Vec<ErrorProto>,
    pub cache_hit: Option<bool>,
    #[serde(deserialize_with = "lenient_u64")]
    pub num_dml_affected_rows: Option<u64>,
    pub dml_stats: Option<DmlStats>,
    pub session_info: Option<SessionInfo>,
}

impl QueryResponse {
    /// The job has finished (a reply that does not say is a finished one).
    pub fn complete(&self) -> bool {
        self.job_complete.unwrap_or(true)
    }

    /// The rows a DML statement touched, when the reply says.
    pub fn affected(&self) -> Option<u64> {
        self.num_dml_affected_rows.or_else(|| {
            let d = self.dml_stats.as_ref()?;
            let parts = [
                d.inserted_row_count,
                d.deleted_row_count,
                d.updated_row_count,
            ];
            // The counts are the server's: added without overflowing.
            parts.iter().any(Option::is_some).then(|| {
                parts
                    .iter()
                    .flatten()
                    .fold(0u64, |a, &n| a.saturating_add(n))
            })
        })
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_info
            .as_ref()
            .and_then(|s| s.session_id.as_deref())
            .filter(|s| !s.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JobStatus {
    /// `PENDING`, `RUNNING` or `DONE`.
    pub state: Option<String>,
    pub error_result: Option<ErrorProto>,
    pub errors: Vec<ErrorProto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QueryStatistics {
    #[serde(deserialize_with = "lenient_u64")]
    pub total_bytes_processed: Option<u64>,
    #[serde(deserialize_with = "lenient_u64")]
    pub total_bytes_billed: Option<u64>,
    /// `SELECT`, `INSERT`, `SCRIPT`, `CREATE_TABLE`…
    pub statement_type: Option<String>,
    #[serde(deserialize_with = "lenient_u64")]
    pub num_dml_affected_rows: Option<u64>,
    pub cache_hit: Option<bool>,
    pub schema: Option<TableSchema>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JobStatistics {
    #[serde(deserialize_with = "lenient_i64")]
    pub creation_time: Option<i64>,
    #[serde(deserialize_with = "lenient_i64")]
    pub start_time: Option<i64>,
    #[serde(deserialize_with = "lenient_i64")]
    pub end_time: Option<i64>,
    #[serde(deserialize_with = "lenient_u64")]
    pub total_bytes_processed: Option<u64>,
    pub query: Option<QueryStatistics>,
    pub session_info: Option<SessionInfo>,
    #[serde(deserialize_with = "lenient_u64")]
    pub num_child_jobs: Option<u64>,
    pub parent_job_id: Option<String>,
}

/// A job (`jobs.insert`, `jobs.get`, `jobs.cancel`'s `job`, an item of `jobs.list`).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Job {
    pub id: Option<String>,
    pub job_reference: Option<JobReference>,
    /// `jobs.list` puts the state here as well as in `status`.
    pub state: Option<String>,
    pub status: Option<JobStatus>,
    pub statistics: Option<JobStatistics>,
    // The API spells this one in snake case.
    #[serde(rename = "user_email", alias = "userEmail")]
    pub user_email: Option<String>,
    pub configuration: Option<serde_json::Value>,
}

impl Job {
    pub fn state(&self) -> Option<&str> {
        self.status
            .as_ref()
            .and_then(|s| s.state.as_deref())
            .or(self.state.as_deref())
    }

    pub fn done(&self) -> bool {
        self.state() == Some("DONE")
    }

    /// The error a finished job reports.
    pub fn error(&self) -> Option<(&ErrorProto, &[ErrorProto])> {
        let s = self.status.as_ref()?;
        Some((s.error_result.as_ref()?, s.errors.as_slice()))
    }

    /// The SQL of a query job, for the process list.
    pub fn query_text(&self) -> Option<&str> {
        self.configuration
            .as_ref()?
            .get("query")?
            .get("query")?
            .as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JobList {
    pub jobs: Vec<Job>,
    pub next_page_token: Option<String>,
}

/// The reply of `jobs.cancel`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CancelReply {
    pub job: Option<Job>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn opts() -> QueryOptions {
        QueryOptions {
            location: Some("asia-southeast2".into()),
            default_dataset: Some(DatasetReference {
                project_id: "p".into(),
                dataset_id: "shop".into(),
            }),
            maximum_bytes_billed: Some(1 << 30),
            use_query_cache: true,
            job_timeout_ms: Some(60_000),
            labels: BTreeMap::from([("app".to_string(), "tabler".to_string())]),
            session_id: Some("CgwK".into()),
            create_session: false,
            dry_run: false,
        }
    }

    #[test]
    fn insert_bodies_carry_the_guard_and_the_session() {
        let params = vec![json!({"name": "p1", "parameterType": {"type": "INT64"},
            "parameterValue": {"value": "1"}})];
        let b = insert_body("p", "tabler_1", "SELECT @p1", &params, &opts());
        assert_eq!(
            b,
            json!({
                "jobReference": {"projectId": "p", "jobId": "tabler_1", "location": "asia-southeast2"},
                "configuration": {
                    "query": {
                        "query": "SELECT @p1",
                        "useLegacySql": false,
                        "useQueryCache": true,
                        "parameterMode": "NAMED",
                        "queryParameters": params,
                        "defaultDataset": {"projectId": "p", "datasetId": "shop"},
                        "maximumBytesBilled": "1073741824",
                        "connectionProperties": [{"key": "session_id", "value": "CgwK"}]
                    },
                    "jobTimeoutMs": "60000",
                    "labels": {"app": "tabler"}
                }
            })
        );
        let plain = insert_body(
            "p",
            "j",
            "SELECT 1",
            &[],
            &QueryOptions {
                dry_run: true,
                create_session: true,
                ..QueryOptions::default()
            },
        );
        assert_eq!(plain["configuration"]["dryRun"], true);
        assert_eq!(plain["configuration"]["query"]["createSession"], true);
        assert!(
            plain["configuration"]["query"]
                .get("maximumBytesBilled")
                .is_none()
        );
        assert!(
            plain["configuration"]["query"]
                .get("parameterMode")
                .is_none()
        );
        assert!(plain["jobReference"].get("location").is_none());
    }

    #[test]
    fn query_bodies_ask_for_int64_timestamps() {
        let b = query_body("SELECT 1", &[], &opts(), Some(100), Some(10_000));
        assert_eq!(b["formatOptions"], json!({"useInt64Timestamp": true}));
        assert_eq!(b["location"], "asia-southeast2");
        assert_eq!(b["maxResults"], 100);
        assert_eq!(b["timeoutMs"], 10_000);
        assert_eq!(b["maximumBytesBilled"], "1073741824");
        assert_eq!(b["useLegacySql"], false);
        let id = new_job_id();
        assert!(id.starts_with("tabler_") && id.len() == 39, "{id}");
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
    }

    #[test]
    fn replies_read_int64_strings_and_dml_counts() {
        let r: QueryResponse = serde_json::from_value(json!({
            "kind": "bigquery#queryResponse",
            "jobReference": {"projectId": "p", "jobId": "j", "location": "US"},
            "totalRows": "12",
            "totalBytesProcessed": "1048576",
            "jobComplete": true,
            "numDmlAffectedRows": "3",
            "sessionInfo": {"sessionId": "S1"},
            "rows": [{"f": [{"v": "1"}]}]
        }))
        .unwrap();
        assert_eq!(r.total_rows, Some(12));
        assert_eq!(r.total_bytes_processed, Some(1_048_576));
        assert_eq!(r.affected(), Some(3));
        assert_eq!(r.session_id(), Some("S1"));
        assert!(r.complete());
        let stats: QueryResponse = serde_json::from_value(json!({
            "jobComplete": false,
            "dmlStats": {"insertedRowCount": "2", "deletedRowCount": "1"}
        }))
        .unwrap();
        assert!(!stats.complete());
        assert_eq!(stats.affected(), Some(3));
        let huge: QueryResponse = serde_json::from_value(json!({
            "dmlStats": {"insertedRowCount": u64::MAX.to_string(), "updatedRowCount": "5"}
        }))
        .unwrap();
        assert_eq!(huge.affected(), Some(u64::MAX));
        let junk: QueryResponse =
            serde_json::from_value(json!({"totalRows": "many", "rows": []})).unwrap();
        assert_eq!(junk.total_rows, None);
        assert_eq!(junk.affected(), None);
    }

    #[test]
    fn jobs_report_state_errors_and_sql() {
        let j: Job = serde_json::from_value(json!({
            "jobReference": {"projectId": "p", "jobId": "j"},
            "status": {"state": "DONE", "errorResult": {"reason": "stopped", "message": "x"},
                       "errors": [{"reason": "stopped", "message": "x"}]},
            "configuration": {"query": {"query": "SELECT 1"}},
            "statistics": {"creationTime": "1700000000000", "query": {"statementType": "SELECT"}}
        }))
        .unwrap();
        assert!(j.done());
        assert_eq!(
            j.error().map(|(e, _)| e.reason.as_deref()),
            Some(Some("stopped"))
        );
        assert_eq!(j.query_text(), Some("SELECT 1"));
        let listed: JobList = serde_json::from_value(json!({
            "jobs": [{"id": "p:US.j", "state": "RUNNING", "jobReference": {"projectId": "p", "jobId": "j"}}]
        }))
        .unwrap();
        assert_eq!(listed.jobs[0].state(), Some("RUNNING"));
        assert!(!listed.jobs[0].done());
    }
}
