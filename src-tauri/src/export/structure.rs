//! The structure around the rows of an SQL export script, ordered as pg_dump orders a dump:
//! every `DROP TABLE` first (tables that reference another one before it), then every table
//! without its foreign keys (referenced tables first), then the rows, then the sequence resets,
//! and last the foreign keys and triggers. So a script loads over an existing copy (no table is
//! dropped while another one still references it), into an empty database (sequences and
//! partitions exist before the tables that use them), and with any rows: a key is checked once
//! every row is in (also a table referencing itself), and no trigger fires on the loaded rows.
//!
//! PostgreSQL partitions: a partitioned table is created with its `PARTITION BY` key, together
//! with the partitions the script does not hold on their own (`CREATE TABLE … PARTITION OF`,
//! then the indexes those partitions have of their own; their own triggers come with the other
//! triggers); a partition the script holds is created as a table of its own (with its indexes)
//! and then attached. Rows are read once, from the topmost partitioned table in the script,
//! never from one of its partitions as well.
//!
//! PostgreSQL sequences: a sequence several tables of the script draw from is created once
//! (before the first of them) and moved past the rows of all of them once (one `setval`); a table
//! drawing from a sequence another table of the script owns comes after it (and is dropped
//! before it, so the owner's DROP does not fail on the dependent default).

use crate::drivers::DbSession;
use crate::error::AppError;
use crate::sqlgen::ddl::{
    PgPartition, PgPartitionObject, PgPartitionObjectKind, PgSequence, PgTableContext,
    partition_objects_sql, partition_of_sql, pg_context_ancestors_sql, pg_context_clones_sql,
    pg_context_partition_objects_sql, pg_context_partitions_sql, pg_context_relation_sql,
    pg_context_sequences_sql, shared_sequence_reset_sql, table_parts,
};
use crate::sqlgen::transfer::{
    drop_table_if_exists, foreign_key_footer, foreign_key_header, script_footer, script_header,
    table_structure, time_zone_footer, time_zone_header,
};
use crate::sqlgen::{BuiltSql, Dialect};
use crate::types::schema::{TableMeta, TableRef};
use crate::types::value::Value;

use super::is_view;
use super::writer::SqlScript;

/// The same table: the same name, and the same schema where both references name one.
pub fn same_table(a: &TableRef, b: &TableRef) -> bool {
    a.name == b.name
        && match (a.schema.as_deref(), b.schema.as_deref()) {
            (Some(x), Some(y)) => x == y,
            _ => true,
        }
}

/// A table of a script with what the catalog adds to its metadata (PostgreSQL).
#[derive(Debug, Clone)]
pub struct ScriptTable<'a> {
    pub meta: &'a TableMeta,
    pub context: PgTableContext,
}

/// The parts of a script around its rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptLayout {
    /// Indexes into the tables in the order their rows load (referenced tables first).
    pub order: Vec<usize>,
    /// Per table: whether the script carries its rows (a partition whose partitioned ancestor is
    /// in the script is loaded through that ancestor).
    pub rows: Vec<bool>,
    /// Header, DROPs and CREATEs, ending in a blank line (empty when there are none).
    pub opening: String,
    /// After the rows: sequence resets, foreign keys, triggers (one section each, lines joined).
    pub closing: Vec<String>,
    /// The statements that end the script (MySQL: foreign key checks restored).
    pub footer: Vec<String>,
}

/// The tables in an order that loads: a table after the tables in `parents` (the tables it
/// references or is a partition of), otherwise in the given order. Tables in a reference cycle
/// keep the given order.
pub fn dependency_order(parents: &[Vec<usize>]) -> Vec<usize> {
    let n = parents.len();
    let mut placed = vec![false; n];
    let mut order = Vec::with_capacity(n);
    while order.len() < n {
        let next = (0..n).find(|&i| !placed[i] && parents[i].iter().all(|&p| p == i || placed[p]));
        // A cycle: take the first table left.
        let i = next.unwrap_or_else(|| (0..n).find(|&i| !placed[i]).unwrap_or(0));
        placed[i] = true;
        order.push(i);
    }
    order
}

