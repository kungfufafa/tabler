//! Database objects of the sidebar besides tables (M8): routines (functions / procedures),
//! PostgreSQL sequences and system schemas, the Item Overview, and the planning / execution of
//! the sidebar's object operations. Works on any `DbSession` through `query` / `execute` /
//! `transaction`; SQL for the operations comes from `sqlgen::objects`.

use std::time::Instant;

use base64::Engine;

use crate::drivers::{DbSession, TxStatement};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::objects::{
    self, AggregateInfo, CloneSource, SequenceInfo, SerialSource, mysql_routine_script,
    mysql_show_create, pg_routine_script,
};
use crate::types::ddl::{DdlPlan, DdlResult};
use crate::types::objects::{
    ObjectOp, ObjectTarget, OverviewItem, RoutineKind, RoutineRef, RoutineSummary, SequenceSummary,
};
use crate::types::schema::{SchemaInfo, TableKind, TableRef};
use crate::types::value::Value;

/// How long a MySQL object operation waits for a metadata lock (as `commands::ddl` does).
const MYSQL_LOCK_WAIT_SECS: u64 = 5;

// --- cells -----------------------------------------------------------------------------------

/// A cell as text: PG introspection casts to `text`, MySQL's text protocol may hand out bytes.
fn text(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::Null | Value::Default => None,
        Value::Text(s)
        | Value::BigInt(s)
        | Value::Decimal(s)
        | Value::Enum(s)
        | Value::Json(s)
        | Value::Uuid(s)
        | Value::Date(s)
        | Value::Time(s)
        | Value::Timestamp(s)
        | Value::TimestampTz(s)
        | Value::Interval(s) => Some(s.clone()),
        Value::Int(i) => Some(i.to_string()),
        Value::Float(f) => Some(f.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Other(o) => Some(o.text.clone()),
        Value::Bytes(b) => base64::engine::general_purpose::STANDARD
            .decode(&b.base64)
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
        Value::Array(_) => None,
    }
}

/// Non-empty text.
fn some_text(v: Option<&Value>) -> Option<String> {
    text(v).filter(|s| !s.is_empty())
}

fn int(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Int(i) => Some(*i),
        Value::Float(f) if f.is_finite() => Some(*f as i64),
        Value::Bool(b) => Some(i64::from(*b)),
        other => text(Some(other)).and_then(|s| {
            let s = s.trim();
            s.parse::<i64>()
                .ok()
                .or_else(|| s.parse::<f64>().ok().map(|f| f as i64))
        }),
    }
}

fn boolean(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::Int(i)) => *i != 0,
        other => matches!(text(other).as_deref(), Some("t" | "true" | "1" | "YES")),
    }
}

fn not_found(what: String) -> AppError {
    AppError::NotFound(what)
}

fn is_undefined_column(e: &AppError) -> bool {
    matches!(e, AppError::Db { sqlstate: Some(s), .. } if s == "42703")
}

// --- routines --------------------------------------------------------------------------------

/// `pg_proc.prokind` (PG 11+), or what older servers spell with `proisagg` / `proiswindow`.
fn pg_kind_expr(legacy: bool) -> &'static str {
    if legacy {
        "(CASE WHEN p.proisagg THEN 'a' WHEN p.proiswindow THEN 'w' ELSE 'f' END)"
    } else {
        "p.prokind::text"
    }
}

/// Runs a catalog query that reads the routine kind, falling back to the pre-11 columns.
async fn pg_query_with_kind(
    session: &dyn DbSession,
    sql: impl Fn(&str) -> String,
    params: &[Value],
) -> Result<Vec<Vec<Value>>, AppError> {
    match session.query(&sql(pg_kind_expr(false)), params).await {
        Ok(rs) => Ok(rs.rows),
        Err(e) if is_undefined_column(&e) => {
            Ok(session.query(&sql(pg_kind_expr(true)), params).await?.rows)
        }
        Err(e) => Err(e),
    }
}

/// Excludes routines and relations an extension installed (`pg_depend.deptype = 'e'`).
const PG_NOT_EXTENSION_PROC: &str = "NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep \
     WHERE dep.classid = 'pg_catalog.pg_proc'::regclass AND dep.objid = p.oid AND dep.deptype = 'e')";
const PG_NOT_EXTENSION_REL: &str = "NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep \
     WHERE dep.classid = 'pg_catalog.pg_class'::regclass AND dep.objid = c.oid AND dep.deptype = 'e')";

fn pg_routines_sql(kind: &str) -> String {
    format!(
        "SELECT p.proname::text, {kind}, pg_catalog.pg_get_function_arguments(p.oid)::text, \
                pg_catalog.pg_get_function_identity_arguments(p.oid)::text, \
                CASE WHEN {kind} <> 'p' THEN pg_catalog.pg_get_function_result(p.oid)::text END, \
                l.lanname::text, pg_catalog.obj_description(p.oid, 'pg_proc')::text \
         FROM pg_catalog.pg_proc p \
         JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
         LEFT JOIN pg_catalog.pg_language l ON l.oid = p.prolang \
         WHERE n.nspname = $1 AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
           AND {PG_NOT_EXTENSION_PROC} \
         ORDER BY p.proname, 4"
    )
}

