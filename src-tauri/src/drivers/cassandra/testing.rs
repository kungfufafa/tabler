//! Fakes of the cassandra lane's tests: cells as the driver decodes them untyped, and the
//! `system_schema` rows a Cassandra 4.1 node returns for a table `shop.events` (in the server's
//! order: `system_schema.columns` clusters by column name).

use ::scylla::value::CqlValue;

use crate::drivers::cassandra::catalog::{CellRow, TableRows};

pub fn t(s: &str) -> Option<CqlValue> {
    Some(CqlValue::Text(s.into()))
}

/// An `int` cell, or a `bigint` one beyond 32 bits.
pub fn i(n: i64) -> Option<CqlValue> {
    Some(match i32::try_from(n) {
        Ok(n) => CqlValue::Int(n),
        Err(_) => CqlValue::BigInt(n),
    })
}

pub fn b(v: bool) -> Option<CqlValue> {
    Some(CqlValue::Boolean(v))
}

pub fn d(f: f64) -> Option<CqlValue> {
    Some(CqlValue::Double(f))
}

/// A `map<text, text>` cell.
pub fn map(pairs: &[(&str, &str)]) -> Option<CqlValue> {
    Some(CqlValue::Map(
        pairs
            .iter()
            .map(|(k, v)| (CqlValue::Text((*k).into()), CqlValue::Text((*v).into())))
            .collect(),
    ))
}

/// A `list<text>` cell.
pub fn list(items: &[&str]) -> Option<CqlValue> {
    Some(CqlValue::List(
        items.iter().map(|s| CqlValue::Text((*s).into())).collect(),
    ))
}

pub fn inet(addr: &str) -> Option<CqlValue> {
    Some(CqlValue::Inet(addr.parse().expect("a test address")))
}

/// `COLUMNS` rows of `shop.events`: `PRIMARY KEY ((user_id, bucket), ts, seq)` with `ts DESC`,
/// a static column, and regular ones.
pub fn events_columns() -> Vec<CellRow> {
    let col =
        |name, kind, position, order, ty| vec![t(name), t(kind), i(position), t(order), t(ty)];
    vec![
        col("amount", "regular", -1, "none", "decimal"),
        col("bucket", "partition_key", 1, "none", "int"),
        col("owner", "static", -1, "none", "text"),
        col("seq", "clustering", 1, "asc", "int"),
        col("tags", "regular", -1, "none", "frozen<list<text>>"),
        col("ts", "clustering", 0, "desc", "timestamp"),
        col("user_id", "partition_key", 0, "none", "uuid"),
        col("z_note", "regular", -1, "none", "text"),
    ]
}

/// Every introspection row of `shop.events`.
pub struct EventsFixture {
    pub columns: Vec<CellRow>,
    pub options: Vec<CellRow>,
    pub indexes: Vec<CellRow>,
    pub triggers: Vec<CellRow>,
    pub size_estimates: Vec<CellRow>,
}

impl EventsFixture {
    pub fn rows(&self) -> TableRows<'_> {
        TableRows {
            columns: &self.columns,
            options: &self.options,
            indexes: &self.indexes,
            triggers: &self.triggers,
            size_estimates: &self.size_estimates,
            view: None,
        }
    }
}

pub fn events_fixture() -> EventsFixture {
    EventsFixture {
        columns: events_columns(),
        options: vec![vec![
            t("user events"),
            i(0),
            i(864_000),
            d(0.01),
            map(&[("keys", "ALL"), ("rows_per_partition", "NONE")]),
            map(&[
                (
                    "class",
                    "org.apache.cassandra.db.compaction.SizeTieredCompactionStrategy",
                ),
                ("max_threshold", "32"),
                ("min_threshold", "4"),
            ]),
            map(&[
                ("chunk_length_in_kb", "16"),
                ("class", "org.apache.cassandra.io.compress.LZ4Compressor"),
            ]),
            t("99p"),
        ]],
        indexes: vec![vec![
            t("events_by_owner"),
            t("COMPOSITES"),
            map(&[("target", "owner")]),
        ]],
        triggers: Vec::new(),
        size_estimates: vec![vec![i(120)], vec![i(80)]],
    }
}
