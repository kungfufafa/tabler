//! DuckDB's types: a result column's logical type as a tree (read once per column, before any
//! row), its name as DuckDB's catalog spells it, and the `ValueKind` of its values; and the kind
//! of a type the catalog names (`duckdb_columns().data_type`).
//!
//! The tree is what the decoders go by. Arrow, which carries the rows, gives several DuckDB
//! types one shape (HUGEINT, UHUGEINT and DECIMAL(38,0) are all `Decimal128(38, 0)`; UUID and
//! JSON are text; TIMETZ is a plain time), so the logical type decides what a value is.

use ::duckdb::core::{LogicalTypeHandle, LogicalTypeId};
use ::duckdb::ffi;

use crate::sqlgen::dialect::duckdb::type_member_name;
use crate::types::schema::ValueKind;

/// Nested types deeper than this are shown by name only (DuckDB has no such types in practice;
/// the bound keeps a hostile type from exhausting the stack).
const MAX_DEPTH: usize = 32;

/// The precision of a TIMESTAMP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsUnit {
    Seconds,
    Millis,
    Micros,
    Nanos,
}

/// A DuckDB logical type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DuckType {
    /// `SELECT NULL`'s type.
    Null,
    Boolean,
    TinyInt,
    SmallInt,
    Integer,
    BigInt,
    UTinyInt,
    USmallInt,
    UInteger,
    UBigInt,
    HugeInt,
    UHugeInt,
    Float,
    Double,
    Decimal {
        width: u8,
        scale: u8,
    },
    Varchar,
    /// VARCHAR with the alias JSON.
    Json,
    Blob,
    Bit,
    /// Arbitrary-precision integer (VARINT before DuckDB 1.4).
    Bignum,
    Date,
    Time,
    TimeNs,
    TimeTz,
    Timestamp(TsUnit),
    TimestampTz,
    Interval,
    Uuid,
    /// The name of a user-defined ENUM type, when the column has one.
    Enum(Option<String>),
    List(Box<DuckType>),
    /// The size is the arrow shape's (`FixedSizeList`); the logical type does not tell it here.
    Array(Box<DuckType>, Option<usize>),
    Struct(Vec<(String, DuckType)>),
    Map(Box<DuckType>, Box<DuckType>),
    Union(Vec<(String, DuckType)>),
    Geometry,
    Variant,
    /// Anything else, by name.
    Other(String),
}

impl DuckType {
    /// The tree of a logical type. Children are read only within the counts DuckDB reports, so
    /// none of duckdb-rs's panicking accessors is reached.
    pub fn from_logical(t: &LogicalTypeHandle) -> DuckType {
        Self::read(t, 0)
    }

    fn read(t: &LogicalTypeHandle, depth: usize) -> DuckType {
        let id = match t.try_id() {
            Ok(id) => id,
            Err(raw) => return DuckType::Other(format!("type {raw}")),
        };
        if depth > MAX_DEPTH {
            return DuckType::Other(format!("{id:?}").to_uppercase());
        }
        let child = |i: usize| {
            if i < t.num_children() {
                Self::read(&t.child(i), depth + 1)
            } else {
                DuckType::Other("?".into())
            }
        };
        let members = || {
            (0..t.num_children())
                .map(|i| (t.child_name(i), Self::read(&t.child(i), depth + 1)))
                .collect::<Vec<_>>()
        };
        match id {
            LogicalTypeId::SqlNull => DuckType::Null,
            LogicalTypeId::Boolean => DuckType::Boolean,
            LogicalTypeId::Tinyint => DuckType::TinyInt,
            LogicalTypeId::Smallint => DuckType::SmallInt,
            LogicalTypeId::Integer => DuckType::Integer,
            LogicalTypeId::Bigint => DuckType::BigInt,
            LogicalTypeId::UTinyint => DuckType::UTinyInt,
            LogicalTypeId::USmallint => DuckType::USmallInt,
            LogicalTypeId::UInteger => DuckType::UInteger,
            LogicalTypeId::UBigint => DuckType::UBigInt,
            LogicalTypeId::Hugeint => DuckType::HugeInt,
            LogicalTypeId::UHugeint => DuckType::UHugeInt,
            LogicalTypeId::Float => DuckType::Float,
            LogicalTypeId::Double => DuckType::Double,
            LogicalTypeId::Decimal => DuckType::Decimal {
                width: t.decimal_width(),
                scale: t.decimal_scale(),
            },
            LogicalTypeId::Varchar => match alias(t) {
                Some(a) if a.eq_ignore_ascii_case("json") => DuckType::Json,
                _ => DuckType::Varchar,
            },
            LogicalTypeId::Blob => DuckType::Blob,
            LogicalTypeId::Bit => DuckType::Bit,
            LogicalTypeId::Bignum => DuckType::Bignum,
            LogicalTypeId::Date => DuckType::Date,
            LogicalTypeId::Time => DuckType::Time,
            LogicalTypeId::TimeNs => DuckType::TimeNs,
            LogicalTypeId::TimeTZ => DuckType::TimeTz,
            LogicalTypeId::Timestamp => DuckType::Timestamp(TsUnit::Micros),
            LogicalTypeId::TimestampS => DuckType::Timestamp(TsUnit::Seconds),
            LogicalTypeId::TimestampMs => DuckType::Timestamp(TsUnit::Millis),
            LogicalTypeId::TimestampNs => DuckType::Timestamp(TsUnit::Nanos),
            LogicalTypeId::TimestampTZ => DuckType::TimestampTz,
            LogicalTypeId::Interval => DuckType::Interval,
            LogicalTypeId::Uuid => DuckType::Uuid,
            LogicalTypeId::Enum => DuckType::Enum(alias(t)),
            LogicalTypeId::List => DuckType::List(Box::new(child(0))),
            LogicalTypeId::Array => DuckType::Array(Box::new(child(0)), None),
            LogicalTypeId::Map => DuckType::Map(Box::new(child(0)), Box::new(child(1))),
            LogicalTypeId::Struct => DuckType::Struct(members()),
            LogicalTypeId::Union => DuckType::Union(members()),
            LogicalTypeId::Geometry => DuckType::Geometry,
            LogicalTypeId::Variant => DuckType::Variant,
            other => DuckType::Other(format!("{other:?}").to_uppercase()),
        }
    }