const MYSQL_ROUTINES_SQL: &str = "SELECT r.ROUTINE_NAME, r.ROUTINE_TYPE, \
       (SELECT GROUP_CONCAT(CONCAT_WS(' ', CASE WHEN r.ROUTINE_TYPE = 'PROCEDURE' THEN p.PARAMETER_MODE END, \
                p.PARAMETER_NAME, p.DTD_IDENTIFIER) \
               ORDER BY p.ORDINAL_POSITION SEPARATOR ', ') \
          FROM information_schema.PARAMETERS p \
         WHERE p.SPECIFIC_SCHEMA = r.ROUTINE_SCHEMA AND p.SPECIFIC_NAME = r.SPECIFIC_NAME \
           AND p.ROUTINE_TYPE = r.ROUTINE_TYPE AND p.ORDINAL_POSITION > 0), \
       r.DTD_IDENTIFIER, r.ROUTINE_BODY, r.ROUTINE_COMMENT \
  FROM information_schema.ROUTINES r WHERE r.ROUTINE_SCHEMA = ? \
 ORDER BY r.ROUTINE_NAME";

fn pg_routine_kind(kind: Option<&str>) -> RoutineKind {
    match kind {
        Some("p") => RoutineKind::Procedure,
        Some("a") => RoutineKind::Aggregate,
        Some("w") => RoutineKind::Window,
        _ => RoutineKind::Function,
    }
}

/// A row of `pg_routines_sql`: name, kind, arguments, identity arguments, result, language,
/// comment.
pub fn routine_from_pg_row(schema: &str, row: &[Value]) -> Option<RoutineSummary> {
    let kind = pg_routine_kind(text(row.get(1)).as_deref());
    Some(RoutineSummary {
        schema: Some(schema.to_string()),
        name: some_text(row.first())?,
        kind,
        arguments: text(row.get(2)).unwrap_or_default(),
        identity_arguments: text(row.get(3)).unwrap_or_default(),
        return_type: if kind == RoutineKind::Procedure {
            None
        } else {
            some_text(row.get(4))
        },
        language: some_text(row.get(5)),
        comment: some_text(row.get(6)),
    })
}

/// A row of `MYSQL_ROUTINES_SQL`: name, type, arguments, return type, body language, comment.
pub fn routine_from_mysql_row(schema: &str, row: &[Value]) -> Option<RoutineSummary> {
    let kind = match text(row.get(1)).as_deref() {
        Some(t) if t.eq_ignore_ascii_case("PROCEDURE") => RoutineKind::Procedure,
        _ => RoutineKind::Function,
    };
    Some(RoutineSummary {
        schema: Some(schema.to_string()),
        name: some_text(row.first())?,
        kind,
        arguments: text(row.get(2)).unwrap_or_default(),
        identity_arguments: String::new(),
        return_type: if kind == RoutineKind::Function {
            some_text(row.get(3))
        } else {
            None
        },
        language: some_text(row.get(4)),
        comment: some_text(row.get(5)),
    })
}

/// Functions and procedures of `schema` (PG: aggregates and window functions too), extension
/// members left out. SQLite has none.
pub async fn list_routines(
    session: &dyn DbSession,
    schema: &str,
) -> Result<Vec<RoutineSummary>, AppError> {
    let params = [Value::Text(schema.to_string())];
    let rows = match session.dialect() {
        Dialect::Postgres => pg_query_with_kind(session, pg_routines_sql, &params).await?,
        Dialect::Mysql => session.query(MYSQL_ROUTINES_SQL, &params).await?.rows,
        Dialect::Sqlite => return Ok(Vec::new()),
    };
    let from_row = match session.dialect() {
        Dialect::Postgres => routine_from_pg_row,
        _ => routine_from_mysql_row,
    };
    Ok(rows.iter().filter_map(|r| from_row(schema, r)).collect())
}

/// PG: the routine's oid-free identity (schema, name, identity arguments) must match an existing
/// overload, so the argument list sent back by the renderer is the server's own text.
async fn pg_find_routine(
    session: &dyn DbSession,
    r: &RoutineRef,
    select: &str,
) -> Result<Vec<Value>, AppError> {
    let schema = r.schema.clone().unwrap_or_else(|| "public".into());
    let params = [
        Value::Text(schema),
        Value::Text(r.name.clone()),
        Value::Text(r.identity_arguments.clone()),
    ];
    let rows = pg_query_with_kind(
        session,
        |kind| {
            format!(
                "SELECT {} FROM pg_catalog.pg_proc p \
                 JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
                 LEFT JOIN pg_catalog.pg_aggregate a ON a.aggfnoid = p.oid \
                 WHERE n.nspname = $1 AND p.proname = $2 \
                   AND pg_catalog.pg_get_function_identity_arguments(p.oid) = $3",
                select.replace("{kind}", kind)
            )
        },
        &params,
    )
    .await?;
    rows.into_iter()
        .next()
        .ok_or_else(|| not_found(format!("routine {}({})", r.name, r.identity_arguments)))
}