/// Lays out a script of `tables` (DROP / structure / rows as `script` asks). `with_header` adds
/// the statements around a multi-table script (MySQL: no foreign key checks while it loads). A
/// script with rows always sets the session time zone to `zone` (the zone the exporting session
/// read the values in; `None` = the dialect's default) and puts the previous one back at its end.
pub fn script_layout(
    dialect: Dialect,
    tables: &[ScriptTable<'_>],
    script: SqlScript,
    with_header: bool,
    zone: Option<&str>,
) -> ScriptLayout {
    let position = |t: &TableRef| tables.iter().position(|s| same_table(&s.meta.table, t));
    let sequences = shared_sequences(tables);
    let parents: Vec<Vec<usize>> = tables
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mut ps: Vec<usize> = t
                .meta
                .foreign_keys
                .iter()
                .map(|fk| &fk.ref_table)
                .chain(&t.context.ancestors)
                .filter_map(&position)
                .collect();
            // the owner of a sequence this table draws from
            ps.extend(
                sequences
                    .iter()
                    .filter(|s| s.users.iter().any(|&(u, _)| u == i))
                    .flat_map(|s| s.owners.iter().copied()),
            );
            ps.sort_unstable();
            ps.dedup();
            ps
        })
        .collect();
    let order = dependency_order(&parents);
    let rows: Vec<bool> = tables
        .iter()
        .map(|t| !t.context.ancestors.iter().any(|a| position(a).is_some()))
        .collect();

    let structured: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&i| !is_view(tables[i].meta.kind))
        .collect();
    let mut drops = String::new();
    if script.drop_table {
        for &i in structured.iter().rev() {
            drops.push_str(&drop_table_if_exists(dialect, &tables[i].meta.table));
            drops.push('\n');
        }
    }
    let mut creates = Vec::new();
    let (mut resets, mut foreign_keys, mut triggers) = (Vec::new(), Vec::new(), Vec::new());
    if script.structure {
        let mut created_sequences: Vec<String> = Vec::new();
        // per shared sequence: whether one of its tables resets it (it owns it)
        let mut shared_reset = vec![false; sequences.len()];
        for &i in &structured {
            let t = &tables[i];
            if dialect == Dialect::Postgres {
                let mut parts = table_parts(dialect, t.meta, &t.context);
                parts.sequences.retain(|s| !created_sequences.contains(s));
                created_sequences.extend(parts.sequences.iter().cloned());
                let partitions = own_partitions(tables, i);
                let partition_of: Vec<String> =
                    partitions.iter().map(|p| partition_of_sql(p)).collect();
                let mut partition_indexes = Vec::new();
                let mut partition_triggers = Vec::new();
                for p in &partitions {
                    let (indexes, triggers) = partition_objects_sql(p);
                    partition_indexes.extend(indexes);
                    partition_triggers.extend(triggers);
                }
                creates.push(
                    parts
                        .create_sections(&partition_of, &partition_indexes)
                        .join("\n\n"),
                );
                if script.content {
                    for (reset, column) in
                        parts.sequence_resets.into_iter().zip(&parts.reset_columns)
                    {
                        let shared = sequences
                            .iter()
                            .position(|s| s.users.iter().any(|(u, c)| *u == i && c == column));
                        match shared {
                            Some(k) => shared_reset[k] = true,
                            None => resets.push(reset),
                        }
                    }
                }
                foreign_keys.extend(parts.foreign_keys);
                triggers.extend(parts.triggers);
                triggers.extend(partition_triggers);
            } else {
                // The server's own DDL (foreign keys inline: MySQL loads with its checks off,
                // SQLite checks them on the rows only).
                creates.push(table_structure(dialect, t.meta));
            }
        }
        for (s, reset) in sequences.iter().zip(shared_reset) {
            if reset {
                let columns: Vec<(TableRef, String)> = s
                    .users
                    .iter()
                    .map(|(u, c)| (tables[*u].meta.table.clone(), c.clone()))
                    .collect();
                resets.push(shared_sequence_reset_sql(s.sequence, &columns));
            }
        }
    }

    let mut opening = String::new();
    // Without rows there are no timestamps: a structure-only script sets no time zone.
    let header = match (with_header, script.content) {
        (true, true) => script_header(dialect, zone),
        (true, false) => foreign_key_header(dialect),
        (false, true) => time_zone_header(dialect, zone),
        (false, false) => Vec::new(),
    };
    for line in header {
        opening.push_str(&line);
        opening.push('\n');
    }
    opening.push_str(&drops);
    if !drops.is_empty() && (creates.is_empty() || tables.len() > 1) {
        opening.push('\n');
    }
    for create in &creates {
        opening.push_str(create.trim_end());
        opening.push_str("\n\n");
    }
    let closing = [resets, foreign_keys, triggers]
        .into_iter()
        .filter(|section| !section.is_empty())
        .map(|section| section.join("\n"))
        .collect();
    ScriptLayout {
        order,
        rows,
        opening,
        closing,
        footer: match (with_header, script.content) {
            (true, true) => script_footer(dialect),
            (true, false) => foreign_key_footer(dialect),
            (false, true) => time_zone_footer(dialect),
            (false, false) => Vec::new(),
        },
    }
}

