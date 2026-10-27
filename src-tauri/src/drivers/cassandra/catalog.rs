//! Introspection of Cassandra and ScyllaDB: the `system` and `system_schema` queries (fixed text
//! with `?` binds for the keyspace and the table, and no column aliases, which DataStax Astra
//! refuses) and the parsers of their rows, as the driver decodes them untyped.
//!
//! A keyspace is a database (and the session's only schema). A table's columns come in key
//! order: the partition key by position, the clustering columns by position, then static and
//! regular columns by name — never all alphabetical, which hides the key.

use std::collections::BTreeMap;

use ::scylla::value::CqlValue;

use crate::drivers::cassandra::types::{CqlType, udt_name};
use crate::sqlgen::dialect::cql::{self, DdlColumn, DdlOption, IndexTarget, KeyRole};
use crate::sqlgen::dialect::spec::single_quoted;
use crate::types::schema::{
    ColumnMeta, CompletionMetadata, DatabaseInfo, IndexMeta, TableKind, TableMeta, TableRef,
    TableSummary, TriggerMeta, UserTypeInfo,
};

/// The cells of a row, in the order of the query's select list.
pub type Cells = [Option<CqlValue>];

/// A row as the driver decodes it untyped (`Row::columns`).
pub type CellRow = Vec<Option<CqlValue>>;

/// The node's own row (`system.local` always has the key `local`).
pub const LOCAL: &str = "SELECT release_version, cql_version, native_protocol_version, \
                         cluster_name, data_center, rack, partitioner FROM system.local WHERE \
                         key = 'local'";
/// ScyllaDB's version; Cassandra has no such table, so an error here means Cassandra.
pub const SCYLLA_VERSION: &str = "SELECT version FROM system.versions WHERE key = 'local'";
pub const PEERS: &str =
    "SELECT peer, data_center, rack, release_version, rpc_address FROM system.peers";
pub const KEYSPACES: &str =
    "SELECT keyspace_name, durable_writes, replication FROM system_schema.keyspaces";
/// Binds: keyspace.
pub const TABLES: &str =
    "SELECT table_name, comment FROM system_schema.tables WHERE keyspace_name = ?";
/// Binds: keyspace.
pub const VIEWS: &str =
    "SELECT view_name, base_table_name, comment FROM system_schema.views WHERE keyspace_name = ?";
/// The options a rebuilt `CREATE TABLE` shows, all present from Cassandra 3.0 and in ScyllaDB.
/// Binds: keyspace, table.
pub const TABLE_OPTIONS: &str = "SELECT comment, default_time_to_live, gc_grace_seconds, \
                                 bloom_filter_fp_chance, caching, compaction, compression, \
                                 speculative_retry FROM system_schema.tables WHERE \
                                 keyspace_name = ? AND table_name = ?";
/// Binds: keyspace, view.
pub const VIEW: &str = "SELECT base_table_name, where_clause, include_all_columns, comment FROM \
                        system_schema.views WHERE keyspace_name = ? AND view_name = ?";
/// Binds: keyspace, table (or view).
pub const COLUMNS: &str = "SELECT column_name, kind, position, clustering_order, type FROM \
                           system_schema.columns WHERE keyspace_name = ? AND table_name = ?";
/// Every column of a keyspace, for completion. Binds: keyspace.
pub const KEYSPACE_COLUMNS: &str =
    "SELECT table_name, column_name FROM system_schema.columns WHERE keyspace_name = ?";
/// Binds: keyspace, table.
pub const INDEXES: &str = "SELECT index_name, kind, options FROM system_schema.indexes WHERE \
                           keyspace_name = ? AND table_name = ?";
/// Binds: keyspace, table.
pub const TRIGGERS: &str = "SELECT trigger_name, options FROM system_schema.triggers WHERE \
                            keyspace_name = ? AND table_name = ?";
/// Binds: keyspace.
pub const TYPES: &str = "SELECT type_name, field_names, field_types FROM system_schema.types \
                         WHERE keyspace_name = ?";
/// The node's estimate per token range, summed for the table. Binds: keyspace, table.
pub const SIZE_ESTIMATES: &str = "SELECT partitions_count FROM system.size_estimates WHERE \
                                  keyspace_name = ? AND table_name = ?";

// Cell readers: a missing cell or one of another type reads as nothing, never a panic.

fn cell(row: &Cells, i: usize) -> Option<&CqlValue> {
    row.get(i).and_then(Option::as_ref)
}

fn text(row: &Cells, i: usize) -> Option<String> {
    match cell(row, i)? {
        CqlValue::Text(s) | CqlValue::Ascii(s) => Some(s.clone()),
        _ => None,
    }
}