    /// The name DuckDB's catalog gives the type (`DECIMAL(18,3)`, `INTEGER[]`,
    /// `STRUCT(a INTEGER, b VARCHAR)`, `TIMESTAMP WITH TIME ZONE`).
    pub fn name(&self) -> String {
        match self {
            DuckType::Null => "NULL".into(),
            DuckType::Boolean => "BOOLEAN".into(),
            DuckType::TinyInt => "TINYINT".into(),
            DuckType::SmallInt => "SMALLINT".into(),
            DuckType::Integer => "INTEGER".into(),
            DuckType::BigInt => "BIGINT".into(),
            DuckType::UTinyInt => "UTINYINT".into(),
            DuckType::USmallInt => "USMALLINT".into(),
            DuckType::UInteger => "UINTEGER".into(),
            DuckType::UBigInt => "UBIGINT".into(),
            DuckType::HugeInt => "HUGEINT".into(),
            DuckType::UHugeInt => "UHUGEINT".into(),
            DuckType::Float => "FLOAT".into(),
            DuckType::Double => "DOUBLE".into(),
            DuckType::Decimal { width, scale } => format!("DECIMAL({width},{scale})"),
            DuckType::Varchar => "VARCHAR".into(),
            DuckType::Json => "JSON".into(),
            DuckType::Blob => "BLOB".into(),
            DuckType::Bit => "BIT".into(),
            DuckType::Bignum => "BIGNUM".into(),
            DuckType::Date => "DATE".into(),
            DuckType::Time => "TIME".into(),
            DuckType::TimeNs => "TIME_NS".into(),
            DuckType::TimeTz => "TIME WITH TIME ZONE".into(),
            DuckType::Timestamp(TsUnit::Micros) => "TIMESTAMP".into(),
            DuckType::Timestamp(TsUnit::Seconds) => "TIMESTAMP_S".into(),
            DuckType::Timestamp(TsUnit::Millis) => "TIMESTAMP_MS".into(),
            DuckType::Timestamp(TsUnit::Nanos) => "TIMESTAMP_NS".into(),
            DuckType::TimestampTz => "TIMESTAMP WITH TIME ZONE".into(),
            DuckType::Interval => "INTERVAL".into(),
            DuckType::Uuid => "UUID".into(),
            DuckType::Enum(name) => name.clone().unwrap_or_else(|| "ENUM".into()),
            DuckType::List(t) => format!("{}[]", t.name()),
            DuckType::Array(t, Some(n)) => format!("{}[{n}]", t.name()),
            DuckType::Array(t, None) => format!("{}[]", t.name()),
            DuckType::Struct(members) => format!("STRUCT({})", member_list(members)),
            DuckType::Map(k, v) => format!("MAP({}, {})", k.name(), v.name()),
            DuckType::Union(members) => format!("UNION({})", member_list(members)),
            DuckType::Geometry => "GEOMETRY".into(),
            DuckType::Variant => "VARIANT".into(),
            DuckType::Other(name) => name.clone(),
        }
    }

