//! `pg_catalog` introspection. Every query casts its output to text/bool/int so no optional
//! tokio-postgres type features are needed; list-valued columns are joined with `LIST_SEP`.

use std::collections::BTreeMap;
use std::ops::Deref;
use std::time::Instant;

use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, Row};

use super::decode::{kind_from_pg_type, split_list};
use crate::drivers::{SqlTrace, trace_text};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::ddl::check_expression;
use crate::types::browse::{TraceCategory, TracedSql};
use crate::types::schema::{
    CheckMeta, ColumnMeta, CompletionMetadata, DatabaseInfo, ForeignKeyMeta, IndexMeta, SchemaInfo,
    TableKind, TableMeta, TableRef, TableSummary, TriggerMeta, ValueKind,
};
use crate::types::value::Value;

/// A worker client whose catalog queries are recorded in the session's trace as structure
/// queries (the console log shows the `pg_catalog` SELECTs the app really ran). Stands in for the
/// `Client` in this module (`query`, `query_one` and `query_opt` are the recording versions; every
/// other method is the client's).
pub struct Traced<'a> {
    client: &'a Client,
    trace: &'a SqlTrace,
}

impl<'a> Traced<'a> {
    pub fn new(client: &'a Client, trace: &'a SqlTrace) -> Traced<'a> {
        Traced { client, trace }
    }

    fn note(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
        started: Instant,
        rows: Option<usize>,
        error: Option<&tokio_postgres::Error>,
    ) {
        let shown: Vec<Value> = params.iter().map(|p| debug_param(*p)).collect();
        self.trace.record(TracedSql {
            sql: trace_text(Dialect::Postgres, sql, &shown),
            category: TraceCategory::Structure,
            duration_ms: started.elapsed().as_millis() as u64,
            rows: rows.map(|n| n as u64),
            affected: None,
            error: error.map(ToString::to_string),
        });
    }

    pub async fn query(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Row>, tokio_postgres::Error> {
        let started = Instant::now();
        let result = self.client.query(sql, params).await;
        self.note(
            sql,
            params,
            started,
            result.as_ref().ok().map(Vec::len),
            result.as_ref().err(),
        );
        result
    }

    pub async fn query_one(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Row, tokio_postgres::Error> {
        let started = Instant::now();
        let result = self.client.query_one(sql, params).await;
        self.note(
            sql,
            params,
            started,
            result.as_ref().ok().map(|_| 1),
            result.as_ref().err(),
        );
        result
    }

    pub async fn query_opt(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Option<Row>, tokio_postgres::Error> {
        let started = Instant::now();
        let result = self.client.query_opt(sql, params).await;
        self.note(
            sql,
            params,
            started,
            result.as_ref().ok().map(|r| usize::from(r.is_some())),
            result.as_ref().err(),
        );
        result
    }
}

impl Deref for Traced<'_> {
    type Target = Client;

    fn deref(&self) -> &Client {
        self.client
    }
}

/// A bound parameter as the value it shows in the trace (read from its `Debug` form: the bound
/// types of this module are strings, numbers and booleans).
fn debug_param(param: &(dyn ToSql + Sync)) -> Value {
    let text = format!("{param:?}");
    value_of_debug(&text)
}

fn value_of_debug(text: &str) -> Value {
    if text == "None" {
        return Value::Null;
    }
    if let Some(inner) = text.strip_prefix("Some(").and_then(|t| t.strip_suffix(')')) {
        return value_of_debug(inner);
    }
    if let Some(quoted) = text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
        let mut out = String::with_capacity(quoted.len());
        let mut chars = quoted.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        }
        return Value::Text(out);
    }
    match text {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => text
            .parse::<i64>()
            .map(Value::Int)
            .unwrap_or_else(|_| Value::Text(text.to_string())),
    }
}

const USER_SCHEMA_FILTER: &str = "n.nspname NOT IN ('pg_catalog','information_schema','pg_toast') \
     AND n.nspname NOT LIKE 'pg\\_temp\\_%' AND n.nspname NOT LIKE 'pg\\_toast\\_temp\\_%'";