/// What opening a routine shows: PG `pg_get_functiondef` (aggregates rebuilt from
/// `pg_aggregate`); MySQL `DROP … IF EXISTS` + `SHOW CREATE`.
pub async fn routine_definition(
    session: &dyn DbSession,
    r: &RoutineRef,
) -> Result<String, AppError> {
    match session.dialect() {
        Dialect::Postgres => {
            let row = pg_find_routine(
                session,
                r,
                "{kind}, CASE WHEN {kind} <> 'a' THEN pg_catalog.pg_get_functiondef(p.oid)::text END, \
                 pg_catalog.pg_get_function_arguments(p.oid)::text, a.aggtransfn::regproc::text, \
                 pg_catalog.format_type(a.aggtranstype, NULL)::text, \
                 CASE WHEN a.aggfinalfn <> 0 THEN a.aggfinalfn::regproc::text END, \
                 CASE WHEN a.aggcombinefn <> 0 THEN a.aggcombinefn::regproc::text END, \
                 a.agginitval::text, \
                 CASE WHEN a.aggsortop <> 0 THEN a.aggsortop::regoper::text END",
            )
            .await?;
            if text(row.first()).as_deref() == Some("a") {
                return Ok(objects::aggregate_ddl(&AggregateInfo {
                    schema: r.schema.clone(),
                    name: r.name.clone(),
                    arguments: text(row.get(2)).unwrap_or_default(),
                    sfunc: some_text(row.get(3)).unwrap_or_default(),
                    stype: some_text(row.get(4)).unwrap_or_default(),
                    finalfunc: some_text(row.get(5)),
                    combinefunc: some_text(row.get(6)),
                    initcond: text(row.get(7)),
                    sortop: some_text(row.get(8)),
                }));
            }
            let def = some_text(row.get(1))
                .ok_or_else(|| not_found(format!("the definition of {}", r.name)))?;
            Ok(pg_routine_script(&def))
        }
        Dialect::Mysql => {
            let sql = mysql_show_create(r.kind, r.schema.as_deref(), &r.name);
            let rs = session.query(&sql, &[]).await?;
            let create = rs
                .rows
                .first()
                .and_then(|row| some_text(row.get(2)))
                .ok_or_else(|| {
                    AppError::Unsupported(format!(
                        "the server did not show the definition of {} (missing privileges?)",
                        r.name
                    ))
                })?;
            Ok(mysql_routine_script(r.kind, &r.name, &create))
        }
        Dialect::Sqlite => Err(AppError::Unsupported(
            "SQLite has no stored routines".into(),
        )),
    }
}

// --- sequences / schemas ---------------------------------------------------------------------

fn pg_sequences_sql() -> String {
    format!(
        "SELECT c.relname::text, pg_catalog.format_type(s.seqtypid, NULL)::text, \
                (SELECT t.relname::text || '.' || a.attname::text FROM pg_catalog.pg_depend d \
                   JOIN pg_catalog.pg_class t ON t.oid = d.refobjid \
                   JOIN pg_catalog.pg_attribute a ON a.attrelid = d.refobjid AND a.attnum = d.refobjsubid \
                  WHERE d.classid = 'pg_catalog.pg_class'::regclass AND d.objid = c.oid \
                    AND d.refclassid = 'pg_catalog.pg_class'::regclass AND d.deptype IN ('a', 'i') \
                  LIMIT 1), \
                pg_catalog.obj_description(c.oid, 'pg_class')::text \
         FROM pg_catalog.pg_class c \
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         JOIN pg_catalog.pg_sequence s ON s.seqrelid = c.oid \
         WHERE c.relkind = 'S' AND n.nspname = $1 AND {PG_NOT_EXTENSION_REL} \
         ORDER BY 1"
    )
}

/// A row of `pg_sequences_sql`: name, type, owner column, comment.
pub fn sequence_from_row(schema: &str, row: &[Value]) -> Option<SequenceSummary> {
    Some(SequenceSummary {
        schema: Some(schema.to_string()),
        name: some_text(row.first())?,
        data_type: some_text(row.get(1)),
        owned_by: some_text(row.get(2)),
        comment: some_text(row.get(3)),
    })
}

/// Sequences of a PostgreSQL schema (empty elsewhere).
pub async fn list_sequences(
    session: &dyn DbSession,
    schema: &str,
) -> Result<Vec<SequenceSummary>, AppError> {
    if session.dialect() != Dialect::Postgres {
        return Ok(Vec::new());
    }
    let rs = session
        .query(&pg_sequences_sql(), &[Value::Text(schema.to_string())])
        .await?;
    Ok(rs
        .rows
        .iter()
        .filter_map(|r| sequence_from_row(schema, r))
        .collect())
}

/// `CREATE SEQUENCE` text of a PostgreSQL sequence.
pub async fn sequence_definition(
    session: &dyn DbSession,
    sequence: &TableRef,
) -> Result<String, AppError> {
    if session.dialect() != Dialect::Postgres {
        return Err(AppError::Unsupported(
            "sequences are listed for PostgreSQL only".into(),
        ));
    }
    let schema = sequence.schema.clone().unwrap_or_else(|| "public".into());
    let sql = "SELECT pg_catalog.format_type(s.seqtypid, NULL)::text, s.seqstart::text, \
                      s.seqincrement::text, s.seqmin::text, s.seqmax::text, s.seqcache::text, \
                      s.seqcycle, o.ts, o.tt, o.ta \
               FROM pg_catalog.pg_class c \
               JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
               JOIN pg_catalog.pg_sequence s ON s.seqrelid = c.oid \
               LEFT JOIN LATERAL ( \
                 SELECT tn.nspname::text AS ts, t.relname::text AS tt, a.attname::text AS ta \
                 FROM pg_catalog.pg_depend d \
                 JOIN pg_catalog.pg_class t ON t.oid = d.refobjid \
                 JOIN pg_catalog.pg_namespace tn ON tn.oid = t.relnamespace \
                 JOIN pg_catalog.pg_attribute a ON a.attrelid = d.refobjid AND a.attnum = d.refobjsubid \
                 WHERE d.classid = 'pg_catalog.pg_class'::regclass AND d.objid = c.oid \
                   AND d.refclassid = 'pg_catalog.pg_class'::regclass AND d.deptype = 'a' \
                 LIMIT 1) o ON true \
               WHERE c.relkind = 'S' AND n.nspname = $1 AND c.relname = $2";
    let rs = session
        .query(
            sql,
            &[
                Value::Text(schema.clone()),
                Value::Text(sequence.name.clone()),
            ],
        )
        .await?;
    let row = rs
        .rows
        .first()
        .ok_or_else(|| not_found(format!("sequence {}", sequence.name)))?;
    let owned_by = match (some_text(row.get(8)), some_text(row.get(9))) {
        (Some(table), Some(column)) => Some((some_text(row.get(7)), table, column)),
        _ => None,
    };
    Ok(objects::sequence_ddl(&SequenceInfo {
        schema: Some(schema),
        name: sequence.name.clone(),
        data_type: some_text(row.first()),
        start: text(row.get(1)),
        increment: text(row.get(2)),
        min: text(row.get(3)),
        max: text(row.get(4)),
        cache: text(row.get(5)),
        cycle: boolean(row.get(6)),
        owned_by,
    }))
}