    /// How the grid shows and edits values of the type.
    pub fn kind(&self) -> ValueKind {
        match self {
            DuckType::Null | DuckType::Variant | DuckType::Other(_) => ValueKind::Other,
            DuckType::Boolean => ValueKind::Bool,
            DuckType::TinyInt
            | DuckType::SmallInt
            | DuckType::Integer
            | DuckType::UTinyInt
            | DuckType::USmallInt
            | DuckType::UInteger => ValueKind::Int,
            DuckType::BigInt
            | DuckType::UBigInt
            | DuckType::HugeInt
            | DuckType::UHugeInt
            | DuckType::Bignum => ValueKind::BigInt,
            DuckType::Float | DuckType::Double => ValueKind::Float,
            DuckType::Decimal { .. } => ValueKind::Decimal,
            DuckType::Varchar => ValueKind::Text,
            DuckType::Json
            | DuckType::Struct(_)
            | DuckType::Map(..)
            | DuckType::Union(_)
            | DuckType::List(_)
            | DuckType::Array(..) => ValueKind::Json,
            DuckType::Blob => ValueKind::Bytes,
            DuckType::Bit => ValueKind::Bit,
            DuckType::Date => ValueKind::Date,
            DuckType::Time | DuckType::TimeNs | DuckType::TimeTz => ValueKind::Time,
            DuckType::Timestamp(_) => ValueKind::Timestamp,
            DuckType::TimestampTz => ValueKind::TimestampTz,
            DuckType::Interval => ValueKind::Interval,
            DuckType::Uuid => ValueKind::Uuid,
            DuckType::Enum(_) => ValueKind::Enum,
            DuckType::Geometry => ValueKind::Geometry,
        }
    }
}

// The handle is its `duckdb_logical_type` and nothing else (`alias` reads it that way).
const _: () = assert!(
    std::mem::size_of::<LogicalTypeHandle>() == std::mem::size_of::<ffi::duckdb_logical_type>()
);

/// The type's alias (`JSON`, a user type's name), with bytes that are no UTF-8 replaced:
/// duckdb-rs's `get_alias` unwraps the UTF-8 check, and a crafted database file may name a type
/// in other bytes. (`child_name`, used for struct members, is lossy already.)
fn alias(t: &LogicalTypeHandle) -> Option<String> {
    // SAFETY: `LogicalTypeHandle` has one field, its `duckdb_logical_type`, and the assertion
    // above keeps the sizes equal, so the field is all of the handle and reads as that pointer.
    // The handle outlives the call; the alias is DuckDB's copy, which `DuckDbString` frees.
    unsafe {
        let ptr =
            std::ptr::read((t as *const LogicalTypeHandle).cast::<ffi::duckdb_logical_type>());
        ffi::DuckDbString::from_nullable_ptr(ffi::duckdb_logical_type_get_alias(ptr))
            .map(|s| s.to_string_lossy().into_owned())
    }
}

