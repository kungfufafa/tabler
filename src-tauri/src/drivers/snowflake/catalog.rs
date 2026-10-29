//! Snowflake's catalog through `SHOW`, which needs no running warehouse (INFORMATION_SCHEMA
//! does, and refuses unselective filters), and the parsers of its rows. The statements come
//! from `sqlgen::dialect::snowflake`; rows are read by column name, as `SHOW` adds columns
//! between releases.
//!
//! An account holds databases, a database schemas, a schema tables (permanent, transient,
//! temporary, dynamic, hybrid, Iceberg, external), views and materialized views. A session sees
//! one database at a time. Primary, unique and foreign keys are informational (enforced only on
//! hybrid tables, which also have indexes); unquoted names are stored upper case, so names are
//! always quoted with the case `SHOW` returns.

use std::collections::BTreeMap;

use crate::types::objects::{RoutineKind, RoutineSummary, SequenceSummary};
use crate::types::schema::{
    ColumnMeta, DatabaseInfo, ForeignKeyMeta, IndexMeta, SchemaInfo, TableKind, TableRef,
    TableSummary,
};
use crate::types::value::Value;

use super::decode::SfType;

/// Rows of a `SHOW` (or any result) with the names of their columns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

/// One row, read by column name (case-insensitive: `SHOW` names its columns in lower case).
pub struct Row<'a> {
    columns: &'a [String],
    values: &'a [Value],
}

impl<'a> Row<'a> {
    fn value(&self, name: &str) -> Option<&'a Value> {
        let i = self
            .columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case(name))?;
        self.values.get(i).filter(|v| !v.is_null())
    }

    /// Text of a column; None for NULL or a missing column.
    pub fn text(&self, name: &str) -> Option<String> {
        self.value(name).and_then(Value::as_text)
    }

    /// Text that is not empty.
    pub fn some(&self, name: &str) -> Option<String> {
        self.text(name).filter(|s| !s.trim().is_empty())
    }

    /// `Y` / `true` (SHOW spells flags both ways).
    pub fn flag(&self, name: &str) -> bool {
        match self.value(name) {
            Some(Value::Bool(b)) => *b,
            Some(v) => v.as_text().is_some_and(|s| {
                matches!(
                    s.trim().to_ascii_lowercase().as_str(),
                    "y" | "yes" | "true" | "1"
                )
            }),
            None => false,
        }
    }

    pub fn int(&self, name: &str) -> Option<i64> {
        match self.value(name)? {
            Value::Int(n) => Some(*n),
            v => v.as_text()?.trim().split('.').next()?.parse().ok(),
        }
    }
}

impl Rows {
    pub fn iter(&self) -> impl Iterator<Item = Row<'_>> {
        self.rows.iter().map(|values| Row {
            columns: &self.columns,
            values,
        })
    }

    /// The `name` of the last row: where the next page of a SHOW starts (`LIMIT … FROM`).
    pub fn last_name(&self) -> Option<String> {
        self.iter().last().and_then(|r| r.text("name"))
    }
}

/// `SHOW DATABASES`.
pub fn databases(rows: &Rows) -> Vec<DatabaseInfo> {
    rows.iter()
        .filter_map(|r| {
            Some(DatabaseInfo {
                name: r.text("name")?,
                owner: r.some("owner"),
                encoding: None,
                is_current: r.flag("is_current"),
            })
        })
        .collect()
}

/// The schemas only Snowflake keeps (the descriptor's `system_schemas`).
pub fn is_system_schema(name: &str) -> bool {
    name.eq_ignore_ascii_case("INFORMATION_SCHEMA")
}

/// `SHOW SCHEMAS IN DATABASE …`; the session's schema is the default.
pub fn schemas(rows: &Rows) -> Vec<SchemaInfo> {
    rows.iter()
        .filter_map(|r| {
            Some(SchemaInfo {
                name: r.text("name")?,
                is_default: r.flag("is_current"),
            })
        })
        .collect()
}

