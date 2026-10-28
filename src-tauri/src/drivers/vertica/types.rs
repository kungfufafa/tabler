//! Vertica's types: the OIDs a RowDescription names, the names Tabler shows for them (in
//! `v_catalog.columns.data_type`'s spelling, lower case with the modifier: `varchar(80)`,
//! `numeric(10,2)`, `interval day to second`), and the `ValueKind` each one decodes to. The type
//! codes and modifiers follow vertica-python's `datatypes`.

use super::wire::TypeRef;
use crate::types::schema::ValueKind;

pub const UNKNOWN: u32 = 4;
pub const BOOL: u32 = 5;
pub const INT8: u32 = 6;
pub const FLOAT8: u32 = 7;
pub const CHAR: u32 = 8;
pub const VARCHAR: u32 = 9;
pub const DATE: u32 = 10;
pub const TIME: u32 = 11;
pub const TIMESTAMP: u32 = 12;
pub const TIMESTAMPTZ: u32 = 13;
pub const INTERVAL: u32 = 14;
pub const TIMETZ: u32 = 15;
pub const NUMERIC: u32 = 16;
pub const VARBINARY: u32 = 17;
pub const UUID: u32 = 20;
pub const INTERVALYM: u32 = 114;
pub const LONGVARCHAR: u32 = 115;
pub const LONGVARBINARY: u32 = 116;
pub const BINARY: u32 = 117;
pub const ROW: u32 = 300;
/// A multidimensional array, or one of rows.
pub const ARRAY: u32 = 301;
pub const MAP: u32 = 302;
/// One-dimensional arrays of a primitive type: 1500 + the element's OID (`ARRAY1D_INT8` 1506);
/// the types above 100 go to 1519–1522.
const ARRAY1D: std::ops::RangeInclusive<u32> = 1505..=1522;
/// Sets: 2700 + the element's OID, the same way.
const SET: std::ops::RangeInclusive<u32> = 2705..=2722;

/// The element type of a one-dimensional array or set OID, as vertica-python's
/// `COMPLEX_ELEMENT_TYPE` maps it.
pub fn element_oid(oid: u32) -> Option<u32> {
    let offset = if ARRAY1D.contains(&oid) {
        oid - 1500
    } else if SET.contains(&oid) {
        oid - 2700
    } else {
        return None;
    };
    Some(match offset {
        5..=17 => offset,
        18 => LONGVARBINARY,
        19 => LONGVARCHAR,
        20 => UUID,
        21 => INTERVALYM,
        22 => BINARY,
        _ => return None,
    })
}

/// A complex type (ROW, ARRAY, MAP, SET or a one-dimensional array): its values come as JSON.
pub fn is_complex(oid: u32) -> bool {
    matches!(oid, ROW | ARRAY | MAP) || element_oid(oid).is_some()
}

/// The decoded kind of a value of the type.
pub fn kind(ty: &TypeRef) -> ValueKind {
    match ty.oid {
        BOOL => ValueKind::Bool,
        INT8 => ValueKind::Int,
        FLOAT8 => ValueKind::Float,
        NUMERIC => ValueKind::Decimal,
        CHAR | VARCHAR | LONGVARCHAR | UNKNOWN => ValueKind::Text,
        VARBINARY | LONGVARBINARY | BINARY => ValueKind::Bytes,
        DATE => ValueKind::Date,
        TIME | TIMETZ => ValueKind::Time,
        TIMESTAMP => ValueKind::Timestamp,
        TIMESTAMPTZ => ValueKind::TimestampTz,
        INTERVAL | INTERVALYM => ValueKind::Interval,
        UUID => ValueKind::Uuid,
        oid if is_complex(oid) => ValueKind::Json,
        _ => ValueKind::Other,
    }
}

const MONTH: i32 = 1 << 17;
const YEAR: i32 = 1 << 18;
const DAY: i32 = 1 << 19;
const HOUR: i32 = 1 << 26;
const MINUTE: i32 = 1 << 27;
const SECOND: i32 = 1 << 28;

/// The fields of an interval type (`day to second`), from the modifier's range bits.
pub fn interval_range(oid: u32, modifier: i32) -> &'static str {
    let has = |mask: i32| modifier & mask == mask;
    if oid == INTERVALYM {
        return match modifier {
            -1 => "year to month",
            _ if has(YEAR | MONTH) => "year to month",
            _ if has(YEAR) => "year",
            _ if has(MONTH) => "month",
            _ => "year to month",
        };
    }
    if modifier == -1 {
        return "day to second";
    }
    if has(DAY | HOUR | MINUTE | SECOND) {
        "day to second"
    } else if has(DAY | HOUR | MINUTE) {
        "day to minute"
    } else if has(DAY | HOUR) {
        "day to hour"
    } else if has(DAY) {
        "day"
    } else if has(HOUR | MINUTE | SECOND) {
        "hour to second"
    } else if has(HOUR | MINUTE) {
        "hour to minute"
    } else if has(HOUR) {
        "hour"
    } else if has(MINUTE | SECOND) {
        "minute to second"
    } else if has(MINUTE) {
        "minute"
    } else if has(SECOND) {
        "second"
    } else {
        "day to second"
    }
}

