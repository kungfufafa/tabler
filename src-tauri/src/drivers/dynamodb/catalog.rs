//! The catalog in the sidebar's terms: the region is the database and the schema, `ListTables`
//! pages are the tables, and `DescribeTable` (with a sample of items) is a table's structure:
//!
//! - columns: the key attributes first (partition key `pk_ordinal` 1, sort key 2, typed by
//!   `AttributeDefinitions`), then the attributes of the sampled items in the order they were
//!   first seen, typed by what was seen (`S|N|NULL`), then index keys the sample lacked;
//! - indexes: the primary key, each global (`GSI`) and local (`LSI`) secondary index with its
//!   projection;
//! - the item count DynamoDB refreshes about every six hours (an estimate), a comment with the
//!   status, billing mode, TTL attribute and stream, and the table as a `CreateTable` request
//!   (its definition and DDL).

use std::collections::BTreeSet;

use serde_json::{Map, Value as J, json};

use super::attr::{AttrType, Item};
use crate::error::AppError;
use crate::sqlgen::native::dynamo::{KeySchema, request_text};
use crate::types::schema::{
    ColumnMeta, DatabaseInfo, IndexMeta, SchemaInfo, TableKind, TableMeta, TableRef, TableSummary,
    ValueKind,
};

/// The grid shows at most this many sampled attributes.
pub const MAX_COLUMNS: usize = 500;
/// `ListTables` pages followed before giving up (100 tables each).
pub const MAX_TABLE_PAGES: usize = 10_000;

fn protocol(what: impl std::fmt::Display) -> AppError {
    AppError::db(format!(
        "unexpected DescribeTable reply from DynamoDB: {what}"
    ))
}

fn str_at<'a>(v: &'a J, key: &str) -> Option<&'a str> {
    v.get(key).and_then(J::as_str)
}

/// A count as DynamoDB sends it (an integer, sometimes written as a float).
fn int_at(v: &J, key: &str) -> Option<i64> {
    let n = v.get(key)?;
    n.as_i64()
        .or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f as i64))
}

/// The table names of a `ListTables` reply and where the next page starts.
pub fn parse_list_tables(reply: &J) -> Result<(Vec<String>, Option<String>), AppError> {
    let names = match reply.get("TableNames") {
        Some(J::Array(names)) => names
            .iter()
            .map(|n| {
                n.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| AppError::db(format!("unexpected table name {n}")))
            })
            .collect::<Result<Vec<_>, _>>()?,
        None => Vec::new(),
        Some(other) => return Err(AppError::db(format!("unexpected TableNames {other}"))),
    };
    let next = str_at(reply, "LastEvaluatedTableName")
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok((names, next))
}

/// A secondary index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDescription {
    pub name: String,
    pub keys: KeySchema,
    /// `ALL`, `KEYS_ONLY` or `INCLUDE`.
    pub projection: String,
    pub non_key: Vec<String>,
    pub status: Option<String>,
    pub item_count: Option<i64>,
    pub raw: J,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableDescription {
    pub name: String,
    pub status: Option<String>,
    pub keys: KeySchema,
    /// `AttributeDefinitions`: the table's and its indexes' key attributes.
    pub attributes: Vec<(String, AttrType)>,
    pub global_indexes: Vec<IndexDescription>,
    pub local_indexes: Vec<IndexDescription>,
    pub item_count: Option<i64>,
    pub size_bytes: Option<i64>,
    /// `PAY_PER_REQUEST` or `PROVISIONED`.
    pub billing_mode: String,
    /// The stream's view type, when a stream is on.
    pub stream: Option<String>,
    pub table_class: Option<String>,
    pub deletion_protection: bool,
    pub raw: J,
}

impl TableDescription {
    /// The keys of `index` (a secondary index), or of the table for `None`.
    pub fn keys_of(&self, index: Option<&str>) -> Option<&KeySchema> {
        match index {
            None => Some(&self.keys),
            Some(name) => self
                .global_indexes
                .iter()
                .chain(&self.local_indexes)
                .find(|i| i.name == name)
                .map(|i| &i.keys),
        }
    }
}

