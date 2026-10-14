//! PostgreSQL type classification and value decoding.
//!
//! Two decoding paths exist:
//!
//! * **Text** (`text_to_value`): the browsing path. `sqlgen` casts every selected column to
//!   `::text`, so the server's textual form (with the session's `DateStyle = 'ISO, YMD'`,
//!   `IntervalStyle = 'postgres'`, `extra_float_digits = 3`) is decoded losslessly for *any* type,
//!   including ones tokio-postgres knows nothing about (PostGIS, enums, ranges, composites).
//! * **Binary** (`decode_row`): the extended protocol always returns the binary format, so a raw
//!   `query()` that does not cast its columns only gets native decoding for a small set of types
//!   (bool, ints, floats, text-likes, bytea, uuid, json/jsonb, date/time/timestamp). Everything
//!   else is reported as `Value::Other` with a placeholder telling the caller to cast to text.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use tokio_postgres::Row;
use tokio_postgres::types::{FromSql, Kind, Type};

use crate::error::AppError;
use crate::types::query::{ColumnOrigin, ResultColumn};
use crate::types::schema::{TableRef, ValueKind};
use crate::types::value::Value;

/// Separator used by the catalog queries to join arrays into one text column
/// (`string_agg(..., E'\x1f')`), picked because it cannot appear in identifiers.
pub const LIST_SEP: char = '\u{1f}';

