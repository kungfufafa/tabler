//! Greenplum and its forks: Greenplum 6 (and Greengage, WarehousePG 6) is PostgreSQL 9.4, Greenplum
//! 7 is PostgreSQL 12, Apache Cloudberry 2 is PostgreSQL 14. The catalog SQL is chosen by
//! `server_version_num`, not by name: the PostgreSQL driver's queries run from Greenplum 7 on,
//! and Greenplum 6 gets them without what PostgreSQL 10–12 added (identity and generated
//! columns, INCLUDE columns of indexes).
//!
//! What Greenplum adds is shown read-only: a table's distribution (`DISTRIBUTED BY (…)`,
//! `RANDOMLY`, `REPLICATED`) and storage (heap, append-optimized row or column, external; with
//! its options) in `TableMeta.options`, and both in the table's DDL with its partitioning
//! (Greenplum 6's legacy `PARTITION BY … (START … END … EVERY …)` or the declarative key). The
//! children of a partitioned table are hidden from the table list (Greenplum 6 names them
//! `<parent>_1_prt_<name>`), and no table size is read there, since that would ask every
//! segment.
//!
//! `ctid` is unique on one segment only, so a table without a key is not editable. Greenplum 6
//! lacks the sequence catalog, `array_position`, `relispartition`, `collprovider` and
//! `pg_stat_ssl` the sequences, the diagram, the Create Database options and the session details
//! read, and `backend_type` / `wait_event`, so its sessions come from this file's own query.

use async_trait::async_trait;
use std::collections::BTreeMap;

use super::catalog::{first_char, fk_action, kind_of_relkind, schema_of, text_rows, typmod_info};
use crate::drivers::postgres::decode::{kind_from_pg_type, split_list};
use crate::drivers::postgres::flavor::PgFlavorHooks;
use crate::drivers::postgres::introspect::{self, Traced, parse_trigger_def};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::ddl::{PgTableContext, check_expression, table_parts};
use crate::types::admin::ProcessRow;
use crate::types::caps::{Feature, SessionCaps};
use crate::types::driver::PgFlavor;
use crate::types::schema::{
    CheckMeta, ColumnMeta, CompletionMetadata, ForeignKeyMeta, IndexMeta, SchemaInfo, TableKind,
    TableMeta, TableOptions, TableRef, TableSummary, TriggerMeta, ValueKind,
};

pub(super) struct Greenplum;

pub(super) static GREENPLUM: Greenplum = Greenplum;

/// Greenplum 7 (PostgreSQL 12): the first version the PostgreSQL driver's catalog SQL runs on.
const GP7: i32 = 120_000;
/// PostgreSQL 9.5, 10: what Greenplum 6 (9.4) lacks of them.
const PG95: i32 = 90_500;
const PG10: i32 = 100_000;

/// The schemas Greenplum keeps for itself beyond PostgreSQL's.
const SYSTEM_SCHEMAS: &[&str] = &["gp_toolkit", "pg_aoseg", "pg_bitmapindex", "pg_ext_aux"];

/// A schema's tables, with the storage (Greenplum 6) and whether each is a partitioned table;
/// partitions are left out, and so are sizes. Columns: name, relkind, storage, comment, row
/// estimate, partitioned.
fn list_tables_sql(v: i32) -> &'static str {
    if v < GP7 {
        "SELECT c.relname::text, c.relkind::text, c.relstorage::text, \
                obj_description(c.oid, 'pg_class')::text, \
                CASE WHEN c.reltuples < 0 THEN NULL ELSE c.reltuples::bigint END, \
                EXISTS (SELECT 1 FROM pg_partition p WHERE p.parrelid = c.oid) \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relkind IN ('r','v','m','f') \
           AND NOT EXISTS (SELECT 1 FROM pg_partition_rule r WHERE r.parchildrelid = c.oid) \
         ORDER BY c.relname"
    } else {
        "SELECT c.relname::text, c.relkind::text, NULL::text, \
                obj_description(c.oid, 'pg_class')::text, \
                CASE WHEN c.reltuples < 0 THEN NULL ELSE c.reltuples::bigint END, \
                c.relkind = 'p' \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relkind IN ('r','p','v','m','f') AND NOT c.relispartition \
         ORDER BY c.relname"
    }
}