/// PostgreSQL's system schemas (`information_schema`, `pg_catalog`, `pg_toast`), which
/// `list_schemas` leaves out; the sidebar adds them when "Show System Schemas" is on.
pub async fn system_schemas(session: &dyn DbSession) -> Result<Vec<SchemaInfo>, AppError> {
    if session.dialect() != Dialect::Postgres {
        return Ok(Vec::new());
    }
    let rs = session
        .query(
            "SELECT n.nspname::text FROM pg_catalog.pg_namespace n \
             WHERE n.nspname IN ('information_schema', 'pg_catalog', 'pg_toast') ORDER BY 1",
            &[],
        )
        .await?;
    Ok(rs
        .rows
        .iter()
        .filter_map(|r| some_text(r.first()))
        .map(|name| SchemaInfo {
            name,
            is_default: false,
        })
        .collect())
}

// --- item overview ---------------------------------------------------------------------------

const PG_OVERVIEW_SQL: &str = "SELECT c.relname::text, c.relkind::text, \
        CASE WHEN c.reltuples < 0 OR c.relkind NOT IN ('r', 'p', 'm', 'f') THEN NULL \
             ELSE c.reltuples::bigint END, \
        CASE WHEN c.relkind IN ('r', 'p', 'm') THEN pg_catalog.pg_total_relation_size(c.oid) END, \
        CASE WHEN c.relkind IN ('r', 'p', 'm') THEN pg_catalog.pg_table_size(c.oid) END, \
        CASE WHEN c.relkind IN ('r', 'p', 'm') THEN pg_catalog.pg_indexes_size(c.oid) END, \
        pg_catalog.pg_get_userbyid(c.relowner)::text, \
        pg_catalog.obj_description(c.oid, 'pg_class')::text \
   FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
  WHERE n.nspname = $1 AND c.relkind IN ('r', 'p', 'v', 'm', 'f') \
  ORDER BY 1";

const MYSQL_OVERVIEW_SQL: &str = "SELECT TABLE_NAME, TABLE_TYPE, TABLE_ROWS, DATA_LENGTH, \
        INDEX_LENGTH, ENGINE, TABLE_COLLATION, TABLE_COMMENT \
   FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME";

fn pg_relkind(kind: Option<&str>) -> TableKind {
    match kind {
        Some("v") => TableKind::View,
        Some("m") => TableKind::MaterializedView,
        Some("f") => TableKind::ForeignTable,
        Some("p") => TableKind::Partitioned,
        _ => TableKind::Table,
    }
}

/// A row of `PG_OVERVIEW_SQL`.
pub fn overview_from_pg_row(schema: &str, row: &[Value]) -> Option<OverviewItem> {
    Some(OverviewItem {
        name: some_text(row.first())?,
        schema: Some(schema.to_string()),
        kind: pg_relkind(text(row.get(1)).as_deref()),
        estimated_rows: int(row.get(2)),
        total_size: int(row.get(3)),
        data_size: int(row.get(4)),
        index_size: int(row.get(5)),
        engine: None,
        charset: None,
        collation: None,
        owner: some_text(row.get(6)),
        comment: some_text(row.get(7)),
    })
}

/// A row of `MYSQL_OVERVIEW_SQL`. The character set is the collation's prefix
/// (`utf8mb4_0900_ai_ci` → `utf8mb4`); views carry the comment `VIEW`, which is dropped.
pub fn overview_from_mysql_row(schema: &str, row: &[Value]) -> Option<OverviewItem> {
    let kind = match text(row.get(1)).as_deref() {
        Some("VIEW") => TableKind::View,
        Some("SYSTEM VIEW") => TableKind::SystemView,
        _ => TableKind::Table,
    };
    let view = kind != TableKind::Table;
    let data = int(row.get(3));
    let index = int(row.get(4));
    let collation = some_text(row.get(6));
    let charset = collation
        .as_deref()
        .map(|c| c.split('_').next().unwrap_or(c).to_string());
    Some(OverviewItem {
        name: some_text(row.first())?,
        schema: Some(schema.to_string()),
        kind,
        estimated_rows: if view { None } else { int(row.get(2)) },
        total_size: match (data, index) {
            (Some(d), Some(i)) => Some(d + i),
            (d, i) => d.or(i),
        },
        data_size: data,
        index_size: index,
        engine: some_text(row.get(5)),
        charset,
        collation,
        owner: None,
        comment: some_text(row.get(7)).filter(|c| !(view && c == "VIEW")),
    })
}

