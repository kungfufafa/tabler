//! Introspection through the free REST metadata calls (`projects.list`, `datasets.list`,
//! `tables.list`, `tables.get`, `routines.list`, `jobs.list`, `tabledata.list`) and what Tabler
//! makes of them. INFORMATION_SCHEMA reads are billed (10 MB at least each), so only Show DDL and
//! completion use them, on demand (`dialect::bigquery` builds them).
//!
//! Projects are databases, datasets schemas. There are no indexes: partitioning and clustering
//! stand in for them, and the primary and foreign keys of `tableConstraints` are listed as BigQuery
//! keeps them, NOT ENFORCED.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::decode::{TableSchema, column_meta, lenient_i64, lenient_u64};
use super::jobs::{DatasetReference, Job};
use crate::types::admin::ProcessRow;
use crate::types::objects::{RoutineKind, RoutineSummary};
use crate::types::schema::{
    DatabaseInfo, ForeignKeyMeta, IndexMeta, SchemaInfo, TableKind, TableMeta, TableRef,
    TableSummary,
};
use crate::types::value::Value;

/// The prefix of the jobs Tabler starts (`jobs::new_job_id`).
pub const OWN_JOB_PREFIX: &str = "tabler_";

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectReference {
    pub project_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectItem {
    pub id: Option<String>,
    pub project_reference: Option<ProjectReference>,
    pub friendly_name: Option<String>,
}

impl ProjectItem {
    pub fn project_id(&self) -> Option<&str> {
        self.project_reference
            .as_ref()
            .map(|r| r.project_id.as_str())
            .or(self.id.as_deref())
            .filter(|p| !p.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectList {
    pub projects: Vec<ProjectItem>,
    pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DatasetItem {
    pub dataset_reference: DatasetReference,
    pub location: Option<String>,
    pub friendly_name: Option<String>,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DatasetList {
    pub datasets: Vec<DatasetItem>,
    pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TableReference {
    pub project_id: String,
    pub dataset_id: String,
    pub table_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TableItem {
    pub table_reference: TableReference,
    /// `TABLE`, `VIEW`, `MATERIALIZED_VIEW`, `EXTERNAL`, `SNAPSHOT`.
    #[serde(rename = "type")]
    pub ty: Option<String>,
    pub friendly_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TableList {
    pub tables: Vec<TableItem>,
    pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QueryDefinition {
    pub query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TimePartitioning {
    /// `DAY`, `HOUR`, `MONTH`, `YEAR`.
    #[serde(rename = "type")]
    pub ty: Option<String>,
    pub field: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PartitionRange {
    #[serde(deserialize_with = "lenient_i64")]
    pub start: Option<i64>,
    #[serde(deserialize_with = "lenient_i64")]
    pub end: Option<i64>,
    #[serde(deserialize_with = "lenient_i64")]
    pub interval: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RangePartitioning {
    pub field: Option<String>,
    pub range: Option<PartitionRange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Clustering {
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrimaryKey {
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ColumnReference {
    pub referencing_column: String,
    pub referenced_column: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ForeignKey {
    pub name: Option<String>,
    pub referenced_table: TableReference,
    pub column_references: Vec<ColumnReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TableConstraints {
    pub primary_key: Option<PrimaryKey>,
    pub foreign_keys: Vec<ForeignKey>,
}

/// `tables.get`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Table {
    pub table_reference: TableReference,
    #[serde(rename = "type")]
    pub ty: Option<String>,
    pub schema: Option<TableSchema>,
    #[serde(deserialize_with = "lenient_u64")]
    pub num_rows: Option<u64>,
    #[serde(deserialize_with = "lenient_u64")]
    pub num_bytes: Option<u64>,
    pub description: Option<String>,
    pub view: Option<QueryDefinition>,
    pub materialized_view: Option<QueryDefinition>,
    pub time_partitioning: Option<TimePartitioning>,
    pub range_partitioning: Option<RangePartitioning>,
    pub clustering: Option<Clustering>,
    pub require_partition_filter: Option<bool>,
    pub table_constraints: Option<TableConstraints>,
    pub location: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StandardField {
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub ty: Option<StandardSqlDataType>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StandardStruct {
    pub fields: Vec<StandardField>,
}

/// A routine's argument or return type (`StandardSqlDataType`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StandardSqlDataType {
    pub type_kind: Option<String>,
    pub array_element_type: Option<Box<StandardSqlDataType>>,
    pub struct_type: Option<StandardStruct>,
    pub range_element_type: Option<Box<StandardSqlDataType>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RoutineArgument {
    pub name: Option<String>,
    pub data_type: Option<StandardSqlDataType>,
    /// `IN`, `OUT`, `INOUT` (procedures).
    pub mode: Option<String>,
    /// `FIXED_TYPE` or `ANY_TYPE` (templated).
    pub argument_kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RoutineReference {
    pub project_id: String,
    pub dataset_id: String,
    pub routine_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReturnTable {
    pub columns: Vec<StandardField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Routine {
    pub routine_reference: RoutineReference,
    /// `SCALAR_FUNCTION`, `PROCEDURE`, `TABLE_VALUED_FUNCTION`, `AGGREGATE_FUNCTION`.
    pub routine_type: Option<String>,
    /// `SQL`, `JAVASCRIPT`, `PYTHON`, `JAVA`, `SCALA`.
    pub language: Option<String>,
    pub arguments: Vec<RoutineArgument>,
    pub return_type: Option<StandardSqlDataType>,
    pub return_table_type: Option<ReturnTable>,
    pub definition_body: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RoutineList {
    pub routines: Vec<Routine>,
    pub next_page_token: Option<String>,
}

/// `tabledata.list`: a page of a table's rows, in the table's schema.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TableData {
    #[serde(deserialize_with = "lenient_u64")]
    pub total_rows: Option<u64>,
    pub page_token: Option<String>,
    pub rows: Vec<serde_json::Value>,
}

/// The sidebar's databases: the profile's project, the extra ones it lists, then the projects
/// the account can see, each once.
pub fn databases(current: &str, extra: &[String], listed: &[ProjectItem]) -> Vec<DatabaseInfo> {
    let mut names: Vec<&str> = vec![current];
    let more = extra
        .iter()
        .map(|p| p.trim())
        .chain(listed.iter().filter_map(ProjectItem::project_id));
    for p in more {
        if !p.is_empty() && !names.contains(&p) {
            names.push(p);
        }
    }
    names
        .into_iter()
        .map(|name| DatabaseInfo {
            name: name.to_string(),
            owner: None,
            encoding: None,
            is_current: name == current,
        })
        .collect()
}

/// A project's datasets, by name.
pub fn schemas(items: &[DatasetItem], default_dataset: Option<&str>) -> Vec<SchemaInfo> {
    let mut out: Vec<SchemaInfo> = items
        .iter()
        .map(|d| &d.dataset_reference.dataset_id)
        .filter(|d| !d.is_empty())
        .map(|d| SchemaInfo {
            name: d.clone(),
            is_default: Some(d.as_str()) == default_dataset,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn table_kind(ty: Option<&str>) -> TableKind {
    match ty.unwrap_or("TABLE") {
        "VIEW" => TableKind::View,
        "MATERIALIZED_VIEW" => TableKind::MaterializedView,
        "EXTERNAL" => TableKind::ForeignTable,
        // TABLE, SNAPSHOT (a read-only copy) and anything newer are tables.
        _ => TableKind::Table,
    }
}

/// A dataset's tables, by name (`tables.list` has no row counts).
pub fn table_summaries(items: &[TableItem], dataset: &str) -> Vec<TableSummary> {
    let mut out: Vec<TableSummary> = items
        .iter()
        .filter(|t| !t.table_reference.table_id.is_empty())
        .map(|t| TableSummary {
            name: t.table_reference.table_id.clone(),
            schema: Some(dataset.to_string()),
            kind: table_kind(t.ty.as_deref()),
            row_estimate: None,
            size_bytes: None,
            comment: t.friendly_name.clone().filter(|f| !f.is_empty()),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The structure of a table from `tables.get`. Another project's referenced table is named
/// `project.dataset` in its schema.
pub fn table_meta(t: &Table) -> TableMeta {
    let r = &t.table_reference;
    let fields = t
        .schema
        .as_ref()
        .map(|s| s.fields.as_slice())
        .unwrap_or_default();
    let pk: Vec<String> = t
        .table_constraints
        .as_ref()
        .and_then(|c| c.primary_key.as_ref())
        .map(|p| p.columns.clone())
        .unwrap_or_default();
    let columns = fields
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut c = column_meta(f, u32::try_from(i + 1).unwrap_or(u32::MAX));
            if let Some(pos) = pk.iter().position(|k| *k == f.name) {
                c.is_pk = true;
                c.pk_ordinal = u32::try_from(pos + 1).ok();
            }
            c
        })
        .collect();
    TableMeta {
        table: TableRef::new(Some(r.dataset_id.as_str()), &r.table_id),
        kind: table_kind(t.ty.as_deref()),
        columns,
        primary_key: pk.clone(),
        indexes: stand_in_indexes(t, &pk),
        foreign_keys: foreign_keys(t),
        triggers: Vec::new(),
        comment: t.description.clone().filter(|d| !d.is_empty()),
        row_estimate: t.num_rows.and_then(|n| i64::try_from(n).ok()),
        ddl: None,
        checks: Vec::new(),
        options: None,
    }
}

/// The primary key, partitioning and clustering, which take the place of indexes.
fn stand_in_indexes(t: &Table, pk: &[String]) -> Vec<IndexMeta> {
    let quote = |c: &String| crate::sqlgen::dialect::spec::backticked_backslash(c);
    let list = |cols: &[String]| cols.iter().map(quote).collect::<Vec<_>>().join(", ");
    let index =
        |name: &str, columns: Vec<String>, primary: bool, method: Option<String>, def: String| {
            IndexMeta {
                name: name.to_string(),
                columns,
                unique: false,
                primary,
                method,
                predicate: None,
                definition: Some(def),
                comment: None,
                include: Vec::new(),
            }
        };
    let mut out = Vec::new();
    if !pk.is_empty() {
        out.push(index(
            "PRIMARY KEY",
            pk.to_vec(),
            true,
            None,
            format!("PRIMARY KEY ({}) NOT ENFORCED", list(pk)),
        ));
    }
    if let Some(p) = &t.time_partitioning {
        let unit = p.ty.clone().unwrap_or_else(|| "DAY".into());
        let (columns, def) = match p.field.as_deref().filter(|f| !f.is_empty()) {
            Some(f) => {
                let col = quote(&f.to_string());
                (
                    vec![f.to_string()],
                    format!("PARTITION BY TIMESTAMP_TRUNC({col}, {unit})"),
                )
            }
            // Ingestion-time partitioning.
            None => (
                vec!["_PARTITIONTIME".to_string()],
                format!("PARTITION BY TIMESTAMP_TRUNC(_PARTITIONTIME, {unit})"),
            ),
        };
        let def = if t.require_partition_filter == Some(true) {
            format!("{def} (partition filter required)")
        } else {
            def
        };
        out.push(index("partitioning", columns, false, Some(unit), def));
    }
    if let Some(p) = &t.range_partitioning
        && let Some(f) = p.field.as_deref().filter(|f| !f.is_empty())
    {
        let r = p.range.clone().unwrap_or_default();
        let n = |v: Option<i64>| v.map_or("?".to_string(), |v| v.to_string());
        out.push(index(
            "partitioning",
            vec![f.to_string()],
            false,
            Some("RANGE".into()),
            format!(
                "PARTITION BY RANGE_BUCKET({}, GENERATE_ARRAY({}, {}, {}))",
                quote(&f.to_string()),
                n(r.start),
                n(r.end),
                n(r.interval)
            ),
        ));
    }
    if let Some(c) = &t.clustering
        && !c.fields.is_empty()
    {
        out.push(index(
            "clustering",
            c.fields.clone(),
            false,
            Some("CLUSTER".into()),
            format!("CLUSTER BY {}", list(&c.fields)),
        ));
    }
    out
}

fn foreign_keys(t: &Table) -> Vec<ForeignKeyMeta> {
    let Some(c) = &t.table_constraints else {
        return Vec::new();
    };
    let own = &t.table_reference.project_id;
    c.foreign_keys
        .iter()
        .enumerate()
        .map(|(i, fk)| {
            let rt = &fk.referenced_table;
            let schema = if rt.project_id.is_empty() || &rt.project_id == own {
                rt.dataset_id.clone()
            } else {
                format!("{}.{}", rt.project_id, rt.dataset_id)
            };
            ForeignKeyMeta {
                name: fk
                    .name
                    .clone()
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| format!("fk_{}", i + 1)),
                columns: fk
                    .column_references
                    .iter()
                    .map(|c| c.referencing_column.clone())
                    .collect(),
                ref_table: TableRef::new(Some(schema.as_str()), &rt.table_id),
                ref_columns: fk
                    .column_references
                    .iter()
                    .map(|c| c.referenced_column.clone())
                    .collect(),
                on_update: "NO ACTION".into(),
                on_delete: "NO ACTION".into(),
                definition: Some("NOT ENFORCED".into()),
            }
        })
        .collect()
}

/// The SQL of a view or materialized view.
pub fn view_definition(t: &Table) -> Option<String> {
    t.view
        .as_ref()
        .or(t.materialized_view.as_ref())
        .and_then(|v| v.query.clone())
}

/// A routine argument or return type as GoogleSQL spells it.
pub fn standard_type(t: &StandardSqlDataType) -> String {
    let kind = t.type_kind.as_deref().unwrap_or("ANY TYPE");
    match kind {
        "ARRAY" => match &t.array_element_type {
            Some(e) => format!("ARRAY<{}>", standard_type(e)),
            None => "ARRAY".into(),
        },
        "STRUCT" => match &t.struct_type {
            Some(s) => {
                let fields: Vec<String> = s
                    .fields
                    .iter()
                    .map(|f| {
                        let ty = f.ty.as_ref().map(standard_type).unwrap_or_default();
                        match f.name.as_deref().filter(|n| !n.is_empty()) {
                            Some(n) => format!("{n} {ty}"),
                            None => ty,
                        }
                    })
                    .collect();
                format!("STRUCT<{}>", fields.join(", "))
            }
            None => "STRUCT".into(),
        },
        "RANGE" => match &t.range_element_type {
            Some(e) => format!("RANGE<{}>", standard_type(e)),
            None => "RANGE".into(),
        },
        other => other.to_string(),
    }
}

/// A dataset's routines, by name.
pub fn routine_summaries(items: &[Routine], dataset: &str) -> Vec<RoutineSummary> {
    let mut out: Vec<RoutineSummary> = items
        .iter()
        .filter(|r| !r.routine_reference.routine_id.is_empty())
        .map(|r| {
            let kind = match r.routine_type.as_deref() {
                Some("PROCEDURE") => RoutineKind::Procedure,
                Some("AGGREGATE_FUNCTION") => RoutineKind::Aggregate,
                _ => RoutineKind::Function,
            };
            let arguments = r
                .arguments
                .iter()
                .map(|a| {
                    let ty = match (&a.data_type, a.argument_kind.as_deref()) {
                        (_, Some("ANY_TYPE")) => "ANY TYPE".to_string(),
                        (Some(t), _) => standard_type(t),
                        (None, _) => String::new(),
                    };
                    let mode = a
                        .mode
                        .as_deref()
                        .filter(|m| kind == RoutineKind::Procedure && *m != "IN")
                        .map(|m| format!("{m} "))
                        .unwrap_or_default();
                    format!("{mode}{} {ty}", a.name.as_deref().unwrap_or_default())
                        .trim()
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join(", ");
            let return_type = match (&r.return_type, &r.return_table_type) {
                (Some(t), _) => Some(standard_type(t)),
                (None, Some(table)) => Some(format!(
                    "TABLE<{}>",
                    table
                        .columns
                        .iter()
                        .map(|c| format!(
                            "{} {}",
                            c.name.as_deref().unwrap_or_default(),
                            c.ty.as_ref().map(standard_type).unwrap_or_default()
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
                (None, None) => None,
            };
            RoutineSummary {
                schema: Some(dataset.to_string()),
                name: r.routine_reference.routine_id.clone(),
                kind,
                arguments,
                identity_arguments: String::new(),
                return_type,
                language: r.language.clone(),
                comment: r.description.clone().filter(|d| !d.is_empty()),
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Completion's tables and columns from the rows of `completion_columns_query` (table name,
/// column name).
pub fn completion_columns(rows: &[Vec<Value>]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in rows {
        if let (Some(Value::Text(t)), Some(Value::Text(c))) = (row.first(), row.get(1)) {
            out.entry(t.clone()).or_default().push(c.clone());
        }
    }
    out
}

/// A process id: BigQuery's own full job id, `project:LOCATION.job_id`.
pub fn process_id(project: &str, location: Option<&str>, job_id: &str) -> String {
    match location.filter(|l| !l.is_empty()) {
        Some(l) => format!("{project}:{l}.{job_id}"),
        None => format!("{project}:{job_id}"),
    }
}

/// The project, location and job id of a process id. Domain-scoped projects
/// (`example.com:proj`) hold a colon of their own, so the last one separates the job.
pub fn parse_process_id(id: &str) -> Option<(String, Option<String>, String)> {
    let (project, rest) = id.rsplit_once(':')?;
    let (location, job) = match rest.split_once('.') {
        Some((l, j)) => (Some(l.to_string()), j),
        None => (None, rest),
    };
    if project.is_empty() || job.is_empty() {
        return None;
    }
    Some((project.to_string(), location, job.to_string()))
}

/// The process list: running and pending jobs. `now_ms`: milliseconds since the epoch.
pub fn process_rows(jobs: &[Job], now_ms: i64) -> Vec<ProcessRow> {
    jobs.iter()
        .filter_map(|j| {
            let r = j.job_reference.as_ref()?;
            if r.job_id.is_empty() {
                return None;
            }
            let stats = j.statistics.as_ref();
            let started = stats.and_then(|s| s.start_time.or(s.creation_time));
            Some(ProcessRow {
                id: process_id(&r.project_id, r.location.as_deref(), &r.job_id),
                user: j.user_email.clone(),
                database: Some(r.project_id.clone()),
                application: None,
                client: None,
                state: j.state().map(str::to_string),
                command: stats
                    .and_then(|s| s.query.as_ref())
                    .and_then(|q| q.statement_type.clone()),
                started_at: started
                    .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
                    .map(|t| t.format("%Y-%m-%d %H:%M:%S%.3f+00").to_string()),
                duration_ms: started.map(|s| now_ms.saturating_sub(s).max(0)),
                wait_event: None,
                query: j.query_text().map(str::to_string),
                current: false,
                own: r.job_id.starts_with(OWN_JOB_PREFIX),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::schema::ValueKind;
    use serde_json::json;

    #[test]
    fn projects_datasets_and_tables() {
        let projects: ProjectList = serde_json::from_value(json!({"projects": [
            {"id": "demo-proj", "projectReference": {"projectId": "demo-proj"}},
            {"id": "other", "projectReference": {"projectId": "other"}, "friendlyName": "O"},
            {"friendlyName": "no id"}
        ]}))
        .unwrap();
        let dbs = databases(
            "demo-proj",
            &[" bigquery-public-data ".into(), "other".into()],
            &projects.projects,
        );
        let names: Vec<&str> = dbs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["demo-proj", "bigquery-public-data", "other"]);
        assert!(dbs[0].is_current && !dbs[1].is_current);

        let ds: DatasetList = serde_json::from_value(json!({"datasets": [
            {"datasetReference": {"projectId": "p", "datasetId": "shop"}, "location": "US"},
            {"datasetReference": {"projectId": "p", "datasetId": "analytics_1"}},
            {"datasetReference": {"projectId": "p", "datasetId": ""}}
        ], "nextPageToken": "t"}))
        .unwrap();
        let s = schemas(&ds.datasets, Some("shop"));
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].name, "analytics_1");
        assert!(s[1].is_default);

        let tl: TableList = serde_json::from_value(json!({"tables": [
            {"tableReference": {"projectId": "p", "datasetId": "shop", "tableId": "orders"}, "type": "TABLE"},
            {"tableReference": {"projectId": "p", "datasetId": "shop", "tableId": "v_orders"}, "type": "VIEW"},
            {"tableReference": {"projectId": "p", "datasetId": "shop", "tableId": "mv"}, "type": "MATERIALIZED_VIEW"},
            {"tableReference": {"projectId": "p", "datasetId": "shop", "tableId": "ext"}, "type": "EXTERNAL"},
            {"tableReference": {"projectId": "p", "datasetId": "shop", "tableId": "snap"}, "type": "SNAPSHOT",
             "friendlyName": "Nightly"}
        ]}))
        .unwrap();
        let t = table_summaries(&tl.tables, "shop");
        let kinds: Vec<(&str, TableKind)> = t.iter().map(|t| (t.name.as_str(), t.kind)).collect();
        assert_eq!(
            kinds,
            [
                ("ext", TableKind::ForeignTable),
                ("mv", TableKind::MaterializedView),
                ("orders", TableKind::Table),
                ("snap", TableKind::Table),
                ("v_orders", TableKind::View)
            ]
        );
        assert_eq!(t[3].comment.as_deref(), Some("Nightly"));
        assert_eq!(t[0].schema.as_deref(), Some("shop"));
    }

    #[test]
    fn table_structure_with_keys_partitioning_and_clustering() {
        let t: Table = serde_json::from_value(json!({
            "tableReference": {"projectId": "p", "datasetId": "shop", "tableId": "orders"},
            "type": "TABLE",
            "numRows": "12345",
            "numBytes": "999",
            "description": "All orders",
            "schema": {"fields": [
                {"name": "id", "type": "INTEGER", "mode": "REQUIRED"},
                {"name": "customer_id", "type": "INTEGER"},
                {"name": "created_at", "type": "TIMESTAMP"},
                {"name": "items", "type": "RECORD", "mode": "REPEATED", "fields": [{"name": "sku", "type": "STRING"}]}
            ]},
            "timePartitioning": {"type": "DAY", "field": "created_at"},
            "requirePartitionFilter": true,
            "clustering": {"fields": ["customer_id"]},
            "tableConstraints": {
                "primaryKey": {"columns": ["id"]},
                "foreignKeys": [
                    {"name": "fk_customer", "referencedTable": {"projectId": "p", "datasetId": "shop", "tableId": "customers"},
                     "columnReferences": [{"referencingColumn": "customer_id", "referencedColumn": "id"}]},
                    {"referencedTable": {"projectId": "q", "datasetId": "crm", "tableId": "people"},
                     "columnReferences": [{"referencingColumn": "customer_id", "referencedColumn": "pid"}]}
                ]
            }
        }))
        .unwrap();
        let m = table_meta(&t);
        assert_eq!(m.table, TableRef::new(Some("shop"), "orders"));
        assert_eq!(m.primary_key, ["id"]);
        assert!(m.columns[0].is_pk && m.columns[0].pk_ordinal == Some(1));
        assert!(!m.columns[1].is_pk);
        assert_eq!(m.columns[3].data_type, "ARRAY<STRUCT<sku STRING>>");
        assert_eq!(m.columns[3].kind, ValueKind::Json);
        assert_eq!(m.row_estimate, Some(12345));
        assert_eq!(m.comment.as_deref(), Some("All orders"));
        let defs: Vec<&str> = m
            .indexes
            .iter()
            .filter_map(|i| i.definition.as_deref())
            .collect();
        assert_eq!(
            defs,
            [
                "PRIMARY KEY (`id`) NOT ENFORCED",
                "PARTITION BY TIMESTAMP_TRUNC(`created_at`, DAY) (partition filter required)",
                "CLUSTER BY `customer_id`"
            ]
        );
        assert!(m.indexes[0].primary && !m.indexes[0].unique);
        assert_eq!(m.foreign_keys.len(), 2);
        assert_eq!(
            m.foreign_keys[0].ref_table,
            TableRef::new(Some("shop"), "customers")
        );
        assert_eq!(m.foreign_keys[1].name, "fk_2");
        assert_eq!(
            m.foreign_keys[1].ref_table,
            TableRef::new(Some("q.crm"), "people")
        );
        assert_eq!(m.foreign_keys[1].ref_columns, ["pid"]);
        assert_eq!(view_definition(&t), None);

        let ingestion: Table = serde_json::from_value(json!({
            "tableReference": {"projectId": "p", "datasetId": "d", "tableId": "t"},
            "timePartitioning": {"type": "HOUR"},
            "rangePartitioning": {"field": "n", "range": {"start": "0", "end": "100", "interval": "10"}}
        }))
        .unwrap();
        let m = table_meta(&ingestion);
        assert!(m.columns.is_empty() && m.primary_key.is_empty());
        assert_eq!(
            m.indexes
                .iter()
                .filter_map(|i| i.definition.as_deref())
                .collect::<Vec<_>>(),
            [
                "PARTITION BY TIMESTAMP_TRUNC(_PARTITIONTIME, HOUR)",
                "PARTITION BY RANGE_BUCKET(`n`, GENERATE_ARRAY(0, 100, 10))"
            ]
        );
        let view: Table = serde_json::from_value(json!({
            "tableReference": {"projectId": "p", "datasetId": "d", "tableId": "v"},
            "type": "VIEW", "view": {"query": "SELECT 1", "useLegacySql": false}
        }))
        .unwrap();
        assert_eq!(table_meta(&view).kind, TableKind::View);
        assert_eq!(view_definition(&view).as_deref(), Some("SELECT 1"));
    }

    #[test]
    fn routines_with_their_signatures() {
        let rl: RoutineList = serde_json::from_value(json!({"routines": [
            {"routineReference": {"projectId": "p", "datasetId": "d", "routineId": "add_one"},
             "routineType": "SCALAR_FUNCTION", "language": "SQL",
             "arguments": [{"name": "x", "dataType": {"typeKind": "INT64"}}],
             "returnType": {"typeKind": "INT64"}},
            {"routineReference": {"projectId": "p", "datasetId": "d", "routineId": "load_all"},
             "routineType": "PROCEDURE",
             "arguments": [{"name": "n", "mode": "OUT", "dataType": {"typeKind": "INT64"}},
                           {"name": "tags", "mode": "IN", "dataType": {"typeKind": "ARRAY",
                             "arrayElementType": {"typeKind": "STRING"}}}]},
            {"routineReference": {"projectId": "p", "datasetId": "d", "routineId": "tvf"},
             "routineType": "TABLE_VALUED_FUNCTION",
             "arguments": [{"name": "v", "argumentKind": "ANY_TYPE"}],
             "returnTableType": {"columns": [{"name": "a", "type": {"typeKind": "STRUCT",
               "structType": {"fields": [{"name": "b", "type": {"typeKind": "DATE"}}]}}}]}}
        ]}))
        .unwrap();
        let r = routine_summaries(&rl.routines, "d");
        assert_eq!(r[0].name, "add_one");
        assert_eq!(r[0].arguments, "x INT64");
        assert_eq!(r[0].return_type.as_deref(), Some("INT64"));
        assert_eq!(r[1].kind, RoutineKind::Procedure);
        assert_eq!(r[1].arguments, "OUT n INT64, tags ARRAY<STRING>");
        assert_eq!(r[1].return_type, None);
        assert_eq!(r[2].arguments, "v ANY TYPE");
        assert_eq!(r[2].return_type.as_deref(), Some("TABLE<a STRUCT<b DATE>>"));
    }

    #[test]
    fn completion_and_processes() {
        let rows = vec![
            vec![Value::Text("orders".into()), Value::Text("id".into())],
            vec![Value::Text("orders".into()), Value::Text("total".into())],
            vec![Value::Text("users".into()), Value::Text("id".into())],
            vec![Value::Null, Value::Text("x".into())],
        ];
        let c = completion_columns(&rows);
        assert_eq!(c["orders"], ["id", "total"]);
        assert_eq!(c.len(), 2);

        assert_eq!(process_id("p", Some("US"), "j1"), "p:US.j1");
        assert_eq!(
            parse_process_id("example.com:proj:asia-southeast2.tabler_1"),
            Some((
                "example.com:proj".into(),
                Some("asia-southeast2".into()),
                "tabler_1".into()
            ))
        );
        assert_eq!(
            parse_process_id("p:j"),
            Some(("p".into(), None, "j".into()))
        );
        for bad in ["", "p", ":j", "p:"] {
            assert_eq!(parse_process_id(bad), None, "{bad}");
        }
        let jobs: super::super::jobs::JobList = serde_json::from_value(json!({"jobs": [
            {"jobReference": {"projectId": "p", "jobId": "tabler_x", "location": "US"},
             "state": "RUNNING", "user_email": "a@example.com",
             "statistics": {"startTime": "1700000000000", "query": {"statementType": "SELECT"}},
             "configuration": {"query": {"query": "SELECT 1"}}},
            {"jobReference": {"projectId": "p", "jobId": "bquxjob_1"}, "status": {"state": "PENDING"}},
            {"state": "RUNNING"}
        ]}))
        .unwrap();
        let p = process_rows(&jobs.jobs, 1_700_000_005_000);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].id, "p:US.tabler_x");
        assert!(p[0].own && !p[1].own);
        assert_eq!(p[0].user.as_deref(), Some("a@example.com"));
        assert_eq!(p[0].duration_ms, Some(5000));
        assert_eq!(
            p[0].started_at.as_deref(),
            Some("2023-11-14 22:13:20.000+00")
        );
        assert_eq!(p[0].query.as_deref(), Some("SELECT 1"));
        assert_eq!(p[0].command.as_deref(), Some("SELECT"));
        assert_eq!(p[1].state.as_deref(), Some("PENDING"));
    }
}