/// The precision of a NUMERIC (`((m - 4) >> 16) & 0xFFFF`) and of the temporal types (`m &
/// 0xF`, fractional digits); None where the modifier says nothing.
pub fn precision(oid: u32, modifier: i32) -> Option<i32> {
    match oid {
        NUMERIC if modifier >= 4 => Some(((modifier - 4) >> 16) & 0xFFFF),
        NUMERIC => Some(1024),
        TIME | TIMETZ | TIMESTAMP | TIMESTAMPTZ | INTERVAL | INTERVALYM if modifier >= 0 => {
            Some(modifier & 0xF)
        }
        TIME | TIMETZ | TIMESTAMP | TIMESTAMPTZ | INTERVAL | INTERVALYM => Some(6),
        _ => None,
    }
}

/// The scale of a NUMERIC (`(m - 4) & 0xFFFF`, 15 without a modifier). The precision has the
/// high 16 bits and a scale may exceed 255 (it goes up to the precision, 1024), so the whole low
/// half is the scale; vertica-python's `getScale` masks 0xFF, the same below 256.
pub fn scale(oid: u32, modifier: i32) -> Option<i32> {
    match oid {
        NUMERIC if modifier >= 4 => Some((modifier - 4) & 0xFFFF),
        NUMERIC => Some(15),
        _ => None,
    }
}

/// The declared length of a character or binary type (`m - 4`), None when it has none.
pub fn length(oid: u32, modifier: i32) -> Option<i32> {
    match oid {
        CHAR | VARCHAR | LONGVARCHAR | BINARY | VARBINARY | LONGVARBINARY if modifier >= 4 => {
            Some(modifier - 4)
        }
        _ => None,
    }
}

/// The base name of a built-in type (lower case, without modifier), as `v_catalog` spells it.
fn base_name(oid: u32) -> Option<&'static str> {
    Some(match oid {
        UNKNOWN => "unknown",
        BOOL => "boolean",
        INT8 => "int",
        FLOAT8 => "float",
        CHAR => "char",
        VARCHAR => "varchar",
        LONGVARCHAR => "long varchar",
        DATE => "date",
        TIME => "time",
        TIMETZ => "timetz",
        TIMESTAMP => "timestamp",
        TIMESTAMPTZ => "timestamptz",
        BINARY => "binary",
        VARBINARY => "varbinary",
        LONGVARBINARY => "long varbinary",
        NUMERIC => "numeric",
        UUID => "uuid",
        INTERVAL | INTERVALYM => "interval",
        ROW => "row",
        ARRAY => "array",
        MAP => "map",
        _ => return None,
    })
}

/// The type's name with its modifier: `varchar(80)`, `numeric(10,2)`, `timestamp(3)`,
/// `interval day to second`, `array[int]`, `set[varchar]`, a user type's own name (`geometry`), or
/// `oid <n>` for one this does not know.
pub fn type_name(ty: &TypeRef) -> String {
    if let Some(name) = &ty.user_type {
        return name.to_ascii_lowercase();
    }
    let (oid, m) = (ty.oid, ty.modifier);
    if let Some(elem) = element_oid(oid) {
        let inner = type_name(&TypeRef {
            oid: elem,
            user_type: None,
            modifier: -1,
        });
        let outer = if ARRAY1D.contains(&oid) {
            "array"
        } else {
            "set"
        };
        return format!("{outer}[{inner}]");
    }
    let Some(base) = base_name(oid) else {
        return format!("oid {oid}");
    };
    match oid {
        INTERVAL | INTERVALYM => format!("interval {}", interval_range(oid, m)),
        NUMERIC if m >= 4 => format!(
            "numeric({},{})",
            precision(oid, m).unwrap_or(37),
            scale(oid, m).unwrap_or(15)
        ),
        TIME | TIMETZ | TIMESTAMP | TIMESTAMPTZ if m >= 0 => {
            format!("{base}({})", m & 0xF)
        }
        _ => match length(oid, m) {
            Some(n) => format!("{base}({n})"),
            None => base.to_string(),
        },
    }
}