/// SQLite sizes from `dbstat` (when the library has it) and row estimates from `sqlite_stat1`
/// (after ANALYZE), added to the table list.
async fn sqlite_overview(
    session: &dyn DbSession,
    schema: &str,
) -> Result<Vec<OverviewItem>, AppError> {
    let d = Dialect::Sqlite;
    let tables = session.list_tables(schema).await?;
    let sizes = session
        .query(
            &format!(
                "SELECT m.tbl_name, m.type, SUM(s.pgsize) FROM {}.sqlite_master AS m \
                 JOIN dbstat(?1) AS s ON s.name = m.name \
                 WHERE m.type IN ('table', 'index') GROUP BY m.tbl_name, m.type",
                d.quote_ident(schema)
            ),
            &[Value::Text(schema.to_string())],
        )
        .await
        .map(|rs| rs.rows)
        .unwrap_or_default();
    let stats = session
        .query(
            &format!(
                "SELECT tbl, MAX(CAST(stat AS INTEGER)) FROM {}.sqlite_stat1 GROUP BY tbl",
                d.quote_ident(schema)
            ),
            &[],
        )
        .await
        .map(|rs| rs.rows)
        .unwrap_or_default();
    let size_of = |name: &str, ty: &str| {
        sizes.iter().find_map(|r| {
            (text(r.first()).as_deref() == Some(name) && text(r.get(1)).as_deref() == Some(ty))
                .then(|| int(r.get(2)))
                .flatten()
        })
    };
    let has_dbstat = !sizes.is_empty();
    Ok(tables
        .into_iter()
        .map(|t| {
            let table = t.kind == TableKind::Table;
            let data = (has_dbstat && table).then(|| size_of(&t.name, "table").unwrap_or(0));
            let index = (has_dbstat && table).then(|| size_of(&t.name, "index").unwrap_or(0));
            OverviewItem {
                estimated_rows: stats.iter().find_map(|r| {
                    (text(r.first()).as_deref() == Some(t.name.as_str()))
                        .then(|| int(r.get(1)))
                        .flatten()
                }),
                total_size: data.zip(index).map(|(d, i)| d + i),
                data_size: data,
                index_size: index,
                name: t.name,
                schema: t.schema,
                kind: t.kind,
                engine: None,
                charset: None,
                collation: None,
                owner: None,
                comment: t.comment,
            }
        })
        .collect())
}

/// Every table / view of `schema` with its size and statistics (Item Overview).
pub async fn item_overview(
    session: &dyn DbSession,
    schema: &str,
) -> Result<Vec<OverviewItem>, AppError> {
    let params = [Value::Text(schema.to_string())];
    match session.dialect() {
        Dialect::Postgres => Ok(session
            .query(PG_OVERVIEW_SQL, &params)
            .await?
            .rows
            .iter()
            .filter_map(|r| overview_from_pg_row(schema, r))
            .collect()),
        Dialect::Mysql => Ok(session
            .query(MYSQL_OVERVIEW_SQL, &params)
            .await?
            .rows
            .iter()
            .filter_map(|r| overview_from_mysql_row(schema, r))
            .collect()),
        Dialect::Sqlite => sqlite_overview(session, schema).await,
    }
}

// --- operations ------------------------------------------------------------------------------

/// PG: (partition, ancestor) pairs for the partitions among `tables`, every level up. Empty when
/// the catalog cannot tell (servers before 10).
async fn pg_partition_ancestors(
    session: &dyn DbSession,
    tables: &[TableRef],
) -> Vec<(TableRef, TableRef)> {
    let names = Value::Array(tables.iter().map(|t| Value::Text(t.name.clone())).collect());
    let sql = "WITH RECURSIVE up(child, parent) AS ( \
                 SELECT i.inhrelid, i.inhparent FROM pg_catalog.pg_inherits i \
                 JOIN pg_catalog.pg_class c ON c.oid = i.inhrelid \
                 WHERE c.relispartition AND c.relname::text = ANY($1::text[]) \
                 UNION \
                 SELECT up.child, i.inhparent FROM up \
                 JOIN pg_catalog.pg_class pc ON pc.oid = up.parent AND pc.relispartition \
                 JOIN pg_catalog.pg_inherits i ON i.inhrelid = up.parent) \
               SELECT cn.nspname::text, c.relname::text, pn.nspname::text, p.relname::text \
               FROM up \
               JOIN pg_catalog.pg_class c ON c.oid = up.child \
               JOIN pg_catalog.pg_namespace cn ON cn.oid = c.relnamespace \
               JOIN pg_catalog.pg_class p ON p.oid = up.parent \
               JOIN pg_catalog.pg_namespace pn ON pn.oid = p.relnamespace";
    let Ok(rs) = session.query(sql, &[names]).await else {
        return Vec::new();
    };
    rs.rows
        .iter()
        .filter_map(|r| {
            Some((
                TableRef::new(Some(&some_text(r.first())?), &some_text(r.get(1))?),
                TableRef::new(Some(&some_text(r.get(2))?), &some_text(r.get(3))?),
            ))
        })
        .collect()
}