fn key_schema(raw: Option<&J>, attributes: &[(String, AttrType)]) -> Result<KeySchema, AppError> {
    let entries = raw
        .and_then(J::as_array)
        .ok_or_else(|| protocol("no KeySchema"))?;
    let mut hash = None;
    let mut range = None;
    for e in entries {
        let name = str_at(e, "AttributeName").ok_or_else(|| protocol("a key without a name"))?;
        let ty = attributes
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| *t)
            .ok_or_else(|| protocol(format!("the key {name} has no AttributeDefinition")))?;
        match str_at(e, "KeyType") {
            Some("HASH") => hash = Some((name.to_string(), ty)),
            Some("RANGE") => range = Some((name.to_string(), ty)),
            other => return Err(protocol(format!("key type {other:?}"))),
        }
    }
    Ok(KeySchema {
        hash: hash.ok_or_else(|| protocol("no partition key"))?,
        range,
    })
}

fn index(raw: &J, attributes: &[(String, AttrType)]) -> Result<IndexDescription, AppError> {
    let projection = raw.get("Projection").cloned().unwrap_or(J::Null);
    Ok(IndexDescription {
        name: str_at(raw, "IndexName")
            .ok_or_else(|| protocol("an index without a name"))?
            .to_string(),
        keys: key_schema(raw.get("KeySchema"), attributes)?,
        projection: str_at(&projection, "ProjectionType")
            .unwrap_or("ALL")
            .to_string(),
        non_key: projection
            .get("NonKeyAttributes")
            .and_then(J::as_array)
            .map(|a| a.iter().filter_map(J::as_str).map(str::to_string).collect())
            .unwrap_or_default(),
        status: str_at(raw, "IndexStatus").map(str::to_string),
        item_count: int_at(raw, "ItemCount"),
        raw: raw.clone(),
    })
}

/// The `Table` of a `DescribeTable` reply.
pub fn parse_describe_table(reply: &J) -> Result<TableDescription, AppError> {
    let t = reply.get("Table").ok_or_else(|| protocol("no Table"))?;
    let attributes: Vec<(String, AttrType)> = t
        .get("AttributeDefinitions")
        .and_then(J::as_array)
        .map(|defs| {
            defs.iter()
                .filter_map(|d| {
                    let name = str_at(d, "AttributeName")?;
                    let ty = AttrType::parse(str_at(d, "AttributeType")?)?;
                    Some((name.to_string(), ty))
                })
                .collect()
        })
        .unwrap_or_default();
    let indexes = |key: &str| -> Result<Vec<IndexDescription>, AppError> {
        t.get(key)
            .and_then(J::as_array)
            .map(|list| list.iter().map(|i| index(i, &attributes)).collect())
            .unwrap_or(Ok(Vec::new()))
    };
    let provisioned_reads = t
        .get("ProvisionedThroughput")
        .and_then(|p| int_at(p, "ReadCapacityUnits"))
        .unwrap_or(0);
    // Tables made before on-demand existed have no summary and are provisioned.
    let billing_mode = t
        .get("BillingModeSummary")
        .and_then(|b| str_at(b, "BillingMode"))
        .map(str::to_string)
        .unwrap_or_else(|| {
            if provisioned_reads > 0 {
                "PROVISIONED".into()
            } else {
                "PAY_PER_REQUEST".into()
            }
        });
    let stream = t
        .get("StreamSpecification")
        .filter(|s| s.get("StreamEnabled").and_then(J::as_bool) == Some(true))
        .and_then(|s| str_at(s, "StreamViewType"))
        .map(str::to_string);
    Ok(TableDescription {
        name: str_at(t, "TableName")
            .ok_or_else(|| protocol("no TableName"))?
            .to_string(),
        status: str_at(t, "TableStatus").map(str::to_string),
        keys: key_schema(t.get("KeySchema"), &attributes)?,
        global_indexes: indexes("GlobalSecondaryIndexes")?,
        local_indexes: indexes("LocalSecondaryIndexes")?,
        attributes,
        item_count: int_at(t, "ItemCount"),
        size_bytes: int_at(t, "TableSizeBytes"),
        billing_mode,
        stream,
        table_class: t
            .get("TableClassSummary")
            .and_then(|c| str_at(c, "TableClass"))
            .map(str::to_string),
        deletion_protection: t
            .get("DeletionProtectionEnabled")
            .and_then(J::as_bool)
            .unwrap_or(false),
        raw: t.clone(),
    })
}