/// The kind of a type named the way `v_catalog.columns.data_type` names it (`varchar(80)`,
/// `numeric(37,15)`, `interval day to second`, `Array[int]`, `Row(…)`, `geometry(1000)`).
pub fn kind_from_name(data_type: &str) -> ValueKind {
    let lower = data_type.trim().to_ascii_lowercase();
    let base = lower.split(['(', '[']).next().unwrap_or_default().trim();
    match base {
        "boolean" | "bool" => ValueKind::Bool,
        "int" | "integer" | "bigint" | "smallint" | "tinyint" | "int8" => ValueKind::Int,
        "float" | "float8" | "real" | "double precision" => ValueKind::Float,
        "numeric" | "decimal" | "number" | "money" => ValueKind::Decimal,
        "char" | "character" | "varchar" | "character varying" | "long varchar" => ValueKind::Text,
        "binary" | "varbinary" | "long varbinary" | "bytea" | "raw" => ValueKind::Bytes,
        // Spatial values arrive as the bytes of their (Vertica's own) binary form.
        "geometry" | "geography" => ValueKind::Bytes,
        "date" => ValueKind::Date,
        "time" | "timetz" | "time with time zone" => ValueKind::Time,
        "timestamp" | "datetime" | "smalldatetime" => ValueKind::Timestamp,
        "timestamptz" | "timestamp with time zone" => ValueKind::TimestampTz,
        "uuid" => ValueKind::Uuid,
        "array" | "set" | "row" | "map" => ValueKind::Json,
        _ if base.starts_with("interval") => ValueKind::Interval,
        _ => ValueKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(oid: u32, modifier: i32) -> TypeRef {
        TypeRef {
            oid,
            user_type: None,
            modifier,
        }
    }

    #[test]
    fn names_with_modifiers() {
        assert_eq!(type_name(&t(VARCHAR, 84)), "varchar(80)");
        assert_eq!(type_name(&t(VARCHAR, -1)), "varchar");
        assert_eq!(
            type_name(&t(LONGVARBINARY, 1_048_580)),
            "long varbinary(1048576)"
        );
        // numeric(10,2): modifier = (10 << 16 | 2) + 4.
        assert_eq!(type_name(&t(NUMERIC, (10 << 16 | 2) + 4)), "numeric(10,2)");
        assert_eq!(type_name(&t(NUMERIC, -1)), "numeric");
        assert_eq!(type_name(&t(TIMESTAMPTZ, 3)), "timestamptz(3)");
        assert_eq!(type_name(&t(TIMESTAMP, -1)), "timestamp");
        assert_eq!(type_name(&t(INT8, -1)), "int");
        assert_eq!(type_name(&t(INTERVAL, -1)), "interval day to second");
        assert_eq!(
            type_name(&t(INTERVAL, DAY | HOUR | 6)),
            "interval day to hour"
        );
        assert_eq!(
            type_name(&t(INTERVAL, MINUTE | SECOND)),
            "interval minute to second"
        );
        assert_eq!(type_name(&t(INTERVALYM, YEAR)), "interval year");
        assert_eq!(
            type_name(&t(INTERVALYM, YEAR | MONTH)),
            "interval year to month"
        );
        assert_eq!(type_name(&t(1506, -1)), "array[int]");
        assert_eq!(type_name(&t(2709, -1)), "set[varchar]");
        assert_eq!(type_name(&t(1519, -1)), "array[long varchar]");
        assert_eq!(type_name(&t(ROW, -1)), "row");
        assert_eq!(type_name(&t(999, -1)), "oid 999");
        let geo = TypeRef {
            oid: LONGVARBINARY,
            user_type: Some("GEOMETRY".into()),
            modifier: -1,
        };
        assert_eq!(type_name(&geo), "geometry");
        assert_eq!(kind(&geo), ValueKind::Bytes);
    }

    #[test]
    fn kinds() {
        assert_eq!(kind(&t(INT8, -1)), ValueKind::Int);
        assert_eq!(kind(&t(NUMERIC, -1)), ValueKind::Decimal);
        assert_eq!(kind(&t(TIMETZ, -1)), ValueKind::Time);
        assert_eq!(kind(&t(INTERVALYM, -1)), ValueKind::Interval);
        assert_eq!(kind(&t(2716, -1)), ValueKind::Json);
        assert_eq!(kind(&t(MAP, -1)), ValueKind::Json);
        assert_eq!(kind(&t(1523, -1)), ValueKind::Other);
        assert_eq!(element_oid(1522), Some(BINARY));
        assert_eq!(element_oid(2721), Some(INTERVALYM));
        assert_eq!(element_oid(1504), None);
        assert_eq!(precision(NUMERIC, (37 << 16 | 15) + 4), Some(37));
        assert_eq!(scale(NUMERIC, (37 << 16 | 15) + 4), Some(15));
        // numeric(1000,300): a scale above 255 keeps its high bits.
        assert_eq!(precision(NUMERIC, (1000 << 16 | 300) + 4), Some(1000));
        assert_eq!(scale(NUMERIC, (1000 << 16 | 300) + 4), Some(300));
        assert_eq!(precision(TIMESTAMP, -1), Some(6));
        for (name, k) in [
            ("varchar(80)", ValueKind::Text),
            ("Long Varchar(1048576)", ValueKind::Text),
            ("numeric(37,15)", ValueKind::Decimal),
            ("int", ValueKind::Int),
            ("timestamptz", ValueKind::TimestampTz),
            ("interval day to second", ValueKind::Interval),
            ("Array[int]", ValueKind::Json),
            ("Row(a int, b varchar(10))", ValueKind::Json),
            ("geometry(1000)", ValueKind::Bytes),
            ("uuid", ValueKind::Uuid),
            ("", ValueKind::Other),
        ] {
            assert_eq!(kind_from_name(name), k, "{name}");
        }
    }
}