/// SQLite: (child, parent) foreign keys between `tables` (parents matched without case, in the
/// child's schema, as SQLite resolves them).
async fn sqlite_references(
    session: &dyn DbSession,
    tables: &[TableRef],
) -> Vec<(TableRef, TableRef)> {
    let mut out = Vec::new();
    for t in tables {
        let schema = t.schema.clone().unwrap_or_else(|| "main".into());
        let Ok(rs) = session
            .query(
                "SELECT \"table\" FROM pragma_foreign_key_list(?1, ?2)",
                &[Value::Text(t.name.clone()), Value::Text(schema)],
            )
            .await
        else {
            continue;
        };
        for parent in rs.rows.iter().filter_map(|r| some_text(r.first())) {
            if let Some(p) = tables
                .iter()
                .find(|x| x.schema == t.schema && x.name.eq_ignore_ascii_case(&parent))
            {
                out.push((t.clone(), p.clone()));
            }
        }
    }
    out
}

/// SQLite: `targets` with referencing tables before the tables they reference.
async fn sqlite_children_first(
    session: &dyn DbSession,
    targets: &[ObjectTarget],
) -> Vec<ObjectTarget> {
    let tables: Vec<TableRef> = targets.iter().map(|t| t.table.clone()).collect();
    let references = sqlite_references(session, &tables).await;
    let mut rest = targets.to_vec();
    objects::children_first(&tables, &references)
        .into_iter()
        .filter_map(|t| {
            let i = rest.iter().position(|x| x.table == t)?;
            Some(rest.remove(i))
        })
        .collect()
}

/// PG serial columns of `source`: a default of `nextval` of a sequence the column owns
/// (`pg_depend` type `a`), with the sequence's settings. Empty when the catalog cannot tell
/// (servers before 10 have no `pg_sequence`).
async fn pg_serial_sources(session: &dyn DbSession, source: &TableRef) -> Vec<SerialSource> {
    let d = Dialect::Postgres;
    let sql = "SELECT a.attname::text, pg_catalog.format_type(q.seqtypid, NULL)::text, \
                      q.seqstart::text, q.seqincrement::text, q.seqmin::text, q.seqmax::text, \
                      q.seqcache::text, q.seqcycle \
               FROM pg_catalog.pg_depend d \
               JOIN pg_catalog.pg_class s ON s.oid = d.objid AND s.relkind = 'S' \
               JOIN pg_catalog.pg_sequence q ON q.seqrelid = s.oid \
               JOIN pg_catalog.pg_attribute a ON a.attrelid = d.refobjid AND a.attnum = d.refobjsubid \
               JOIN pg_catalog.pg_attrdef ad ON ad.adrelid = a.attrelid AND ad.adnum = a.attnum \
               WHERE d.classid = 'pg_catalog.pg_class'::regclass \
                 AND d.refclassid = 'pg_catalog.pg_class'::regclass AND d.deptype = 'a' \
                 AND d.refobjid = $1::text::regclass \
                 AND EXISTS (SELECT 1 FROM pg_catalog.pg_depend dd \
                             WHERE dd.classid = 'pg_catalog.pg_attrdef'::regclass AND dd.objid = ad.oid \
                               AND dd.refclassid = 'pg_catalog.pg_class'::regclass AND dd.refobjid = s.oid) \
               ORDER BY a.attnum";
    let name = d.qualify(source.schema.as_deref(), &source.name);
    let Ok(rs) = session.query(sql, &[Value::Text(name)]).await else {
        return Vec::new();
    };
    rs.rows
        .iter()
        .filter_map(|r| {
            Some(SerialSource {
                column: some_text(r.first())?,
                sequence: SequenceInfo {
                    data_type: some_text(r.get(1)),
                    start: text(r.get(2)),
                    increment: text(r.get(3)),
                    min: text(r.get(4)),
                    max: text(r.get(5)),
                    cache: text(r.get(6)),
                    cycle: boolean(r.get(7)),
                    ..SequenceInfo::default()
                },
            })
        })
        .collect()
}

/// Names a clone's new sequences / indexes must not take: PG relations of the schema, SQLite
/// `sqlite_master` entries of the schema.
async fn taken_names(session: &dyn DbSession, schema: Option<&str>) -> Vec<String> {
    let rows = match session.dialect() {
        Dialect::Postgres => session
            .query(
                "SELECT c.relname::text FROM pg_catalog.pg_class c \
                 JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = $1",
                &[Value::Text(schema.unwrap_or("public").to_string())],
            )
            .await
            .map(|rs| rs.rows),
        Dialect::Sqlite => session
            .query(
                &format!(
                    "SELECT name FROM {}.sqlite_master",
                    Dialect::Sqlite.quote_ident(schema.unwrap_or("main"))
                ),
                &[],
            )
            .await
            .map(|rs| rs.rows),
        Dialect::Mysql => Ok(Vec::new()),
    };
    rows.unwrap_or_default()
        .iter()
        .filter_map(|r| some_text(r.first()))
        .collect()
}

/// SQLite: `(name, CREATE INDEX text)` of the indexes of `table` that have one.
async fn sqlite_indexes(
    session: &dyn DbSession,
    table: &TableRef,
) -> Result<Vec<(String, String)>, AppError> {
    let sql = format!(
        "SELECT name, sql FROM {}.sqlite_master \
         WHERE type = 'index' AND tbl_name = ?1 COLLATE NOCASE AND sql IS NOT NULL ORDER BY name",
        Dialect::Sqlite.quote_ident(table.schema.as_deref().unwrap_or("main"))
    );
    let rs = session
        .query(&sql, &[Value::Text(table.name.clone())])
        .await?;
    Ok(rs
        .rows
        .iter()
        .filter_map(|r| Some((some_text(r.first())?, some_text(r.get(1))?)))
        .collect())
}