/// A table's distribution, storage, storage options and partitioning. Columns: the distribution
/// clause, the storage (Greenplum 6 `relstorage`, then the access method), the options, the
/// partitioning (Greenplum 6's whole legacy clause, then the key of `PARTITION BY`).
fn details_sql(v: i32) -> &'static str {
    if v < GP7 {
        "SELECT pg_get_table_distributedby(c.oid)::text, c.relstorage::text, \
                array_to_string(c.reloptions, ', ')::text, \
                pg_get_partition_def(c.oid, true)::text \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relname = $2"
    } else {
        "SELECT pg_get_table_distributedby(c.oid)::text, am.amname::text, \
                array_to_string(c.reloptions, ', ')::text, pg_get_partkeydef(c.oid)::text \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         LEFT JOIN pg_am am ON am.oid = c.relam \
         WHERE n.nspname = $1 AND c.relname = $2"
    }
}

/// The relation of a table on Greenplum 6: kind, comment, row estimate, storage, partitioned.
const LEGACY_RELATION: &str = "SELECT c.relkind::text, obj_description(c.oid, 'pg_class')::text, \
            CASE WHEN c.reltuples < 0 THEN NULL ELSE c.reltuples::bigint END, \
            c.relstorage::text, \
            EXISTS (SELECT 1 FROM pg_partition p WHERE p.parrelid = c.oid) \
     FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
     WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind IN ('r','v','m','f')";

const PRIMARY_KEY: &str = "SELECT a.attname::text, k.ord::int \
     FROM pg_constraint con \
     JOIN pg_class c ON c.oid = con.conrelid \
     JOIN pg_namespace n ON n.oid = c.relnamespace \
     CROSS JOIN LATERAL unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord) \
     JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum = k.attnum \
     WHERE con.contype = 'p' AND n.nspname = $1 AND c.relname = $2 ORDER BY k.ord";

/// The PostgreSQL driver's column query, without identity columns before PostgreSQL 10 and
/// generated ones before 12 (`introspect::table_meta` reads the same columns).
fn columns_sql(v: i32) -> String {
    let identity = if v < PG10 {
        "NULL::text"
    } else {
        "a.attidentity::text"
    };
    let generated = if v < GP7 {
        "false"
    } else {
        "a.attgenerated <> ''"
    };
    format!(
        "SELECT a.attnum::int, a.attname::text, format_type(a.atttypid, a.atttypmod)::text, \
                t.typname::text, t.typtype::text, t.typcategory::text, \
                bt.typname::text, bt.typtype::text, bt.typcategory::text, \
                et.typname::text, \
                NOT a.attnotnull, \
                pg_get_expr(d.adbin, d.adrelid)::text, \
                {identity}, {generated}, \
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
         ORDER BY a.attnum"
    )
}

/// The PostgreSQL driver's index query; before PostgreSQL 11 every index column is a key
/// (`indnatts`, no `indnkeyatts`).
fn indexes_sql(v: i32) -> String {
    let keys = if v < 110_000 {
        "ix.indnatts"
    } else {
        "ix.indnkeyatts"
    };
    format!(
        "SELECT i.relname::text, ix.indisunique, ix.indisprimary, am.amname::text, \
                pg_get_indexdef(ix.indexrelid)::text, \
                pg_get_expr(ix.indpred, ix.indrelid)::text, \
                (SELECT string_agg(pg_get_indexdef(ix.indexrelid, k.ord::int, true), \
                                   E'\\x1f' ORDER BY k.ord) \
                   FROM unnest(ix.indkey::int2[]) WITH ORDINALITY AS k(attnum, ord) \
                  WHERE k.ord <= {keys}), \
                obj_description(ix.indexrelid, 'pg_class')::text, \
                (SELECT string_agg(a.attname::text, E'\\x1f' ORDER BY k.ord) \
                   FROM unnest(ix.indkey::int2[]) WITH ORDINALITY AS k(attnum, ord) \
                   JOIN pg_attribute a ON a.attrelid = ix.indrelid AND a.attnum = k.attnum \
                  WHERE k.ord > {keys}) \
         FROM pg_index ix \
         JOIN pg_class i ON i.oid = ix.indexrelid \
         JOIN pg_class t ON t.oid = ix.indrelid \
         JOIN pg_namespace n ON n.oid = t.relnamespace \
         JOIN pg_am am ON am.oid = i.relam \
         WHERE n.nspname = $1 AND t.relname = $2 \
         ORDER BY ix.indisprimary DESC, i.relname"
    )
}

const FOREIGN_KEYS: &str = "SELECT con.conname::text, con.confupdtype::text, con.confdeltype::text, \
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
     ORDER BY con.conname";

const CHECKS: &str = "SELECT con.conname::text, pg_get_constraintdef(con.oid)::text, \
            (SELECT string_agg(a.attname::text, E'\\x1f' ORDER BY k.ord) \
               FROM unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord) \
               JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum) \
     FROM pg_constraint con \
     JOIN pg_class c ON c.oid = con.conrelid \
     JOIN pg_namespace n ON n.oid = c.relnamespace \
     WHERE con.contype = 'c' AND n.nspname = $1 AND c.relname = $2 \
     ORDER BY con.conname";