/// `SHOW TABLES IN SCHEMA …`: tables of every kind, with their row count and size from the
/// metadata (no warehouse).
pub fn tables(rows: &Rows) -> Vec<TableSummary> {
    rows.iter()
        .filter_map(|r| {
            let kind = if r.flag("is_external") {
                TableKind::ForeignTable
            } else {
                TableKind::Table
            };
            Some(TableSummary {
                name: r.text("name")?,
                schema: r.some("schema_name"),
                kind,
                row_estimate: r.int("rows"),
                size_bytes: r.int("bytes"),
                comment: r.some("comment"),
            })
        })
        .collect()
}

/// `SHOW VIEWS IN SCHEMA …` (materialized views too).
pub fn views(rows: &Rows) -> Vec<TableSummary> {
    rows.iter()
        .filter_map(|r| {
            Some(TableSummary {
                name: r.text("name")?,
                schema: r.some("schema_name"),
                kind: if r.flag("is_materialized") {
                    TableKind::MaterializedView
                } else {
                    TableKind::View
                },
                row_estimate: None,
                size_bytes: None,
                comment: r.some("comment"),
            })
        })
        .collect()
}

/// The `text` of a view's row (`SHOW VIEWS LIKE …`): its CREATE statement, empty for a secure
/// view the role does not own.
pub fn view_text(rows: &Rows, name: &str) -> Option<String> {
    rows.iter()
        .find(|r| r.text("name").as_deref() == Some(name))
        .and_then(|r| r.some("text"))
}

/// A column type from the `data_type` JSON of `SHOW COLUMNS`
/// (`{"type":"FIXED","precision":38,"scale":0,"nullable":true}`); its nullability beside it.
pub fn column_type(json: &str) -> (SfType, Option<String>, Option<bool>) {
    let Ok(serde_json::Value::Object(o)) = serde_json::from_str::<serde_json::Value>(json) else {
        let ty = SfType::named(json, None, None, None);
        return (ty, None, None);
    };
    let num = |k: &str| o.get(k).and_then(serde_json::Value::as_u64);
    let name = o
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let ty = SfType::named(
        name,
        num("length").and_then(|n| u32::try_from(n).ok()),
        num("precision").and_then(|n| u8::try_from(n).ok()),
        num("scale").and_then(|n| u8::try_from(n).ok()),
    );
    // A vector names its element type and dimension.
    let vector = || {
        let elem = ["vectorElementType", "elementType", "element_type"]
            .iter()
            .find_map(|k| o.get(*k).and_then(serde_json::Value::as_str))?;
        let dim = ["vectorDimension", "dimension"]
            .iter()
            .find_map(|k| num(k))?;
        let elem = if elem.to_ascii_uppercase().starts_with("INT") {
            "INT"
        } else {
            "FLOAT"
        };
        Some(format!("VECTOR({elem}, {dim})"))
    };
    let spelled = match ty {
        SfType::Vector => vector(),
        _ => None,
    };
    let nullable = o.get("nullable").and_then(serde_json::Value::as_bool);
    (ty, spelled, nullable)
}

/// `SHOW COLUMNS IN TABLE …`, in their order, with the primary key's columns (key order) marked.
pub fn columns(rows: &Rows, primary_key: &[String]) -> Vec<ColumnMeta> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let name = r.text("column_name")?;
            let (ty, spelled, json_nullable) =
                column_type(&r.text("data_type").unwrap_or_default());
            let nullable = match r.text("null?") {
                Some(s) => !matches!(
                    s.trim().to_ascii_uppercase().as_str(),
                    "FALSE" | "N" | "NOT_NULL"
                ),
                None => json_nullable.unwrap_or(true),
            };
            let pk_at = primary_key.iter().position(|k| *k == name);
            let autoincrement = r.some("autoincrement");
            let expression = r.some("expression");
            let (precision, scale, length) = match &ty {
                SfType::Fixed { precision, scale } => {
                    (precision.map(i32::from), scale.map(i32::from), None)
                }
                SfType::Text { length } | SfType::Binary { length } => {
                    (None, None, length.map(i64::from))
                }
                _ => (None, None, None),
            };
            Some(ColumnMeta {
                name,
                ordinal: u32::try_from(i + 1).unwrap_or(u32::MAX),
                data_type: spelled.unwrap_or_else(|| ty.sql_name()),
                kind: ty.kind(),
                nullable,
                default_expr: r.some("default"),
                is_pk: pk_at.is_some(),
                pk_ordinal: pk_at.and_then(|p| u32::try_from(p + 1).ok()),
                is_auto_increment: autoincrement.is_some(),
                is_generated: r
                    .text("kind")
                    .is_some_and(|k| k.eq_ignore_ascii_case("VIRTUAL_COLUMN"))
                    || expression.is_some(),
                is_unsigned: false,
                comment: r.some("comment"),
                enum_values: None,
                char_max_len: length,
                numeric_precision: precision,
                numeric_scale: scale,
                collation: None,
                array_elem_type: None,
                extra: autoincrement.or(expression),
                base_type: None,
            })
        })
        .collect()
}

