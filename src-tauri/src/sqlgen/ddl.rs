//! DDL generation (M5): turns table-designer operations into dialect SQL following the ALTER
//! TABLE matrix of the backend design, plus `CREATE TABLE` reconstruction for "Show DDL".
//!
//! Identifiers only go through `Dialect::quote_ident`/`qualify`; data types, defaults, `USING`
//! and index predicates are the user's own SQL and are inserted verbatim; comments are quoted
//! literals. Everything here is pure: execution lives in `commands::ddl`.
//!
//! The planner keeps a working model of the table whose metadata it was given, so later ops in a
//! batch see the effect of earlier ones (MySQL `MODIFY COLUMN` needs the complete, current column
//! definition; PG needs the primary-key constraint name).
//!
//! SQLite cannot change most column properties in place. Such an op starts a "12-step" rebuild
//! (sqlite.org/lang_altertable.html#otheralter) and every following op on the same table is
//! folded into it: create `<t>__tabler_new` from the model, copy the rows (and the
//! AUTOINCREMENT counter), drop the old table, rename (with `legacy_alter_table` on, so views and
//! other tables' triggers that name the table do not fail the schema check while it is
//! missing), replay indexes and triggers. Column renames are never folded into the new table:
//! they run as `RENAME COLUMN` after it is in place, so SQLite itself rewrites triggers, views
//! and the foreign keys of child tables. `PRAGMA foreign_keys` is a no-op inside a transaction,
//! so a plan containing a rebuild is non-transactional and carries its own `BEGIN`/`COMMIT`;
//! before the commit it checks the rebuilt table and every table referencing it.

use std::sync::LazyLock;

use regex::Regex;

use crate::error::AppError;
use crate::sqlgen::quote::MysqlLiteralMode;
use crate::sqlgen::{BuiltSql, Dialect};
use crate::types::ddl::{ColumnDef, DdlOp, DdlPlan, IndexDef};
use crate::types::schema::{
    CheckMeta, ColumnMeta, IndexMeta, TableKind, TableMeta, TableOptions, TableRef, TriggerMeta,
    ValueKind,
};
use crate::types::value::Value;

/// Suffix of the scratch table a SQLite rebuild copies into.
pub const REBUILD_SUFFIX: &str = "__tabler_new";
/// Suffix of a temporary column name used while renames of a rebuild are sorted out.
const RENAME_SUFFIX: &str = "__tabler_tmp";
/// Suffix of the scratch table whose column rename makes SQLite check views and triggers
/// after a rebuild dropped columns.
const CHECK_SUFFIX: &str = "__tabler_check";
/// Toggles around the drop + rename of a SQLite rebuild.
pub const LEGACY_ALTER_ON: &str = "PRAGMA legacy_alter_table = ON";
pub const LEGACY_ALTER_OFF: &str = "PRAGMA legacy_alter_table = OFF";

/// An identifier as SQL spells it: `"quoted"`, `` `quoted` ``, `[quoted]` or bare.
const IDENT: &str = r#"(?:"(?:[^"]|"")*"|`(?:[^`]|``)*`|\[[^\]]*\]|[^\s"`\[\]().,;]+)"#;

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static regex")
}

/// Literals SQLite accepts as a constant `DEFAULT` (and therefore in `ADD COLUMN`).
static SQLITE_LITERAL: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)^(?:[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?|0x[0-9a-f]+|'(?:[^']|'')*'|x'[0-9a-f]*'|null|true|false)$",
    )
});
static SQLITE_TIME_KEYWORD: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^current_(?:time|date|timestamp)$"));
static NUMERIC: LazyLock<Regex> =
    LazyLock::new(|| re(r"^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$"));
/// `CURRENT_TIMESTAMP` and its synonyms: the only non-literal defaults MySQL accepts without
/// parentheses (temporal columns only).
static MYSQL_NOW: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)^(?:(?:current_timestamp|localtime|localtimestamp)(?:\(\s*\d*\s*\))?|now\(\s*\d*\s*\))$",
    )
});
/// A MySQL literal usable as `ALTER COLUMN … SET DEFAULT` / plain `DEFAULT` value.
static MYSQL_LITERAL: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r#"(?i)^(?:[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?|0x[0-9a-f]+|0b[01]+|null|true|false|(?:_[a-z0-9]+\s*|n)?'(?:[^'\\]|''|\\.)*'|"(?:[^"\\]|""|\\.)*"|[bx]'[0-9a-f]*'|(?:date|time|timestamp)\s*'(?:[^'\\]|''|\\.)*')$"#,
    )
});
/// A complete single-quoted literal (MariaDB reports string defaults this way).
static QUOTED_LITERAL: LazyLock<Regex> = LazyLock::new(|| re(r"^'(?:[^'\\]|''|\\.)*'$"));
static MYSQL_ON_UPDATE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\bon update ([a-z_]+(?:\(\d*\))?)"));
/// A character set or collation named in a MySQL column type (`ASCII` / `UNICODE` are
/// shorthands for `CHARACTER SET latin1` / `ucs2`).
static MYSQL_CHARSET_CLAUSE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\b(?:character\s+set|charset|collate|ascii|unicode)\b"));
/// A bare function call as MariaDB reports expression defaults (`uuid()`, no space before `(`).
static FUNCTION_CALL: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z_][A-Za-z0-9_]*\(.*\)$"));
/// `NOT NULL` at the top level of a column definition.
static NOT_NULL: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bNOT\s+NULL\b"));
static COLLATION_NAME: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z0-9_.@-]+$"));
static INDEX_METHOD: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z_][A-Za-z0-9_]*$"));
/// `name`, `name(10)`, `name DESC`: a plain column reference inside an index column list.
static INDEX_PART: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^([^\s()]+)(\(\d+\))?(?:\s+(asc|desc))?$"));
/// The same with an arbitrary (possibly spaced) name, checked against known columns.
static INDEX_PART_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^(.*?)(\(\d+\))?(?:\s+(asc|desc))?$"));
static QUALIFIED: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"^{IDENT}\s*\.\s*{IDENT}$")));
static INDEX_DEF: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"(?is)^\s*CREATE\s+(UNIQUE\s+)?INDEX\s+(IF\s+NOT\s+EXISTS\s+)?({IDENT}(?:\s*\.\s*{IDENT})?)\s+ON\s+({IDENT}(?:\s*\.\s*{IDENT})?)\s*(\(.*)$"
    ))
});
static TRIGGER_DEF: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"(?is)^(\s*CREATE\s+(?:TEMP\s+|TEMPORARY\s+)?TRIGGER\s+(?:IF\s+NOT\s+EXISTS\s+)?)({IDENT}(?:\s*\.\s*{IDENT})?)(.*)$"
    ))
});
static FK_TOGGLE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^\s*PRAGMA\s+(?:\S+\s*\.\s*)?foreign_keys\s*="));
static SESSION_TOGGLE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^\s*PRAGMA\s+(?:\S+\s*\.\s*)?(foreign_keys|legacy_alter_table)\s*="));
static FK_CHECK: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?is)^\s*PRAGMA\s+(?:\S+\s*\.\s*)?foreign_key_check\b|^\s*SELECT\b.*\bpragma_foreign_key_check\s*\(",
    )
});
/// PG statements that refuse to run inside a transaction block.
static PG_NON_TRANSACTIONAL: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?is)\bCONCURRENTLY\b|^\s*(?:VACUUM|CREATE\s+DATABASE|DROP\s+DATABASE|ALTER\s+SYSTEM|CREATE\s+TABLESPACE|DROP\s+TABLESPACE)\b|^\s*ALTER\s+TYPE\b.*\bADD\s+VALUE\b",
    )
});

/// SQLite statements that fail or do nothing inside a transaction.
static SQLITE_NON_TRANSACTIONAL: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^\s*(?:VACUUM|PRAGMA)\b"));
/// `COLLATE name` at the top level of a column definition.
static COLUMN_COLLATE: LazyLock<Regex> = LazyLock::new(|| {
    re(r#"(?i)\bCOLLATE\s+("(?:[^"]|"")*"|`(?:[^`]|``)*`|\[[^\]]*\]|[A-Za-z0-9_]+)"#)
});

/// `PRAGMA foreign_keys = …` (the toggles around a SQLite rebuild).
pub fn is_fk_toggle(sql: &str) -> bool {
    FK_TOGGLE.is_match(sql)
}

/// `PRAGMA foreign_keys = …` or `PRAGMA legacy_alter_table = …`: connection settings a SQLite
/// rebuild changes and that must be restored even when the plan fails. Returns the setting.
pub fn session_toggle(sql: &str) -> Option<String> {
    SESSION_TOGGLE
        .captures(sql)
        .map(|c| c[1].to_ascii_lowercase())
}

/// `PRAGMA foreign_key_check…` or a `SELECT` over `pragma_foreign_key_check(…)` (the check a
/// SQLite rebuild ends with): rows returned by it are violations.
pub fn is_fk_check(sql: &str) -> bool {
    FK_CHECK.is_match(sql)
}

/// The check run before a SQLite rebuild commits: foreign keys of the rebuilt table and of every
/// table referencing it (a parent losing a key or a column breaks its children). Returns the
/// violations as `table, rowid, parent, fkid` rows; a child whose key no longer matches makes
/// it fail with "foreign key mismatch".
fn sqlite_fk_check_sql(schema: Option<&str>, table: &str) -> String {
    let d = Dialect::Sqlite;
    let schema = non_main(schema).unwrap_or("main");
    let name = d.quote_literal(table);
    let lit_schema = d.quote_literal(schema);
    format!(
        "SELECT k.* FROM {}.sqlite_master AS m, pragma_foreign_key_check(m.name, {lit_schema}) AS k \
         WHERE m.type = 'table' AND (m.name = {name} COLLATE NOCASE OR EXISTS \
         (SELECT 1 FROM pragma_foreign_key_list(m.name, {lit_schema}) AS f WHERE f.\"table\" = {name} COLLATE NOCASE))",
        d.quote_ident(schema)
    )
}

// ---------------------------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------------------------

/// What planning needs to know about the designed table beyond its `TableMeta` (looked up on the
/// server by `commands::ddl`).
#[derive(Debug, Clone, Default)]
pub struct PlanContext {
    /// PG: indexes of the table that back a constraint (`pg_constraint.conindid` of a PRIMARY
    /// KEY, UNIQUE or EXCLUDE constraint). PostgreSQL refuses `DROP INDEX` on them, so dropping
    /// one becomes `ALTER TABLE … DROP CONSTRAINT` (the constraint has the index's name).
    pub constraint_indexes: Vec<String>,
    /// PG: sequences owned by a column of the table without being its identity sequence
    /// (`serial`, `OWNED BY`): `(column, sequence)`. Turning such a column into an identity
    /// column detaches them first (see `pg_auto_increment`).
    pub owned_sequences: Vec<(String, TableRef)>,
    /// MySQL: how the session that runs the plan reads backslashes in string literals
    /// (comments, defaults); `sql_mode` with `NO_BACKSLASH_ESCAPES` takes them as they are.
    pub mysql_mode: MysqlLiteralMode,
}

/// Plans `ops` for `dialect`. `meta` describes the table being designed (when there is one);
/// ops that need it (MySQL `MODIFY`, PG drop-primary-key, SQLite rebuilds) fail with
/// `InvalidInput` without it.
pub fn plan_ddl(
    dialect: Dialect,
    meta: Option<&TableMeta>,
    ops: &[DdlOp],
) -> Result<DdlPlan, AppError> {
    plan_ddl_with(dialect, meta, ops, &PlanContext::default())
}

/// `plan_ddl` with what the server reported beyond `meta` (see `PlanContext`).
pub fn plan_ddl_with(
    dialect: Dialect,
    meta: Option<&TableMeta>,
    ops: &[DdlOp],
    context: &PlanContext,
) -> Result<DdlPlan, AppError> {
    let mut planner = Planner::with_mode(dialect, meta, context.mysql_mode);
    if let Some(m) = planner.model.as_mut() {
        m.constraint_indexes = context.constraint_indexes.clone();
        m.owned_sequences = context.owned_sequences.clone();
    }
    let mut i = 0;
    while i < ops.len() {
        if planner.replace_primary_key(&ops[i], ops.get(i + 1))?
            || planner.replace_index_or_check(&ops[i], ops.get(i + 1))?
        {
            i += 2;
            continue;
        }
        planner.op(&ops[i], &ops[i + 1..])?;
        i += 1;
    }
    planner.finish()
}

/// Reconstructed `CREATE TABLE` script for a table (informational: PG has no `SHOW CREATE`):
/// the table with identity/defaults/primary key, then its indexes, foreign keys, triggers and
/// comments, one statement per line.
pub fn create_table_sql(dialect: Dialect, meta: &TableMeta) -> String {
    create_table_sql_with(dialect, meta, &PgTableContext::default())
}

/// `create_table_sql` with what the server reported beyond `meta` (PG: sequences, partitioning,
/// cloned constraints; see `PgTableContext`).
pub fn create_table_sql_with(
    dialect: Dialect,
    meta: &TableMeta,
    context: &PgTableContext,
) -> String {
    table_parts(dialect, meta, context).script()
}

/// The statements that recreate a table, in the parts an export script orders separately.
pub fn table_parts(dialect: Dialect, meta: &TableMeta, context: &PgTableContext) -> TableParts {
    Planner::new(dialect, Some(meta)).render_parts(context)
}

/// `DROP TABLE` / `DROP VIEW` / `DROP MATERIALIZED VIEW` for a relation of the given kind.
pub fn drop_statement_for(dialect: Dialect, table: &TableRef, kind: TableKind) -> String {
    let what = match (dialect, kind) {
        (Dialect::Postgres, TableKind::MaterializedView) => "MATERIALIZED VIEW",
        (Dialect::Postgres, TableKind::ForeignTable) => "FOREIGN TABLE",
        (_, TableKind::View | TableKind::MaterializedView | TableKind::SystemView) => "VIEW",
        _ => "TABLE",
    };
    format!(
        "DROP {what} {}",
        dialect.qualify(table.schema.as_deref(), &table.name)
    )
}

// ---------------------------------------------------------------------------------------------
// PostgreSQL: what a script that recreates a table needs beyond its `TableMeta`
// ---------------------------------------------------------------------------------------------

/// A sequence a column default draws from (`nextval('…'::regclass)`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PgSequence {
    pub column: String,
    pub schema: String,
    pub name: String,
    /// Owned by the column (`serial`, `OWNED BY`): it goes when the table is dropped.
    pub owned: bool,
    /// `integer`, `bigint`, `smallint`; the options are `None` when the catalog was unreadable.
    pub data_type: Option<String>,
    pub start: Option<i64>,
    pub increment: Option<i64>,
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub cache: Option<i64>,
    pub cycle: bool,
}

/// A partition below a partitioned table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgPartition {
    pub table: TableRef,
    pub parent: TableRef,
    /// `FOR VALUES …` or `DEFAULT`.
    pub bound: String,
    /// The partition is partitioned itself: its key (`LIST (region)`).
    pub key: Option<String>,
    /// The indexes and triggers the partition has of its own (not the copies PostgreSQL keeps of
    /// its parent's), in catalog order (`pg_context_partition_objects_sql`).
    pub objects: Vec<PgPartitionObject>,
}

/// What an object of a partition is (`PgPartitionObject`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PgPartitionObjectKind {
    /// An index (`pg_get_indexdef`).
    Index,
    /// The index of a PRIMARY KEY / UNIQUE / EXCLUDE constraint (`pg_get_constraintdef`).
    Constraint,
    /// A trigger (`pg_get_triggerdef`).
    Trigger,
}

/// An index or trigger a partition has of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgPartitionObject {
    pub kind: PgPartitionObjectKind,
    /// Index, constraint or trigger name.
    pub name: String,
    /// The catalog's definition of it (see `PgPartitionObjectKind`).
    pub definition: String,
    /// A trigger that is disabled.
    pub disabled: bool,
}

/// What the PostgreSQL catalog says about a table beyond its `TableMeta`, for a script that
/// recreates it (`pg_context_*_sql`). Empty elsewhere.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PgTableContext {
    /// The sequences column defaults draw from: a fresh database needs them before the table.
    pub sequences: Vec<PgSequence>,
    /// A partitioned table: its key (`RANGE (ts)`).
    pub partition_key: Option<String>,
    /// A partition: its bound (`FOR VALUES …` / `DEFAULT`); its parent is `ancestors[0]`.
    pub partition_bound: Option<String>,
    /// A partition: the partitioned tables above it, nearest first.
    pub ancestors: Vec<TableRef>,
    /// A partitioned table: the partitions below it, parents before their own partitions
    /// (only when asked for).
    pub partitions: Vec<PgPartition>,
    /// Foreign keys PostgreSQL keeps as clones of another constraint (`conparentid`): those of a
    /// partition inherited from its parent, and the per-partition copies of a key that references
    /// a partitioned table. Recreating the original recreates them.
    pub inherited_fks: Vec<String>,
    /// Triggers of a partition that are clones of its parent's.
    pub cloned_triggers: Vec<String>,
}

/// `sql` with `{schema}` naming the table's schema (`$2`, or the current schema when the
/// reference has none); `$1` is the table's name.
fn pg_catalog_query(table: &TableRef, sql: &str) -> BuiltSql {
    let mut params = vec![Value::Text(table.name.clone())];
    let schema = match table.schema.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => {
            params.push(Value::Text(s.to_string()));
            "$2"
        }
        None => "current_schema()",
    };
    BuiltSql {
        sql: sql.replace("{schema}", schema),
        params,
    }
}

/// `relkind`, `relispartition`, the partition key of a partitioned table and the bound of a
/// partition (one row, text columns).
pub fn pg_context_relation_sql(table: &TableRef) -> BuiltSql {
    pg_catalog_query(
        table,
        "SELECT c.relkind::text, c.relispartition::text, \
                CASE WHEN c.relkind = 'p' THEN pg_catalog.pg_get_partkeydef(c.oid) END, \
                CASE WHEN c.relispartition THEN pg_catalog.pg_get_expr(c.relpartbound, c.oid) END \
         FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.relname = $1 AND n.nspname = {schema}",
    )
}

/// The partitioned tables above a partition, nearest first: schema, name.
pub fn pg_context_ancestors_sql(table: &TableRef) -> BuiltSql {
    pg_catalog_query(
        table,
        "WITH RECURSIVE up(oid, depth) AS ( \
           SELECT i.inhparent, 1 FROM pg_catalog.pg_inherits i \
             JOIN pg_catalog.pg_class c ON c.oid = i.inhrelid \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
            WHERE c.relname = $1 AND n.nspname = {schema} AND c.relispartition \
           UNION ALL \
           SELECT i.inhparent, up.depth + 1 FROM up \
             JOIN pg_catalog.pg_class c ON c.oid = up.oid AND c.relispartition \
             JOIN pg_catalog.pg_inherits i ON i.inhrelid = up.oid) \
         SELECT n.nspname::text, c.relname::text FROM up \
           JOIN pg_catalog.pg_class c ON c.oid = up.oid \
           JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         ORDER BY up.depth",
    )
}

/// The partitions below a partitioned table, parents first: schema, name, parent schema, parent
/// name, bound, key (when partitioned itself). Foreign-table partitions are left out.
pub fn pg_context_partitions_sql(table: &TableRef) -> BuiltSql {
    pg_catalog_query(
        table,
        "WITH RECURSIVE down(oid, parent, depth) AS ( \
           SELECT i.inhrelid, i.inhparent, 1 FROM pg_catalog.pg_inherits i \
             JOIN pg_catalog.pg_class c ON c.oid = i.inhparent \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
            WHERE c.relname = $1 AND n.nspname = {schema} AND c.relkind = 'p' \
           UNION ALL \
           SELECT i.inhrelid, i.inhparent, down.depth + 1 FROM down \
             JOIN pg_catalog.pg_inherits i ON i.inhparent = down.oid) \
         SELECT cn.nspname::text, c.relname::text, pn.nspname::text, p.relname::text, \
                pg_catalog.pg_get_expr(c.relpartbound, c.oid), \
                CASE WHEN c.relkind = 'p' THEN pg_catalog.pg_get_partkeydef(c.oid) END \
         FROM down \
           JOIN pg_catalog.pg_class c ON c.oid = down.oid \
           JOIN pg_catalog.pg_namespace cn ON cn.oid = c.relnamespace \
           JOIN pg_catalog.pg_class p ON p.oid = down.parent \
           JOIN pg_catalog.pg_namespace pn ON pn.oid = p.relnamespace \
         WHERE c.relispartition AND c.relkind IN ('r', 'p') \
         ORDER BY down.depth, cn.nspname, c.relname",
    )
}

/// The sequences the table's column defaults draw from: column, sequence schema, sequence name,
/// type, start, increment, min, max, cache, cycle, owned by the column (all text).
pub fn pg_context_sequences_sql(table: &TableRef) -> BuiltSql {
    pg_catalog_query(
        table,
        "SELECT a.attname::text, sn.nspname::text, s.relname::text, \
                pg_catalog.format_type(q.seqtypid, NULL), q.seqstart::text, q.seqincrement::text, \
                q.seqmin::text, q.seqmax::text, q.seqcache::text, q.seqcycle::text, \
                EXISTS (SELECT 1 FROM pg_catalog.pg_depend o \
                         WHERE o.classid = 'pg_catalog.pg_class'::regclass AND o.objid = s.oid \
                           AND o.refclassid = 'pg_catalog.pg_class'::regclass \
                           AND o.refobjid = c.oid AND o.refobjsubid = a.attnum \
                           AND o.deptype = 'a')::text \
         FROM pg_catalog.pg_class c \
           JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
           JOIN pg_catalog.pg_attrdef d ON d.adrelid = c.oid \
           JOIN pg_catalog.pg_attribute a \
             ON a.attrelid = c.oid AND a.attnum = d.adnum AND NOT a.attisdropped \
           JOIN pg_catalog.pg_depend x \
             ON x.classid = 'pg_catalog.pg_attrdef'::regclass AND x.objid = d.oid \
            AND x.refclassid = 'pg_catalog.pg_class'::regclass \
           JOIN pg_catalog.pg_class s ON s.oid = x.refobjid AND s.relkind = 'S' \
           JOIN pg_catalog.pg_namespace sn ON sn.oid = s.relnamespace \
           LEFT JOIN pg_catalog.pg_sequence q ON q.seqrelid = s.oid \
         WHERE c.relname = $1 AND n.nspname = {schema} \
         ORDER BY a.attnum, sn.nspname, s.relname",
    )
}

/// Constraints and triggers PostgreSQL maintains as clones: `f` + the name of a cloned foreign
/// key, `t` + the name of a trigger a partition took from its parent.
pub fn pg_context_clones_sql(table: &TableRef) -> BuiltSql {
    pg_catalog_query(
        table,
        "SELECT 'f', con.conname::text FROM pg_catalog.pg_constraint con \
           JOIN pg_catalog.pg_class c ON c.oid = con.conrelid \
           JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
          WHERE con.contype = 'f' AND con.conparentid <> 0 \
            AND c.relname = $1 AND n.nspname = {schema} \
         UNION ALL \
         SELECT 't', t.tgname::text FROM pg_catalog.pg_trigger t \
           JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid \
           JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
          WHERE NOT t.tgisinternal AND c.relispartition \
            AND c.relname = $1 AND n.nspname = {schema} \
            AND EXISTS (SELECT 1 FROM pg_catalog.pg_inherits i \
                          JOIN pg_catalog.pg_trigger p \
                            ON p.tgrelid = i.inhparent AND p.tgname = t.tgname \
                         WHERE i.inhrelid = t.tgrelid)",
    )
}

/// The indexes and triggers the partitions below a partitioned table have of their own, parents
/// first: partition schema, partition name, kind (`i` index, `c` constraint, `t` trigger), name,
/// definition, disabled (text). Indexes that are partitions of a partitioned index (PostgreSQL
/// creates them with the partition) and triggers cloned from a parent are left out.
pub fn pg_context_partition_objects_sql(table: &TableRef) -> BuiltSql {
    pg_catalog_query(
        table,
        "WITH RECURSIVE down(oid, depth) AS ( \
           SELECT i.inhrelid, 1 FROM pg_catalog.pg_inherits i \
             JOIN pg_catalog.pg_class c ON c.oid = i.inhparent \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
            WHERE c.relname = $1 AND n.nspname = {schema} AND c.relkind = 'p' \
           UNION ALL \
           SELECT i.inhrelid, down.depth + 1 FROM down \
             JOIN pg_catalog.pg_inherits i ON i.inhparent = down.oid) \
         SELECT cn.nspname::text, c.relname::text, o.kind, o.name, o.definition, o.disabled::text \
         FROM down \
           JOIN pg_catalog.pg_class c \
             ON c.oid = down.oid AND c.relispartition AND c.relkind IN ('r', 'p') \
           JOIN pg_catalog.pg_namespace cn ON cn.oid = c.relnamespace \
           CROSS JOIN LATERAL ( \
             SELECT CASE WHEN con.oid IS NULL THEN 'i' ELSE 'c' END AS kind, \
                    COALESCE(con.conname, x.relname)::text AS name, \
                    CASE WHEN con.oid IS NULL THEN pg_catalog.pg_get_indexdef(ix.indexrelid) \
                         ELSE pg_catalog.pg_get_constraintdef(con.oid) END AS definition, \
                    false AS disabled \
               FROM pg_catalog.pg_index ix \
               JOIN pg_catalog.pg_class x ON x.oid = ix.indexrelid \
               LEFT JOIN pg_catalog.pg_constraint con \
                 ON con.conindid = ix.indexrelid AND con.conrelid = c.oid \
                AND con.contype IN ('p', 'u', 'x') \
              WHERE ix.indrelid = c.oid \
                AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_inherits h \
                                 WHERE h.inhrelid = ix.indexrelid) \
             UNION ALL \
             SELECT 't', t.tgname::text, pg_catalog.pg_get_triggerdef(t.oid), t.tgenabled = 'D' \
               FROM pg_catalog.pg_trigger t \
              WHERE t.tgrelid = c.oid AND NOT t.tgisinternal \
                AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_inherits h \
                                  JOIN pg_catalog.pg_trigger p \
                                    ON p.tgrelid = h.inhparent AND p.tgname = t.tgname \
                                 WHERE h.inhrelid = t.tgrelid)) o \
         ORDER BY down.depth, cn.nspname, c.relname, o.kind, o.name",
    )
}

/// Sequences owned by a column of the table that are not its identity sequence: column,
/// sequence schema, sequence name (see `PlanContext::owned_sequences`).
pub fn pg_owned_sequences_sql(table: &TableRef) -> BuiltSql {
    pg_catalog_query(
        table,
        "SELECT a.attname::text, sn.nspname::text, s.relname::text \
         FROM pg_catalog.pg_depend o \
           JOIN pg_catalog.pg_class s ON s.oid = o.objid AND s.relkind = 'S' \
           JOIN pg_catalog.pg_namespace sn ON sn.oid = s.relnamespace \
           JOIN pg_catalog.pg_class c ON c.oid = o.refobjid \
           JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
           JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid AND a.attnum = o.refobjsubid \
         WHERE o.classid = 'pg_catalog.pg_class'::regclass \
           AND o.refclassid = 'pg_catalog.pg_class'::regclass AND o.deptype = 'a' \
           AND c.relname = $1 AND n.nspname = {schema} \
         ORDER BY a.attnum, sn.nspname, s.relname",
    )
}

/// `CREATE TABLE … PARTITION OF …` for a partition an export recreates along with its parent.
pub fn partition_of_sql(partition: &PgPartition) -> String {
    let d = Dialect::Postgres;
    let key = partition
        .key
        .as_deref()
        .map(|k| format!(" PARTITION BY {k}"))
        .unwrap_or_default();
    format!(
        "CREATE TABLE {} PARTITION OF {} {}{key};",
        d.qualify(partition.table.schema.as_deref(), &partition.table.name),
        d.qualify(partition.parent.schema.as_deref(), &partition.parent.name),
        partition.bound.trim()
    )
}

/// The statements of the indexes and triggers a partition has of its own (`PgPartition::objects`):
/// `(indexes, triggers)`, every one terminated. A script runs the indexes once the partition and
/// its own partitions exist (an index of a partitioned partition then reaches them: `ON ONLY`
/// is dropped), and the triggers after the rows. A disabled trigger is disabled again.
pub fn partition_objects_sql(partition: &PgPartition) -> (Vec<String>, Vec<String>) {
    let d = Dialect::Postgres;
    let qt = d.qualify(partition.table.schema.as_deref(), &partition.table.name);
    let (mut indexes, mut triggers) = (Vec::new(), Vec::new());
    for o in &partition.objects {
        match o.kind {
            PgPartitionObjectKind::Index => {
                indexes.push(terminated(&without_only(&o.definition)));
            }
            PgPartitionObjectKind::Constraint => indexes.push(format!(
                "ALTER TABLE {qt} ADD CONSTRAINT {} {};",
                d.quote_ident(&o.name),
                o.definition.trim().trim_end_matches(';')
            )),
            PgPartitionObjectKind::Trigger => {
                triggers.push(terminated(&o.definition));
                if o.disabled {
                    triggers.push(format!(
                        "{};",
                        trigger_state_sql(&partition.table, &o.name, false)
                    ));
                }
            }
        }
    }
    (indexes, triggers)
}

/// A `pg_get_indexdef` text without the `ONLY` of `… ON ONLY <table>` (outside quoted names):
/// the index of a partitioned table then also reaches the partitions it has.
fn without_only(def: &str) -> String {
    const ON_ONLY: &str = " ON ONLY ";
    let mut quoted = false;
    for (i, ch) in def.char_indices() {
        if ch == '"' {
            quoted = !quoted;
        } else if !quoted
            && def[i..]
                .get(..ON_ONLY.len())
                .is_some_and(|s| s.eq_ignore_ascii_case(ON_ONLY))
        {
            return format!("{} ON {}", &def[..i], &def[i + ON_ONLY.len()..]);
        }
    }
    def.to_string()
}

/// PG, after the rows of a script: moves a sequence that several of the script's tables draw
/// from past the rows of all of them, in one `setval` (`TableParts::sequence_resets` does this
/// for one table): the largest value of `columns` (the smallest for a descending sequence), never
/// beyond the sequence's MINVALUE (MAXVALUE). Nothing changes when every column is empty.
pub fn shared_sequence_reset_sql(sequence: &PgSequence, columns: &[(TableRef, String)]) -> String {
    let d = Dialect::Postgres;
    let increment = sequence.increment.unwrap_or(1);
    let (min, max) = sequence_bounds(sequence);
    let (agg, clamp, limit) = if increment < 0 {
        ("MIN", "LEAST", max)
    } else {
        ("MAX", "GREATEST", min)
    };
    let values = columns
        .iter()
        .map(|(t, c)| {
            format!(
                "(SELECT {agg}({}) FROM {})",
                d.quote_ident(c),
                d.qualify(t.schema.as_deref(), &t.name)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "SELECT setval({}, {clamp}(v, {limit})) FROM (SELECT {clamp}({values}) AS v) AS s WHERE v IS NOT NULL;",
        d.quote_literal(&d.qualify(Some(&sequence.schema), &sequence.name))
    )
}

/// PG `ALTER TABLE <table> ENABLE | DISABLE TRIGGER <name>` (unterminated).
pub fn trigger_state_sql(table: &TableRef, name: &str, enabled: bool) -> String {
    let d = Dialect::Postgres;
    format!(
        "ALTER TABLE {} {} TRIGGER {}",
        d.qualify(table.schema.as_deref(), &table.name),
        if enabled { "ENABLE" } else { "DISABLE" },
        d.quote_ident(name)
    )
}

/// The statements that recreate a table (`table_parts`), every one terminated. A script loads
/// them in this order: `sequences`, `table`, `indexes`, `attach`, `comments`, then the rows,
/// then `sequence_resets`, `foreign_keys` and `triggers` (as pg_dump does: keys that rows of a
/// later table satisfy, and triggers that would fire on the loaded rows, come last).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableParts {
    /// PG: `CREATE SEQUENCE IF NOT EXISTS` for the sequences the column defaults draw from.
    pub sequences: Vec<String>,
    /// `CREATE TABLE` (PG: `PARTITION BY …`), then PG `ALTER SEQUENCE … OWNED BY …`.
    pub table: Vec<String>,
    pub indexes: Vec<String>,
    /// PG partition: `ALTER TABLE <parent> ATTACH PARTITION …`. It comes after the partition's
    /// indexes, which the attach then adopts instead of creating its own.
    pub attach: Option<String>,
    pub foreign_keys: Vec<String>,
    pub triggers: Vec<String>,
    pub comments: Vec<String>,
    /// PG, after the rows: moves the sequence of each serial / identity column past them.
    pub sequence_resets: Vec<String>,
    /// The column each of `sequence_resets` is for (same order).
    pub reset_columns: Vec<String>,
}

impl TableParts {
    /// The parts that come before the rows, as sections (lines joined), in load order;
    /// `partitions` (`partition_of_sql`) go after the indexes and before the attach, and the
    /// partitions' own indexes (`partition_objects_sql`) after the attach.
    pub fn create_sections(
        &self,
        partitions: &[String],
        partition_indexes: &[String],
    ) -> Vec<String> {
        let attach: Vec<String> = self.attach.iter().cloned().collect();
        [
            &self.sequences[..],
            &self.table,
            &self.indexes,
            partitions,
            &attach,
            partition_indexes,
            &self.comments,
        ]
        .into_iter()
        .filter(|section| !section.is_empty())
        .map(|section| section.join("\n"))
        .collect()
    }

    /// The whole script ("Show DDL"): table, indexes, foreign keys, triggers and comments.
    pub fn script(&self) -> String {
        let attach: Vec<String> = self.attach.iter().cloned().collect();
        [
            &self.sequences[..],
            &self.table,
            &self.indexes,
            &attach,
            &self.foreign_keys,
            &self.triggers,
            &self.comments,
        ]
        .into_iter()
        .filter(|section| !section.is_empty())
        .map(|section| section.join("\n"))
        .collect::<Vec<_>>()
        .join("\n\n")
    }
}

// ---------------------------------------------------------------------------------------------
// Working model
// ---------------------------------------------------------------------------------------------

/// A column as the planner tracks it.
#[derive(Debug, Clone)]
struct Col {
    name: String,
    /// Physical column holding this column's data, used as the copy source of a SQLite rebuild;
    /// `None` for a column added inside a pending rebuild.
    source: Option<String>,
    data_type: String,
    nullable: bool,
    /// SQL expression (literals already quoted).
    default_expr: Option<String>,
    /// PG identity / MySQL `AUTO_INCREMENT` / SQLite `AUTOINCREMENT` keyword.
    auto_increment: bool,
    unsigned: bool,
    comment: Option<String>,
    collation: Option<String>,
    /// Generated column: PG keeps the expression in `default_expr`, SQLite in `verbatim`.
    generated: bool,
    /// PG identity flavour (`ALWAYS` / `BY DEFAULT`).
    identity: Option<String>,
    /// MySQL `ON UPDATE …` expression.
    on_update: Option<String>,
    /// MySQL `INVISIBLE`.
    invisible: bool,
    /// SQLite: the original definition text (generated columns are carried over verbatim).
    verbatim: Option<String>,
    /// SQLite: `nullable` is false only because the column is part of the primary key (no
    /// `NOT NULL` was declared); dropping the key makes it nullable again.
    implicit_not_null: bool,
}

impl Col {
    fn from_meta(dialect: Dialect, c: &ColumnMeta, mode: MysqlLiteralMode) -> Col {
        let mut col = Col {
            name: c.name.clone(),
            source: Some(c.name.clone()),
            data_type: c.data_type.clone(),
            nullable: c.nullable,
            default_expr: c.default_expr.clone(),
            auto_increment: false,
            unsigned: c.is_unsigned,
            comment: c.comment.clone().filter(|s| !s.is_empty()),
            collation: c.collation.clone(),
            generated: c.is_generated,
            identity: None,
            on_update: None,
            invisible: false,
            verbatim: None,
            implicit_not_null: false,
        };
        let extra = c.extra.as_deref().unwrap_or("");
        match dialect {
            Dialect::Postgres => {
                if !c.is_generated && matches!(extra, "ALWAYS" | "BY DEFAULT") {
                    col.identity = Some(extra.to_string());
                    col.auto_increment = true;
                }
            }
            Dialect::Mysql => {
                col.generated = mysql_is_generated(c);
                col.auto_increment = c.is_auto_increment;
                col.default_expr = mysql_default_from_meta(c, mode);
                col.on_update = MYSQL_ON_UPDATE.captures(extra).map(|m| m[1].to_string());
                col.invisible = extra.to_ascii_lowercase().contains("invisible");
            }
            Dialect::Sqlite => {
                // A plain `INTEGER PRIMARY KEY` stays a rowid alias through the table-level
                // PRIMARY KEY clause; only the AUTOINCREMENT keyword needs carrying over.
                col.auto_increment = c.is_auto_increment && extra == "AUTOINCREMENT";
            }
        }
        col
    }
}

/// Builds a column from a designer definition, with warnings for what the dialect ignores.
fn col_from_def(dialect: Dialect, def: &ColumnDef) -> (Col, Vec<String>) {
    let mut warnings = Vec::new();
    let mut col = Col {
        name: def.name.clone(),
        source: None,
        data_type: def.data_type.trim().to_string(),
        nullable: def.nullable,
        default_expr: non_empty(def.default_expr.as_deref()),
        auto_increment: def.auto_increment,
        unsigned: def.unsigned,
        comment: def.comment.clone().filter(|s| !s.is_empty()),
        collation: non_empty(def.collation.as_deref()),
        generated: false,
        identity: None,
        on_update: None,
        invisible: false,
        verbatim: None,
        implicit_not_null: false,
    };
    let name = &def.name;
    if let Some(e) = non_empty(def.on_update.as_deref()) {
        if dialect == Dialect::Mysql {
            col.on_update = Some(e);
        } else {
            warnings.push(format!(
                "ON UPDATE is MySQL-only; ignored for column \"{name}\""
            ));
        }
    }
    if col.unsigned && dialect != Dialect::Mysql {
        warnings.push(format!(
            "UNSIGNED is MySQL-only; ignored for column \"{name}\""
        ));
        col.unsigned = false;
    }
    match dialect {
        Dialect::Postgres if col.auto_increment => {
            if is_pg_serial(&col.data_type) {
                // serial types already draw from a sequence
                col.auto_increment = false;
            } else if !is_integer_type(dialect, &col.data_type) {
                warnings.push(format!(
                    "auto-increment needs an integer type; ignored for column \"{name}\""
                ));
                col.auto_increment = false;
            } else {
                col.identity = Some("BY DEFAULT".into());
                if col.default_expr.take().is_some() {
                    warnings.push(format!(
                        "an identity column cannot have a default; the default of \"{name}\" is ignored"
                    ));
                }
            }
        }
        Dialect::Sqlite if col.comment.is_some() => {
            warnings.push(SQLITE_NO_COMMENTS.into());
            col.comment = None;
        }
        _ => {}
    }
    (col, warnings)
}

/// A foreign key as the planner tracks it.
#[derive(Debug, Clone)]
struct Fk {
    /// Name used to address the key (synthetic on SQLite, which does not report names).
    name: String,
    /// Whether `name` is a real constraint name to emit as `CONSTRAINT …`.
    named: bool,
    columns: Vec<String>,
    ref_table: TableRef,
    ref_columns: Vec<String>,
    on_update: Option<String>,
    on_delete: Option<String>,
    /// PG `pg_get_constraintdef` text.
    definition: Option<String>,
}

#[derive(Debug, Clone)]
struct Model {
    table: TableRef,
    columns: Vec<Col>,
    primary_key: Vec<String>,
    /// Constraint name of the primary key (PG `t_pkey`, MySQL `PRIMARY`).
    pk_name: Option<String>,
    indexes: Vec<IndexMeta>,
    fks: Vec<Fk>,
    triggers: Vec<TriggerMeta>,
    comment: Option<String>,
    /// SQLite options after the column list (`WITHOUT ROWID`, `STRICT`).
    table_options: Option<String>,
    dropped: bool,
    /// SQLite: indexes added while a rebuild is pending. Their definitions use the final column
    /// names, so they are created after the rebuild's column renames; every other stored index
    /// and trigger definition uses the physical names of the table as it is in the database.
    fresh_indexes: Vec<String>,
    /// SQLite: triggers whose stored text no longer matches the database because a column they
    /// mention was renamed earlier in the plan (`(trigger, column)`); they cannot be replayed.
    stale_triggers: Vec<(String, String)>,
    /// PG: indexes backing a constraint (`PlanContext::constraint_indexes`).
    constraint_indexes: Vec<String>,
    /// PG: sequences owned by a column besides its identity (`PlanContext::owned_sequences`).
    owned_sequences: Vec<(String, TableRef)>,
    checks: Vec<Check>,
    /// MySQL table options as introspected (the current character set of `CONVERT TO`).
    options: TableOptions,
}

/// A CHECK constraint as the planner tracks it.
#[derive(Debug, Clone)]
struct Check {
    name: String,
    /// Whether `name` is a real constraint name to emit as `CONSTRAINT …` (SQLite does not
    /// report names of unnamed checks; they get a synthetic `ck_<table>_<n>`).
    named: bool,
    expression: String,
    columns: Vec<String>,
    /// SQLite: added while a rebuild is pending; its expression spells the final column names
    /// (the stored ones spell the physical names of the table as it is in the database).
    fresh: bool,
}

impl Model {
    fn from_meta(dialect: Dialect, meta: &TableMeta, mode: MysqlLiteralMode) -> Model {
        let mut columns: Vec<Col> = meta
            .columns
            .iter()
            .map(|c| Col::from_meta(dialect, c, mode))
            .collect();
        let mut table_options = None;
        if dialect == Dialect::Sqlite
            && let Some(ddl) = meta.ddl.as_deref()
        {
            let (defs, tail) = split_create_table(ddl);
            for col in &mut columns {
                let Some((_, def)) = defs.iter().find(|(n, _)| n.eq_ignore_ascii_case(&col.name))
                else {
                    continue;
                };
                if col.generated {
                    col.verbatim = Some(def.clone());
                } else if col.collation.is_none() {
                    // introspection does not report collations; the original text does
                    col.collation = column_collation(def);
                }
                // introspection reports every key column as NOT NULL
                let key = meta
                    .primary_key
                    .iter()
                    .any(|k| k.eq_ignore_ascii_case(&col.name));
                if key && !col.nullable && !NOT_NULL.is_match(&top_level(after_leading_ident(def)))
                {
                    col.implicit_not_null = true;
                }
            }
            table_options = tail;
        }
        if dialect == Dialect::Mysql
            && let Some(ddl) = meta.ddl.as_deref()
        {
            // `SHOW CREATE TABLE` spells every default exactly as the server will accept it back
            // (information_schema loses the difference between 'now()' and now())
            let (defs, _) = split_create_table(ddl);
            for (col, cm) in columns.iter_mut().zip(&meta.columns) {
                if col.generated {
                    continue;
                }
                let def = defs
                    .iter()
                    .find(|(n, d)| d.trim_start().starts_with('`') && n == &col.name);
                let Some(default) = def.and_then(|(_, d)| mysql_default_from_ddl(d)) else {
                    continue;
                };
                // MySQL quotes numeric defaults (`DEFAULT '0'`); keep them bare
                let numeric = matches!(
                    cm.kind,
                    ValueKind::Int | ValueKind::BigInt | ValueKind::Decimal | ValueKind::Float
                );
                col.default_expr = default.map(|v| {
                    let inner = v
                        .strip_prefix('\'')
                        .and_then(|s| s.strip_suffix('\''))
                        .filter(|s| NUMERIC.is_match(s));
                    match inner {
                        Some(n) if numeric => n.to_string(),
                        _ => v,
                    }
                });
            }
        }
        let fks = meta
            .foreign_keys
            .iter()
            .map(|f| Fk {
                name: f.name.clone(),
                named: dialect != Dialect::Sqlite,
                columns: f.columns.clone(),
                ref_table: f.ref_table.clone(),
                ref_columns: f.ref_columns.clone(),
                on_update: non_empty(Some(&f.on_update)),
                on_delete: non_empty(Some(&f.on_delete)),
                definition: f.definition.clone(),
            })
            .collect();
        let checks = match (dialect, meta.ddl.as_deref()) {
            (Dialect::Sqlite, Some(ddl)) => {
                let generated: Vec<&str> = columns
                    .iter()
                    .filter(|c| c.generated)
                    .map(|c| c.name.as_str())
                    .collect();
                let names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
                parse_sqlite_checks(ddl, &meta.table.name)
                    .into_iter()
                    // a generated column carries its own checks in its verbatim definition
                    .filter(|p| {
                        p.column
                            .as_deref()
                            .is_none_or(|c| !generated.iter().any(|g| g.eq_ignore_ascii_case(c)))
                    })
                    .map(|p| Check {
                        columns: parsed_check_columns(&p, &names),
                        named: p.name.is_some(),
                        name: p.name.unwrap_or(p.synthetic),
                        expression: p.expression,
                        fresh: false,
                    })
                    .collect()
            }
            _ => meta
                .checks
                .iter()
                .map(|c| Check {
                    name: c.name.clone(),
                    named: true,
                    expression: c.expression.clone(),
                    columns: c.columns.clone(),
                    fresh: false,
                })
                .collect(),
        };
        Model {
            table: meta.table.clone(),
            columns,
            primary_key: meta.primary_key.clone(),
            pk_name: meta
                .indexes
                .iter()
                .find(|i| i.primary)
                .map(|i| i.name.clone()),
            indexes: meta.indexes.clone(),
            fks,
            triggers: meta.triggers.clone(),
            comment: meta.comment.clone().filter(|s| !s.is_empty()),
            table_options,
            dropped: false,
            fresh_indexes: Vec::new(),
            stale_triggers: Vec::new(),
            constraint_indexes: Vec::new(),
            owned_sequences: Vec::new(),
            checks,
            options: meta.options.clone().unwrap_or_default(),
        }
    }

    fn col(&self, name: &str) -> Option<&Col> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// A column, other than the one called `except`, that a column called `name` would
    /// duplicate (`same_column`).
    fn clash(&self, dialect: Dialect, name: &str, except: Option<&str>) -> Option<&Col> {
        self.columns
            .iter()
            .find(|c| Some(c.name.as_str()) != except && same_column(dialect, &c.name, name))
    }

    fn col_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }

    fn names(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.name.clone()).collect()
    }

    /// Whether `index` uses the column called `name` in the model. Stored definitions spell the
    /// column by its physical name, except for indexes added during the pending rebuild.
    fn index_uses(&self, index: &IndexMeta, name: &str) -> bool {
        if index
            .columns
            .iter()
            .any(|e| e == name || entry_base(e) == name)
            || index.include.iter().any(|c| c == name)
        {
            return true;
        }
        let Some(def) = index.definition.as_deref() else {
            return false;
        };
        if self.fresh_indexes.contains(&index.name) {
            return index_def_references(def, name);
        }
        match self.col(name).map(|c| c.source.as_deref()) {
            Some(Some(physical)) => index_def_references(def, physical),
            // a column added in the pending rebuild is in no stored definition
            Some(None) => false,
            None => index_def_references(def, name),
        }
    }

    /// Whether SQLite's in-place `DROP COLUMN` would refuse this column (key, index, FK).
    fn column_is_constrained(&self, name: &str) -> bool {
        self.primary_key.iter().any(|c| c == name)
            || self.indexes.iter().any(|i| self.index_uses(i, name))
            || self.fks.iter().any(|f| f.columns.iter().any(|c| c == name))
            || self
                .checks
                .iter()
                .any(|k| k.columns.iter().any(|c| c == name))
    }

    /// Clears the primary key; SQLite columns that were NOT NULL only because of it become
    /// nullable again (introspection cannot tell a declared NOT NULL from the key's).
    fn clear_primary_key(&mut self, dialect: Dialect) {
        self.primary_key.clear();
        self.pk_name = None;
        if dialect == Dialect::Sqlite {
            for c in &mut self.columns {
                c.auto_increment = false;
                if c.implicit_not_null {
                    c.nullable = true;
                    c.implicit_not_null = false;
                }
            }
        }
    }

    /// Removes a column and whatever depends on it; returns notes about the dependents.
    fn drop_column(&mut self, name: &str) -> Vec<String> {
        let table = self.table.name.clone();
        let mut notes = Vec::new();
        let uses: Vec<bool> = self
            .indexes
            .iter()
            .map(|i| !i.primary && self.index_uses(i, name))
            .collect();
        self.columns.retain(|c| c.name != name);
        if self.primary_key.iter().any(|c| c == name) {
            self.primary_key.retain(|c| c != name);
            if self.primary_key.is_empty() {
                self.pk_name = None;
                notes.push(format!(
                    "dropping \"{name}\" removes the primary key of \"{table}\""
                ));
            } else {
                notes.push(format!(
                    "dropping \"{name}\" shrinks the primary key of \"{table}\" to ({})",
                    self.primary_key.join(", ")
                ));
            }
        }
        let mut kept = Vec::with_capacity(self.indexes.len());
        for (i, used) in std::mem::take(&mut self.indexes).into_iter().zip(uses) {
            if used {
                self.fresh_indexes.retain(|n| n != &i.name);
                notes.push(format!(
                    "index \"{}\" uses \"{name}\" and is dropped with it",
                    i.name
                ));
            } else {
                kept.push(i);
            }
        }
        self.indexes = kept;
        let (gone, kept): (Vec<Fk>, Vec<Fk>) = self
            .fks
            .drain(..)
            .partition(|f| f.columns.iter().any(|c| c == name));
        self.fks = kept;
        for f in gone {
            notes.push(format!(
                "foreign key \"{}\" uses \"{name}\" and is dropped with it",
                f.name
            ));
        }
        let (gone, kept): (Vec<Check>, Vec<Check>) = self
            .checks
            .drain(..)
            .partition(|k| k.columns.iter().any(|c| c == name));
        self.checks = kept;
        for k in gone {
            notes.push(format!(
                "CHECK constraint \"{}\" uses \"{name}\" and is dropped with it",
                k.name
            ));
        }
        notes
    }

    /// Renames a column everywhere the model refers to it by name. Stored definitions (indexes,
    /// triggers, generated columns) spell physical names, so their text changes only when the
    /// rename runs in the database right away (`physical`). A rename deferred to the end of a
    /// pending SQLite rebuild leaves them alone, except for indexes added during that rebuild:
    /// SQLite rewrites the rest when the deferred `RENAME COLUMN` runs.
    fn rename_column(&mut self, dialect: Dialect, old: &str, new: &str, physical: bool) {
        let quoted = dialect.quote_ident(new);
        let self_ref = self.table.clone();
        for c in &mut self.columns {
            if c.name == old {
                c.name = new.to_string();
                if physical {
                    c.source = Some(new.to_string());
                }
            }
        }
        rename_in(&mut self.primary_key, old, new);
        for i in &mut self.indexes {
            for entry in &mut i.columns {
                *entry = rename_index_entry(entry, old, new);
            }
            rename_in(&mut i.include, old, new);
        }
        for f in &mut self.fks {
            rename_in(&mut f.columns, old, new);
            if same_table(&f.ref_table, &self_ref) {
                rename_in(&mut f.ref_columns, old, new);
            }
        }
        for k in &mut self.checks {
            rename_in(&mut k.columns, old, new);
        }
        if physical {
            self.renamed_in_database(dialect, old, new);
        } else {
            for i in &mut self.indexes {
                if self.fresh_indexes.contains(&i.name)
                    && let Some(def) = &i.definition
                {
                    i.definition = Some(rename_in_index_def(def, old, &quoted));
                }
            }
            for k in self.checks.iter_mut().filter(|k| k.fresh) {
                k.expression = rename_ident(&k.expression, old, &quoted);
            }
        }
    }

    /// Follows a `RENAME COLUMN old TO new` that ran in the database: rewrites the stored
    /// definitions the way SQLite does. Index and generated-column text only ever names columns
    /// of this table, so a token rename is exact there; trigger bodies can name other tables'
    /// columns, so a trigger mentioning `old` is marked stale instead (SQLite's own rewrite is
    /// in the database, but its text is not known here).
    fn renamed_in_database(&mut self, dialect: Dialect, old: &str, new: &str) {
        let quoted = dialect.quote_ident(new);
        for c in &mut self.columns {
            if let Some(v) = &c.verbatim {
                c.verbatim = Some(rename_ident(v, old, &quoted));
            }
        }
        for i in &mut self.indexes {
            if !self.fresh_indexes.contains(&i.name)
                && let Some(def) = &i.definition
            {
                i.definition = Some(rename_in_index_def(def, old, &quoted));
            }
        }
        for k in self.checks.iter_mut().filter(|k| !k.fresh) {
            k.expression = rename_ident(&k.expression, old, &quoted);
        }
        if dialect == Dialect::Sqlite {
            for t in &self.triggers {
                if trigger_references(&t.definition, old)
                    && !self.stale_triggers.iter().any(|(n, _)| n == &t.name)
                {
                    self.stale_triggers.push((t.name.clone(), old.to_string()));
                }
            }
        }
    }

    fn rename_table(&mut self, dialect: Dialect, new_name: &str) {
        let old = std::mem::replace(&mut self.table.name, new_name.to_string());
        let quoted = dialect.quote_ident(new_name);
        for f in &mut self.fks {
            if f.ref_table.name == old {
                f.ref_table.name = new_name.to_string();
            }
        }
        for t in &mut self.triggers {
            t.definition = rename_in_trigger_def(&t.definition, &old, &quoted);
        }
    }
}

fn rename_in(list: &mut [String], old: &str, new: &str) {
    for c in list.iter_mut() {
        if c == old {
            *c = new.to_string();
        }
    }
}

/// The column name an index entry starts with (`a DESC` → `a`).
fn entry_base(entry: &str) -> &str {
    match INDEX_PART.captures(entry.trim()) {
        Some(c) => c.get(1).map_or("", |m| m.as_str()),
        None => entry.trim(),
    }
}

fn rename_index_entry(entry: &str, old: &str, new: &str) -> String {
    if entry == old {
        return new.to_string();
    }
    match INDEX_PART.captures(entry.trim()) {
        Some(c) if &c[1] == old => format!("{new}{}", &entry.trim()[c[1].len()..]),
        _ => entry.to_string(),
    }
}

// ---------------------------------------------------------------------------------------------
// Planner
// ---------------------------------------------------------------------------------------------

const SQLITE_NO_COMMENTS: &str = "SQLite does not store table or column comments; ignored";

struct Planner {
    dialect: Dialect,
    /// MySQL: how the session reads backslashes in string literals (`PlanContext::mysql_mode`).
    mysql_mode: MysqlLiteralMode,
    model: Option<Model>,
    statements: Vec<String>,
    warnings: Vec<String>,
    /// SQLite: ops on the model table are being folded into a rebuild not yet emitted.
    pending: bool,
    /// SQLite: the physical columns of the table when the pending rebuild started.
    rebuild_base: Vec<String>,
    /// SQLite: at least one rebuild was emitted.
    rebuilt: bool,
    /// A raw statement cannot run inside a transaction block (PG `VACUUM`, SQLite `PRAGMA`).
    sequential: bool,
    /// Triggers dropped so far in the plan.
    dropped_triggers: Vec<String>,
}

fn op_table(op: &DdlOp) -> Option<&TableRef> {
    match op {
        DdlOp::CreateTable { table, .. }
        | DdlOp::DropTable { table, .. }
        | DdlOp::TruncateTable { table, .. }
        | DdlOp::RenameTable { table, .. }
        | DdlOp::AddColumn { table, .. }
        | DdlOp::DropColumn { table, .. }
        | DdlOp::RenameColumn { table, .. }
        | DdlOp::AlterColumnType { table, .. }
        | DdlOp::AlterColumnNullable { table, .. }
        | DdlOp::AlterColumnDefault { table, .. }
        | DdlOp::SetColumnComment { table, .. }
        | DdlOp::AlterColumnAutoIncrement { table, .. }
        | DdlOp::AlterColumnOnUpdate { table, .. }
        | DdlOp::AlterColumnCollation { table, .. }
        | DdlOp::AlterTableOptions { table, .. }
        | DdlOp::AddCheck { table, .. }
        | DdlOp::DropCheck { table, .. }
        | DdlOp::CreateTrigger { table, .. }
        | DdlOp::DropTrigger { table, .. }
        | DdlOp::EnableTrigger { table, .. }
        | DdlOp::DisableTrigger { table, .. }
        | DdlOp::SetTableComment { table, .. }
        | DdlOp::AddIndex { table, .. }
        | DdlOp::DropIndex { table, .. }
        | DdlOp::AddForeignKey { table, .. }
        | DdlOp::DropForeignKey { table, .. }
        | DdlOp::AddPrimaryKey { table, .. }
        | DdlOp::DropPrimaryKey { table } => Some(table),
        DdlOp::RawSql { .. } => None,
    }
}

fn op_table_mut(op: &mut DdlOp) -> Option<&mut TableRef> {
    match op {
        DdlOp::CreateTable { table, .. }
        | DdlOp::DropTable { table, .. }
        | DdlOp::TruncateTable { table, .. }
        | DdlOp::RenameTable { table, .. }
        | DdlOp::AddColumn { table, .. }
        | DdlOp::DropColumn { table, .. }
        | DdlOp::RenameColumn { table, .. }
        | DdlOp::AlterColumnType { table, .. }
        | DdlOp::AlterColumnNullable { table, .. }
        | DdlOp::AlterColumnDefault { table, .. }
        | DdlOp::SetColumnComment { table, .. }
        | DdlOp::AlterColumnAutoIncrement { table, .. }
        | DdlOp::AlterColumnOnUpdate { table, .. }
        | DdlOp::AlterColumnCollation { table, .. }
        | DdlOp::AlterTableOptions { table, .. }
        | DdlOp::AddCheck { table, .. }
        | DdlOp::DropCheck { table, .. }
        | DdlOp::CreateTrigger { table, .. }
        | DdlOp::DropTrigger { table, .. }
        | DdlOp::EnableTrigger { table, .. }
        | DdlOp::DisableTrigger { table, .. }
        | DdlOp::SetTableComment { table, .. }
        | DdlOp::AddIndex { table, .. }
        | DdlOp::DropIndex { table, .. }
        | DdlOp::AddForeignKey { table, .. }
        | DdlOp::DropForeignKey { table, .. }
        | DdlOp::AddPrimaryKey { table, .. }
        | DdlOp::DropPrimaryKey { table } => Some(table),
        DdlOp::RawSql { .. } => None,
    }
}

/// Ops that can join a pending SQLite rebuild of their table.
fn foldable(op: &DdlOp) -> bool {
    !matches!(
        op,
        DdlOp::CreateTable { .. }
            | DdlOp::DropTable { .. }
            | DdlOp::TruncateTable { .. }
            | DdlOp::RenameTable { .. }
            | DdlOp::AlterTableOptions { .. }
            | DdlOp::CreateTrigger { .. }
            | DdlOp::DropTrigger { .. }
            | DdlOp::EnableTrigger { .. }
            | DdlOp::DisableTrigger { .. }
            | DdlOp::RawSql { .. }
    )
}

fn same_table(a: &TableRef, b: &TableRef) -> bool {
    let sa = a.schema.as_deref().filter(|s| !s.is_empty());
    let sb = b.schema.as_deref().filter(|s| !s.is_empty());
    a.name == b.name
        && match (sa, sb) {
            (Some(x), Some(y)) => x == y,
            _ => true,
        }
}

/// Whether two column names denote the same column: SQLite and MySQL compare them ignoring
/// ASCII case, PostgreSQL exactly (the generator always quotes identifiers).
fn same_column(dialect: Dialect, a: &str, b: &str) -> bool {
    match dialect {
        Dialect::Postgres => a == b,
        Dialect::Mysql | Dialect::Sqlite => a.eq_ignore_ascii_case(b),
    }
}

fn duplicate_column(name: &str, existing: &str, table: &TableRef) -> AppError {
    if name == existing {
        invalid(format!(
            "column \"{name}\" already exists in \"{}\"",
            table.name
        ))
    } else {
        invalid(format!(
            "column \"{name}\" would duplicate column \"{existing}\" of \"{}\" (column names are case-insensitive)",
            table.name
        ))
    }
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn invalid(msg: impl Into<String>) -> AppError {
    AppError::InvalidInput(msg.into())
}

fn require(value: &str, what: &str) -> Result<(), AppError> {
    if value.trim().is_empty() {
        Err(invalid(format!("{what} must not be empty")))
    } else {
        Ok(())
    }
}

fn unknown_column(name: &str, table: &TableRef) -> AppError {
    invalid(format!(
        "column \"{name}\" does not exist in \"{}\"",
        table.name
    ))
}

/// Validates and normalises a referential action (`set null` → `SET NULL`).
fn fk_action(action: Option<&str>) -> Result<Option<String>, AppError> {
    let Some(a) = action else { return Ok(None) };
    let norm = a
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_uppercase();
    match norm.as_str() {
        "" => Ok(None),
        "CASCADE" | "SET NULL" | "SET DEFAULT" | "RESTRICT" | "NO ACTION" => Ok(Some(norm)),
        _ => Err(invalid(format!("unknown referential action \"{a}\""))),
    }
}

fn validate_column(dialect: Dialect, c: &ColumnDef) -> Result<(), AppError> {
    require(&c.name, "column name")?;
    if dialect != Dialect::Sqlite {
        require(&c.data_type, &format!("type of column \"{}\"", c.name))?;
    }
    if let Some(coll) = c.collation.as_deref().map(str::trim)
        && !coll.is_empty()
        && !COLLATION_NAME.is_match(coll)
    {
        return Err(invalid(format!("invalid collation name \"{coll}\"")));
    }
    Ok(())
}

fn validate(dialect: Dialect, op: &DdlOp) -> Result<(), AppError> {
    if let Some(t) = op_table(op) {
        require(&t.name, "table name")?;
    }
    match op {
        DdlOp::CreateTable {
            columns,
            primary_key,
            ..
        } => {
            if columns.is_empty() {
                return Err(invalid("a table needs at least one column"));
            }
            for (i, c) in columns.iter().enumerate() {
                validate_column(dialect, c)?;
                if columns[..i]
                    .iter()
                    .any(|o| same_column(dialect, &o.name, &c.name))
                {
                    return Err(invalid(format!("duplicate column \"{}\"", c.name)));
                }
            }
            for k in primary_key {
                if !columns.iter().any(|c| &c.name == k) {
                    return Err(invalid(format!(
                        "primary key column \"{k}\" is not defined"
                    )));
                }
            }
        }
        DdlOp::RenameTable { new_name, .. } => require(new_name, "new table name")?,
        DdlOp::AddColumn { column, .. } => {
            validate_column(dialect, column)?;
            if dialect == Dialect::Sqlite && column.auto_increment {
                return Err(invalid(
                    "SQLite cannot add an AUTOINCREMENT column to an existing table",
                ));
            }
        }
        DdlOp::DropColumn { name, .. }
        | DdlOp::AlterColumnNullable { name, .. }
        | DdlOp::AlterColumnDefault { name, .. }
        | DdlOp::SetColumnComment { name, .. }
        | DdlOp::AlterColumnAutoIncrement { name, .. } => require(name, "column name")?,
        DdlOp::AlterColumnOnUpdate {
            name, on_update, ..
        } => {
            require(name, "column name")?;
            if on_update.as_deref().is_some_and(|e| e.contains(';')) {
                return Err(invalid("an ON UPDATE expression must not contain ';'"));
            }
        }
        DdlOp::AlterColumnCollation {
            name, collation, ..
        } => {
            require(name, "column name")?;
            if let Some(c) = non_empty(collation.as_deref())
                && !COLLATION_NAME.is_match(&c)
            {
                return Err(invalid(format!("invalid collation name \"{c}\"")));
            }
        }
        DdlOp::AlterTableOptions { options, .. } => validate_table_options(options)?,
        DdlOp::AddCheck { check, .. } => {
            require(&check.expression, "CHECK expression")?;
            if let Some(n) = &check.name {
                require(n, "CHECK constraint name")?;
            }
        }
        DdlOp::DropCheck { name, .. } => require(name, "CHECK constraint name")?,
        DdlOp::CreateTrigger { name, sql, .. } => {
            require(name, "trigger name")?;
            require(sql, "trigger SQL")?;
        }
        DdlOp::DropTrigger { name, .. }
        | DdlOp::EnableTrigger { name, .. }
        | DdlOp::DisableTrigger { name, .. } => require(name, "trigger name")?,
        DdlOp::RenameColumn { name, new_name, .. } => {
            require(name, "column name")?;
            require(new_name, "new column name")?;
        }
        DdlOp::AlterColumnType { name, new_type, .. } => {
            require(name, "column name")?;
            require(new_type, "new type")?;
        }
        DdlOp::AddIndex { index, .. } => {
            require(&index.name, "index name")?;
            if index.columns.is_empty() {
                return Err(invalid(format!(
                    "index \"{}\" needs at least one column",
                    index.name
                )));
            }
            for c in index.columns.iter().chain(&index.include) {
                require(c, "index column")?;
            }
            if let Some(m) = index.method.as_deref().map(str::trim)
                && !m.is_empty()
                && !INDEX_METHOD.is_match(m)
            {
                return Err(invalid(format!("invalid index method \"{m}\"")));
            }
        }
        DdlOp::DropIndex { name, .. } => require(name, "index name")?,
        DdlOp::DropForeignKey { name, .. } => require(name, "foreign key name")?,
        DdlOp::AddForeignKey { fk, .. } => {
            if fk.columns.is_empty() {
                return Err(invalid("a foreign key needs at least one column"));
            }
            for c in fk.columns.iter().chain(fk.ref_columns.iter()) {
                require(c, "foreign key column")?;
            }
            require(&fk.ref_table.name, "referenced table")?;
            let implicit = dialect == Dialect::Sqlite && fk.ref_columns.is_empty();
            if !implicit && fk.ref_columns.len() != fk.columns.len() {
                return Err(invalid(
                    "a foreign key needs as many referenced columns as columns",
                ));
            }
            fk_action(fk.on_update.as_deref())?;
            fk_action(fk.on_delete.as_deref())?;
        }
        DdlOp::AddPrimaryKey { columns, .. } => {
            if columns.is_empty() {
                return Err(invalid("a primary key needs at least one column"));
            }
            for c in columns {
                require(c, "primary key column")?;
            }
        }
        DdlOp::RawSql { sql } => require(sql, "SQL")?,
        DdlOp::DropTable { .. } | DdlOp::TruncateTable { .. } | DdlOp::SetTableComment { .. } => {}
        DdlOp::DropPrimaryKey { .. } => {}
    }
    Ok(())
}

/// Engine, character set, collation and row format names: one word.
static OPTION_NAME: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z][A-Za-z0-9_]*$"));

fn validate_table_options(o: &TableOptions) -> Result<(), AppError> {
    let fields = [
        ("engine", &o.engine),
        ("character set", &o.charset),
        ("collation", &o.collation),
        ("row format", &o.row_format),
    ];
    for (what, value) in fields {
        if let Some(v) = value
            && !OPTION_NAME.is_match(v.trim())
        {
            return Err(invalid(format!("invalid {what} \"{v}\"")));
        }
    }
    if *o == TableOptions::default() {
        return Err(invalid("no table option to change"));
    }
    Ok(())
}

/// The character set a MySQL collation belongs to (`utf8mb4_0900_ai_ci` → `utf8mb4`).
fn charset_of_collation(collation: &str) -> String {
    collation.split('_').next().unwrap_or(collation).to_string()
}

/// A column after `op` (type / nullable / default / comment change) is applied to it.
fn changed_col(dialect: Dialect, col: &Col, op: &DdlOp) -> Col {
    let mut c = col.clone();
    match op {
        DdlOp::AlterColumnType { new_type, .. } => {
            let new_type = new_type.trim();
            let lower = new_type.to_ascii_lowercase();
            match dialect {
                Dialect::Mysql => {
                    if lower.contains("signed") {
                        c.unsigned = lower.contains("unsigned");
                    } else if !is_mysql_numeric(&lower) {
                        c.unsigned = false;
                    }
                    // The old collation only carries over to a text type that does not name its
                    // own character set or collation (it would conflict with that one).
                    if !is_mysql_textual(&lower) || mysql_type_sets_charset(new_type) {
                        c.collation = None;
                    }
                }
                Dialect::Sqlite => {
                    if !new_type.eq_ignore_ascii_case("INTEGER") {
                        c.auto_increment = false;
                    }
                }
                Dialect::Postgres => {}
            }
            c.data_type = new_type.to_string();
        }
        DdlOp::AlterColumnNullable { nullable, .. } => {
            c.nullable = *nullable;
            c.implicit_not_null = false;
        }
        DdlOp::AlterColumnDefault { default_expr, .. } => {
            c.default_expr = non_empty(default_expr.as_deref());
            if dialect == Dialect::Mysql {
                c.default_expr = c.default_expr.as_deref().map(mysql_default_sql);
            }
        }
        DdlOp::SetColumnComment { comment, .. } => {
            c.comment = comment.clone().filter(|s| !s.is_empty());
        }
        DdlOp::AlterColumnAutoIncrement { auto_increment, .. } => {
            c.auto_increment = *auto_increment;
            // (MySQL refuses a DEFAULT on an AUTO_INCREMENT column)
            if dialect == Dialect::Mysql && *auto_increment {
                c.default_expr = None;
            }
            if dialect == Dialect::Postgres {
                if *auto_increment {
                    c.identity = c.identity.clone().or(Some("BY DEFAULT".into()));
                    c.default_expr = None;
                    c.nullable = false;
                } else {
                    c.identity = None;
                    if c.default_expr.as_deref().is_some_and(is_nextval) {
                        c.default_expr = None;
                    }
                }
            }
        }
        DdlOp::AlterColumnOnUpdate { on_update, .. } => {
            c.on_update = non_empty(on_update.as_deref());
        }
        DdlOp::AlterColumnCollation { collation, .. } => {
            c.collation = non_empty(collation.as_deref());
        }
        _ => {}
    }
    c
}

/// A PG sequence default (`nextval('t_id_seq'::regclass)`).
fn is_nextval(expr: &str) -> bool {
    expr.trim_start()
        .to_ascii_lowercase()
        .starts_with("nextval(")
}

impl Planner {
    fn new(dialect: Dialect, meta: Option<&TableMeta>) -> Planner {
        Planner::with_mode(dialect, meta, MysqlLiteralMode::default())
    }

    fn with_mode(
        dialect: Dialect,
        meta: Option<&TableMeta>,
        mysql_mode: MysqlLiteralMode,
    ) -> Planner {
        Planner {
            dialect,
            mysql_mode,
            model: meta.map(|m| Model::from_meta(dialect, m, mysql_mode)),
            statements: Vec::new(),
            warnings: Vec::new(),
            pending: false,
            rebuild_base: Vec::new(),
            rebuilt: false,
            sequential: false,
            dropped_triggers: Vec::new(),
        }
    }

    fn q(&self, ident: &str) -> String {
        self.dialect.quote_ident(ident)
    }

    fn qt(&self, t: &TableRef) -> String {
        self.dialect.qualify(t.schema.as_deref(), &t.name)
    }

    fn cols(&self, names: &[String]) -> String {
        names
            .iter()
            .map(|n| self.q(n))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// A string literal in a statement that runs: MySQL spells it for the session's
    /// `sql_mode` (backslashes doubled only where they are escapes).
    fn lit(&self, s: &str) -> String {
        match self.dialect {
            Dialect::Mysql => self.dialect.quote_literal_with(s, self.mysql_mode),
            Dialect::Postgres | Dialect::Sqlite => self.dialect.quote_literal(s),
        }
    }

    fn warn(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        if !self.warnings.contains(&msg) {
            self.warnings.push(msg);
        }
    }

    fn push(&mut self, sql: String) {
        self.statements.push(sql);
    }

    fn is_model(&self, t: &TableRef) -> bool {
        self.model
            .as_ref()
            .is_some_and(|m| !m.dropped && same_table(&m.table, t))
    }

    fn need_meta(&self, t: &TableRef, what: &str) -> AppError {
        invalid(format!(
            "{what} on \"{}\" needs the table's metadata (pass the table to plan against)",
            t.name
        ))
    }

    fn model_of(&self, t: &TableRef, what: &str) -> Result<&Model, AppError> {
        match &self.model {
            Some(m) if !m.dropped && same_table(&m.table, t) => Ok(m),
            _ => Err(self.need_meta(t, what)),
        }
    }

    fn model_col(&self, t: &TableRef, name: &str, what: &str) -> Result<&Col, AppError> {
        let m = self.model_of(t, what)?;
        m.col(name).ok_or_else(|| unknown_column(name, &m.table))
    }

    /// Ops naming the designed table without a schema address it through its real schema.
    fn normalize(&self, op: &DdlOp) -> DdlOp {
        let mut op = op.clone();
        if let Some(m) = &self.model
            && let Some(t) = op_table_mut(&mut op)
            && t.schema.as_deref().is_none_or(str::is_empty)
            && same_table(&m.table, t)
        {
            t.schema = m.table.schema.clone();
        }
        op
    }

    /// MySQL: a primary-key drop directly followed by a primary-key add on the same table
    /// becomes one `ALTER TABLE … DROP PRIMARY KEY, ADD PRIMARY KEY (…)`. On its own the drop
    /// fails whenever an AUTO_INCREMENT column depends on the key (error 1075), which is the
    /// usual shape of a MySQL table. Returns whether both ops were handled.
    fn replace_primary_key(
        &mut self,
        first: &DdlOp,
        next: Option<&DdlOp>,
    ) -> Result<bool, AppError> {
        let Some(next) = next.filter(|_| self.dialect == Dialect::Mysql) else {
            return Ok(false);
        };
        let drop = self.normalize(first);
        let add = self.normalize(next);
        let table = match &drop {
            DdlOp::DropPrimaryKey { table } => table,
            DdlOp::DropIndex { table, name } if name == "PRIMARY" => table,
            _ => return Ok(false),
        };
        let DdlOp::AddPrimaryKey {
            table: add_table,
            columns,
        } = &add
        else {
            return Ok(false);
        };
        if !same_table(table, add_table) {
            return Ok(false);
        }
        validate(self.dialect, first)?;
        validate(self.dialect, next)?;
        let sql = format!(
            "ALTER TABLE {} DROP PRIMARY KEY, ADD PRIMARY KEY ({})",
            self.qt(table),
            self.cols(columns)
        );
        self.push(sql);
        if self.is_model(table) {
            self.update_model(&drop, true)?;
            self.update_model(&add, true)?;
        }
        Ok(true)
    }

    /// MySQL: an index drop directly followed by an index add on the same table (how the
    /// structure editor stages an edited index), or a CHECK drop directly followed by a CHECK
    /// add, becomes one `ALTER TABLE … DROP …, ADD …`. MySQL DDL is not transactional: as two
    /// statements, a failing add (a UNIQUE index over duplicates, a CHECK the rows violate)
    /// would leave the original dropped; one ALTER TABLE fails as a whole. Returns whether both
    /// ops were handled.
    fn replace_index_or_check(
        &mut self,
        first: &DdlOp,
        next: Option<&DdlOp>,
    ) -> Result<bool, AppError> {
        let Some(next) = next.filter(|_| self.dialect == Dialect::Mysql) else {
            return Ok(false);
        };
        let drop = self.normalize(first);
        let add = self.normalize(next);
        let (table, sql) = match (&drop, &add) {
            (
                DdlOp::DropIndex { table, name },
                DdlOp::AddIndex {
                    table: add_table,
                    index,
                },
            ) if same_table(table, add_table) => {
                validate(self.dialect, first)?;
                validate(self.dialect, next)?;
                self.index_warnings(index);
                let dropped = if name == "PRIMARY" {
                    "DROP PRIMARY KEY".to_string()
                } else {
                    format!("DROP INDEX {}", self.q(name))
                };
                let sql = format!(
                    "ALTER TABLE {} {dropped}, {}",
                    self.qt(table),
                    self.mysql_add_index_clause(table, index)
                );
                (table, sql)
            }
            (
                DdlOp::DropCheck { table, name },
                DdlOp::AddCheck {
                    table: add_table,
                    check,
                },
            ) if same_table(table, add_table) => {
                validate(self.dialect, first)?;
                validate(self.dialect, next)?;
                let sql = format!(
                    "ALTER TABLE {} DROP CONSTRAINT {}, ADD {}",
                    self.qt(table),
                    self.q(name),
                    self.check_clause(
                        non_empty(check.name.as_deref()).as_deref(),
                        &check.expression
                    )
                );
                (table, sql)
            }
            _ => return Ok(false),
        };
        let on_model = self.is_model(table);
        self.push(sql);
        if on_model {
            self.update_model(&drop, true)?;
            self.update_model(&add, true)?;
        }
        Ok(true)
    }

    /// `rest` are the ops after `op` (SQLite looks ahead for a rebuild of the same table).
    fn op(&mut self, op: &DdlOp, rest: &[DdlOp]) -> Result<(), AppError> {
        validate(self.dialect, op)?;
        let op = &self.normalize(op);
        let on_model = op_table(op).is_some_and(|t| self.is_model(t));
        if self.dialect == Dialect::Sqlite {
            if self.pending && !(on_model && foldable(op)) {
                self.flush()?;
            }
            // A rename followed by a rebuild of the same table waits for the rebuild: the
            // rebuild then replays indexes and triggers exactly as stored and the rename, run
            // last, lets SQLite rewrite them (and views, and child foreign keys) itself.
            let defer_rename = on_model
                && !self.pending
                && matches!(op, DdlOp::RenameColumn { .. })
                && self.group_needs_rebuild(op, rest);
            if on_model && (self.pending || defer_rename || self.sqlite_needs_rebuild(op)) {
                return self.fold(op);
            }
        }
        self.direct(op)?;
        if on_model {
            self.update_model(op, true)?;
        }
        Ok(())
    }

    /// Whether `first` and the ops directly after it on the model table (up to the first op
    /// that cannot join a rebuild) include one that needs a SQLite rebuild.
    fn group_needs_rebuild(&self, first: &DdlOp, rest: &[DdlOp]) -> bool {
        let mut sim = Planner {
            dialect: self.dialect,
            mysql_mode: self.mysql_mode,
            model: self.model.clone(),
            statements: Vec::new(),
            warnings: Vec::new(),
            pending: false,
            rebuild_base: Vec::new(),
            rebuilt: false,
            sequential: false,
            dropped_triggers: Vec::new(),
        };
        if sim.update_model(first, true).is_err() {
            return false;
        }
        for op in rest {
            if validate(sim.dialect, op).is_err() {
                return false;
            }
            let op = sim.normalize(op);
            if !(op_table(&op).is_some_and(|t| sim.is_model(t)) && foldable(&op)) {
                return false;
            }
            if sim.sqlite_needs_rebuild(&op) {
                return true;
            }
            if sim.update_model(&op, true).is_err() {
                return false;
            }
        }
        false
    }

    fn finish(mut self) -> Result<DdlPlan, AppError> {
        if self.pending {
            self.flush()?;
        }
        if self.dialect == Dialect::Sqlite && self.rebuilt && self.sequential {
            // They would have to run outside the rebuild's transaction, between the foreign-key
            // toggles; keep the two apart instead.
            return Err(invalid(
                "VACUUM and PRAGMA statements cannot be applied together with changes that rebuild a SQLite table; apply them separately",
            ));
        }
        let transactional = match self.dialect {
            Dialect::Postgres => !self.sequential,
            Dialect::Mysql => false,
            Dialect::Sqlite => !self.rebuilt && !self.sequential,
        };
        let mut statements = std::mem::take(&mut self.statements);
        if self.dialect == Dialect::Sqlite && self.rebuilt {
            let mut wrapped = vec!["PRAGMA foreign_keys = OFF".to_string(), "BEGIN".to_string()];
            wrapped.append(&mut statements);
            if let Some(m) = self.model.as_ref().filter(|m| !m.dropped) {
                wrapped.push(sqlite_fk_check_sql(
                    m.table.schema.as_deref(),
                    &m.table.name,
                ));
            }
            wrapped.push("COMMIT".into());
            wrapped.push("PRAGMA foreign_keys = ON".into());
            statements = wrapped;
        }
        Ok(DdlPlan {
            statements,
            warnings: self.warnings,
            transactional,
        })
    }

    // --- SQLite rebuild -----------------------------------------------------------------------

    fn sqlite_needs_rebuild(&self, op: &DdlOp) -> bool {
        let Some(m) = &self.model else { return false };
        match op {
            DdlOp::AlterColumnType { .. }
            | DdlOp::AlterColumnNullable { .. }
            | DdlOp::AlterColumnDefault { .. }
            | DdlOp::AlterColumnAutoIncrement { .. }
            | DdlOp::AlterColumnCollation { .. }
            | DdlOp::AddForeignKey { .. }
            | DdlOp::DropForeignKey { .. }
            | DdlOp::AddPrimaryKey { .. }
            | DdlOp::DropPrimaryKey { .. }
            | DdlOp::AddCheck { .. }
            | DdlOp::DropCheck { .. } => true,
            DdlOp::AddColumn { column, .. } => column
                .default_expr
                .as_deref()
                .map(str::trim)
                .is_some_and(|d| !d.is_empty() && !sqlite_constant(d)),
            DdlOp::DropColumn { name, .. } => m.column_is_constrained(name),
            // An index backing a PRIMARY KEY or UNIQUE constraint (no stored SQL) can only go
            // away with a rebuild; SQLite refuses `DROP INDEX` on it.
            DdlOp::DropIndex { name, .. } => m
                .indexes
                .iter()
                .any(|i| &i.name == name && (i.primary || i.definition.is_none())),
            _ => false,
        }
    }

    fn ensure_not_generated(&self, t: &TableRef, name: &str) -> Result<(), AppError> {
        if self.model_col(t, name, "changing a column")?.generated {
            return Err(AppError::Unsupported(format!(
                "\"{name}\" is a generated column; change its definition with raw SQL"
            )));
        }
        Ok(())
    }

    fn fold(&mut self, op: &DdlOp) -> Result<(), AppError> {
        let table = op_table(op).cloned().expect("table-scoped op");
        match op {
            DdlOp::SetColumnComment { .. } | DdlOp::SetTableComment { .. } => {
                self.warn(SQLITE_NO_COMMENTS);
                return Ok(());
            }
            DdlOp::AddColumn { column, after, .. } => {
                if after.is_some() {
                    self.warn("column position (AFTER/FIRST) is MySQL-only; ignored");
                }
                let (_, warnings) = col_from_def(self.dialect, column);
                for w in warnings {
                    self.warn(w);
                }
            }
            DdlOp::AlterColumnType { name, using, .. } => {
                self.ensure_not_generated(&table, name)?;
                if using.as_deref().is_some_and(|u| !u.trim().is_empty()) {
                    self.warn(
                        "USING is PostgreSQL-only; SQLite copies the existing values unchanged",
                    );
                }
            }
            DdlOp::AlterColumnNullable { name, .. }
            | DdlOp::AlterColumnDefault { name, .. }
            | DdlOp::AlterColumnAutoIncrement { name, .. }
            | DdlOp::AlterColumnCollation { name, .. } => {
                self.ensure_not_generated(&table, name)?;
            }
            DdlOp::AlterColumnOnUpdate { .. } => {
                return Err(AppError::Unsupported("ON UPDATE is MySQL-only".into()));
            }
            DdlOp::AddIndex { index, .. } => self.index_warnings(index),
            _ => {}
        }
        if !self.pending {
            self.rebuild_base = self
                .model
                .as_ref()
                .map(|m| m.columns.iter().filter_map(|c| c.source.clone()).collect())
                .unwrap_or_default();
        }
        let notes = self.update_model(op, false)?;
        for n in notes {
            self.warn(n);
        }
        self.pending = true;
        Ok(())
    }

    /// Emits the pending SQLite rebuild of the model table.
    fn flush(&mut self) -> Result<(), AppError> {
        self.pending = false;
        let d = Dialect::Sqlite;
        let Some(m) = self.model.clone() else {
            return Ok(());
        };
        let name = m.table.name.clone();
        if m.columns.is_empty() {
            return Err(invalid(format!("\"{name}\" would have no columns left")));
        }
        for (i, c) in m.columns.iter().enumerate() {
            if let Some(o) = m.columns[..i]
                .iter()
                .find(|o| same_column(d, &o.name, &c.name))
            {
                return Err(duplicate_column(&c.name, &o.name, &m.table));
            }
        }
        for c in &m.columns {
            if c.generated && c.verbatim.is_none() {
                return Err(AppError::Unsupported(format!(
                    "cannot rebuild \"{name}\": the expression of generated column \"{}\" could not be recovered",
                    c.name
                )));
            }
        }
        for k in &m.primary_key {
            if m.col(k).is_none() {
                return Err(unknown_column(k, &m.table));
            }
        }
        for f in &m.fks {
            for c in &f.columns {
                if m.col(c).is_none() {
                    return Err(unknown_column(c, &m.table));
                }
            }
        }
        if let Some((trigger, column)) = m
            .stale_triggers
            .iter()
            .find(|(t, _)| m.triggers.iter().any(|x| &x.name == t))
        {
            return Err(AppError::Unsupported(format!(
                "cannot rebuild \"{name}\": trigger \"{trigger}\" mentions column \"{column}\", which an earlier change of this set renames, and the trigger could not be recreated faithfully; apply the rename and the other changes to \"{name}\" separately"
            )));
        }
        let schema = m.table.schema.as_deref();
        let tmp_name = format!("{name}{REBUILD_SUFFIX}");
        let tmp = d.qualify(schema, &tmp_name);
        let current = d.qualify(schema, &name);
        self.warn(format!(
            "\"{name}\" is rebuilt from introspected metadata (new table, copy, drop, rename): ON CONFLICT clauses, DEFERRABLE foreign-key options and any COLLATE or generated-column expression that cannot be recovered from the original definition may be lost; review the statements before applying"
        ));

        // The new table keeps the physical column names (stored index and trigger text uses
        // them); columns renamed in this rebuild get their new names afterwards.
        let phys = rebuild_names(&m.columns);
        let to_phys = |n: &String| {
            m.col_index(n)
                .map_or_else(|| n.clone(), |i| phys[i].clone())
        };
        let cols: Vec<Col> = m
            .columns
            .iter()
            .zip(&phys)
            .map(|(c, p)| Col {
                name: p.clone(),
                ..c.clone()
            })
            .collect();
        let pk: Vec<String> = m.primary_key.iter().map(to_phys).collect();
        let uniques: Vec<Vec<String>> = sqlite_unique_constraints(&m)
            .iter()
            .map(|u| u.iter().map(to_phys).collect())
            .collect();
        let fks: Vec<Fk> = m
            .fks
            .iter()
            .map(|f| {
                let mut f = f.clone();
                f.columns = f.columns.iter().map(to_phys).collect();
                if same_table(&f.ref_table, &m.table) {
                    f.ref_columns = f.ref_columns.iter().map(to_phys).collect();
                }
                f
            })
            .collect();

        // Checks added in this rebuild spell the final column names; the new table still has
        // the physical ones.
        let to_physical: Vec<(String, String)> = m
            .columns
            .iter()
            .zip(&phys)
            .filter(|(c, p)| &c.name != *p)
            .map(|(c, p)| (c.name.clone(), p.clone()))
            .collect();
        let checks: Vec<Check> = m
            .checks
            .iter()
            .map(|k| {
                let mut k = k.clone();
                if k.fresh {
                    k.expression = rename_all(d, &k.expression, &to_physical);
                    k.fresh = false;
                }
                k
            })
            .collect();
        let inline = self.sqlite_inline_pk(&cols, &pk);
        for (i, c) in m.columns.iter().enumerate() {
            if c.auto_increment && inline != Some(i) {
                self.warn(format!(
                    "AUTOINCREMENT needs a single INTEGER PRIMARY KEY column; ignored for \"{}\"",
                    c.name
                ));
            }
        }
        let body = self.table_body(&cols, &pk, inline, None, &uniques, &fks, &checks);
        let mut create = format!("CREATE TABLE {tmp} (\n{body}\n)");
        if let Some(opts) = &m.table_options {
            create.push(' ');
            create.push_str(opts);
        }
        self.push(create);

        let copied: Vec<&Col> = cols
            .iter()
            .filter(|c| !c.generated && c.source.is_some())
            .collect();
        if !copied.is_empty() {
            let targets = copied
                .iter()
                .map(|c| d.quote_ident(&c.name))
                .collect::<Vec<_>>()
                .join(", ");
            let sources = copied
                .iter()
                .map(|c| d.quote_ident(c.source.as_deref().unwrap_or(&c.name)))
                .collect::<Vec<_>>()
                .join(", ");
            self.push(format!(
                "INSERT INTO {tmp} ({targets}) SELECT {sources} FROM {current}"
            ));
        }
        if inline.is_some() {
            // Keep the AUTOINCREMENT counter: the copy set the new table's counter to the
            // largest id copied, and DROP TABLE deletes the old one (ids of deleted rows would
            // be handed out again). The rename below carries the row over to the final name.
            let seq = d.qualify(schema, "sqlite_sequence");
            self.push(format!(
                "DELETE FROM {seq} WHERE name = {}",
                self.lit(&tmp_name)
            ));
            self.push(format!(
                "UPDATE {seq} SET name = {} WHERE name = {}",
                self.lit(&tmp_name),
                self.lit(&name)
            ));
        }
        // Views and other tables' triggers naming the table are dangling between the drop and
        // the rename; the modern rename would refuse to run because of them.
        self.push(LEGACY_ALTER_ON.to_string());
        self.push(format!("DROP TABLE {current}"));
        self.push(format!(
            "ALTER TABLE {tmp} RENAME TO {}",
            d.quote_ident(&name)
        ));
        self.push(LEGACY_ALTER_OFF.to_string());
        let fresh = |i: &IndexMeta| m.fresh_indexes.contains(&i.name);
        for idx in m.indexes.iter().filter(|i| !fresh(i)) {
            if let Some(def) = &idx.definition
                && !idx.name.starts_with("sqlite_autoindex_")
            {
                self.push(replay_index(def, schema, &name));
            }
        }
        for t in &m.triggers {
            if !t.definition.trim().is_empty() {
                self.push(replay_trigger(&t.definition, schema));
            }
        }
        // Views and triggers (the replayed ones too) still naming a dropped column would only
        // fail when used. SQLite's own DROP COLUMN refuses such a change: make the block fail
        // the same way. Renaming a column of a scratch table runs SQLite's check of every view
        // and trigger of the schema, and edits nothing else.
        let dropped: Vec<String> = self
            .rebuild_base
            .iter()
            .filter(|p| !m.columns.iter().any(|c| c.source.as_ref() == Some(*p)))
            .cloned()
            .collect();
        if !dropped.is_empty() {
            let check = d.qualify(schema, &format!("{name}{CHECK_SUFFIX}"));
            self.push(format!("CREATE TABLE {check} (\"c\")"));
            self.push(format!("ALTER TABLE {check} RENAME COLUMN \"c\" TO \"d\""));
            self.push(format!("DROP TABLE {check}"));
            let list = dropped
                .iter()
                .map(|p| format!("\"{p}\""))
                .collect::<Vec<_>>()
                .join(", ");
            self.warn(format!(
                "dropping {list} rebuilds \"{name}\": if a view or trigger still uses a dropped column, applying fails and nothing is changed"
            ));
        }
        let current_names: Vec<String> = cols.iter().map(|c| c.name.clone()).collect();
        let finals: Vec<String> = m.columns.iter().map(|c| c.name.clone()).collect();
        let renames = ordered_renames(&current_names, &finals)?;
        for (from, to) in &renames {
            self.push(format!(
                "ALTER TABLE {current} RENAME COLUMN {} TO {}",
                d.quote_ident(from),
                d.quote_ident(to)
            ));
        }
        for idx in m.indexes.iter().filter(|i| fresh(i)) {
            if let Some(def) = &idx.definition {
                self.push(replay_index(def, schema, &name));
            }
        }
        if let Some(model) = self.model.as_mut() {
            model.checks = checks;
            for (from, to) in &renames {
                model.renamed_in_database(d, from, to);
            }
            model.fresh_indexes.clear();
            for c in &mut model.columns {
                c.source = Some(c.name.clone());
            }
        }
        self.rebuilt = true;
        Ok(())
    }

    /// SQLite `AUTOINCREMENT` lives on the column: `"id" INTEGER PRIMARY KEY AUTOINCREMENT`.
    fn sqlite_inline_pk(&self, cols: &[Col], pk: &[String]) -> Option<usize> {
        if self.dialect != Dialect::Sqlite || pk.len() != 1 {
            return None;
        }
        let i = cols.iter().position(|c| c.name == pk[0])?;
        (cols[i].auto_increment && cols[i].data_type.eq_ignore_ascii_case("INTEGER")).then_some(i)
    }

    // --- model updates ------------------------------------------------------------------------

    /// Applies `op` to the model. `physical` is false while folding into a pending SQLite
    /// rebuild (the database is not touched yet; unknown names are errors then). Returns notes
    /// about dependents that went away.
    fn update_model(&mut self, op: &DdlOp, physical: bool) -> Result<Vec<String>, AppError> {
        let strict = !physical;
        let dialect = self.dialect;
        let index_def = match op {
            DdlOp::AddIndex { table, index } if dialect == Dialect::Sqlite => {
                Some(self.index_sql(table, index))
            }
            _ => None,
        };
        let Some(m) = self.model.as_mut() else {
            return Ok(Vec::new());
        };
        let mut notes = Vec::new();
        match op {
            DdlOp::AddColumn { column, after, .. } => {
                if let Some(existing) = m.clash(dialect, &column.name, None) {
                    if strict {
                        return Err(duplicate_column(&column.name, &existing.name, &m.table));
                    }
                    // the database refuses the statement; the model stays as it is
                    return Ok(notes);
                }
                let (mut col, _) = col_from_def(dialect, column);
                if physical {
                    col.source = Some(col.name.clone());
                }
                let pos = match (dialect, after.as_deref()) {
                    (Dialect::Mysql, Some("")) => 0,
                    (Dialect::Mysql, Some(a)) => m.col_index(a).map_or(m.columns.len(), |i| i + 1),
                    _ => m.columns.len(),
                };
                m.columns.insert(pos, col);
            }
            DdlOp::DropColumn { name, .. } => {
                if m.col(name).is_none() {
                    if strict {
                        return Err(unknown_column(name, &m.table));
                    }
                    return Ok(notes);
                }
                notes = m.drop_column(name);
            }
            DdlOp::RenameColumn { name, new_name, .. } => {
                if m.col(name).is_none() {
                    if strict {
                        return Err(unknown_column(name, &m.table));
                    }
                    return Ok(notes);
                }
                // (a column may take another spelling of its own name)
                if let Some(existing) = m.clash(dialect, new_name, Some(name)) {
                    if strict {
                        return Err(duplicate_column(new_name, &existing.name, &m.table));
                    }
                    // the database refuses the statement; the model stays as it is
                    return Ok(notes);
                }
                m.rename_column(dialect, name, new_name, physical);
                for (column, _) in &mut m.owned_sequences {
                    if column == name {
                        column.clone_from(new_name);
                    }
                }
            }
            DdlOp::AlterColumnType { name, .. }
            | DdlOp::AlterColumnNullable { name, .. }
            | DdlOp::AlterColumnDefault { name, .. }
            | DdlOp::SetColumnComment { name, .. }
            | DdlOp::AlterColumnAutoIncrement { name, .. }
            | DdlOp::AlterColumnOnUpdate { name, .. }
            | DdlOp::AlterColumnCollation { name, .. } => match m.col_index(name) {
                Some(i) => m.columns[i] = changed_col(dialect, &m.columns[i], op),
                None if strict => return Err(unknown_column(name, &m.table)),
                None => {}
            },
            DdlOp::SetTableComment { comment, .. } => {
                m.comment = comment.clone().filter(|s| !s.is_empty());
            }
            DdlOp::AlterTableOptions { options, .. } => {
                let o = &mut m.options;
                if let Some(v) = &options.engine {
                    o.engine = Some(v.clone());
                }
                if let Some(v) = &options.collation {
                    o.charset = Some(charset_of_collation(v));
                    o.collation = Some(v.clone());
                } else if let Some(v) = &options.charset {
                    o.charset = Some(v.clone());
                    o.collation = None;
                }
                if let Some(v) = options.auto_increment {
                    o.auto_increment = Some(v);
                }
                if let Some(v) = &options.row_format {
                    o.row_format = Some(v.clone());
                }
            }
            DdlOp::AddCheck { check, .. } => {
                let name = non_empty(check.name.as_deref());
                if let Some(n) = &name
                    && m.checks.iter().any(|k| k.name.eq_ignore_ascii_case(n))
                {
                    if strict {
                        return Err(invalid(format!(
                            "CHECK constraint \"{n}\" already exists on \"{}\"",
                            m.table.name
                        )));
                    }
                    return Ok(notes);
                }
                let expression = check.expression.trim().to_string();
                let names = m.names();
                let synthetic = unique_name(
                    &format!("ck_{}_{}", m.table.name, m.checks.len() + 1),
                    &[&m.checks.iter().map(|k| k.name.clone()).collect::<Vec<_>>()],
                );
                m.checks.push(Check {
                    columns: check_columns(&expression, &names),
                    named: name.is_some(),
                    name: name.unwrap_or(synthetic),
                    expression,
                    fresh: !physical && dialect == Dialect::Sqlite,
                });
            }
            DdlOp::DropCheck { name, .. } => {
                let before = m.checks.len();
                m.checks.retain(|k| &k.name != name);
                if strict && before == m.checks.len() {
                    return Err(invalid(format!(
                        "CHECK constraint \"{name}\" does not exist on \"{}\"",
                        m.table.name
                    )));
                }
            }
            DdlOp::CreateTrigger { name, sql, .. } => {
                m.triggers.retain(|t| &t.name != name);
                m.stale_triggers.retain(|(t, _)| t != name);
                m.triggers.push(TriggerMeta {
                    name: name.clone(),
                    timing: None,
                    events: Vec::new(),
                    enabled: true,
                    definition: sql.trim().trim_end_matches(';').trim_end().to_string(),
                });
            }
            DdlOp::DropTrigger { name, .. } => {
                m.triggers.retain(|t| &t.name != name);
                m.stale_triggers.retain(|(t, _)| t != name);
            }
            DdlOp::EnableTrigger { name, .. } | DdlOp::DisableTrigger { name, .. } => {
                let enabled = matches!(op, DdlOp::EnableTrigger { .. });
                match m.triggers.iter_mut().find(|t| &t.name == name) {
                    Some(t) => t.enabled = enabled,
                    None if strict => {
                        return Err(invalid(format!(
                            "trigger \"{name}\" does not exist on \"{}\"",
                            m.table.name
                        )));
                    }
                    None => {}
                }
            }
            DdlOp::AddIndex { index, .. } => {
                if m.indexes.iter().any(|i| i.name == index.name) {
                    if strict {
                        return Err(invalid(format!("index \"{}\" already exists", index.name)));
                    }
                    return Ok(notes);
                }
                m.indexes.push(IndexMeta {
                    name: index.name.clone(),
                    columns: index.columns.clone(),
                    unique: index.unique,
                    primary: false,
                    method: index.method.clone(),
                    predicate: index.predicate.clone(),
                    definition: index_def,
                    comment: non_empty(index.comment.as_deref()),
                    include: index.include.clone(),
                });
                if !physical && dialect == Dialect::Sqlite {
                    m.fresh_indexes.push(index.name.clone());
                }
            }
            DdlOp::DropIndex { name, .. } => {
                m.fresh_indexes.retain(|n| n != name);
                match m.indexes.iter().find(|i| &i.name == name) {
                    Some(i) if i.primary => m.clear_primary_key(dialect),
                    Some(_) => {}
                    None if strict => {
                        return Err(invalid(format!(
                            "index \"{name}\" does not exist on \"{}\"",
                            m.table.name
                        )));
                    }
                    None => {}
                }
                m.indexes.retain(|i| &i.name != name);
            }
            DdlOp::AddForeignKey { fk, .. } => {
                if strict && let Some(c) = fk.columns.iter().find(|c| m.col(c).is_none()) {
                    return Err(unknown_column(c, &m.table));
                }
                let name = non_empty(fk.name.as_deref());
                m.fks.push(Fk {
                    named: name.is_some(),
                    name: name
                        .unwrap_or_else(|| format!("fk_{}_{}", m.table.name, fk.columns.join("_"))),
                    columns: fk.columns.clone(),
                    ref_table: fk.ref_table.clone(),
                    ref_columns: fk.ref_columns.clone(),
                    on_update: fk_action(fk.on_update.as_deref())?,
                    on_delete: fk_action(fk.on_delete.as_deref())?,
                    definition: None,
                });
            }
            DdlOp::DropForeignKey { name, .. } => {
                let before = m.fks.len();
                m.fks.retain(|f| &f.name != name);
                if strict && before == m.fks.len() {
                    return Err(invalid(format!(
                        "foreign key \"{name}\" does not exist on \"{}\"",
                        m.table.name
                    )));
                }
            }
            DdlOp::AddPrimaryKey { columns, .. } => {
                if strict {
                    if !m.primary_key.is_empty() {
                        return Err(invalid(format!(
                            "\"{}\" already has a primary key; drop it first",
                            m.table.name
                        )));
                    }
                    if let Some(c) = columns.iter().find(|c| m.col(c).is_none()) {
                        return Err(unknown_column(c, &m.table));
                    }
                }
                m.primary_key = columns.clone();
                m.pk_name = match dialect {
                    Dialect::Postgres => Some(format!("{}_pkey", m.table.name)),
                    Dialect::Mysql => Some("PRIMARY".into()),
                    Dialect::Sqlite => None,
                };
                for c in m.columns.iter_mut().filter(|c| columns.contains(&c.name)) {
                    c.nullable = false;
                    c.implicit_not_null = false;
                }
            }
            DdlOp::DropPrimaryKey { .. } => {
                if strict && m.primary_key.is_empty() {
                    return Err(invalid(format!("\"{}\" has no primary key", m.table.name)));
                }
                m.clear_primary_key(dialect);
                m.indexes.retain(|i| !i.primary);
            }
            DdlOp::RenameTable { new_name, .. } => m.rename_table(dialect, new_name),
            DdlOp::DropTable { .. } => m.dropped = true,
            DdlOp::TruncateTable { .. } | DdlOp::CreateTable { .. } | DdlOp::RawSql { .. } => {}
        }
        Ok(notes)
    }

    // --- direct statements --------------------------------------------------------------------

    fn direct(&mut self, op: &DdlOp) -> Result<(), AppError> {
        let d = self.dialect;
        match op {
            DdlOp::CreateTable {
                table,
                columns,
                primary_key,
                if_not_exists,
            } => self.create_table(table, columns, primary_key, *if_not_exists),
            DdlOp::DropTable {
                table,
                if_exists,
                cascade,
            } => {
                let mut sql = format!(
                    "DROP TABLE {}{}",
                    if *if_exists { "IF EXISTS " } else { "" },
                    self.qt(table)
                );
                if *cascade {
                    if d == Dialect::Postgres {
                        sql.push_str(" CASCADE");
                    } else {
                        self.warn("CASCADE is PostgreSQL-only; ignored for DROP TABLE");
                    }
                }
                self.push(sql);
            }
            DdlOp::TruncateTable {
                table,
                restart_identity,
                cascade,
            } => self.truncate(table, *restart_identity, *cascade),
            DdlOp::RenameTable { table, new_name } => {
                let sql = match d {
                    Dialect::Mysql => format!(
                        "RENAME TABLE {} TO {}",
                        self.qt(table),
                        d.qualify(table.schema.as_deref(), new_name)
                    ),
                    _ => format!(
                        "ALTER TABLE {} RENAME TO {}",
                        self.qt(table),
                        self.q(new_name)
                    ),
                };
                self.push(sql);
            }
            DdlOp::AddColumn {
                table,
                column,
                after,
            } => return self.add_column(table, column, after.as_deref()),
            DdlOp::DropColumn { table, name } => {
                let sql = format!(
                    "ALTER TABLE {} DROP COLUMN {}",
                    self.qt(table),
                    self.q(name)
                );
                self.push(sql);
            }
            DdlOp::RenameColumn {
                table,
                name,
                new_name,
            } => {
                let sql = format!(
                    "ALTER TABLE {} RENAME COLUMN {} TO {}",
                    self.qt(table),
                    self.q(name),
                    self.q(new_name)
                );
                self.push(sql);
            }
            DdlOp::AlterColumnType {
                table,
                name,
                new_type,
                using,
            } => match d {
                Dialect::Postgres => {
                    let mut sql = format!(
                        "ALTER TABLE {} ALTER COLUMN {} TYPE {}",
                        self.qt(table),
                        self.q(name),
                        new_type.trim()
                    );
                    if let Some(u) = non_empty(using.as_deref()) {
                        sql.push_str(" USING ");
                        sql.push_str(&u);
                    }
                    self.push(sql);
                }
                Dialect::Mysql => {
                    if non_empty(using.as_deref()).is_some() {
                        self.warn("USING is PostgreSQL-only; MySQL converts values itself");
                    }
                    self.mysql_modify(table, name, op)?;
                }
                Dialect::Sqlite => return Err(self.need_meta(table, "changing a column type")),
            },
            DdlOp::AlterColumnNullable {
                table,
                name,
                nullable,
            } => match d {
                Dialect::Postgres => {
                    let sql = format!(
                        "ALTER TABLE {} ALTER COLUMN {} {} NOT NULL",
                        self.qt(table),
                        self.q(name),
                        if *nullable { "DROP" } else { "SET" }
                    );
                    self.push(sql);
                }
                Dialect::Mysql => self.mysql_modify(table, name, op)?,
                Dialect::Sqlite => return Err(self.need_meta(table, "changing nullability")),
            },
            DdlOp::AlterColumnDefault {
                table,
                name,
                default_expr,
            } => match d {
                // `ALTER COLUMN … SET DEFAULT` only takes a literal or `(expr)` on MySQL, not
                // CURRENT_TIMESTAMP or a bare function call: restate the column instead when its
                // definition is known (works on MariaDB and older servers too).
                Dialect::Mysql
                    if non_empty(default_expr.as_deref())
                        .is_some_and(|e| !MYSQL_LITERAL.is_match(&e))
                        && self.model_col(table, name, "").is_ok() =>
                {
                    self.mysql_modify(table, name, op)?;
                }
                Dialect::Postgres | Dialect::Mysql => {
                    let action = match non_empty(default_expr.as_deref()) {
                        Some(e) if d == Dialect::Mysql && !MYSQL_LITERAL.is_match(&e) => {
                            format!("SET DEFAULT {}", parenthesized(&e))
                        }
                        Some(e) => format!("SET DEFAULT {e}"),
                        None => "DROP DEFAULT".to_string(),
                    };
                    let sql = format!(
                        "ALTER TABLE {} ALTER COLUMN {} {action}",
                        self.qt(table),
                        self.q(name)
                    );
                    self.push(sql);
                }
                Dialect::Sqlite => return Err(self.need_meta(table, "changing a default")),
            },
            DdlOp::SetColumnComment {
                table,
                name,
                comment,
            } => match d {
                Dialect::Postgres => {
                    let sql = format!(
                        "COMMENT ON COLUMN {}.{} IS {}",
                        self.qt(table),
                        self.q(name),
                        self.comment_lit(comment.as_deref())
                    );
                    self.push(sql);
                }
                Dialect::Mysql => self.mysql_modify(table, name, op)?,
                Dialect::Sqlite => self.warn(SQLITE_NO_COMMENTS),
            },
            DdlOp::AlterColumnAutoIncrement {
                table,
                name,
                auto_increment,
            } => match d {
                Dialect::Postgres => self.pg_auto_increment(table, name, *auto_increment),
                Dialect::Mysql => self.mysql_modify(table, name, op)?,
                Dialect::Sqlite => return Err(self.need_meta(table, "changing AUTOINCREMENT")),
            },
            DdlOp::AlterColumnOnUpdate { table, name, .. } => match d {
                Dialect::Mysql => self.mysql_modify(table, name, op)?,
                _ => return Err(AppError::Unsupported("ON UPDATE is MySQL-only".into())),
            },
            DdlOp::AlterColumnCollation {
                table,
                name,
                collation,
            } => match d {
                Dialect::Postgres => {
                    let col = self.model_col(table, name, "changing a collation")?;
                    let ty = col.data_type.clone();
                    let coll = match non_empty(collation.as_deref()) {
                        Some(c) => self.q(&c),
                        None => "\"default\"".to_string(),
                    };
                    let sql = format!(
                        "ALTER TABLE {} ALTER COLUMN {} TYPE {ty} COLLATE {coll}",
                        self.qt(table),
                        self.q(name)
                    );
                    self.push(sql);
                }
                Dialect::Mysql => self.mysql_modify(table, name, op)?,
                Dialect::Sqlite => return Err(self.need_meta(table, "changing a collation")),
            },
            DdlOp::AlterTableOptions { table, options } => {
                if d != Dialect::Mysql {
                    return Err(AppError::Unsupported(
                        "engine, character set, AUTO_INCREMENT and row format are MySQL table options"
                            .into(),
                    ));
                }
                let sql = self.mysql_table_options(table, options);
                self.push(sql);
            }
            DdlOp::AddCheck { table, check } => {
                if d == Dialect::Sqlite {
                    return Err(self.need_meta(table, "adding a CHECK constraint"));
                }
                let sql = format!(
                    "ALTER TABLE {} ADD {}",
                    self.qt(table),
                    self.check_clause(
                        non_empty(check.name.as_deref()).as_deref(),
                        &check.expression
                    )
                );
                self.push(sql);
            }
            DdlOp::DropCheck { table, name } => {
                if d == Dialect::Sqlite {
                    return Err(self.need_meta(table, "dropping a CHECK constraint"));
                }
                // (MySQL 8.0.19+ and MariaDB; `DROP CHECK` is MySQL-only)
                let sql = format!(
                    "ALTER TABLE {} DROP CONSTRAINT {}",
                    self.qt(table),
                    self.q(name)
                );
                self.push(sql);
            }
            DdlOp::CreateTrigger { name, sql, .. } => {
                if d == Dialect::Mysql && self.dropped_triggers.iter().any(|t| t == name) {
                    self.warn(format!(
                        "MySQL runs these statements one by one: if creating trigger \"{name}\" fails, the old trigger is already dropped"
                    ));
                }
                let sql = sql.trim().trim_end_matches(';').trim_end().to_string();
                self.push(sql);
            }
            DdlOp::DropTrigger { table, name } => {
                let sql = match d {
                    Dialect::Postgres => {
                        format!("DROP TRIGGER {} ON {}", self.q(name), self.qt(table))
                    }
                    Dialect::Mysql | Dialect::Sqlite => {
                        format!("DROP TRIGGER {}", d.qualify(table.schema.as_deref(), name))
                    }
                };
                self.dropped_triggers.push(name.clone());
                self.push(sql);
            }
            DdlOp::EnableTrigger { table, name } | DdlOp::DisableTrigger { table, name } => {
                if d != Dialect::Postgres {
                    return Err(AppError::Unsupported(format!(
                        "{} triggers cannot be disabled: drop the trigger instead",
                        if d == Dialect::Mysql {
                            "MySQL"
                        } else {
                            "SQLite"
                        }
                    )));
                }
                let action = if matches!(op, DdlOp::EnableTrigger { .. }) {
                    "ENABLE"
                } else {
                    "DISABLE"
                };
                let sql = trigger_state_sql(table, name, action == "ENABLE");
                self.push(sql);
            }
            DdlOp::SetTableComment { table, comment } => match d {
                Dialect::Postgres => {
                    let sql = format!(
                        "COMMENT ON TABLE {} IS {}",
                        self.qt(table),
                        self.comment_lit(comment.as_deref())
                    );
                    self.push(sql);
                }
                Dialect::Mysql => {
                    let sql = format!(
                        "ALTER TABLE {} COMMENT = {}",
                        self.qt(table),
                        self.lit(comment.as_deref().unwrap_or(""))
                    );
                    self.push(sql);
                }
                Dialect::Sqlite => self.warn(SQLITE_NO_COMMENTS),
            },
            DdlOp::AddIndex { table, index } => {
                self.index_warnings(index);
                let sql = self.index_sql(table, index);
                self.push(sql);
                if d == Dialect::Postgres
                    && let Some(comment) = non_empty(index.comment.as_deref())
                {
                    let sql = format!(
                        "COMMENT ON INDEX {} IS {}",
                        d.qualify(table.schema.as_deref(), &index.name),
                        self.lit(&comment)
                    );
                    self.push(sql);
                }
            }
            DdlOp::DropIndex { table, name } => {
                let sql = match d {
                    Dialect::Postgres => {
                        // PostgreSQL refuses DROP INDEX on the index of a PRIMARY KEY, UNIQUE
                        // or EXCLUDE constraint; the constraint (same name) goes instead.
                        let constraint = self
                            .model
                            .as_ref()
                            .filter(|m| same_table(&m.table, table))
                            .is_some_and(|m| {
                                m.constraint_indexes.contains(name)
                                    || m.indexes.iter().any(|i| &i.name == name && i.primary)
                            });
                        if constraint {
                            format!(
                                "ALTER TABLE {} DROP CONSTRAINT {}",
                                self.qt(table),
                                self.q(name)
                            )
                        } else {
                            format!("DROP INDEX {}", d.qualify(table.schema.as_deref(), name))
                        }
                    }
                    Dialect::Mysql if name == "PRIMARY" => {
                        format!("ALTER TABLE {} DROP PRIMARY KEY", self.qt(table))
                    }
                    Dialect::Mysql => {
                        format!("DROP INDEX {} ON {}", self.q(name), self.qt(table))
                    }
                    Dialect::Sqlite => {
                        format!("DROP INDEX {}", d.qualify(table.schema.as_deref(), name))
                    }
                };
                self.push(sql);
            }
            DdlOp::AddForeignKey { table, fk } => {
                if d == Dialect::Sqlite {
                    return Err(self.need_meta(table, "adding a foreign key"));
                }
                let clause = self.fk_clause(
                    non_empty(fk.name.as_deref()).as_deref(),
                    &fk.columns,
                    &fk.ref_table,
                    &fk.ref_columns,
                    fk_action(fk.on_update.as_deref())?.as_deref(),
                    fk_action(fk.on_delete.as_deref())?.as_deref(),
                );
                let sql = format!("ALTER TABLE {} ADD {clause}", self.qt(table));
                self.push(sql);
            }
            DdlOp::DropForeignKey { table, name } => {
                let sql = match d {
                    Dialect::Postgres => format!(
                        "ALTER TABLE {} DROP CONSTRAINT {}",
                        self.qt(table),
                        self.q(name)
                    ),
                    Dialect::Mysql => format!(
                        "ALTER TABLE {} DROP FOREIGN KEY {}",
                        self.qt(table),
                        self.q(name)
                    ),
                    Dialect::Sqlite => {
                        return Err(self.need_meta(table, "dropping a foreign key"));
                    }
                };
                self.push(sql);
            }
            DdlOp::AddPrimaryKey { table, columns } => {
                if d == Dialect::Sqlite {
                    return Err(self.need_meta(table, "adding a primary key"));
                }
                let sql = format!(
                    "ALTER TABLE {} ADD PRIMARY KEY ({})",
                    self.qt(table),
                    self.cols(columns)
                );
                self.push(sql);
            }
            DdlOp::DropPrimaryKey { table } => {
                let sql = match d {
                    Dialect::Postgres => {
                        let m = self.model_of(table, "dropping the primary key")?;
                        let pk = m.pk_name.clone().ok_or_else(|| {
                            invalid(format!("\"{}\" has no primary key", table.name))
                        })?;
                        format!(
                            "ALTER TABLE {} DROP CONSTRAINT {}",
                            self.qt(table),
                            self.q(&pk)
                        )
                    }
                    Dialect::Mysql => format!("ALTER TABLE {} DROP PRIMARY KEY", self.qt(table)),
                    Dialect::Sqlite => {
                        return Err(self.need_meta(table, "dropping the primary key"));
                    }
                };
                self.push(sql);
            }
            DdlOp::RawSql { sql } => {
                let sql = sql.trim().trim_end_matches(';').trim_end().to_string();
                let non_transactional = match d {
                    Dialect::Postgres => PG_NON_TRANSACTIONAL.is_match(&sql),
                    Dialect::Sqlite => SQLITE_NON_TRANSACTIONAL.is_match(&sql),
                    Dialect::Mysql => false,
                };
                if non_transactional {
                    self.sequential = true;
                    self.warn(
                        "a statement cannot run inside a transaction block; the plan runs statement by statement",
                    );
                }
                self.push(sql);
            }
        }
        Ok(())
    }

    fn comment_lit(&self, comment: Option<&str>) -> String {
        match comment.filter(|c| !c.is_empty()) {
            Some(c) => self.lit(c),
            None => "NULL".to_string(),
        }
    }

    fn truncate(&mut self, table: &TableRef, restart_identity: bool, cascade: bool) {
        let d = self.dialect;
        let qt = self.qt(table);
        match d {
            Dialect::Postgres => {
                let mut sql = format!("TRUNCATE TABLE {qt}");
                if restart_identity {
                    sql.push_str(" RESTART IDENTITY");
                }
                if cascade {
                    sql.push_str(" CASCADE");
                }
                self.push(sql);
            }
            Dialect::Mysql => {
                if cascade {
                    self.warn("CASCADE is PostgreSQL-only; ignored for TRUNCATE");
                }
                if !restart_identity {
                    self.warn("MySQL TRUNCATE always resets AUTO_INCREMENT");
                }
                self.push(format!("TRUNCATE TABLE {qt}"));
            }
            Dialect::Sqlite => {
                if cascade {
                    self.warn("CASCADE is PostgreSQL-only; ignored for TRUNCATE");
                }
                self.push(format!("DELETE FROM {qt}"));
                // Only AUTOINCREMENT tables keep a counter (sqlite_sequence may not even exist).
                let counter = match self.model.as_ref().filter(|m| same_table(&m.table, table)) {
                    Some(m) => m.columns.iter().any(|c| c.auto_increment),
                    None => true,
                };
                if restart_identity && counter {
                    let seq = d.qualify(table.schema.as_deref(), "sqlite_sequence");
                    let sql = format!("DELETE FROM {seq} WHERE name = {}", self.lit(&table.name));
                    self.push(sql);
                }
            }
        }
    }

    fn create_table(
        &mut self,
        table: &TableRef,
        columns: &[ColumnDef],
        primary_key: &[String],
        if_not_exists: bool,
    ) {
        let d = self.dialect;
        let mut cols = Vec::with_capacity(columns.len());
        for def in columns {
            let (col, warnings) = col_from_def(d, def);
            for w in warnings {
                self.warn(w);
            }
            cols.push(col);
        }
        let inline = self.sqlite_inline_pk(&cols, primary_key);
        if d == Dialect::Sqlite {
            for (i, c) in cols.iter().enumerate() {
                if c.auto_increment && inline != Some(i) {
                    self.warn(format!(
                        "AUTOINCREMENT needs a single INTEGER PRIMARY KEY column; ignored for \"{}\"",
                        c.name
                    ));
                }
            }
        }
        let body = self.table_body(&cols, primary_key, inline, None, &[], &[], &[]);
        let mut sql = format!(
            "CREATE TABLE {}{} (\n{body}\n)",
            if if_not_exists { "IF NOT EXISTS " } else { "" },
            self.qt(table)
        );
        if d == Dialect::Mysql {
            sql.push_str(" ENGINE=InnoDB");
        }
        self.push(sql);
        if d == Dialect::Postgres {
            for c in &cols {
                if let Some(comment) = &c.comment {
                    let sql = format!(
                        "COMMENT ON COLUMN {}.{} IS {}",
                        self.qt(table),
                        self.q(&c.name),
                        self.lit(comment)
                    );
                    self.push(sql);
                }
            }
        }
    }

    fn add_column(
        &mut self,
        table: &TableRef,
        def: &ColumnDef,
        after: Option<&str>,
    ) -> Result<(), AppError> {
        let d = self.dialect;
        let (col, warnings) = col_from_def(d, def);
        for w in warnings {
            self.warn(w);
        }
        if d == Dialect::Sqlite
            && col
                .default_expr
                .as_deref()
                .is_some_and(|e| !sqlite_constant(e))
        {
            return Err(self.need_meta(table, "adding a column with a non-constant default"));
        }
        let mut sql = format!(
            "ALTER TABLE {} ADD COLUMN {}",
            self.qt(table),
            self.column_sql(&col, false)
        );
        match (d, after) {
            (Dialect::Mysql, Some("")) => sql.push_str(" FIRST"),
            (Dialect::Mysql, Some(a)) => {
                sql.push_str(" AFTER ");
                sql.push_str(&self.q(a));
            }
            (_, Some(_)) => self.warn("column position (AFTER/FIRST) is MySQL-only; ignored"),
            _ => {}
        }
        self.push(sql);
        if d == Dialect::Postgres
            && let Some(comment) = &col.comment
        {
            let sql = format!(
                "COMMENT ON COLUMN {}.{} IS {}",
                self.qt(table),
                self.q(&col.name),
                self.lit(comment)
            );
            self.push(sql);
        }
        Ok(())
    }

    /// MySQL `MODIFY COLUMN` with the complete current definition plus the change.
    fn mysql_modify(&mut self, table: &TableRef, name: &str, op: &DdlOp) -> Result<(), AppError> {
        let col = self.model_col(
            table,
            name,
            "changing a column (MODIFY COLUMN restates the whole definition)",
        )?;
        if col.generated {
            return Err(AppError::Unsupported(format!(
                "\"{name}\" is a generated column; change its definition with raw SQL"
            )));
        }
        let changed = changed_col(Dialect::Mysql, col, op);
        let sql = format!(
            "ALTER TABLE {} MODIFY COLUMN {}",
            self.qt(table),
            self.column_sql(&changed, false)
        );
        self.push(sql);
        Ok(())
    }

    /// PG identity on / off. Turning it on drops the column's default and makes it NOT NULL
    /// first (both are required), then moves the new sequence past the existing values.
    ///
    /// A `serial` column still owns its old sequence, and `pg_get_serial_sequence` would keep
    /// returning that one instead of the identity sequence: the old sequence is detached first
    /// (`OWNED BY NONE`). It is kept, not dropped: something else (another default, the
    /// application) may still draw from it, and it holds no rows.
    fn pg_auto_increment(&mut self, table: &TableRef, name: &str, on: bool) {
        let qt = self.qt(table);
        let qc = self.q(name);
        let model = self.model.as_ref().filter(|m| same_table(&m.table, table));
        let col = model.and_then(|m| m.col(name)).cloned();
        let owned: Vec<TableRef> = model
            .map(|m| {
                m.owned_sequences
                    .iter()
                    .filter(|(column, _)| column == name)
                    .map(|(_, seq)| seq.clone())
                    .collect()
            })
            .unwrap_or_default();
        if on {
            if col.as_ref().is_some_and(|c| c.identity.is_some()) {
                self.warn(format!("\"{name}\" already is an identity column"));
                return;
            }
            if col.as_ref().is_some_and(|c| c.default_expr.is_some()) {
                self.push(format!("ALTER TABLE {qt} ALTER COLUMN {qc} DROP DEFAULT"));
            }
            for seq in &owned {
                let qs = self.qt(seq);
                self.push(format!("ALTER SEQUENCE {qs} OWNED BY NONE"));
                self.warn(format!(
                    "the sequence {qs} that \"{name}\" drew from is kept, no longer owned by the column; drop it when nothing else uses it"
                ));
            }
            if col.as_ref().is_some_and(|c| c.nullable) {
                self.push(format!("ALTER TABLE {qt} ALTER COLUMN {qc} SET NOT NULL"));
            }
            self.push(format!(
                "ALTER TABLE {qt} ALTER COLUMN {qc} ADD GENERATED BY DEFAULT AS IDENTITY"
            ));
            self.push(format!(
                "SELECT setval(pg_get_serial_sequence({}, {}), GREATEST(COALESCE(MAX({qc}), 0) + 1, 1), false) FROM {qt}",
                self.lit(&qt),
                self.lit(name)
            ));
        } else {
            match col {
                Some(c) if c.identity.is_none() => {
                    if c.default_expr.as_deref().is_some_and(is_nextval) {
                        self.push(format!("ALTER TABLE {qt} ALTER COLUMN {qc} DROP DEFAULT"));
                    } else {
                        self.warn(format!("\"{name}\" does not auto-increment"));
                    }
                }
                _ => self.push(format!(
                    "ALTER TABLE {qt} ALTER COLUMN {qc} DROP IDENTITY IF EXISTS"
                )),
            }
        }
    }

    /// `ALTER TABLE … ENGINE = …, CONVERT TO CHARACTER SET … COLLATE …, AUTO_INCREMENT = …,
    /// ROW_FORMAT = …` (MySQL).
    fn mysql_table_options(&self, table: &TableRef, o: &TableOptions) -> String {
        let mut parts = Vec::new();
        if let Some(e) = &o.engine {
            parts.push(format!("ENGINE = {}", e.trim()));
        }
        let collation = o.collation.as_deref().map(str::trim);
        let charset = o
            .charset
            .as_deref()
            .map(str::trim)
            .map(str::to_string)
            .or_else(|| collation.map(charset_of_collation));
        if let Some(cs) = charset {
            let mut convert = format!("CONVERT TO CHARACTER SET {cs}");
            if let Some(c) = collation {
                convert.push_str(&format!(" COLLATE {c}"));
            }
            parts.push(convert);
        }
        if let Some(n) = o.auto_increment {
            parts.push(format!("AUTO_INCREMENT = {n}"));
        }
        if let Some(r) = &o.row_format {
            parts.push(format!("ROW_FORMAT = {}", r.trim().to_ascii_uppercase()));
        }
        format!("ALTER TABLE {} {}", self.qt(table), parts.join(", "))
    }

    /// `[CONSTRAINT name] CHECK (expr)`.
    fn check_clause(&self, name: Option<&str>, expression: &str) -> String {
        let constraint = name
            .map(|n| format!("CONSTRAINT {} ", self.q(n)))
            .unwrap_or_default();
        format!("{constraint}CHECK ({})", expression.trim())
    }

    // --- rendering ----------------------------------------------------------------------------

    /// One column definition. `inline_pk` renders SQLite's `PRIMARY KEY AUTOINCREMENT`.
    fn column_sql(&self, col: &Col, inline_pk: bool) -> String {
        let d = self.dialect;
        if d == Dialect::Sqlite
            && col.generated
            && let Some(v) = &col.verbatim
        {
            return v.clone();
        }
        let mut s = self.q(&col.name);
        if !col.data_type.is_empty() {
            s.push(' ');
            s.push_str(&col.data_type);
        }
        match d {
            Dialect::Postgres => {
                if let Some(c) = &col.collation {
                    s.push_str(" COLLATE ");
                    s.push_str(&self.q(c));
                }
                let identity = col
                    .identity
                    .clone()
                    .or_else(|| col.auto_increment.then(|| "BY DEFAULT".to_string()));
                if col.generated {
                    if let Some(e) = &col.default_expr {
                        s.push_str(&format!(" GENERATED ALWAYS AS ({e}) STORED"));
                    }
                } else if let Some(kind) = &identity {
                    s.push_str(&format!(" GENERATED {kind} AS IDENTITY"));
                }
                if !col.nullable {
                    s.push_str(" NOT NULL");
                }
                if !col.generated
                    && identity.is_none()
                    && let Some(e) = &col.default_expr
                {
                    s.push_str(" DEFAULT ");
                    s.push_str(e);
                }
            }
            Dialect::Mysql => {
                if col.unsigned && !col.data_type.to_ascii_lowercase().contains("unsigned") {
                    s.push_str(" UNSIGNED");
                }
                if let Some(c) = &col.collation {
                    s.push_str(" COLLATE ");
                    s.push_str(c);
                }
                s.push_str(if col.nullable { " NULL" } else { " NOT NULL" });
                if let Some(e) = &col.default_expr {
                    s.push_str(" DEFAULT ");
                    s.push_str(e);
                }
                if let Some(e) = &col.on_update {
                    s.push_str(" ON UPDATE ");
                    s.push_str(e);
                }
                if col.auto_increment {
                    s.push_str(" AUTO_INCREMENT");
                }
                if col.invisible {
                    s.push_str(" INVISIBLE");
                }
                if let Some(c) = &col.comment {
                    s.push_str(" COMMENT ");
                    s.push_str(&self.lit(c));
                }
            }
            Dialect::Sqlite => {
                if inline_pk {
                    s.push_str(" PRIMARY KEY AUTOINCREMENT");
                } else if !col.nullable && !col.implicit_not_null {
                    // (a key column's implicit NOT NULL is not restated: SQLite lets a
                    // non-integer key of a rowid table hold NULL unless it was declared)
                    s.push_str(" NOT NULL");
                }
                if let Some(e) = &col.default_expr {
                    s.push_str(" DEFAULT ");
                    s.push_str(&sqlite_default_sql(e));
                }
                if let Some(c) = &col.collation {
                    s.push_str(" COLLATE ");
                    s.push_str(&self.q(c));
                }
            }
        }
        s
    }

    /// The inside of `CREATE TABLE (…)`: columns, primary key, unique constraints, CHECK
    /// constraints, foreign keys.
    #[allow(clippy::too_many_arguments)]
    fn table_body(
        &self,
        cols: &[Col],
        pk: &[String],
        inline_pk: Option<usize>,
        pk_name: Option<&str>,
        uniques: &[Vec<String>],
        fks: &[Fk],
        checks: &[Check],
    ) -> String {
        let mut lines: Vec<String> = cols
            .iter()
            .enumerate()
            .map(|(i, c)| self.column_sql(c, inline_pk == Some(i)))
            .collect();
        if !pk.is_empty() && inline_pk.is_none() {
            let constraint = pk_name
                .map(|n| format!("CONSTRAINT {} ", self.q(n)))
                .unwrap_or_default();
            lines.push(format!("{constraint}PRIMARY KEY ({})", self.cols(pk)));
        }
        for u in uniques {
            lines.push(format!("UNIQUE ({})", self.cols(u)));
        }
        for k in checks {
            lines.push(self.check_clause(k.named.then_some(k.name.as_str()), &k.expression));
        }
        for f in fks {
            lines.push(self.fk_clause(
                f.named.then_some(f.name.as_str()),
                &f.columns,
                &f.ref_table,
                &f.ref_columns,
                f.on_update.as_deref(),
                f.on_delete.as_deref(),
            ));
        }
        lines
            .iter()
            .map(|l| format!("  {l}"))
            .collect::<Vec<_>>()
            .join(",\n")
    }

    fn fk_clause(
        &self,
        name: Option<&str>,
        columns: &[String],
        ref_table: &TableRef,
        ref_columns: &[String],
        on_update: Option<&str>,
        on_delete: Option<&str>,
    ) -> String {
        let mut s = String::new();
        if let Some(n) = name {
            s.push_str(&format!("CONSTRAINT {} ", self.q(n)));
        }
        let target = match self.dialect {
            // SQLite foreign keys always point into the same database and reject a qualifier.
            Dialect::Sqlite => self.q(&ref_table.name),
            _ => self.qt(ref_table),
        };
        s.push_str(&format!(
            "FOREIGN KEY ({}) REFERENCES {target}",
            self.cols(columns)
        ));
        if ref_columns.iter().any(|c| !c.is_empty()) {
            s.push_str(&format!(" ({})", self.cols(ref_columns)));
        }
        for (event, action) in [("UPDATE", on_update), ("DELETE", on_delete)] {
            if let Some(a) = action.filter(|a| !a.eq_ignore_ascii_case("NO ACTION")) {
                s.push_str(&format!(" ON {event} {a}"));
            }
        }
        s
    }

    fn index_warnings(&mut self, index: &IndexDef) {
        let has_predicate = non_empty(index.predicate.as_deref()).is_some();
        let has_method = non_empty(index.method.as_deref()).is_some();
        match self.dialect {
            Dialect::Mysql if has_predicate => self.warn(format!(
                "MySQL has no partial indexes; the predicate of \"{}\" is ignored",
                index.name
            )),
            Dialect::Sqlite if has_method => self.warn(format!(
                "SQLite has a single index method; the method of \"{}\" is ignored",
                index.name
            )),
            _ => {}
        }
        if self.dialect != Dialect::Postgres && index.include.iter().any(|c| !c.trim().is_empty()) {
            self.warn(format!(
                "INCLUDE columns are PostgreSQL-only; those of \"{}\" are ignored",
                index.name
            ));
        }
        if self.dialect == Dialect::Sqlite && non_empty(index.comment.as_deref()).is_some() {
            self.warn(format!(
                "SQLite does not store index comments; the comment of \"{}\" is ignored",
                index.name
            ));
        }
    }

    fn index_sql(&self, table: &TableRef, index: &IndexDef) -> String {
        let d = self.dialect;
        let known = self
            .model
            .as_ref()
            .filter(|m| same_table(&m.table, table))
            .map(Model::names)
            .unwrap_or_default();
        let parts = index
            .columns
            .iter()
            .map(|c| self.index_part(c, &known))
            .collect::<Vec<_>>()
            .join(", ");
        let unique = if index.unique { "UNIQUE " } else { "" };
        let method = non_empty(index.method.as_deref());
        let predicate = non_empty(index.predicate.as_deref())
            .map(|p| format!(" WHERE {p}"))
            .unwrap_or_default();
        match d {
            Dialect::Postgres => {
                let include: Vec<String> = index
                    .include
                    .iter()
                    .map(|c| c.trim())
                    .filter(|c| !c.is_empty())
                    .map(|c| self.q(c))
                    .collect();
                let include = if include.is_empty() {
                    String::new()
                } else {
                    format!(" INCLUDE ({})", include.join(", "))
                };
                format!(
                    "CREATE {unique}INDEX {} ON {}{} ({parts}){include}{predicate}",
                    self.q(&index.name),
                    self.qt(table),
                    method.map(|m| format!(" USING {m}")).unwrap_or_default()
                )
            }
            Dialect::Mysql => {
                let (kind, tail) = self.mysql_index_kind_and_options(index);
                format!(
                    "CREATE {kind}INDEX {} ON {} ({parts}){tail}",
                    self.q(&index.name),
                    self.qt(table)
                )
            }
            Dialect::Sqlite => format!(
                "CREATE {unique}INDEX {} ON {} ({parts}){predicate}",
                d.qualify(table.schema.as_deref(), &index.name),
                self.q(&table.name)
            ),
        }
    }

    /// MySQL: the index kind (`UNIQUE `, `FULLTEXT `, `SPATIAL ` or nothing) and what follows
    /// the column list (` USING …`, ` COMMENT …`).
    fn mysql_index_kind_and_options(&self, index: &IndexDef) -> (&'static str, String) {
        let unique = if index.unique { "UNIQUE " } else { "" };
        let upper = non_empty(index.method.as_deref()).map(|m| m.to_ascii_uppercase());
        let (kind, using) = match upper.as_deref() {
            Some("FULLTEXT") => ("FULLTEXT ", None),
            Some("SPATIAL") => ("SPATIAL ", None),
            Some(m) => (unique, Some(m.to_string())),
            None => (unique, None),
        };
        let using = using.map(|m| format!(" USING {m}")).unwrap_or_default();
        let comment = non_empty(index.comment.as_deref())
            .map(|c| format!(" COMMENT {}", self.lit(&c)))
            .unwrap_or_default();
        (kind, format!("{using}{comment}"))
    }

    /// MySQL `ADD [UNIQUE|FULLTEXT|SPATIAL] INDEX name (…) …` clause of an `ALTER TABLE`.
    fn mysql_add_index_clause(&self, table: &TableRef, index: &IndexDef) -> String {
        let known = self
            .model
            .as_ref()
            .filter(|m| same_table(&m.table, table))
            .map(Model::names)
            .unwrap_or_default();
        let parts = index
            .columns
            .iter()
            .map(|c| self.index_part(c, &known))
            .collect::<Vec<_>>()
            .join(", ");
        let (kind, tail) = self.mysql_index_kind_and_options(index);
        format!("ADD {kind}INDEX {} ({parts}){tail}", self.q(&index.name))
    }

    /// One entry of an index column list: quoted when it names a column, verbatim otherwise.
    fn index_part(&self, entry: &str, known: &[String]) -> String {
        let e = entry.trim();
        if known.iter().any(|k| k == e) {
            return self.q(e);
        }
        let suffix = |c: &regex::Captures<'_>| {
            let len = c.get(2).map_or("", |m| m.as_str());
            let dir = c
                .get(3)
                .map(|m| format!(" {}", m.as_str().to_ascii_uppercase()))
                .unwrap_or_default();
            format!("{len}{dir}")
        };
        if let Some(c) = INDEX_PART_SUFFIX.captures(e)
            && (c.get(2).is_some() || c.get(3).is_some())
            && known.iter().any(|k| k == &c[1])
        {
            return format!("{}{}", self.q(&c[1]), suffix(&c));
        }
        if e.starts_with(['"', '`', '[', '(']) {
            return e.to_string();
        }
        match INDEX_PART.captures(e) {
            Some(c) => format!("{}{}", self.q(&c[1]), suffix(&c)),
            None => e.to_string(),
        }
    }

    /// `CREATE TABLE` plus indexes, foreign keys, triggers and comments, from the model; on PG
    /// also what `context` adds: the sequences of the column defaults, `PARTITION BY`, the
    /// attach of a partition, and no clones of a parent's foreign keys or triggers.
    fn render_parts(&self, context: &PgTableContext) -> TableParts {
        let d = self.dialect;
        let Some(m) = &self.model else {
            return TableParts::default();
        };
        let pg = d == Dialect::Postgres;
        let qt = self.qt(&m.table);
        let parent = context
            .ancestors
            .first()
            .filter(|_| pg && context.partition_bound.is_some());
        // A partition takes its identity from its parent: PostgreSQL refuses to attach a table
        // with an identity column of its own.
        let stripped: Vec<Col>;
        let columns: &[Col] = if parent.is_some() {
            stripped = m
                .columns
                .iter()
                .cloned()
                .map(|mut c| {
                    c.identity = None;
                    c.auto_increment = false;
                    c
                })
                .collect();
            &stripped
        } else {
            &m.columns
        };
        let inline = self.sqlite_inline_pk(columns, &m.primary_key);
        let (uniques, inline_fks): (Vec<Vec<String>>, &[Fk]) = if d == Dialect::Sqlite {
            (sqlite_unique_constraints(m), &m.fks)
        } else {
            (Vec::new(), &[])
        };
        let pk_name = if pg { m.pk_name.as_deref() } else { None };
        let body = self.table_body(
            columns,
            &m.primary_key,
            inline,
            pk_name,
            &uniques,
            inline_fks,
            &m.checks,
        );
        let mut create = format!("CREATE TABLE {qt} (\n{body}\n)");
        match d {
            Dialect::Mysql => {
                if let Some(c) = &m.comment {
                    create.push_str(&format!(" COMMENT={}", self.lit(c)));
                }
            }
            Dialect::Sqlite => {
                if let Some(opts) = &m.table_options {
                    create.push(' ');
                    create.push_str(opts);
                }
            }
            Dialect::Postgres => {
                if let Some(key) = context.partition_key.as_deref().map(str::trim) {
                    create.push_str(&format!(" PARTITION BY {key}"));
                }
            }
        }
        create.push(';');
        let mut table = vec![create];

        // PG: the sequences the defaults draw from exist before the table (a fresh database has
        // none); one owned by its column is owned again, so it goes with the table and
        // `pg_get_serial_sequence` finds it.
        let mut sequences = Vec::new();
        let mut owned: Vec<&PgSequence> = Vec::new();
        if pg {
            for s in &context.sequences {
                let sql = self.create_sequence_sql(s);
                if !sequences.contains(&sql) {
                    sequences.push(sql);
                }
                if s.owned && columns.iter().any(|c| c.name == s.column) {
                    table.push(format!(
                        "ALTER SEQUENCE {} OWNED BY {qt}.{};",
                        d.qualify(Some(&s.schema), &s.name),
                        self.q(&s.column)
                    ));
                    owned.push(s);
                }
            }
        }

        let mut indexes = Vec::new();
        for i in m.indexes.iter().filter(|i| !i.primary) {
            match (&i.definition, d) {
                (Some(def), _) => indexes.push(terminated(def)),
                // constraint-backed: emitted as UNIQUE (…) in the table body
                (None, Dialect::Sqlite) => {}
                (None, _) => {
                    let method = i
                        .method
                        .as_deref()
                        .filter(|m| {
                            matches!(m.to_ascii_uppercase().as_str(), "FULLTEXT" | "SPATIAL")
                        })
                        .map(str::to_string);
                    let def = IndexDef {
                        name: i.name.clone(),
                        columns: i.columns.clone(),
                        unique: i.unique,
                        method,
                        predicate: None,
                        comment: i.comment.clone(),
                        include: i.include.clone(),
                    };
                    indexes.push(terminated(&self.index_sql(&m.table, &def)));
                }
            }
        }

        let attach = parent
            .zip(context.partition_bound.as_deref())
            .map(|(p, bound)| {
                format!(
                    "ALTER TABLE {} ATTACH PARTITION {qt} {};",
                    self.qt(p),
                    bound.trim()
                )
            });

        let mut fks = Vec::new();
        if d != Dialect::Sqlite {
            for f in m
                .fks
                .iter()
                .filter(|f| !context.inherited_fks.contains(&f.name))
            {
                let clause = match &f.definition {
                    Some(def) => format!("CONSTRAINT {} {def}", self.q(&f.name)),
                    None => self.fk_clause(
                        Some(&f.name),
                        &f.columns,
                        &f.ref_table,
                        &f.ref_columns,
                        f.on_update.as_deref(),
                        f.on_delete.as_deref(),
                    ),
                };
                fks.push(format!("ALTER TABLE {qt} ADD {clause};"));
            }
        }

        let mut triggers = Vec::new();
        for t in &m.triggers {
            if t.definition.trim().is_empty() || context.cloned_triggers.contains(&t.name) {
                continue;
            }
            match d {
                Dialect::Mysql if !first_word(&t.definition).eq_ignore_ascii_case("CREATE") => {
                    // information_schema only has the action statement
                    let timing = t.timing.as_deref().unwrap_or("BEFORE");
                    let event = t.events.first().map(String::as_str).unwrap_or("INSERT");
                    triggers.push(terminated(&format!(
                        "CREATE TRIGGER {} {timing} {event} ON {qt} FOR EACH ROW {}",
                        self.q(&t.name),
                        t.definition.trim()
                    )));
                }
                _ => triggers.push(terminated(&t.definition)),
            }
            // PG keeps a disabled trigger disabled (pg_get_triggerdef does not say it).
            if pg && !t.enabled {
                triggers.push(format!("{};", trigger_state_sql(&m.table, &t.name, false)));
            }
        }

        let mut comments = Vec::new();
        if pg {
            if let Some(c) = &m.comment {
                comments.push(format!("COMMENT ON TABLE {qt} IS {};", self.lit(c)));
            }
            for i in &m.indexes {
                if let Some(text) = i.comment.as_deref().filter(|c| !c.is_empty()) {
                    comments.push(format!(
                        "COMMENT ON INDEX {} IS {};",
                        d.qualify(m.table.schema.as_deref(), &i.name),
                        self.lit(text)
                    ));
                }
            }
            for c in &m.columns {
                if let Some(text) = &c.comment {
                    comments.push(format!(
                        "COMMENT ON COLUMN {qt}.{} IS {};",
                        self.q(&c.name),
                        self.lit(text)
                    ));
                }
            }
        }

        // PG: rows loaded with their own key values leave the sequences where they started.
        let mut sequence_resets = Vec::new();
        let mut reset_columns = Vec::new();
        if pg {
            for c in columns.iter().filter(|c| !c.generated) {
                let bounds = if c.identity.is_some() {
                    // the identity the script creates has the default options
                    Some((1, 1))
                } else {
                    owned
                        .iter()
                        .find(|s| s.column == c.name)
                        .map(|s| (s.increment.unwrap_or(1), sequence_bounds(s)))
                        .map(|(inc, (min, max))| (inc, if inc < 0 { max } else { min }))
                };
                if let Some((increment, limit)) = bounds {
                    sequence_resets
                        .push(self.sequence_reset_sql(&m.table, &c.name, increment, limit));
                    reset_columns.push(c.name.clone());
                }
            }
        }

        TableParts {
            sequences,
            table,
            indexes,
            attach,
            foreign_keys: fks,
            triggers,
            comments,
            sequence_resets,
            reset_columns,
        }
    }

    /// `CREATE SEQUENCE IF NOT EXISTS` with the options that differ from the defaults.
    fn create_sequence_sql(&self, s: &PgSequence) -> String {
        let mut sql = format!(
            "CREATE SEQUENCE IF NOT EXISTS {}",
            self.dialect.qualify(Some(&s.schema), &s.name)
        );
        if let Some(t) = s.data_type.as_deref().filter(|t| !t.is_empty()) {
            sql.push_str(&format!(" AS {t}"));
        }
        let increment = s.increment.unwrap_or(1);
        let (default_min, default_max) = type_bounds(s.data_type.as_deref());
        let (default_min, default_max) = if increment < 0 {
            (default_min, -1)
        } else {
            (1, default_max)
        };
        if increment != 1 {
            sql.push_str(&format!(" INCREMENT BY {increment}"));
        }
        if let Some(min) = s.min.filter(|&v| v != default_min) {
            sql.push_str(&format!(" MINVALUE {min}"));
        }
        if let Some(max) = s.max.filter(|&v| v != default_max) {
            sql.push_str(&format!(" MAXVALUE {max}"));
        }
        let default_start = if increment < 0 {
            s.max.unwrap_or(default_max)
        } else {
            s.min.unwrap_or(default_min)
        };
        if let Some(start) = s.start.filter(|&v| v != default_start) {
            sql.push_str(&format!(" START WITH {start}"));
        }
        if let Some(cache) = s.cache.filter(|&v| v != 1) {
            sql.push_str(&format!(" CACHE {cache}"));
        }
        if s.cycle {
            sql.push_str(" CYCLE");
        }
        sql.push(';');
        sql
    }

    /// Moves the sequence of `column` past the loaded rows: the next value follows the largest
    /// key (the smallest for a descending sequence), never beyond the sequence's `limit` (its
    /// MINVALUE, or MAXVALUE when descending). An empty table leaves it where it starts.
    fn sequence_reset_sql(
        &self,
        table: &TableRef,
        column: &str,
        increment: i64,
        limit: i64,
    ) -> String {
        let qt = self.qt(table);
        let qc = self.q(column);
        let (agg, clamp) = if increment < 0 {
            ("MIN", "LEAST")
        } else {
            ("MAX", "GREATEST")
        };
        format!(
            "SELECT setval(pg_get_serial_sequence({}, {}), {clamp}({agg}({qc}), {limit})) FROM {qt} HAVING {agg}({qc}) IS NOT NULL;",
            self.lit(&qt),
            self.lit(column)
        )
    }
}

/// The value range of a sequence type (`bigint` when unknown).
fn type_bounds(data_type: Option<&str>) -> (i64, i64) {
    match data_type.map(str::trim) {
        Some("smallint") => (i16::MIN.into(), i16::MAX.into()),
        Some("integer") => (i32::MIN.into(), i32::MAX.into()),
        _ => (i64::MIN, i64::MAX),
    }
}

/// MINVALUE and MAXVALUE of a sequence (the defaults of its type and direction when unknown).
fn sequence_bounds(s: &PgSequence) -> (i64, i64) {
    let (type_min, type_max) = type_bounds(s.data_type.as_deref());
    let descending = s.increment.unwrap_or(1) < 0;
    let min = s.min.unwrap_or(if descending { type_min } else { 1 });
    let max = s.max.unwrap_or(if descending { -1 } else { type_max });
    (min, max)
}

/// The first word of a statement (`CREATE`).
fn first_word(sql: &str) -> &str {
    sql.split_whitespace().next().unwrap_or("")
}

fn terminated(sql: &str) -> String {
    let s = sql.trim();
    if s.ends_with(';') {
        s.to_string()
    } else {
        format!("{s};")
    }
}

/// UNIQUE constraints of a SQLite table: unique indexes without stored SQL (autoindexes).
fn sqlite_unique_constraints(m: &Model) -> Vec<Vec<String>> {
    m.indexes
        .iter()
        .filter(|i| i.unique && !i.primary && i.definition.is_none())
        .map(|i| i.columns.clone())
        .filter(|cols| !cols.is_empty() && cols.iter().all(|c| m.col(c).is_some()))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Type and default helpers
// ---------------------------------------------------------------------------------------------

fn base_type(ty: &str) -> String {
    ty.trim()
        .split(|c: char| c == '(' || c.is_whitespace())
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn is_integer_type(dialect: Dialect, ty: &str) -> bool {
    let b = base_type(ty);
    match dialect {
        Dialect::Postgres => matches!(
            b.as_str(),
            "smallint" | "integer" | "int" | "bigint" | "int2" | "int4" | "int8"
        ),
        Dialect::Mysql => matches!(
            b.as_str(),
            "tinyint" | "smallint" | "mediumint" | "int" | "integer" | "bigint"
        ),
        Dialect::Sqlite => b == "integer",
    }
}

fn is_pg_serial(ty: &str) -> bool {
    matches!(
        base_type(ty).as_str(),
        "serial" | "bigserial" | "smallserial" | "serial2" | "serial4" | "serial8"
    )
}

fn is_mysql_numeric(lower: &str) -> bool {
    matches!(
        base_type(lower).as_str(),
        "tinyint"
            | "smallint"
            | "mediumint"
            | "int"
            | "integer"
            | "bigint"
            | "decimal"
            | "dec"
            | "numeric"
            | "fixed"
            | "float"
            | "double"
            | "real"
    )
}

fn is_mysql_textual(lower: &str) -> bool {
    let b = base_type(lower);
    b.contains("char")
        || b.contains("text")
        || matches!(b.as_str(), "enum" | "set" | "national" | "character")
}

/// Whether a MySQL column type names its character set or collation itself
/// (`varchar(20) CHARACTER SET latin1`, `text COLLATE utf8mb4_bin`, `nvarchar(10)`, which is
/// utf8mb3), so that a collation kept from the old definition would conflict with it.
fn mysql_type_sets_charset(ty: &str) -> bool {
    // (outside parentheses and string literals: `enum('ascii')` names no character set)
    MYSQL_CHARSET_CLAUSE.is_match(&top_level(ty))
        || matches!(base_type(ty).as_str(), "nchar" | "nvarchar" | "national")
}

/// Whether a MySQL column is generated. `information_schema` marks expression defaults
/// `DEFAULT_GENERATED` in EXTRA; generated columns say `VIRTUAL GENERATED` / `STORED GENERATED`
/// (MariaDB also `PERSISTENT GENERATED`).
fn mysql_is_generated(c: &ColumnMeta) -> bool {
    if !c.is_generated {
        return false;
    }
    let extra = c.extra.as_deref().unwrap_or("").to_ascii_lowercase();
    let generated_word = extra
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .any(|w| w == "generated");
    generated_word || !extra.contains("default_generated")
}

/// `expr` wrapped in one pair of parentheses.
fn parenthesized(expr: &str) -> String {
    let e = expr.trim();
    if wrapped_in_parens(e) {
        e.to_string()
    } else {
        format!("({e})")
    }
}

/// A designer default as a MySQL column definition takes it: literals and CURRENT_TIMESTAMP (and
/// synonyms) as they are, any other expression parenthesised (8.0.13+ expression default).
fn mysql_default_sql(expr: &str) -> String {
    let e = expr.trim();
    if MYSQL_LITERAL.is_match(e) || MYSQL_NOW.is_match(e) {
        e.to_string()
    } else {
        parenthesized(e)
    }
}

/// The `DEFAULT` clause of a column line of `SHOW CREATE TABLE`, verbatim: `Some(None)` when
/// the column has none (or `DEFAULT NULL`), `None` when the line cannot be read with certainty.
fn mysql_default_from_ddl(def: &str) -> Option<Option<String>> {
    const FOLLOWERS: &[&str] = &[
        "ON",
        "COMMENT",
        "NOT",
        "NULL",
        "AUTO_INCREMENT",
        "INVISIBLE",
        "VISIBLE",
        "COLLATE",
        "CHARACTER",
        "CHECK",
        "CONSTRAINT",
        "REFERENCES",
        "UNIQUE",
        "PRIMARY",
        "KEY",
        "COLUMN_FORMAT",
        "STORAGE",
        "SRID",
        "ENGINE_ATTRIBUTE",
        "SECONDARY_ENGINE_ATTRIBUTE",
    ];
    let chars: Vec<char> = def.chars().collect();
    if chars.first() != Some(&'`') {
        return None;
    }
    let mut i = skip_quoted(&chars, 0);
    let mut depth = 0i32;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' | '"' | '`' => {
                i = skip_quoted(&chars, i);
                continue;
            }
            '(' => depth += 1,
            ')' => depth -= 1,
            c if depth == 0 && (c.is_ascii_alphabetic() || c == '_') => {
                let start = i;
                while i < chars.len() && is_word_char(chars[i]) {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                if !word.eq_ignore_ascii_case("DEFAULT") {
                    continue;
                }
                let (value, end) = default_value(&chars, i)?;
                // the value must end where the next attribute starts
                let next: String = chars[end..]
                    .iter()
                    .skip_while(|c| c.is_whitespace())
                    .take_while(|c| is_word_char(**c))
                    .collect();
                let rest_blank = chars[end..].iter().all(|c| c.is_whitespace());
                if !rest_blank && !FOLLOWERS.iter().any(|f| f.eq_ignore_ascii_case(&next)) {
                    return None;
                }
                return Some((!value.eq_ignore_ascii_case("NULL")).then_some(value));
            }
            _ => {}
        }
        i += 1;
    }
    Some(None)
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// Index just past the quoted run starting at `start` (`'…'`, `"…"` or `` `…` ``; doubled
/// quotes and, inside string literals, backslash escapes).
fn skip_quoted(chars: &[char], start: usize) -> usize {
    let q = chars[start];
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' if q != '`' => i += 2,
            c if c == q => {
                if chars.get(i + 1) == Some(&q) {
                    i += 2;
                } else {
                    return i + 1;
                }
            }
            _ => i += 1,
        }
    }
    chars.len()
}

/// Index just past the parenthesised group starting at `start`.
fn skip_group(chars: &[char], start: usize) -> usize {
    let mut depth = 0i32;
    let mut i = start;
    while i < chars.len() {
        match chars[i] {
            '\'' | '"' | '`' => {
                i = skip_quoted(chars, i);
                continue;
            }
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    chars.len()
}

/// The value after `DEFAULT` (starting at `start`) and the index after it.
fn default_value(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut i = start;
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    let from = i;
    match chars.get(i)? {
        '(' => i = skip_group(chars, i),
        '\'' | '"' => i = skip_quoted(chars, i),
        _ => {
            while i < chars.len() && (is_word_char(chars[i]) || matches!(chars[i], '.' | '+' | '-'))
            {
                i += 1;
            }
            if i == from {
                return None;
            }
            // `_utf8mb4'…'`, `_binary '…'`, `b'01'`, `x'0a'`; `CURRENT_TIMESTAMP(3)`, `uuid()`
            let mut j = i;
            if chars[from] == '_' {
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
            }
            match chars.get(j) {
                Some('\'') => i = skip_quoted(chars, j),
                Some('(') if j == i => i = skip_group(chars, i),
                _ => {}
            }
        }
    }
    Some((chars[from..i].iter().collect(), i))
}

/// Fallback for a default from `information_schema` when `SHOW CREATE TABLE` is unavailable.
/// MySQL reports literals unquoted and marks expressions `DEFAULT_GENERATED`; MariaDB quotes
/// literals and reports expressions bare.
fn mysql_default_from_meta(c: &ColumnMeta, mode: MysqlLiteralMode) -> Option<String> {
    let d = c.default_expr.as_deref()?;
    let extra = c.extra.as_deref().unwrap_or("").to_ascii_lowercase();
    let numeric = matches!(
        c.kind,
        ValueKind::Int
            | ValueKind::BigInt
            | ValueKind::Decimal
            | ValueKind::Float
            | ValueKind::Bool
    );
    let temporal = matches!(
        c.kind,
        ValueKind::Date | ValueKind::Time | ValueKind::Timestamp | ValueKind::TimestampTz
    );
    let lower = d.to_ascii_lowercase();
    let now = temporal && MYSQL_NOW.is_match(d);
    if extra.contains("default_generated") && !now {
        // expression default (8.0.13+): information_schema escapes quotes inside it
        return Some(format!("({})", d.replace("\\'", "'")));
    }
    let as_is = now || QUOTED_LITERAL.is_match(d) || d == "NULL" || FUNCTION_CALL.is_match(d);
    let bare_literal = lower.starts_with("b'")
        || lower.starts_with("x'")
        || lower.starts_with("0x")
        || (numeric && NUMERIC.is_match(d));
    Some(if as_is || bare_literal {
        d.to_string()
    } else {
        Dialect::Mysql.quote_literal_with(d, mode)
    })
}

/// Whether SQLite accepts `expr` as the default of `ALTER TABLE … ADD COLUMN`.
fn sqlite_constant(expr: &str) -> bool {
    let e = expr.trim();
    SQLITE_LITERAL.is_match(e)
        || (e.starts_with('(')
            && e.ends_with(')')
            && SQLITE_LITERAL.is_match(e[1..e.len() - 1].trim()))
}

/// SQLite reports `DEFAULT (expr)` without its parentheses; put them back for expressions.
fn sqlite_default_sql(expr: &str) -> String {
    let e = expr.trim();
    if SQLITE_LITERAL.is_match(e) || SQLITE_TIME_KEYWORD.is_match(e) || wrapped_in_parens(e) {
        e.to_string()
    } else {
        format!("({e})")
    }
}

/// Whether the whole expression is one parenthesised group.
fn wrapped_in_parens(e: &str) -> bool {
    if !e.starts_with('(') || !e.ends_with(')') {
        return false;
    }
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for (i, ch) in e.char_indices() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 && i != e.len() - 1 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

// ---------------------------------------------------------------------------------------------
// SQL text surgery (SQLite stored definitions)
// ---------------------------------------------------------------------------------------------

/// Replaces identifier tokens equal to `old` (ASCII case-insensitive, quoted or bare) with
/// `new_quoted`, leaving string literals and comments alone.
fn rename_ident(sql: &str, old: &str, new_quoted: &str) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len() + 8);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' => {
                let start = i;
                i += 1;
                while i < chars.len() {
                    if chars[i] == '\'' {
                        if chars.get(i + 1) == Some(&'\'') {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                out.extend(&chars[start..i]);
            }
            '"' | '`' | '[' => {
                let close = if c == '[' { ']' } else { c };
                let start = i;
                i += 1;
                let mut name = String::new();
                while i < chars.len() {
                    if chars[i] == close {
                        if close != ']' && chars.get(i + 1) == Some(&close) {
                            name.push(close);
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    name.push(chars[i]);
                    i += 1;
                }
                if name.eq_ignore_ascii_case(old) {
                    out.push_str(new_quoted);
                } else {
                    out.extend(&chars[start..i]);
                }
            }
            '-' if chars.get(i + 1) == Some(&'-') => {
                let start = i;
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                out.extend(&chars[start..i]);
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                let start = i;
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i = (i + 2).min(chars.len());
                out.extend(&chars[start..i]);
            }
            c if c.is_ascii_digit() => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '.') {
                    i += 1;
                }
                out.extend(&chars[start..i]);
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$')
                {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                // `X'00'` is a blob literal, not an identifier
                let blob_prefix = word.eq_ignore_ascii_case("x") && chars.get(i) == Some(&'\'');
                if !blob_prefix && word.eq_ignore_ascii_case(old) {
                    out.push_str(new_quoted);
                } else {
                    out.push_str(&word);
                }
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

fn references_ident(sql: &str, name: &str) -> bool {
    rename_ident(sql, name, "\u{0}") != sql
}

/// Index definition: renames only inside the column list / predicate.
fn rename_in_index_def(def: &str, old: &str, new_quoted: &str) -> String {
    match INDEX_DEF.captures(def).and_then(|c| c.get(5)) {
        Some(rest) => format!(
            "{}{}",
            &def[..rest.start()],
            rename_ident(rest.as_str(), old, new_quoted)
        ),
        None => rename_ident(def, old, new_quoted),
    }
}

fn index_def_references(def: &str, column: &str) -> bool {
    match INDEX_DEF.captures(def).and_then(|c| c.get(5)) {
        Some(rest) => references_ident(rest.as_str(), column),
        None => references_ident(def, column),
    }
}

/// Trigger definition: renames everywhere after the trigger's own name.
fn rename_in_trigger_def(def: &str, old: &str, new_quoted: &str) -> String {
    match TRIGGER_DEF.captures(def).and_then(|c| c.get(3)) {
        Some(rest) => format!(
            "{}{}",
            &def[..rest.start()],
            rename_ident(rest.as_str(), old, new_quoted)
        ),
        None => rename_ident(def, old, new_quoted),
    }
}

fn non_main(schema: Option<&str>) -> Option<&str> {
    schema.filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("main"))
}

/// A stored SQLite index definition re-targeted at `table` (and qualified for attached schemas).
fn replay_index(def: &str, schema: Option<&str>, table: &str) -> String {
    let d = Dialect::Sqlite;
    let Some(c) = INDEX_DEF.captures(def) else {
        return def.to_string();
    };
    let mut name = c[3].to_string();
    if let Some(s) = non_main(schema)
        && !QUALIFIED.is_match(&name)
    {
        name = format!("{}.{name}", d.quote_ident(s));
    }
    format!(
        "CREATE {}INDEX {}{name} ON {} {}",
        if c.get(1).is_some() { "UNIQUE " } else { "" },
        if c.get(2).is_some() {
            "IF NOT EXISTS "
        } else {
            ""
        },
        d.quote_ident(table),
        &c[5]
    )
}

/// A stored SQLite trigger definition, qualified for attached schemas.
fn replay_trigger(def: &str, schema: Option<&str>) -> String {
    let Some(s) = non_main(schema) else {
        return def.to_string();
    };
    match TRIGGER_DEF.captures(def) {
        Some(c) if !QUALIFIED.is_match(&c[2]) => format!(
            "{}{}.{}{}",
            &c[1],
            Dialect::Sqlite.quote_ident(s),
            &c[2],
            &c[3]
        ),
        _ => def.to_string(),
    }
}

/// Column definitions `(name, text)` of a SQLite `CREATE TABLE` statement, plus the table
/// options after the closing parenthesis (`WITHOUT ROWID`, `STRICT`).
fn split_create_table(ddl: &str) -> (Vec<(String, String)>, Option<String>) {
    let (parts, tail) = split_create_parts(ddl);
    let defs = parts
        .into_iter()
        .filter_map(|p| {
            let (name, quoted) = leading_ident(&p)?;
            let keyword = !quoted
                && matches!(
                    name.to_ascii_uppercase().as_str(),
                    "CONSTRAINT" | "PRIMARY" | "UNIQUE" | "CHECK" | "FOREIGN"
                );
            (!keyword).then_some((name, p))
        })
        .collect();
    (defs, tail)
}

/// Every element (column definition or table constraint) of a `CREATE TABLE` statement, with
/// comments blanked out, plus the table options after the closing parenthesis.
fn split_create_parts(ddl: &str) -> (Vec<String>, Option<String>) {
    let chars: Vec<char> = ddl.chars().collect();
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut started = false;
    let mut tail = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // (end of the run, whether it is a comment)
        let run = match c {
            '\'' | '"' | '`' | '[' => {
                let close = if c == '[' { ']' } else { c };
                let mut j = i + 1;
                while j < chars.len() {
                    if chars[j] == close {
                        if close != ']' && chars.get(j + 1) == Some(&close) {
                            j += 2;
                            continue;
                        }
                        j += 1;
                        break;
                    }
                    j += 1;
                }
                Some((j, false))
            }
            '-' if chars.get(i + 1) == Some(&'-') => {
                let mut j = i;
                while j < chars.len() && chars[j] != '\n' {
                    j += 1;
                }
                Some((j, true))
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                let mut j = i + 2;
                while j < chars.len() && !(chars[j] == '*' && chars.get(j + 1) == Some(&'/')) {
                    j += 1;
                }
                Some(((j + 2).min(chars.len()), true))
            }
            _ => None,
        };
        if let Some((j, comment)) = run {
            if started {
                if comment {
                    cur.push(' ');
                } else {
                    cur.extend(&chars[i..j]);
                }
            }
            i = j;
            continue;
        }
        match c {
            '(' => {
                depth += 1;
                if depth == 1 && !started {
                    started = true;
                    i += 1;
                    continue;
                }
            }
            ')' => {
                depth -= 1;
                if depth == 0 && started {
                    parts.push(cur.trim().to_string());
                    let rest: String = chars[i + 1..].iter().collect();
                    let rest = rest.trim().trim_end_matches(';').trim();
                    tail = (!rest.is_empty()).then(|| rest.to_string());
                    break;
                }
            }
            ',' if depth == 1 && started => {
                parts.push(cur.trim().to_string());
                cur.clear();
                i += 1;
                continue;
            }
            _ => {}
        }
        if started {
            cur.push(c);
        }
        i += 1;
    }
    (parts, tail)
}

/// The collation of a SQLite column definition (`COLLATE NOCASE`), ignoring anything inside
/// parentheses or string literals (CHECK expressions, defaults).
fn column_collation(def: &str) -> Option<String> {
    let top = top_level(def);
    let name = COLUMN_COLLATE.captures(&top)?.get(1)?.as_str().to_string();
    // unquote `"NOCASE"` / `[NOCASE]`
    leading_ident(&name)
        .map(|(n, _)| n)
        .filter(|n| !n.is_empty())
}

/// A column definition without parenthesised parts and string literals (quoted identifiers
/// are kept).
fn top_level(def: &str) -> String {
    let mut top = String::with_capacity(def.len());
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for ch in def.chars() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                }
                if depth == 0 && q != '\'' {
                    top.push(ch);
                }
                continue;
            }
            None => match ch {
                '\'' => {
                    quote = Some(ch);
                    continue;
                }
                '"' | '`' => quote = Some(ch),
                '[' => quote = Some(']'),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    continue;
                }
                _ => {}
            },
        }
        if depth == 0 {
            top.push(ch);
        }
    }
    top
}

/// A column definition without its leading column name.
fn after_leading_ident(def: &str) -> &str {
    let s = def.trim_start();
    let chars: Vec<char> = s.chars().collect();
    let end = match chars.first() {
        Some('"' | '`') => skip_quoted(&chars, 0),
        Some('[') => chars
            .iter()
            .position(|c| *c == ']')
            .map_or(chars.len(), |p| p + 1),
        _ => chars
            .iter()
            .position(|c| c.is_whitespace() || *c == '(')
            .unwrap_or(chars.len()),
    };
    let byte = s.char_indices().nth(end).map_or(s.len(), |(b, _)| b);
    &s[byte..]
}

/// Whether a trigger body mentions `column` (anywhere after the trigger's own name).
fn trigger_references(def: &str, column: &str) -> bool {
    match TRIGGER_DEF.captures(def).and_then(|c| c.get(3)) {
        Some(rest) => references_ident(rest.as_str(), column),
        None => references_ident(def, column),
    }
}

/// `base`, or `base2`, `base3`… whichever no name in `taken` uses (ASCII case-insensitive).
fn unique_name(base: &str, taken: &[&[String]]) -> String {
    let used = |n: &str| {
        taken
            .iter()
            .any(|list| list.iter().any(|t| t.eq_ignore_ascii_case(n)))
    };
    let mut name = base.to_string();
    let mut n = 1;
    while used(&name) {
        n += 1;
        name = format!("{base}{n}");
    }
    name
}

/// Column names of the table a SQLite rebuild creates: the physical name of every existing
/// column (renames follow the rebuild), the final name of a column added by it unless an
/// existing column still has that name, in which case it gets a temporary one.
fn rebuild_names(cols: &[Col]) -> Vec<String> {
    let finals: Vec<String> = cols.iter().map(|c| c.name.clone()).collect();
    let mut out: Vec<String> = cols
        .iter()
        .map(|c| c.source.clone().unwrap_or_default())
        .collect();
    for (i, c) in cols.iter().enumerate() {
        if c.source.is_some() {
            continue;
        }
        let clash = out
            .iter()
            .enumerate()
            .any(|(j, o)| j != i && o.eq_ignore_ascii_case(&c.name));
        out[i] = if clash {
            unique_name(&format!("{}{RENAME_SUFFIX}", c.name), &[&out, &finals])
        } else {
            c.name.clone()
        };
    }
    out
}

/// `RENAME COLUMN` steps turning the column names `current` into `finals` (same positions),
/// ordered so that no step hits a name still in use; columns swapping names go through a
/// temporary name. Both lists must be free of names equal ignoring ASCII case (SQLite's
/// comparison): a target held by a column that keeps its name could never be reached.
fn ordered_renames(
    current: &[String],
    finals: &[String],
) -> Result<Vec<(String, String)>, AppError> {
    for names in [current, finals] {
        if let Some((i, n)) = names
            .iter()
            .enumerate()
            .find(|(i, n)| names[..*i].iter().any(|o| o.eq_ignore_ascii_case(n)))
        {
            let first = names[..i]
                .iter()
                .find(|o| o.eq_ignore_ascii_case(n))
                .map_or("", String::as_str);
            return Err(invalid(format!(
                "duplicate column name \"{n}\" (same as \"{first}\"; column names are case-insensitive)"
            )));
        }
    }
    let mut names = current.to_vec();
    let mut todo: Vec<usize> = (0..names.len())
        .filter(|&i| names[i] != finals[i])
        .collect();
    let mut out = Vec::new();
    // Each column moves at most twice (to a temporary name, then to its final one).
    let mut budget = 2 * todo.len();
    while !todo.is_empty() {
        if budget == 0 {
            return Err(AppError::Internal(
                "could not order the column renames of a SQLite rebuild".into(),
            ));
        }
        budget -= 1;
        let free = |names: &[String], i: usize| {
            names
                .iter()
                .enumerate()
                .all(|(j, n)| j == i || !n.eq_ignore_ascii_case(&finals[i]))
        };
        match todo.iter().position(|&i| free(&names, i)) {
            Some(pos) => {
                let i = todo.remove(pos);
                out.push((names[i].clone(), finals[i].clone()));
                names[i] = finals[i].clone();
            }
            None => {
                let i = todo[0];
                let tmp = unique_name(&format!("{}{RENAME_SUFFIX}", names[i]), &[&names, finals]);
                out.push((names[i].clone(), tmp.clone()));
                names[i] = tmp;
            }
        }
    }
    Ok(out)
}

/// The first identifier of a definition, unquoted, and whether it was quoted.
fn leading_ident(s: &str) -> Option<(String, bool)> {
    let s = s.trim_start();
    let first = s.chars().next()?;
    if matches!(first, '"' | '`' | '[') {
        let close = if first == '[' { ']' } else { first };
        let mut name = String::new();
        let mut chars = s.chars().skip(1).peekable();
        while let Some(ch) = chars.next() {
            if ch == close {
                if close != ']' && chars.peek() == Some(&close) {
                    chars.next();
                    name.push(close);
                    continue;
                }
                return Some((name, true));
            }
            name.push(ch);
        }
        None
    } else {
        let name: String = s
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '(')
            .collect();
        (!name.is_empty()).then_some((name, false))
    }
}

// ---------------------------------------------------------------------------------------------
// CHECK constraints, index predicates (introspection helpers)
// ---------------------------------------------------------------------------------------------

/// A CHECK constraint found in a SQLite `CREATE TABLE` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedCheck {
    /// `CONSTRAINT <name>` before it, when there is one.
    name: Option<String>,
    /// `ck_<table>_<n>` for the n-th unnamed check (how it is addressed).
    synthetic: String,
    expression: String,
    /// The column whose definition holds it (column-level check).
    column: Option<String>,
}

/// Index just past the SQLite quoted run (`'…'`, `"…"`, `` `…` ``, `[…]`) starting at `start`.
/// SQLite has no backslash escapes: quotes are doubled.
fn sqlite_skip_quoted(chars: &[char], start: usize) -> usize {
    let close = if chars[start] == '[' {
        ']'
    } else {
        chars[start]
    };
    let mut i = start + 1;
    while i < chars.len() {
        if chars[i] == close {
            if close != ']' && chars.get(i + 1) == Some(&close) {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    chars.len()
}

/// Index just past the parenthesised group starting at `start` (SQLite quoting).
fn sqlite_skip_group(chars: &[char], start: usize) -> usize {
    let mut depth = 0i32;
    let mut i = start;
    while i < chars.len() {
        match chars[i] {
            '\'' | '"' | '`' | '[' => {
                i = sqlite_skip_quoted(chars, i);
                continue;
            }
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    chars.len()
}

/// The CHECK constraints at the top level of one table-element text (a column definition after
/// its name, or a table constraint), with the `CONSTRAINT` name before each.
fn scan_checks(text: &str, column: Option<&str>, out: &mut Vec<ParsedCheck>) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut pending: Option<String> = None;
    // the next token is the name of a `CONSTRAINT`
    let mut naming = false;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' | '"' | '`' | '[' => {
                let end = sqlite_skip_quoted(&chars, i);
                if naming && c != '\'' {
                    let raw: String = chars[i..end].iter().collect();
                    pending = leading_ident(&raw).map(|(n, _)| n);
                } else {
                    pending = None;
                }
                naming = false;
                i = end;
            }
            '(' => {
                i = sqlite_skip_group(&chars, i);
                pending = None;
                naming = false;
            }
            c if is_word_char(c) => {
                let start = i;
                while i < chars.len() && is_word_char(chars[i]) {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                if naming {
                    pending = Some(word);
                    naming = false;
                } else if word.eq_ignore_ascii_case("CONSTRAINT") {
                    naming = true;
                    pending = None;
                } else if word.eq_ignore_ascii_case("CHECK") {
                    let mut j = i;
                    while j < chars.len() && chars[j].is_whitespace() {
                        j += 1;
                    }
                    if chars.get(j) == Some(&'(') {
                        let end = sqlite_skip_group(&chars, j);
                        let close = if end > j + 1 && chars[end - 1] == ')' {
                            end - 1
                        } else {
                            end
                        };
                        let inner: String = chars[j + 1..close].iter().collect();
                        out.push(ParsedCheck {
                            name: pending.take(),
                            synthetic: String::new(),
                            expression: inner.trim().to_string(),
                            column: column.map(str::to_string),
                        });
                        i = end;
                    }
                    pending = None;
                } else {
                    // the name belonged to another kind of constraint
                    pending = None;
                }
            }
            _ => i += 1,
        }
    }
}

/// Every CHECK constraint of a SQLite `CREATE TABLE` statement, column-level ones first (the
/// order SQLite requires: column definitions come before table constraints).
fn parse_sqlite_checks(ddl: &str, table: &str) -> Vec<ParsedCheck> {
    let (parts, _) = split_create_parts(ddl);
    let mut out = Vec::new();
    for p in &parts {
        let Some((lead, quoted)) = leading_ident(p) else {
            continue;
        };
        let constraint = !quoted
            && matches!(
                lead.to_ascii_uppercase().as_str(),
                "CONSTRAINT" | "PRIMARY" | "UNIQUE" | "CHECK" | "FOREIGN"
            );
        if constraint {
            scan_checks(p, None, &mut out);
        } else {
            scan_checks(after_leading_ident(p), Some(&lead), &mut out);
        }
    }
    let mut n = 0;
    for c in &mut out {
        if c.name.is_none() {
            n += 1;
            c.synthetic = format!("ck_{table}_{n}");
        }
    }
    out
}

/// Columns of `columns` a check uses: its own column first, then those its expression names.
fn parsed_check_columns(p: &ParsedCheck, columns: &[String]) -> Vec<String> {
    let mut out: Vec<String> = p
        .column
        .iter()
        .filter_map(|c| columns.iter().find(|n| n.eq_ignore_ascii_case(c)).cloned())
        .collect();
    for c in check_columns(&p.expression, columns) {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// The columns (of `columns`) that `expression` mentions, compared ignoring ASCII case; string
/// literals and comments are not looked at.
pub fn check_columns(expression: &str, columns: &[String]) -> Vec<String> {
    columns
        .iter()
        .filter(|c| references_ident(expression, c))
        .cloned()
        .collect()
}

/// The CHECK constraints of a SQLite table from its `CREATE TABLE` text. Unnamed checks are
/// called `ck_<table>_<n>`.
pub fn sqlite_checks(ddl: &str, table: &str, columns: &[String]) -> Vec<CheckMeta> {
    parse_sqlite_checks(ddl, table)
        .into_iter()
        .map(|p| CheckMeta {
            columns: parsed_check_columns(&p, columns),
            name: p.name.clone().unwrap_or(p.synthetic.clone()),
            expression: p.expression,
        })
        .collect()
}

/// The expression of a check as PG's `pg_get_constraintdef` or MySQL's `CHECK_CLAUSE` spell it
/// (`CHECK ((price > 0)) NOT VALID`, `(`price` > 0)`), without `CHECK` and redundant outer
/// parentheses: `price > 0`.
pub fn check_expression(def: &str) -> String {
    let t = def.trim();
    let body = match t.get(..5) {
        Some(head) if head.eq_ignore_ascii_case("CHECK") => t[5..].trim_start(),
        _ => t,
    };
    let chars: Vec<char> = body.chars().collect();
    let mut expr = if chars.first() == Some(&'(') {
        let end = skip_group(&chars, 0);
        let close = if end > 1 && chars[end - 1] == ')' {
            end - 1
        } else {
            end
        };
        chars[1..close].iter().collect::<String>()
    } else {
        body.to_string()
    };
    loop {
        let e = expr.trim();
        if e.len() >= 2 && wrapped_in_parens(e) {
            expr = e[1..e.len() - 1].to_string();
        } else {
            return e.to_string();
        }
    }
}

/// What follows the trigger name in a `CREATE TRIGGER` statement (`AFTER INSERT ON t …`).
pub fn trigger_tail(def: &str) -> Option<&str> {
    TRIGGER_DEF.captures(def)?.get(3).map(|m| m.as_str())
}

/// The partial-index predicate of a stored `CREATE INDEX … WHERE …` statement.
pub fn index_predicate(def: &str) -> Option<String> {
    let rest = INDEX_DEF.captures(def)?.get(5)?.as_str();
    let chars: Vec<char> = rest.chars().collect();
    let end = sqlite_skip_group(&chars, 0);
    let tail: String = chars[end..].iter().collect();
    let tail = tail.trim().trim_end_matches(';').trim();
    let head = tail.get(..5)?;
    if !head.eq_ignore_ascii_case("WHERE") || !tail[5..].starts_with(char::is_whitespace) {
        return None;
    }
    non_empty(Some(&tail[5..]))
}

/// `expr` with every identifier `from` replaced by `to` (quoted), all at once: swapped names
/// (`a` → `b`, `b` → `a`) do not interfere.
fn rename_all(dialect: Dialect, expr: &str, renames: &[(String, String)]) -> String {
    let mut out = expr.to_string();
    let marks: Vec<String> = (0..renames.len())
        .map(|i| format!("\"__tabler_mark_{i}__\""))
        .collect();
    for ((from, _), mark) in renames.iter().zip(&marks) {
        out = rename_ident(&out, from, mark);
    }
    for (i, (_, to)) in renames.iter().enumerate() {
        out = rename_ident(
            &out,
            &format!("__tabler_mark_{i}__"),
            &dialect.quote_ident(to),
        );
    }
    out
}

/// PG: user-defined types a column can use (enums, domains, ranges, standalone composite
/// types) as `format_type` spells them (qualified when not on the search path).
pub const PG_USER_TYPES_SQL: &str = "SELECT format_type(t.oid, NULL)::text, \
     CASE t.typtype WHEN 'e' THEN 'enum' WHEN 'd' THEN 'domain' WHEN 'r' THEN 'range' ELSE 'composite' END \
     FROM pg_catalog.pg_type t JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace \
     WHERE (t.typtype IN ('e', 'd', 'r') OR (t.typtype = 'c' AND EXISTS \
       (SELECT 1 FROM pg_catalog.pg_class c WHERE c.oid = t.typrelid AND c.relkind = 'c'))) \
       AND n.nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast') \
       AND n.nspname NOT LIKE 'pg\\_%' \
     ORDER BY 1";

/// MySQL: storage engines that can be used (the default first).
pub const MYSQL_ENGINES_SQL: &str = "SELECT ENGINE FROM information_schema.ENGINES \
     WHERE SUPPORT IN ('YES', 'DEFAULT') ORDER BY SUPPORT = 'DEFAULT' DESC, ENGINE";

/// MySQL: `charset, collation, is default` rows, default collation of each set first.
pub const MYSQL_COLLATIONS_SQL: &str = "SELECT CHARACTER_SET_NAME, COLLATION_NAME, IS_DEFAULT \
     FROM information_schema.COLLATIONS WHERE CHARACTER_SET_NAME IS NOT NULL \
     ORDER BY CHARACTER_SET_NAME, IS_DEFAULT DESC, COLLATION_NAME";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ddl::{CheckDef, FkDef};
    use crate::types::schema::ForeignKeyMeta;

    fn t(schema: Option<&str>, name: &str) -> TableRef {
        TableRef::new(schema, name)
    }

    fn cm(name: &str, ty: &str, nullable: bool) -> ColumnMeta {
        ColumnMeta {
            name: name.into(),
            ordinal: 0,
            data_type: ty.into(),
            kind: ValueKind::Text,
            nullable,
            default_expr: None,
            is_pk: false,
            pk_ordinal: None,
            is_auto_increment: false,
            is_generated: false,
            is_unsigned: false,
            comment: None,
            enum_values: None,
            char_max_len: None,
            numeric_precision: None,
            numeric_scale: None,
            collation: None,
            array_elem_type: None,
            extra: None,
            base_type: None,
        }
    }

    fn meta(table: TableRef, columns: Vec<ColumnMeta>, pk: &[&str]) -> TableMeta {
        TableMeta {
            table,
            kind: TableKind::Table,
            columns,
            primary_key: pk.iter().map(|s| s.to_string()).collect(),
            indexes: vec![],
            foreign_keys: vec![],
            triggers: vec![],
            comment: None,
            row_estimate: None,
            ddl: None,
            checks: vec![],
            options: None,
        }
    }

    fn index(
        name: &str,
        columns: &[&str],
        unique: bool,
        primary: bool,
        def: Option<&str>,
    ) -> IndexMeta {
        IndexMeta {
            name: name.into(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
            unique,
            primary,
            method: None,
            predicate: None,
            definition: def.map(str::to_string),
            comment: None,
            include: Vec::new(),
        }
    }

    fn cdef(name: &str, ty: &str, nullable: bool) -> ColumnDef {
        ColumnDef {
            name: name.into(),
            data_type: ty.into(),
            nullable,
            default_expr: None,
            auto_increment: false,
            comment: None,
            collation: None,
            unsigned: false,
            on_update: None,
        }
    }

    fn plan(d: Dialect, meta: Option<&TableMeta>, ops: Vec<DdlOp>) -> DdlPlan {
        plan_ddl(d, meta, &ops).unwrap_or_else(|e| panic!("plan failed: {e}"))
    }

    fn invalid_input(d: Dialect, meta: Option<&TableMeta>, ops: Vec<DdlOp>) -> String {
        match plan_ddl(d, meta, &ops) {
            Err(AppError::InvalidInput(msg)) => msg,
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    // --- PostgreSQL -------------------------------------------------------------------------

    fn pg_users() -> TableMeta {
        let mut id = cm("id", "bigint", false);
        id.is_pk = true;
        id.is_auto_increment = true;
        id.extra = Some("BY DEFAULT".into());
        let mut created = cm("created_at", "timestamp with time zone", false);
        created.default_expr = Some("now()".into());
        let mut email = cm("email", "character varying(255)", false);
        email.comment = Some("login".into());
        let mut m = meta(
            t(Some("public"), "users"),
            vec![id, email, cm("name", "text", true), created],
            &["id"],
        );
        m.indexes = vec![
            index(
                "users_pkey",
                &["id"],
                true,
                true,
                Some("CREATE UNIQUE INDEX users_pkey ON public.users USING btree (id)"),
            ),
            index(
                "users_email_key",
                &["email"],
                true,
                false,
                Some("CREATE UNIQUE INDEX users_email_key ON public.users USING btree (email)"),
            ),
        ];
        m.comment = Some("app users".into());
        m
    }

    #[test]
    fn pg_add_column_with_default_not_null_and_comment() {
        let mut col = cdef("status", "varchar(20)", false);
        col.default_expr = Some("'active'".into());
        col.comment = Some("it's state".into());
        let p = plan(
            Dialect::Postgres,
            None,
            vec![DdlOp::AddColumn {
                table: t(Some("public"), "users"),
                column: col,
                after: Some("name".into()),
            }],
        );
        assert_eq!(
            p.statements,
            [
                r#"ALTER TABLE "public"."users" ADD COLUMN "status" varchar(20) NOT NULL DEFAULT 'active'"#,
                r#"COMMENT ON COLUMN "public"."users"."status" IS 'it''s state'"#,
            ]
        );
        assert!(p.transactional);
        assert_eq!(p.warnings.len(), 1, "{:?}", p.warnings);
    }

    #[test]
    fn pg_column_alterations() {
        let users = t(Some("public"), "users");
        let p = plan(
            Dialect::Postgres,
            None,
            vec![
                DdlOp::RenameColumn {
                    table: users.clone(),
                    name: "name".into(),
                    new_name: "full name".into(),
                },
                DdlOp::AlterColumnType {
                    table: users.clone(),
                    name: "age".into(),
                    new_type: "integer".into(),
                    using: Some("age::integer".into()),
                },
                DdlOp::AlterColumnType {
                    table: users.clone(),
                    name: "bio".into(),
                    new_type: "text".into(),
                    using: None,
                },
                DdlOp::AlterColumnNullable {
                    table: users.clone(),
                    name: "bio".into(),
                    nullable: false,
                },
                DdlOp::AlterColumnNullable {
                    table: users.clone(),
                    name: "age".into(),
                    nullable: true,
                },
                DdlOp::AlterColumnDefault {
                    table: users.clone(),
                    name: "age".into(),
                    default_expr: Some("18".into()),
                },
                DdlOp::AlterColumnDefault {
                    table: users.clone(),
                    name: "bio".into(),
                    default_expr: None,
                },
                DdlOp::DropColumn {
                    table: users.clone(),
                    name: "legacy".into(),
                },
                DdlOp::SetColumnComment {
                    table: users.clone(),
                    name: "age".into(),
                    comment: Some("years".into()),
                },
                DdlOp::SetColumnComment {
                    table: users.clone(),
                    name: "bio".into(),
                    comment: None,
                },
                DdlOp::SetTableComment {
                    table: users.clone(),
                    comment: Some("people".into()),
                },
                DdlOp::RenameTable {
                    table: users.clone(),
                    new_name: "members".into(),
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                r#"ALTER TABLE "public"."users" RENAME COLUMN "name" TO "full name""#,
                r#"ALTER TABLE "public"."users" ALTER COLUMN "age" TYPE integer USING age::integer"#,
                r#"ALTER TABLE "public"."users" ALTER COLUMN "bio" TYPE text"#,
                r#"ALTER TABLE "public"."users" ALTER COLUMN "bio" SET NOT NULL"#,
                r#"ALTER TABLE "public"."users" ALTER COLUMN "age" DROP NOT NULL"#,
                r#"ALTER TABLE "public"."users" ALTER COLUMN "age" SET DEFAULT 18"#,
                r#"ALTER TABLE "public"."users" ALTER COLUMN "bio" DROP DEFAULT"#,
                r#"ALTER TABLE "public"."users" DROP COLUMN "legacy""#,
                r#"COMMENT ON COLUMN "public"."users"."age" IS 'years'"#,
                r#"COMMENT ON COLUMN "public"."users"."bio" IS NULL"#,
                r#"COMMENT ON TABLE "public"."users" IS 'people'"#,
                r#"ALTER TABLE "public"."users" RENAME TO "members""#,
            ]
        );
        assert!(p.warnings.is_empty());
    }

    #[test]
    fn pg_indexes_and_foreign_keys() {
        let users = t(Some("public"), "users");
        let p = plan(
            Dialect::Postgres,
            None,
            vec![
                DdlOp::AddIndex {
                    table: users.clone(),
                    index: IndexDef {
                        name: "users_email_live".into(),
                        columns: vec![
                            "email".into(),
                            "lower(name)".into(),
                            "created_at desc".into(),
                        ],
                        unique: true,
                        method: Some("btree".into()),
                        predicate: Some("deleted_at IS NULL".into()),
                        comment: None,
                        include: vec![],
                    },
                },
                DdlOp::DropIndex {
                    table: users.clone(),
                    name: "users_old_idx".into(),
                },
                DdlOp::AddForeignKey {
                    table: t(Some("public"), "posts"),
                    fk: FkDef {
                        name: Some("posts_user_fk".into()),
                        columns: vec!["user_id".into()],
                        ref_table: users.clone(),
                        ref_columns: vec!["id".into()],
                        on_update: Some("no action".into()),
                        on_delete: Some("cascade".into()),
                    },
                },
                DdlOp::AddForeignKey {
                    table: t(None, "posts"),
                    fk: FkDef {
                        name: None,
                        columns: vec!["a".into(), "b".into()],
                        ref_table: t(None, "pairs"),
                        ref_columns: vec!["x".into(), "y".into()],
                        on_update: Some("SET NULL".into()),
                        on_delete: None,
                    },
                },
                DdlOp::DropForeignKey {
                    table: t(Some("public"), "posts"),
                    name: "posts_user_fk".into(),
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                r#"CREATE UNIQUE INDEX "users_email_live" ON "public"."users" USING btree ("email", lower(name), "created_at" DESC) WHERE deleted_at IS NULL"#,
                r#"DROP INDEX "public"."users_old_idx""#,
                r#"ALTER TABLE "public"."posts" ADD CONSTRAINT "posts_user_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users" ("id") ON DELETE CASCADE"#,
                r#"ALTER TABLE "posts" ADD FOREIGN KEY ("a", "b") REFERENCES "pairs" ("x", "y") ON UPDATE SET NULL"#,
                r#"ALTER TABLE "public"."posts" DROP CONSTRAINT "posts_user_fk""#,
            ]
        );
    }

    #[test]
    fn pg_primary_keys_use_the_constraint_name_from_metadata() {
        let users = pg_users();
        let p = plan(
            Dialect::Postgres,
            Some(&users),
            vec![
                DdlOp::DropPrimaryKey {
                    table: t(None, "users"),
                },
                DdlOp::AddPrimaryKey {
                    table: t(None, "users"),
                    columns: vec!["id".into(), "email".into()],
                },
                // the key added above is addressed by PG's default name
                DdlOp::DropPrimaryKey {
                    table: t(None, "users"),
                },
                DdlOp::DropIndex {
                    table: t(None, "users"),
                    name: "users_email_key".into(),
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                r#"ALTER TABLE "public"."users" DROP CONSTRAINT "users_pkey""#,
                r#"ALTER TABLE "public"."users" ADD PRIMARY KEY ("id", "email")"#,
                r#"ALTER TABLE "public"."users" DROP CONSTRAINT "users_pkey""#,
                r#"DROP INDEX "public"."users_email_key""#,
            ]
        );
        let msg = invalid_input(
            Dialect::Postgres,
            None,
            vec![DdlOp::DropPrimaryKey {
                table: t(None, "users"),
            }],
        );
        assert!(msg.contains("metadata"), "{msg}");
        // A primary key dropped through its index name becomes DROP CONSTRAINT.
        let p = plan(
            Dialect::Postgres,
            Some(&users),
            vec![DdlOp::DropIndex {
                table: t(None, "users"),
                name: "users_pkey".into(),
            }],
        );
        assert_eq!(
            p.statements,
            [r#"ALTER TABLE "public"."users" DROP CONSTRAINT "users_pkey""#]
        );
    }

    #[test]
    fn pg_create_table_with_composite_key_and_identity() {
        let mut id = cdef("id", "bigint", false);
        id.auto_increment = true;
        let mut tenant = cdef("tenant_id", "integer", false);
        tenant.default_expr = Some("0".into());
        let mut note = cdef("note", "text", true);
        note.comment = Some("free text".into());
        note.collation = Some("C".into());
        let mut serial = cdef("seq", "bigserial", false);
        serial.auto_increment = true;
        let mut bad = cdef("label", "text", true);
        bad.auto_increment = true;
        bad.unsigned = true;
        let p = plan(
            Dialect::Postgres,
            None,
            vec![DdlOp::CreateTable {
                table: t(Some("app"), "events"),
                columns: vec![id, tenant, note, serial, bad],
                primary_key: vec!["tenant_id".into(), "id".into()],
                if_not_exists: true,
            }],
        );
        assert_eq!(
            p.statements,
            [
                "CREATE TABLE IF NOT EXISTS \"app\".\"events\" (\n  \"id\" bigint GENERATED BY DEFAULT AS IDENTITY NOT NULL,\n  \"tenant_id\" integer NOT NULL DEFAULT 0,\n  \"note\" text COLLATE \"C\",\n  \"seq\" bigserial NOT NULL,\n  \"label\" text,\n  PRIMARY KEY (\"tenant_id\", \"id\")\n)",
                r#"COMMENT ON COLUMN "app"."events"."note" IS 'free text'"#,
            ]
        );
        assert_eq!(p.warnings.len(), 2, "{:?}", p.warnings);
    }

    #[test]
    fn pg_drop_truncate_and_raw_sql() {
        let users = t(Some("public"), "users");
        let p = plan(
            Dialect::Postgres,
            None,
            vec![
                DdlOp::TruncateTable {
                    table: users.clone(),
                    restart_identity: true,
                    cascade: true,
                },
                DdlOp::TruncateTable {
                    table: users.clone(),
                    restart_identity: false,
                    cascade: false,
                },
                DdlOp::DropTable {
                    table: users.clone(),
                    if_exists: true,
                    cascade: true,
                },
                DdlOp::RawSql {
                    sql: "  VACUUM users; ".into(),
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                r#"TRUNCATE TABLE "public"."users" RESTART IDENTITY CASCADE"#,
                r#"TRUNCATE TABLE "public"."users""#,
                r#"DROP TABLE IF EXISTS "public"."users" CASCADE"#,
                "VACUUM users",
            ]
        );
        assert!(!p.transactional, "VACUUM cannot run in a transaction block");
    }

    #[test]
    fn rejects_bad_input() {
        let users = t(None, "users");
        let fk = |on_delete: &str| DdlOp::AddForeignKey {
            table: users.clone(),
            fk: FkDef {
                name: None,
                columns: vec!["a".into()],
                ref_table: t(None, "r"),
                ref_columns: vec!["id".into()],
                on_update: None,
                on_delete: Some(on_delete.into()),
            },
        };
        assert!(
            invalid_input(Dialect::Postgres, None, vec![fk("DROP TABLE x")])
                .contains("referential action")
        );
        let idx = DdlOp::AddIndex {
            table: users.clone(),
            index: IndexDef {
                name: "i".into(),
                columns: vec!["a".into()],
                unique: false,
                method: Some("btree; drop".into()),
                predicate: None,
                comment: None,
                include: vec![],
            },
        };
        assert!(invalid_input(Dialect::Postgres, None, vec![idx]).contains("index method"));
        let mut col = cdef("c", "text", true);
        col.collation = Some("x\" y".into());
        assert!(
            invalid_input(
                Dialect::Postgres,
                None,
                vec![DdlOp::AddColumn {
                    table: users.clone(),
                    column: col,
                    after: None
                }]
            )
            .contains("collation")
        );
        assert!(
            invalid_input(
                Dialect::Mysql,
                None,
                vec![DdlOp::CreateTable {
                    table: users.clone(),
                    columns: vec![],
                    primary_key: vec![],
                    if_not_exists: false
                }]
            )
            .contains("at least one column")
        );
        assert!(
            invalid_input(
                Dialect::Mysql,
                None,
                vec![DdlOp::CreateTable {
                    table: users.clone(),
                    columns: vec![cdef("a", "int", false)],
                    primary_key: vec!["b".into()],
                    if_not_exists: false
                }]
            )
            .contains("\"b\"")
        );
        assert!(
            invalid_input(
                Dialect::Mysql,
                None,
                vec![DdlOp::RawSql { sql: "  ".into() }]
            )
            .contains("empty")
        );
    }

    // --- MySQL ------------------------------------------------------------------------------

    fn my_items() -> TableMeta {
        let mut id = cm("id", "int unsigned", false);
        id.kind = ValueKind::Int;
        id.is_pk = true;
        id.is_unsigned = true;
        id.is_auto_increment = true;
        id.extra = Some("auto_increment".into());
        let mut qty = cm("qty", "int unsigned", false);
        qty.kind = ValueKind::Int;
        qty.is_unsigned = true;
        qty.default_expr = Some("0".into());
        qty.comment = Some("how many".into());
        let mut name = cm("name", "varchar(50)", true);
        name.collation = Some("utf8mb4_bin".into());
        name.default_expr = Some("it's".into());
        let mut touched = cm("touched_at", "timestamp(3)", true);
        touched.kind = ValueKind::Timestamp;
        touched.default_expr = Some("CURRENT_TIMESTAMP(3)".into());
        touched.extra = Some("DEFAULT_GENERATED on update CURRENT_TIMESTAMP(3)".into());
        let mut m = meta(
            t(Some("shop"), "items"),
            vec![id, qty, name, touched],
            &["id"],
        );
        m.indexes = vec![index("PRIMARY", &["id"], true, true, None)];
        m
    }

    #[test]
    fn mysql_add_column_with_position_and_attributes() {
        let mut col = cdef("price", "decimal(10,2)", false);
        col.unsigned = true;
        col.default_expr = Some("0.00".into());
        col.comment = Some("unit price".into());
        let mut first = cdef("code", "varchar(8)", true);
        first.collation = Some("ascii_bin".into());
        let p = plan(
            Dialect::Mysql,
            None,
            vec![
                DdlOp::AddColumn {
                    table: t(Some("shop"), "items"),
                    column: col,
                    after: Some("qty".into()),
                },
                DdlOp::AddColumn {
                    table: t(Some("shop"), "items"),
                    column: first,
                    after: Some(String::new()),
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                "ALTER TABLE `shop`.`items` ADD COLUMN `price` decimal(10,2) UNSIGNED NOT NULL DEFAULT 0.00 COMMENT 'unit price' AFTER `qty`",
                "ALTER TABLE `shop`.`items` ADD COLUMN `code` varchar(8) COLLATE ascii_bin NULL FIRST",
            ]
        );
        assert!(!p.transactional);
    }

    #[test]
    fn mysql_modify_merges_the_full_definition() {
        let items = my_items();
        let tbl = t(None, "items");
        let p = plan(
            Dialect::Mysql,
            Some(&items),
            vec![
                DdlOp::AlterColumnType {
                    table: tbl.clone(),
                    name: "id".into(),
                    new_type: "bigint".into(),
                    using: None,
                },
                DdlOp::AlterColumnType {
                    table: tbl.clone(),
                    name: "qty".into(),
                    new_type: "bigint".into(),
                    using: Some("x".into()),
                },
                DdlOp::AlterColumnNullable {
                    table: tbl.clone(),
                    name: "qty".into(),
                    nullable: true,
                },
                DdlOp::SetColumnComment {
                    table: tbl.clone(),
                    name: "name".into(),
                    comment: Some("label".into()),
                },
                DdlOp::AlterColumnType {
                    table: tbl.clone(),
                    name: "name".into(),
                    new_type: "int".into(),
                    using: None,
                },
                DdlOp::AlterColumnNullable {
                    table: tbl.clone(),
                    name: "touched_at".into(),
                    nullable: false,
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                "ALTER TABLE `shop`.`items` MODIFY COLUMN `id` bigint UNSIGNED NOT NULL AUTO_INCREMENT",
                "ALTER TABLE `shop`.`items` MODIFY COLUMN `qty` bigint UNSIGNED NOT NULL DEFAULT 0 COMMENT 'how many'",
                // the second change to qty sees the first one
                "ALTER TABLE `shop`.`items` MODIFY COLUMN `qty` bigint UNSIGNED NULL DEFAULT 0 COMMENT 'how many'",
                "ALTER TABLE `shop`.`items` MODIFY COLUMN `name` varchar(50) COLLATE utf8mb4_bin NULL DEFAULT 'it''s' COMMENT 'label'",
                // a non-text type drops the collation and keeps the (quoted) default
                "ALTER TABLE `shop`.`items` MODIFY COLUMN `name` int NULL DEFAULT 'it''s' COMMENT 'label'",
                "ALTER TABLE `shop`.`items` MODIFY COLUMN `touched_at` timestamp(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3)",
            ]
        );
        assert_eq!(p.warnings.len(), 1, "{:?}", p.warnings);
        // An explicit signedness wins over the kept UNSIGNED.
        let p = plan(
            Dialect::Mysql,
            Some(&items),
            vec![DdlOp::AlterColumnType {
                table: tbl.clone(),
                name: "qty".into(),
                new_type: "int signed".into(),
                using: None,
            }],
        );
        assert_eq!(
            p.statements,
            [
                "ALTER TABLE `shop`.`items` MODIFY COLUMN `qty` int signed NOT NULL DEFAULT 0 COMMENT 'how many'"
            ]
        );
        // MODIFY needs metadata, and the column must exist.
        let msg = invalid_input(
            Dialect::Mysql,
            None,
            vec![DdlOp::AlterColumnNullable {
                table: tbl.clone(),
                name: "qty".into(),
                nullable: true,
            }],
        );
        assert!(msg.contains("metadata"), "{msg}");
        let msg = invalid_input(
            Dialect::Mysql,
            Some(&items),
            vec![DdlOp::AlterColumnNullable {
                table: tbl.clone(),
                name: "nope".into(),
                nullable: true,
            }],
        );
        assert!(msg.contains("\"nope\""), "{msg}");
    }

    #[test]
    fn mysql_modify_sees_earlier_ops_of_the_batch() {
        let items = my_items();
        let tbl = t(None, "items");
        let mut col = cdef("sku", "varchar(20)", true);
        col.comment = Some("stock unit".into());
        let p = plan(
            Dialect::Mysql,
            Some(&items),
            vec![
                DdlOp::AddColumn {
                    table: tbl.clone(),
                    column: col,
                    after: None,
                },
                DdlOp::RenameColumn {
                    table: tbl.clone(),
                    name: "sku".into(),
                    new_name: "code".into(),
                },
                DdlOp::AlterColumnNullable {
                    table: tbl.clone(),
                    name: "code".into(),
                    nullable: false,
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                "ALTER TABLE `shop`.`items` ADD COLUMN `sku` varchar(20) NULL COMMENT 'stock unit'",
                "ALTER TABLE `shop`.`items` RENAME COLUMN `sku` TO `code`",
                "ALTER TABLE `shop`.`items` MODIFY COLUMN `code` varchar(20) NOT NULL COMMENT 'stock unit'",
            ]
        );
    }

    #[test]
    fn mysql_other_ops() {
        let items = t(Some("shop"), "items");
        let mut id = cdef("id", "bigint", false);
        id.auto_increment = true;
        id.unsigned = true;
        let p = plan(
            Dialect::Mysql,
            None,
            vec![
                DdlOp::CreateTable {
                    table: items.clone(),
                    columns: vec![id, cdef("name", "varchar(50)", true)],
                    primary_key: vec!["id".into()],
                    if_not_exists: false,
                },
                DdlOp::AlterColumnDefault {
                    table: items.clone(),
                    name: "name".into(),
                    default_expr: Some("'n/a'".into()),
                },
                DdlOp::AlterColumnDefault {
                    table: items.clone(),
                    name: "name".into(),
                    default_expr: None,
                },
                DdlOp::SetTableComment {
                    table: items.clone(),
                    comment: Some("stock".into()),
                },
                DdlOp::SetTableComment {
                    table: items.clone(),
                    comment: None,
                },
                DdlOp::AddIndex {
                    table: items.clone(),
                    index: IndexDef {
                        name: "items_name".into(),
                        columns: vec!["name(10)".into()],
                        unique: true,
                        method: Some("btree".into()),
                        predicate: Some("x > 1".into()),
                        comment: None,
                        include: vec![],
                    },
                },
                DdlOp::AddIndex {
                    table: items.clone(),
                    index: IndexDef {
                        name: "items_ft".into(),
                        columns: vec!["name".into()],
                        unique: false,
                        method: Some("fulltext".into()),
                        predicate: None,
                        comment: None,
                        include: vec![],
                    },
                },
                DdlOp::DropIndex {
                    table: items.clone(),
                    name: "items_name".into(),
                },
                DdlOp::DropIndex {
                    table: items.clone(),
                    name: "PRIMARY".into(),
                },
                DdlOp::AddForeignKey {
                    table: items.clone(),
                    fk: FkDef {
                        name: Some("items_cat".into()),
                        columns: vec!["cat_id".into()],
                        ref_table: t(Some("shop"), "cats"),
                        ref_columns: vec!["id".into()],
                        on_update: Some("cascade".into()),
                        on_delete: Some("set null".into()),
                    },
                },
                DdlOp::DropForeignKey {
                    table: items.clone(),
                    name: "items_cat".into(),
                },
                DdlOp::AddPrimaryKey {
                    table: items.clone(),
                    columns: vec!["id".into()],
                },
                DdlOp::DropPrimaryKey {
                    table: items.clone(),
                },
                DdlOp::RenameTable {
                    table: items.clone(),
                    new_name: "goods".into(),
                },
                DdlOp::TruncateTable {
                    table: t(Some("shop"), "goods"),
                    restart_identity: true,
                    cascade: false,
                },
                DdlOp::DropTable {
                    table: t(Some("shop"), "goods"),
                    if_exists: true,
                    cascade: false,
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                "CREATE TABLE `shop`.`items` (\n  `id` bigint UNSIGNED NOT NULL AUTO_INCREMENT,\n  `name` varchar(50) NULL,\n  PRIMARY KEY (`id`)\n) ENGINE=InnoDB",
                "ALTER TABLE `shop`.`items` ALTER COLUMN `name` SET DEFAULT 'n/a'",
                "ALTER TABLE `shop`.`items` ALTER COLUMN `name` DROP DEFAULT",
                "ALTER TABLE `shop`.`items` COMMENT = 'stock'",
                "ALTER TABLE `shop`.`items` COMMENT = ''",
                "CREATE UNIQUE INDEX `items_name` ON `shop`.`items` (`name`(10)) USING BTREE",
                "CREATE FULLTEXT INDEX `items_ft` ON `shop`.`items` (`name`)",
                "DROP INDEX `items_name` ON `shop`.`items`",
                "ALTER TABLE `shop`.`items` DROP PRIMARY KEY",
                "ALTER TABLE `shop`.`items` ADD CONSTRAINT `items_cat` FOREIGN KEY (`cat_id`) REFERENCES `shop`.`cats` (`id`) ON UPDATE CASCADE ON DELETE SET NULL",
                "ALTER TABLE `shop`.`items` DROP FOREIGN KEY `items_cat`",
                "ALTER TABLE `shop`.`items` ADD PRIMARY KEY (`id`)",
                "ALTER TABLE `shop`.`items` DROP PRIMARY KEY",
                "RENAME TABLE `shop`.`items` TO `shop`.`goods`",
                "TRUNCATE TABLE `shop`.`goods`",
                "DROP TABLE IF EXISTS `shop`.`goods`",
            ]
        );
        assert_eq!(p.warnings.len(), 1, "{:?}", p.warnings);
    }

    #[test]
    fn mysql_defaults_from_information_schema() {
        let col = |ty: &str, kind: ValueKind, default: &str, extra: &str| {
            let mut c = cm("c", ty, true);
            c.kind = kind;
            c.default_expr = Some(default.into());
            c.extra = Some(extra.into());
            mysql_default_from_meta(&c, MysqlLiteralMode::default()).unwrap()
        };
        assert_eq!(col("int", ValueKind::Int, "0", ""), "0");
        assert_eq!(col("decimal(5,2)", ValueKind::Decimal, "1.50", ""), "1.50");
        assert_eq!(col("varchar(5)", ValueKind::Text, "", ""), "''");
        assert_eq!(col("varchar(5)", ValueKind::Text, "12", ""), "'12'");
        assert_eq!(col("varchar(5)", ValueKind::Text, "it's", ""), "'it''s'");
        assert_eq!(col("bit(1)", ValueKind::Bit, "b'1'", ""), "b'1'");
        assert_eq!(
            col(
                "datetime",
                ValueKind::Timestamp,
                "CURRENT_TIMESTAMP",
                "DEFAULT_GENERATED"
            ),
            "CURRENT_TIMESTAMP"
        );
        assert_eq!(
            col("char(36)", ValueKind::Text, "uuid()", "DEFAULT_GENERATED"),
            "(uuid())"
        );
        assert_eq!(
            col(
                "varchar(9)",
                ValueKind::Text,
                "concat(_latin1\\'a\\',_latin1\\'b\\')",
                "DEFAULT_GENERATED"
            ),
            "(concat(_latin1'a',_latin1'b'))"
        );
        // MariaDB quotes literals itself and reports expressions bare.
        assert_eq!(col("varchar(5)", ValueKind::Text, "'x'", ""), "'x'");
        assert_eq!(
            col("datetime", ValueKind::Timestamp, "current_timestamp()", ""),
            "current_timestamp()"
        );
        assert_eq!(col("varchar(5)", ValueKind::Text, "NULL", ""), "NULL");
        // MySQL string defaults that look like something else stay strings
        assert_eq!(
            col("varchar(30)", ValueKind::Text, "TBD (later)", ""),
            "'TBD (later)'"
        );
        assert_eq!(
            col("varchar(30)", ValueKind::Text, "'quoted", ""),
            "'''quoted'"
        );
        assert_eq!(col("varchar(30)", ValueKind::Text, "now", ""), "'now'");
        assert_eq!(
            col("varchar(30)", ValueKind::Text, "localtime", ""),
            "'localtime'"
        );
        assert_eq!(
            col("varchar(30)", ValueKind::Text, "CURRENT_TIMESTAMP", ""),
            "'CURRENT_TIMESTAMP'"
        );
    }

    #[test]
    fn mysql_reads_defaults_from_show_create_lines() {
        let d = |line: &str| mysql_default_from_ddl(line);
        let some = |s: &str| Some(Some(s.to_string()));
        assert_eq!(d("`a` int DEFAULT NULL"), Some(None));
        assert_eq!(d("`a` int NOT NULL"), Some(None));
        assert_eq!(d("`a` char(36) DEFAULT (uuid())"), some("(uuid())"));
        assert_eq!(
            d("`a` varchar(9) DEFAULT (concat(_utf8mb4'a)',_utf8mb4'b'))"),
            some("(concat(_utf8mb4'a)',_utf8mb4'b'))")
        );
        assert_eq!(
            d("`a` varbinary(3) DEFAULT _binary 'ab'"),
            some("_binary 'ab'")
        );
        assert_eq!(d("`a` bit(1) DEFAULT b'1'"), some("b'1'"));
        assert_eq!(
            d("`a` datetime(3) DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3)"),
            some("CURRENT_TIMESTAMP(3)")
        );
        assert_eq!(
            d("`a` timestamp NOT NULL DEFAULT current_timestamp() ON UPDATE current_timestamp()"),
            some("current_timestamp()")
        );
        assert_eq!(d("`a` decimal(5,2) DEFAULT '-1.50'"), some("'-1.50'"));
        assert_eq!(
            d("`default` varchar(9) NOT NULL DEFAULT 'it''s' COMMENT 'DEFAULT x'"),
            some("'it''s'")
        );
        assert_eq!(d("`a` int DEFAULT -1"), some("-1"));
        // anything not followed by a known attribute is left to the fallback
        assert_eq!(d("`a` int DEFAULT 1 + 1"), None);
        assert_eq!(d("KEY `k` (`a`)"), None);
    }

    #[test]
    fn mysql_modify_restates_defaults_from_show_create_table() {
        let col = |name: &str, default: &str| {
            let mut c = cm(name, "varchar(30)", false);
            c.default_expr = Some(default.into());
            c.extra = Some(String::new());
            c
        };
        let mut n = cm("n", "int", false);
        n.kind = ValueKind::Int;
        n.default_expr = Some("0".into());
        // what introspection reports for a DEFAULT CURRENT_TIMESTAMP column
        let mut ts = cm("ts", "timestamp", false);
        ts.kind = ValueKind::Timestamp;
        ts.is_generated = true;
        ts.default_expr = Some("CURRENT_TIMESTAMP".into());
        ts.extra = Some("DEFAULT_GENERATED on update CURRENT_TIMESTAMP".into());
        let mut m = meta(
            t(Some("db"), "d"),
            vec![
                col("c1", "TBD (later)"),
                col("c2", "'quoted"),
                col("c3", "now"),
                col("c4", "NULL"),
                col("c5", "localtime"),
                n,
                ts,
            ],
            &[],
        );
        m.ddl = Some(
            "CREATE TABLE `d` (\n  `c1` varchar(30) NOT NULL DEFAULT 'TBD (later)',\n  `c2` varchar(30) NOT NULL DEFAULT '''quoted',\n  `c3` varchar(30) NOT NULL DEFAULT 'now',\n  `c4` varchar(30) NOT NULL DEFAULT 'NULL',\n  `c5` varchar(30) NOT NULL DEFAULT 'localtime' COMMENT 'x, y',\n  `n` int NOT NULL DEFAULT '0',\n  `ts` timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,\n  KEY `k` (`c1`)\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4"
                .into(),
        );
        let ops = ["c1", "c2", "c3", "c4", "c5", "n", "ts"]
            .iter()
            .map(|c| DdlOp::SetColumnComment {
                table: t(None, "d"),
                name: c.to_string(),
                comment: Some("c".into()),
            })
            .collect();
        let p = plan(Dialect::Mysql, Some(&m), ops);
        assert_eq!(
            p.statements,
            [
                "ALTER TABLE `db`.`d` MODIFY COLUMN `c1` varchar(30) NOT NULL DEFAULT 'TBD (later)' COMMENT 'c'",
                "ALTER TABLE `db`.`d` MODIFY COLUMN `c2` varchar(30) NOT NULL DEFAULT '''quoted' COMMENT 'c'",
                "ALTER TABLE `db`.`d` MODIFY COLUMN `c3` varchar(30) NOT NULL DEFAULT 'now' COMMENT 'c'",
                "ALTER TABLE `db`.`d` MODIFY COLUMN `c4` varchar(30) NOT NULL DEFAULT 'NULL' COMMENT 'c'",
                "ALTER TABLE `db`.`d` MODIFY COLUMN `c5` varchar(30) NOT NULL DEFAULT 'localtime' COMMENT 'c'",
                "ALTER TABLE `db`.`d` MODIFY COLUMN `n` int NOT NULL DEFAULT 0 COMMENT 'c'",
                "ALTER TABLE `db`.`d` MODIFY COLUMN `ts` timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT 'c'",
            ]
        );
    }

    #[test]
    fn mysql_expression_defaults_are_not_generated_columns() {
        // introspection flags DEFAULT_GENERATED columns as generated
        let mut ts = cm("ts", "timestamp", false);
        ts.kind = ValueKind::Timestamp;
        ts.is_generated = true;
        ts.default_expr = Some("CURRENT_TIMESTAMP".into());
        ts.extra = Some("DEFAULT_GENERATED on update CURRENT_TIMESTAMP".into());
        let mut u = cm("u", "char(36)", true);
        u.is_generated = true;
        u.default_expr = Some("uuid()".into());
        u.extra = Some("DEFAULT_GENERATED".into());
        let mut g = cm("g", "int", true);
        g.kind = ValueKind::Int;
        g.is_generated = true;
        g.extra = Some("VIRTUAL GENERATED".into());
        let m = meta(t(Some("db"), "t"), vec![ts, u, g], &[]);
        let tbl = t(None, "t");
        let p = plan(
            Dialect::Mysql,
            Some(&m),
            vec![
                DdlOp::AlterColumnNullable {
                    table: tbl.clone(),
                    name: "ts".into(),
                    nullable: true,
                },
                DdlOp::AlterColumnType {
                    table: tbl.clone(),
                    name: "u".into(),
                    new_type: "varchar(36)".into(),
                    using: None,
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                "ALTER TABLE `db`.`t` MODIFY COLUMN `ts` timestamp NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP",
                "ALTER TABLE `db`.`t` MODIFY COLUMN `u` varchar(36) NULL DEFAULT (uuid())",
            ]
        );
        let err = plan_ddl(
            Dialect::Mysql,
            Some(&m),
            &[DdlOp::AlterColumnNullable {
                table: tbl.clone(),
                name: "g".into(),
                nullable: false,
            }],
        )
        .unwrap_err();
        assert!(matches!(err, AppError::Unsupported(_)), "{err:?}");
    }

    #[test]
    fn mysql_set_default_to_an_expression() {
        let items = my_items();
        let set = |name: &str, e: &str| DdlOp::AlterColumnDefault {
            table: t(None, "items"),
            name: name.into(),
            default_expr: Some(e.into()),
        };
        // ALTER COLUMN … SET DEFAULT rejects CURRENT_TIMESTAMP and bare calls: MODIFY instead
        let p = plan(
            Dialect::Mysql,
            Some(&items),
            vec![
                set("touched_at", "now(3)"),
                set("name", "uuid()"),
                set("qty", "7"),
                set("name", "'x'"),
            ],
        );
        assert_eq!(
            p.statements,
            [
                "ALTER TABLE `shop`.`items` MODIFY COLUMN `touched_at` timestamp(3) NULL DEFAULT now(3) ON UPDATE CURRENT_TIMESTAMP(3)",
                "ALTER TABLE `shop`.`items` MODIFY COLUMN `name` varchar(50) COLLATE utf8mb4_bin NULL DEFAULT (uuid())",
                "ALTER TABLE `shop`.`items` ALTER COLUMN `qty` SET DEFAULT 7",
                "ALTER TABLE `shop`.`items` ALTER COLUMN `name` SET DEFAULT 'x'",
            ]
        );
        // without metadata the expression is parenthesised (MySQL 8.0.13+)
        let p = plan(
            Dialect::Mysql,
            None,
            vec![
                set("touched_at", "CURRENT_TIMESTAMP"),
                set("name", "(uuid())"),
            ],
        );
        assert_eq!(
            p.statements,
            [
                "ALTER TABLE `items` ALTER COLUMN `touched_at` SET DEFAULT (CURRENT_TIMESTAMP)",
                "ALTER TABLE `items` ALTER COLUMN `name` SET DEFAULT (uuid())",
            ]
        );
    }

    // --- SQLite -----------------------------------------------------------------------------

    /// `items(id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, qty INT DEFAULT 0,
    /// note TEXT)` with an index on name and an update trigger.
    fn lite_items() -> TableMeta {
        let mut id = cm("id", "INTEGER", false);
        id.is_pk = true;
        id.is_auto_increment = true;
        id.extra = Some("AUTOINCREMENT".into());
        let mut qty = cm("qty", "INT", true);
        qty.default_expr = Some("0".into());
        let mut stamp = cm("stamp", "TEXT", true);
        stamp.default_expr = Some("datetime('now')".into());
        let mut m = meta(
            t(Some("main"), "items"),
            vec![
                id,
                cm("name", "TEXT", false),
                qty,
                cm("note", "TEXT", true),
                stamp,
            ],
            &["id"],
        );
        m.indexes = vec![index(
            "items_name",
            &["name"],
            false,
            false,
            Some("CREATE INDEX items_name ON items(name)"),
        )];
        m.triggers = vec![TriggerMeta {
            name: "items_touch".into(),
            timing: Some("AFTER".into()),
            events: vec!["UPDATE".into()],
            enabled: true,
            definition: "CREATE TRIGGER items_touch AFTER UPDATE ON items BEGIN UPDATE items SET note = 'x' WHERE id = NEW.id; END".into(),
        }];
        m.ddl = Some("CREATE TABLE items (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, qty INT DEFAULT 0, note TEXT, stamp TEXT DEFAULT (datetime('now')))".into());
        m
    }

    const LITE_BODY: &str = "  \"id\" INTEGER PRIMARY KEY AUTOINCREMENT,\n  \"name\" TEXT NOT NULL,\n  \"qty\" BIGINT DEFAULT 0,\n  \"note\" TEXT,\n  \"stamp\" TEXT DEFAULT (datetime('now'))";

    #[test]
    fn sqlite_simple_ops_run_in_place() {
        let items = lite_items();
        let tbl = t(None, "items");
        let mut col = cdef("flag", "INTEGER", false);
        col.default_expr = Some("1".into());
        let p = plan(
            Dialect::Sqlite,
            Some(&items),
            vec![
                DdlOp::AddColumn {
                    table: tbl.clone(),
                    column: col,
                    after: None,
                },
                DdlOp::RenameColumn {
                    table: tbl.clone(),
                    name: "note".into(),
                    new_name: "remark".into(),
                },
                DdlOp::DropColumn {
                    table: tbl.clone(),
                    name: "remark".into(),
                },
                DdlOp::AddIndex {
                    table: tbl.clone(),
                    index: IndexDef {
                        name: "items_qty".into(),
                        columns: vec!["qty".into()],
                        unique: true,
                        method: None,
                        predicate: Some("qty > 0".into()),
                        comment: None,
                        include: vec![],
                    },
                },
                DdlOp::DropIndex {
                    table: tbl.clone(),
                    name: "items_qty".into(),
                },
                DdlOp::SetColumnComment {
                    table: tbl.clone(),
                    name: "qty".into(),
                    comment: Some("x".into()),
                },
                DdlOp::TruncateTable {
                    table: tbl.clone(),
                    restart_identity: true,
                    cascade: false,
                },
                DdlOp::RenameTable {
                    table: tbl.clone(),
                    new_name: "goods".into(),
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                r#"ALTER TABLE "main"."items" ADD COLUMN "flag" INTEGER NOT NULL DEFAULT 1"#,
                r#"ALTER TABLE "main"."items" RENAME COLUMN "note" TO "remark""#,
                r#"ALTER TABLE "main"."items" DROP COLUMN "remark""#,
                r#"CREATE UNIQUE INDEX "main"."items_qty" ON "items" ("qty") WHERE qty > 0"#,
                r#"DROP INDEX "main"."items_qty""#,
                r#"DELETE FROM "main"."items""#,
                r#"DELETE FROM "main"."sqlite_sequence" WHERE name = 'items'"#,
                r#"ALTER TABLE "main"."items" RENAME TO "goods""#,
            ]
        );
        assert!(p.transactional);
        assert_eq!(p.warnings, [SQLITE_NO_COMMENTS]);
        // Without AUTOINCREMENT there is no counter to reset.
        let plain = meta(t(Some("main"), "p"), vec![cm("a", "TEXT", true)], &[]);
        let p = plan(
            Dialect::Sqlite,
            Some(&plain),
            vec![DdlOp::TruncateTable {
                table: t(None, "p"),
                restart_identity: true,
                cascade: false,
            }],
        );
        assert_eq!(p.statements, [r#"DELETE FROM "main"."p""#]);
    }

    #[test]
    fn sqlite_type_change_rebuilds_and_replays_indexes_and_triggers() {
        let items = lite_items();
        let p = plan(
            Dialect::Sqlite,
            Some(&items),
            vec![DdlOp::AlterColumnType {
                table: t(None, "items"),
                name: "qty".into(),
                new_type: "BIGINT".into(),
                using: None,
            }],
        );
        assert_eq!(
            p.statements,
            [
                "PRAGMA foreign_keys = OFF".to_string(),
                "BEGIN".to_string(),
                format!("CREATE TABLE \"main\".\"items__tabler_new\" (\n{LITE_BODY}\n)"),
                r#"INSERT INTO "main"."items__tabler_new" ("id", "name", "qty", "note", "stamp") SELECT "id", "name", "qty", "note", "stamp" FROM "main"."items""#.to_string(),
                // the AUTOINCREMENT counter moves to the new table instead of being recomputed
                r#"DELETE FROM "main"."sqlite_sequence" WHERE name = 'items__tabler_new'"#.to_string(),
                r#"UPDATE "main"."sqlite_sequence" SET name = 'items__tabler_new' WHERE name = 'items'"#.to_string(),
                "PRAGMA legacy_alter_table = ON".to_string(),
                r#"DROP TABLE "main"."items""#.to_string(),
                r#"ALTER TABLE "main"."items__tabler_new" RENAME TO "items""#.to_string(),
                "PRAGMA legacy_alter_table = OFF".to_string(),
                r#"CREATE INDEX items_name ON "items" (name)"#.to_string(),
                "CREATE TRIGGER items_touch AFTER UPDATE ON items BEGIN UPDATE items SET note = 'x' WHERE id = NEW.id; END".to_string(),
                // the rebuilt table and every table referencing it
                "SELECT k.* FROM \"main\".sqlite_master AS m, pragma_foreign_key_check(m.name, 'main') AS k \
                 WHERE m.type = 'table' AND (m.name = 'items' COLLATE NOCASE OR EXISTS \
                 (SELECT 1 FROM pragma_foreign_key_list(m.name, 'main') AS f WHERE f.\"table\" = 'items' COLLATE NOCASE))".to_string(),
                "COMMIT".to_string(),
                "PRAGMA foreign_keys = ON".to_string(),
            ]
        );
        assert!(is_fk_check(&p.statements[12]));
        assert!(!p.transactional);
        assert!(
            p.warnings
                .iter()
                .any(|w| w.contains("rebuilt from introspected metadata")),
            "{:?}",
            p.warnings
        );
    }

    #[test]
    fn sqlite_rebuild_folds_later_ops_and_tracks_renames() {
        let items = lite_items();
        let tbl = t(None, "items");
        let mut extra = cdef("added", "TEXT", true);
        extra.default_expr = Some("CURRENT_TIMESTAMP".into());
        let p = plan(
            Dialect::Sqlite,
            Some(&items),
            vec![
                DdlOp::AlterColumnNullable {
                    table: tbl.clone(),
                    name: "note".into(),
                    nullable: false,
                },
                DdlOp::RenameColumn {
                    table: tbl.clone(),
                    name: "name".into(),
                    new_name: "title".into(),
                },
                DdlOp::AddColumn {
                    table: tbl.clone(),
                    column: extra,
                    after: None,
                },
                DdlOp::AlterColumnDefault {
                    table: tbl.clone(),
                    name: "qty".into(),
                    default_expr: Some("abs(-1)".into()),
                },
                DdlOp::DropColumn {
                    table: tbl.clone(),
                    name: "stamp".into(),
                },
                DdlOp::AddIndex {
                    table: tbl.clone(),
                    index: IndexDef {
                        name: "items_title_qty".into(),
                        columns: vec!["title".into(), "qty desc".into()],
                        unique: false,
                        method: None,
                        predicate: None,
                        comment: None,
                        include: vec![],
                    },
                },
                // a different table ends the fold
                DdlOp::DropTable {
                    table: t(None, "other"),
                    if_exists: true,
                    cascade: false,
                },
            ],
        );
        let check = sqlite_fk_check_sql(Some("main"), "items");
        assert_eq!(
            p.statements,
            [
                "PRAGMA foreign_keys = OFF",
                "BEGIN",
                // the renamed column keeps its physical name until the rebuild is in place
                "CREATE TABLE \"main\".\"items__tabler_new\" (\n  \"id\" INTEGER PRIMARY KEY AUTOINCREMENT,\n  \"name\" TEXT NOT NULL,\n  \"qty\" INT DEFAULT (abs(-1)),\n  \"note\" TEXT NOT NULL,\n  \"added\" TEXT DEFAULT CURRENT_TIMESTAMP\n)",
                r#"INSERT INTO "main"."items__tabler_new" ("id", "name", "qty", "note") SELECT "id", "name", "qty", "note" FROM "main"."items""#,
                r#"DELETE FROM "main"."sqlite_sequence" WHERE name = 'items__tabler_new'"#,
                r#"UPDATE "main"."sqlite_sequence" SET name = 'items__tabler_new' WHERE name = 'items'"#,
                "PRAGMA legacy_alter_table = ON",
                r#"DROP TABLE "main"."items""#,
                r#"ALTER TABLE "main"."items__tabler_new" RENAME TO "items""#,
                "PRAGMA legacy_alter_table = OFF",
                // stored definitions are replayed as they are …
                r#"CREATE INDEX items_name ON "items" (name)"#,
                "CREATE TRIGGER items_touch AFTER UPDATE ON items BEGIN UPDATE items SET note = 'x' WHERE id = NEW.id; END",
                // "stamp" is gone: SQLite checks every view and trigger against the new table
                r#"CREATE TABLE "main"."items__tabler_check" ("c")"#,
                r#"ALTER TABLE "main"."items__tabler_check" RENAME COLUMN "c" TO "d""#,
                r#"DROP TABLE "main"."items__tabler_check""#,
                // … and SQLite's own rename rewrites them (and views, child foreign keys)
                r#"ALTER TABLE "main"."items" RENAME COLUMN "name" TO "title""#,
                // an index added in the same change set uses the final names
                r#"CREATE INDEX "main"."items_title_qty" ON "items" ("title", "qty" DESC)"#,
                r#"DROP TABLE IF EXISTS "other""#,
                check.as_str(),
                "COMMIT",
                "PRAGMA foreign_keys = ON",
            ]
        );
    }

    #[test]
    fn sqlite_drop_column_rebuilds_when_the_column_is_indexed() {
        let items = lite_items();
        let p = plan(
            Dialect::Sqlite,
            Some(&items),
            vec![DdlOp::DropColumn {
                table: t(None, "items"),
                name: "name".into(),
            }],
        );
        assert_eq!(
            p.statements[2],
            "CREATE TABLE \"main\".\"items__tabler_new\" (\n  \"id\" INTEGER PRIMARY KEY AUTOINCREMENT,\n  \"qty\" INT DEFAULT 0,\n  \"note\" TEXT,\n  \"stamp\" TEXT DEFAULT (datetime('now'))\n)"
        );
        assert_eq!(
            p.statements[3],
            r#"INSERT INTO "main"."items__tabler_new" ("id", "qty", "note", "stamp") SELECT "id", "qty", "note", "stamp" FROM "main"."items""#
        );
        assert!(
            !p.statements.iter().any(|s| s.contains("items_name")),
            "{:#?}",
            p.statements
        );
        assert!(
            p.warnings.iter().any(|w| w.contains("items_name")),
            "{:?}",
            p.warnings
        );
    }

    #[test]
    fn sqlite_foreign_and_primary_keys_rebuild() {
        let mut child = meta(
            t(Some("main"), "child"),
            vec![
                cm("id", "TEXT", false),
                cm("parent_id", "INTEGER", true),
                cm("code", "TEXT", true),
            ],
            &["id"],
        );
        child.columns[0].is_pk = true;
        child.indexes = vec![
            index("sqlite_autoindex_child_1", &["id"], true, true, None),
            index("sqlite_autoindex_child_2", &["code"], true, false, None),
        ];
        child.foreign_keys = vec![ForeignKeyMeta {
            name: "fk_child_0".into(),
            columns: vec!["parent_id".into()],
            ref_table: t(Some("main"), "parent"),
            ref_columns: vec!["id".into()],
            on_update: "NO ACTION".into(),
            on_delete: "CASCADE".into(),
            definition: None,
        }];
        child.ddl = Some("CREATE TABLE child (id TEXT PRIMARY KEY, parent_id INTEGER REFERENCES parent(id) ON DELETE CASCADE, code TEXT UNIQUE) WITHOUT ROWID".into());
        let tbl = t(None, "child");
        let p = plan(
            Dialect::Sqlite,
            Some(&child),
            vec![
                DdlOp::DropForeignKey {
                    table: tbl.clone(),
                    name: "fk_child_0".into(),
                },
                DdlOp::AddForeignKey {
                    table: tbl.clone(),
                    fk: FkDef {
                        name: Some("child_parent".into()),
                        columns: vec!["parent_id".into()],
                        ref_table: t(Some("main"), "parent"),
                        ref_columns: vec![],
                        on_update: None,
                        on_delete: Some("SET NULL".into()),
                    },
                },
            ],
        );
        assert_eq!(
            p.statements[2],
            // the key's implicit NOT NULL is not restated (none was declared)
            "CREATE TABLE \"main\".\"child__tabler_new\" (\n  \"id\" TEXT,\n  \"parent_id\" INTEGER,\n  \"code\" TEXT,\n  PRIMARY KEY (\"id\"),\n  UNIQUE (\"code\"),\n  CONSTRAINT \"child_parent\" FOREIGN KEY (\"parent_id\") REFERENCES \"parent\" ON DELETE SET NULL\n) WITHOUT ROWID"
        );
        assert_eq!(p.statements.len(), 11, "{:#?}", p.statements);

        // Dropping the primary key; dropping the UNIQUE-backing index also needs the rebuild.
        // "id" was NOT NULL only because of the key (none declared), so it becomes nullable.
        let p = plan(
            Dialect::Sqlite,
            Some(&child),
            vec![
                DdlOp::DropPrimaryKey { table: tbl.clone() },
                DdlOp::DropIndex {
                    table: tbl.clone(),
                    name: "sqlite_autoindex_child_2".into(),
                },
            ],
        );
        assert_eq!(
            p.statements[2],
            "CREATE TABLE \"main\".\"child__tabler_new\" (\n  \"id\" TEXT,\n  \"parent_id\" INTEGER,\n  \"code\" TEXT,\n  FOREIGN KEY (\"parent_id\") REFERENCES \"parent\" (\"id\") ON DELETE CASCADE\n) WITHOUT ROWID"
        );
        // Adding a primary key back.
        let p = plan(
            Dialect::Sqlite,
            Some(&child),
            vec![
                DdlOp::DropPrimaryKey { table: tbl.clone() },
                DdlOp::AddPrimaryKey {
                    table: tbl.clone(),
                    columns: vec!["id".into(), "code".into()],
                },
            ],
        );
        assert!(
            p.statements[2]
                .contains("  \"code\" TEXT NOT NULL,\n  PRIMARY KEY (\"id\", \"code\"),"),
            "{}",
            p.statements[2]
        );
        // Without metadata these ops cannot be planned.
        for op in [
            DdlOp::DropPrimaryKey { table: tbl.clone() },
            DdlOp::AlterColumnType {
                table: tbl.clone(),
                name: "code".into(),
                new_type: "INT".into(),
                using: None,
            },
            DdlOp::DropForeignKey {
                table: tbl.clone(),
                name: "x".into(),
            },
        ] {
            assert!(invalid_input(Dialect::Sqlite, None, vec![op]).contains("metadata"));
        }
        // Unknown names are rejected while folding.
        let msg = invalid_input(
            Dialect::Sqlite,
            Some(&child),
            vec![DdlOp::DropForeignKey {
                table: tbl.clone(),
                name: "nope".into(),
            }],
        );
        assert!(msg.contains("\"nope\""), "{msg}");
    }

    #[test]
    fn sqlite_rebuild_keeps_generated_columns_verbatim() {
        let mut gen_col = cm("total", "INT", true);
        gen_col.is_generated = true;
        gen_col.extra = Some("VIRTUAL".into());
        let mut m = meta(
            t(Some("main"), "g"),
            vec![cm("a", "INT", true), gen_col],
            &[],
        );
        m.ddl = Some("CREATE TABLE \"g\" (a INT, \"total\" INT GENERATED ALWAYS AS (a * 2) VIRTUAL, CHECK (a > 0))".into());
        let p = plan(
            Dialect::Sqlite,
            Some(&m),
            vec![DdlOp::AlterColumnNullable {
                table: t(None, "g"),
                name: "a".into(),
                nullable: false,
            }],
        );
        assert_eq!(
            p.statements[2],
            "CREATE TABLE \"main\".\"g__tabler_new\" (\n  \"a\" INT NOT NULL,\n  \"total\" INT GENERATED ALWAYS AS (a * 2) VIRTUAL,\n  CHECK (a > 0)\n)"
        );
        assert_eq!(
            p.statements[3],
            r#"INSERT INTO "main"."g__tabler_new" ("a") SELECT "a" FROM "main"."g""#
        );
        let err = plan_ddl(
            Dialect::Sqlite,
            Some(&m),
            &[DdlOp::AlterColumnType {
                table: t(None, "g"),
                name: "total".into(),
                new_type: "TEXT".into(),
                using: None,
            }],
        )
        .unwrap_err();
        assert!(matches!(err, AppError::Unsupported(_)), "{err:?}");
    }

    #[test]
    fn sqlite_rebuild_recovers_collations_from_the_original_ddl() {
        let mut m = meta(
            t(Some("main"), "c"),
            vec![
                cm("a", "TEXT", true),
                cm("b", "TEXT", true),
                cm("c", "TEXT", true),
            ],
            &[],
        );
        m.ddl = Some(
            "CREATE TABLE c (a TEXT COLLATE NOCASE, b TEXT CHECK (b COLLATE BINARY <> 'x'), c TEXT DEFAULT 'COLLATE RTRIM' COLLATE \"RTRIM\")"
                .into(),
        );
        assert_eq!(
            column_collation("a TEXT COLLATE NOCASE"),
            Some("NOCASE".into())
        );
        assert_eq!(
            column_collation("b TEXT CHECK (b COLLATE BINARY <> 'x')"),
            None
        );
        let p = plan(
            Dialect::Sqlite,
            Some(&m),
            vec![DdlOp::AlterColumnNullable {
                table: t(None, "c"),
                name: "b".into(),
                nullable: false,
            }],
        );
        assert_eq!(
            p.statements[2],
            "CREATE TABLE \"main\".\"c__tabler_new\" (\n  \"a\" TEXT COLLATE \"NOCASE\",\n  \"b\" TEXT NOT NULL,\n  \"c\" TEXT COLLATE \"RTRIM\",\n  CHECK (b COLLATE BINARY <> 'x')\n)"
        );
    }

    #[test]
    fn sqlite_raw_pragmas_run_outside_a_transaction() {
        let p = plan(
            Dialect::Sqlite,
            None,
            vec![DdlOp::RawSql {
                sql: "PRAGMA journal_mode = WAL;".into(),
            }],
        );
        assert_eq!(p.statements, ["PRAGMA journal_mode = WAL"]);
        assert!(!p.transactional);
    }

    #[test]
    fn sqlite_add_column_with_expression_default_rebuilds() {
        let items = lite_items();
        let mut col = cdef("created", "TEXT", true);
        col.default_expr = Some("(datetime('now'))".into());
        let p = plan(
            Dialect::Sqlite,
            Some(&items),
            vec![DdlOp::AddColumn {
                table: t(None, "items"),
                column: col.clone(),
                after: None,
            }],
        );
        assert!(!p.transactional);
        assert!(
            p.statements[2].ends_with(",\n  \"created\" TEXT DEFAULT (datetime('now'))\n)"),
            "{}",
            p.statements[2]
        );
        let msg = invalid_input(
            Dialect::Sqlite,
            None,
            vec![DdlOp::AddColumn {
                table: t(None, "items"),
                column: col,
                after: None,
            }],
        );
        assert!(msg.contains("metadata"));
        let mut auto = cdef("n", "INTEGER", false);
        auto.auto_increment = true;
        assert!(
            invalid_input(
                Dialect::Sqlite,
                None,
                vec![DdlOp::AddColumn {
                    table: t(None, "items"),
                    column: auto,
                    after: None
                }]
            )
            .contains("AUTOINCREMENT")
        );
    }

    /// `items(id INTEGER PRIMARY KEY, qty INT)` with a trigger writing to another table whose
    /// column has the same name as one of `items`.
    fn lite_audited() -> TableMeta {
        let mut id = cm("id", "INTEGER", false);
        id.is_pk = true;
        let mut m = meta(
            t(Some("main"), "items"),
            vec![id, cm("qty", "INT", true)],
            &["id"],
        );
        m.triggers = vec![TriggerMeta {
            name: "trg".into(),
            timing: Some("AFTER".into()),
            events: vec!["UPDATE".into()],
            enabled: true,
            definition: "CREATE TRIGGER trg AFTER UPDATE ON items BEGIN INSERT INTO audit(id, note) VALUES (NEW.id, 'u'); END".into(),
        }];
        m.ddl = Some("CREATE TABLE items(id INTEGER PRIMARY KEY, qty INT)".into());
        m
    }

    #[test]
    fn sqlite_rename_next_to_a_rebuild_runs_after_it() {
        let m = lite_audited();
        let tbl = t(None, "items");
        let rename = DdlOp::RenameColumn {
            table: tbl.clone(),
            name: "id".into(),
            new_name: "item_id".into(),
        };
        let not_null = DdlOp::AlterColumnNullable {
            table: tbl.clone(),
            name: "qty".into(),
            nullable: false,
        };
        // Either order: the trigger is replayed exactly as stored (it names `audit.id`, which
        // a text rename would corrupt) and SQLite renames the column afterwards.
        for ops in [
            vec![rename.clone(), not_null.clone()],
            vec![not_null.clone(), rename.clone()],
        ] {
            let p = plan(Dialect::Sqlite, Some(&m), ops);
            let body = &p.statements[2..p.statements.len() - 3];
            assert_eq!(
                body,
                [
                    "CREATE TABLE \"main\".\"items__tabler_new\" (\n  \"id\" INTEGER,\n  \"qty\" INT NOT NULL,\n  PRIMARY KEY (\"id\")\n)",
                    r#"INSERT INTO "main"."items__tabler_new" ("id", "qty") SELECT "id", "qty" FROM "main"."items""#,
                    "PRAGMA legacy_alter_table = ON",
                    r#"DROP TABLE "main"."items""#,
                    r#"ALTER TABLE "main"."items__tabler_new" RENAME TO "items""#,
                    "PRAGMA legacy_alter_table = OFF",
                    "CREATE TRIGGER trg AFTER UPDATE ON items BEGIN INSERT INTO audit(id, note) VALUES (NEW.id, 'u'); END",
                    r#"ALTER TABLE "main"."items" RENAME COLUMN "id" TO "item_id""#,
                ]
            );
        }
        // A rename that already ran before an unrelated statement leaves the stored trigger
        // text unknown: a later rebuild refuses instead of replaying it wrongly.
        let err = plan_ddl(
            Dialect::Sqlite,
            Some(&m),
            &[
                rename.clone(),
                DdlOp::TruncateTable {
                    table: tbl.clone(),
                    restart_identity: false,
                    cascade: false,
                },
                not_null.clone(),
            ],
        )
        .unwrap_err();
        assert!(
            matches!(&err, AppError::Unsupported(msg) if msg.contains("\"trg\"") && msg.contains("\"id\"")),
            "{err:?}"
        );
        // … but only when the trigger mentions the renamed column.
        let p = plan(
            Dialect::Sqlite,
            Some(&m),
            vec![
                DdlOp::RenameColumn {
                    table: tbl.clone(),
                    name: "qty".into(),
                    new_name: "amount".into(),
                },
                DdlOp::TruncateTable {
                    table: tbl.clone(),
                    restart_identity: false,
                    cascade: false,
                },
                DdlOp::AlterColumnNullable {
                    table: tbl.clone(),
                    name: "amount".into(),
                    nullable: false,
                },
            ],
        );
        assert_eq!(
            p.statements[2],
            r#"ALTER TABLE "main"."items" RENAME COLUMN "qty" TO "amount""#
        );
        assert!(
            p.statements[4].contains("\"amount\" INT NOT NULL"),
            "{:#?}",
            p.statements
        );
    }

    #[test]
    fn sqlite_rebuild_sorts_out_swapped_and_reused_names() {
        let m = meta(
            t(Some("main"), "s"),
            vec![
                cm("a", "TEXT", true),
                cm("b", "TEXT", true),
                cm("c", "TEXT", true),
            ],
            &[],
        );
        let tbl = t(None, "s");
        let rename = |from: &str, to: &str| DdlOp::RenameColumn {
            table: tbl.clone(),
            name: from.into(),
            new_name: to.into(),
        };
        let p = plan(
            Dialect::Sqlite,
            Some(&m),
            vec![
                DdlOp::AlterColumnNullable {
                    table: tbl.clone(),
                    name: "c".into(),
                    nullable: false,
                },
                // swap a and b
                rename("a", "x"),
                rename("b", "a"),
                rename("x", "b"),
                // c becomes d, and a new column takes the name c
                rename("c", "d"),
                DdlOp::AddColumn {
                    table: tbl.clone(),
                    column: {
                        let mut c = cdef("c", "INT", true);
                        c.default_expr = Some("(1 + 1)".into());
                        c
                    },
                    after: None,
                },
                DdlOp::AddIndex {
                    table: tbl.clone(),
                    index: IndexDef {
                        name: "s_c".into(),
                        columns: vec!["c".into(), "d".into()],
                        unique: false,
                        method: None,
                        predicate: Some("a IS NOT NULL".into()),
                        comment: None,
                        include: vec![],
                    },
                },
            ],
        );
        let body = &p.statements[2..p.statements.len() - 3];
        assert_eq!(
            body,
            [
                "CREATE TABLE \"main\".\"s__tabler_new\" (\n  \"a\" TEXT,\n  \"b\" TEXT,\n  \"c\" TEXT NOT NULL,\n  \"c__tabler_tmp\" INT DEFAULT (1 + 1)\n)",
                r#"INSERT INTO "main"."s__tabler_new" ("a", "b", "c") SELECT "a", "b", "c" FROM "main"."s""#,
                "PRAGMA legacy_alter_table = ON",
                r#"DROP TABLE "main"."s""#,
                r#"ALTER TABLE "main"."s__tabler_new" RENAME TO "s""#,
                "PRAGMA legacy_alter_table = OFF",
                r#"ALTER TABLE "main"."s" RENAME COLUMN "c" TO "d""#,
                r#"ALTER TABLE "main"."s" RENAME COLUMN "c__tabler_tmp" TO "c""#,
                r#"ALTER TABLE "main"."s" RENAME COLUMN "a" TO "a__tabler_tmp""#,
                r#"ALTER TABLE "main"."s" RENAME COLUMN "b" TO "a""#,
                r#"ALTER TABLE "main"."s" RENAME COLUMN "a__tabler_tmp" TO "b""#,
                // written against the final names, so created after the renames
                r#"CREATE INDEX "main"."s_c" ON "s" ("c", "d") WHERE a IS NOT NULL"#,
            ]
        );
        assert_eq!(
            ordered_renames(&["a".into(), "b".into()], &["b".into(), "a".into()]).unwrap(),
            [
                ("a".to_string(), "a__tabler_tmp".to_string()),
                ("b".to_string(), "a".to_string()),
                ("a__tabler_tmp".to_string(), "b".to_string()),
            ]
        );
        // A column may take another spelling of its own name.
        assert_eq!(
            ordered_renames(&["a".into(), "b".into()], &["A".into(), "b".into()]).unwrap(),
            [("a".to_string(), "A".to_string())]
        );
        // Targets equal ignoring case can never all be reached: an error, not a loop.
        for (current, finals) in [
            (["id", "a", "b"], ["A", "a", "b"]),
            (["id", "a", "A__tabler_tmp"], ["id", "a", "A"]),
        ] {
            let current = current.map(String::from);
            let finals = finals.map(String::from);
            assert!(matches!(
                ordered_renames(&current, &finals),
                Err(AppError::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn sqlite_drop_primary_key_keeps_only_declared_not_null() {
        let mut id = cm("id", "INTEGER", false);
        id.is_pk = true;
        let mut code = cm("code", "TEXT", false);
        code.is_pk = true;
        let mut m = meta(
            t(Some("main"), "p"),
            vec![id, code, cm("name", "TEXT", true)],
            &["id", "code"],
        );
        // introspection reports both key columns NOT NULL; only `code` declares it
        m.ddl = Some(
            "CREATE TABLE p(id INTEGER, code TEXT NOT NULL CHECK (code IS NOT NULL), name TEXT, PRIMARY KEY (id, code))"
                .into(),
        );
        let p = plan(
            Dialect::Sqlite,
            Some(&m),
            vec![DdlOp::DropPrimaryKey {
                table: t(None, "p"),
            }],
        );
        assert_eq!(
            p.statements[2],
            "CREATE TABLE \"main\".\"p__tabler_new\" (\n  \"id\" INTEGER,\n  \"code\" TEXT NOT NULL,\n  \"name\" TEXT,\n  CHECK (code IS NOT NULL)\n)"
        );
        // A NOT NULL requested in the same change set stays.
        let p = plan(
            Dialect::Sqlite,
            Some(&m),
            vec![
                DdlOp::AlterColumnNullable {
                    table: t(None, "p"),
                    name: "id".into(),
                    nullable: false,
                },
                DdlOp::DropPrimaryKey {
                    table: t(None, "p"),
                },
            ],
        );
        assert!(
            p.statements[2].contains("\"id\" INTEGER NOT NULL,"),
            "{}",
            p.statements[2]
        );
    }

    #[test]
    fn sqlite_rejects_vacuum_or_pragma_next_to_a_rebuild() {
        let msg = invalid_input(
            Dialect::Sqlite,
            Some(&lite_items()),
            vec![
                DdlOp::AlterColumnType {
                    table: t(None, "items"),
                    name: "qty".into(),
                    new_type: "TEXT".into(),
                    using: None,
                },
                DdlOp::RawSql {
                    sql: "VACUUM".into(),
                },
            ],
        );
        assert!(msg.contains("separately"), "{msg}");
    }

    #[test]
    fn sqlite_fk_check_covers_referencing_tables() {
        let sql = sqlite_fk_check_sql(Some("aux"), "it's");
        assert_eq!(
            sql,
            "SELECT k.* FROM \"aux\".sqlite_master AS m, pragma_foreign_key_check(m.name, 'aux') AS k \
             WHERE m.type = 'table' AND (m.name = 'it''s' COLLATE NOCASE OR EXISTS \
             (SELECT 1 FROM pragma_foreign_key_list(m.name, 'aux') AS f WHERE f.\"table\" = 'it''s' COLLATE NOCASE))"
        );
        assert!(is_fk_check(&sql));
        assert!(!is_fk_check("SELECT 1"));
        assert_eq!(
            session_toggle("PRAGMA legacy_alter_table = OFF").as_deref(),
            Some("legacy_alter_table")
        );
        assert_eq!(
            session_toggle("pragma main.FOREIGN_KEYS=on").as_deref(),
            Some("foreign_keys")
        );
        assert_eq!(session_toggle("PRAGMA foreign_key_check"), None);
    }

    #[test]
    fn sqlite_create_table() {
        let mut id = cdef("id", "INTEGER", false);
        id.auto_increment = true;
        let mut name = cdef("name", "TEXT", false);
        name.default_expr = Some("'x'".into());
        name.collation = Some("NOCASE".into());
        let p = plan(
            Dialect::Sqlite,
            None,
            vec![
                DdlOp::CreateTable {
                    table: t(None, "a"),
                    columns: vec![id, name],
                    primary_key: vec!["id".into()],
                    if_not_exists: false,
                },
                DdlOp::CreateTable {
                    table: t(None, "b"),
                    columns: vec![cdef("x", "INT", false), cdef("y", "", true)],
                    primary_key: vec!["x".into(), "y".into()],
                    if_not_exists: true,
                },
                DdlOp::DropTable {
                    table: t(None, "c"),
                    if_exists: false,
                    cascade: true,
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                "CREATE TABLE \"a\" (\n  \"id\" INTEGER PRIMARY KEY AUTOINCREMENT,\n  \"name\" TEXT NOT NULL DEFAULT 'x' COLLATE \"NOCASE\"\n)",
                "CREATE TABLE IF NOT EXISTS \"b\" (\n  \"x\" INT NOT NULL,\n  \"y\",\n  PRIMARY KEY (\"x\", \"y\")\n)",
                "DROP TABLE \"c\"",
            ]
        );
        assert_eq!(p.warnings.len(), 1);
    }

    // --- create_table_sql / drop_statement_for ----------------------------------------------

    #[test]
    fn reconstructs_a_postgres_table() {
        let mut m = pg_users();
        m.columns.push({
            let mut c = cm("search", "tsvector", true);
            c.is_generated = true;
            c.default_expr = Some("to_tsvector('simple'::regconfig, name)".into());
            c
        });
        m.foreign_keys = vec![ForeignKeyMeta {
            name: "users_org_fk".into(),
            columns: vec!["org_id".into()],
            ref_table: t(Some("public"), "orgs"),
            ref_columns: vec!["id".into()],
            on_update: "NO ACTION".into(),
            on_delete: "CASCADE".into(),
            definition: Some("FOREIGN KEY (org_id) REFERENCES orgs(id) ON DELETE CASCADE".into()),
        }];
        m.triggers = vec![TriggerMeta {
            name: "users_touch".into(),
            timing: Some("BEFORE".into()),
            events: vec!["UPDATE".into()],
            enabled: true,
            definition: "CREATE TRIGGER users_touch BEFORE UPDATE ON public.users FOR EACH ROW EXECUTE FUNCTION touch()".into(),
        }];
        let sql = create_table_sql(Dialect::Postgres, &m);
        assert_eq!(
            sql,
            "CREATE TABLE \"public\".\"users\" (\n  \"id\" bigint GENERATED BY DEFAULT AS IDENTITY NOT NULL,\n  \"email\" character varying(255) NOT NULL,\n  \"name\" text,\n  \"created_at\" timestamp with time zone NOT NULL DEFAULT now(),\n  \"search\" tsvector GENERATED ALWAYS AS (to_tsvector('simple'::regconfig, name)) STORED,\n  CONSTRAINT \"users_pkey\" PRIMARY KEY (\"id\")\n);\n\n\
             CREATE UNIQUE INDEX users_email_key ON public.users USING btree (email);\n\n\
             ALTER TABLE \"public\".\"users\" ADD CONSTRAINT \"users_org_fk\" FOREIGN KEY (org_id) REFERENCES orgs(id) ON DELETE CASCADE;\n\n\
             CREATE TRIGGER users_touch BEFORE UPDATE ON public.users FOR EACH ROW EXECUTE FUNCTION touch();\n\n\
             COMMENT ON TABLE \"public\".\"users\" IS 'app users';\n\
             COMMENT ON COLUMN \"public\".\"users\".\"email\" IS 'login';"
        );
    }

    #[test]
    fn reconstructs_mysql_and_sqlite_tables() {
        let mut m = my_items();
        m.indexes.push(index(
            "items_name",
            &["name(10)", "qty DESC"],
            false,
            false,
            None,
        ));
        m.comment = Some("stock".into());
        assert_eq!(
            create_table_sql(Dialect::Mysql, &m),
            "CREATE TABLE `shop`.`items` (\n  `id` int unsigned NOT NULL AUTO_INCREMENT,\n  `qty` int unsigned NOT NULL DEFAULT 0 COMMENT 'how many',\n  `name` varchar(50) COLLATE utf8mb4_bin NULL DEFAULT 'it''s',\n  `touched_at` timestamp(3) NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),\n  PRIMARY KEY (`id`)\n) COMMENT='stock';\n\n\
             CREATE INDEX `items_name` ON `shop`.`items` (`name`(10), `qty` DESC);"
        );
        let lite = lite_items();
        let sql = create_table_sql(Dialect::Sqlite, &lite);
        assert!(
            sql.starts_with(
                "CREATE TABLE \"main\".\"items\" (\n  \"id\" INTEGER PRIMARY KEY AUTOINCREMENT,"
            ),
            "{sql}"
        );
        assert!(
            sql.contains(
                "\n\nCREATE INDEX items_name ON items(name);\n\nCREATE TRIGGER items_touch"
            ),
            "{sql}"
        );
    }

    #[test]
    fn drop_statements_by_kind() {
        let v = t(Some("s"), "v");
        assert_eq!(
            drop_statement_for(Dialect::Postgres, &v, TableKind::MaterializedView),
            r#"DROP MATERIALIZED VIEW "s"."v""#
        );
        assert_eq!(
            drop_statement_for(Dialect::Postgres, &v, TableKind::View),
            r#"DROP VIEW "s"."v""#
        );
        assert_eq!(
            drop_statement_for(Dialect::Postgres, &v, TableKind::ForeignTable),
            r#"DROP FOREIGN TABLE "s"."v""#
        );
        assert_eq!(
            drop_statement_for(Dialect::Postgres, &v, TableKind::Partitioned),
            r#"DROP TABLE "s"."v""#
        );
        assert_eq!(
            drop_statement_for(Dialect::Mysql, &v, TableKind::Table),
            "DROP TABLE `s`.`v`"
        );
        assert_eq!(
            drop_statement_for(Dialect::Sqlite, &t(None, "v"), TableKind::View),
            r#"DROP VIEW "v""#
        );
    }

    // --- text helpers -----------------------------------------------------------------------

    #[test]
    fn renames_identifiers_outside_literals() {
        let q = "\"new\"";
        assert_eq!(
            rename_ident("a + \"a\" + `A` + [a] + 'a' + aa + x'0a'", "a", q),
            "\"new\" + \"new\" + \"new\" + \"new\" + 'a' + aa + x'0a'"
        );
        assert_eq!(
            rename_ident("NEW.a -- a\n/* a */ 1e5a", "a", q),
            "NEW.\"new\" -- a\n/* a */ 1e5a"
        );
        assert_eq!(rename_ident("x = X'00'", "x", q), "\"new\" = X'00'");
        assert!(references_ident(
            "CREATE INDEX i ON t (lower(email))",
            "email"
        ));
        assert!(!references_ident(
            "CREATE INDEX i ON t (lower(email))",
            "mail"
        ));
        assert_eq!(
            rename_in_index_def("CREATE INDEX a ON a (a, b) WHERE a > 0", "a", q),
            "CREATE INDEX a ON a (\"new\", b) WHERE \"new\" > 0"
        );
        assert_eq!(
            rename_in_trigger_def(
                "CREATE TRIGGER a AFTER UPDATE OF a ON t BEGIN SELECT NEW.a; END",
                "a",
                q
            ),
            "CREATE TRIGGER a AFTER UPDATE OF \"new\" ON t BEGIN SELECT NEW.\"new\"; END"
        );
    }

    #[test]
    fn replays_definitions_for_attached_schemas() {
        assert_eq!(
            replay_index(
                "CREATE UNIQUE INDEX IF NOT EXISTS \"i x\" ON old(a) WHERE a IS NOT NULL",
                Some("aux"),
                "t"
            ),
            "CREATE UNIQUE INDEX IF NOT EXISTS \"aux\".\"i x\" ON \"t\" (a) WHERE a IS NOT NULL"
        );
        assert_eq!(
            replay_index("CREATE INDEX i ON t(a)", Some("main"), "t"),
            "CREATE INDEX i ON \"t\" (a)"
        );
        assert_eq!(
            replay_trigger(
                "CREATE TRIGGER trg AFTER INSERT ON t BEGIN SELECT 1; END",
                Some("aux")
            ),
            "CREATE TRIGGER \"aux\".trg AFTER INSERT ON t BEGIN SELECT 1; END"
        );
        assert_eq!(
            replay_trigger(
                "CREATE TRIGGER trg AFTER INSERT ON t BEGIN SELECT 1; END",
                None
            ),
            "CREATE TRIGGER trg AFTER INSERT ON t BEGIN SELECT 1; END"
        );
    }

    #[test]
    fn splits_sqlite_create_table() {
        let (defs, tail) = split_create_table(
            "CREATE TABLE \"a(b\" (\n  id INTEGER PRIMARY KEY, -- the key, really\n  \"x,y\" TEXT DEFAULT 'a,b)' CHECK (length(\"x,y\") > 0),\n  [z] REAL,\n  CONSTRAINT u UNIQUE (id, z),\n  FOREIGN KEY (z) REFERENCES p(id)\n) STRICT, WITHOUT ROWID;",
        );
        let names: Vec<&str> = defs.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["id", "x,y", "z"]);
        assert_eq!(
            defs[1].1,
            "\"x,y\" TEXT DEFAULT 'a,b)' CHECK (length(\"x,y\") > 0)"
        );
        assert_eq!(tail.as_deref(), Some("STRICT, WITHOUT ROWID"));
        assert_eq!(split_create_table("CREATE TABLE t (a)").1, None);
    }

    #[test]
    fn sqlite_default_and_constant_rules() {
        assert!(sqlite_constant("-1.5e3"));
        assert!(sqlite_constant("'it''s'"));
        assert!(sqlite_constant("X'00ff'"));
        assert!(sqlite_constant("NULL"));
        assert!(sqlite_constant("(42)"));
        assert!(!sqlite_constant("CURRENT_TIMESTAMP"));
        assert!(!sqlite_constant("(datetime('now'))"));
        assert_eq!(sqlite_default_sql("datetime('now')"), "(datetime('now'))");
        assert_eq!(sqlite_default_sql("(1) + (2)"), "((1) + (2))");
        assert_eq!(sqlite_default_sql("(1 + 2)"), "(1 + 2)");
        assert_eq!(sqlite_default_sql("CURRENT_DATE"), "CURRENT_DATE");
        assert_eq!(sqlite_default_sql("'a'"), "'a'");
    }

    #[test]
    fn recognises_fk_pragmas() {
        assert!(is_fk_toggle("PRAGMA foreign_keys = OFF"));
        assert!(is_fk_toggle("pragma main.foreign_keys=on"));
        assert!(!is_fk_toggle("PRAGMA foreign_key_check(\"t\")"));
        assert!(is_fk_check("PRAGMA foreign_key_check(\"t\")"));
        assert!(is_fk_check("PRAGMA \"aux\".foreign_key_check"));
        assert!(!is_fk_check("PRAGMA foreign_keys = ON"));
    }

    // --- regressions ----------------------------------------------------------------------

    /// `plan_ddl` on another thread, failing the test instead of hanging it.
    fn plan_within_a_second(
        d: Dialect,
        meta: &TableMeta,
        ops: Vec<DdlOp>,
    ) -> Result<DdlPlan, AppError> {
        let (tx, rx) = std::sync::mpsc::channel();
        let meta = meta.clone();
        std::thread::spawn(move || {
            let _ = tx.send(plan_ddl(d, Some(&meta), &ops));
        });
        rx.recv_timeout(std::time::Duration::from_secs(1))
            .expect("plan_ddl did not return")
    }

    /// A rebuild producing two column names equal ignoring case used to loop forever (and keep
    /// allocating) while ordering the renames.
    #[test]
    fn sqlite_rejects_column_names_differing_only_in_case() {
        let m = meta(
            t(Some("main"), "t"),
            vec![
                cm("id", "INTEGER", false),
                cm("a", "TEXT", true),
                cm("b", "TEXT", true),
            ],
            &["id"],
        );
        let tbl = t(None, "t");
        let retype = DdlOp::AlterColumnType {
            table: tbl.clone(),
            name: "b".into(),
            new_type: "VARCHAR(5)".into(),
            using: None,
        };
        let rename = |to: &str| DdlOp::RenameColumn {
            table: tbl.clone(),
            name: "id".into(),
            new_name: to.into(),
        };
        let add = |name: &str| DdlOp::AddColumn {
            table: tbl.clone(),
            column: {
                let mut c = cdef(name, "TEXT", true);
                c.default_expr = Some("CURRENT_TIMESTAMP".into());
                c
            },
            after: None,
        };
        for ops in [
            vec![retype.clone(), rename("A")],
            vec![rename("A"), retype.clone()],
            vec![add("A")],
            vec![retype.clone(), add("A")],
            vec![retype.clone(), add("ID")],
        ] {
            match plan_within_a_second(Dialect::Sqlite, &m, ops.clone()) {
                Err(AppError::InvalidInput(msg)) => {
                    assert!(msg.contains("case-insensitive"), "{msg}")
                }
                other => panic!("{ops:?}: expected InvalidInput, got {other:?}"),
            }
        }
        // Another spelling of a column's own name is fine, inside a rebuild too.
        let p = plan_within_a_second(
            Dialect::Sqlite,
            &m,
            vec![
                retype,
                DdlOp::RenameColumn {
                    table: tbl.clone(),
                    name: "a".into(),
                    new_name: "A".into(),
                },
            ],
        )
        .expect("plan");
        assert!(
            p.statements
                .contains(&r#"ALTER TABLE "main"."t" RENAME COLUMN "a" TO "A""#.to_string()),
            "{:#?}",
            p.statements
        );
        // MySQL names are case-insensitive too; PG ones are not.
        assert!(
            invalid_input(
                Dialect::Mysql,
                None,
                vec![DdlOp::CreateTable {
                    table: tbl.clone(),
                    columns: vec![cdef("a", "int", true), cdef("A", "int", true)],
                    primary_key: vec![],
                    if_not_exists: false,
                }],
            )
            .contains("duplicate")
        );
        plan(
            Dialect::Postgres,
            None,
            vec![DdlOp::CreateTable {
                table: tbl,
                columns: vec![cdef("a", "int", true), cdef("A", "int", true)],
                primary_key: vec![],
                if_not_exists: false,
            }],
        );
    }

    /// Dropping a column through a rebuild used to commit even when a view or trigger still
    /// used it (SQLite's own DROP COLUMN refuses that).
    #[test]
    fn sqlite_rebuild_dropping_a_column_has_sqlite_check_views_and_triggers() {
        let items = lite_items();
        let check = [
            r#"CREATE TABLE "main"."items__tabler_check" ("c")"#,
            r#"ALTER TABLE "main"."items__tabler_check" RENAME COLUMN "c" TO "d""#,
            r#"DROP TABLE "main"."items__tabler_check""#,
        ];
        let p = plan(
            Dialect::Sqlite,
            Some(&items),
            vec![DdlOp::DropColumn {
                table: t(None, "items"),
                name: "name".into(),
            }],
        );
        let trigger = p
            .statements
            .iter()
            .position(|s| s.starts_with("CREATE TRIGGER items_touch"))
            .expect("trigger replayed");
        // after the replayed trigger, before the foreign-key check
        assert_eq!(p.statements[trigger + 1..trigger + 4], check);
        assert!(is_fk_check(&p.statements[trigger + 4]));
        assert!(
            p.warnings
                .iter()
                .any(|w| w.contains("\"name\"") && w.contains("view or trigger")),
            "{:?}",
            p.warnings
        );
        // A rebuild that keeps every column needs no such check.
        let p = plan(
            Dialect::Sqlite,
            Some(&items),
            vec![DdlOp::AlterColumnType {
                table: t(None, "items"),
                name: "qty".into(),
                new_type: "BIGINT".into(),
                using: None,
            }],
        );
        assert!(!p.statements.iter().any(|s| s.contains(CHECK_SUFFIX)));
    }

    /// The primary-key index of a SQLite table cannot be dropped with DROP INDEX; dropping it
    /// drops the key, like `DropPrimaryKey`.
    #[test]
    fn sqlite_drop_index_of_the_primary_key_rebuilds() {
        let mut k = meta(
            t(Some("main"), "k"),
            vec![cm("code", "TEXT", false), cm("v", "TEXT", true)],
            &["code"],
        );
        k.columns[0].is_pk = true;
        k.indexes = vec![index("sqlite_autoindex_k_1", &["code"], true, true, None)];
        k.ddl = Some("CREATE TABLE k (code TEXT PRIMARY KEY, v TEXT)".into());
        let by_index = plan(
            Dialect::Sqlite,
            Some(&k),
            vec![DdlOp::DropIndex {
                table: t(None, "k"),
                name: "sqlite_autoindex_k_1".into(),
            }],
        );
        assert!(
            !by_index
                .statements
                .iter()
                .any(|s| s.starts_with("DROP INDEX")),
            "{:#?}",
            by_index.statements
        );
        assert_eq!(
            by_index.statements[2],
            "CREATE TABLE \"main\".\"k__tabler_new\" (\n  \"code\" TEXT,\n  \"v\" TEXT\n)"
        );
        let by_key = plan(
            Dialect::Sqlite,
            Some(&k),
            vec![DdlOp::DropPrimaryKey {
                table: t(None, "k"),
            }],
        );
        assert_eq!(by_index.statements, by_key.statements);
    }

    /// A type naming its own character set or collation used to get the old COLLATE appended
    /// (MySQL error 1253, or 1064 for two COLLATE clauses).
    #[test]
    fn mysql_type_change_drops_a_collation_the_new_type_overrides() {
        let items = my_items();
        let retype = |ty: &str| {
            plan(
                Dialect::Mysql,
                Some(&items),
                vec![DdlOp::AlterColumnType {
                    table: t(None, "items"),
                    name: "name".into(),
                    new_type: ty.into(),
                    using: None,
                }],
            )
            .statements
            .remove(0)
        };
        let tail = "NULL DEFAULT 'it''s'";
        for ty in [
            "varchar(20) CHARACTER SET latin1",
            "varchar(20) charset latin1",
            "varchar(20) COLLATE latin1_bin",
            "varchar(20) character set utf8mb4 collate utf8mb4_0900_ai_ci",
            "nvarchar(20)",
            "national varchar(20)",
        ] {
            assert_eq!(
                retype(ty),
                format!("ALTER TABLE `shop`.`items` MODIFY COLUMN `name` {ty} {tail}")
            );
        }
        // a plain text type keeps the column's collation (and so its character set)
        for ty in ["varchar(20)", "text", "enum('ascii','unicode')"] {
            assert_eq!(
                retype(ty),
                format!(
                    "ALTER TABLE `shop`.`items` MODIFY COLUMN `name` {ty} COLLATE utf8mb4_bin {tail}"
                )
            );
        }
    }

    /// Dropping the key on its own fails on a table with an AUTO_INCREMENT column (error 1075):
    /// a drop directly followed by an add runs as one statement.
    #[test]
    fn mysql_primary_key_swap_is_one_statement() {
        let items = my_items();
        let tbl = t(None, "items");
        let add = DdlOp::AddPrimaryKey {
            table: tbl.clone(),
            columns: vec!["id".into(), "qty".into()],
        };
        let swap = "ALTER TABLE `shop`.`items` DROP PRIMARY KEY, ADD PRIMARY KEY (`id`, `qty`)";
        for drop in [
            DdlOp::DropPrimaryKey { table: tbl.clone() },
            DdlOp::DropIndex {
                table: tbl.clone(),
                name: "PRIMARY".into(),
            },
        ] {
            let p = plan(
                Dialect::Mysql,
                Some(&items),
                vec![
                    drop,
                    add.clone(),
                    // later ops see the new key
                    DdlOp::DropPrimaryKey { table: tbl.clone() },
                ],
            );
            assert_eq!(
                p.statements,
                [swap, "ALTER TABLE `shop`.`items` DROP PRIMARY KEY"]
            );
        }
        // without metadata too
        let p = plan(
            Dialect::Mysql,
            None,
            vec![
                DdlOp::DropPrimaryKey {
                    table: t(Some("shop"), "items"),
                },
                DdlOp::AddPrimaryKey {
                    table: t(Some("shop"), "items"),
                    columns: vec!["id".into(), "qty".into()],
                },
            ],
        );
        assert_eq!(p.statements, [swap]);
        // not adjacent, or another table: separate statements
        let p = plan(
            Dialect::Mysql,
            Some(&items),
            vec![
                DdlOp::DropPrimaryKey { table: tbl.clone() },
                DdlOp::SetTableComment {
                    table: tbl.clone(),
                    comment: None,
                },
                add.clone(),
            ],
        );
        assert_eq!(p.statements.len(), 3, "{:#?}", p.statements);
        let p = plan(
            Dialect::Mysql,
            None,
            vec![
                DdlOp::DropPrimaryKey {
                    table: t(Some("shop"), "a"),
                },
                DdlOp::AddPrimaryKey {
                    table: t(Some("shop"), "b"),
                    columns: vec!["id".into()],
                },
            ],
        );
        assert_eq!(p.statements.len(), 2, "{:#?}", p.statements);
    }

    /// PostgreSQL refuses DROP INDEX on the index of a UNIQUE constraint; with the constraint
    /// indexes known, the constraint is dropped instead.
    #[test]
    fn pg_drop_index_of_a_constraint_drops_the_constraint() {
        let users = pg_users();
        let drop = |name: &str| DdlOp::DropIndex {
            table: t(None, "users"),
            name: name.into(),
        };
        let context = PlanContext {
            constraint_indexes: vec!["users_pkey".into(), "users_email_key".into()],
            ..PlanContext::default()
        };
        let p = plan_ddl_with(
            Dialect::Postgres,
            Some(&users),
            &[drop("users_email_key"), drop("users_other")],
            &context,
        )
        .expect("plan");
        assert_eq!(
            p.statements,
            [
                r#"ALTER TABLE "public"."users" DROP CONSTRAINT "users_email_key""#,
                r#"DROP INDEX "public"."users_other""#,
            ]
        );
    }

    // --- checks, triggers, table options, identity, collations (M8) ------------------------

    fn unsupported(d: Dialect, meta: Option<&TableMeta>, ops: Vec<DdlOp>) -> String {
        match plan_ddl(d, meta, &ops) {
            Err(AppError::Unsupported(msg)) => msg,
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    fn check(name: Option<&str>, expression: &str) -> CheckDef {
        CheckDef {
            name: name.map(str::to_string),
            expression: expression.into(),
        }
    }

    #[test]
    fn pg_checks_triggers_and_index_comments() {
        let users = pg_users();
        let tbl = t(None, "users");
        let p = plan(
            Dialect::Postgres,
            Some(&users),
            vec![
                DdlOp::AddCheck {
                    table: tbl.clone(),
                    check: check(Some("users_name_check"), "length(name) > 1"),
                },
                DdlOp::AddCheck {
                    table: tbl.clone(),
                    check: check(None, " name <> '' "),
                },
                DdlOp::DropCheck {
                    table: tbl.clone(),
                    name: "users_email_check".into(),
                },
                DdlOp::DropTrigger {
                    table: tbl.clone(),
                    name: "users_touch".into(),
                },
                DdlOp::CreateTrigger {
                    table: tbl.clone(),
                    name: "users_touch".into(),
                    sql: "CREATE TRIGGER users_touch BEFORE UPDATE ON users FOR EACH ROW EXECUTE FUNCTION touch();\n".into(),
                },
                DdlOp::AddIndex {
                    table: tbl.clone(),
                    index: IndexDef {
                        name: "users_name_idx".into(),
                        columns: vec!["name".into()],
                        unique: false,
                        method: None,
                        predicate: None,
                        comment: Some("for search".into()),
                        include: vec![],
                    },
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                r#"ALTER TABLE "public"."users" ADD CONSTRAINT "users_name_check" CHECK (length(name) > 1)"#,
                r#"ALTER TABLE "public"."users" ADD CHECK (name <> '')"#,
                r#"ALTER TABLE "public"."users" DROP CONSTRAINT "users_email_check""#,
                r#"DROP TRIGGER "users_touch" ON "public"."users""#,
                "CREATE TRIGGER users_touch BEFORE UPDATE ON users FOR EACH ROW EXECUTE FUNCTION touch()",
                r#"CREATE INDEX "users_name_idx" ON "public"."users" ("name")"#,
                r#"COMMENT ON INDEX "public"."users_name_idx" IS 'for search'"#,
            ]
        );
        assert!(p.transactional);
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        let msg = invalid_input(
            Dialect::Postgres,
            Some(&users),
            vec![DdlOp::AddCheck {
                table: tbl.clone(),
                check: check(None, "  "),
            }],
        );
        assert!(msg.contains("CHECK expression"), "{msg}");
    }

    #[test]
    fn pg_trigger_enable_disable_and_index_include() {
        let mut users = pg_users();
        let trigger = |name: &str, enabled: bool| TriggerMeta {
            name: name.into(),
            timing: Some("BEFORE".into()),
            events: vec!["UPDATE".into()],
            enabled,
            definition: format!(
                "CREATE TRIGGER {name} BEFORE UPDATE ON public.users FOR EACH ROW EXECUTE FUNCTION touch()"
            ),
        };
        users.triggers = vec![trigger("users_touch", true), trigger("users_audit", false)];
        let tbl = t(None, "users");
        let cover = IndexDef {
            name: "users_name_cover".into(),
            columns: vec!["name".into()],
            unique: true,
            method: Some("btree".into()),
            predicate: Some("name IS NOT NULL".into()),
            comment: None,
            include: vec!["email".into(), " created_at ".into()],
        };
        let p = plan(
            Dialect::Postgres,
            Some(&users),
            vec![
                DdlOp::DisableTrigger {
                    table: tbl.clone(),
                    name: "users_touch".into(),
                },
                DdlOp::EnableTrigger {
                    table: tbl.clone(),
                    name: "users_audit".into(),
                },
                DdlOp::AddIndex {
                    table: tbl.clone(),
                    index: cover.clone(),
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                r#"ALTER TABLE "public"."users" DISABLE TRIGGER "users_touch""#,
                r#"ALTER TABLE "public"."users" ENABLE TRIGGER "users_audit""#,
                r#"CREATE UNIQUE INDEX "users_name_cover" ON "public"."users" USING btree ("name") INCLUDE ("email", "created_at") WHERE name IS NOT NULL"#,
            ]
        );
        assert!(p.transactional);
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        // without metadata too
        let p = plan(
            Dialect::Postgres,
            None,
            vec![DdlOp::DisableTrigger {
                table: t(Some("app"), "orders"),
                name: "Audit".into(),
            }],
        );
        assert_eq!(
            p.statements,
            [r#"ALTER TABLE "app"."orders" DISABLE TRIGGER "Audit""#]
        );
        let msg = invalid_input(
            Dialect::Postgres,
            Some(&users),
            vec![DdlOp::EnableTrigger {
                table: tbl.clone(),
                name: " ".into(),
            }],
        );
        assert!(msg.contains("trigger name"), "{msg}");
        let msg = invalid_input(
            Dialect::Postgres,
            Some(&users),
            vec![DdlOp::AddIndex {
                table: tbl.clone(),
                index: IndexDef {
                    include: vec!["".into()],
                    ..cover.clone()
                },
            }],
        );
        assert!(msg.contains("index column"), "{msg}");

        // The table's DDL keeps a disabled trigger disabled.
        let parts = table_parts(Dialect::Postgres, &users, &PgTableContext::default());
        assert_eq!(
            parts.triggers,
            [
                "CREATE TRIGGER users_touch BEFORE UPDATE ON public.users FOR EACH ROW EXECUTE FUNCTION touch();",
                "CREATE TRIGGER users_audit BEFORE UPDATE ON public.users FOR EACH ROW EXECUTE FUNCTION touch();",
                r#"ALTER TABLE "public"."users" DISABLE TRIGGER "users_audit";"#,
            ]
        );
    }

    #[test]
    fn trigger_states_and_include_columns_elsewhere() {
        // MySQL and SQLite have no disabled triggers: refused.
        for (d, meta) in [
            (Dialect::Mysql, Some(my_items())),
            (Dialect::Mysql, None),
            (Dialect::Sqlite, Some(lite_items())),
        ] {
            let table = meta
                .as_ref()
                .map_or_else(|| t(None, "items"), |m| m.table.clone());
            for op in [
                DdlOp::DisableTrigger {
                    table: table.clone(),
                    name: "items_touch".into(),
                },
                DdlOp::EnableTrigger {
                    table: table.clone(),
                    name: "items_touch".into(),
                },
            ] {
                let err = plan_ddl(d, meta.as_ref(), &[op]).unwrap_err();
                assert!(matches!(err, AppError::Unsupported(_)), "{d:?}: {err:?}");
            }
        }
        // INCLUDE is PostgreSQL-only: ignored with a warning.
        let index = IndexDef {
            name: "items_name_idx".into(),
            columns: vec!["name".into()],
            unique: false,
            method: None,
            predicate: None,
            comment: None,
            include: vec!["qty".into()],
        };
        for (d, expected) in [
            (
                Dialect::Mysql,
                "CREATE INDEX `items_name_idx` ON `items` (`name`)",
            ),
            (
                Dialect::Sqlite,
                r#"CREATE INDEX "items_name_idx" ON "items" ("name")"#,
            ),
        ] {
            let p = plan(
                d,
                None,
                vec![DdlOp::AddIndex {
                    table: t(None, "items"),
                    index: index.clone(),
                }],
            );
            assert_eq!(p.statements, [expected], "{d:?}");
            assert!(
                p.warnings.iter().any(|w| w.contains("INCLUDE")),
                "{d:?}: {:?}",
                p.warnings
            );
        }
    }

    #[test]
    fn pg_include_columns_are_quoted_once() {
        // Introspection reports INCLUDE columns by name (`Email`, `first name`), never quoted.
        let mut people = meta(
            t(Some("public"), "People"),
            vec![
                cm("id", "bigint", false),
                cm("Email", "text", true),
                cm("first name", "text", true),
            ],
            &["id"],
        );
        let mut cover = index("People_cover", &["id"], true, false, None);
        cover.include = vec!["Email".into(), "first name".into()];
        people.indexes = vec![cover];
        let table = people.table.clone();
        let edited = IndexDef {
            name: "People_cover".into(),
            columns: vec!["id".into()],
            unique: false,
            method: None,
            predicate: None,
            comment: None,
            include: vec!["Email".into(), "first name".into()],
        };
        // Editing the covering index: drop, then create it again with its INCLUDE columns.
        let p = plan(
            Dialect::Postgres,
            Some(&people),
            vec![
                DdlOp::DropIndex {
                    table: table.clone(),
                    name: "People_cover".into(),
                },
                DdlOp::AddIndex {
                    table: table.clone(),
                    index: edited,
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                r#"DROP INDEX "public"."People_cover""#,
                r#"CREATE INDEX "People_cover" ON "public"."People" ("id") INCLUDE ("Email", "first name")"#,
            ]
        );
        // The table's DDL renders an index without a stored definition the same way.
        let parts = table_parts(Dialect::Postgres, &people, &PgTableContext::default());
        assert_eq!(
            parts.indexes,
            [
                r#"CREATE UNIQUE INDEX "People_cover" ON "public"."People" ("id") INCLUDE ("Email", "first name");"#
            ]
        );
    }

    #[test]
    fn partition_objects_and_shared_sequence_resets() {
        let partition = PgPartition {
            table: t(Some("public"), "m_eu"),
            parent: t(Some("public"), "m"),
            bound: "FOR VALUES IN ('eu')".into(),
            key: Some("RANGE (ts)".into()),
            objects: vec![
                PgPartitionObject {
                    kind: PgPartitionObjectKind::Index,
                    name: "m_eu_ts".into(),
                    definition: r#"CREATE INDEX "x ON ONLY y" ON ONLY public.m_eu USING btree (ts)"#
                        .into(),
                    disabled: false,
                },
                PgPartitionObject {
                    kind: PgPartitionObjectKind::Constraint,
                    name: "m_eu_pk".into(),
                    definition: "PRIMARY KEY (id, ts)".into(),
                    disabled: false,
                },
                PgPartitionObject {
                    kind: PgPartitionObjectKind::Trigger,
                    name: "m_eu_trg".into(),
                    definition:
                        "CREATE TRIGGER m_eu_trg AFTER INSERT ON public.m_eu FOR EACH ROW EXECUTE FUNCTION f()"
                            .into(),
                    disabled: true,
                },
                PgPartitionObject {
                    kind: PgPartitionObjectKind::Trigger,
                    name: "m_eu_on".into(),
                    definition:
                        "CREATE TRIGGER m_eu_on AFTER DELETE ON public.m_eu FOR EACH ROW EXECUTE FUNCTION f();"
                            .into(),
                    disabled: false,
                },
            ],
        };
        let (indexes, triggers) = partition_objects_sql(&partition);
        assert_eq!(
            indexes,
            [
                r#"CREATE INDEX "x ON ONLY y" ON public.m_eu USING btree (ts);"#,
                r#"ALTER TABLE "public"."m_eu" ADD CONSTRAINT "m_eu_pk" PRIMARY KEY (id, ts);"#,
            ]
        );
        assert_eq!(
            triggers,
            [
                "CREATE TRIGGER m_eu_trg AFTER INSERT ON public.m_eu FOR EACH ROW EXECUTE FUNCTION f();",
                r#"ALTER TABLE "public"."m_eu" DISABLE TRIGGER "m_eu_trg";"#,
                "CREATE TRIGGER m_eu_on AFTER DELETE ON public.m_eu FOR EACH ROW EXECUTE FUNCTION f();",
            ]
        );

        let descending = PgSequence {
            column: "id".into(),
            schema: "public".into(),
            name: "down_seq".into(),
            increment: Some(-1),
            data_type: Some("bigint".into()),
            ..PgSequence::default()
        };
        assert_eq!(
            shared_sequence_reset_sql(
                &descending,
                &[
                    (t(Some("public"), "a"), "id".into()),
                    (t(Some("s"), "b"), "Key".into())
                ]
            ),
            r#"SELECT setval('"public"."down_seq"', LEAST(v, -1)) FROM (SELECT LEAST((SELECT MIN("id") FROM "public"."a"), (SELECT MIN("Key") FROM "s"."b")) AS v) AS s WHERE v IS NOT NULL;"#
        );
    }

    #[test]
    fn pg_identity_on_and_off() {
        let mut users = pg_users();
        let mut legacy = cm("legacy_id", "integer", true);
        legacy.default_expr = Some("0".into());
        users.columns.push(legacy);
        let mut serial = cm("seq_id", "integer", false);
        serial.default_expr = Some("nextval('users_seq_id_seq'::regclass)".into());
        serial.is_auto_increment = true;
        users.columns.push(serial);
        let tbl = t(None, "users");
        let ai = |name: &str, on: bool| DdlOp::AlterColumnAutoIncrement {
            table: tbl.clone(),
            name: name.into(),
            auto_increment: on,
        };
        let p = plan(Dialect::Postgres, Some(&users), vec![ai("legacy_id", true)]);
        assert_eq!(
            p.statements,
            [
                r#"ALTER TABLE "public"."users" ALTER COLUMN "legacy_id" DROP DEFAULT"#,
                r#"ALTER TABLE "public"."users" ALTER COLUMN "legacy_id" SET NOT NULL"#,
                r#"ALTER TABLE "public"."users" ALTER COLUMN "legacy_id" ADD GENERATED BY DEFAULT AS IDENTITY"#,
                r#"SELECT setval(pg_get_serial_sequence('"public"."users"', 'legacy_id'), GREATEST(COALESCE(MAX("legacy_id"), 0) + 1, 1), false) FROM "public"."users""#,
            ]
        );
        // A serial column: its old sequence is detached first, so `pg_get_serial_sequence`
        // (and the setval) reaches the identity sequence, not the old one.
        let context = PlanContext {
            owned_sequences: vec![("seq_id".into(), t(Some("public"), "users_seq_id_seq"))],
            ..PlanContext::default()
        };
        let p = plan_ddl_with(
            Dialect::Postgres,
            Some(&users),
            &[ai("seq_id", true)],
            &context,
        )
        .unwrap();
        assert_eq!(
            p.statements,
            [
                r#"ALTER TABLE "public"."users" ALTER COLUMN "seq_id" DROP DEFAULT"#,
                r#"ALTER SEQUENCE "public"."users_seq_id_seq" OWNED BY NONE"#,
                r#"ALTER TABLE "public"."users" ALTER COLUMN "seq_id" ADD GENERATED BY DEFAULT AS IDENTITY"#,
                r#"SELECT setval(pg_get_serial_sequence('"public"."users"', 'seq_id'), GREATEST(COALESCE(MAX("seq_id"), 0) + 1, 1), false) FROM "public"."users""#,
            ]
        );
        assert_eq!(p.warnings.len(), 1, "{:?}", p.warnings);
        assert!(p.warnings[0].contains("users_seq_id_seq"));
        // …also after a rename earlier in the plan
        let p = plan_ddl_with(
            Dialect::Postgres,
            Some(&users),
            &[
                DdlOp::RenameColumn {
                    table: tbl.clone(),
                    name: "seq_id".into(),
                    new_name: "n".into(),
                },
                ai("n", true),
            ],
            &context,
        )
        .unwrap();
        assert!(
            p.statements.contains(
                &r#"ALTER SEQUENCE "public"."users_seq_id_seq" OWNED BY NONE"#.to_string()
            ),
            "{:?}",
            p.statements
        );
        let p = plan(
            Dialect::Postgres,
            Some(&users),
            vec![ai("id", false), ai("seq_id", false)],
        );
        assert_eq!(
            p.statements,
            [
                r#"ALTER TABLE "public"."users" ALTER COLUMN "id" DROP IDENTITY IF EXISTS"#,
                r#"ALTER TABLE "public"."users" ALTER COLUMN "seq_id" DROP DEFAULT"#,
            ]
        );
        // already an identity: nothing to do
        let p = plan(Dialect::Postgres, Some(&users), vec![ai("id", true)]);
        assert!(p.statements.is_empty());
        assert_eq!(p.warnings, [r#""id" already is an identity column"#]);
        // the model follows: a later MODIFY-like reconstruction sees the identity
        let ddl = create_table_sql(Dialect::Postgres, &users);
        assert!(
            ddl.contains(r#""id" bigint GENERATED BY DEFAULT AS IDENTITY NOT NULL"#),
            "{ddl}"
        );
    }

    #[test]
    fn pg_collations_and_mysql_only_options() {
        let users = pg_users();
        let tbl = t(None, "users");
        let p = plan(
            Dialect::Postgres,
            Some(&users),
            vec![
                DdlOp::AlterColumnCollation {
                    table: tbl.clone(),
                    name: "email".into(),
                    collation: Some("C".into()),
                },
                DdlOp::AlterColumnCollation {
                    table: tbl.clone(),
                    name: "name".into(),
                    collation: None,
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                r#"ALTER TABLE "public"."users" ALTER COLUMN "email" TYPE character varying(255) COLLATE "C""#,
                r#"ALTER TABLE "public"."users" ALTER COLUMN "name" TYPE text COLLATE "default""#,
            ]
        );
        let msg = unsupported(
            Dialect::Postgres,
            Some(&users),
            vec![DdlOp::AlterColumnOnUpdate {
                table: tbl.clone(),
                name: "name".into(),
                on_update: Some("now()".into()),
            }],
        );
        assert!(msg.contains("MySQL-only"), "{msg}");
        let msg = unsupported(
            Dialect::Postgres,
            Some(&users),
            vec![DdlOp::AlterTableOptions {
                table: tbl.clone(),
                options: TableOptions {
                    engine: Some("InnoDB".into()),
                    ..TableOptions::default()
                },
            }],
        );
        assert!(msg.contains("MySQL table options"), "{msg}");
        let msg = invalid_input(
            Dialect::Postgres,
            Some(&users),
            vec![DdlOp::AlterColumnCollation {
                table: tbl,
                name: "name".into(),
                collation: Some("bad name".into()),
            }],
        );
        assert!(msg.contains("invalid collation"), "{msg}");
    }

    #[test]
    fn pg_reconstruction_lists_checks() {
        let mut users = pg_users();
        users.checks = vec![CheckMeta {
            name: "users_name_check".into(),
            expression: "length(name) > 1".into(),
            columns: vec!["name".into()],
        }];
        users.indexes[1].comment = Some("unique login".into());
        let ddl = create_table_sql(Dialect::Postgres, &users);
        assert!(
            ddl.contains(
                "  CONSTRAINT \"users_pkey\" PRIMARY KEY (\"id\"),\n  CONSTRAINT \"users_name_check\" CHECK (length(name) > 1)\n);"
            ),
            "{ddl}"
        );
        assert!(
            ddl.contains(r#"COMMENT ON INDEX "public"."users_email_key" IS 'unique login';"#),
            "{ddl}"
        );
        // dropping the column drops the check (as PostgreSQL does)
        let p = plan(
            Dialect::Postgres,
            Some(&users),
            vec![DdlOp::DropColumn {
                table: t(None, "users"),
                name: "name".into(),
            }],
        );
        assert_eq!(
            p.statements,
            [r#"ALTER TABLE "public"."users" DROP COLUMN "name""#]
        );
    }

    #[test]
    fn mysql_checks_options_and_extra() {
        let items = my_items();
        let tbl = t(None, "items");
        let p = plan(
            Dialect::Mysql,
            Some(&items),
            vec![
                DdlOp::AddCheck {
                    table: tbl.clone(),
                    check: check(Some("qty_max"), "qty < 1000"),
                },
                DdlOp::DropCheck {
                    table: tbl.clone(),
                    name: "items_chk_1".into(),
                },
                DdlOp::AlterColumnOnUpdate {
                    table: tbl.clone(),
                    name: "touched_at".into(),
                    on_update: None,
                },
                DdlOp::AlterColumnAutoIncrement {
                    table: tbl.clone(),
                    name: "id".into(),
                    auto_increment: false,
                },
                DdlOp::AlterColumnCollation {
                    table: tbl.clone(),
                    name: "name".into(),
                    collation: Some("utf8mb4_unicode_ci".into()),
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                "ALTER TABLE `shop`.`items` ADD CONSTRAINT `qty_max` CHECK (qty < 1000)",
                "ALTER TABLE `shop`.`items` DROP CONSTRAINT `items_chk_1`",
                "ALTER TABLE `shop`.`items` MODIFY COLUMN `touched_at` timestamp(3) NULL DEFAULT CURRENT_TIMESTAMP(3)",
                "ALTER TABLE `shop`.`items` MODIFY COLUMN `id` int unsigned NOT NULL",
                "ALTER TABLE `shop`.`items` MODIFY COLUMN `name` varchar(50) COLLATE utf8mb4_unicode_ci NULL DEFAULT 'it''s'",
            ]
        );
        assert!(!p.transactional);
        // AUTO_INCREMENT on, with ON UPDATE back on another column; the default goes (MySQL
        // refuses one on an AUTO_INCREMENT column)
        let p = plan(
            Dialect::Mysql,
            Some(&items),
            vec![
                DdlOp::AlterColumnAutoIncrement {
                    table: tbl.clone(),
                    name: "qty".into(),
                    auto_increment: true,
                },
                DdlOp::AlterColumnOnUpdate {
                    table: tbl.clone(),
                    name: "name".into(),
                    on_update: Some("CURRENT_TIMESTAMP".into()),
                },
            ],
        );
        assert_eq!(
            p.statements[0],
            "ALTER TABLE `shop`.`items` MODIFY COLUMN `qty` int unsigned NOT NULL AUTO_INCREMENT COMMENT 'how many'"
        );
        assert!(
            p.statements[1].ends_with("ON UPDATE CURRENT_TIMESTAMP"),
            "{}",
            p.statements[1]
        );
    }

    #[test]
    fn mysql_table_options_and_triggers() {
        let items = my_items();
        let tbl = t(None, "items");
        let opts = |o: TableOptions| DdlOp::AlterTableOptions {
            table: tbl.clone(),
            options: o,
        };
        let p = plan(
            Dialect::Mysql,
            Some(&items),
            vec![
                opts(TableOptions {
                    engine: Some("MyISAM".into()),
                    charset: None,
                    collation: Some("utf8mb4_bin".into()),
                    auto_increment: Some(100),
                    row_format: Some("dynamic".into()),
                }),
                opts(TableOptions {
                    charset: Some("latin1".into()),
                    ..TableOptions::default()
                }),
                DdlOp::SetTableComment {
                    table: tbl.clone(),
                    comment: Some("stock".into()),
                },
                DdlOp::DropTrigger {
                    table: tbl.clone(),
                    name: "items_bi".into(),
                },
                DdlOp::CreateTrigger {
                    table: tbl.clone(),
                    name: "items_bi".into(),
                    sql: "CREATE TRIGGER items_bi BEFORE INSERT ON items FOR EACH ROW BEGIN SET NEW.qty = 0; END;".into(),
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                "ALTER TABLE `shop`.`items` ENGINE = MyISAM, CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_bin, AUTO_INCREMENT = 100, ROW_FORMAT = DYNAMIC",
                "ALTER TABLE `shop`.`items` CONVERT TO CHARACTER SET latin1",
                "ALTER TABLE `shop`.`items` COMMENT = 'stock'",
                "DROP TRIGGER `shop`.`items_bi`",
                "CREATE TRIGGER items_bi BEFORE INSERT ON items FOR EACH ROW BEGIN SET NEW.qty = 0; END",
            ]
        );
        assert!(
            p.warnings
                .iter()
                .any(|w| w.contains("old trigger is already dropped")),
            "{:?}",
            p.warnings
        );
        let msg = invalid_input(
            Dialect::Mysql,
            Some(&items),
            vec![opts(TableOptions::default())],
        );
        assert!(msg.contains("no table option"), "{msg}");
        let msg = invalid_input(
            Dialect::Mysql,
            Some(&items),
            vec![opts(TableOptions {
                engine: Some("Inno DB; DROP".into()),
                ..TableOptions::default()
            })],
        );
        assert!(msg.contains("invalid engine"), "{msg}");
        // index comments are an index option
        let p = plan(
            Dialect::Mysql,
            Some(&items),
            vec![DdlOp::AddIndex {
                table: tbl,
                index: IndexDef {
                    name: "items_name".into(),
                    columns: vec!["name".into()],
                    unique: false,
                    method: Some("BTREE".into()),
                    predicate: None,
                    comment: Some("by name".into()),
                    include: vec![],
                },
            }],
        );
        assert_eq!(
            p.statements,
            ["CREATE INDEX `items_name` ON `shop`.`items` (`name`) USING BTREE COMMENT 'by name'"]
        );
    }

    #[test]
    fn on_update_of_a_new_column_is_mysql_only() {
        let mut col = cdef("updated_at", "timestamp", true);
        col.default_expr = Some("CURRENT_TIMESTAMP".into());
        col.on_update = Some("CURRENT_TIMESTAMP".into());
        let add = |table: TableRef| DdlOp::AddColumn {
            table,
            column: col.clone(),
            after: None,
        };
        let p = plan(Dialect::Mysql, None, vec![add(t(Some("shop"), "items"))]);
        assert_eq!(
            p.statements,
            [
                "ALTER TABLE `shop`.`items` ADD COLUMN `updated_at` timestamp NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP"
            ]
        );
        let p = plan(
            Dialect::Postgres,
            None,
            vec![add(t(Some("public"), "items"))],
        );
        assert_eq!(
            p.statements,
            [
                r#"ALTER TABLE "public"."items" ADD COLUMN "updated_at" timestamp DEFAULT CURRENT_TIMESTAMP"#
            ]
        );
        assert_eq!(
            p.warnings,
            [r#"ON UPDATE is MySQL-only; ignored for column "updated_at""#]
        );
    }

    /// `items(id INTEGER PRIMARY KEY, price REAL CHECK (price > 0), qty INT,
    /// CONSTRAINT cheap CHECK (price < 1000))`.
    fn lite_checked() -> TableMeta {
        let mut id = cm("id", "INTEGER", false);
        id.is_pk = true;
        id.is_auto_increment = true;
        id.extra = Some("rowid alias".into());
        let mut m = meta(
            t(Some("main"), "items"),
            vec![id, cm("price", "REAL", true), cm("qty", "INT", true)],
            &["id"],
        );
        m.ddl = Some(
            "CREATE TABLE items (id INTEGER PRIMARY KEY, price REAL CHECK (price > 0), qty INT, CONSTRAINT cheap CHECK (price < 1000))"
                .into(),
        );
        m.triggers = vec![TriggerMeta {
            name: "items_touch".into(),
            timing: Some("AFTER".into()),
            events: vec!["UPDATE".into()],
            enabled: true,
            definition: "CREATE TRIGGER items_touch AFTER UPDATE ON items BEGIN SELECT 1; END"
                .into(),
        }];
        m
    }

    fn create_of(p: &DdlPlan) -> &str {
        p.statements
            .iter()
            .find(|s| s.starts_with("CREATE TABLE"))
            .map(String::as_str)
            .unwrap_or_else(|| panic!("no CREATE TABLE in {:#?}", p.statements))
    }

    #[test]
    fn sqlite_checks_go_through_the_rebuild() {
        let items = lite_checked();
        let tbl = t(None, "items");
        // an unrelated rebuild keeps both checks (named and unnamed)
        let p = plan(
            Dialect::Sqlite,
            Some(&items),
            vec![DdlOp::AlterColumnType {
                table: tbl.clone(),
                name: "qty".into(),
                new_type: "INTEGER".into(),
                using: None,
            }],
        );
        assert_eq!(
            create_of(&p),
            "CREATE TABLE \"main\".\"items__tabler_new\" (\n  \"id\" INTEGER,\n  \"price\" REAL,\n  \"qty\" INTEGER,\n  PRIMARY KEY (\"id\"),\n  CHECK (price > 0),\n  CONSTRAINT \"cheap\" CHECK (price < 1000)\n)"
        );
        assert!(
            !p.warnings.iter().any(|w| w.contains("CHECK")),
            "{:?}",
            p.warnings
        );
        // add one, drop the unnamed one
        let p = plan(
            Dialect::Sqlite,
            Some(&items),
            vec![
                DdlOp::AddCheck {
                    table: tbl.clone(),
                    check: check(Some("qty_ok"), "qty >= 0"),
                },
                DdlOp::DropCheck {
                    table: tbl.clone(),
                    name: "ck_items_1".into(),
                },
            ],
        );
        let create = create_of(&p);
        assert!(!create.contains("price > 0"), "{create}");
        assert!(create.contains("  CONSTRAINT \"cheap\" CHECK (price < 1000),\n  CONSTRAINT \"qty_ok\" CHECK (qty >= 0)\n)"), "{create}");
        assert_eq!(p.statements[0], "PRAGMA foreign_keys = OFF");
        assert!(p.statements.contains(
            &"CREATE TRIGGER items_touch AFTER UPDATE ON items BEGIN SELECT 1; END".to_string()
        ));
        // a check added after a rename spells the new name; the new table still has the old one
        let p = plan(
            Dialect::Sqlite,
            Some(&items),
            vec![
                DdlOp::RenameColumn {
                    table: tbl.clone(),
                    name: "price".into(),
                    new_name: "cost".into(),
                },
                DdlOp::AddCheck {
                    table: tbl.clone(),
                    check: check(None, "cost < 5"),
                },
            ],
        );
        assert!(
            create_of(&p).contains("  CHECK (\"price\" < 5)\n)"),
            "{}",
            create_of(&p)
        );
        assert!(
            p.statements.contains(
                &"ALTER TABLE \"main\".\"items\" RENAME COLUMN \"price\" TO \"cost\"".to_string()
            ),
            "{:#?}",
            p.statements
        );
        // dropping a column drops its checks, with a note
        let p = plan(
            Dialect::Sqlite,
            Some(&items),
            vec![DdlOp::DropColumn {
                table: tbl.clone(),
                name: "price".into(),
            }],
        );
        assert!(!create_of(&p).contains("CHECK"), "{}", create_of(&p));
        assert!(
            p.warnings
                .iter()
                .any(|w| w.contains("CHECK constraint \"cheap\" uses \"price\"")),
            "{:?}",
            p.warnings
        );
        // unknown check
        let msg = invalid_input(
            Dialect::Sqlite,
            Some(&items),
            vec![DdlOp::DropCheck {
                table: tbl.clone(),
                name: "nope".into(),
            }],
        );
        assert!(msg.contains("\"nope\" does not exist"), "{msg}");
        // without metadata there is no rebuild to do it with
        let msg = invalid_input(
            Dialect::Sqlite,
            None,
            vec![DdlOp::AddCheck {
                table: tbl,
                check: check(None, "1"),
            }],
        );
        assert!(msg.contains("needs the table's metadata"), "{msg}");
    }

    #[test]
    fn sqlite_autoincrement_triggers_and_collations() {
        let items = lite_checked();
        let tbl = t(None, "items");
        let p = plan(
            Dialect::Sqlite,
            Some(&items),
            vec![
                DdlOp::AlterColumnAutoIncrement {
                    table: tbl.clone(),
                    name: "id".into(),
                    auto_increment: true,
                },
                DdlOp::AlterColumnCollation {
                    table: tbl.clone(),
                    name: "price".into(),
                    collation: Some("NOCASE".into()),
                },
            ],
        );
        let create = create_of(&p);
        assert!(
            create.contains(
                "  \"id\" INTEGER PRIMARY KEY AUTOINCREMENT,\n  \"price\" REAL COLLATE \"NOCASE\","
            ),
            "{create}"
        );
        // AUTOINCREMENT on a column that cannot have it
        let p = plan(
            Dialect::Sqlite,
            Some(&items),
            vec![DdlOp::AlterColumnAutoIncrement {
                table: tbl.clone(),
                name: "qty".into(),
                auto_increment: true,
            }],
        );
        assert!(
            p.warnings.iter().any(|w| w.contains("ignored for \"qty\"")),
            "{:?}",
            p.warnings
        );
        // triggers: dropped and created in place, after a pending rebuild
        let p = plan(
            Dialect::Sqlite,
            Some(&items),
            vec![
                DdlOp::DropCheck {
                    table: tbl.clone(),
                    name: "cheap".into(),
                },
                DdlOp::DropTrigger {
                    table: tbl.clone(),
                    name: "items_touch".into(),
                },
                DdlOp::CreateTrigger {
                    table: tbl.clone(),
                    name: "items_touch".into(),
                    sql: "CREATE TRIGGER items_touch AFTER INSERT ON items BEGIN SELECT 2; END"
                        .into(),
                },
            ],
        );
        // (inside the rebuild's BEGIN … COMMIT block, after the old trigger was replayed)
        let n = p.statements.len();
        assert_eq!(
            &p.statements[n - 5..n - 2],
            [
                "DROP TRIGGER \"main\".\"items_touch\"",
                "CREATE TRIGGER items_touch AFTER INSERT ON items BEGIN SELECT 2; END",
                &p.statements[n - 3],
            ]
        );
        assert!(is_fk_check(&p.statements[n - 3]));
        assert_eq!(
            &p.statements[n - 2..],
            ["COMMIT", "PRAGMA foreign_keys = ON"]
        );
        assert!(!create_of(&p).contains("cheap"), "{}", create_of(&p));
        let msg = unsupported(
            Dialect::Sqlite,
            Some(&items),
            vec![DdlOp::AlterColumnOnUpdate {
                table: tbl,
                name: "qty".into(),
                on_update: None,
            }],
        );
        assert!(msg.contains("MySQL-only"), "{msg}");
    }

    #[test]
    fn parses_check_expressions_and_index_predicates() {
        assert_eq!(
            check_expression("CHECK ((price > (0)::numeric))"),
            "price > (0)::numeric"
        );
        assert_eq!(
            check_expression("CHECK ((a > 0) AND (b > 0)) NOT VALID"),
            "(a > 0) AND (b > 0)"
        );
        assert_eq!(check_expression("(`price` > 0)"), "`price` > 0");
        assert_eq!(check_expression("json_valid(`doc`)"), "json_valid(`doc`)");
        assert_eq!(
            check_columns(
                "\"Price\" > 0 AND qty <> 'price'",
                &["price".into(), "qty".into(), "id".into()]
            ),
            ["price", "qty"]
        );
        assert_eq!(
            index_predicate("CREATE INDEX i ON t (a, (lower(b))) WHERE a IS NOT NULL;").as_deref(),
            Some("a IS NOT NULL")
        );
        assert_eq!(index_predicate("CREATE INDEX i ON t (a)"), None);
        assert_eq!(index_predicate("CREATE INDEX i ON t (\"where\")"), None);
        let checks = sqlite_checks(
            "CREATE TABLE \"t\" (\n  a INT CONSTRAINT \"pos\" CHECK (a > 0) NOT NULL, -- CHECK (no)\n  b TEXT DEFAULT ('CHECK (x)') CHECK(b IN ('x', 'y')),\n  [c d] INT CHECK ([c d] < a),\n  CHECK (a < 10)\n)",
            "t",
            &["a".into(), "b".into(), "c d".into()],
        );
        let got: Vec<(String, String, Vec<String>)> = checks
            .into_iter()
            .map(|c| (c.name, c.expression, c.columns))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "pos".to_string(),
                    "a > 0".to_string(),
                    vec!["a".to_string()]
                ),
                ("ck_t_1".into(), "b IN ('x', 'y')".into(), vec!["b".into()]),
                (
                    "ck_t_2".into(),
                    "[c d] < a".into(),
                    vec!["c d".into(), "a".into()]
                ),
                ("ck_t_3".into(), "a < 10".into(), vec!["a".into()]),
            ]
        );
        assert_eq!(
            rename_all(
                Dialect::Sqlite,
                "a < b AND 'a' <> b",
                &[("a".into(), "b".into()), ("b".into(), "a".into())]
            ),
            "\"b\" < \"a\" AND 'a' <> \"a\""
        );
    }

    // --- PG context: sequences, partitions, clones (export / Show DDL) ------------------------

    fn serial_users() -> (TableMeta, PgTableContext) {
        let mut id = cm("id", "integer", false);
        id.is_pk = true;
        id.is_auto_increment = true;
        id.default_expr = Some("nextval('users_id_seq'::regclass)".into());
        let mut code = cm("code", "bigint", true);
        code.default_expr = Some("nextval('shared_seq'::regclass)".into());
        let mut m = meta(t(Some("public"), "users"), vec![id, code], &["id"]);
        m.indexes = vec![index("users_pkey", &["id"], true, true, None)];
        m.foreign_keys = vec![ForeignKeyMeta {
            name: "users_boss_fk".into(),
            columns: vec!["code".into()],
            ref_table: t(Some("public"), "users"),
            ref_columns: vec!["id".into()],
            on_update: "NO ACTION".into(),
            on_delete: "NO ACTION".into(),
            definition: Some("FOREIGN KEY (code) REFERENCES users(id)".into()),
        }];
        let context = PgTableContext {
            sequences: vec![
                PgSequence {
                    column: "id".into(),
                    schema: "public".into(),
                    name: "users_id_seq".into(),
                    owned: true,
                    data_type: Some("integer".into()),
                    start: Some(1),
                    increment: Some(1),
                    min: Some(1),
                    max: Some(i32::MAX.into()),
                    cache: Some(1),
                    cycle: false,
                },
                PgSequence {
                    column: "code".into(),
                    schema: "public".into(),
                    name: "shared_seq".into(),
                    owned: false,
                    data_type: Some("bigint".into()),
                    start: Some(1000),
                    increment: Some(10),
                    min: Some(1000),
                    max: Some(i64::MAX),
                    cache: Some(5),
                    cycle: true,
                },
            ],
            ..PgTableContext::default()
        };
        (m, context)
    }

    #[test]
    fn pg_serial_columns_get_their_sequences_and_a_reset() {
        let (m, context) = serial_users();
        let parts = table_parts(Dialect::Postgres, &m, &context);
        assert_eq!(
            parts.sequences,
            [
                r#"CREATE SEQUENCE IF NOT EXISTS "public"."users_id_seq" AS integer;"#,
                r#"CREATE SEQUENCE IF NOT EXISTS "public"."shared_seq" AS bigint INCREMENT BY 10 MINVALUE 1000 CACHE 5 CYCLE;"#,
            ]
        );
        assert!(
            parts.table[0]
                .contains(r#""id" integer NOT NULL DEFAULT nextval('users_id_seq'::regclass)"#),
            "{:?}",
            parts.table
        );
        // only the owned sequence is owned again (and reset): the shared one is not this
        // table's to move
        assert_eq!(
            parts.table[1],
            r#"ALTER SEQUENCE "public"."users_id_seq" OWNED BY "public"."users"."id";"#
        );
        assert_eq!(parts.table.len(), 2);
        assert_eq!(
            parts.sequence_resets,
            [
                r#"SELECT setval(pg_get_serial_sequence('"public"."users"', 'id'), GREATEST(MAX("id"), 1)) FROM "public"."users" HAVING MAX("id") IS NOT NULL;"#
            ]
        );
        assert_eq!(parts.foreign_keys.len(), 1);
        // Show DDL: the sequences come first
        let ddl = create_table_sql_with(Dialect::Postgres, &m, &context);
        assert!(
            ddl.starts_with("CREATE SEQUENCE IF NOT EXISTS \"public\".\"users_id_seq\" AS integer;\nCREATE SEQUENCE"),
            "{ddl}"
        );
        // without a context the script is the one `TableMeta` alone gives
        assert_eq!(
            create_table_sql(Dialect::Postgres, &m),
            table_parts(Dialect::Postgres, &m, &PgTableContext::default()).script()
        );
        assert!(!create_table_sql(Dialect::Postgres, &m).contains("SEQUENCE"));
    }

    #[test]
    fn pg_identity_columns_are_reset_after_the_rows() {
        let users = pg_users();
        let parts = table_parts(Dialect::Postgres, &users, &PgTableContext::default());
        assert_eq!(
            parts.sequence_resets,
            [
                r#"SELECT setval(pg_get_serial_sequence('"public"."users"', 'id'), GREATEST(MAX("id"), 1)) FROM "public"."users" HAVING MAX("id") IS NOT NULL;"#
            ]
        );
        // a descending serial sequence follows the smallest key
        let (m, mut context) = serial_users();
        context.sequences[0].increment = Some(-1);
        context.sequences[0].min = Some(i32::MIN.into());
        context.sequences[0].max = Some(-1);
        context.sequences[0].start = Some(-1);
        let parts = table_parts(Dialect::Postgres, &m, &context);
        assert_eq!(
            parts.sequences[0],
            r#"CREATE SEQUENCE IF NOT EXISTS "public"."users_id_seq" AS integer INCREMENT BY -1;"#
        );
        assert_eq!(
            parts.sequence_resets,
            [
                r#"SELECT setval(pg_get_serial_sequence('"public"."users"', 'id'), LEAST(MIN("id"), -1)) FROM "public"."users" HAVING MIN("id") IS NOT NULL;"#
            ]
        );
    }

    #[test]
    fn pg_partitioned_tables_and_partitions() {
        // the parent: PARTITION BY
        let mut parent = pg_users();
        parent.kind = TableKind::Partitioned;
        let context = PgTableContext {
            partition_key: Some("RANGE (created_at)".into()),
            ..PgTableContext::default()
        };
        let parts = table_parts(Dialect::Postgres, &parent, &context);
        assert!(
            parts.table[0].ends_with(") PARTITION BY RANGE (created_at);"),
            "{}",
            parts.table[0]
        );
        assert_eq!(parts.attach, None);

        // a partition: a table of its own without an identity (it takes the parent's), its
        // indexes, then the attach; no clones of the parent's keys and triggers
        let mut child = pg_users();
        child.table = t(Some("public"), "users_2024");
        child.foreign_keys = vec![
            ForeignKeyMeta {
                name: "users_org_fk".into(),
                columns: vec!["name".into()],
                ref_table: t(Some("public"), "orgs"),
                ref_columns: vec!["name".into()],
                on_update: "NO ACTION".into(),
                on_delete: "NO ACTION".into(),
                definition: Some("FOREIGN KEY (name) REFERENCES orgs(name)".into()),
            },
            ForeignKeyMeta {
                name: "users_2024_own_fk".into(),
                columns: vec!["email".into()],
                ref_table: t(Some("public"), "emails"),
                ref_columns: vec!["email".into()],
                on_update: "NO ACTION".into(),
                on_delete: "NO ACTION".into(),
                definition: Some("FOREIGN KEY (email) REFERENCES emails(email)".into()),
            },
        ];
        child.triggers = vec![
            TriggerMeta {
                name: "users_touch".into(),
                timing: Some("BEFORE".into()),
                events: vec!["UPDATE".into()],
                enabled: true,
                definition: "CREATE TRIGGER users_touch BEFORE UPDATE ON public.users_2024 FOR EACH ROW EXECUTE FUNCTION touch()".into(),
            },
            TriggerMeta {
                name: "own_trigger".into(),
                timing: Some("AFTER".into()),
                events: vec!["INSERT".into()],
                enabled: true,
                definition: "CREATE TRIGGER own_trigger AFTER INSERT ON public.users_2024 FOR EACH ROW EXECUTE FUNCTION audit()".into(),
            },
        ];
        let context = PgTableContext {
            partition_bound: Some("FOR VALUES FROM ('2024-01-01') TO ('2025-01-01')".into()),
            ancestors: vec![t(Some("public"), "users")],
            inherited_fks: vec!["users_org_fk".into()],
            cloned_triggers: vec!["users_touch".into()],
            ..PgTableContext::default()
        };
        let parts = table_parts(Dialect::Postgres, &child, &context);
        assert!(
            parts.table[0].contains("\"id\" bigint NOT NULL,"),
            "{}",
            parts.table[0]
        );
        assert!(!parts.table[0].contains("IDENTITY"));
        assert_eq!(
            parts.attach.as_deref(),
            Some(
                r#"ALTER TABLE "public"."users" ATTACH PARTITION "public"."users_2024" FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');"#
            )
        );
        assert!(parts.sequence_resets.is_empty());
        assert_eq!(parts.foreign_keys.len(), 1);
        assert!(parts.foreign_keys[0].contains("users_2024_own_fk"));
        assert_eq!(parts.triggers.len(), 1);
        assert!(parts.triggers[0].contains("own_trigger"));
        let sections = parts.create_sections(
            &["CREATE TABLE p PARTITION OF x DEFAULT;".into()],
            &["CREATE INDEX p_local ON p (id);".into()],
        );
        let joined = sections.join("\n\n");
        let index_at = joined.find("CREATE UNIQUE INDEX users_email_key").unwrap();
        let partition_at = joined.find("PARTITION OF x").unwrap();
        let attach_at = joined.find("ATTACH PARTITION").unwrap();
        let local_at = joined.find("CREATE INDEX p_local").unwrap();
        assert!(
            index_at < partition_at && partition_at < attach_at && attach_at < local_at,
            "{joined}"
        );
        assert!(!joined.contains("FOREIGN KEY") && !joined.contains("CREATE TRIGGER"));

        assert_eq!(
            partition_of_sql(&PgPartition {
                table: t(Some("public"), "m_2025"),
                parent: t(Some("public"), "m"),
                bound: "FOR VALUES IN ('eu')".into(),
                key: Some("RANGE (ts)".into()),
                objects: Vec::new(),
            }),
            r#"CREATE TABLE "public"."m_2025" PARTITION OF "public"."m" FOR VALUES IN ('eu') PARTITION BY RANGE (ts);"#
        );
    }

    #[test]
    fn pg_catalog_queries_bind_the_table() {
        let q = pg_context_relation_sql(&t(Some("app"), "users"));
        assert!(
            q.sql.contains("c.relname = $1 AND n.nspname = $2"),
            "{}",
            q.sql
        );
        assert_eq!(
            q.params,
            [Value::Text("users".into()), Value::Text("app".into())]
        );
        let q = pg_context_sequences_sql(&t(None, "users"));
        assert!(q.sql.contains("n.nspname = current_schema()"), "{}", q.sql);
        assert_eq!(q.params, [Value::Text("users".into())]);
        for q in [
            pg_context_ancestors_sql(&t(Some("a"), "b")),
            pg_context_partitions_sql(&t(Some("a"), "b")),
            pg_context_clones_sql(&t(Some("a"), "b")),
            pg_owned_sequences_sql(&t(Some("a"), "b")),
        ] {
            assert!(!q.sql.contains("{schema}"));
            assert_eq!(q.params.len(), 2);
        }
    }

    // --- MySQL: an edited index / CHECK is one ALTER TABLE --------------------------------------

    #[test]
    fn mysql_index_and_check_edits_are_atomic() {
        let mut m = my_items();
        m.indexes
            .push(index("items_qty", &["qty"], false, false, None));
        m.checks = vec![CheckMeta {
            name: "qty_positive".into(),
            expression: "qty > 0".into(),
            columns: vec!["qty".into()],
        }];
        let tbl = t(None, "items");
        let p = plan(
            Dialect::Mysql,
            Some(&m),
            vec![
                DdlOp::DropIndex {
                    table: tbl.clone(),
                    name: "items_qty".into(),
                },
                DdlOp::AddIndex {
                    table: tbl.clone(),
                    index: IndexDef {
                        name: "items_qty".into(),
                        columns: vec!["qty".into(), "name(10)".into()],
                        unique: true,
                        method: Some("btree".into()),
                        predicate: None,
                        comment: Some("it's".into()),
                        include: vec![],
                    },
                },
                DdlOp::DropCheck {
                    table: tbl.clone(),
                    name: "qty_positive".into(),
                },
                DdlOp::AddCheck {
                    table: tbl.clone(),
                    check: CheckDef {
                        name: Some("qty_positive".into()),
                        expression: "qty >= 0".into(),
                    },
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                "ALTER TABLE `shop`.`items` DROP INDEX `items_qty`, ADD UNIQUE INDEX `items_qty` (`qty`, `name`(10)) USING BTREE COMMENT 'it''s'",
                "ALTER TABLE `shop`.`items` DROP CONSTRAINT `qty_positive`, ADD CONSTRAINT `qty_positive` CHECK (qty >= 0)",
            ]
        );
        // the model follows: a later op sees the new index
        let p = plan(
            Dialect::Mysql,
            Some(&m),
            vec![
                DdlOp::DropIndex {
                    table: tbl.clone(),
                    name: "items_qty".into(),
                },
                DdlOp::AddIndex {
                    table: tbl.clone(),
                    index: IndexDef {
                        name: "items_qty2".into(),
                        columns: vec!["qty".into()],
                        unique: false,
                        method: Some("FULLTEXT".into()),
                        predicate: None,
                        comment: None,
                        include: vec![],
                    },
                },
                DdlOp::DropIndex {
                    table: tbl.clone(),
                    name: "items_qty2".into(),
                },
            ],
        );
        assert_eq!(
            p.statements,
            [
                "ALTER TABLE `shop`.`items` DROP INDEX `items_qty`, ADD FULLTEXT INDEX `items_qty2` (`qty`)",
                "DROP INDEX `items_qty2` ON `shop`.`items`",
            ]
        );
        // a creation on its own stays CREATE INDEX; PostgreSQL keeps two statements (its DDL
        // runs in one transaction)
        let p = plan(
            Dialect::Mysql,
            Some(&m),
            vec![DdlOp::AddIndex {
                table: tbl.clone(),
                index: IndexDef {
                    name: "x".into(),
                    columns: vec!["qty".into()],
                    unique: false,
                    method: None,
                    predicate: None,
                    comment: None,
                    include: vec![],
                },
            }],
        );
        assert_eq!(p.statements, ["CREATE INDEX `x` ON `shop`.`items` (`qty`)"]);
        let users = pg_users();
        let p = plan(
            Dialect::Postgres,
            Some(&users),
            vec![
                DdlOp::DropIndex {
                    table: t(None, "users"),
                    name: "users_name_idx".into(),
                },
                DdlOp::AddIndex {
                    table: t(None, "users"),
                    index: IndexDef {
                        name: "users_name_idx".into(),
                        columns: vec!["name".into()],
                        unique: true,
                        method: None,
                        predicate: None,
                        comment: None,
                        include: vec![],
                    },
                },
            ],
        );
        assert_eq!(p.statements.len(), 2);
        assert!(p.transactional);
    }

    #[test]
    fn mysql_literals_follow_the_session_sql_mode() {
        let m = my_items();
        let tbl = t(None, "items");
        let ops = [
            DdlOp::SetTableComment {
                table: tbl.clone(),
                comment: Some(r"C:\temp it's".into()),
            },
            DdlOp::SetColumnComment {
                table: tbl.clone(),
                name: "qty".into(),
                comment: Some(r"a\b".into()),
            },
        ];
        let plan_in = |mode: MysqlLiteralMode| {
            let context = PlanContext {
                mysql_mode: mode,
                ..PlanContext::default()
            };
            plan_ddl_with(Dialect::Mysql, Some(&m), &ops, &context)
                .unwrap()
                .statements
        };
        // backslashes are escapes (the default): doubled
        assert_eq!(
            plan_in(MysqlLiteralMode::BackslashEscapes),
            [
                r"ALTER TABLE `shop`.`items` COMMENT = 'C:\\temp it''s'",
                r"ALTER TABLE `shop`.`items` MODIFY COLUMN `qty` int unsigned NOT NULL DEFAULT 0 COMMENT 'a\\b'",
            ]
        );
        // NO_BACKSLASH_ESCAPES: as they are (doubling would store two); the default from
        // information_schema (`it's`) is spelled the same way
        assert_eq!(
            plan_in(MysqlLiteralMode::NoBackslashEscapes),
            [
                r"ALTER TABLE `shop`.`items` COMMENT = 'C:\temp it''s'",
                r"ALTER TABLE `shop`.`items` MODIFY COLUMN `qty` int unsigned NOT NULL DEFAULT 0 COMMENT 'a\b'",
            ]
        );
        let mut c = cm("c", "varchar(9)", true);
        c.default_expr = Some(r"x\y'z".into());
        assert_eq!(
            mysql_default_from_meta(&c, MysqlLiteralMode::NoBackslashEscapes).unwrap(),
            r"'x\y''z'"
        );
        assert_eq!(
            mysql_default_from_meta(&c, MysqlLiteralMode::BackslashEscapes).unwrap(),
            r"'x\\y''z'"
        );
        // PostgreSQL keeps its quoting whatever the mode
        let users = pg_users();
        let context = PlanContext {
            mysql_mode: MysqlLiteralMode::NoBackslashEscapes,
            ..PlanContext::default()
        };
        let p = plan_ddl_with(
            Dialect::Postgres,
            Some(&users),
            &[DdlOp::SetTableComment {
                table: t(None, "users"),
                comment: Some(r"a\b it's".into()),
            }],
            &context,
        )
        .unwrap();
        assert_eq!(
            p.statements,
            [r#"COMMENT ON TABLE "public"."users" IS 'a\b it''s'"#]
        );
    }
}