fn member_list(members: &[(String, DuckType)]) -> String {
    members
        .iter()
        .map(|(name, t)| format!("{} {}", type_member_name(name), t.name()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The kind of a type as the catalog names it (`duckdb_columns().data_type`, `typeof(x)`). A
/// user-defined type's own name is `Other` (the catalog resolves ENUMs through `duckdb_types()`).
pub fn kind_of_type(name: &str) -> ValueKind {
    let upper = name.trim().to_ascii_uppercase();
    if upper.ends_with(']') {
        return ValueKind::Json;
    }
    let base = upper.split('(').next().unwrap_or_default().trim();
    match base {
        "BOOLEAN" | "BOOL" | "LOGICAL" => ValueKind::Bool,
        "TINYINT" | "INT1" | "SMALLINT" | "INT2" | "SHORT" | "INTEGER" | "INT" | "INT4"
        | "SIGNED" | "UTINYINT" | "USMALLINT" | "UINTEGER" => ValueKind::Int,
        "BIGINT" | "INT8" | "LONG" | "UBIGINT" | "HUGEINT" | "INT128" | "UHUGEINT" | "UINT128"
        | "BIGNUM" | "VARINT" => ValueKind::BigInt,
        "FLOAT" | "FLOAT4" | "REAL" | "DOUBLE" | "FLOAT8" => ValueKind::Float,
        "DECIMAL" | "NUMERIC" => ValueKind::Decimal,
        "VARCHAR" | "TEXT" | "STRING" | "CHAR" | "BPCHAR" | "NVARCHAR" => ValueKind::Text,
        "JSON" | "STRUCT" | "MAP" | "UNION" | "ROW" => ValueKind::Json,
        "BLOB" | "BYTEA" | "BINARY" | "VARBINARY" => ValueKind::Bytes,
        "BIT" | "BITSTRING" => ValueKind::Bit,
        "DATE" => ValueKind::Date,
        "TIME" | "TIME_NS" | "TIMETZ" | "TIME WITH TIME ZONE" => ValueKind::Time,
        "TIMESTAMP" | "DATETIME" | "TIMESTAMP_S" | "TIMESTAMP_MS" | "TIMESTAMP_NS"
        | "TIMESTAMP_US" => ValueKind::Timestamp,
        "TIMESTAMPTZ" | "TIMESTAMP WITH TIME ZONE" => ValueKind::TimestampTz,
        "INTERVAL" => ValueKind::Interval,
        "UUID" | "GUID" => ValueKind::Uuid,
        "ENUM" => ValueKind::Enum,
        "GEOMETRY" => ValueKind::Geometry,
        _ => ValueKind::Other,
    }
}

/// The element type of a LIST or ARRAY type name (`INTEGER[]` → `INTEGER`, `VARCHAR[3][]` →
/// `VARCHAR[3]`).
pub fn array_element(name: &str) -> Option<String> {
    let t = name.trim();
    if !t.ends_with(']') {
        return None;
    }
    let open = t.rfind('[')?;
    let inner = &t[open + 1..t.len() - 1];
    (inner.is_empty() || inner.chars().all(|c| c.is_ascii_digit())).then(|| t[..open].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_catalog() {
        let t = DuckType::Struct(vec![
            ("a".into(), DuckType::Integer),
            (
                "B c".into(),
                DuckType::List(Box::new(DuckType::Decimal {
                    width: 18,
                    scale: 3,
                })),
            ),
        ]);
        assert_eq!(t.name(), "STRUCT(a INTEGER, \"B c\" DECIMAL(18,3)[])");
        assert_eq!(
            DuckType::Map(Box::new(DuckType::Varchar), Box::new(DuckType::Uuid)).name(),
            "MAP(VARCHAR, UUID)"
        );
        assert_eq!(
            DuckType::Array(Box::new(DuckType::Integer), Some(3)).name(),
            "INTEGER[3]"
        );
        assert_eq!(DuckType::Enum(Some("mood".into())).name(), "mood");
        assert_eq!(DuckType::TimestampTz.kind(), ValueKind::TimestampTz);
        assert_eq!(DuckType::HugeInt.kind(), ValueKind::BigInt);
        assert_eq!(DuckType::Json.kind(), ValueKind::Json);
    }

    #[test]
    fn aliases_read_lossily() {
        let mut t = LogicalTypeHandle::from(LogicalTypeId::Varchar);
        assert_eq!(alias(&t), None);
        t.set_alias("JSON");
        assert_eq!(DuckType::from_logical(&t), DuckType::Json);
        // An alias that is no UTF-8 (duckdb-rs's `get_alias` panics on it).
        let name = std::ffi::CString::new(vec![b'm', 0xff]).unwrap();
        // SAFETY: as in `alias`; DuckDB copies the name.
        unsafe {
            let ptr =
                std::ptr::read((&t as *const LogicalTypeHandle).cast::<ffi::duckdb_logical_type>());
            ffi::duckdb_logical_type_set_alias(ptr, name.as_ptr());
        }
        assert_eq!(alias(&t).as_deref(), Some("m\u{fffd}"));
        assert_eq!(DuckType::from_logical(&t), DuckType::Varchar);
    }

    #[test]
    fn kinds_of_catalog_names() {
        for (name, kind) in [
            ("INTEGER", ValueKind::Int),
            ("BIGINT", ValueKind::BigInt),
            ("HUGEINT", ValueKind::BigInt),
            ("DECIMAL(18,3)", ValueKind::Decimal),
            ("VARCHAR", ValueKind::Text),
            ("TIMESTAMP WITH TIME ZONE", ValueKind::TimestampTz),
            ("TIME WITH TIME ZONE", ValueKind::Time),
            ("TIMESTAMP_NS", ValueKind::Timestamp),
            ("INTEGER[]", ValueKind::Json),
            ("STRUCT(a INTEGER)", ValueKind::Json),
            ("MAP(VARCHAR, INTEGER)", ValueKind::Json),
            ("ENUM('a', 'b')", ValueKind::Enum),
            ("UUID", ValueKind::Uuid),
            ("BLOB", ValueKind::Bytes),
            ("BIT", ValueKind::Bit),
            ("mood", ValueKind::Other),
        ] {
            assert_eq!(kind_of_type(name), kind, "{name}");
        }
        assert_eq!(array_element("INTEGER[]").as_deref(), Some("INTEGER"));
        assert_eq!(array_element("VARCHAR[3][]").as_deref(), Some("VARCHAR[3]"));
        assert_eq!(array_element("INTEGER[2]").as_deref(), Some("INTEGER"));
        assert_eq!(array_element("VARCHAR"), None);
    }
}