fn kind_of_relkind(relkind: &str) -> TableKind {
    match relkind {
        "v" => TableKind::View,
        "m" => TableKind::MaterializedView,
        "f" => TableKind::ForeignTable,
        "p" => TableKind::Partitioned,
        _ => TableKind::Table,
    }
}

fn first_char(s: Option<&str>) -> Option<char> {
    s.and_then(|s| s.chars().next())
}

/// First entry of `current_schemas(false)`, i.e. where unqualified names resolve (usually
/// `public`).
pub async fn default_schema(client: &Traced<'_>) -> Result<Option<String>, AppError> {
    let row = client
        .query_one("SELECT (current_schemas(false))[1]::text", &[])
        .await?;
    Ok(row.try_get::<_, Option<String>>(0)?)
}

async fn schema_of(client: &Traced<'_>, table: &TableRef) -> Result<String, AppError> {
    match table.schema.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Ok(s.to_string()),
        None => Ok(default_schema(client)
            .await?
            .unwrap_or_else(|| "public".to_string())),
    }
}

pub async fn list_databases(client: &Traced<'_>) -> Result<Vec<DatabaseInfo>, AppError> {
    let rows = client
        .query(
            "SELECT d.datname::text, pg_get_userbyid(d.datdba)::text AS owner, \
                    pg_encoding_to_char(d.encoding)::text AS encoding, \
                    d.datname = current_database() AS is_current \
             FROM pg_database d WHERE NOT d.datistemplate AND d.datallowconn ORDER BY 1",
            &[],
        )
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(DatabaseInfo {
            name: row.try_get(0)?,
            owner: row.try_get(1)?,
            encoding: row.try_get(2)?,
            is_current: row.try_get::<_, Option<bool>>(3)?.unwrap_or(false),
        });
    }
    Ok(out)
}

pub async fn list_schemas(client: &Traced<'_>) -> Result<Vec<SchemaInfo>, AppError> {
    let sql = format!(
        "SELECT n.nspname::text, \
                COALESCE(n.nspname = (current_schemas(false))[1], false) AS is_default \
         FROM pg_namespace n WHERE {USER_SCHEMA_FILTER} ORDER BY 1"
    );
    let rows = client.query(&sql, &[]).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(SchemaInfo {
            name: row.try_get(0)?,
            is_default: row.try_get(1)?,
        });
    }
    Ok(out)
}

pub async fn list_tables(client: &Traced<'_>, schema: &str) -> Result<Vec<TableSummary>, AppError> {
    let rows = client
        .query(
            "SELECT c.relname::text, c.relkind::text, obj_description(c.oid, 'pg_class')::text, \
                    CASE WHEN c.reltuples < 0 THEN NULL ELSE c.reltuples::bigint END, \
                    CASE WHEN c.relkind IN ('r','p','m') THEN pg_total_relation_size(c.oid)::bigint END \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relkind IN ('r','p','v','m','f') ORDER BY c.relname",
            &[&schema],
        )
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let relkind: String = row.try_get(1)?;
        out.push(TableSummary {
            name: row.try_get(0)?,
            schema: Some(schema.to_string()),
            kind: kind_of_relkind(&relkind),
            row_estimate: row.try_get(3)?,
            size_bytes: row.try_get(4)?,
            comment: row.try_get(2)?,
        });
    }
    Ok(out)
}

pub async fn view_definition(
    client: &Traced<'_>,
    table: &TableRef,
) -> Result<Option<String>, AppError> {
    let schema = schema_of(client, table).await?;
    let row = client
        .query_opt(
            "SELECT pg_get_viewdef(c.oid, true)::text \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind IN ('v','m')",
            &[&schema, &table.name],
        )
        .await?;
    match row {
        Some(row) => Ok(row.try_get(0)?),
        None => Ok(None),
    }
}

