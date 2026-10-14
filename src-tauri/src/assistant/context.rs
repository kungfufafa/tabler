//! The database context of an assistant request: the STRUCTURE of the active connection — the
//! engine and version, the current table's DDL and a compact list of the schema's tables and
//! columns. It is read through introspection only (`list_tables`, `completion_metadata`,
//! `table_meta`, `view_definition`, the PG catalog), never through a query of user data, and
//! nothing here holds a row or a cell value. Row counts, sizes and AUTO_INCREMENT counters are
//! left out too.

use std::fmt::Write as _;
use std::sync::LazyLock;

use regex::Regex;

use crate::drivers::DbSession;
use crate::export::structure::pg_table_context;
use crate::sqlgen::Dialect;
use crate::sqlgen::ddl::create_table_sql_with;
use crate::types::assistant::AssistantContextPreview;
use crate::types::profile::Driver;
use crate::types::schema::{TableKind, TableRef};

/// At most this many tables are listed.
pub const MAX_TABLES: usize = 200;
/// The whole context text stays under this many characters.
pub const MAX_CONTEXT_CHARS: usize = 40_000;
/// The current table's DDL is cut after this many characters.
pub const MAX_DDL_CHARS: usize = 12_000;
/// Columns listed per table (the rest are counted).
pub const MAX_COLUMNS_PER_TABLE: usize = 60;

/// What the server is, as the connection reported it.
#[derive(Debug, Clone, Default)]
pub struct ServerFacts {
    pub server_version: String,
    pub current_database: Option<String>,
}

/// A table of the schema: name, kind and column names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableOutline {
    pub name: String,
    pub kind: Option<TableKind>,
    pub columns: Vec<String>,
}

/// The table of the active tab with its `CREATE` text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentTable {
    pub table: TableRef,
    pub kind: TableKind,
    pub ddl: String,
}

/// Everything sent about the database. Holds names, types and DDL text only.
#[derive(Debug, Clone)]
pub struct SchemaContext {
    pub engine: &'static str,
    pub dialect: Dialect,
    pub server_version: String,
    pub database: Option<String>,
    pub schema: Option<String>,
    pub tables: Vec<TableOutline>,
    pub current: Option<CurrentTable>,
    /// What could not be read (the rest is still sent).
    pub warning: Option<String>,
}

fn engine_name(driver: Driver, mariadb: bool) -> &'static str {
    match driver {
        Driver::Postgres => "PostgreSQL",
        Driver::Mysql if mariadb => "MariaDB",
        Driver::Mysql => "MySQL",
        Driver::Sqlite => "SQLite",
    }
}

fn dialect_name(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Postgres => "postgres",
        Dialect::Mysql => "mysql",
        Dialect::Sqlite => "sqlite",
    }
}

fn kind_label(kind: TableKind) -> &'static str {
    match kind {
        TableKind::Table => "table",
        TableKind::View => "view",
        TableKind::MaterializedView => "materialized view",
        TableKind::ForeignTable => "foreign table",
        TableKind::Partitioned => "partitioned table",
        TableKind::SystemView => "system view",
    }
}

/// `AUTO_INCREMENT=<n>` table options of MySQL's `SHOW CREATE TABLE`: the next id says how many
/// rows were ever inserted, so it is dropped.
static AUTO_INCREMENT_OPTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s+AUTO_INCREMENT\s*=\s*\d+").expect("valid regex"));

pub fn strip_counters(ddl: &str) -> String {
    AUTO_INCREMENT_OPTION.replace_all(ddl, "").into_owned()
}

/// The first `max` characters of `s` (on a char boundary) and whether it was cut.
fn cut(s: &str, max: usize) -> (&str, bool) {
    match s.char_indices().nth(max) {
        Some((i, _)) => (&s[..i], true),
        None => (s, false),
    }
}

/// `CREATE` text of a table or view, from the server's own DDL (MySQL, SQLite), the PG catalog
/// (tables) or the view definition. Structure only.
async fn table_ddl(
    session: &dyn DbSession,
    table: &TableRef,
) -> Result<CurrentTable, crate::error::AppError> {
    let meta = session.table_meta(table).await?;
    let dialect = session.dialect();
    let ddl = if let Some(ddl) = meta.ddl.as_deref().filter(|d| !d.trim().is_empty()) {
        ddl.to_string()
    } else {
        match meta.kind {
            TableKind::View | TableKind::MaterializedView | TableKind::SystemView => {
                let definition = session
                    .view_definition(&meta.table)
                    .await?
                    .unwrap_or_default();
                let what = if meta.kind == TableKind::MaterializedView {
                    "MATERIALIZED VIEW"
                } else {
                    "VIEW"
                };
                let body = definition.trim();
                let body = body.strip_suffix(';').unwrap_or(body).trim_end();
                format!(
                    "CREATE {what} {} AS\n{body};",
                    dialect.qualify(meta.table.schema.as_deref(), &meta.table.name)
                )
            }
            _ => {
                let context = if dialect == Dialect::Postgres {
                    pg_table_context(session, &meta.table, false)
                        .await
                        .unwrap_or_default()
                } else {
                    Default::default()
                };
                create_table_sql_with(dialect, &meta, &context)
            }
        }
    };
    Ok(CurrentTable {
        table: meta.table.clone(),
        kind: meta.kind,
        ddl: strip_counters(&ddl),
    })
}

