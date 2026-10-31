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

use super::catalog::text_rows;
use crate::drivers::postgres::flavor::PgFlavorHooks;
use crate::drivers::postgres::introspect::{self, Relation, Traced, kind_of_relkind, schema_of};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::ddl::{PgMppClauses, PgTableContext, table_parts};
use crate::types::admin::ProcessRow;
use crate::types::caps::{Feature, SessionCaps};
use crate::types::driver::PgFlavor;
use crate::types::schema::{
    CompletionMetadata, SchemaInfo, TableKind, TableMeta, TableOptions, TableRef, TableSummary,
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
            // ON CONFLICT came with PostgreSQL 9.5: the import offers no upsert.
            adjusted.data.upsert = false;
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
    /// As the Structure tab shows them (`TableOptions`: the storage as the engine, and the
    /// distribution).
    fn table_options(&self) -> TableOptions {
        let engine = match (&self.storage, &self.options) {
            (Some(s), Some(o)) => Some(format!("{s} ({o})")),
            (Some(s), None) => Some(s.clone()),
            (None, Some(o)) => Some(o.clone()),
            (None, None) => None,
        };
        TableOptions {
            engine,
            distribution: self.distribution.clone(),
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
    let Details {
        distribution,
        storage,
        options,
        partitioning,
    } = details;
    let (partition_key, legacy_partitioning) = if legacy {
        (None, partitioning)
    } else {
        (partitioning, None)
    };
    let context = PgTableContext {
        partition_key,
        mpp: PgMppClauses {
            // Greenplum 6 names its storage in `WITH (…)` only.
            access_method: storage.filter(|am| !legacy && am != "heap"),
            storage_options: options,
            distribution,
            partitioning: legacy_partitioning,
        },
        ..PgTableContext::default()
    };
    Ok(table_parts(Dialect::Postgres, meta, &context).script())
}

/// `introspect::table_meta` on Greenplum 6, whose relation this file reads: external tables and
/// legacy partitioned tables are told by their storage and `pg_partition`. The rest is the
/// PostgreSQL driver's, with the catalog SQL of 9.4 that the server version picks there.
async fn legacy_table_meta(c: &Traced<'_>, table: &TableRef) -> Result<TableMeta, AppError> {
    let schema = schema_of(c, table).await?;
    let name = table.name.as_str();
    let rel = c
        .query_opt(LEGACY_RELATION, &[&schema, &name])
        .await?
        .ok_or_else(|| AppError::NotFound(format!("table {schema}.{name}")))?;
    let relkind: String = rel.try_get(0)?;
    let storage: Option<String> = rel.try_get(3)?;
    let partitioned: Option<bool> = rel.try_get(4)?;
    let relation = Relation {
        kind: kind_of(&relkind, storage.as_deref(), partitioned == Some(true)),
        comment: rel.try_get(1)?,
        row_estimate: rel.try_get(2)?,
    };
    introspect::table_meta_of(c, schema, name, relation).await
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{self, CLOUDBERRY, CoreColumn, GREENPLUM6, GREENPLUM7};
    use super::*;
    use crate::drivers::DbSession;
    use crate::drivers::postgres::testing::{Cell, Col, FakePg};
    use crate::sqlgen::SqlDialect;
    use crate::types::driver::Flavor;
    use crate::types::schema::ValueKind;

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
            let columns = introspect::columns_sql(v);
            assert_eq!(columns.contains("attidentity"), !legacy, "{v}");
            assert_eq!(columns.contains("attgenerated"), !legacy, "{v}");
            let indexes = introspect::indexes_sql(v);
            assert_eq!(indexes.contains("indnkeyatts"), !legacy, "{v}");
        }
        // From Greenplum 7 on, the column and index queries are the PostgreSQL driver's own.
        assert!(
            introspect::columns_sql(120_012).contains("a.attidentity::text, a.attgenerated <> ''")
        );
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
            assert_eq!(caps.data.upsert, !legacy, "{v}");
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
        assert_eq!(options.distribution.as_deref(), Some("DISTRIBUTED BY (id)"));
        assert_eq!(options.row_format, None);
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