/// Splits a `LIST_SEP`-joined catalog column back into its elements.
pub fn split_list(text: Option<&str>) -> Vec<String> {
    match text {
        Some(s) if !s.is_empty() => s.split(LIST_SEP).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

/// Maps a `pg_type.typname` to a `ValueKind`. Handles plain names only; arrays/enums/domains are
/// resolved by the callers that know the type's `typtype`/`typcategory`.
pub fn kind_from_name(type_name: &str) -> ValueKind {
    match type_name {
        "bool" => ValueKind::Bool,
        "int2" | "int4" | "oid" => ValueKind::Int,
        "int8" => ValueKind::BigInt,
        "numeric" => ValueKind::Decimal,
        "float4" | "float8" => ValueKind::Float,
        "text" | "varchar" | "bpchar" | "char" | "name" | "citext" | "xml" => ValueKind::Text,
        "bytea" => ValueKind::Bytes,
        "date" => ValueKind::Date,
        "time" | "timetz" => ValueKind::Time,
        "timestamp" => ValueKind::Timestamp,
        "timestamptz" => ValueKind::TimestampTz,
        "interval" => ValueKind::Interval,
        "uuid" => ValueKind::Uuid,
        "json" | "jsonb" => ValueKind::Json,
        "bit" | "varbit" => ValueKind::Bit,
        "geometry" | "geography" => ValueKind::Geometry,
        _ => ValueKind::Other,
    }
}

/// Classifies a column type from catalog facts: `typname`, `typtype` (`b` base, `e` enum, `d`
/// domain…), `typcategory` (`A` array…) and, for arrays, the element type name.
pub fn kind_from_pg_type(
    type_name: &str,
    typtype: Option<char>,
    typcategory: Option<char>,
    elem: Option<&str>,
) -> ValueKind {
    if typcategory == Some('A') || (type_name.starts_with('_') && type_name.len() > 1) {
        // `elem` is informative only; an array stays an array whatever it contains.
        let _ = elem;
        return ValueKind::Array;
    }
    if typtype == Some('e') {
        return ValueKind::Enum;
    }
    kind_from_name(type_name)
}

/// Classifies a described result-column type.
pub fn kind_from_type(ty: &Type) -> ValueKind {
    match ty.kind() {
        Kind::Array(_) => ValueKind::Array,
        Kind::Enum(_) => ValueKind::Enum,
        Kind::Domain(inner) => kind_from_type(inner),
        _ => kind_from_name(ty.name()),
    }
}

/// Element type name for an array type name (`_int4` → `int4`).
fn elem_name_of(type_name: &str) -> Option<&str> {
    type_name.strip_prefix('_').filter(|s| !s.is_empty())
}

/// Decodes the textual form of a value of kind `kind` (type `type_name`) into a `Value`.
/// Never fails: anything unparseable is kept as `Value::Other` so no data is lost.
pub fn text_to_value(kind: ValueKind, type_name: &str, text: &str) -> Value {
    let elem = elem_name_of(type_name).map(|n| (kind_from_name(n), n));
    text_to_value_with_elem(kind, type_name, elem, text)
}

/// Like `text_to_value` but with an explicit element kind/name for arrays (used when the
/// described `Type` knows the element type, e.g. arrays of enums).
pub fn text_to_value_with_elem(
    kind: ValueKind,
    type_name: &str,
    elem: Option<(ValueKind, &str)>,
    text: &str,
) -> Value {
    match kind {
        ValueKind::Bool => match text {
            "t" | "true" | "TRUE" | "yes" | "on" | "1" => Value::Bool(true),
            "f" | "false" | "FALSE" | "no" | "off" | "0" => Value::Bool(false),
            other => Value::other(type_name, other),
        },
        ValueKind::Int | ValueKind::BigInt => match text.trim().parse::<i64>() {
            Ok(n) => Value::from_i64(n),
            Err(_) => Value::BigInt(text.to_string()),
        },
        ValueKind::Decimal => Value::Decimal(text.to_string()),
        ValueKind::Float => match text.trim().parse::<f64>() {
            Ok(f) => Value::from_f64(type_name, f),
            Err(_) => Value::other(type_name, text),
        },
        ValueKind::Bytes => Value::from_bytes(&bytea_text_to_bytes(text)),
        ValueKind::Date => Value::Date(text.to_string()),
        ValueKind::Time => Value::Time(text.to_string()),
        ValueKind::Timestamp => Value::Timestamp(text.to_string()),
        ValueKind::TimestampTz => Value::TimestampTz(text.to_string()),
        ValueKind::Interval => Value::Interval(text.to_string()),
        ValueKind::Uuid => Value::Uuid(text.to_string()),
        ValueKind::Json => Value::Json(text.to_string()),
        ValueKind::Enum => Value::Enum(text.to_string()),
        ValueKind::Text => Value::Text(text.to_string()),
        ValueKind::Array => match parse_pg_array(text) {
            Some(items) => {
                let (elem_kind, elem_ty) = elem.unwrap_or((ValueKind::Text, type_name));
                Value::Array(
                    items
                        .into_iter()
                        .map(|item| match item {
                            None => Value::Null,
                            // Elements are one-dimensional here; a nested array kind would mean
                            // the literal parser already rejected the value.
                            Some(s) => text_to_value_with_elem(elem_kind, elem_ty, None, &s),
                        })
                        .collect(),
                )
            }
            // Multidimensional (or malformed) literals are kept verbatim.
            None => Value::other(type_name, text),
        },
        ValueKind::Geometry => geometry_value(type_name, text, crate::geometry::convert_to_wkt()),
        ValueKind::Set | ValueKind::Bit | ValueKind::Other => Value::other(type_name, text),
    }
}

/// A PostGIS `geometry` / `geography` value from its text output (hex EWKB): EWKT while the
/// "convert to WKT" preference is on (`SRID=4326;POINT(1 2)`), the hex otherwise or when it does
/// not parse.
fn geometry_value(type_name: &str, text: &str, wkt: bool) -> Value {
    match wkt
        .then(|| crate::geometry::ewkb_hex_to_ewkt(text))
        .flatten()
    {
        Some(ewkt) => Value::other(type_name, ewkt),
        None => Value::other(type_name, text),
    }
}

/// Decodes bytea text output: `\x` hex (the default `bytea_output = 'hex'`); anything else is
/// treated as the legacy escape format and decoded byte by byte.
pub fn bytea_text_to_bytes(text: &str) -> Vec<u8> {
    if let Some(hex) = text.strip_prefix("\\x") {
        let bytes = hex.as_bytes();
        let mut out = Vec::with_capacity(bytes.len() / 2);
        let mut i = 0;
        while i + 1 < bytes.len() {
            let hi = (bytes[i] as char).to_digit(16);
            let lo = (bytes[i + 1] as char).to_digit(16);
            match (hi, lo) {
                (Some(h), Some(l)) => out.push((h * 16 + l) as u8),
                // Not actually hex: fall back to the raw text.
                _ => return text.as_bytes().to_vec(),
            }
            i += 2;
        }
        return out;
    }
    // Escape format: `\\` → backslash, `\nnn` → octal byte, everything else literal.
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                out.push(b'\\');
                i += 2;
                continue;
            }
            if i + 3 < bytes.len() {
                let oct = &text[i + 1..i + 4];
                if let Ok(b) = u8::from_str_radix(oct, 8) {
                    out.push(b);
                    i += 4;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// Parses a one-dimensional PostgreSQL array literal (`{a,"b c",NULL}`, optionally prefixed with
/// a dimension spec like `[1:3]=`). Returns `None` for multidimensional or malformed input.
pub fn parse_pg_array(text: &str) -> Option<Vec<Option<String>>> {
    let mut s = text.trim();
    if s.starts_with('[') {
        let eq = s.find('=')?;
        s = s[eq + 1..].trim_start();
    }
    let inner = s.strip_prefix('{')?.strip_suffix('}')?;
    let chars: Vec<char> = inner.chars().collect();
    let mut items: Vec<Option<String>> = Vec::new();
    let mut i = 0;
    let n = chars.len();

    // Skip leading whitespace; `{}` is the empty array.
    while i < n && chars[i].is_whitespace() {
        i += 1;
    }
    if i == n {
        return Some(items);
    }

    loop {
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= n {
            // Trailing comma or similar.
            return None;
        }
        match chars[i] {
            '{' => return None, // nested array → multidimensional
            '"' => {
                i += 1;
                let mut buf = String::new();
                let mut closed = false;
                while i < n {
                    match chars[i] {
                        '\\' if i + 1 < n => {
                            buf.push(chars[i + 1]);
                            i += 2;
                        }
                        '"' => {
                            closed = true;
                            i += 1;
                            break;
                        }
                        c => {
                            buf.push(c);
                            i += 1;
                        }
                    }
                }
                if !closed {
                    return None;
                }
                items.push(Some(buf));
            }
            _ => {
                let mut buf = String::new();
                while i < n && chars[i] != ',' {
                    match chars[i] {
                        '\\' if i + 1 < n => {
                            buf.push(chars[i + 1]);
                            i += 2;
                        }
                        '{' | '}' | '"' => return None,
                        c => {
                            buf.push(c);
                            i += 1;
                        }
                    }
                }
                let trimmed = buf.trim_end();
                if trimmed.eq_ignore_ascii_case("NULL") {
                    items.push(None);
                } else {
                    items.push(Some(trimmed.to_string()));
                }
            }
        }
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= n {
            return Some(items);
        }
        if chars[i] != ',' {
            return None;
        }
        i += 1;
    }
}

// ---------------------------------------------------------------------------------------------
// Binary row decoding (extended protocol)
// ---------------------------------------------------------------------------------------------

/// Accepts every type and captures the raw cell bytes (or `None` for NULL), so NULL detection
/// works uniformly and the decoder below can interpret the bytes it understands.
struct RawCell(Option<Vec<u8>>);

impl<'a> FromSql<'a> for RawCell {
    fn from_sql(
        _ty: &Type,
        raw: &'a [u8],
    ) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(RawCell(Some(raw.to_vec())))
    }

    fn from_sql_null(_ty: &Type) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(RawCell(None))
    }

    fn accepts(_ty: &Type) -> bool {
        true
    }
}

/// Describes a result column for the renderer.
pub fn result_column(col: &tokio_postgres::Column) -> ResultColumn {
    ResultColumn {
        name: col.name().to_string(),
        data_type: col.type_().name().to_string(),
        kind: kind_from_type(col.type_()),
        nullable: None,
        table: None,
        origin: None,
    }
}

/// Names of one relation as the catalog reported them: its schema and name, and its columns by
/// attribute number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationNames {
    pub schema: String,
    pub name: String,
    pub columns: HashMap<i16, String>,
}

/// What `RelationNames` lookups found, by table OID; `None`: no such relation (dropped).
pub type RelationCache = HashMap<u32, Option<Arc<RelationNames>>>;

tokio::task_local! {
    /// Relation names looked up while one SQL-editor run executes (`with_origin_cache`), so a
    /// script of many SELECTs on the same tables asks the catalog once per table.
    static ORIGIN_CACHE: RefCell<RelationCache>;
}

/// Runs `fut` (one SQL-editor run) with a fresh cache of the relation names that resolve result
/// column origins. Outside such a scope every lookup goes to the catalog.
pub async fn with_origin_cache<F: std::future::Future>(fut: F) -> F::Output {
    ORIGIN_CACHE.scope(RefCell::new(HashMap::new()), fut).await
}

/// Forgets the cached names: a statement of the run may have renamed or dropped a relation.
pub fn clear_origin_cache() {
    let _ = ORIGIN_CACHE.try_with(|c| c.borrow_mut().clear());
}

/// The cached lookup of `oid`: `None` when it was not looked up yet in this run (or there is no
/// run scope), `Some(None)` when the relation does not exist.
pub fn cached_relation(oid: u32) -> Option<Option<Arc<RelationNames>>> {
    ORIGIN_CACHE
        .try_with(|c| c.borrow().get(&oid).cloned())
        .ok()
        .flatten()
}

/// Remembers lookups for the rest of the run (no-op outside a run scope).
pub fn cache_relations(found: &RelationCache) {
    let _ = ORIGIN_CACHE.try_with(|c| {
        let mut cache = c.borrow_mut();
        for (oid, names) in found {
            cache.insert(*oid, names.clone());
        }
    });
}

/// The table OID and attribute number a RowDescription field reports, when it is a plain column
/// of a relation (0 / missing for expressions; negative attribute numbers are system columns such
/// as `ctid`, which cannot be edited).
pub fn column_source(col: &tokio_postgres::Column) -> Option<(u32, i16)> {
    let oid = col.table_oid().filter(|o| *o != 0)?;
    let attnum = col.column_id().filter(|n| *n > 0)?;
    Some((oid, attnum))
}

/// Origins of result columns from their `column_source` and the relations looked up.
pub fn origins_from(
    sources: &[Option<(u32, i16)>],
    relations: &RelationCache,
) -> Vec<Option<ColumnOrigin>> {
    sources
        .iter()
        .map(|src| {
            let (oid, attnum) = (*src)?;
            let rel = relations.get(&oid)?.as_ref()?;
            let column = rel.columns.get(&attnum)?;
            // RowDescription does not say which reference to the table a column came through
            // (a self-join's two sides look alike): no instance.
            Some(ColumnOrigin {
                table: TableRef::new(Some(rel.schema.as_str()), &rel.name),
                column: column.clone(),
                instance: None,
            })
        })
        .collect()
}

/// Decodes one binary-format row by its described column types.
pub fn decode_row(row: &Row) -> Result<Vec<Value>, AppError> {
    let mut out = Vec::with_capacity(row.len());
    for (i, col) in row.columns().iter().enumerate() {
        let cell: RawCell = row.try_get(i)?;
        out.push(match cell.0 {
            None => Value::Null,
            Some(raw) => binary_to_value(col.type_(), &raw),
        });
    }
    Ok(out)
}

const PLACEHOLDER: &str = "<binary; cast to text>";

/// Microseconds between 1970-01-01 and 2000-01-01 (the PostgreSQL epoch).
const PG_EPOCH_OFFSET_MICROS: i64 = 946_684_800_000_000;
const PG_EPOCH_OFFSET_DAYS: i64 = 10_957;
/// `date` special values are stored as the i32 extremes.
const DATE_POS_INFINITY: i64 = i32::MAX as i64;
const DATE_NEG_INFINITY: i64 = i32::MIN as i64;

fn binary_to_value(ty: &Type, raw: &[u8]) -> Value {
    let name = ty.name();
    if let Kind::Domain(inner) = ty.kind() {
        return binary_to_value(inner, raw);
    }
    match *ty {
        Type::BOOL => Value::Bool(raw.first().copied().unwrap_or(0) != 0),
        Type::INT2 => be_int(raw, 2).map_or_else(|| other(name), Value::from_i64),
        Type::INT4 => be_int(raw, 4).map_or_else(|| other(name), Value::from_i64),
        Type::INT8 => be_int(raw, 8).map_or_else(|| other(name), Value::from_i64),
        Type::OID => raw.try_into().ok().map_or_else(
            || other(name),
            |b| Value::from_u64(u32::from_be_bytes(b) as u64),
        ),
        Type::FLOAT4 => raw.try_into().ok().map_or_else(
            || other(name),
            |b| Value::from_f64(name, f32::from_be_bytes(b) as f64),
        ),
        Type::FLOAT8 => raw.try_into().ok().map_or_else(
            || other(name),
            |b| Value::from_f64(name, f64::from_be_bytes(b)),
        ),
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::UNKNOWN => {
            Value::Text(String::from_utf8_lossy(raw).into_owned())
        }
        Type::CHAR => Value::Text(String::from_utf8_lossy(raw).into_owned()),
        Type::BYTEA => Value::from_bytes(raw),
        Type::UUID => raw.try_into().ok().map_or_else(
            || other(name),
            |b: [u8; 16]| Value::Uuid(uuid::Uuid::from_bytes(b).hyphenated().to_string()),
        ),
        Type::JSON => Value::Json(String::from_utf8_lossy(raw).into_owned()),
        // jsonb binary = one version byte (1) followed by the JSON text.
        Type::JSONB => match raw.split_first() {
            Some((1, rest)) => Value::Json(String::from_utf8_lossy(rest).into_owned()),
            _ => other(name),
        },
        Type::DATE => match be_int(raw, 4) {
            Some(DATE_POS_INFINITY) => Value::Date("infinity".into()),
            Some(DATE_NEG_INFINITY) => Value::Date("-infinity".into()),
            Some(days) => {
                chrono::DateTime::from_timestamp((days + PG_EPOCH_OFFSET_DAYS) * 86_400, 0)
                    .map_or_else(
                        || other(name),
                        |d| Value::Date(d.format("%Y-%m-%d").to_string()),
                    )
            }
            None => other(name),
        },
        Type::TIME => be_int(raw, 8).map_or_else(
            || other(name),
            |micros| Value::Time(format_time_of_day(micros)),
        ),
        Type::TIMESTAMP | Type::TIMESTAMPTZ => match be_int(raw, 8) {
            Some(i64::MAX) => stamp(ty, "infinity"),
            Some(i64::MIN) => stamp(ty, "-infinity"),
            Some(micros) => {
                let unix = micros + PG_EPOCH_OFFSET_MICROS;
                chrono::DateTime::from_timestamp_micros(unix).map_or_else(
                    || other(name),
                    |dt| {
                        let text = dt.format("%Y-%m-%d %H:%M:%S%.6f").to_string();
                        let text = trim_fraction(&text);
                        if *ty == Type::TIMESTAMPTZ {
                            Value::TimestampTz(format!("{text}+00"))
                        } else {
                            Value::Timestamp(text)
                        }
                    },
                )
            }
            None => other(name),
        },
        _ => match ty.name() {
            // citext is an extension type with a text payload.
            "citext" => Value::Text(String::from_utf8_lossy(raw).into_owned()),
            _ => other(name),
        },
    }
}

fn other(name: &str) -> Value {
    Value::other(name, PLACEHOLDER)
}

fn stamp(ty: &Type, text: &str) -> Value {
    if *ty == Type::TIMESTAMPTZ {
        Value::TimestampTz(text.to_string())
    } else {
        Value::Timestamp(text.to_string())
    }
}

/// Big-endian signed integer of `width` bytes.
fn be_int(raw: &[u8], width: usize) -> Option<i64> {
    if raw.len() != width {
        return None;
    }
    Some(match width {
        2 => i16::from_be_bytes([raw[0], raw[1]]) as i64,
        4 => i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as i64,
        8 => i64::from_be_bytes([
            raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
        ]),
        _ => return None,
    })
}

fn format_time_of_day(micros: i64) -> String {
    let secs = micros.div_euclid(1_000_000);
    let frac = micros.rem_euclid(1_000_000);
    let text = format!(
        "{:02}:{:02}:{:02}.{:06}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60,
        frac
    );
    trim_fraction(&text)
}

/// Drops trailing zeros of a fractional-seconds part (and the dot when nothing remains), matching
/// the server's text output.
fn trim_fraction(text: &str) -> String {
    match text.rfind('.') {
        Some(dot) if text[dot + 1..].chars().all(|c| c.is_ascii_digit()) => {
            let trimmed = text.trim_end_matches('0');
            trimmed.strip_suffix('.').unwrap_or(trimmed).to_string()
        }
        _ => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::value::BytesValue;

    #[test]
    fn results_origins_from_relations() {
        let users = RelationNames {
            schema: "public".into(),
            name: "users".into(),
            columns: HashMap::from([(1, "id".to_string()), (3, "email".to_string())]),
        };
        let relations: RelationCache =
            HashMap::from([(16384, Some(Arc::new(users))), (16999, None)]);
        let origins = origins_from(
            &[
                Some((16384, 1)),
                Some((16384, 3)),
                None,             // an expression
                Some((16384, 2)), // a column unknown to the lookup (dropped meanwhile)
                Some((16999, 1)), // a relation that no longer exists
                Some((17000, 1)), // never looked up
            ],
            &relations,
        );
        let o = origins[0].as_ref().unwrap();
        assert_eq!(o.table, TableRef::new(Some("public"), "users"));
        assert_eq!(o.column, "id");
        assert_eq!(origins[1].as_ref().unwrap().column, "email");
        assert!(origins[2..].iter().all(Option::is_none));
    }

    #[tokio::test]
    async fn results_origin_cache_is_scoped_to_a_run() {
        let names = Some(Arc::new(RelationNames {
            schema: "s".into(),
            name: "t".into(),
            columns: HashMap::new(),
        }));
        // Outside a run nothing is cached.
        cache_relations(&HashMap::from([(1, names.clone())]));
        assert_eq!(cached_relation(1), None);
        with_origin_cache(async {
            assert_eq!(cached_relation(1), None);
            cache_relations(&HashMap::from([(1, names.clone()), (2, None)]));
            assert_eq!(cached_relation(1), Some(names.clone()));
            assert_eq!(cached_relation(2), Some(None));
            clear_origin_cache();
            assert_eq!(cached_relation(1), None);
        })
        .await;
        // A new run starts empty.
        with_origin_cache(async { assert_eq!(cached_relation(1), None) }).await;
    }

    #[test]
    fn parses_array_literals() {
        assert_eq!(parse_pg_array("{}"), Some(vec![]));
        assert_eq!(
            parse_pg_array("{a,b}"),
            Some(vec![Some("a".into()), Some("b".into())])
        );
        assert_eq!(
            parse_pg_array(r#"{"a b","c\"d",NULL}"#),
            Some(vec![Some("a b".into()), Some("c\"d".into()), None])
        );
        assert_eq!(
            parse_pg_array(r#"{"back\\slash",x}"#),
            Some(vec![Some("back\\slash".into()), Some("x".into())])
        );
        assert_eq!(
            parse_pg_array("[1:3]={1,2,3}"),
            Some(vec![Some("1".into()), Some("2".into()), Some("3".into())])
        );
        assert_eq!(
            parse_pg_array("{ 1 , 2 }"),
            Some(vec![Some("1".into()), Some("2".into())])
        );
        // A quoted "NULL" is the string, an unquoted one is SQL NULL.
        assert_eq!(
            parse_pg_array(r#"{"NULL",null}"#),
            Some(vec![Some("NULL".into()), None])
        );
        assert_eq!(parse_pg_array("{{1,2},{3,4}}"), None);
        assert_eq!(parse_pg_array("not an array"), None);
        assert_eq!(parse_pg_array("{a,}"), None);
        assert_eq!(parse_pg_array(r#"{"unterminated}"#), None);
    }

    #[test]
    fn decodes_text_for_every_kind() {
        assert_eq!(
            text_to_value(ValueKind::Bool, "bool", "t"),
            Value::Bool(true)
        );
        assert_eq!(
            text_to_value(ValueKind::Bool, "bool", "f"),
            Value::Bool(false)
        );
        assert_eq!(text_to_value(ValueKind::Int, "int4", "42"), Value::Int(42));
        assert_eq!(
            text_to_value(ValueKind::BigInt, "int8", "9007199254740993"),
            Value::BigInt("9007199254740993".into())
        );
        assert_eq!(
            text_to_value(ValueKind::BigInt, "int8", "-7"),
            Value::Int(-7)
        );
        assert_eq!(
            text_to_value(ValueKind::Decimal, "numeric", "1.500"),
            Value::Decimal("1.500".into())
        );
        assert_eq!(
            text_to_value(ValueKind::Float, "float8", "1.5"),
            Value::Float(1.5)
        );
        assert_eq!(
            text_to_value(ValueKind::Float, "float8", "NaN"),
            Value::other("float8", "NaN")
        );
        assert_eq!(
            text_to_value(ValueKind::Float, "float8", "Infinity"),
            Value::other("float8", "inf")
        );
        match text_to_value(ValueKind::Bytes, "bytea", "\\x00ff") {
            Value::Bytes(BytesValue { len, base64, .. }) => {
                assert_eq!(len, 2);
                assert_eq!(base64, "AP8=");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            text_to_value(ValueKind::Date, "date", "2026-10-11"),
            Value::Date("2026-10-11".into())
        );
        assert_eq!(
            text_to_value(ValueKind::Time, "time", "12:34:56"),
            Value::Time("12:34:56".into())
        );
        assert_eq!(
            text_to_value(ValueKind::Timestamp, "timestamp", "2026-10-11 12:34:56.5"),
            Value::Timestamp("2026-10-11 12:34:56.5".into())
        );
        assert_eq!(
            text_to_value(
                ValueKind::TimestampTz,
                "timestamptz",
                "2026-10-11 12:34:56+07"
            ),
            Value::TimestampTz("2026-10-11 12:34:56+07".into())
        );
        assert_eq!(
            text_to_value(ValueKind::Interval, "interval", "1 day 02:00:00"),
            Value::Interval("1 day 02:00:00".into())
        );
        let u = "123e4567-e89b-12d3-a456-426614174000";
        assert_eq!(
            text_to_value(ValueKind::Uuid, "uuid", u),
            Value::Uuid(u.into())
        );
        assert_eq!(
            text_to_value(ValueKind::Json, "jsonb", r#"{"a": 1}"#),
            Value::Json(r#"{"a": 1}"#.into())
        );
        assert_eq!(
            text_to_value(ValueKind::Enum, "mood", "happy"),
            Value::Enum("happy".into())
        );
        assert_eq!(
            text_to_value(ValueKind::Text, "text", "hi"),
            Value::Text("hi".into())
        );
        assert_eq!(
            text_to_value(ValueKind::Array, "_text", r#"{"a","b",NULL}"#),
            Value::Array(vec![
                Value::Text("a".into()),
                Value::Text("b".into()),
                Value::Null
            ])
        );
        assert_eq!(
            text_to_value(ValueKind::Array, "_int4", "{1,2}"),
            Value::Array(vec![Value::Int(1), Value::Int(2)])
        );
        assert_eq!(
            text_to_value(ValueKind::Array, "_int4", "{{1,2},{3,4}}"),
            Value::other("_int4", "{{1,2},{3,4}}")
        );
        assert_eq!(
            text_to_value_with_elem(
                ValueKind::Array,
                "_mood",
                Some((ValueKind::Enum, "mood")),
                "{happy,sad}"
            ),
            Value::Array(vec![Value::Enum("happy".into()), Value::Enum("sad".into())])
        );
        assert_eq!(
            text_to_value(ValueKind::Bit, "bit", "101"),
            Value::other("bit", "101")
        );
        assert_eq!(
            text_to_value(ValueKind::Geometry, "geometry", "0101"),
            Value::other("geometry", "0101")
        );
        assert_eq!(
            text_to_value(ValueKind::Other, "inet", "10.0.0.1/32"),
            Value::other("inet", "10.0.0.1/32")
        );
    }

    #[test]
    fn decodes_postgis_hex_ewkb_as_ewkt() {
        let hex = "0101000020E6100000000000000000F03F0000000000000040";
        assert_eq!(
            geometry_value("geometry", hex, true),
            Value::other("geometry", "SRID=4326;POINT(1 2)")
        );
        assert_eq!(
            geometry_value("geography", hex, true),
            Value::other("geography", "SRID=4326;POINT(1 2)")
        );
        // The preference off, or text that is not hex EWKB: kept as it came.
        assert_eq!(
            geometry_value("geometry", hex, false),
            Value::other("geometry", hex)
        );
        assert_eq!(
            geometry_value("geometry", "POINT(1 2)", true),
            Value::other("geometry", "POINT(1 2)")
        );
    }

    #[test]
    fn decodes_bytea_escape_format() {
        assert_eq!(bytea_text_to_bytes("a\\\\b\\001"), b"a\\b\x01".to_vec());
        assert_eq!(bytea_text_to_bytes("\\xzz"), b"\\xzz".to_vec());
    }

    #[test]
    fn classifies_catalog_types() {
        assert_eq!(
            kind_from_pg_type("int4", Some('b'), Some('N'), None),
            ValueKind::Int
        );
        assert_eq!(
            kind_from_pg_type("int8", Some('b'), Some('N'), None),
            ValueKind::BigInt
        );
        assert_eq!(
            kind_from_pg_type("_text", Some('b'), Some('A'), Some("text")),
            ValueKind::Array
        );
        assert_eq!(
            kind_from_pg_type("_mood", Some('b'), Some('A'), Some("mood")),
            ValueKind::Array
        );
        assert_eq!(
            kind_from_pg_type("mood", Some('e'), Some('E'), None),
            ValueKind::Enum
        );
        assert_eq!(
            kind_from_pg_type("timestamptz", Some('b'), Some('D'), None),
            ValueKind::TimestampTz
        );
        assert_eq!(
            kind_from_pg_type("jsonb", Some('b'), Some('U'), None),
            ValueKind::Json
        );
        assert_eq!(
            kind_from_pg_type("bytea", Some('b'), Some('U'), None),
            ValueKind::Bytes
        );
        assert_eq!(
            kind_from_pg_type("varbit", Some('b'), Some('V'), None),
            ValueKind::Bit
        );
        assert_eq!(
            kind_from_pg_type("geography", Some('b'), Some('U'), None),
            ValueKind::Geometry
        );
        assert_eq!(
            kind_from_pg_type("inet", Some('b'), Some('I'), None),
            ValueKind::Other
        );
        assert_eq!(
            kind_from_pg_type("citext", None, None, None),
            ValueKind::Text
        );
        // A lone underscore is not an array name.
        assert_eq!(kind_from_pg_type("_", None, None, None), ValueKind::Other);
    }

    #[test]
    fn classifies_described_types() {
        assert_eq!(kind_from_type(&Type::BOOL), ValueKind::Bool);
        assert_eq!(kind_from_type(&Type::INT8), ValueKind::BigInt);
        assert_eq!(kind_from_type(&Type::TEXT_ARRAY), ValueKind::Array);
        assert_eq!(kind_from_type(&Type::NUMERIC), ValueKind::Decimal);
        assert_eq!(kind_from_type(&Type::INET), ValueKind::Other);
    }

    #[test]
    fn decodes_binary_cells() {
        assert_eq!(binary_to_value(&Type::BOOL, &[1]), Value::Bool(true));
        assert_eq!(binary_to_value(&Type::INT2, &[0xff, 0xfe]), Value::Int(-2));
        assert_eq!(
            binary_to_value(&Type::INT4, &42i32.to_be_bytes()),
            Value::Int(42)
        );
        assert_eq!(
            binary_to_value(&Type::INT8, &(i64::MAX).to_be_bytes()),
            Value::BigInt(i64::MAX.to_string())
        );
        assert_eq!(
            binary_to_value(&Type::OID, &16u32.to_be_bytes()),
            Value::Int(16)
        );
        assert_eq!(
            binary_to_value(&Type::FLOAT8, &1.5f64.to_be_bytes()),
            Value::Float(1.5)
        );
        assert_eq!(
            binary_to_value(&Type::FLOAT4, &f32::NAN.to_be_bytes()),
            Value::other("float4", "NaN")
        );
        assert_eq!(
            binary_to_value(&Type::TEXT, b"hi"),
            Value::Text("hi".into())
        );
        assert_eq!(
            binary_to_value(&Type::JSONB, b"\x01{\"a\":1}"),
            Value::Json("{\"a\":1}".into())
        );
        assert_eq!(
            binary_to_value(&Type::DATE, &0i32.to_be_bytes()),
            Value::Date("2000-01-01".into())
        );
        assert_eq!(
            binary_to_value(&Type::TIMESTAMP, &1_500_000i64.to_be_bytes()),
            Value::Timestamp("2000-01-01 00:00:01.5".into())
        );
        assert_eq!(
            binary_to_value(&Type::TIMESTAMPTZ, &0i64.to_be_bytes()),
            Value::TimestampTz("2000-01-01 00:00:00+00".into())
        );
        assert_eq!(
            binary_to_value(&Type::TIME, &(3_600_000_000i64 + 250_000).to_be_bytes()),
            Value::Time("01:00:00.25".into())
        );
        assert_eq!(
            binary_to_value(&Type::NUMERIC, &[0, 0]),
            Value::other("numeric", PLACEHOLDER)
        );
        match binary_to_value(&Type::BYTEA, &[1, 2, 3]) {
            Value::Bytes(b) => assert_eq!(b.len, 3),
            other => panic!("unexpected {other:?}"),
        }
    }
}