/// (char_max_len, numeric_precision, numeric_scale) from a type name and its `atttypmod`.
fn typmod_info(type_name: &str, typmod: i32) -> (Option<i64>, Option<i32>, Option<i32>) {
    match type_name {
        "varchar" | "bpchar" if typmod >= 4 => (Some((typmod - 4) as i64), None, None),
        "bit" | "varbit" if typmod >= 0 => (Some(typmod as i64), None, None),
        "numeric" if typmod >= 4 => {
            let t = typmod - 4;
            let precision = (t >> 16) & 0xffff;
            // Scale is an 11-bit two's-complement field (negative scales since PG 15).
            let scale = ((t & 0x7ff) ^ 1024) - 1024;
            (None, Some(precision), Some(scale))
        }
        "int2" => (None, Some(16), Some(0)),
        "int4" | "oid" => (None, Some(32), Some(0)),
        "int8" => (None, Some(64), Some(0)),
        "float4" => (None, Some(24), None),
        "float8" => (None, Some(53), None),
        _ => (None, None, None),
    }
}

/// Timing (`BEFORE`/`AFTER`/`INSTEAD OF`) and events from a `pg_get_triggerdef` string.
pub fn parse_trigger_def(def: &str) -> (Option<String>, Vec<String>) {
    let upper = def.to_ascii_uppercase();
    let timings = ["INSTEAD OF", "BEFORE", "AFTER"];
    let mut best: Option<(usize, &str)> = None;
    for t in timings {
        if let Some(pos) = upper.find(&format!(" {t} "))
            && best.is_none_or(|(p, _)| pos < p)
        {
            best = Some((pos, t));
        }
    }
    let Some((pos, timing)) = best else {
        return (None, Vec::new());
    };
    let after = &upper[pos + timing.len() + 2..];
    let clause = match after.find(" ON ") {
        Some(end) => &after[..end],
        None => after,
    };
    let events = clause
        .split(" OR ")
        .filter_map(|part| part.split_whitespace().next())
        .filter(|w| matches!(*w, "INSERT" | "UPDATE" | "DELETE" | "TRUNCATE"))
        .map(str::to_string)
        .collect();
    (Some(timing.to_string()), events)
}

fn fk_action(code: &str) -> String {
    match code {
        "r" => "RESTRICT",
        "c" => "CASCADE",
        "n" => "SET NULL",
        "d" => "SET DEFAULT",
        _ => "NO ACTION",
    }
    .to_string()
}