/// The text of `closing` sections, a blank line between them (and before the first one when
/// `blank_before`).
pub fn closing_text(sections: &[String], blank_before: bool) -> String {
    if sections.is_empty() {
        return String::new();
    }
    let body = sections
        .iter()
        .map(|s| format!("{s}\n"))
        .collect::<Vec<_>>()
        .join("\n");
    if blank_before {
        format!("\n{body}")
    } else {
        body
    }
}

/// A PostgreSQL sequence that several tables of a script draw from (column defaults).
struct SharedSequence<'a> {
    sequence: &'a PgSequence,
    /// The tables (indexes into the script's tables) and columns that draw from it.
    users: Vec<(usize, String)>,
    /// The tables of the script whose column owns it.
    owners: Vec<usize>,
}

/// The sequences that more than one table of the script draws from (views left out).
fn shared_sequences<'a>(tables: &'a [ScriptTable<'_>]) -> Vec<SharedSequence<'a>> {
    let mut all: Vec<SharedSequence<'a>> = Vec::new();
    for (i, t) in tables.iter().enumerate() {
        if is_view(t.meta.kind) {
            continue;
        }
        for s in &t.context.sequences {
            let k = match all
                .iter()
                .position(|x| x.sequence.schema == s.schema && x.sequence.name == s.name)
            {
                Some(k) => k,
                None => {
                    all.push(SharedSequence {
                        sequence: s,
                        users: Vec::new(),
                        owners: Vec::new(),
                    });
                    all.len() - 1
                }
            };
            let entry = &mut all[k];
            if !entry.users.iter().any(|(u, c)| *u == i && c == &s.column) {
                entry.users.push((i, s.column.clone()));
            }
            if s.owned && !entry.owners.contains(&i) {
                entry.owners.push(i);
            }
        }
    }
    all.retain(|s| {
        let first = s.users.first().map(|(u, _)| *u);
        s.users.iter().any(|(u, _)| Some(*u) != first)
    });
    all
}

/// The partitions table `i` recreates with itself: those below it that the script does not hold,
/// and that hang from it through tables the script does not hold either (a partition in the
/// script recreates its own).
fn own_partitions<'a>(tables: &'a [ScriptTable<'_>], i: usize) -> Vec<&'a PgPartition> {
    let me = &tables[i].meta.table;
    let partitions = &tables[i].context.partitions;
    let held = |t: &TableRef| tables.iter().any(|s| same_table(&s.meta.table, t));
    let parent_of = |t: &TableRef| {
        partitions
            .iter()
            .find(|p| same_table(&p.table, t))
            .map(|p| &p.parent)
    };
    partitions
        .iter()
        .filter(|p| {
            if held(&p.table) {
                return false;
            }
            let mut up = &p.parent;
            for _ in 0..=partitions.len() {
                if same_table(up, me) {
                    return true;
                }
                if held(up) {
                    return false;
                }
                match parent_of(up) {
                    Some(next) => up = next,
                    None => return false,
                }
            }
            false
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// PostgreSQL catalog context
// ---------------------------------------------------------------------------------------------

/// The catalog context a script of `meta` needs: PostgreSQL tables only, when the script holds
/// their structure (`structure`) or several tables (whose partitions must not load twice).
/// A catalog that cannot answer (a server too old for these catalogs) leaves it empty: the
/// script then has what `TableMeta` says.
pub async fn context_for(
    session: &dyn DbSession,
    meta: &TableMeta,
    structure: bool,
    several: bool,
) -> PgTableContext {
    if session.dialect() != Dialect::Postgres || is_view(meta.kind) || !(structure || several) {
        return PgTableContext::default();
    }
    match pg_table_context(session, &meta.table, structure).await {
        Ok(context) => context,
        Err(e) => {
            log::warn!("export: no catalog context for {}: {e}", meta.table.name);
            PgTableContext::default()
        }
    }
}

/// Reads what the PostgreSQL catalog adds to a table's metadata (`PgTableContext`); the
/// partitions below a partitioned table only when `partitions`.
pub async fn pg_table_context(
    session: &dyn DbSession,
    table: &TableRef,
    partitions: bool,
) -> Result<PgTableContext, AppError> {
    let mut context = PgTableContext::default();
    let relation = rows(session, pg_context_relation_sql(table)).await?;
    let Some(row) = relation.first() else {
        return Ok(context);
    };
    let partitioned = text(row, 0).as_deref() == Some("p");
    let is_partition = text(row, 1).as_deref() == Some("true");
    context.partition_key = text(row, 2).filter(|_| partitioned);
    if is_partition {
        context.partition_bound = text(row, 3);
        context.ancestors = rows(session, pg_context_ancestors_sql(table))
            .await?
            .iter()
            .filter_map(|r| Some(TableRef::new(Some(&text(r, 0)?), &text(r, 1)?)))
            .collect();
    }
    if partitions && partitioned {
        context.partitions = rows(session, pg_context_partitions_sql(table))
            .await?
            .iter()
            .filter_map(|r| {
                Some(PgPartition {
                    table: TableRef::new(Some(&text(r, 0)?), &text(r, 1)?),
                    parent: TableRef::new(Some(&text(r, 2)?), &text(r, 3)?),
                    bound: text(r, 4)?,
                    key: text(r, 5),
                    objects: Vec::new(),
                })
            })
            .collect();
        if !context.partitions.is_empty() {
            for r in rows(session, pg_context_partition_objects_sql(table)).await? {
                let (Some(schema), Some(name)) = (text(&r, 0), text(&r, 1)) else {
                    continue;
                };
                let kind = match text(&r, 2).as_deref() {
                    Some("i") => PgPartitionObjectKind::Index,
                    Some("c") => PgPartitionObjectKind::Constraint,
                    Some("t") => PgPartitionObjectKind::Trigger,
                    _ => continue,
                };
                let (Some(object), Some(definition)) = (text(&r, 3), text(&r, 4)) else {
                    continue;
                };
                let partition = TableRef::new(Some(&schema), &name);
                if let Some(p) = context
                    .partitions
                    .iter_mut()
                    .find(|p| same_table(&p.table, &partition))
                {
                    p.objects.push(PgPartitionObject {
                        kind,
                        name: object,
                        definition,
                        disabled: text(&r, 5).as_deref() == Some("true"),
                    });
                }
            }
        }
    }
    context.sequences = rows(session, pg_context_sequences_sql(table))
        .await?
        .iter()
        .filter_map(|r| {
            let number = |i: usize| text(r, i).and_then(|t| t.parse::<i64>().ok());
            Some(PgSequence {
                column: text(r, 0)?,
                schema: text(r, 1)?,
                name: text(r, 2)?,
                data_type: text(r, 3),
                start: number(4),
                increment: number(5),
                min: number(6),
                max: number(7),
                cache: number(8),
                cycle: text(r, 9).as_deref() == Some("true"),
                owned: text(r, 10).as_deref() == Some("true"),
            })
        })
        .collect();
    for r in rows(session, pg_context_clones_sql(table)).await? {
        match (text(&r, 0).as_deref(), text(&r, 1)) {
            (Some("f"), Some(name)) => context.inherited_fks.push(name),
            (Some("t"), Some(name)) => context.cloned_triggers.push(name),
            _ => {}
        }
    }
    Ok(context)
}

async fn rows(session: &dyn DbSession, query: BuiltSql) -> Result<Vec<Vec<Value>>, AppError> {
    Ok(session.query(&query.sql, &query.params).await?.rows)
}

fn text(row: &[Value], i: usize) -> Option<String> {
    row.get(i).and_then(Value::as_text)
}