/// The TTL attribute of a `DescribeTimeToLive` reply, while TTL is on (or being turned on).
pub fn parse_ttl(reply: &J) -> Option<String> {
    let d = reply.get("TimeToLiveDescription")?;
    match str_at(d, "TimeToLiveStatus") {
        Some("ENABLED" | "ENABLING") => str_at(d, "AttributeName").map(str::to_string),
        _ => None,
    }
}

/// The region as the sidebar's one database.
pub fn databases(region: &str) -> Vec<DatabaseInfo> {
    vec![DatabaseInfo {
        name: region.to_string(),
        owner: None,
        encoding: None,
        is_current: true,
    }]
}

/// The region as the one schema.
pub fn schemas(region: &str) -> Vec<SchemaInfo> {
    vec![SchemaInfo {
        name: region.to_string(),
        is_default: true,
    }]
}

pub fn summary(name: &str, region: &str) -> TableSummary {
    TableSummary {
        name: name.to_string(),
        schema: Some(region.to_string()),
        kind: TableKind::Table,
        row_estimate: None,
        size_bytes: None,
        comment: None,
    }
}

/// The attributes of items in the order they were first seen, with the types seen.
pub fn sampled_attributes(items: &[Item]) -> Vec<(String, BTreeSet<AttrType>)> {
    let mut out: Vec<(String, BTreeSet<AttrType>)> = Vec::new();
    for item in items {
        for (name, value) in item {
            match out.iter_mut().find(|(n, _)| n == name) {
                Some((_, types)) => {
                    types.insert(value.ty());
                }
                None => out.push((name.clone(), BTreeSet::from([value.ty()]))),
            }
        }
    }
    out
}

/// `S|N|NULL` (`sqlgen::native::dynamo::column_types` reads it back).
pub fn type_label(types: &BTreeSet<AttrType>) -> String {
    types
        .iter()
        .map(|t| t.as_str())
        .collect::<Vec<_>>()
        .join("|")
}

/// The grid kind of a column with these types: the one type's besides NULL, else `Other`.
pub fn kind_of(types: &BTreeSet<AttrType>) -> ValueKind {
    let mut real = types.iter().filter(|t| **t != AttrType::Null);
    match (real.next(), real.next()) {
        (Some(t), None) => t.kind(),
        _ => ValueKind::Other,
    }
}

fn column(name: &str, ordinal: u32, types: &BTreeSet<AttrType>) -> ColumnMeta {
    let elem = types.iter().find_map(|t| match t {
        AttrType::Ss => Some("S"),
        AttrType::Ns => Some("N"),
        AttrType::Bs => Some("B"),
        _ => None,
    });
    ColumnMeta {
        name: name.to_string(),
        ordinal,
        data_type: type_label(types),
        kind: kind_of(types),
        nullable: true,
        default_expr: None,
        is_pk: false,
        pk_ordinal: None,
        is_auto_increment: false,
        is_generated: false,
        is_unsigned: false,
        comment: None,
        enum_values: None,
        char_max_len: None,
        numeric_precision: None,
        numeric_scale: None,
        collation: None,
        array_elem_type: elem.map(str::to_string),
        extra: None,
        base_type: None,
    }
}