/// The statements an operation runs. PG routine drops are checked against the catalog first;
/// drops / truncates of several tables read the partitions (PG) or foreign keys (SQLite) among
/// them; a clone reads the source table's columns, serial sequences (PG), indexes and stored
/// `CREATE TABLE` (SQLite); a MySQL comment reads the session's `sql_mode`.
pub async fn plan_op(session: &dyn DbSession, op: &ObjectOp) -> Result<DdlPlan, AppError> {
    let d = session.dialect();
    match op {
        ObjectOp::DropTables {
            tables,
            cascade,
            ignore_foreign_keys,
        } => {
            let (targets, skipped) = match d {
                Dialect::Postgres if tables.len() > 1 => {
                    let refs: Vec<TableRef> = tables.iter().map(|t| t.table.clone()).collect();
                    let ancestors = pg_partition_ancestors(session, &refs).await;
                    objects::skip_partitions(tables, &ancestors)
                }
                Dialect::Sqlite if tables.len() > 1 => {
                    (sqlite_children_first(session, tables).await, 0)
                }
                _ => (tables.clone(), 0),
            };
            let mut plan = objects::drop_tables(d, &targets, *cascade, *ignore_foreign_keys)?;
            if skipped > 0 {
                plan.warnings
                    .push(objects::skipped_partitions_warning(skipped));
            }
            Ok(plan)
        }
        ObjectOp::TruncateTables {
            tables,
            cascade,
            restart_identity,
            ignore_foreign_keys,
        } => {
            let ordered = if d == Dialect::Sqlite && tables.len() > 1 {
                let references = sqlite_references(session, tables).await;
                objects::children_first(tables, &references)
            } else {
                tables.clone()
            };
            objects::truncate_tables(
                d,
                &ordered,
                *cascade,
                *restart_identity,
                *ignore_foreign_keys,
            )
        }
        ObjectOp::CloneTable {
            source,
            new_name,
            with_data,
        } => {
            objects::clone_name(source, new_name)?;
            let meta = session.table_meta(source).await?;
            let columns: Vec<String> = meta
                .columns
                .iter()
                .filter(|c| !c.is_generated)
                .map(|c| c.name.clone())
                .collect();
            let identity: Vec<String> = meta
                .columns
                .iter()
                .filter(|c| {
                    d == Dialect::Postgres
                        && matches!(c.extra.as_deref(), Some("ALWAYS" | "BY DEFAULT"))
                })
                .map(|c| c.name.clone())
                .collect();
            let serials = match d {
                Dialect::Postgres => pg_serial_sources(session, source).await,
                _ => Vec::new(),
            };
            let indexes = match d {
                Dialect::Sqlite => sqlite_indexes(session, source).await?,
                _ => Vec::new(),
            };
            let taken = if serials.is_empty() && indexes.is_empty() {
                Vec::new()
            } else {
                taken_names(session, source.schema.as_deref()).await
            };
            let src = CloneSource {
                columns: &columns,
                identity_columns: &identity,
                serial_columns: &serials,
                create_sql: meta.ddl.as_deref(),
                indexes: &indexes,
                taken_names: &taken,
                has_triggers: !meta.triggers.is_empty(),
            };
            objects::clone_table(d, source, new_name, *with_data, &src)
        }
        ObjectOp::DropRoutines { routines, cascade } => {
            if d == Dialect::Postgres {
                for r in routines {
                    pg_find_routine(session, r, "p.oid::text").await?;
                }
            }
            objects::drop_routines(d, routines, *cascade)
        }
        ObjectOp::DropSequences { sequences, cascade } => {
            objects::drop_sequences(d, sequences, *cascade)
        }
        ObjectOp::SetComment { target, comment } => {
            let mode = crate::admin::mysql_literal_mode(session).await?;
            objects::set_comment(d, target, comment.as_deref(), mode)
        }
    }
}