/// `(constraint, columns in key order)` of `SHOW PRIMARY KEYS` / `SHOW UNIQUE KEYS` rows of one
/// table, by constraint.
fn keys(rows: &Rows) -> BTreeMap<String, Vec<(i64, String)>> {
    let mut by_name: BTreeMap<String, Vec<(i64, String)>> = BTreeMap::new();
    for r in rows.iter() {
        let (Some(column), Some(seq)) = (r.text("column_name"), r.int("key_sequence")) else {
            continue;
        };
        let name = r.text("constraint_name").unwrap_or_default();
        by_name.entry(name).or_default().push((seq, column));
    }
    for cols in by_name.values_mut() {
        cols.sort();
    }
    by_name
}

/// The primary key's columns in key order (`SHOW PRIMARY KEYS IN TABLE …`).
pub fn primary_key(rows: &Rows) -> (Option<String>, Vec<String>) {
    match keys(rows).into_iter().next() {
        Some((name, cols)) => (
            Some(name).filter(|n| !n.is_empty()),
            cols.into_iter().map(|(_, c)| c).collect(),
        ),
        None => (None, Vec::new()),
    }
}

/// Unique constraints as indexes (`SHOW UNIQUE KEYS IN TABLE …`), and the primary key as one.
pub fn key_indexes(primary: &Rows, unique: &Rows) -> Vec<IndexMeta> {
    let index = |name: String, cols: Vec<(i64, String)>, primary: bool| IndexMeta {
        name,
        columns: cols.into_iter().map(|(_, c)| c).collect(),
        unique: true,
        primary,
        method: None,
        predicate: None,
        definition: None,
        comment: None,
        include: Vec::new(),
    };
    keys(primary)
        .into_iter()
        .map(|(n, c)| index(n, c, true))
        .chain(keys(unique).into_iter().map(|(n, c)| index(n, c, false)))
        .collect()
}

