//! SQL of data transfer. Export scripts: `DROP TABLE IF EXISTS`, the table's structure and the
//! session settings around a multi-table script. Export reads: the SELECT of a table export in a
//! core dialect (`sql::export_select`). CSV import: `CREATE TABLE` from the wizard's column types
//! and batched multi-row INSERTs (bind parameters only) with the wizard's insert option: plain
//! INSERT (STOP ON ERROR), REPLACE, or UPDATE ON DUPLICATE.

use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::ddl::create_table_sql;
use crate::sqlgen::narrow::Narrowing;
use crate::sqlgen::select::{BuiltSql, SelectSpec, build_select, pg_cast_type};
use crate::types::import::{ImportColumnDef, ImportType, InsertMode};
use crate::types::schema::{ColumnMeta, TableKind, TableMeta, TableRef, ValueKind};
use crate::types::value::{BYTES_PREVIEW_CAP, Value};

// ---------------------------------------------------------------------------------------------
// Export scripts
// ---------------------------------------------------------------------------------------------

/// How an export script names a table: PostgreSQL qualifies it with its schema, as its
/// reconstructed `CREATE TABLE` does; MySQL and SQLite name it alone, as their own DDL text does
/// (so the script loads into whichever database it is run against).
pub fn script_table(dialect: Dialect, table: &TableRef) -> String {
    match dialect {
        Dialect::Postgres => dialect.qualify(table.schema.as_deref(), &table.name),
        Dialect::Mysql | Dialect::Sqlite => dialect.quote_ident(&table.name),
    }
}

pub fn drop_table_if_exists(dialect: Dialect, table: &TableRef) -> String {
    format!("DROP TABLE IF EXISTS {};", script_table(dialect, table))
}

fn terminated(sql: &str) -> String {
    let sql = sql.trim();
    if sql.ends_with(';') {
        sql.to_string()
    } else {
        format!("{sql};")
    }
}

/// The table's structure for an export script, every statement terminated: the server's own
/// DDL where it has one (MySQL `SHOW CREATE TABLE`; SQLite's stored `CREATE TABLE` plus its
/// `CREATE INDEX` statements), the reconstructed script on PostgreSQL (table, indexes, foreign
/// keys, triggers, comments).
pub fn table_structure(dialect: Dialect, meta: &TableMeta) -> String {
    let own = meta.ddl.as_deref().map(str::trim).filter(|d| !d.is_empty());
    match (dialect, own) {
        (Dialect::Mysql, Some(ddl)) => terminated(ddl),
        (Dialect::Sqlite, Some(ddl)) => {
            let mut out = terminated(ddl);
            for index in meta.indexes.iter().filter(|i| !i.primary) {
                if let Some(def) = index.definition.as_deref().filter(|d| !d.trim().is_empty()) {
                    out.push('\n');
                    out.push_str(&terminated(def));
                }
            }
            out
        }
        _ => create_table_sql(dialect, meta).trim().to_string(),
    }
}

/// The session time zone an export script assumes when the exporting session's own could not be
/// read (see `export::session_time_zone`).
pub fn default_time_zone(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Mysql => "+00:00",
        Dialect::Postgres => "UTC",
        Dialect::Sqlite => "",
    }
}

/// A time zone as a quoted string literal; anything but a plain zone name or offset (the server
/// answers with these) falls back to the default zone rather than being written into SQL.
fn zone_literal(dialect: Dialect, zone: &str) -> String {
    let plain = !zone.is_empty()
        && zone.len() <= 64
        && zone.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | '+' | '-' | ':' | '/' | ' ' | '.' | '<' | '>')
        });
    format!(
        "'{}'",
        if plain {
            zone
        } else {
            default_time_zone(dialect)
        }
    )
}

/// The statements that make a script's timestamp literals mean what they meant when they were
/// read: the session time zone is set to `zone` (the zone of the exporting session) and put back
/// by `time_zone_footer`. MySQL `TIMESTAMP` literals carry no offset, so they load under whatever
/// zone the importing session has; PostgreSQL's `timestamp` has none either. `None` = the
/// dialect's default zone (see `default_time_zone`); SQLite has no session time zone.
pub fn time_zone_header(dialect: Dialect, zone: Option<&str>) -> Vec<String> {
    match dialect {
        Dialect::Mysql => vec![
            "SET @OLD_TIME_ZONE=@@TIME_ZONE;".into(),
            format!(
                "SET TIME_ZONE={};",
                zone_literal(dialect, zone.unwrap_or(default_time_zone(dialect)))
            ),
        ],
        Dialect::Postgres => vec![format!(
            "SET TIME ZONE {};",
            zone_literal(dialect, zone.unwrap_or(default_time_zone(dialect)))
        )],
        Dialect::Sqlite => Vec::new(),
    }
}

/// Puts the session time zone back (see `time_zone_header`).
pub fn time_zone_footer(dialect: Dialect) -> Vec<String> {
    match dialect {
        Dialect::Mysql => vec!["SET TIME_ZONE=@OLD_TIME_ZONE;".into()],
        Dialect::Postgres => vec!["RESET TIME ZONE;".into()],
        Dialect::Sqlite => Vec::new(),
    }
}

/// MySQL checks no foreign keys while a multi-table script loads, as mysqldump does (the previous
/// setting is restored by `foreign_key_footer`); the other dialects need nothing.
pub fn foreign_key_header(dialect: Dialect) -> Vec<String> {
    match dialect {
        Dialect::Mysql => vec![
            "SET @OLD_FOREIGN_KEY_CHECKS = @@FOREIGN_KEY_CHECKS, FOREIGN_KEY_CHECKS = 0;".into(),
        ],
        Dialect::Postgres | Dialect::Sqlite => Vec::new(),
    }
}

pub fn foreign_key_footer(dialect: Dialect) -> Vec<String> {
    match dialect {
        Dialect::Mysql => vec!["SET FOREIGN_KEY_CHECKS = @OLD_FOREIGN_KEY_CHECKS;".into()],
        Dialect::Postgres | Dialect::Sqlite => Vec::new(),
    }
}

/// Statements that open a script with rows: the session time zone is set to `zone` (see
/// `time_zone_header`), then MySQL's foreign key checks go off (see `foreign_key_header`).
pub fn script_header(dialect: Dialect, zone: Option<&str>) -> Vec<String> {
    let mut lines = time_zone_header(dialect, zone);
    lines.extend(foreign_key_header(dialect));
    lines
}