fn int(row: &Cells, i: usize) -> Option<i64> {
    match cell(row, i)? {
        CqlValue::Int(n) => Some(i64::from(*n)),
        CqlValue::BigInt(n) => Some(*n),
        CqlValue::SmallInt(n) => Some(i64::from(*n)),
        CqlValue::TinyInt(n) => Some(i64::from(*n)),
        CqlValue::Counter(c) => Some(c.0),
        _ => None,
    }
}

fn boolean(row: &Cells, i: usize) -> Option<bool> {
    match cell(row, i)? {
        CqlValue::Boolean(b) => Some(*b),
        _ => None,
    }
}

fn plain(v: &CqlValue) -> Option<String> {
    match v {
        CqlValue::Text(s) | CqlValue::Ascii(s) => Some(s.clone()),
        _ => None,
    }
}

/// A `map<text, text>` cell, in the server's (sorted) order.
fn text_map(row: &Cells, i: usize) -> Vec<(String, String)> {
    match cell(row, i) {
        Some(CqlValue::Map(pairs)) => pairs
            .iter()
            .filter_map(|(k, v)| Some((plain(k)?, plain(v)?)))
            .collect(),
        _ => Vec::new(),
    }
}

/// A `list<text>` or `set<text>` cell.
fn text_list(row: &Cells, i: usize) -> Vec<String> {
    match cell(row, i) {
        Some(CqlValue::List(items) | CqlValue::Set(items)) => {
            items.iter().filter_map(plain).collect()
        }
        _ => Vec::new(),
    }
}

/// The node, from `LOCAL`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalNode {
    pub release_version: Option<String>,
    pub cql_version: Option<String>,
    pub native_protocol_version: Option<String>,
    pub cluster_name: Option<String>,
    pub data_center: Option<String>,
    pub rack: Option<String>,
    pub partitioner: Option<String>,
}

pub fn parse_local(rows: &[CellRow]) -> Option<LocalNode> {
    let row = rows.first()?;
    Some(LocalNode {
        release_version: text(row, 0),
        cql_version: text(row, 1),
        native_protocol_version: text(row, 2),
        cluster_name: text(row, 3),
        data_center: text(row, 4),
        rack: text(row, 5),
        partitioner: text(row, 6),
    })
}

/// ScyllaDB's version, from `SCYLLA_VERSION`.
pub fn parse_scylla_version(rows: &[CellRow]) -> Option<String> {
    rows.first().and_then(|r| text(r, 0))
}

/// A peer of the node, from `PEERS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub peer: String,
    pub data_center: Option<String>,
    pub rack: Option<String>,
    pub release_version: Option<String>,
    pub rpc_address: Option<String>,
}

pub fn parse_peers(rows: &[CellRow]) -> Vec<Peer> {
    let inet = |row: &Cells, i| match cell(row, i) {
        Some(CqlValue::Inet(ip)) => Some(ip.to_string()),
        _ => None,
    };
    rows.iter()
        .filter_map(|row| {
            Some(Peer {
                peer: inet(row, 0)?,
                data_center: text(row, 1),
                rack: text(row, 2),
                release_version: text(row, 3),
                rpc_address: inet(row, 4),
            })
        })
        .collect()
}

/// A keyspace, from `KEYSPACES`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keyspace {
    pub name: String,
    pub durable_writes: Option<bool>,
    /// `class` and the replication factors (`replication_factor`, or one per datacenter).
    pub replication: Vec<(String, String)>,
}