/// `[A, B]` or `["A","B"]` of `SHOW INDEXES` as names.
fn name_list(text: &str) -> Vec<String> {
    if let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(text) {
        return items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
    }
    text.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// The indexes of a hybrid table (`SHOW INDEXES IN TABLE …`).
pub fn hybrid_indexes(rows: &Rows) -> Vec<IndexMeta> {
    rows.iter()
        .filter_map(|r| {
            Some(IndexMeta {
                name: r.text("name")?,
                columns: name_list(&r.text("columns").unwrap_or_default()),
                unique: r.flag("is_unique"),
                primary: false,
                method: None,
                predicate: None,
                definition: None,
                comment: None,
                include: name_list(&r.text("included_columns").unwrap_or_default()),
            })
        })
        .collect()
}

/// The table's foreign keys (`SHOW IMPORTED KEYS IN TABLE …`), grouped by constraint in key
/// order. A key to another database names only the schema and table (TableRef has no database).
pub fn foreign_keys(rows: &Rows) -> Vec<ForeignKeyMeta> {
    struct Part {
        seq: i64,
        column: String,
        ref_column: String,
    }
    let mut by_name: BTreeMap<String, (TableRef, String, String, Vec<Part>)> = BTreeMap::new();
    for r in rows.iter() {
        let (Some(column), Some(ref_column), Some(ref_table)) = (
            r.text("fk_column_name"),
            r.text("pk_column_name"),
            r.text("pk_table_name"),
        ) else {
            continue;
        };
        let name = r.text("fk_name").unwrap_or_default();
        let entry = by_name.entry(name).or_insert_with(|| {
            (
                TableRef {
                    schema: r.some("pk_schema_name"),
                    name: ref_table,
                },
                r.some("update_rule").unwrap_or_else(|| "NO ACTION".into()),
                r.some("delete_rule").unwrap_or_else(|| "NO ACTION".into()),
                Vec::new(),
            )
        });
        entry.3.push(Part {
            seq: r.int("key_sequence").unwrap_or(0),
            column,
            ref_column,
        });
    }
    by_name
        .into_iter()
        .map(|(name, (ref_table, on_update, on_delete, mut parts))| {
            parts.sort_by_key(|p| p.seq);
            ForeignKeyMeta {
                name,
                columns: parts.iter().map(|p| p.column.clone()).collect(),
                ref_table,
                ref_columns: parts.iter().map(|p| p.ref_column.clone()).collect(),
                on_update,
                on_delete,
                definition: None,
            }
        })
        .collect()
}

/// `NAME(NUMBER, VARCHAR) RETURN NUMBER` of SHOW FUNCTIONS / PROCEDURES as its argument types and
/// return type.
pub fn signature(arguments: &str) -> (String, Option<String>) {
    let Some(open) = arguments.find('(') else {
        return (String::new(), None);
    };
    // The argument list ends at the parenthesis that closes the first one.
    let mut depth = 0usize;
    let mut close = None;
    for (i, c) in arguments[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return (String::new(), None);
    };
    let args = arguments[open + 1..close].trim().to_string();
    let ret = arguments[close + 1..]
        .trim()
        .strip_prefix("RETURN")
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty());
    (args, ret)
}

/// `SHOW USER FUNCTIONS IN SCHEMA …` / `SHOW PROCEDURES IN SCHEMA …` (built-ins left out).
pub fn routines(rows: &Rows, procedures: bool) -> Vec<RoutineSummary> {
    rows.iter()
        .filter(|r| !r.flag("is_builtin"))
        .filter_map(|r| {
            let name = r.text("name")?;
            let text = r.text("arguments").unwrap_or_default();
            // Past the name: a quoted one may hold a `(` of its own.
            let (arguments, return_type) = signature(
                text.strip_prefix(name.as_str())
                    .filter(|rest| rest.starts_with('('))
                    .unwrap_or(&text),
            );
            let kind = if procedures {
                RoutineKind::Procedure
            } else if r.flag("is_aggregate") {
                RoutineKind::Aggregate
            } else {
                RoutineKind::Function
            };
            Some(RoutineSummary {
                schema: r.some("schema_name"),
                name,
                kind,
                identity_arguments: arguments.clone(),
                arguments,
                return_type,
                language: r.some("language"),
                comment: r
                    .some("description")
                    .filter(|d| d != "user-defined function" && d != "user-defined procedure"),
            })
        })
        .collect()
}

/// `SHOW SEQUENCES IN SCHEMA …`.
pub fn sequences(rows: &Rows) -> Vec<SequenceSummary> {
    rows.iter()
        .filter_map(|r| {
            Some(SequenceSummary {
                schema: r.some("schema_name"),
                name: r.text("name")?,
                data_type: Some("NUMBER".into()),
                owned_by: None,
                comment: r.some("comment"),
            })
        })
        .collect()
}

/// What `SERVER_INFO` returns: the version and where the session stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionState {
    pub version: String,
    pub account: Option<String>,
    pub region: Option<String>,
    pub user: Option<String>,
    pub role: Option<String>,
    pub warehouse: Option<String>,
    pub database: Option<String>,
    pub schema: Option<String>,
}