pub async fn table_meta(client: &Traced<'_>, table: &TableRef) -> Result<TableMeta, AppError> {
    let schema = schema_of(client, table).await?;
    let name = table.name.as_str();
    let params: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[&schema, &name];

    // Relation
    let rel = client
        .query_opt(
            "SELECT c.relkind::text, obj_description(c.oid, 'pg_class')::text, \
                    CASE WHEN c.reltuples < 0 THEN NULL ELSE c.reltuples::bigint END \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind IN ('r','p','v','m','f')",
            params,
        )
        .await?
        .ok_or_else(|| AppError::NotFound(format!("table {schema}.{name}")))?;
    let relkind: String = rel.try_get(0)?;
    let kind = kind_of_relkind(&relkind);
    let comment: Option<String> = rel.try_get(1)?;
    let row_estimate: Option<i64> = rel.try_get(2)?;

    // Primary key (ordered)
    let pk_rows = client
        .query(
            "SELECT a.attname::text, k.ord::int \
             FROM pg_constraint con \
             JOIN pg_class c ON c.oid = con.conrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             CROSS JOIN LATERAL unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord) \
             JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum = k.attnum \
             WHERE con.contype = 'p' AND n.nspname = $1 AND c.relname = $2 ORDER BY k.ord",
            params,
        )
        .await?;
    let mut primary_key: Vec<String> = Vec::with_capacity(pk_rows.len());
    let mut pk_ordinals: BTreeMap<String, u32> = BTreeMap::new();
    for row in pk_rows {
        let col: String = row.try_get(0)?;
        let ord: i32 = row.try_get(1)?;
        pk_ordinals.insert(col.clone(), ord as u32);
        primary_key.push(col);
    }

    // Columns
    let col_rows = client
        .query(
            "SELECT a.attnum::int, a.attname::text, format_type(a.atttypid, a.atttypmod)::text, \
                    t.typname::text, t.typtype::text, t.typcategory::text, \
                    bt.typname::text, bt.typtype::text, bt.typcategory::text, \
                    et.typname::text, \
                    NOT a.attnotnull, \
                    pg_get_expr(d.adbin, d.adrelid)::text, \
                    a.attidentity::text, a.attgenerated <> '', \
                    col_description(c.oid, a.attnum)::text, \
                    (SELECT string_agg(e.enumlabel::text, E'\\x1f' ORDER BY e.enumsortorder) \
                       FROM pg_enum e WHERE e.enumtypid = COALESCE(bt.oid, t.oid)), \
                    (SELECT string_agg(e.enumlabel::text, E'\\x1f' ORDER BY e.enumsortorder) \
                       FROM pg_enum e WHERE e.enumtypid = t.typelem), \
                    CASE WHEN a.attcollation <> 0 AND a.attcollation <> t.typcollation \
                         THEN co.collname::text END, \
                    a.atttypmod::int, COALESCE(bt.typtypmod, -1)::int, \
                    (WITH RECURSIVE dom(base, depth) AS ( \
                       SELECT CASE WHEN t.typtype = 'd' THEN t.typbasetype ELSE et.typbasetype END, 1 \
                        WHERE t.typtype = 'd' OR et.typtype = 'd' \
                       UNION ALL \
                       SELECT d.typbasetype, dom.depth + 1 FROM dom \
                         JOIN pg_type d ON d.oid = dom.base AND d.typtype = 'd') \
                     SELECT format_type(dom.base, NULL) || CASE WHEN t.typtype = 'd' THEN '' ELSE '[]' END \
                       FROM dom ORDER BY dom.depth DESC LIMIT 1) \
             FROM pg_attribute a \
             JOIN pg_class c ON c.oid = a.attrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             JOIN pg_type t ON t.oid = a.atttypid \
             LEFT JOIN pg_type bt ON t.typtype = 'd' AND bt.oid = t.typbasetype \
             LEFT JOIN pg_type et ON t.typcategory = 'A' AND et.oid = t.typelem \
             LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
             LEFT JOIN pg_collation co ON co.oid = a.attcollation \
             WHERE n.nspname = $1 AND c.relname = $2 AND a.attnum > 0 AND NOT a.attisdropped \
             ORDER BY a.attnum",
            params,
        )
        .await?;
    let mut columns = Vec::with_capacity(col_rows.len());
    for row in col_rows {
        let attnum: i32 = row.try_get(0)?;
        let col_name: String = row.try_get(1)?;
        let data_type: String = row.try_get(2)?;
        let typname: String = row.try_get(3)?;
        let typtype: Option<String> = row.try_get(4)?;
        let typcategory: Option<String> = row.try_get(5)?;
        let base_typname: Option<String> = row.try_get(6)?;
        let base_typtype: Option<String> = row.try_get(7)?;
        let base_typcategory: Option<String> = row.try_get(8)?;
        let elem_typname: Option<String> = row.try_get(9)?;
        let nullable: bool = row.try_get(10)?;
        let default_expr: Option<String> = row.try_get(11)?;
        let identity: Option<String> = row.try_get(12)?;
        let is_generated: bool = row.try_get(13)?;
        let col_comment: Option<String> = row.try_get(14)?;
        let enum_values: Option<String> = row.try_get(15)?;
        let elem_enum_values: Option<String> = row.try_get(16)?;
        let collation: Option<String> = row.try_get(17)?;
        let atttypmod: i32 = row.try_get(18)?;
        let domain_typmod: i32 = row.try_get(19)?;
        // A domain (or an array of one): the type it is over, through nested domains.
        let base_type: Option<String> = row.try_get(20)?;

        // Domains are classified by their base type.
        let (eff_name, eff_type, eff_cat) = match base_typname.as_deref() {
            Some(base) => (
                base.to_string(),
                first_char(base_typtype.as_deref()),
                first_char(base_typcategory.as_deref()),
            ),
            None => (
                typname.clone(),
                first_char(typtype.as_deref()),
                first_char(typcategory.as_deref()),
            ),
        };
        let kind = kind_from_pg_type(&eff_name, eff_type, eff_cat, elem_typname.as_deref());

        let enum_values = match kind {
            ValueKind::Enum => Some(split_list(enum_values.as_deref())),
            ValueKind::Array => {
                let v = split_list(elem_enum_values.as_deref());
                if v.is_empty() { None } else { Some(v) }
            }
            _ => None,
        };

        let typmod = if atttypmod == -1 && base_typname.is_some() {
            domain_typmod
        } else {
            atttypmod
        };
        let typmod_type = match (kind, elem_typname.as_deref()) {
            (ValueKind::Array, Some(elem)) => elem.to_string(),
            _ => eff_name.clone(),
        };
        let (char_max_len, numeric_precision, numeric_scale) = typmod_info(&typmod_type, typmod);

        let identity = identity.filter(|s| !s.is_empty());
        let is_auto_increment = identity.is_some()
            || default_expr
                .as_deref()
                .map(|d| d.starts_with("nextval("))
                .unwrap_or(false);
        let extra = identity.as_deref().map(|i| match i {
            "a" => "ALWAYS".to_string(),
            "d" => "BY DEFAULT".to_string(),
            other => other.to_string(),
        });

        columns.push(ColumnMeta {
            is_pk: pk_ordinals.contains_key(&col_name),
            pk_ordinal: pk_ordinals.get(&col_name).copied(),
            name: col_name,
            ordinal: attnum.max(0) as u32,
            data_type,
            kind,
            nullable,
            default_expr,
            is_auto_increment,
            is_generated,
            is_unsigned: false,
            comment: col_comment,
            enum_values,
            char_max_len,
            numeric_precision,
            numeric_scale,
            collation,
            array_elem_type: if kind == ValueKind::Array {
                elem_typname
            } else {
                None
            },
            extra,
            base_type,
        });
    }

    // Indexes
    let mut indexes = Vec::new();
    if matches!(
        kind,
        TableKind::Table | TableKind::Partitioned | TableKind::MaterializedView
    ) {
        let rows = client
            .query(
                "SELECT i.relname::text, ix.indisunique, ix.indisprimary, am.amname::text, \
                        pg_get_indexdef(ix.indexrelid)::text, \
                        pg_get_expr(ix.indpred, ix.indrelid)::text, \
                        (SELECT string_agg(pg_get_indexdef(ix.indexrelid, k.ord::int, true), \
                                           E'\\x1f' ORDER BY k.ord) \
                           FROM unnest(ix.indkey::int2[]) WITH ORDINALITY AS k(attnum, ord) \
                          WHERE k.ord <= ix.indnkeyatts), \
                        obj_description(ix.indexrelid, 'pg_class')::text, \
                        (SELECT string_agg(a.attname::text, E'\\x1f' ORDER BY k.ord) \
                           FROM unnest(ix.indkey::int2[]) WITH ORDINALITY AS k(attnum, ord) \
                           JOIN pg_attribute a ON a.attrelid = ix.indrelid AND a.attnum = k.attnum \
                          WHERE k.ord > ix.indnkeyatts) \
                 FROM pg_index ix \
                 JOIN pg_class i ON i.oid = ix.indexrelid \
                 JOIN pg_class t ON t.oid = ix.indrelid \
                 JOIN pg_namespace n ON n.oid = t.relnamespace \
                 JOIN pg_am am ON am.oid = i.relam \
                 WHERE n.nspname = $1 AND t.relname = $2 \
                 ORDER BY ix.indisprimary DESC, i.relname",
                params,
            )
            .await?;
        for row in rows {
            let cols: Option<String> = row.try_get(6)?;
            let include: Option<String> = row.try_get(8)?;
            indexes.push(IndexMeta {
                name: row.try_get(0)?,
                columns: split_list(cols.as_deref()),
                unique: row.try_get(1)?,
                primary: row.try_get(2)?,
                method: row.try_get(3)?,
                predicate: row.try_get(5)?,
                definition: row.try_get(4)?,
                comment: row.try_get(7)?,
                include: split_list(include.as_deref()),
            });
        }
    }

    // Foreign keys
    let mut foreign_keys = Vec::new();
    if matches!(kind, TableKind::Table | TableKind::Partitioned) {
        let rows = client
            .query(
                "SELECT con.conname::text, con.confupdtype::text, con.confdeltype::text, \
                        fn.nspname::text, fc.relname::text, pg_get_constraintdef(con.oid)::text, \
                        (SELECT string_agg(a.attname::text, E'\\x1f' ORDER BY k.ord) \
                           FROM unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord) \
                           JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum), \
                        (SELECT string_agg(a.attname::text, E'\\x1f' ORDER BY k.ord) \
                           FROM unnest(con.confkey) WITH ORDINALITY AS k(attnum, ord) \
                           JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.attnum) \
                 FROM pg_constraint con \
                 JOIN pg_class c ON c.oid = con.conrelid \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 JOIN pg_class fc ON fc.oid = con.confrelid \
                 JOIN pg_namespace fn ON fn.oid = fc.relnamespace \
                 WHERE con.contype = 'f' AND n.nspname = $1 AND c.relname = $2 \
                 ORDER BY con.conname",
                params,
            )
            .await?;
        for row in rows {
            let on_update: String = row.try_get(1)?;
            let on_delete: String = row.try_get(2)?;
            let ref_schema: String = row.try_get(3)?;
            let ref_table: String = row.try_get(4)?;
            let cols: Option<String> = row.try_get(6)?;
            let ref_cols: Option<String> = row.try_get(7)?;
            foreign_keys.push(ForeignKeyMeta {
                name: row.try_get(0)?,
                columns: split_list(cols.as_deref()),
                ref_table: TableRef::new(Some(&ref_schema), &ref_table),
                ref_columns: split_list(ref_cols.as_deref()),
                on_update: fk_action(&on_update),
                on_delete: fk_action(&on_delete),
                definition: row.try_get(5)?,
            });
        }
    }

    // CHECK constraints
    let mut checks = Vec::new();
    if matches!(kind, TableKind::Table | TableKind::Partitioned) {
        let rows = client
            .query(
                "SELECT con.conname::text, pg_get_constraintdef(con.oid)::text, \
                        (SELECT string_agg(a.attname::text, E'\\x1f' ORDER BY k.ord) \
                           FROM unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord) \
                           JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum) \
                 FROM pg_constraint con \
                 JOIN pg_class c ON c.oid = con.conrelid \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE con.contype = 'c' AND n.nspname = $1 AND c.relname = $2 \
                 ORDER BY con.conname",
                params,
            )
            .await?;
        for row in rows {
            let def: Option<String> = row.try_get(1)?;
            let cols: Option<String> = row.try_get(2)?;
            checks.push(CheckMeta {
                name: row.try_get(0)?,
                expression: check_expression(def.as_deref().unwrap_or("")),
                columns: split_list(cols.as_deref()),
            });
        }
    }

    // Triggers (views can have INSTEAD OF triggers, so always query)
    let mut triggers = Vec::new();
    {
        let rows = client
            .query(
                "SELECT t.tgname::text, t.tgenabled <> 'D', pg_get_triggerdef(t.oid, true)::text \
                 FROM pg_trigger t \
                 JOIN pg_class c ON c.oid = t.tgrelid \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE NOT t.tgisinternal AND n.nspname = $1 AND c.relname = $2 \
                 ORDER BY t.tgname",
                params,
            )
            .await?;
        for row in rows {
            let definition: String = row.try_get(2)?;
            let (timing, events) = parse_trigger_def(&definition);
            triggers.push(TriggerMeta {
                name: row.try_get(0)?,
                timing,
                events,
                enabled: row.try_get(1)?,
                definition,
            });
        }
    }

    Ok(TableMeta {
        table: TableRef::new(Some(&schema), name),
        kind,
        columns,
        primary_key,
        indexes,
        foreign_keys,
        triggers,
        comment,
        row_estimate,
        ddl: None,
        checks,
        options: None,
    })
}