/// Reads the structure of `schema` (or the connection's default) and of `table`. Failures are
/// collected in `warning`: the request still goes, with what could be read.
pub async fn gather(
    session: &dyn DbSession,
    facts: &ServerFacts,
    schema: Option<&str>,
    table: Option<&TableRef>,
) -> SchemaContext {
    let mut warnings: Vec<String> = Vec::new();
    let completion = match session.completion_metadata().await {
        Ok(c) => Some(c),
        Err(e) => {
            warnings.push(format!("table list: {e}"));
            None
        }
    };
    let schema = schema
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| table.and_then(|t| t.schema.clone()))
        .or_else(|| completion.as_ref().and_then(|c| c.default_schema.clone()))
        .or_else(|| (session.driver() == Driver::Sqlite).then(|| "main".to_string()));

    let columns_of = |name: &str| -> Vec<String> {
        completion
            .as_ref()
            .zip(schema.as_deref())
            .and_then(|(c, s)| c.schemas.get(s))
            .and_then(|tables| tables.get(name))
            .cloned()
            .unwrap_or_default()
    };
    let mut tables: Vec<TableOutline> = Vec::new();
    if let Some(s) = schema.as_deref() {
        match session.list_tables(s).await {
            Ok(list) => {
                tables = list
                    .into_iter()
                    .map(|t| TableOutline {
                        columns: columns_of(&t.name),
                        name: t.name,
                        kind: Some(t.kind),
                    })
                    .collect();
            }
            Err(e) => {
                warnings.push(format!("tables of {s}: {e}"));
                if let Some(names) = completion.as_ref().and_then(|c| c.schemas.get(s)) {
                    tables = names
                        .iter()
                        .map(|(name, columns)| TableOutline {
                            name: name.clone(),
                            kind: None,
                            columns: columns.clone(),
                        })
                        .collect();
                }
            }
        }
    }

    let current = match table {
        Some(t) => match table_ddl(session, t).await {
            Ok(c) => Some(c),
            Err(e) => {
                warnings.push(format!("structure of {}: {e}", t.name));
                None
            }
        },
        None => None,
    };

    SchemaContext {
        engine: engine_name(session.driver(), session.is_mariadb()),
        dialect: session.dialect(),
        server_version: facts.server_version.clone(),
        database: facts.current_database.clone(),
        schema,
        tables,
        current,
        warning: (!warnings.is_empty()).then(|| {
            format!(
                "Could not read part of the structure ({})",
                warnings.join("; ")
            )
        }),
    }
}

impl SchemaContext {
    /// The context text appended to the system prompt, under the size caps.
    pub fn render(&self) -> AssistantContextPreview {
        let mut out = String::from("<database_context>\n");
        let version = self.server_version.trim();
        let _ = writeln!(
            out,
            "Engine: {}{} (SQL dialect: {})",
            self.engine,
            if version.is_empty() {
                String::new()
            } else {
                format!(" — server version {version}")
            },
            dialect_name(self.dialect)
        );
        if let Some(db) = self.database.as_deref().filter(|d| !d.is_empty()) {
            let _ = writeln!(out, "Current database: {db}");
        }
        if let Some(s) = self.schema.as_deref() {
            let _ = writeln!(out, "Current schema: {s}");
        }
        let mut truncated = false;
        if let Some(c) = &self.current {
            let (ddl, cut_ddl) = cut(c.ddl.trim(), MAX_DDL_CHARS);
            truncated |= cut_ddl;
            let _ = writeln!(
                out,
                "\nThe user is looking at the {} {}:\n```sql\n{ddl}{}\n```",
                kind_label(c.kind),
                self.dialect
                    .qualify(c.table.schema.as_deref(), &c.table.name),
                if cut_ddl { "\n-- … (cut)" } else { "" }
            );
        }
        let mut listed = 0usize;
        if !self.tables.is_empty() {
            let _ = writeln!(
                out,
                "\nTables and views in {} ({}), with their columns:",
                self.schema.as_deref().unwrap_or("the current schema"),
                self.tables.len()
            );
            // Room left for the list, keeping the closing lines.
            let budget = MAX_CONTEXT_CHARS.saturating_sub(out.len() + 120);
            let mut used = 0usize;
            for t in &self.tables {
                if listed >= MAX_TABLES {
                    break;
                }
                let line = outline_line(t);
                if used + line.len() > budget {
                    break;
                }
                used += line.len();
                out.push_str(&line);
                listed += 1;
            }
            if listed < self.tables.len() {
                truncated = true;
                let _ = writeln!(out, "… {} more not listed", self.tables.len() - listed);
            }
        }
        out.push_str("</database_context>");
        let summary = match (&self.current, self.tables.len()) {
            (Some(c), 0) => c.table.name.clone(),
            (Some(c), n) => format!("{} + {n} tables", c.table.name),
            (None, 0) => format!("{} (no tables)", self.engine),
            (None, n) => format!(
                "{n} tables of {}",
                self.schema.as_deref().unwrap_or("schema")
            ),
        };
        AssistantContextPreview {
            text: out,
            summary,
            tables: listed as u32,
            truncated,
            warning: self.warning.clone(),
        }
    }
}