/// A table's columns: its keys, the sampled attributes, then index keys the sample lacked.
pub fn columns(desc: &TableDescription, sample: &[Item]) -> Vec<ColumnMeta> {
    let mut out: Vec<ColumnMeta> = Vec::new();
    let key_cols = [
        Some((&desc.keys.hash, 1u32, "partition key", "HASH")),
        desc.keys
            .range
            .as_ref()
            .map(|r| (r, 2u32, "sort key", "RANGE")),
    ];
    for (key, pk, role, key_type) in key_cols.into_iter().flatten() {
        let mut c = column(&key.0, 0, &BTreeSet::from([key.1]));
        c.nullable = false;
        c.is_pk = true;
        c.pk_ordinal = Some(pk);
        c.comment = Some(role.to_string());
        c.extra = Some(key_type.to_string());
        out.push(c);
    }
    for (name, types) in sampled_attributes(sample) {
        if out.len() >= MAX_COLUMNS {
            break;
        }
        if !out.iter().any(|c| c.name == name) {
            out.push(column(&name, 0, &types));
        }
    }
    for (name, ty) in &desc.attributes {
        if !out.iter().any(|c| c.name == *name) {
            let mut c = column(name, 0, &BTreeSet::from([*ty]));
            c.comment = Some("index key".into());
            out.push(c);
        }
    }
    for (i, c) in out.iter_mut().enumerate() {
        c.ordinal = i as u32 + 1;
    }
    out
}

/// Adds attributes the columns lack and types they were not seen with (a grid page's items).
pub fn merge_columns(columns: &mut Vec<ColumnMeta>, items: &[Item]) {
    for (name, types) in sampled_attributes(items) {
        let room = columns.len() < MAX_COLUMNS;
        match columns.iter_mut().find(|c| c.name == name) {
            Some(c) if !c.is_pk => {
                let mut all: BTreeSet<AttrType> =
                    crate::sqlgen::native::dynamo::column_types(&c.data_type)
                        .into_iter()
                        .collect();
                let before = all.len();
                all.extend(types);
                if all.len() != before {
                    c.data_type = type_label(&all);
                    c.kind = kind_of(&all);
                }
            }
            Some(_) => {}
            None if room => {
                let ordinal = columns.len() as u32 + 1;
                columns.push(column(&name, ordinal, &types));
            }
            None => {}
        }
    }
}

fn index_meta(i: &IndexDescription, method: &str) -> IndexMeta {
    let projection = i.raw.get("Projection").cloned().unwrap_or(J::Null);
    let mut comment = Vec::new();
    if let Some(s) = &i.status {
        comment.push(s.clone());
    }
    if let Some(n) = i.item_count {
        comment.push(format!("about {n} items"));
    }
    IndexMeta {
        name: i.name.clone(),
        columns: i.keys.names().into_iter().map(str::to_string).collect(),
        unique: false,
        primary: false,
        method: Some(method.to_string()),
        predicate: None,
        definition: Some(format!("{} projection: {projection}", i.projection)),
        comment: (!comment.is_empty()).then(|| comment.join(", ")),
        include: i.non_key.clone(),
    }
}

pub fn indexes(desc: &TableDescription) -> Vec<IndexMeta> {
    let mut out = vec![IndexMeta {
        name: "PRIMARY".into(),
        columns: desc.keys.names().into_iter().map(str::to_string).collect(),
        unique: true,
        primary: true,
        method: Some("primary key".into()),
        predicate: None,
        definition: None,
        comment: None,
        include: Vec::new(),
    }];
    out.extend(desc.global_indexes.iter().map(|i| index_meta(i, "GSI")));
    out.extend(desc.local_indexes.iter().map(|i| index_meta(i, "LSI")));
    out
}