const TRIGGERS: &str = "SELECT t.tgname::text, t.tgenabled <> 'D', pg_get_triggerdef(t.oid, true)::text \
     FROM pg_trigger t \
     JOIN pg_class c ON c.oid = t.tgrelid \
     JOIN pg_namespace n ON n.oid = c.relnamespace \
     WHERE NOT t.tgisinternal AND n.nspname = $1 AND c.relname = $2 \
     ORDER BY t.tgname";

/// Greenplum 6's sessions as the PostgreSQL driver lists them (`sqlgen::admin::PG_PROCESS_LIST`,
/// the same columns): without `backend_type`, and waits from `waiting` / `waiting_reason`.
const LEGACY_PROCESSES: &str = "SELECT pid::text, usename::text, datname::text, \
     application_name::text, \
     CASE WHEN client_addr IS NOT NULL THEN host(client_addr) || ':' || client_port::text \
          WHEN client_port = -1 THEN 'local socket' END, \
     state::text, \
     COALESCE(CASE WHEN state = 'active' THEN query_start END, state_change, backend_start)::text, \
     (EXTRACT(EPOCH FROM clock_timestamp() - COALESCE(CASE WHEN state = 'active' THEN query_start END, \
          state_change, backend_start)) * 1000)::bigint::text, \
     CASE WHEN waiting THEN COALESCE(waiting_reason::text, 'waiting') END, \
     query::text, \
     (pid = pg_backend_pid())::text, \
     (application_name = 'tabler')::text \
     FROM pg_stat_activity ORDER BY pid";

#[async_trait]
impl PgFlavorHooks for Greenplum {
    fn flavor(&self) -> Option<PgFlavor> {
        Some(PgFlavor::Greenplum)
    }

    async fn list_schemas(&self, c: &Traced<'_>) -> Option<Result<Vec<SchemaInfo>, AppError>> {
        Some(introspect::list_schemas(c).await.map(|schemas| {
            schemas
                .into_iter()
                .filter(|s| !SYSTEM_SCHEMAS.contains(&s.name.as_str()))
                .collect()
        }))
    }

    async fn list_tables(
        &self,
        c: &Traced<'_>,
        schema: &str,
    ) -> Option<Result<Vec<TableSummary>, AppError>> {
        Some(list_tables(c, schema).await)
    }

    async fn table_meta(
        &self,
        c: &Traced<'_>,
        table: &TableRef,
    ) -> Option<Result<TableMeta, AppError>> {
        Some(table_meta(c, table).await.map(|(meta, _)| meta))
    }

    async fn completion_metadata(
        &self,
        c: &Traced<'_>,
    ) -> Option<Result<CompletionMetadata, AppError>> {
        Some(introspect::completion_metadata(c).await.map(|mut meta| {
            meta.schemas
                .retain(|s, _| !SYSTEM_SCHEMAS.contains(&s.as_str()));
            meta
        }))
    }

    /// Greenplum 6's own query; from 7 on the PostgreSQL one.
    async fn list_processes(&self, c: &Traced<'_>) -> Option<Result<Vec<ProcessRow>, AppError>> {
        if c.server_version_num() >= GP7 {
            return None;
        }
        Some(legacy_processes(c).await)
    }

    async fn table_ddl(
        &self,
        c: &Traced<'_>,
        table: &TableRef,
    ) -> Option<Result<String, AppError>> {
        match table_meta(c, table).await {
            Ok((meta, details))
                if matches!(
                    meta.kind,
                    TableKind::Table | TableKind::Partitioned | TableKind::ForeignTable
                ) =>
            {
                Some(table_ddl(c, &meta, details))
            }
            // Views and materialized views: the PostgreSQL driver's `CREATE … VIEW`.
            Ok(_) => None,
            Err(e) => Some(Err(e)),
        }
    }

    fn adjust_caps(&self, caps: &mut SessionCaps, server_version_num: i32) {
        // pg_cancel_backend / pg_terminate_backend by pid, as on PostgreSQL.
        let mut adjusted = caps
            .clone()
            .with(&[Feature::KillProcess])
            .without(&[Feature::EditWithoutKey]);
        if server_version_num < PG10 {
            adjusted = adjusted.without(&[
                Feature::Sequences,
                Feature::Diagram,
                Feature::CreateDatabase,
            ]);
        }
        if server_version_num < PG95 {
            adjusted = adjusted.without(&[Feature::SessionDetails]);
        }
        *caps = adjusted;
    }
}