pub fn parse_keyspaces(rows: &[CellRow]) -> Vec<Keyspace> {
    let mut out: Vec<Keyspace> = rows
        .iter()
        .filter_map(|row| {
            Some(Keyspace {
                name: text(row, 0)?,
                durable_writes: boolean(row, 1),
                replication: text_map(row, 2),
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The keyspaces of the server itself, of DSE and of the managed services, hidden from the
/// keyspace list.
pub fn is_system_keyspace(name: &str) -> bool {
    name == "system"
        || name.starts_with("system_")
        || name.starts_with("dse_")
        || name == "solr_admin"
        || name == "OpsCenter"
}

/// The databases a user picks from: the user keyspaces, and the current one even when it is a
/// system keyspace.
pub fn databases(keyspaces: &[Keyspace], current: Option<&str>) -> Vec<DatabaseInfo> {
    keyspaces
        .iter()
        .filter(|k| !is_system_keyspace(&k.name) || Some(k.name.as_str()) == current)
        .map(|k| DatabaseInfo {
            name: k.name.clone(),
            owner: None,
            encoding: None,
            is_current: Some(k.name.as_str()) == current,
        })
        .collect()
}

/// Tables (`TABLES`) and materialized views (`VIEWS`) of a keyspace, by name.
pub fn parse_tables(keyspace: &str, tables: &[CellRow], views: &[CellRow]) -> Vec<TableSummary> {
    let summary = |row: &Cells, kind| {
        Some(TableSummary {
            name: text(row, 0)?,
            schema: Some(keyspace.to_string()),
            kind,
            row_estimate: None,
            size_bytes: None,
            comment: text(row, if kind == TableKind::Table { 1 } else { 2 })
                .filter(|c| !c.is_empty()),
        })
    };
    let mut out: Vec<TableSummary> = tables
        .iter()
        .filter_map(|r| summary(r, TableKind::Table))
        .chain(
            views
                .iter()
                .filter_map(|r| summary(r, TableKind::MaterializedView)),
        )
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A column of a table or view, from `COLUMNS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaColumn {
    pub name: String,
    pub role: KeyRole,
    /// Its place in the partition key or among the clustering columns.
    pub position: i64,
    /// The server's type text (`frozen<list<text>>`).
    pub cql_type: String,
}

/// The columns in table order (see the module doc). A compact table's hidden column (no name,
/// type `empty`) is left out.
pub fn parse_columns(rows: &[CellRow]) -> Vec<SchemaColumn> {
    let mut cols: Vec<SchemaColumn> = rows
        .iter()
        .filter_map(|row| {
            let name = text(row, 0)?;
            let cql_type = text(row, 4)?;
            if name.is_empty() && cql_type == "empty" {
                return None;
            }
            let role = match text(row, 1)?.as_str() {
                "partition_key" => KeyRole::Partition,
                "clustering" => KeyRole::Clustering {
                    desc: text(row, 3).is_some_and(|o| o.eq_ignore_ascii_case("desc")),
                },
                "static" => KeyRole::Static,
                _ => KeyRole::Regular,
            };
            Some(SchemaColumn {
                name,
                role,
                position: int(row, 2).unwrap_or(-1),
                cql_type,
            })
        })
        .collect();
    let rank = |r: &KeyRole| match r {
        KeyRole::Partition => 0,
        KeyRole::Clustering { .. } => 1,
        KeyRole::Static => 2,
        KeyRole::Regular => 3,
    };
    cols.sort_by(|a, b| {
        let key = |c: &SchemaColumn| {
            let by_position = matches!(c.role, KeyRole::Partition | KeyRole::Clustering { .. });
            (rank(&c.role), if by_position { c.position } else { 0 })
        };
        key(a).cmp(&key(b)).then_with(|| a.name.cmp(&b.name))
    });
    cols
}

/// The columns as `ColumnMeta` and the primary key (partition then clustering columns).
pub fn column_metas(cols: &[SchemaColumn]) -> (Vec<ColumnMeta>, Vec<String>) {
    let mut primary_key = Vec::new();
    let metas = cols
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let is_pk = matches!(c.role, KeyRole::Partition | KeyRole::Clustering { .. });
            if is_pk {
                primary_key.push(c.name.clone());
            }
            let extra = match c.role {
                KeyRole::Partition => Some("partition key".to_string()),
                KeyRole::Clustering { desc } => {
                    Some(format!("clustering {}", if desc { "DESC" } else { "ASC" }))
                }
                KeyRole::Static => Some("static".to_string()),
                KeyRole::Regular => None,
            };
            let ty = CqlType::parse(&c.cql_type);
            ColumnMeta {
                name: c.name.clone(),
                ordinal: u32::try_from(i + 1).unwrap_or(u32::MAX),
                data_type: c.cql_type.clone(),
                kind: ty.kind(),
                // Key columns are never null; every other cell may be missing.
                nullable: !is_pk,
                default_expr: None,
                is_pk,
                pk_ordinal: is_pk.then(|| u32::try_from(primary_key.len()).unwrap_or(u32::MAX)),
                is_auto_increment: false,
                is_generated: false,
                is_unsigned: false,
                comment: None,
                enum_values: None,
                char_max_len: None,
                numeric_precision: None,
                numeric_scale: None,
                collation: None,
                array_elem_type: match ty.unfrozen() {
                    CqlType::List(e) | CqlType::Set(e) | CqlType::Vector(e, _) => {
                        Some(type_text(e))
                    }
                    _ => None,
                },
                extra,
                base_type: None,
            }
        })
        .collect();
    (metas, primary_key)
}

/// A parsed type back as text (element types of collections).
fn type_text(t: &CqlType) -> String {
    let list = |items: &[&CqlType]| -> String {
        items
            .iter()
            .map(|t| type_text(t))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match t {
        CqlType::Native(n) => n.clone(),
        CqlType::List(e) => format!("list<{}>", type_text(e)),
        CqlType::Set(e) => format!("set<{}>", type_text(e)),
        CqlType::Map(k, v) => format!("map<{}>", list(&[k.as_ref(), v.as_ref()])),
        CqlType::Tuple(items) => format!("tuple<{}>", list(&items.iter().collect::<Vec<_>>())),
        CqlType::Vector(e, n) => format!("vector<{}, {n}>", type_text(e)),
        CqlType::Frozen(e) => format!("frozen<{}>", type_text(e)),
        CqlType::Udt(n) => n.clone(),
        CqlType::Custom(c) => c.clone(),
    }
}

/// The index target of `options.target`: `c`, `"Mixed"`, `keys(m)`, `values(m)`, `entries(m)`,
/// `full(l)`, or ScyllaDB's JSON of a local index (`{"pk":["p"],"ck":["c"]}`).
pub fn parse_index_target(target: &str) -> IndexTarget {
    let target = target.trim();
    if target.starts_with('{')
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(target)
    {
        let names = |k: &str| -> Vec<String> {
            json.get(k)
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|n| n.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        return IndexTarget::Local {
            partition: names("pk"),
            columns: names("ck"),
        };
    }
    for part in ["keys", "values", "entries", "full"] {
        if let Some(inner) = target
            .strip_prefix(part)
            .and_then(|r| r.strip_prefix('('))
            .and_then(|r| r.strip_suffix(')'))
        {
            return IndexTarget::Column {
                part: Some(part),
                column: unquote(inner.trim()),
            };
        }
    }
    IndexTarget::Column {
        part: None,
        column: unquote(target),
    }
}

/// A CQL name: `"Mixed""Case"` unquoted, a bare name as it is.
fn unquote(name: &str) -> String {
    match name
        .strip_prefix('"')
        .and_then(|n| n.strip_suffix('"'))
        .filter(|_| name.len() >= 2)
    {
        Some(inner) => inner.replace("\"\"", "\""),
        None => name.to_string(),
    }
}

/// The indexes of a table, from `INDEXES`.
pub fn parse_indexes(keyspace: &str, table: &str, rows: &[CellRow]) -> Vec<IndexMeta> {
    let mut out: Vec<IndexMeta> = rows
        .iter()
        .filter_map(|row| {
            let name = text(row, 0)?;
            let kind = text(row, 1).unwrap_or_default();
            let options = text_map(row, 2);
            let option = |k: &str| {
                options
                    .iter()
                    .find(|(name, _)| name == k)
                    .map(|(_, v)| v.as_str())
            };
            let target = parse_index_target(option("target").unwrap_or_default());
            let class = option("class_name").filter(|_| kind.eq_ignore_ascii_case("CUSTOM"));
            let method = match (class, &target) {
                (Some(c), _) if c.ends_with("StorageAttachedIndex") => "sai".to_string(),
                (Some(c), _) if c.ends_with("SASIIndex") => "sasi".to_string(),
                (Some(c), _) => c.to_string(),
                (None, IndexTarget::Local { .. }) => "local secondary".to_string(),
                (None, _) => "secondary".to_string(),
            };
            let others: Vec<(String, String)> = options
                .iter()
                .filter(|(k, _)| k != "target" && k != "class_name")
                .cloned()
                .collect();
            Some(IndexMeta {
                definition: Some(cql::create_index(
                    keyspace, table, &name, &target, class, &others,
                )),
                name,
                columns: target.columns(),
                unique: false,
                primary: false,
                method: Some(method),
                predicate: None,
                comment: None,
                include: Vec::new(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The triggers of a table, from `TRIGGERS`.
pub fn parse_triggers(keyspace: &str, table: &str, rows: &[CellRow]) -> Vec<TriggerMeta> {
    rows.iter()
        .filter_map(|row| {
            let name = text(row, 0)?;
            let class = text_map(row, 1)
                .into_iter()
                .find(|(k, _)| k == "class")
                .map(|(_, v)| v)
                .unwrap_or_default();
            Some(TriggerMeta {
                definition: cql::create_trigger(keyspace, table, &name, &class),
                name,
                timing: None,
                events: Vec::new(),
                enabled: true,
            })
        })
        .collect()
}

/// The table's comment and the options of its rebuilt `CREATE TABLE`, from `TABLE_OPTIONS`.
pub fn parse_table_options(rows: &[CellRow]) -> (Option<String>, Vec<DdlOption>) {
    let Some(row) = rows.first() else {
        return (None, Vec::new());
    };
    let comment = text(row, 0);
    let mut options = Vec::new();
    let mut push = |name: &str, value: Option<String>| {
        if let Some(value) = value {
            options.push(DdlOption {
                name: name.into(),
                value,
            });
        }
    };
    let number = |i| match cell(row, i) {
        Some(CqlValue::Double(f)) if f.is_finite() => Some(f.to_string()),
        Some(CqlValue::Float(f)) if f.is_finite() => Some(f.to_string()),
        _ => int(row, i).map(|n| n.to_string()),
    };
    let map = |i| {
        let pairs = text_map(row, i);
        (!pairs.is_empty()).then(|| cql::map_literal(&pairs))
    };
    push("bloom_filter_fp_chance", number(3));
    push("caching", map(4));
    push("comment", comment.as_deref().map(single_quoted));
    push("compaction", map(5));
    push("compression", map(6));
    push("default_time_to_live", number(1));
    push("gc_grace_seconds", number(2));
    push("speculative_retry", text(row, 7).map(|s| single_quoted(&s)));
    (comment.filter(|c| !c.is_empty()), options)
}

/// A view's base table, filter, whether it selects every column, and comment, from `VIEW`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewInfo {
    pub base_table: String,
    pub where_clause: String,
    pub include_all_columns: bool,
    pub comment: Option<String>,
}

pub fn parse_view(rows: &[CellRow]) -> Option<ViewInfo> {
    let row = rows.first()?;
    Some(ViewInfo {
        base_table: text(row, 0)?,
        where_clause: text(row, 1).unwrap_or_default(),
        include_all_columns: boolean(row, 2).unwrap_or(false),
        comment: text(row, 3).filter(|c| !c.is_empty()),
    })
}

/// The partitions of a table the node estimates, summed over its token ranges, from
/// `SIZE_ESTIMATES`; None when the node has no estimate yet.
pub fn parse_size_estimates(rows: &[CellRow]) -> Option<i64> {
    let counts: Vec<i64> = rows.iter().filter_map(|r| int(r, 0)).collect();
    (!counts.is_empty()).then(|| counts.iter().fold(0i64, |a, n| a.saturating_add(*n)))
}

/// The DDL of a table or view rebuilt from its columns (in table order) and options.
pub fn rebuilt_ddl(
    table: &TableRef,
    cols: &[SchemaColumn],
    options: &[DdlOption],
    view: Option<&ViewInfo>,
) -> String {
    let keyspace = table.schema.as_deref().unwrap_or_default();
    let ddl_cols: Vec<DdlColumn<'_>> = cols
        .iter()
        .map(|c| DdlColumn {
            name: &c.name,
            cql_type: &c.cql_type,
            role: c.role,
        })
        .collect();
    match view {
        Some(v) => cql::create_view(
            keyspace,
            &table.name,
            &v.base_table,
            &ddl_cols,
            v.include_all_columns,
            &v.where_clause,
        ),
        None => cql::create_table(keyspace, &table.name, &ddl_cols, options),
    }
}

/// The rows of a table's introspection queries.
#[derive(Default)]
pub struct TableRows<'a> {
    pub columns: &'a [CellRow],
    pub options: &'a [CellRow],
    pub indexes: &'a [CellRow],
    pub triggers: &'a [CellRow],
    pub size_estimates: &'a [CellRow],
    /// `VIEW` rows: set for a materialized view.
    pub view: Option<&'a [CellRow]>,
}

/// A table's (or materialized view's) structure. `ddl`, when the server described it
/// (`DESCRIBE`), is used as it is; otherwise it is rebuilt from the rows.
pub fn table_meta(table: &TableRef, rows: &TableRows<'_>, ddl: Option<String>) -> TableMeta {
    let keyspace = table.schema.as_deref().unwrap_or_default();
    let cols = parse_columns(rows.columns);
    let (columns, primary_key) = column_metas(&cols);
    let (comment, options) = parse_table_options(rows.options);
    let view = rows.view.and_then(parse_view);
    let comment = view.as_ref().map_or(comment, |v| v.comment.clone());
    let ddl = ddl.or_else(|| Some(rebuilt_ddl(table, &cols, &options, view.as_ref())));
    TableMeta {
        table: table.clone(),
        kind: if rows.view.is_some() {
            TableKind::MaterializedView
        } else {
            TableKind::Table
        },
        columns,
        primary_key,
        indexes: parse_indexes(keyspace, &table.name, rows.indexes),
        foreign_keys: Vec::new(),
        triggers: parse_triggers(keyspace, &table.name, rows.triggers),
        comment,
        row_estimate: parse_size_estimates(rows.size_estimates),
        ddl,
        checks: Vec::new(),
        options: None,
    }
}

/// The text of a server-side `DESCRIBE` (rows of `keyspace_name, type, name,
/// create_statement`): the statements, one after the other.
pub fn parse_describe(rows: &[CellRow]) -> Option<String> {
    let parts: Vec<String> = rows.iter().filter_map(|r| text(r, 3)).collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// Completion for the current keyspace, from `KEYSPACE_COLUMNS`.
pub fn completion(keyspace: &str, rows: &[CellRow]) -> CompletionMetadata {
    let mut tables: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in rows {
        if let (Some(t), Some(c)) = (text(row, 0), text(row, 1))
            && !c.is_empty()
        {
            tables.entry(t).or_default().push(c);
        }
    }
    let mut schemas = BTreeMap::new();
    schemas.insert(keyspace.to_string(), tables);
    CompletionMetadata {
        default_schema: Some(keyspace.to_string()),
        schemas,
    }
}

/// A user-defined type, from `TYPES`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Udt {
    pub name: String,
    /// (name, type text) in declaration order.
    pub fields: Vec<(String, String)>,
}

pub fn parse_types(rows: &[CellRow]) -> Vec<Udt> {
    let mut out: Vec<Udt> = rows
        .iter()
        .filter_map(|row| {
            let names = text_list(row, 1);
            let types = text_list(row, 2);
            Some(Udt {
                name: text(row, 0)?,
                fields: names.into_iter().zip(types).collect(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The UDTs as the column types the structure editor offers.
pub fn user_types(udts: &[Udt]) -> Vec<UserTypeInfo> {
    udts.iter()
        .map(|u| UserTypeInfo {
            name: format!("frozen<{}>", udt_name(&u.name)),
            kind: "composite".into(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::cassandra::testing::{self, b, events_columns, i, list, map, t};
    use crate::types::schema::ValueKind;

    #[test]
    fn columns_come_in_key_order() {
        let rows = events_columns();
        let cols = parse_columns(&rows);
        let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
        // Partition key by position, clustering by position, static, then regular by name.
        assert_eq!(
            names,
            [
                "user_id", "bucket", "ts", "seq", "owner", "amount", "tags", "z_note"
            ]
        );
        let (metas, pk) = column_metas(&cols);
        assert_eq!(pk, ["user_id", "bucket", "ts", "seq"]);
        let ords: Vec<Option<u32>> = metas.iter().map(|c| c.pk_ordinal).collect();
        assert_eq!(
            ords,
            [Some(1), Some(2), Some(3), Some(4), None, None, None, None]
        );
        assert_eq!(metas[0].extra.as_deref(), Some("partition key"));
        assert_eq!(metas[2].extra.as_deref(), Some("clustering DESC"));
        assert_eq!(metas[3].extra.as_deref(), Some("clustering ASC"));
        assert_eq!(metas[4].extra.as_deref(), Some("static"));
        assert_eq!(metas[5].extra, None);
        assert!(!metas[0].nullable && metas[5].nullable);
        assert_eq!(metas[0].kind, ValueKind::Uuid);
        assert_eq!(metas[2].kind, ValueKind::TimestampTz);
        assert_eq!(metas[5].kind, ValueKind::Decimal);
        assert_eq!(metas[6].data_type, "frozen<list<text>>");
        assert_eq!(metas[6].kind, ValueKind::Json);
        assert_eq!(metas[6].array_elem_type.as_deref(), Some("text"));
        assert_eq!(
            metas.iter().map(|c| c.ordinal).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5, 6, 7, 8]
        );
    }

    #[test]
    fn compact_tables_hide_their_empty_column() {
        let rows = vec![
            vec![t("k"), t("partition_key"), i(0), t("none"), t("int")],
            vec![t(""), t("regular"), i(-1), t("none"), t("empty")],
        ];
        let cols = parse_columns(&rows);
        assert_eq!(cols.len(), 1);
        // Junk cells read as nothing rather than panicking.
        let junk = vec![vec![i(1)], vec![], vec![t("x"), b(true), t("y")]];
        assert!(parse_columns(&junk).is_empty());
    }

    #[test]
    fn keyspaces_hide_the_system_ones() {
        let rows = vec![
            vec![
                t("system"),
                b(true),
                map(&[("class", "org.apache.cassandra.locator.LocalStrategy")]),
            ],
            vec![t("system_auth"), b(true), map(&[])],
            vec![t("dse_perf"), b(true), map(&[])],
            vec![
                t("shop"),
                b(true),
                map(&[
                    (
                        "class",
                        "org.apache.cassandra.locator.NetworkTopologyStrategy",
                    ),
                    ("dc1", "3"),
                ]),
            ],
            vec![t("Analytics"), b(false), map(&[])],
        ];
        let ks = parse_keyspaces(&rows);
        let shop = ks.iter().find(|k| k.name == "shop").unwrap();
        assert_eq!(shop.replication[1], ("dc1".to_string(), "3".to_string()));
        let dbs = databases(&ks, Some("shop"));
        let names: Vec<(&str, bool)> = dbs
            .iter()
            .map(|d| (d.name.as_str(), d.is_current))
            .collect();
        assert_eq!(names, [("Analytics", false), ("shop", true)]);
        // The current keyspace shows even when it is a system one.
        assert!(
            databases(&ks, Some("system"))
                .iter()
                .any(|d| d.name == "system")
        );
        assert!(is_system_keyspace("system_virtual_schema") && !is_system_keyspace("systems"));
    }

    #[test]
    fn tables_and_views() {
        let tables = vec![vec![t("orders"), t("all orders")], vec![t("events"), t("")]];
        let views = vec![vec![t("orders_by_day"), t("orders"), t("")]];
        let list = parse_tables("shop", &tables, &views);
        let got: Vec<(&str, TableKind, Option<&str>)> = list
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.comment.as_deref()))
            .collect();
        assert_eq!(
            got,
            [
                ("events", TableKind::Table, None),
                ("orders", TableKind::Table, Some("all orders")),
                ("orders_by_day", TableKind::MaterializedView, None),
            ]
        );
        assert!(list.iter().all(|s| s.schema.as_deref() == Some("shop")));
    }

    #[test]
    fn index_targets() {
        let col = |part, c: &str| IndexTarget::Column {
            part,
            column: c.into(),
        };
        assert_eq!(parse_index_target("email"), col(None, "email"));
        assert_eq!(
            parse_index_target("\"Mixed\"\"Case\""),
            col(None, "Mixed\"Case")
        );
        assert_eq!(
            parse_index_target("keys(attrs)"),
            col(Some("keys"), "attrs")
        );
        assert_eq!(
            parse_index_target("values(\"M\")"),
            col(Some("values"), "M")
        );
        assert_eq!(parse_index_target("full(l)"), col(Some("full"), "l"));
        assert_eq!(
            parse_index_target("{\"pk\":[\"p\"],\"ck\":[\"c\"]}"),
            IndexTarget::Local {
                partition: vec!["p".into()],
                columns: vec!["c".into()]
            }
        );
        assert_eq!(parse_index_target("\""), col(None, "\""));
    }

    #[test]
    fn indexes_and_triggers() {
        let rows = vec![
            vec![
                t("events_by_owner"),
                t("COMPOSITES"),
                map(&[("target", "owner")]),
            ],
            vec![
                t("events_tags"),
                t("CUSTOM"),
                map(&[
                    (
                        "class_name",
                        "org.apache.cassandra.index.sai.StorageAttachedIndex",
                    ),
                    ("target", "values(tags)"),
                ]),
            ],
            vec![
                t("events_note"),
                t("CUSTOM"),
                map(&[
                    ("class_name", "org.apache.cassandra.index.sasi.SASIIndex"),
                    ("mode", "CONTAINS"),
                    ("target", "z_note"),
                ]),
            ],
        ];
        let idx = parse_indexes("shop", "events", &rows);
        let got: Vec<(&str, Vec<String>, Option<&str>)> = idx
            .iter()
            .map(|i| (i.name.as_str(), i.columns.clone(), i.method.as_deref()))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "events_by_owner",
                    vec!["owner".to_string()],
                    Some("secondary")
                ),
                ("events_note", vec!["z_note".to_string()], Some("sasi")),
                ("events_tags", vec!["tags".to_string()], Some("sai")),
            ]
        );
        assert_eq!(
            idx[1].definition.as_deref(),
            Some(
                "CREATE CUSTOM INDEX \"events_note\" ON \"shop\".\"events\" (\"z_note\") USING \
                 'org.apache.cassandra.index.sasi.SASIIndex' WITH OPTIONS = {'mode': 'CONTAINS'};"
            )
        );
        assert_eq!(
            idx[0].definition.as_deref(),
            Some("CREATE INDEX \"events_by_owner\" ON \"shop\".\"events\" (\"owner\");")
        );
        let triggers = vec![vec![t("audit"), map(&[("class", "org.example.Audit")])]];
        let tr = parse_triggers("shop", "events", &triggers);
        assert_eq!(
            tr[0].definition,
            "CREATE TRIGGER \"audit\" ON \"shop\".\"events\" USING 'org.example.Audit';"
        );
    }

    #[test]
    fn table_meta_from_fixture_rows() {
        let fx = testing::events_fixture();
        let table = TableRef::new(Some("shop"), "events");
        let meta = table_meta(&table, &fx.rows(), None);
        assert_eq!(meta.kind, TableKind::Table);
        assert_eq!(meta.primary_key, ["user_id", "bucket", "ts", "seq"]);
        assert_eq!(meta.comment.as_deref(), Some("user events"));
        // 120 + 80, and a count beyond i64 saturates rather than wraps.
        assert_eq!(meta.row_estimate, Some(200));
        assert_eq!(meta.indexes.len(), 1);
        let ddl = meta.ddl.unwrap();
        assert!(
            ddl.starts_with(
                "CREATE TABLE \"shop\".\"events\" (\n    \"user_id\" uuid,\n    \"bucket\" int,"
            ),
            "{ddl}"
        );
        assert!(ddl.contains(
            "    PRIMARY KEY ((\"user_id\", \"bucket\"), \"ts\", \"seq\")\n) WITH CLUSTERING \
             ORDER BY (\"ts\" DESC, \"seq\" ASC)\n    AND bloom_filter_fp_chance = 0.01\n    AND \
             caching = {'keys': 'ALL', 'rows_per_partition': 'NONE'}\n    AND comment = 'user \
             events'"
        ));
        assert!(ddl.contains("AND default_time_to_live = 0\n    AND gc_grace_seconds = 864000"));
        assert!(ddl.ends_with("AND speculative_retry = '99p';"), "{ddl}");
        // A DESCRIBE text wins over the rebuilt one.
        let described = table_meta(&table, &fx.rows(), Some("CREATE TABLE x;".into()));
        assert_eq!(described.ddl.as_deref(), Some("CREATE TABLE x;"));
        assert_eq!(
            parse_size_estimates(&[vec![i(i64::MAX)], vec![i(5)]]),
            Some(i64::MAX)
        );
        assert_eq!(parse_size_estimates(&[]), None);
    }

    #[test]
    fn views_rebuild_as_materialized_views() {
        let cols = vec![
            vec![t("day"), t("partition_key"), i(0), t("none"), t("date")],
            vec![t("id"), t("clustering"), i(0), t("asc"), t("uuid")],
            vec![t("total"), t("regular"), i(-1), t("none"), t("decimal")],
        ];
        let view = vec![vec![
            t("orders"),
            t("day IS NOT NULL AND id IS NOT NULL"),
            b(true),
            t("by day"),
        ]];
        let rows = TableRows {
            columns: &cols,
            view: Some(&view),
            ..TableRows::default()
        };
        let meta = table_meta(&TableRef::new(Some("shop"), "orders_by_day"), &rows, None);
        assert_eq!(meta.kind, TableKind::MaterializedView);
        assert_eq!(meta.comment.as_deref(), Some("by day"));
        assert_eq!(
            meta.ddl.as_deref(),
            Some(
                "CREATE MATERIALIZED VIEW \"shop\".\"orders_by_day\" AS\n    SELECT *\n    FROM \
                 \"shop\".\"orders\"\n    WHERE day IS NOT NULL AND id IS NOT NULL\n    PRIMARY \
                 KEY ((\"day\"), \"id\") WITH CLUSTERING ORDER BY (\"id\" ASC);"
            )
        );
    }

    #[test]
    fn server_rows() {
        let local = vec![vec![
            t("4.1.3"),
            t("3.4.6"),
            t("5"),
            t("Test Cluster"),
            t("dc1"),
            t("rack1"),
            t("org.apache.cassandra.dht.Murmur3Partitioner"),
        ]];
        let node = parse_local(&local).unwrap();
        assert_eq!(node.release_version.as_deref(), Some("4.1.3"));
        assert_eq!(node.data_center.as_deref(), Some("dc1"));
        assert_eq!(parse_local(&[]), None);
        let peers = vec![
            vec![
                testing::inet("10.0.0.2"),
                t("dc1"),
                t("rack1"),
                t("4.1.3"),
                testing::inet("10.0.0.2"),
            ],
            vec![None, t("dc2")],
        ];
        let p = parse_peers(&peers);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].peer, "10.0.0.2");
        assert_eq!(
            parse_scylla_version(&[vec![t("2025.1.0")]]).as_deref(),
            Some("2025.1.0")
        );
        let describe = vec![
            vec![
                t("shop"),
                t("table"),
                t("t"),
                t("CREATE TABLE shop.t (k int PRIMARY KEY);"),
            ],
            vec![
                t("shop"),
                t("index"),
                t("i"),
                t("CREATE INDEX i ON shop.t (v);"),
            ],
        ];
        assert_eq!(
            parse_describe(&describe).as_deref(),
            Some("CREATE TABLE shop.t (k int PRIMARY KEY);\n\nCREATE INDEX i ON shop.t (v);")
        );
    }

    #[test]
    fn completion_and_types() {
        let rows = vec![
            vec![t("orders"), t("id")],
            vec![t("orders"), t("total")],
            vec![t("users"), t("email")],
            vec![t("compact"), t("")],
        ];
        let c = completion("shop", &rows);
        assert_eq!(c.default_schema.as_deref(), Some("shop"));
        assert_eq!(c.schemas["shop"]["orders"], ["id", "total"]);
        assert!(!c.schemas["shop"].contains_key("compact"));
        let types = vec![
            vec![
                t("address"),
                list(&["street", "city"]),
                list(&["text", "frozen<point>"]),
            ],
            vec![t("Point"), list(&["lat"]), list(&["double"])],
        ];
        let udts = parse_types(&types);
        assert_eq!(udts[1].fields[1], ("city".into(), "frozen<point>".into()));
        let names: Vec<String> = user_types(&udts).into_iter().map(|u| u.name).collect();
        assert_eq!(names, ["frozen<\"Point\">", "frozen<address>"]);
    }
}