/// Status, billing, TTL, stream, class and protection in a line.
pub fn comment(desc: &TableDescription, ttl: Option<&str>) -> String {
    let mut parts = Vec::new();
    if let Some(s) = &desc.status {
        parts.push(s.clone());
    }
    parts.push(match desc.billing_mode.as_str() {
        "PAY_PER_REQUEST" => "on-demand".to_string(),
        "PROVISIONED" => "provisioned".to_string(),
        other => other.to_lowercase(),
    });
    if let Some(attr) = ttl {
        parts.push(format!("TTL on {attr}"));
    }
    if let Some(view) = &desc.stream {
        parts.push(format!("stream {view}"));
    }
    if let Some(class) = desc.table_class.as_deref().filter(|c| *c != "STANDARD") {
        parts.push(class.to_string());
    }
    if desc.deletion_protection {
        parts.push("deletion protection".into());
    }
    if let Some(b) = desc.size_bytes {
        parts.push(format!("about {b} bytes"));
    }
    parts.join(" \u{b7} ")
}

/// The `CreateTable` request that makes a table like this one (keys, billing, indexes, stream,
/// class, protection, encryption with a customer key).
pub fn create_table_request(desc: &TableDescription) -> J {
    let t = &desc.raw;
    let provisioned = desc.billing_mode == "PROVISIONED";
    let throughput = |raw: &J| {
        raw.get("ProvisionedThroughput").map(|p| {
            json!({
                "ReadCapacityUnits": p.get("ReadCapacityUnits").cloned().unwrap_or(json!(1)),
                "WriteCapacityUnits": p.get("WriteCapacityUnits").cloned().unwrap_or(json!(1)),
            })
        })
    };
    let mut body = Map::new();
    body.insert("TableName".into(), json!(desc.name));
    for k in ["AttributeDefinitions", "KeySchema"] {
        if let Some(v) = t.get(k) {
            body.insert(k.into(), v.clone());
        }
    }
    body.insert("BillingMode".into(), json!(desc.billing_mode));
    if provisioned && let Some(p) = throughput(t) {
        body.insert("ProvisionedThroughput".into(), p);
    }
    if let Some(od) = t.get("OnDemandThroughput") {
        body.insert("OnDemandThroughput".into(), od.clone());
    }
    let gsis: Vec<J> = desc
        .global_indexes
        .iter()
        .map(|i| {
            let mut g = Map::new();
            g.insert("IndexName".into(), json!(i.name));
            for k in ["KeySchema", "Projection", "OnDemandThroughput"] {
                if let Some(v) = i.raw.get(k) {
                    g.insert(k.into(), v.clone());
                }
            }
            if provisioned && let Some(p) = throughput(&i.raw) {
                g.insert("ProvisionedThroughput".into(), p);
            }
            J::Object(g)
        })
        .collect();
    if !gsis.is_empty() {
        body.insert("GlobalSecondaryIndexes".into(), J::Array(gsis));
    }
    let lsis: Vec<J> = desc
        .local_indexes
        .iter()
        .map(|i| {
            json!({
                "IndexName": i.name,
                "KeySchema": i.raw.get("KeySchema").cloned().unwrap_or(J::Null),
                "Projection": i.raw.get("Projection").cloned().unwrap_or(J::Null),
            })
        })
        .collect();
    if !lsis.is_empty() {
        body.insert("LocalSecondaryIndexes".into(), J::Array(lsis));
    }
    if let Some(view) = &desc.stream {
        body.insert(
            "StreamSpecification".into(),
            json!({ "StreamEnabled": true, "StreamViewType": view }),
        );
    }
    if let Some(class) = &desc.table_class {
        body.insert("TableClass".into(), json!(class));
    }
    if desc.deletion_protection {
        body.insert("DeletionProtectionEnabled".into(), json!(true));
    }
    if let Some(sse) = t.get("SSEDescription")
        && str_at(sse, "SSEType") == Some("KMS")
    {
        let mut spec = Map::new();
        spec.insert("Enabled".into(), json!(true));
        spec.insert("SSEType".into(), json!("KMS"));
        if let Some(key) = str_at(sse, "KMSMasterKeyArn") {
            spec.insert("KMSMasterKeyId".into(), json!(key));
        }
        body.insert("SSESpecification".into(), J::Object(spec));
    }
    J::Object(body)
}