async fn legacy_processes(c: &Traced<'_>) -> Result<Vec<ProcessRow>, AppError> {
    let rows = c.query(LEGACY_PROCESSES, &[]).await?;
    Ok(crate::admin::pg_process_rows(&text_rows(&rows)?))
}

/// What a relation's kind and Greenplum 6's storage make of it.
fn kind_of(relkind: &str, storage: Option<&str>, partitioned: bool) -> TableKind {
    match (kind_of_relkind(relkind), storage) {
        (TableKind::Table, Some("x")) => TableKind::ForeignTable,
        (TableKind::Table, _) if partitioned => TableKind::Partitioned,
        (kind, _) => kind,
    }
}

/// The kinds of relation that are distributed over the segments.
fn has_distribution(kind: TableKind) -> bool {
    matches!(
        kind,
        TableKind::Table | TableKind::Partitioned | TableKind::MaterializedView
    )
}

async fn list_tables(c: &Traced<'_>, schema: &str) -> Result<Vec<TableSummary>, AppError> {
    let rows = c
        .query(list_tables_sql(c.server_version_num()), &[&schema])
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let relkind: String = row.try_get(1)?;
        let storage: Option<String> = row.try_get(2)?;
        let partitioned: Option<bool> = row.try_get(5)?;
        out.push(TableSummary {
            name: row.try_get(0)?,
            schema: Some(schema.to_string()),
            kind: kind_of(&relkind, storage.as_deref(), partitioned == Some(true)),
            row_estimate: row.try_get(4)?,
            size_bytes: None,
            comment: row.try_get(3)?,
        });
    }
    Ok(out)
}

/// What `details_sql` reads.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Details {
    /// `DISTRIBUTED BY (a)`, `DISTRIBUTED RANDOMLY`, `DISTRIBUTED REPLICATED`.
    distribution: Option<String>,
    /// `heap`, `ao_row`, `ao_column`, `external`…
    storage: Option<String>,
    /// `appendonly=true, orientation=column, compresstype=zstd`.
    options: Option<String>,
    /// Greenplum 6: the whole `PARTITION BY …` clause; later: its key.
    partitioning: Option<String>,
}

impl Details {
    /// As the Structure tab shows them (`TableOptions`: the storage as the engine, the
    /// distribution as the row format).
    fn table_options(&self) -> TableOptions {
        let engine = match (&self.storage, &self.options) {
            (Some(s), Some(o)) => Some(format!("{s} ({o})")),
            (Some(s), None) => Some(s.clone()),
            (None, Some(o)) => Some(o.clone()),
            (None, None) => None,
        };
        TableOptions {
            engine,
            row_format: self.distribution.clone(),
            ..TableOptions::default()
        }
    }
}

/// Greenplum 6's `relstorage` letters.
fn storage_name(code: &str) -> &str {
    match code {
        "h" => "heap",
        "a" => "ao_row",
        "c" => "ao_column",
        "x" => "external",
        "f" => "foreign",
        "v" => "virtual",
        other => other,
    }
}

async fn details(c: &Traced<'_>, table: &TableRef) -> Result<Details, AppError> {
    let v = c.server_version_num();
    let row = c
        .query_opt(
            details_sql(v),
            &[&table.schema.as_deref().unwrap_or("public"), &table.name],
        )
        .await?;
    let Some(row) = row else {
        return Ok(Details::default());
    };
    let text = |i: usize| -> Result<Option<String>, AppError> {
        Ok(row
            .try_get::<_, Option<String>>(i)?
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()))
    };
    let storage = text(1)?.map(|s| {
        if v < GP7 {
            storage_name(&s).to_string()
        } else {
            s
        }
    });
    Ok(Details {
        distribution: text(0)?,
        storage,
        options: text(2)?,
        partitioning: text(3)?,
    })
}

/// The table and what Greenplum adds to it.
async fn table_meta(c: &Traced<'_>, table: &TableRef) -> Result<(TableMeta, Details), AppError> {
    let mut meta = if c.server_version_num() < GP7 {
        legacy_table_meta(c, table).await?
    } else {
        introspect::table_meta(c, table).await?
    };
    let details = if has_distribution(meta.kind) {
        details(c, &meta.table).await?
    } else {
        Details::default()
    };
    if details != Details::default() {
        meta.options = Some(details.table_options());
    }
    Ok((meta, details))
}