/// Runs a MySQL plan on ONE pooled connection, so `SET FOREIGN_KEY_CHECKS = 0` covers the
/// statements after it (the pool resets session variables when the connection goes back). DDL
/// commits implicitly: on failure the statements before the failing one stay done.
pub async fn run_mysql_plan(session: &dyn DbSession, plan: &DdlPlan) -> DdlResult {
    let started = Instant::now();
    let mut batch = vec![TxStatement::new(
        format!("SET SESSION lock_wait_timeout = {MYSQL_LOCK_WAIT_SECS}"),
        Vec::new(),
    )];
    batch.extend(
        plan.statements
            .iter()
            .map(|s| TxStatement::new(s.clone(), Vec::new())),
    );
    let outcome = session.transaction(batch).await;
    let duration_ms = started.elapsed().as_millis() as u64;
    match outcome {
        Ok(_) => DdlResult {
            executed: plan.statements.clone(),
            failed_index: None,
            error: None,
            duration_ms,
        },
        Err(err) => {
            let mut payload = err.to_payload();
            let failed = payload
                .statement_index
                .and_then(|i| i.checked_sub(1))
                .filter(|&i| i < plan.statements.len());
            payload.statement_index = failed;
            DdlResult {
                executed: plan.statements[..failed.unwrap_or(0)].to_vec(),
                failed_index: failed,
                error: Some(payload),
                duration_ms,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::value::BytesValue;

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn maps_pg_routine_rows() {
        let row = [
            t("add"),
            t("f"),
            t("a integer, b integer DEFAULT 1"),
            t("a integer, b integer"),
            t("integer"),
            t("plpgsql"),
            Value::Null,
        ];
        assert_eq!(
            routine_from_pg_row("public", &row),
            Some(RoutineSummary {
                schema: Some("public".into()),
                name: "add".into(),
                kind: RoutineKind::Function,
                arguments: "a integer, b integer DEFAULT 1".into(),
                identity_arguments: "a integer, b integer".into(),
                return_type: Some("integer".into()),
                language: Some("plpgsql".into()),
                comment: None,
            })
        );
        let proc = [
            t("archive"),
            t("p"),
            t(""),
            t(""),
            t("ignored"),
            t("sql"),
            t("nightly"),
        ];
        let p = routine_from_pg_row("app", &proc).unwrap();
        assert_eq!(p.kind, RoutineKind::Procedure);
        assert_eq!(p.return_type, None);
        assert_eq!(p.comment.as_deref(), Some("nightly"));
        assert_eq!(
            routine_from_pg_row("x", &[t("s"), t("a")]).unwrap().kind,
            RoutineKind::Aggregate
        );
        assert_eq!(
            routine_from_pg_row("x", &[t("r"), t("w")]).unwrap().kind,
            RoutineKind::Window
        );
        assert!(routine_from_pg_row("x", &[Value::Null, t("f")]).is_none());
    }

    #[test]
    fn maps_mysql_routine_rows() {
        let bytes = |s: &str| {
            Value::Bytes(BytesValue {
                base64: base64::engine::general_purpose::STANDARD.encode(s),
                len: s.len() as u64,
                truncated: false,
            })
        };
        let f = [
            t("calc_total"),
            t("FUNCTION"),
            bytes("order_id int, rate decimal(5,2)"),
            t("decimal(10,2)"),
            t("SQL"),
            t(""),
        ];
        let r = routine_from_mysql_row("shop", &f).unwrap();
        assert_eq!(r.kind, RoutineKind::Function);
        assert_eq!(r.arguments, "order_id int, rate decimal(5,2)");
        assert_eq!(r.return_type.as_deref(), Some("decimal(10,2)"));
        assert_eq!(r.identity_arguments, "");
        assert_eq!(r.comment, None);
        let p = [
            t("archive"),
            t("PROCEDURE"),
            Value::Null,
            t("ignored"),
            t("SQL"),
            t("cleanup"),
        ];
        let r = routine_from_mysql_row("shop", &p).unwrap();
        assert_eq!(r.kind, RoutineKind::Procedure);
        assert_eq!(r.arguments, "");
        assert_eq!(r.return_type, None);
        assert_eq!(r.comment.as_deref(), Some("cleanup"));
    }

    #[test]
    fn maps_sequence_and_overview_rows() {
        let s = sequence_from_row(
            "public",
            &[t("users_id_seq"), t("integer"), t("users.id"), Value::Null],
        )
        .unwrap();
        assert_eq!(s.owned_by.as_deref(), Some("users.id"));
        assert_eq!(s.data_type.as_deref(), Some("integer"));

        let pg = overview_from_pg_row(
            "public",
            &[
                t("users"),
                t("r"),
                Value::Int(50),
                Value::Int(65536),
                Value::Int(49152),
                Value::Int(16384),
                t("tabler"),
                t("Registered accounts"),
            ],
        )
        .unwrap();
        assert_eq!(pg.kind, TableKind::Table);
        assert_eq!(pg.estimated_rows, Some(50));
        assert_eq!(pg.total_size, Some(65536));
        assert_eq!(pg.owner.as_deref(), Some("tabler"));
        let view = overview_from_pg_row(
            "public",
            &[
                t("v"),
                t("v"),
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                t("tabler"),
                Value::Null,
            ],
        )
        .unwrap();
        assert_eq!(view.kind, TableKind::View);
        assert_eq!(view.total_size, None);

        let my = overview_from_mysql_row(
            "shop",
            &[
                t("orders"),
                t("BASE TABLE"),
                Value::BigInt("12".into()),
                Value::Int(16384),
                Value::Int(32768),
                t("InnoDB"),
                t("utf8mb4_0900_ai_ci"),
                t("All orders"),
            ],
        )
        .unwrap();
        assert_eq!(my.estimated_rows, Some(12));
        assert_eq!(my.total_size, Some(49152));
        assert_eq!(my.charset.as_deref(), Some("utf8mb4"));
        assert_eq!(my.collation.as_deref(), Some("utf8mb4_0900_ai_ci"));
        assert_eq!(my.engine.as_deref(), Some("InnoDB"));
        let my_view = overview_from_mysql_row(
            "shop",
            &[
                t("v"),
                t("VIEW"),
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                t("VIEW"),
            ],
        )
        .unwrap();
        assert_eq!(my_view.kind, TableKind::View);
        assert_eq!(my_view.comment, None);
        assert_eq!(my_view.charset, None);
    }

    #[test]
    fn reads_cells_leniently() {
        assert_eq!(int(Some(&t(" 42 "))), Some(42));
        assert_eq!(int(Some(&Value::Decimal("7.0".into()))), Some(7));
        assert_eq!(int(Some(&Value::Null)), None);
        assert!(boolean(Some(&t("t"))));
        assert!(boolean(Some(&Value::Int(1))));
        assert!(!boolean(Some(&t("f"))));
        assert_eq!(some_text(Some(&t(""))), None);
        assert_eq!(text(Some(&Value::Int(3))).as_deref(), Some("3"));
    }
}