pub async fn completion_metadata(client: &Traced<'_>) -> Result<CompletionMetadata, AppError> {
    let mut meta = CompletionMetadata::default();
    for schema in list_schemas(client).await? {
        if schema.is_default {
            meta.default_schema = Some(schema.name.clone());
        }
        meta.schemas.insert(schema.name, BTreeMap::new());
    }
    if meta.default_schema.is_none() {
        meta.default_schema = default_schema(client).await?;
    }
    let sql = format!(
        "SELECT n.nspname::text, c.relname::text, \
                string_agg(a.attname::text, E'\\x1f' ORDER BY a.attnum) \
         FROM pg_class c \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         LEFT JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
         WHERE c.relkind IN ('r','p','v','m','f') AND {USER_SCHEMA_FILTER} \
         GROUP BY n.nspname, c.relname ORDER BY 1, 2"
    );
    for row in client.query(&sql, &[]).await? {
        let schema: String = row.try_get(0)?;
        let table: String = row.try_get(1)?;
        let cols: Option<String> = row.try_get(2)?;
        meta.schemas
            .entry(schema)
            .or_default()
            .insert(table, split_list(cols.as_deref()));
    }
    Ok(meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trigger_definitions() {
        let (timing, events) = parse_trigger_def(
            "CREATE TRIGGER posts_set_updated_at BEFORE UPDATE ON public.posts FOR EACH ROW EXECUTE FUNCTION set_updated_at()",
        );
        assert_eq!(timing.as_deref(), Some("BEFORE"));
        assert_eq!(events, ["UPDATE"]);

        let (timing, events) = parse_trigger_def(
            "CREATE TRIGGER t AFTER INSERT OR UPDATE OF title, body OR DELETE ON t FOR EACH ROW EXECUTE FUNCTION f()",
        );
        assert_eq!(timing.as_deref(), Some("AFTER"));
        assert_eq!(events, ["INSERT", "UPDATE", "DELETE"]);

        let (timing, events) = parse_trigger_def(
            "CREATE TRIGGER v_ins INSTEAD OF INSERT ON v FOR EACH ROW EXECUTE FUNCTION f()",
        );
        assert_eq!(timing.as_deref(), Some("INSTEAD OF"));
        assert_eq!(events, ["INSERT"]);

        let (timing, events) = parse_trigger_def(
            "CREATE TRIGGER trunc AFTER TRUNCATE ON t FOR EACH STATEMENT EXECUTE FUNCTION f()",
        );
        assert_eq!(timing.as_deref(), Some("AFTER"));
        assert_eq!(events, ["TRUNCATE"]);

        assert_eq!(parse_trigger_def("garbage"), (None, vec![]));
    }

    #[test]
    fn decodes_typmods() {
        assert_eq!(typmod_info("varchar", 259), (Some(255), None, None));
        assert_eq!(typmod_info("varchar", -1), (None, None, None));
        // numeric(20,6): ((20 << 16) | 6) + 4
        assert_eq!(
            typmod_info("numeric", ((20 << 16) | 6) + 4),
            (None, Some(20), Some(6))
        );
        // numeric(5,-2)
        assert_eq!(
            typmod_info("numeric", ((5 << 16) | ((-2i32) & 0x7ff)) + 4),
            (None, Some(5), Some(-2))
        );
        assert_eq!(typmod_info("int8", -1), (None, Some(64), Some(0)));
        assert_eq!(typmod_info("varbit", 8), (Some(8), None, None));
        assert_eq!(typmod_info("text", -1), (None, None, None));
    }

    #[test]
    fn maps_fk_actions_and_relkinds() {
        assert_eq!(fk_action("c"), "CASCADE");
        assert_eq!(fk_action("n"), "SET NULL");
        assert_eq!(fk_action("d"), "SET DEFAULT");
        assert_eq!(fk_action("r"), "RESTRICT");
        assert_eq!(fk_action("a"), "NO ACTION");
        assert_eq!(kind_of_relkind("r"), TableKind::Table);
        assert_eq!(kind_of_relkind("p"), TableKind::Partitioned);
        assert_eq!(kind_of_relkind("v"), TableKind::View);
        assert_eq!(kind_of_relkind("m"), TableKind::MaterializedView);
        assert_eq!(kind_of_relkind("f"), TableKind::ForeignTable);
    }
}