/// The PostgreSQL driver's DDL of the table, with Greenplum's storage, distribution and
/// partitioning after its column list, as `CREATE TABLE` takes them there: `USING am` (7 on),
/// `WITH (…)`, `DISTRIBUTED …`, and Greenplum 6's partition clause last.
fn table_ddl(c: &Traced<'_>, meta: &TableMeta, details: Details) -> Result<String, AppError> {
    if meta.kind == TableKind::ForeignTable {
        return Err(AppError::Unsupported(
            "Greenplum: the DDL of an external table is not shown".into(),
        ));
    }
    let legacy = c.server_version_num() < GP7;
    let context = PgTableContext {
        partition_key: details.partitioning.clone().filter(|_| !legacy),
        ..PgTableContext::default()
    };
    let mut parts = table_parts(Dialect::Postgres, meta, &context);
    if let Some(create) = parts.table.first_mut() {
        let mut clauses = Vec::new();
        if !legacy && let Some(am) = details.storage.as_deref().filter(|am| *am != "heap") {
            clauses.push(format!("USING {}", Dialect::Postgres.quote_ident(am)));
        }
        if let Some(options) = &details.options {
            clauses.push(format!("WITH ({options})"));
        }
        clauses.extend(details.distribution.clone());
        if legacy {
            clauses.extend(details.partitioning.clone());
        }
        if !clauses.is_empty() {
            let body = create.trim_end().trim_end_matches(';');
            *create = format!("{body}\n{};", clauses.join("\n"));
        }
    }
    Ok(parts.script())
}