fn outline_line(t: &TableOutline) -> String {
    let mut line = format!("- {}", t.name);
    if let Some(kind) = t.kind.filter(|k| *k != TableKind::Table) {
        let _ = write!(line, " [{}]", kind_label(kind));
    }
    if !t.columns.is_empty() {
        let shown = t.columns.len().min(MAX_COLUMNS_PER_TABLE);
        let _ = write!(line, ": {}", t.columns[..shown].join(", "));
        if t.columns.len() > shown {
            let _ = write!(line, ", … ({} more)", t.columns.len() - shown);
        }
    }
    line.push('\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outline(name: &str, cols: &[&str]) -> TableOutline {
        TableOutline {
            name: name.into(),
            kind: Some(TableKind::Table),
            columns: cols.iter().map(|c| c.to_string()).collect(),
        }
    }

    fn context(tables: Vec<TableOutline>) -> SchemaContext {
        SchemaContext {
            engine: "PostgreSQL",
            dialect: Dialect::Postgres,
            server_version: "17.6".into(),
            database: Some("shop".into()),
            schema: Some("public".into()),
            tables,
            current: Some(CurrentTable {
                table: TableRef::new(Some("public"), "orders"),
                kind: TableKind::Table,
                ddl: "CREATE TABLE \"public\".\"orders\" (id bigint PRIMARY KEY);".into(),
            }),
            warning: None,
        }
    }

    #[test]
    fn renders_engine_ddl_and_tables() {
        let mut tables = vec![outline("orders", &["id", "total"])];
        tables.push(TableOutline {
            name: "recent".into(),
            kind: Some(TableKind::View),
            columns: vec!["id".into()],
        });
        let preview = context(tables).render();
        assert!(preview.text.starts_with("<database_context>"));
        assert!(
            preview
                .text
                .contains("PostgreSQL — server version 17.6 (SQL dialect: postgres)")
        );
        assert!(preview.text.contains("Current database: shop"));
        assert!(preview.text.contains("the table \"public\".\"orders\""));
        assert!(preview.text.contains("- orders: id, total\n"));
        assert!(preview.text.contains("- recent [view]: id\n"));
        assert_eq!(preview.summary, "orders + 2 tables");
        assert_eq!(preview.tables, 2);
        assert!(!preview.truncated);
    }

    #[test]
    fn caps_the_table_count_and_size() {
        let many: Vec<_> = (0..500)
            .map(|i| outline(&format!("t{i}"), &["a", "b"]))
            .collect();
        let preview = context(many).render();
        assert_eq!(preview.tables as usize, MAX_TABLES);
        assert!(preview.truncated);
        assert!(preview.text.contains("… 300 more not listed"));

        let wide: Vec<_> = (0..150)
            .map(|i| {
                let cols: Vec<String> = (0..60).map(|c| format!("column_number_{c}")).collect();
                TableOutline {
                    name: format!("wide_{i}"),
                    kind: None,
                    columns: cols,
                }
            })
            .collect();
        let preview = context(wide).render();
        assert!(preview.text.chars().count() <= MAX_CONTEXT_CHARS);
        assert!(preview.truncated);
        assert!((preview.tables as usize) < 150);
    }

    #[test]
    fn long_ddl_is_cut_and_counters_dropped() {
        let mut ctx = context(Vec::new());
        let long = format!("CREATE TABLE t (\n{}\n)", "  c int,\n".repeat(5000));
        ctx.current.as_mut().unwrap().ddl = long;
        let preview = ctx.render();
        assert!(preview.truncated);
        assert!(preview.text.contains("-- … (cut)"));
        assert_eq!(
            strip_counters(
                "CREATE TABLE `t` (\n  `id` int\n) ENGINE=InnoDB AUTO_INCREMENT=8472 DEFAULT CHARSET=utf8mb4"
            ),
            "CREATE TABLE `t` (\n  `id` int\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4"
        );
        // A column named auto_increment-ish stays.
        assert_eq!(
            strip_counters("`id` int AUTO_INCREMENT"),
            "`id` int AUTO_INCREMENT"
        );
    }

    #[test]
    fn cut_respects_char_boundaries() {
        assert_eq!(cut("héllo", 2), ("hé", true));
        assert_eq!(cut("hi", 5), ("hi", false));
    }
}