/// Undoes `script_header`, in reverse order.
pub fn script_footer(dialect: Dialect) -> Vec<String> {
    let mut lines = foreign_key_footer(dialect);
    lines.extend(time_zone_footer(dialect));
    lines
}

// ---------------------------------------------------------------------------------------------
// Export reads
// ---------------------------------------------------------------------------------------------

/// The SELECT of a table export (`sql::export_select`).
#[derive(Debug, Clone, PartialEq)]
pub struct ExportSelect {
    pub sql: String,
    pub params: Vec<Value>,
    /// For each column after the selected ones: the selected column whose large blob it carries
    /// as hex text (NULL when the blob arrives whole). MySQL and SQLite, whose drivers cap blobs.
    pub blob_hex: Vec<usize>,
}

/// The alias of the `i`-th selected column where an export reads it as text. The file's names
/// come from the metadata; a neutral alias keeps `ORDER BY "c"` on the real column, since
/// PostgreSQL (and the engines that read like it) resolve output names first and would sort
/// numbers and dates as text.
pub(crate) fn export_alias(i: usize) -> String {
    format!("_tabler_c{i}")
}

/// `sql::export_select` of a core dialect: the browse SELECT without paging when none is asked
/// for (`limit` `u32::MAX`, `offset` 0), a page of a table without a key ordered by its `rowid`
/// (a SQLite table of `kind` Table) or by every selected column, PostgreSQL's columns as text, and
/// the hex of MySQL's and SQLite's blobs beyond `BYTES_PREVIEW_CAP`.
pub fn export_select(spec: &SelectSpec<'_>, kind: TableKind) -> Result<ExportSelect, AppError> {
    let dialect = spec.dialect;
    let cols: Vec<&ColumnMeta> = match spec.selected {
        Some(names) => names
            .iter()
            .map(|n| {
                spec.columns
                    .iter()
                    .find(|c| &c.name == n)
                    .ok_or_else(|| AppError::InvalidInput(format!("unknown column \"{n}\"")))
            })
            .collect::<Result<_, _>>()?,
        None => spec.columns.iter().collect(),
    };
    let built = build_select(spec)?;
    let mut sql = built.sql;
    let unbounded = spec.limit == u32::MAX && spec.offset == 0;
    if unbounded {
        // Unbounded: drop the paging clause (the sink enforces the row limit anyway).
        let paging = format!("\nLIMIT {} OFFSET 0", u32::MAX);
        if let Some(stripped) = sql.strip_suffix(&paging) {
            sql = stripped.to_string();
        }
    } else if spec.pk.is_empty() {
        // No key to order by: `rowid` on a SQLite table, else the selected columns (by position,
        // which also orders PostgreSQL's text cells, and JSON columns that have no ordering).
        let terms = if dialect == Dialect::Sqlite && kind == TableKind::Table {
            "rowid ASC".to_string()
        } else {
            (1..=cols.len())
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        if let Some(at) = sql.rfind("\nLIMIT ") {
            let glue = if spec.sort.is_empty() {
                " ORDER BY "
            } else {
                ", "
            };
            sql.insert_str(at, &format!("{glue}{terms}"));
        }
    }

    let mut blob_hex = Vec::new();
    if dialect == Dialect::Postgres {
        let list = cols
            .iter()
            .map(|c| {
                let ident = dialect.quote_ident(&c.name);
                format!("{ident}::text AS {ident}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let prefix = format!("SELECT {list}\nFROM ");
        if let Some(rest) = sql.strip_prefix(&prefix) {
            let aliased = cols
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    format!(
                        "{}::text AS {}",
                        dialect.quote_ident(&c.name),
                        dialect.quote_ident(&export_alias(i))
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            sql = format!("SELECT {aliased}\nFROM {rest}");
        }
    } else {
        // MySQL types are strict: only binary columns hold blobs. In SQLite any column can
        // (whatever its declared type, or none); the `typeof` guard keeps the extra NULL
        // for everything else.
        let blobs: Vec<usize> = cols
            .iter()
            .enumerate()
            .filter(|(_, c)| dialect == Dialect::Sqlite || c.kind == ValueKind::Bytes)
            .map(|(i, _)| i)
            .collect();
        let list = cols
            .iter()
            .map(|c| dialect.quote_ident(&c.name))
            .collect::<Vec<_>>()
            .join(", ");
        let prefix = format!("SELECT {list}\nFROM ");
        if !blobs.is_empty()
            && let Some(rest) = sql.strip_prefix(&prefix)
        {
            let extras: Vec<String> = blobs
                .iter()
                .enumerate()
                .map(|(j, &i)| large_blob_hex(dialect, &cols[i].name, j))
                .collect();
            sql = format!("SELECT {list}, {}\nFROM {rest}", extras.join(", "));
            blob_hex = blobs;
        }
    }
    Ok(ExportSelect {
        sql,
        params: built.params,
        blob_hex,
    })
}

/// Extra select-list item: the hex text of a blob the driver would deliver truncated.
fn large_blob_hex(dialect: Dialect, column: &str, n: usize) -> String {
    let c = dialect.quote_ident(column);
    let alias = dialect.quote_ident(&format!("_tabler_hex_{n}"));
    let cap = BYTES_PREVIEW_CAP;
    match dialect {
        Dialect::Mysql => format!("CASE WHEN LENGTH({c}) > {cap} THEN HEX({c}) END AS {alias}"),
        _ => format!(
            "CASE WHEN typeof({c}) = 'blob' AND length({c}) > {cap} THEN hex({c}) END AS {alias}"
        ),
    }
}

// ---------------------------------------------------------------------------------------------
// CSV import
// ---------------------------------------------------------------------------------------------

/// Bind parameters per statement, below every driver's limit (PostgreSQL and MySQL: 65,535;
/// SQLite: 32,766).
pub fn max_params(dialect: Dialect) -> usize {
    match dialect {
        Dialect::Postgres | Dialect::Mysql => 60_000,
        Dialect::Sqlite => 32_000,
    }
}

/// The column type `ty` in `dialect` for a table the import creates.
pub fn import_type_sql(dialect: Dialect, col: &ImportColumnDef) -> String {
    match (col.ty, dialect) {
        (ImportType::Integer, Dialect::Postgres) => "integer".into(),
        (ImportType::Integer, Dialect::Mysql) => "INT".into(),
        (ImportType::Integer, Dialect::Sqlite) => "INTEGER".into(),
        (ImportType::Bigint, Dialect::Postgres) => "bigint".into(),
        (ImportType::Bigint, Dialect::Mysql) => "BIGINT".into(),
        (ImportType::Bigint, Dialect::Sqlite) => "INTEGER".into(),
        (ImportType::Numeric, Dialect::Postgres) => "numeric".into(),
        (ImportType::Numeric, Dialect::Mysql) => {
            // DECIMAL without a size is DECIMAL(10, 0), which would drop every fraction. The
            // sizes seen come from the first rows only: leave plenty of room (at least
            // DECIMAL(38, 10)). A later value that still does not fit fails its row, as MySQL's
            // rounding note is an error for the import (`mysql_warning_guard`).
            let seen_scale = col.scale.unwrap_or(0).min(30);
            let int_digits = col.precision.unwrap_or(0).saturating_sub(seen_scale);
            let scale = seen_scale.max(10);
            let precision = (int_digits + 10 + scale).clamp(38, 65);
            format!("DECIMAL({precision}, {scale})")
        }
        (ImportType::Numeric, Dialect::Sqlite) => "NUMERIC".into(),
        (ImportType::Boolean, Dialect::Postgres) => "boolean".into(),
        (ImportType::Boolean, _) => "BOOLEAN".into(),
        (ImportType::Date, Dialect::Postgres) => "date".into(),
        (ImportType::Date, _) => "DATE".into(),
        (ImportType::Timestamp, Dialect::Postgres) => "timestamp".into(),
        (ImportType::TimestampTz, Dialect::Postgres) => "timestamptz".into(),
        // Fractions of a second: microseconds, as later rows may have more digits than the
        // first ones (MySQL rounds them without a warning).
        (ImportType::Timestamp | ImportType::TimestampTz, Dialect::Mysql) => {
            if col.scale.is_some_and(|s| s > 0) {
                "DATETIME(6)".into()
            } else {
                "DATETIME".into()
            }
        }
        (ImportType::Timestamp | ImportType::TimestampTz, Dialect::Sqlite) => "TIMESTAMP".into(),
        (ImportType::Text, Dialect::Postgres) => "text".into(),
        // TEXT holds 64 KB: values longer than the sample showed must still fit.
        (ImportType::Text, Dialect::Mysql) => "LONGTEXT".into(),
        (ImportType::Text, Dialect::Sqlite) => "TEXT".into(),
    }
}

/// `CREATE TABLE` for "Create new table": the columns in order, nullable, no key.
pub fn create_import_table(
    dialect: Dialect,
    table: &TableRef,
    columns: &[ImportColumnDef],
) -> Result<String, AppError> {
    if table.name.trim().is_empty() {
        return Err(AppError::InvalidInput("the new table needs a name".into()));
    }
    if columns.is_empty() {
        return Err(AppError::InvalidInput(
            "the new table needs at least one column".into(),
        ));
    }
    let mut seen: Vec<String> = Vec::new();
    let mut defs = Vec::with_capacity(columns.len());
    for c in columns {
        let name = c.name.trim();
        if name.is_empty() {
            return Err(AppError::InvalidInput(
                "every column of the new table needs a name".into(),
            ));
        }
        // MySQL and SQLite compare column names case-insensitively.
        let key = if dialect == Dialect::Postgres {
            name.to_string()
        } else {
            name.to_lowercase()
        };
        if seen.contains(&key) {
            return Err(AppError::InvalidInput(format!(
                "the new table has two columns named \"{name}\""
            )));
        }
        seen.push(key);
        defs.push(format!(
            "  {} {}",
            dialect.quote_ident(name),
            import_type_sql(dialect, c)
        ));
    }
    Ok(format!(
        "CREATE TABLE {} (\n{}\n)",
        dialect.qualify(table.schema.as_deref(), &table.name),
        defs.join(",\n")
    ))
}

/// The table an import writes into and the columns it fills.
pub struct InsertTarget<'a> {
    pub dialect: Dialect,
    pub table: &'a TableRef,
    /// Target columns in the order of each row's values, with their PostgreSQL type (the cast
    /// of each text parameter; ignored elsewhere).
    pub columns: &'a [(String, String)],
    pub mode: InsertMode,
    /// Primary key columns: the conflict target of PostgreSQL / SQLite UPDATE ON DUPLICATE.
    pub primary_key: &'a [String],
}

impl InsertTarget<'_> {
    /// Rows per statement: as many as the parameter limit allows, at most 1,000.
    pub fn rows_per_statement(&self) -> usize {
        self.rows_per_statement_within(&Narrowing::of(self.dialect))
    }

    /// `rows_per_statement` within the values the server binds (`Narrowing::max_params`).
    pub fn rows_per_statement_within(&self, narrowing: &Narrowing) -> usize {
        (narrowing.max_params / self.columns.len().max(1)).clamp(1, 1000)
    }

    /// `build` within what the server narrows: blobs written in, the values checked.
    pub fn build_within(
        &self,
        rows: Vec<Vec<Value>>,
        narrowing: &Narrowing,
    ) -> Result<BuiltSql, AppError> {
        let built = self.build(rows)?;
        if *narrowing == Narrowing::of(self.dialect) {
            return Ok(built);
        }
        narrowing.finish(built, "an INSERT of the import")
    }

    /// Checks the insert option against the dialect and the key (before anything is read).
    pub fn validate(&self) -> Result<(), AppError> {
        if self.columns.is_empty() {
            return Err(AppError::InvalidInput(
                "no CSV column is imported into a table column".into(),
            ));
        }
        match (self.mode, self.dialect) {
            (InsertMode::Replace, Dialect::Postgres) => Err(AppError::Unsupported(
                "REPLACE is not available on PostgreSQL; use UPDATE ON DUPLICATE".into(),
            )),
            (InsertMode::UpdateOnDuplicate, Dialect::Postgres | Dialect::Sqlite) => {
                if self.primary_key.is_empty() {
                    return Err(AppError::InvalidInput(
                        "UPDATE ON DUPLICATE needs a table with a primary key".into(),
                    ));
                }
                let missing: Vec<&str> = self
                    .primary_key
                    .iter()
                    .filter(|k| !self.columns.iter().any(|(c, _)| c == *k))
                    .map(String::as_str)
                    .collect();
                if missing.is_empty() {
                    Ok(())
                } else {
                    Err(AppError::InvalidInput(format!(
                        "UPDATE ON DUPLICATE matches rows by the primary key: import a CSV column \
                         into {}",
                        missing.join(", ")
                    )))
                }
            }
            _ => Ok(()),
        }
    }

    /// One multi-row INSERT for `rows` (each with one value per target column).
    pub fn build(&self, rows: Vec<Vec<Value>>) -> Result<BuiltSql, AppError> {
        let d = self.dialect;
        let width = self.columns.len();
        if rows.is_empty() {
            return Err(AppError::internal("an INSERT batch needs rows"));
        }
        // PostgreSQL refuses to update one row twice in a statement ("ON CONFLICT DO UPDATE
        // command cannot affect row a second time"): of rows with the same key, the last wins,
        // as it would row by row (MySQL and SQLite apply them in order).
        let rows = if d == Dialect::Postgres && self.mode == InsertMode::UpdateOnDuplicate {
            self.last_per_key(rows)
        } else {
            rows
        };
        // Base types (`pg_cast_type`): a value too long for its column fails, it is not cut.
        let casts: Vec<String> = self
            .columns
            .iter()
            .map(|(_, ty)| match d {
                Dialect::Postgres if !ty.is_empty() => format!("::{}", pg_cast_type(ty)),
                _ => String::new(),
            })
            .collect();
        let mut params = Vec::with_capacity(rows.len() * width);
        let mut tuples = Vec::with_capacity(rows.len());
        for row in rows {
            if row.len() != width {
                return Err(AppError::internal(format!(
                    "an import row has {} values for {width} columns",
                    row.len()
                )));
            }
            let mut slots = Vec::with_capacity(width);
            for (value, cast) in row.into_iter().zip(&casts) {
                params.push(value);
                slots.push(format!("{}{cast}", d.placeholder(params.len())));
            }
            tuples.push(format!("({})", slots.join(", ")));
        }
        let names: Vec<String> = self.columns.iter().map(|(c, _)| d.quote_ident(c)).collect();
        let target = d.qualify(self.table.schema.as_deref(), &self.table.name);
        let verb = match (self.mode, d) {
            (InsertMode::Replace, Dialect::Mysql) => "REPLACE INTO",
            (InsertMode::Replace, Dialect::Sqlite) => "INSERT OR REPLACE INTO",
            (InsertMode::Replace, Dialect::Postgres) => {
                return Err(AppError::Unsupported(
                    "REPLACE is not available on PostgreSQL".into(),
                ));
            }
            _ => "INSERT INTO",
        };
        let mut sql = format!(
            "{verb} {target} ({}) VALUES {}",
            names.join(", "),
            tuples.join(", ")
        );
        if self.mode == InsertMode::UpdateOnDuplicate {
            sql.push_str(&self.on_duplicate());
        }
        Ok(BuiltSql { sql, params })
    }

    /// `rows` with one row per primary key value: the last of each (rows with a NULL key, which
    /// cannot conflict, all stay). Keys compare by their text, as the CSV gave them.
    fn last_per_key(&self, rows: Vec<Vec<Value>>) -> Vec<Vec<Value>> {
        let key_at: Vec<usize> = self
            .primary_key
            .iter()
            .filter_map(|k| self.columns.iter().position(|(c, _)| c == k))
            .collect();
        if key_at.is_empty() {
            return rows;
        }
        let mut out: Vec<Vec<Value>> = Vec::with_capacity(rows.len());
        let mut seen: std::collections::HashMap<Vec<String>, usize> =
            std::collections::HashMap::with_capacity(rows.len());
        for row in rows {
            let key: Option<Vec<String>> = key_at
                .iter()
                .map(|&i| row.get(i).and_then(Value::as_text))
                .collect();
            match key {
                Some(key) => match seen.get(&key) {
                    Some(&at) => out[at] = row,
                    None => {
                        seen.insert(key, out.len());
                        out.push(row);
                    }
                },
                None => out.push(row),
            }
        }
        out
    }

    fn on_duplicate(&self) -> String {
        let d = self.dialect;
        let is_key = |c: &str| self.primary_key.iter().any(|k| k == c);
        let updated: Vec<&str> = self
            .columns
            .iter()
            .map(|(c, _)| c.as_str())
            .filter(|c| !is_key(c))
            .collect();
        match d {
            Dialect::Mysql => {
                // `VALUES(c)` works on MySQL and MariaDB alike (MySQL's row alias does not).
                let sets: Vec<String> = if updated.is_empty() {
                    let c = d.quote_ident(&self.columns[0].0);
                    vec![format!("{c} = {c}")]
                } else {
                    updated
                        .iter()
                        .map(|c| {
                            let c = d.quote_ident(c);
                            format!("{c} = VALUES({c})")
                        })
                        .collect()
                };
                format!(" ON DUPLICATE KEY UPDATE {}", sets.join(", "))
            }
            Dialect::Postgres | Dialect::Sqlite => {
                let keys: Vec<String> = self.primary_key.iter().map(|k| d.quote_ident(k)).collect();
                let excluded = if d == Dialect::Postgres {
                    "EXCLUDED"
                } else {
                    "excluded"
                };
                if updated.is_empty() {
                    format!(" ON CONFLICT ({}) DO NOTHING", keys.join(", "))
                } else {
                    let sets: Vec<String> = updated
                        .iter()
                        .map(|c| {
                            let c = d.quote_ident(c);
                            format!("{c} = {excluded}.{c}")
                        })
                        .collect();
                    format!(
                        " ON CONFLICT ({}) DO UPDATE SET {}",
                        keys.join(", "),
                        sets.join(", ")
                    )
                }
            }
        }
    }
}

/// PostgreSQL, after an import wrote its own values into a serial or identity `column`: moves the
/// column's sequence to the largest value in the table when the sequence would hand that value
/// out again, so the next inserted row does not collide. Never moves a sequence back, leaves
/// descending sequences and columns without an owned sequence alone (no row then).
pub fn pg_sync_sequence(table: &TableRef, column: &str) -> BuiltSql {
    let d = Dialect::Postgres;
    let target = d.qualify(table.schema.as_deref(), &table.name);
    let sql = format!(
        "SELECT setval(s.seq, m.top) \
         FROM (SELECT pg_get_serial_sequence($1, $2)::regclass AS seq) s \
         JOIN pg_catalog.pg_sequence q ON q.seqrelid = s.seq \
         CROSS JOIN (SELECT max({})::bigint AS top FROM {target}) m \
         WHERE q.seqincrement > 0 AND m.top IS NOT NULL \
           AND m.top >= COALESCE(pg_catalog.pg_sequence_last_value(s.seq) + q.seqincrement, \
                                 q.seqstart)",
        d.quote_ident(column)
    );
    BuiltSql {
        sql,
        params: vec![Value::Text(target), Value::Text(column.to_string())],
    }
}

/// Marks the text of a MySQL warning in the error `mysql_warning_guard` raises.
pub const MYSQL_WARNING_MARK: &str = "tabler-import-warning: ";

/// MySQL warnings an import ignores: deprecated syntax (`VALUES()` in ON DUPLICATE KEY UPDATE).
const MYSQL_BENIGN_WARNINGS: &str = "1287";

/// Stands in for the commas of a warning's text inside the `sql_mode` value (MySQL would quote
/// only the part before the first comma).
const MYSQL_COMMA: char = '\u{FF0C}';

/// MySQL stores a value it has to change with a note or a warning only, even in strict mode: a
/// DECIMAL rounded to its scale (1265), a date whose time is dropped (1292), and outside strict
/// mode values cut or zeroed. Statements run around each import INSERT turn such a condition into
/// an error naming it: the statement before clears the variables; the ones after read the
/// INSERT's first and last conditions (the first may be the deprecation warning of `VALUES()`, a
/// value change comes after it) and fail when one of them is not a deprecation, setting
/// `sql_mode` to a value made of the condition's text (an unknown mode is an error quoting up to
/// 200 characters of it, commas replaced; otherwise the mode is set to itself).
/// `mysql_warning_text` reads the condition back.
pub fn mysql_warning_guard() -> (String, Vec<String>) {
    let before = "SET @tabler_n = NULL, @tabler_w1 = NULL, @tabler_m1 = NULL, @tabler_w2 = NULL, \
                  @tabler_m2 = NULL"
        .to_string();
    let bad = |w: &str| format!("COALESCE({w} NOT IN ({MYSQL_BENIGN_WARNINGS}), FALSE)");
    let text =
        |m: &str| format!("CONCAT('{MYSQL_WARNING_MARK}', REPLACE({m}, ',', '{MYSQL_COMMA}'))");
    let after = vec![
        "GET DIAGNOSTICS @tabler_n = NUMBER".to_string(),
        "GET DIAGNOSTICS CONDITION 1 @tabler_w1 = MYSQL_ERRNO, @tabler_m1 = MESSAGE_TEXT".into(),
        "GET DIAGNOSTICS CONDITION @tabler_n @tabler_w2 = MYSQL_ERRNO, @tabler_m2 = MESSAGE_TEXT"
            .into(),
        format!(
            "SET @@SESSION.sql_mode = CASE WHEN {} THEN {} WHEN {} THEN {} \
             ELSE @@SESSION.sql_mode END",
            bad("@tabler_w2"),
            text("@tabler_m2"),
            bad("@tabler_w1"),
            text("@tabler_m1")
        ),
    ];
    (before, after)
}

/// The MySQL warning in an error `mysql_warning_guard` raised, with the row of the INSERT it
/// names (`… at row 3`, 1-based).
pub fn mysql_warning_text(error: &str) -> Option<(String, Option<u64>)> {
    let at = error.find(MYSQL_WARNING_MARK)?;
    let text = error[at + MYSQL_WARNING_MARK.len()..].trim_end();
    let text = text
        .strip_suffix('\'')
        .unwrap_or(text)
        .replace(MYSQL_COMMA, ",");
    let row = text
        .rsplit_once(" at row ")
        .and_then(|(_, n)| n.trim().parse::<u64>().ok())
        .filter(|n| *n > 0);
    Some((text, row))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::schema::IndexMeta;

    fn def(name: &str, ty: ImportType) -> ImportColumnDef {
        ImportColumnDef {
            name: name.into(),
            ty,
            precision: None,
            scale: None,
        }
    }

    fn cols(spec: &[(&str, &str)]) -> Vec<(String, String)> {
        spec.iter()
            .map(|(c, t)| (c.to_string(), t.to_string()))
            .collect()
    }

    fn rows() -> Vec<Vec<Value>> {
        vec![
            vec![Value::Text("1".into()), Value::Text("a".into())],
            vec![Value::Text("2".into()), Value::Null],
        ]
    }

    /// A server that binds fewer values (D1: 100) gets fewer rows per INSERT, and its blobs
    /// written in; the dialect's own limit gives the same statements as `build`.
    #[test]
    fn an_import_stays_within_what_the_server_binds() {
        use crate::types::caps::SessionCaps;
        use crate::types::profile::Driver;
        let t = TableRef::new(None, "files");
        let columns = cols(&[("id", ""), ("name", ""), ("data", "")]);
        let target = InsertTarget {
            dialect: Dialect::Sqlite,
            table: &t,
            columns: &columns,
            mode: InsertMode::StopOnError,
            primary_key: &[],
        };
        let d1 = Narrowing::for_caps(&SessionCaps::for_driver(Driver::D1));
        assert_eq!(target.rows_per_statement(), 1000);
        assert_eq!(target.rows_per_statement_within(&d1), 33);
        let rows = || {
            vec![vec![
                Value::Int(1),
                Value::Text("a".into()),
                Value::from_bytes(&[0xab]),
            ]]
        };
        let built = target.build_within(rows(), &d1).unwrap();
        assert_eq!(
            built.sql,
            "INSERT INTO \"files\" (\"id\", \"name\", \"data\") VALUES (?1, ?2, X'ab')"
        );
        assert_eq!(built.params, [Value::Int(1), Value::Text("a".into())]);
        let own = Narrowing::of(Dialect::Sqlite);
        assert_eq!(
            target.build_within(rows(), &own).unwrap(),
            target.build(rows()).unwrap()
        );
    }

    #[test]
    fn plain_inserts_bind_every_value() {
        let t = TableRef::new(Some("public"), "users");
        let columns = cols(&[("id", "integer"), ("name", "character varying(20)")]);
        let target = |dialect| InsertTarget {
            dialect,
            table: &t,
            columns: &columns,
            mode: InsertMode::StopOnError,
            primary_key: &[],
        };
        let pg = target(Dialect::Postgres).build(rows()).unwrap();
        assert_eq!(
            pg.sql,
            "INSERT INTO \"public\".\"users\" (\"id\", \"name\") VALUES \
             ($1::integer, $2::character varying), ($3::integer, $4::character varying)"
        );
        assert_eq!(pg.params.len(), 4);
        assert_eq!(pg.params[3], Value::Null);
        let my = target(Dialect::Mysql).build(rows()).unwrap();
        assert_eq!(
            my.sql,
            "INSERT INTO `public`.`users` (`id`, `name`) VALUES (?, ?), (?, ?)"
        );
        let t2 = TableRef::new(None, "users");
        let lite = InsertTarget {
            dialect: Dialect::Sqlite,
            table: &t2,
            columns: &columns,
            mode: InsertMode::StopOnError,
            primary_key: &[],
        }
        .build(rows())
        .unwrap();
        assert_eq!(
            lite.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES (?1, ?2), (?3, ?4)"
        );
        assert!(target(Dialect::Postgres).build(vec![]).is_err());
        assert!(
            target(Dialect::Postgres)
                .build(vec![vec![Value::Null]])
                .is_err()
        );
    }

    #[test]
    fn insert_options_per_dialect() {
        let t = TableRef::new(None, "t");
        let columns = cols(&[("id", "integer"), ("name", "text")]);
        let pk = vec!["id".to_string()];
        let target = |dialect, mode| InsertTarget {
            dialect,
            table: &t,
            columns: &columns,
            mode,
            primary_key: &pk,
        };
        use InsertMode::*;
        assert_eq!(
            target(Dialect::Mysql, Replace).build(rows()).unwrap().sql,
            "REPLACE INTO `t` (`id`, `name`) VALUES (?, ?), (?, ?)"
        );
        assert_eq!(
            target(Dialect::Sqlite, Replace).build(rows()).unwrap().sql,
            "INSERT OR REPLACE INTO \"t\" (\"id\", \"name\") VALUES (?1, ?2), (?3, ?4)"
        );
        assert!(target(Dialect::Postgres, Replace).validate().is_err());
        assert!(target(Dialect::Postgres, Replace).build(rows()).is_err());
        assert_eq!(
            target(Dialect::Mysql, UpdateOnDuplicate)
                .build(rows())
                .unwrap()
                .sql,
            "INSERT INTO `t` (`id`, `name`) VALUES (?, ?), (?, ?) \
             ON DUPLICATE KEY UPDATE `name` = VALUES(`name`)"
        );
        assert_eq!(
            target(Dialect::Postgres, UpdateOnDuplicate)
                .build(rows())
                .unwrap()
                .sql,
            "INSERT INTO \"t\" (\"id\", \"name\") VALUES ($1::integer, $2::text), ($3::integer, $4::text) \
             ON CONFLICT (\"id\") DO UPDATE SET \"name\" = EXCLUDED.\"name\""
        );
        assert_eq!(
            target(Dialect::Sqlite, UpdateOnDuplicate)
                .build(rows())
                .unwrap()
                .sql,
            "INSERT INTO \"t\" (\"id\", \"name\") VALUES (?1, ?2), (?3, ?4) \
             ON CONFLICT (\"id\") DO UPDATE SET \"name\" = excluded.\"name\""
        );
        // Only key columns imported: nothing to update.
        let only_key = cols(&[("id", "integer")]);
        let keys_only = |dialect| InsertTarget {
            dialect,
            table: &t,
            columns: &only_key,
            mode: UpdateOnDuplicate,
            primary_key: &pk,
        };
        let one = || vec![vec![Value::Text("1".into())]];
        assert!(
            keys_only(Dialect::Postgres)
                .build(one())
                .unwrap()
                .sql
                .ends_with("ON CONFLICT (\"id\") DO NOTHING")
        );
        assert!(
            keys_only(Dialect::Mysql)
                .build(one())
                .unwrap()
                .sql
                .ends_with("ON DUPLICATE KEY UPDATE `id` = `id`")
        );
        // The conflict target must be imported, and exist.
        let no_key = cols(&[("name", "text")]);
        let missing = InsertTarget {
            dialect: Dialect::Sqlite,
            table: &t,
            columns: &no_key,
            mode: UpdateOnDuplicate,
            primary_key: &pk,
        };
        assert!(missing.validate().unwrap_err().to_string().contains("id"));
        let keyless = InsertTarget {
            dialect: Dialect::Postgres,
            table: &t,
            columns: &columns,
            mode: UpdateOnDuplicate,
            primary_key: &[],
        };
        assert!(keyless.validate().is_err());
        // MySQL matches by any unique key: no primary key needed.
        let my_keyless = InsertTarget {
            dialect: Dialect::Mysql,
            table: &t,
            columns: &columns,
            mode: UpdateOnDuplicate,
            primary_key: &[],
        };
        assert!(my_keyless.validate().is_ok());
        assert!(
            my_keyless
                .build(rows())
                .unwrap()
                .sql
                .ends_with("ON DUPLICATE KEY UPDATE `id` = VALUES(`id`), `name` = VALUES(`name`)")
        );
    }

    #[test]
    fn sizes_new_mysql_columns_generously() {
        let numeric = |precision, scale| ImportColumnDef {
            name: "n".into(),
            ty: ImportType::Numeric,
            precision,
            scale,
        };
        assert_eq!(
            import_type_sql(Dialect::Mysql, &numeric(Some(30), Some(12))),
            "DECIMAL(40, 12)"
        );
        assert_eq!(
            import_type_sql(Dialect::Mysql, &numeric(Some(80), Some(40))),
            "DECIMAL(65, 30)"
        );
        assert_eq!(
            import_type_sql(Dialect::Postgres, &numeric(Some(7), Some(2))),
            "numeric"
        );
        let tz = ImportColumnDef {
            name: "at".into(),
            ty: ImportType::TimestampTz,
            precision: None,
            scale: None,
        };
        assert_eq!(import_type_sql(Dialect::Postgres, &tz), "timestamptz");
        assert_eq!(import_type_sql(Dialect::Mysql, &tz), "DATETIME");
        assert_eq!(import_type_sql(Dialect::Sqlite, &tz), "TIMESTAMP");
    }

    #[test]
    fn postgres_upserts_keep_the_last_row_per_key() {
        let t = TableRef::new(None, "t");
        let columns = cols(&[("id", "integer"), ("name", "character(10)")]);
        let pk = vec!["id".to_string()];
        let target = |dialect| InsertTarget {
            dialect,
            table: &t,
            columns: &columns,
            mode: InsertMode::UpdateOnDuplicate,
            primary_key: &pk,
        };
        let row = |id: &str, name: &str| vec![Value::Text(id.into()), Value::Text(name.into())];
        let rows = || {
            vec![
                row("1", "a"),
                row("2", "b"),
                row("1", "c"),
                vec![Value::Null, Value::Text("n".into())],
                vec![Value::Null, Value::Text("m".into())],
            ]
        };
        let pg = target(Dialect::Postgres).build(rows()).unwrap();
        assert_eq!(
            pg.sql,
            "INSERT INTO \"t\" (\"id\", \"name\") VALUES ($1::integer, $2::bpchar), \
             ($3::integer, $4::bpchar), ($5::integer, $6::bpchar), ($7::integer, $8::bpchar) \
             ON CONFLICT (\"id\") DO UPDATE SET \"name\" = EXCLUDED.\"name\""
        );
        assert_eq!(
            pg.params,
            [row("1", "c"), row("2", "b")]
                .concat()
                .into_iter()
                .chain([
                    Value::Null,
                    Value::Text("n".into()),
                    Value::Null,
                    Value::Text("m".into())
                ])
                .collect::<Vec<_>>()
        );
        // MySQL and SQLite apply repeated keys in order themselves.
        assert_eq!(
            target(Dialect::Sqlite).build(rows()).unwrap().params.len(),
            10
        );
        // Plain inserts keep every row (the key violation is the user's to see).
        let plain = InsertTarget {
            mode: InsertMode::StopOnError,
            ..target(Dialect::Postgres)
        };
        assert_eq!(plain.build(rows()).unwrap().params.len(), 10);
    }

    #[test]
    fn sequence_sync_and_mysql_warning_guard() {
        let sync = pg_sync_sequence(&TableRef::new(Some("app"), "Users"), "id");
        assert!(sync.sql.starts_with("SELECT setval(s.seq, m.top) FROM (SELECT pg_get_serial_sequence($1, $2)::regclass AS seq) s"), "{}", sync.sql);
        assert!(
            sync.sql
                .contains("max(\"id\")::bigint AS top FROM \"app\".\"Users\"")
        );
        assert!(sync.sql.contains("q.seqincrement > 0"));
        assert_eq!(
            sync.params,
            vec![
                Value::Text("\"app\".\"Users\"".into()),
                Value::Text("id".into())
            ]
        );
        let (before, after) = mysql_warning_guard();
        assert!(before.starts_with("SET @tabler_n = NULL"));
        assert_eq!(after.len(), 4);
        assert!(after[3].contains("NOT IN (1287)"));
        assert_eq!(
            mysql_warning_text(
                "Variable 'sql_mode' can't be set to the value of 'tabler-import-warning: Data truncated for column 'd' at row 2'"
            ),
            Some(("Data truncated for column 'd' at row 2".into(), Some(2)))
        );
        assert_eq!(
            mysql_warning_text(
                "Variable 'sql_mode' can't be set to the value of 'tabler-import-warning: x\u{FF0C} y'"
            ),
            Some(("x, y".into(), None))
        );
        assert_eq!(mysql_warning_text("Duplicate entry '1'"), None);
    }

    #[test]
    fn batches_respect_the_parameter_limit() {
        let t = TableRef::new(None, "t");
        let wide: Vec<(String, String)> =
            (0..100).map(|i| (format!("c{i}"), String::new())).collect();
        let target = |dialect| InsertTarget {
            dialect,
            table: &t,
            columns: &wide,
            mode: InsertMode::StopOnError,
            primary_key: &[],
        };
        assert_eq!(target(Dialect::Sqlite).rows_per_statement(), 320);
        assert_eq!(target(Dialect::Postgres).rows_per_statement(), 600);
        let narrow = cols(&[("a", "")]);
        let one = InsertTarget {
            dialect: Dialect::Mysql,
            table: &t,
            columns: &narrow,
            mode: InsertMode::StopOnError,
            primary_key: &[],
        };
        assert_eq!(one.rows_per_statement(), 1000);
        let empty: Vec<(String, String)> = Vec::new();
        assert!(
            InsertTarget {
                dialect: Dialect::Mysql,
                table: &t,
                columns: &empty,
                mode: InsertMode::StopOnError,
                primary_key: &[],
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn creates_tables_with_mapped_types() {
        let t = TableRef::new(Some("app"), "people");
        let columns = vec![
            def("id", ImportType::Integer),
            def("big", ImportType::Bigint),
            ImportColumnDef {
                name: "price".into(),
                ty: ImportType::Numeric,
                precision: Some(7),
                scale: Some(2),
            },
            def("ok", ImportType::Boolean),
            def("born", ImportType::Date),
            ImportColumnDef {
                name: "seen".into(),
                ty: ImportType::Timestamp,
                precision: None,
                scale: Some(3),
            },
            def("note", ImportType::Text),
        ];
        assert_eq!(
            create_import_table(Dialect::Postgres, &t, &columns).unwrap(),
            "CREATE TABLE \"app\".\"people\" (\n  \"id\" integer,\n  \"big\" bigint,\n  \
             \"price\" numeric,\n  \"ok\" boolean,\n  \"born\" date,\n  \"seen\" timestamp,\n  \
             \"note\" text\n)"
        );
        assert_eq!(
            create_import_table(Dialect::Mysql, &t, &columns).unwrap(),
            "CREATE TABLE `app`.`people` (\n  `id` INT,\n  `big` BIGINT,\n  \
             `price` DECIMAL(38, 10),\n  `ok` BOOLEAN,\n  `born` DATE,\n  `seen` DATETIME(6),\n  \
             `note` LONGTEXT\n)"
        );
        let lite =
            create_import_table(Dialect::Sqlite, &TableRef::new(None, "p"), &columns).unwrap();
        assert!(lite.starts_with("CREATE TABLE \"p\" (\n  \"id\" INTEGER,"));
        assert!(lite.contains("\"seen\" TIMESTAMP"));
        // Unknown sizes: MySQL still keeps fractions.
        assert_eq!(
            import_type_sql(Dialect::Mysql, &def("n", ImportType::Numeric)),
            "DECIMAL(38, 10)"
        );
        assert_eq!(
            import_type_sql(Dialect::Mysql, &def("t", ImportType::Timestamp)),
            "DATETIME"
        );
        // Names are required and unique (case-insensitively where the server compares so).
        assert!(
            create_import_table(Dialect::Postgres, &TableRef::new(None, " "), &columns).is_err()
        );
        assert!(create_import_table(Dialect::Postgres, &t, &[]).is_err());
        let dup = vec![def("A", ImportType::Text), def("a", ImportType::Text)];
        assert!(create_import_table(Dialect::Postgres, &t, &dup).is_ok());
        assert!(create_import_table(Dialect::Mysql, &t, &dup).is_err());
        assert!(create_import_table(Dialect::Sqlite, &t, &[def(" ", ImportType::Text)]).is_err());
    }

    fn meta(dialect_ddl: Option<&str>) -> TableMeta {
        TableMeta {
            table: TableRef::new(Some("main"), "t"),
            kind: TableKind::Table,
            columns: vec![],
            primary_key: vec![],
            indexes: vec![
                IndexMeta {
                    name: "sqlite_autoindex_t_1".into(),
                    columns: vec!["a".into()],
                    unique: true,
                    primary: false,
                    method: None,
                    predicate: None,
                    definition: None,
                    comment: None,
                    include: vec![],
                },
                IndexMeta {
                    name: "t_b".into(),
                    columns: vec!["b".into()],
                    unique: false,
                    primary: false,
                    method: None,
                    predicate: None,
                    definition: Some("CREATE INDEX t_b ON t (b)".into()),
                    comment: None,
                    include: vec![],
                },
            ],
            foreign_keys: vec![],
            triggers: vec![],
            comment: None,
            row_estimate: None,
            ddl: dialect_ddl.map(str::to_string),
            checks: vec![],
            options: None,
        }
    }

    #[test]
    fn export_script_pieces() {
        let t = TableRef::new(Some("public"), "o\"rders");
        assert_eq!(
            drop_table_if_exists(Dialect::Postgres, &t),
            "DROP TABLE IF EXISTS \"public\".\"o\"\"rders\";"
        );
        assert_eq!(
            drop_table_if_exists(Dialect::Mysql, &TableRef::new(Some("app"), "orders")),
            "DROP TABLE IF EXISTS `orders`;"
        );
        assert_eq!(
            table_structure(Dialect::Sqlite, &meta(Some("CREATE TABLE t (a UNIQUE, b)"))),
            "CREATE TABLE t (a UNIQUE, b);\nCREATE INDEX t_b ON t (b);"
        );
        assert_eq!(
            table_structure(
                Dialect::Mysql,
                &meta(Some("CREATE TABLE `t` (\n  `a` int\n);"))
            ),
            "CREATE TABLE `t` (\n  `a` int\n);"
        );
    }

    #[test]
    fn scripts_set_and_restore_the_session_time_zone() {
        // MySQL: foreign key checks off and the zone of the exporting session, both undone.
        let header = script_header(Dialect::Mysql, Some("+07:00")).join("\n");
        assert!(
            header.contains("SET @OLD_TIME_ZONE=@@TIME_ZONE;"),
            "{header}"
        );
        assert!(header.contains("SET TIME_ZONE='+07:00';"), "{header}");
        assert!(header.contains("FOREIGN_KEY_CHECKS = 0"), "{header}");
        let footer = script_footer(Dialect::Mysql).join("\n");
        assert!(footer.contains("SET TIME_ZONE=@OLD_TIME_ZONE;"), "{footer}");
        assert!(footer.contains("SET FOREIGN_KEY_CHECKS = @OLD_FOREIGN_KEY_CHECKS;"));
        // The zone is set before anything loads and restored after everything did.
        assert!(header.find("TIME_ZONE='+07:00'") < header.find("FOREIGN_KEY_CHECKS = 0"));
        assert!(footer.find("FOREIGN_KEY_CHECKS") < footer.find("OLD_TIME_ZONE"));
        // A named zone is written as it is; one that cannot be read is UTC.
        assert!(
            script_header(Dialect::Mysql, Some("Asia/Jakarta"))
                .join("\n")
                .contains("SET TIME_ZONE='Asia/Jakarta';")
        );
        assert!(
            script_header(Dialect::Mysql, None)
                .join("\n")
                .contains("SET TIME_ZONE='+00:00';")
        );
        // PostgreSQL.
        assert_eq!(
            script_header(Dialect::Postgres, Some("Europe/Berlin")),
            vec!["SET TIME ZONE 'Europe/Berlin';".to_string()]
        );
        assert_eq!(
            script_header(Dialect::Postgres, None),
            vec!["SET TIME ZONE 'UTC';".to_string()]
        );
        assert_eq!(
            script_footer(Dialect::Postgres),
            vec!["RESET TIME ZONE;".to_string()]
        );
        // SQLite has no session time zone.
        assert!(script_header(Dialect::Sqlite, Some("+07:00")).is_empty());
        assert!(script_footer(Dialect::Sqlite).is_empty());
        assert!(time_zone_header(Dialect::Sqlite, Some("UTC")).is_empty());
        // The time zone alone (a script of one table's rows): no foreign key statements.
        assert_eq!(
            time_zone_header(Dialect::Mysql, Some("-05:00")),
            vec![
                "SET @OLD_TIME_ZONE=@@TIME_ZONE;".to_string(),
                "SET TIME_ZONE='-05:00';".to_string()
            ]
        );
        assert_eq!(
            time_zone_footer(Dialect::Mysql),
            vec!["SET TIME_ZONE=@OLD_TIME_ZONE;".to_string()]
        );
        // Text from the server never becomes SQL.
        let attack = script_header(Dialect::Postgres, Some("UTC'; DROP TABLE t; --")).join("");
        assert_eq!(attack, "SET TIME ZONE 'UTC';");
    }
}