/// `introspect::table_meta` with Greenplum 6's catalog SQL, plus its external tables and legacy
/// partitioned tables.
async fn legacy_table_meta(c: &Traced<'_>, table: &TableRef) -> Result<TableMeta, AppError> {
    let v = c.server_version_num();
    let schema = schema_of(c, table).await?;
    let name = table.name.as_str();
    let params: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[&schema, &name];

    let rel = c
        .query_opt(LEGACY_RELATION, params)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("table {schema}.{name}")))?;
    let relkind: String = rel.try_get(0)?;
    let comment: Option<String> = rel.try_get(1)?;
    let row_estimate: Option<i64> = rel.try_get(2)?;
    let storage: Option<String> = rel.try_get(3)?;
    let partitioned: Option<bool> = rel.try_get(4)?;
    let kind = kind_of(&relkind, storage.as_deref(), partitioned == Some(true));

    let mut primary_key = Vec::new();
    let mut pk_ordinals: BTreeMap<String, u32> = BTreeMap::new();
    for row in c.query(PRIMARY_KEY, params).await? {
        let col: String = row.try_get(0)?;
        let ord: i32 = row.try_get(1)?;
        pk_ordinals.insert(col.clone(), ord.max(0) as u32);
        primary_key.push(col);
    }

    let mut columns = Vec::new();
    for row in c.query(&columns_sql(v), params).await? {
        columns.push(column(&row, &pk_ordinals)?);
    }

    let mut indexes = Vec::new();
    if matches!(
        kind,
        TableKind::Table | TableKind::Partitioned | TableKind::MaterializedView
    ) {
        for row in c.query(&indexes_sql(v), params).await? {
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

    let mut foreign_keys = Vec::new();
    let mut checks = Vec::new();
    if matches!(kind, TableKind::Table | TableKind::Partitioned) {
        for row in c.query(FOREIGN_KEYS, params).await? {
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
        for row in c.query(CHECKS, params).await? {
            let def: Option<String> = row.try_get(1)?;
            let cols: Option<String> = row.try_get(2)?;
            checks.push(CheckMeta {
                name: row.try_get(0)?,
                expression: check_expression(def.as_deref().unwrap_or("")),
                columns: split_list(cols.as_deref()),
            });
        }
    }

    let mut triggers = Vec::new();
    for row in c.query(TRIGGERS, params).await? {
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

/// One row of `columns_sql`, read as `introspect::table_meta` reads it.
fn column(row: &tokio_postgres::Row, pk: &BTreeMap<String, u32>) -> Result<ColumnMeta, AppError> {
    let attnum: i32 = row.try_get(0)?;
    let name: String = row.try_get(1)?;
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
    let comment: Option<String> = row.try_get(14)?;
    let enum_values: Option<String> = row.try_get(15)?;
    let elem_enum_values: Option<String> = row.try_get(16)?;
    let collation: Option<String> = row.try_get(17)?;
    let atttypmod: i32 = row.try_get(18)?;
    let domain_typmod: i32 = row.try_get(19)?;
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
            .is_some_and(|d| d.starts_with("nextval("));
    let extra = identity.as_deref().map(|i| match i {
        "a" => "ALWAYS".to_string(),
        "d" => "BY DEFAULT".to_string(),
        other => other.to_string(),
    });
    Ok(ColumnMeta {
        is_pk: pk.contains_key(&name),
        pk_ordinal: pk.get(&name).copied(),
        name,
        ordinal: attnum.max(0) as u32,
        data_type,
        kind,
        nullable,
        default_expr,
        is_auto_increment,
        is_generated,
        is_unsigned: false,
        comment,
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
    })
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{self, CLOUDBERRY, CoreColumn, GREENPLUM6, GREENPLUM7};
    use super::*;
    use crate::drivers::DbSession;
    use crate::drivers::postgres::testing::{Cell, Col, FakePg};
    use crate::sqlgen::SqlDialect;
    use crate::types::driver::Flavor;

    const VERSIONS: [(&str, &str, i32); 3] = [
        (GREENPLUM6, "9.4.26", 90_426),
        (GREENPLUM7, "12.12", 120_012),
        (CLOUDBERRY, "14.4", 140_004),
    ];

    #[test]
    fn the_catalog_sql_follows_the_server_version() {
        for (_, _, v) in VERSIONS {
            let legacy = v < GP7;
            let tables = list_tables_sql(v);
            assert_eq!(tables.contains("pg_partition_rule"), legacy, "{v}");
            assert_eq!(tables.contains("relispartition"), !legacy, "{v}");
            assert!(!tables.contains("pg_total_relation_size"), "{v}");
            let details = details_sql(v);
            assert!(details.contains("pg_get_table_distributedby"), "{v}");
            assert_eq!(details.contains("relstorage"), legacy, "{v}");
            assert_eq!(details.contains("pg_get_partition_def"), legacy, "{v}");
            assert_eq!(details.contains("pg_get_partkeydef"), !legacy, "{v}");
            let columns = columns_sql(v);
            assert_eq!(columns.contains("attidentity"), !legacy, "{v}");
            assert_eq!(columns.contains("attgenerated"), !legacy, "{v}");
            let indexes = indexes_sql(v);
            assert_eq!(indexes.contains("indnkeyatts"), !legacy, "{v}");
        }
        // From Greenplum 7 on, the column and index queries are the PostgreSQL driver's own.
        assert!(columns_sql(120_012).contains("a.attidentity::text, a.attgenerated <> ''"));
    }

    #[test]
    fn caps_follow_the_server_version() {
        for (_, _, v) in VERSIONS {
            let mut caps = crate::drivers::postgres::flavor::adjust_caps(SessionCaps::base(
                crate::types::profile::Driver::Postgres,
                SqlDialect::Postgres,
                Some(Flavor::Greenplum),
            ));
            GREENPLUM.adjust_caps(&mut caps, v);
            assert!(caps.has(Feature::KillProcess) && caps.has(Feature::EditRows));
            assert!(!caps.has(Feature::EditWithoutKey));
            for f in [Feature::StructureEdit, Feature::Backup, Feature::Users] {
                assert!(!caps.has(f), "{v} {f:?}");
            }
            let legacy = v < GP7;
            for f in [
                Feature::Sequences,
                Feature::Diagram,
                Feature::CreateDatabase,
                Feature::SessionDetails,
            ] {
                assert_eq!(caps.has(f), !legacy, "{v} {f:?}");
            }
        }
    }

    #[tokio::test]
    async fn a_session_on_each_version() {
        for (version, server_version, v) in VERSIONS {
            let pg = FakePg::new()
                .server_version(server_version)
                .version(version)
                .start()
                .await;
            let session = testkit::session(&testkit::profile(pg.port())).await;
            assert_eq!(session.flavor(), Some(Flavor::Greenplum));
            assert_eq!(session.dialect(), SqlDialect::Postgres);
            assert_eq!(session.server_version_num, v);
            // The PostgreSQL init, as one batch.
            assert!(pg.queries()[1].starts_with("SET DateStyle = 'ISO, YMD'; SET IntervalStyle"));
            assert_eq!(session.capabilities().has(Feature::Diagram), v >= GP7);
            session.close().await;
        }
    }

    fn table_row(name: &str, relkind: &str, storage: Option<&str>, partitioned: bool) -> Vec<Cell> {
        vec![
            Cell::text(name),
            Cell::text(relkind),
            storage.map_or(Cell::Null, Cell::text),
            Cell::Null,
            Cell::Int(10),
            Cell::Bool(partitioned),
        ]
    }

    fn table_columns() -> Vec<Col> {
        vec![
            Col::text("relname"),
            Col::text("relkind"),
            Col::text("storage"),
            Col::text("comment"),
            Col::int8("reltuples"),
            Col::bool("partitioned"),
        ]
    }

    #[tokio::test]
    async fn lists_tables_without_partitions_or_sizes() {
        let pg = FakePg::new()
            .rows(
                "pg_partition_rule",
                table_columns(),
                vec![
                    table_row("sales", "r", Some("c"), true),
                    table_row("ext", "r", Some("x"), false),
                    table_row("v", "v", Some("v"), false),
                ],
            )
            .start()
            .await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 90_426);
        let tables = GREENPLUM.list_tables(&c, "public").await.unwrap().unwrap();
        let kinds: Vec<TableKind> = tables.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            [
                TableKind::Partitioned,
                TableKind::ForeignTable,
                TableKind::View
            ]
        );
        assert!(tables.iter().all(|t| t.size_bytes.is_none()));
    }

    #[tokio::test]
    async fn hides_the_system_schemas() {
        let pg = FakePg::new()
            .rows(
                "FROM pg_namespace n WHERE",
                vec![Col::text("nspname"), Col::bool("is_default")],
                ["gp_toolkit", "pg_aoseg", "pg_bitmapindex", "public"]
                    .iter()
                    .map(|s| vec![Cell::text(*s), Cell::Bool(*s == "public")])
                    .collect(),
            )
            .start()
            .await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 120_012);
        let schemas = GREENPLUM.list_schemas(&c).await.unwrap().unwrap();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "public");
    }

    fn details_row(
        pg: FakePg,
        storage: &str,
        options: Option<&str>,
        partition: Option<&str>,
    ) -> FakePg {
        pg.rows(
            "pg_get_table_distributedby",
            vec![
                Col::text("distribution"),
                Col::text("storage"),
                Col::text("options"),
                Col::text("partitioning"),
            ],
            vec![vec![
                Cell::text("DISTRIBUTED BY (id)"),
                Cell::text(storage),
                options.map_or(Cell::Null, Cell::text),
                partition.map_or(Cell::Null, Cell::text),
            ]],
        )
    }

    /// Greenplum 6: the 9.4 catalog SQL, the storage and distribution read-only, and the DDL with
    /// the legacy partition clause.
    #[tokio::test]
    async fn greenplum_6_tables() {
        let column_row = |attnum: i64, name: &str, ty: &str, typname: &str| {
            let mut row = vec![
                Cell::Int(attnum),
                Cell::text(name),
                Cell::text(ty),
                Cell::text(typname),
                Cell::text("b"),
                Cell::text("N"),
            ];
            row.extend([Cell::Null, Cell::Null, Cell::Null, Cell::Null]);
            row.extend([Cell::Bool(false), Cell::Null, Cell::Null, Cell::Bool(false)]);
            row.extend([Cell::Null, Cell::Null, Cell::Null, Cell::Null]);
            row.extend([Cell::Int(-1), Cell::Int(-1), Cell::Null]);
            row
        };
        let mut column_types = vec![Col::int4("attnum")];
        column_types.extend((1..10).map(|i| Col::text(&format!("t{i}"))));
        column_types.extend([
            Col::bool("nullable"),
            Col::text("default"),
            Col::text("identity"),
        ]);
        column_types.push(Col::bool("generated"));
        column_types.extend((14..18).map(|i| Col::text(&format!("t{i}"))));
        column_types.extend([Col::int4("typmod"), Col::int4("dtypmod"), Col::text("base")]);
        let pg = FakePg::new()
            .rows(
                "c.relstorage::text, EXISTS (SELECT 1 FROM pg_partition p",
                vec![
                    Col::text("relkind"),
                    Col::text("comment"),
                    Col::int8("reltuples"),
                    Col::text("relstorage"),
                    Col::bool("partitioned"),
                ],
                vec![vec![
                    Cell::text("r"),
                    Cell::Null,
                    Cell::Int(1000),
                    Cell::text("c"),
                    Cell::Bool(true),
                ]],
            )
            .rows(
                "CROSS JOIN LATERAL unnest(con.conkey)",
                vec![Col::text("attname"), Col::int4("ord")],
                vec![vec![Cell::text("id"), Cell::Int(1)]],
            )
            .rows(
                "format_type(a.atttypid, a.atttypmod)::text",
                column_types,
                vec![
                    column_row(1, "id", "bigint", "int8"),
                    column_row(2, "sold", "date", "date"),
                ],
            );
        let pg = details_row(
            pg,
            "c",
            Some("appendonly=true, orientation=column, compresstype=zstd"),
            Some("PARTITION BY RANGE(sold) (START ('2024-01-01'::date) END ('2025-01-01'::date) EVERY ('1 mon'::interval))"),
        )
        .start()
        .await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 90_426);
        let table = TableRef::new(Some("public"), "sales");
        let meta = GREENPLUM.table_meta(&c, &table).await.unwrap().unwrap();
        assert_eq!(meta.kind, TableKind::Partitioned);
        assert_eq!(meta.primary_key, ["id"]);
        assert_eq!(meta.columns.len(), 2);
        assert_eq!(meta.columns[0].kind, ValueKind::BigInt);
        let options = meta.options.clone().unwrap();
        assert_eq!(options.row_format.as_deref(), Some("DISTRIBUTED BY (id)"));
        assert_eq!(
            options.engine.as_deref(),
            Some("ao_column (appendonly=true, orientation=column, compresstype=zstd)")
        );
        // The 9.4 catalog: no identity, generated or INCLUDE columns.
        let sent = pg.queries().join("\n");
        assert!(sent.contains("NULL::text, false"), "{sent}");
        assert!(sent.contains("ix.indnatts") && !sent.contains("indnkeyatts"));
        assert!(!sent.contains("attidentity") && !sent.contains("relispartition"));

        let ddl = GREENPLUM.table_ddl(&c, &table).await.unwrap().unwrap();
        assert!(
            ddl.starts_with("CREATE TABLE \"public\".\"sales\" ("),
            "{ddl}"
        );
        assert!(
            ddl.contains(
                ")\nWITH (appendonly=true, orientation=column, compresstype=zstd)\nDISTRIBUTED BY \
                 (id)\nPARTITION BY RANGE(sold)"
            ),
            "{ddl}"
        );
        assert!(!ddl.contains("USING"), "{ddl}");
    }

    /// Greenplum 7 and Cloudberry: the PostgreSQL driver's catalog SQL, the access method and the
    /// declarative partition key in the DDL.
    #[tokio::test]
    async fn greenplum_7_tables() {
        for v in [120_012, 140_004] {
            let pg = testkit::core_table(
                FakePg::new(),
                "p",
                &[CoreColumn {
                    attnum: 1,
                    name: "id",
                    data_type: "bigint",
                    typname: "int8",
                    nullable: false,
                    default: None,
                    typmod: -1,
                }],
                &["id"],
            );
            let pg = details_row(pg, "ao_row", Some("compresstype=zstd"), Some("RANGE (id)"))
                .start()
                .await;
            let (client, trace) = testkit::client(&pg).await;
            let c = Traced::new(&client, &trace, v);
            let table = TableRef::new(Some("public"), "events");
            let meta = GREENPLUM.table_meta(&c, &table).await.unwrap().unwrap();
            assert_eq!(meta.kind, TableKind::Partitioned);
            assert_eq!(
                meta.options.unwrap().engine.as_deref(),
                Some("ao_row (compresstype=zstd)")
            );
            let ddl = GREENPLUM.table_ddl(&c, &table).await.unwrap().unwrap();
            assert!(
                ddl.contains(
                    ") PARTITION BY RANGE (id)\nUSING \"ao_row\"\nWITH (compresstype=zstd)\n\
                     DISTRIBUTED BY (id);"
                ),
                "{ddl}"
            );
            assert!(pg.queries().iter().any(|q| q.contains("am.amname")));
            // From 7 on, the PostgreSQL process list.
            assert!(GREENPLUM.list_processes(&c).await.is_none());
        }
    }

    #[tokio::test]
    async fn greenplum_6_sessions() {
        let pg = FakePg::new()
            .rows(
                "COALESCE(waiting_reason::text, 'waiting')",
                (0..12).map(|i| Col::text(&format!("c{i}"))).collect(),
                vec![
                    [
                        "4242",
                        "gpadmin",
                        "warehouse",
                        "tabler",
                        "10.0.0.2:5000",
                        "active",
                        "2026-10-29 10:00:00+00",
                        "15",
                        "lock",
                        "SELECT 1",
                        "true",
                        "true",
                    ]
                    .iter()
                    .map(|s| Cell::text(*s))
                    .collect(),
                ],
            )
            .start()
            .await;
        let (client, trace) = testkit::client(&pg).await;
        let c = Traced::new(&client, &trace, 90_426);
        let rows = GREENPLUM.list_processes(&c).await.unwrap().unwrap();
        assert_eq!(rows[0].id, "4242");
        assert_eq!(rows[0].wait_event.as_deref(), Some("lock"));
        assert!(rows[0].current && rows[0].own);
        assert!(!pg.queries()[0].contains("backend_type"));
        // Kills stay the PostgreSQL driver's (pg_cancel_backend by pid).
        assert!(
            GREENPLUM
                .kill(
                    &c,
                    &["4242".to_string()],
                    crate::types::admin::KillScope::Query
                )
                .await
                .is_none()
        );
    }
}