/// The table's definition as the console command that creates it.
pub fn definition(desc: &TableDescription) -> String {
    request_text("CreateTable", &create_table_request(desc))
}

/// A table's structure: `region` is its schema; `ttl` its TTL attribute.
pub fn table_meta(
    desc: &TableDescription,
    sample: &[Item],
    region: &str,
    ttl: Option<&str>,
) -> TableMeta {
    TableMeta {
        table: TableRef::new(Some(region), &desc.name),
        kind: TableKind::Table,
        columns: columns(desc, sample),
        primary_key: desc.keys.names().into_iter().map(str::to_string).collect(),
        indexes: indexes(desc),
        foreign_keys: Vec::new(),
        triggers: Vec::new(),
        comment: Some(comment(desc, ttl)),
        row_estimate: desc.item_count,
        ddl: Some(definition(desc)),
        checks: Vec::new(),
        options: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::dynamodb::attr::{Attr, item_from_json};
    use crate::sqlgen::native::dynamo::parse_command;

    pub(crate) fn music() -> J {
        json!({
            "Table": {
                "TableName": "Music",
                "TableStatus": "ACTIVE",
                "TableArn": "arn:aws:dynamodb:us-east-1:123456789012:table/Music",
                "AttributeDefinitions": [
                    {"AttributeName": "Artist", "AttributeType": "S"},
                    {"AttributeName": "SongTitle", "AttributeType": "S"},
                    {"AttributeName": "Year", "AttributeType": "N"},
                    {"AttributeName": "Genre", "AttributeType": "S"}
                ],
                "KeySchema": [
                    {"AttributeName": "Artist", "KeyType": "HASH"},
                    {"AttributeName": "SongTitle", "KeyType": "RANGE"}
                ],
                "ItemCount": 1234.0,
                "TableSizeBytes": 56789,
                "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 2, "NumberOfDecreasesToday": 0},
                "GlobalSecondaryIndexes": [{
                    "IndexName": "ByGenre",
                    "KeySchema": [{"AttributeName": "Genre", "KeyType": "HASH"}, {"AttributeName": "Year", "KeyType": "RANGE"}],
                    "Projection": {"ProjectionType": "INCLUDE", "NonKeyAttributes": ["Album"]},
                    "IndexStatus": "ACTIVE",
                    "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
                    "ItemCount": 10
                }],
                "LocalSecondaryIndexes": [{
                    "IndexName": "ByYear",
                    "KeySchema": [{"AttributeName": "Artist", "KeyType": "HASH"}, {"AttributeName": "Year", "KeyType": "RANGE"}],
                    "Projection": {"ProjectionType": "KEYS_ONLY"}
                }],
                "StreamSpecification": {"StreamEnabled": true, "StreamViewType": "NEW_AND_OLD_IMAGES"},
                "TableClassSummary": {"TableClass": "STANDARD_INFREQUENT_ACCESS"},
                "DeletionProtectionEnabled": true,
                "SSEDescription": {"Status": "ENABLED", "SSEType": "KMS", "KMSMasterKeyArn": "arn:aws:kms:k"}
            }
        })
    }

    #[test]
    fn list_tables_pages() {
        let (names, next) =
            parse_list_tables(&json!({"TableNames": ["a", "b"], "LastEvaluatedTableName": "b"}))
                .unwrap();
        assert_eq!(
            (names, next.as_deref()),
            (vec!["a".to_string(), "b".to_string()], Some("b"))
        );
        assert_eq!(parse_list_tables(&json!({})).unwrap(), (vec![], None));
        assert!(parse_list_tables(&json!({"TableNames": [1]})).is_err());
        assert!(parse_list_tables(&json!({"TableNames": "a"})).is_err());
    }

    #[test]
    fn describe_table_reads_keys_indexes_and_settings() {
        let d = parse_describe_table(&music()).unwrap();
        assert_eq!(d.keys.hash, ("Artist".into(), AttrType::S));
        assert_eq!(d.keys.range, Some(("SongTitle".into(), AttrType::S)));
        assert_eq!(d.item_count, Some(1234));
        assert_eq!(d.billing_mode, "PROVISIONED");
        assert_eq!(d.stream.as_deref(), Some("NEW_AND_OLD_IMAGES"));
        assert_eq!(
            d.global_indexes[0].keys.range,
            Some(("Year".into(), AttrType::N))
        );
        assert_eq!(d.global_indexes[0].non_key, ["Album"]);
        assert_eq!(d.local_indexes[0].projection, "KEYS_ONLY");
        assert_eq!(d.keys_of(Some("ByYear")).unwrap().hash.0, "Artist");
        assert!(d.keys_of(Some("nope")).is_none());
        for bad in [
            json!({}),
            json!({"Table": {"TableName": "t"}}),
            json!({"Table": {"TableName": "t", "KeySchema": [{"AttributeName": "a", "KeyType": "HASH"}]}}),
            json!({"Table": {"TableName": "t", "AttributeDefinitions": [{"AttributeName": "a", "AttributeType": "S"}], "KeySchema": [{"AttributeName": "a", "KeyType": "RANGE"}]}}),
            json!({"Table": {"AttributeDefinitions": [{"AttributeName": "a", "AttributeType": "S"}], "KeySchema": [{"AttributeName": "a", "KeyType": "HASH"}]}}),
        ] {
            assert!(parse_describe_table(&bad).is_err(), "{bad}");
        }
        // On-demand: no throughput in the definition.
        let od = parse_describe_table(&json!({"Table": {
            "TableName": "t", "AttributeDefinitions": [{"AttributeName": "id", "AttributeType": "N"}],
            "KeySchema": [{"AttributeName": "id", "KeyType": "HASH"}],
            "BillingModeSummary": {"BillingMode": "PAY_PER_REQUEST"},
            "ProvisionedThroughput": {"ReadCapacityUnits": 0, "WriteCapacityUnits": 0}
        }}))
        .unwrap();
        assert_eq!(
            create_table_request(&od),
            json!({
                "TableName": "t",
                "AttributeDefinitions": [{"AttributeName": "id", "AttributeType": "N"}],
                "KeySchema": [{"AttributeName": "id", "KeyType": "HASH"}],
                "BillingMode": "PAY_PER_REQUEST"
            })
        );
        assert_eq!(comment(&od, None), "on-demand");
    }

    #[test]
    fn table_meta_puts_keys_first_then_sampled_attributes() {
        let d = parse_describe_table(&music()).unwrap();
        let sample: Vec<Item> = [
            json!({"Artist": {"S": "A"}, "SongTitle": {"S": "x"}, "Plays": {"N": "3"}, "Tags": {"SS": ["a"]}}),
            json!({"Artist": {"S": "B"}, "SongTitle": {"S": "y"}, "Plays": {"S": "many"}, "Extra": {"NULL": true}, "Info": {"M": {}}}),
        ]
        .iter()
        .map(|j| item_from_json(j).unwrap())
        .collect();
        let m = table_meta(&d, &sample, "us-east-1", Some("expiresAt"));
        let names: Vec<&str> = m.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Artist",
                "SongTitle",
                "Plays",
                "Tags",
                "Extra",
                "Info",
                "Year",
                "Genre"
            ]
        );
        let c = |n: &str| m.columns.iter().find(|c| c.name == n).unwrap();
        assert_eq!(
            (c("Artist").pk_ordinal, c("SongTitle").pk_ordinal),
            (Some(1), Some(2))
        );
        assert!(!c("Artist").nullable && c("Plays").nullable);
        assert_eq!(c("Plays").data_type, "S|N");
        assert_eq!(c("Plays").kind, ValueKind::Other);
        assert_eq!(c("Tags").kind, ValueKind::Array);
        assert_eq!(c("Tags").array_elem_type.as_deref(), Some("S"));
        assert_eq!(c("Extra").data_type, "NULL");
        assert_eq!(c("Info").kind, ValueKind::Json);
        assert_eq!(c("Year").data_type, "N");
        assert_eq!(
            m.columns.iter().map(|c| c.ordinal).collect::<Vec<_>>(),
            (1..=8).collect::<Vec<_>>()
        );
        assert_eq!(m.primary_key, ["Artist", "SongTitle"]);
        assert_eq!(m.table.schema.as_deref(), Some("us-east-1"));
        assert_eq!(m.row_estimate, Some(1234));
        let methods: Vec<_> = m
            .indexes
            .iter()
            .map(|i| (i.name.as_str(), i.method.as_deref()))
            .collect();
        assert_eq!(
            methods,
            [
                ("PRIMARY", Some("primary key")),
                ("ByGenre", Some("GSI")),
                ("ByYear", Some("LSI"))
            ]
        );
        assert_eq!(m.indexes[1].columns, ["Genre", "Year"]);
        assert_eq!(m.indexes[1].include, ["Album"]);
        let comment = m.comment.unwrap();
        assert!(
            comment.contains("TTL on expiresAt")
                && comment.contains("provisioned")
                && comment.contains("deletion protection"),
            "{comment}"
        );
        // The definition is a console command that creates the same table.
        let ddl = m.ddl.unwrap();
        match parse_command(&ddl).unwrap() {
            crate::sqlgen::native::dynamo::ConsoleCommand::Api { action, body } => {
                assert_eq!(action, "CreateTable");
                assert_eq!(
                    body["ProvisionedThroughput"],
                    json!({"ReadCapacityUnits": 5, "WriteCapacityUnits": 2})
                );
                assert_eq!(
                    body["GlobalSecondaryIndexes"][0]["ProvisionedThroughput"]["ReadCapacityUnits"],
                    json!(1)
                );
                assert_eq!(
                    body["LocalSecondaryIndexes"][0]["Projection"],
                    json!({"ProjectionType": "KEYS_ONLY"})
                );
                assert_eq!(
                    body["StreamSpecification"]["StreamViewType"],
                    json!("NEW_AND_OLD_IMAGES")
                );
                assert_eq!(
                    body["SSESpecification"]["KMSMasterKeyId"],
                    json!("arn:aws:kms:k")
                );
                assert_eq!(body["TableClass"], json!("STANDARD_INFREQUENT_ACCESS"));
                assert_eq!(body["DeletionProtectionEnabled"], json!(true));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn page_items_add_columns_and_types() {
        let d = parse_describe_table(&music()).unwrap();
        let mut cols = columns(&d, &[]);
        assert_eq!(cols.len(), 4);
        let page = vec![Item::from([
            ("Artist".to_string(), Attr::S("A".into())),
            ("Year".to_string(), Attr::S("unknown".into())),
            ("New".to_string(), Attr::Bool(true)),
        ])];
        merge_columns(&mut cols, &page);
        let year = cols.iter().find(|c| c.name == "Year").unwrap();
        assert_eq!(year.data_type, "S|N");
        assert_eq!(cols.last().unwrap().name, "New");
        assert_eq!(cols.last().unwrap().ordinal, 5);
        assert_eq!(cols[0].data_type, "S");
    }

    #[test]
    fn ttl_and_the_region() {
        assert_eq!(
            parse_ttl(&json!({"TimeToLiveDescription": {"TimeToLiveStatus": "ENABLED", "AttributeName": "exp"}})).as_deref(),
            Some("exp")
        );
        assert_eq!(
            parse_ttl(&json!({"TimeToLiveDescription": {"TimeToLiveStatus": "DISABLED"}})),
            None
        );
        assert_eq!(parse_ttl(&json!({})), None);
        assert_eq!(databases("eu-west-1")[0].name, "eu-west-1");
        assert!(schemas("eu-west-1")[0].is_default);
        assert_eq!(
            summary("t", "eu-west-1").schema.as_deref(),
            Some("eu-west-1")
        );
    }
}