/// The one row of `SERVER_INFO`, by position.
pub fn session_state(rows: &Rows) -> Option<SessionState> {
    let row = rows.rows.first()?;
    let at = |i: usize| {
        row.get(i)
            .and_then(Value::as_text)
            .filter(|s| !s.is_empty())
    };
    Some(SessionState {
        version: at(0)?,
        account: at(1),
        region: at(2),
        user: at(3),
        role: at(4),
        warehouse: at(5),
        database: at(6),
        schema: at(7),
    })
}

/// The row count of the one table a `SHOW TABLES LIKE` lists (metadata, no warehouse).
pub fn row_count(rows: &Rows, table: &str) -> Option<i64> {
    rows.iter()
        .find(|r| r.text("name").as_deref() == Some(table))
        .and_then(|r| r.int("rows"))
}

/// Completion: `SHOW COLUMNS IN SCHEMA …` as table → columns.
pub fn schema_columns(rows: &Rows) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in rows.iter() {
        if let (Some(t), Some(c)) = (r.text("table_name"), r.text("column_name")) {
            out.entry(t).or_default().push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::schema::ValueKind;

    fn rows(columns: &[&str], data: &[&[Option<&str>]]) -> Rows {
        Rows {
            columns: columns.iter().map(|c| c.to_string()).collect(),
            rows: data
                .iter()
                .map(|r| {
                    r.iter()
                        .map(|c| match c {
                            Some(s) => Value::Text(s.to_string()),
                            None => Value::Null,
                        })
                        .collect()
                })
                .collect(),
        }
    }

    #[test]
    fn databases_schemas_tables_and_views() {
        let dbs = rows(
            &[
                "created_on",
                "name",
                "is_default",
                "is_current",
                "owner",
                "comment",
            ],
            &[
                &[
                    Some("x"),
                    Some("ANALYTICS"),
                    Some("N"),
                    Some("Y"),
                    Some("SYSADMIN"),
                    Some(""),
                ],
                &[
                    Some("x"),
                    Some("raw data"),
                    Some("N"),
                    Some("N"),
                    Some(""),
                    None,
                ],
            ],
        );
        let d = databases(&dbs);
        assert_eq!(d.len(), 2);
        assert!(d[0].is_current && !d[1].is_current);
        assert_eq!(d[0].owner.as_deref(), Some("SYSADMIN"));
        assert_eq!(d[1].owner, None);
        assert_eq!(dbs.last_name().as_deref(), Some("raw data"));

        let sch = rows(
            &["name", "is_default", "is_current"],
            &[
                &[Some("PUBLIC"), Some("N"), Some("Y")],
                &[Some("INFORMATION_SCHEMA"), Some("N"), Some("N")],
            ],
        );
        let s = schemas(&sch);
        assert!(s[0].is_default && !s[1].is_default);
        assert!(is_system_schema(&s[1].name) && !is_system_schema("PUBLIC"));

        let t = tables(&rows(
            &[
                "name",
                "schema_name",
                "kind",
                "comment",
                "rows",
                "bytes",
                "is_external",
            ],
            &[
                &[
                    Some("ORDERS"),
                    Some("PUBLIC"),
                    Some("TABLE"),
                    Some("o"),
                    Some("1500000"),
                    Some("4096"),
                    Some("N"),
                ],
                &[
                    Some("ext"),
                    Some("PUBLIC"),
                    Some("TABLE"),
                    Some(""),
                    None,
                    None,
                    Some("Y"),
                ],
            ],
        ));
        assert_eq!(t[0].row_estimate, Some(1_500_000));
        assert_eq!(t[0].size_bytes, Some(4096));
        assert_eq!(t[0].kind, TableKind::Table);
        assert_eq!(t[1].kind, TableKind::ForeignTable);
        assert_eq!(t[1].comment, None);

        let v = rows(
            &["name", "schema_name", "text", "is_materialized", "comment"],
            &[
                &[
                    Some("V"),
                    Some("PUBLIC"),
                    Some("create view V as select 1"),
                    Some("false"),
                    None,
                ],
                &[Some("MV"), Some("PUBLIC"), Some(""), Some("true"), None],
            ],
        );
        let vs = views(&v);
        assert_eq!(vs[0].kind, TableKind::View);
        assert_eq!(vs[1].kind, TableKind::MaterializedView);
        assert_eq!(
            view_text(&v, "V").as_deref(),
            Some("create view V as select 1")
        );
        assert_eq!(view_text(&v, "MV"), None);
    }

    #[test]
    fn columns_from_their_data_type_json() {
        let c = rows(
            &[
                "table_name",
                "schema_name",
                "column_name",
                "data_type",
                "null?",
                "default",
                "kind",
                "expression",
                "comment",
                "database_name",
                "autoincrement",
            ],
            &[
                &[
                    Some("T"),
                    Some("S"),
                    Some("ID"),
                    Some(r#"{"type":"FIXED","precision":38,"scale":0,"nullable":false}"#),
                    Some("false"),
                    Some(""),
                    Some("COLUMN"),
                    Some(""),
                    None,
                    Some("D"),
                    Some("IDENTITY START 1 INCREMENT 1 ORDER"),
                ],
                &[
                    Some("T"),
                    Some("S"),
                    Some("Name"),
                    Some(
                        r#"{"type":"TEXT","length":80,"byteLength":320,"nullable":true,"fixed":false}"#,
                    ),
                    Some("true"),
                    Some("'x'"),
                    Some("COLUMN"),
                    None,
                    Some("who"),
                    Some("D"),
                    Some(""),
                ],
                &[
                    Some("T"),
                    Some("S"),
                    Some("PRICE"),
                    Some(r#"{"type":"FIXED","precision":10,"scale":2,"nullable":true}"#),
                    Some("true"),
                    None,
                    Some("COLUMN"),
                    None,
                    None,
                    Some("D"),
                    None,
                ],
                &[
                    Some("T"),
                    Some("S"),
                    Some("AT"),
                    Some(r#"{"type":"TIMESTAMP_TZ","precision":0,"scale":3,"nullable":true}"#),
                    Some("true"),
                    None,
                    Some("COLUMN"),
                    None,
                    None,
                    Some("D"),
                    None,
                ],
                &[
                    Some("T"),
                    Some("S"),
                    Some("DOC"),
                    Some(r#"{"type":"VARIANT","nullable":true}"#),
                    Some("true"),
                    None,
                    Some("COLUMN"),
                    None,
                    None,
                    Some("D"),
                    None,
                ],
                &[
                    Some("T"),
                    Some("S"),
                    Some("E"),
                    Some(
                        r#"{"type":"VECTOR","vectorElementType":"FLOAT","vectorDimension":3,"nullable":true}"#,
                    ),
                    Some("true"),
                    None,
                    Some("COLUMN"),
                    None,
                    None,
                    Some("D"),
                    None,
                ],
                &[
                    Some("T"),
                    Some("S"),
                    Some("V2"),
                    Some(r#"{"type":"FIXED","precision":38,"scale":0}"#),
                    Some("true"),
                    None,
                    Some("VIRTUAL_COLUMN"),
                    Some("ID * 2"),
                    None,
                    Some("D"),
                    None,
                ],
                &[
                    Some("T"),
                    Some("S"),
                    Some("ODD"),
                    Some("not json"),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                ],
            ],
        );
        let cols = columns(&c, &["ID".to_string()]);
        assert_eq!(cols.len(), 8);
        let id = &cols[0];
        assert_eq!(
            (id.data_type.as_str(), id.kind),
            ("NUMBER(38,0)", ValueKind::BigInt)
        );
        assert!(id.is_pk && id.pk_ordinal == Some(1) && !id.nullable && id.is_auto_increment);
        assert_eq!(id.numeric_precision, Some(38));
        let name = &cols[1];
        assert_eq!(name.data_type, "VARCHAR(80)");
        assert_eq!(name.char_max_len, Some(80));
        assert_eq!(name.default_expr.as_deref(), Some("'x'"));
        assert_eq!(name.comment.as_deref(), Some("who"));
        assert!(name.nullable && !name.is_pk && !name.is_auto_increment);
        assert_eq!(cols[2].kind, ValueKind::Decimal);
        assert_eq!(cols[2].numeric_scale, Some(2));
        assert_eq!(cols[3].data_type, "TIMESTAMP_TZ(3)");
        assert_eq!(cols[3].kind, ValueKind::TimestampTz);
        assert_eq!(cols[4].kind, ValueKind::Json);
        assert_eq!(cols[5].data_type, "VECTOR(FLOAT, 3)");
        assert!(cols[6].is_generated && cols[6].extra.as_deref() == Some("ID * 2"));
        assert_eq!(cols[7].data_type, "NOT JSON");
        assert_eq!(cols[7].kind, ValueKind::Other);
        assert_eq!(cols[7].ordinal, 8);
    }

    #[test]
    fn keys_indexes_and_foreign_keys() {
        let pk = rows(
            &[
                "table_name",
                "column_name",
                "key_sequence",
                "constraint_name",
            ],
            &[
                &[Some("T"), Some("B"), Some("2"), Some("PK_T")],
                &[Some("T"), Some("A"), Some("1"), Some("PK_T")],
            ],
        );
        assert_eq!(
            primary_key(&pk),
            (Some("PK_T".into()), vec!["A".to_string(), "B".to_string()])
        );
        assert_eq!(primary_key(&Rows::default()), (None, vec![]));
        let uq = rows(
            &["column_name", "key_sequence", "constraint_name"],
            &[&[Some("EMAIL"), Some("1"), Some("UQ_EMAIL")]],
        );
        let idx = key_indexes(&pk, &uq);
        assert_eq!(idx.len(), 2);
        assert!(idx[0].primary && idx[0].columns == ["A", "B"]);
        assert!(!idx[1].primary && idx[1].unique && idx[1].columns == ["EMAIL"]);

        let hy = rows(
            &["name", "is_unique", "columns", "included_columns"],
            &[
                &[Some("IDX_A"), Some("Y"), Some("[A, B]"), Some("[]")],
                &[Some("IDX_C"), Some("N"), Some(r#"["C"]"#), Some(r#"["D"]"#)],
            ],
        );
        let h = hybrid_indexes(&hy);
        assert_eq!(h[0].columns, ["A", "B"]);
        assert!(h[0].unique && h[0].include.is_empty());
        assert_eq!(h[1].include, ["D"]);

        let fk = rows(
            &[
                "pk_database_name",
                "pk_schema_name",
                "pk_table_name",
                "pk_column_name",
                "fk_table_name",
                "fk_column_name",
                "key_sequence",
                "update_rule",
                "delete_rule",
                "fk_name",
            ],
            &[
                &[
                    Some("D"),
                    Some("S"),
                    Some("CUSTOMERS"),
                    Some("REGION"),
                    Some("ORDERS"),
                    Some("CUST_REGION"),
                    Some("2"),
                    Some("NO ACTION"),
                    Some("CASCADE"),
                    Some("FK_CUST"),
                ],
                &[
                    Some("D"),
                    Some("S"),
                    Some("CUSTOMERS"),
                    Some("ID"),
                    Some("ORDERS"),
                    Some("CUST_ID"),
                    Some("1"),
                    Some("NO ACTION"),
                    Some("CASCADE"),
                    Some("FK_CUST"),
                ],
            ],
        );
        let f = foreign_keys(&fk);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].columns, ["CUST_ID", "CUST_REGION"]);
        assert_eq!(f[0].ref_columns, ["ID", "REGION"]);
        assert_eq!(f[0].ref_table, TableRef::new(Some("S"), "CUSTOMERS"));
        assert_eq!(f[0].on_delete, "CASCADE");
    }

    #[test]
    fn routines_sequences_and_the_session() {
        assert_eq!(
            signature("ADD_ONE(NUMBER) RETURN NUMBER(38,0)"),
            ("NUMBER".into(), Some("NUMBER(38,0)".into()))
        );
        assert_eq!(
            signature("T(VARCHAR, ARRAY) RETURN TABLE (A NUMBER)"),
            ("VARCHAR, ARRAY".into(), Some("TABLE (A NUMBER)".into()))
        );
        assert_eq!(
            signature("P() RETURN VARCHAR"),
            (String::new(), Some("VARCHAR".into()))
        );
        assert_eq!(signature("garbage"), (String::new(), None));
        assert_eq!(signature("F(NUMBER"), (String::new(), None));
        let f = rows(
            &[
                "name",
                "schema_name",
                "is_builtin",
                "is_aggregate",
                "arguments",
                "description",
                "language",
            ],
            &[
                &[
                    Some("ADD_ONE"),
                    Some("S"),
                    Some("N"),
                    Some("N"),
                    Some("ADD_ONE(NUMBER) RETURN NUMBER"),
                    Some("user-defined function"),
                    Some("SQL"),
                ],
                &[
                    Some("MY_AGG"),
                    Some("S"),
                    Some("N"),
                    Some("Y"),
                    Some("MY_AGG(FLOAT) RETURN FLOAT"),
                    Some("sums"),
                    Some("PYTHON"),
                ],
                &[
                    Some("ABS"),
                    Some(""),
                    Some("Y"),
                    Some("N"),
                    Some("ABS(NUMBER) RETURN NUMBER"),
                    Some(""),
                    None,
                ],
                // A quoted name holding a parenthesis.
                &[
                    Some("f(x"),
                    Some("S"),
                    Some("N"),
                    Some("N"),
                    Some("f(x(VARCHAR(10), NUMBER) RETURN TABLE (A NUMBER)"),
                    Some("user-defined function"),
                    Some("SQL"),
                ],
            ],
        );
        let r = routines(&f, false);
        assert_eq!(r.len(), 3);
        assert_eq!(r[2].name, "f(x");
        assert_eq!(r[2].arguments, "VARCHAR(10), NUMBER");
        assert_eq!(r[2].return_type.as_deref(), Some("TABLE (A NUMBER)"));
        assert_eq!(r[0].kind, RoutineKind::Function);
        assert_eq!(r[0].comment, None);
        assert_eq!(r[1].kind, RoutineKind::Aggregate);
        assert_eq!(r[1].comment.as_deref(), Some("sums"));
        assert_eq!(r[1].identity_arguments, "FLOAT");
        assert_eq!(routines(&f, true)[0].kind, RoutineKind::Procedure);

        let s = sequences(&rows(
            &["name", "schema_name", "next_value", "comment"],
            &[&[Some("SEQ1"), Some("S"), Some("101"), Some("ids")]],
        ));
        assert_eq!(s[0].name, "SEQ1");
        assert_eq!(s[0].comment.as_deref(), Some("ids"));

        let info = rows(
            &["a", "b", "c", "d", "e", "f", "g", "h"],
            &[&[
                Some("9.1.0"),
                Some("XY12345"),
                Some("AWS_US_EAST_1"),
                Some("ME"),
                Some("SYSADMIN"),
                None,
                Some("DB"),
                Some(""),
            ]],
        );
        let st = session_state(&info).unwrap();
        assert_eq!(st.version, "9.1.0");
        assert_eq!(st.warehouse, None);
        assert_eq!(st.schema, None);
        assert!(session_state(&Rows::default()).is_none());

        let t = rows(
            &["name", "rows"],
            &[&[Some("T_1"), Some("42")], &[Some("TX1"), Some("7")]],
        );
        assert_eq!(row_count(&t, "T_1"), Some(42));
        assert_eq!(row_count(&t, "nope"), None);

        let sc = schema_columns(&rows(
            &["table_name", "column_name"],
            &[
                &[Some("A"), Some("X")],
                &[Some("A"), Some("Y")],
                &[Some("B"), Some("Z")],
            ],
        ));
        assert_eq!(sc["A"], ["X", "Y"]);
        assert_eq!(sc["B"], ["Z"]);
    }
}
