//! SQL for the sidebar's object operations (M8): drop / truncate several tables (one statement
//! per kind where the server takes a list, so foreign keys and partitions among the selected
//! tables do not get in the way), clone a table, drop routines and sequences, set a comment, and
//! the creation scripts of routines and sequences. Pure: introspection and execution live in
//! `objects`.
//!
//! Identifiers only go through `Dialect::quote_ident` / `qualify`; comments are quoted literals
//! (DDL takes no bind parameters) spelled for the session's literal mode. Text the server
//! produced itself (PG argument lists, regproc names, `SHOW CREATE` output, SQLite's stored
//! `CREATE TABLE` / `CREATE INDEX`) is inserted verbatim, apart from the names a clone changes.

use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::quote::MysqlLiteralMode;
use crate::types::ddl::DdlPlan;
use crate::types::objects::{ObjectTarget, RoutineKind, RoutineRef};
use crate::types::schema::{TableKind, TableRef};

/// MySQL: turns foreign-key checks off for the session, around drops / truncates that ignore them.
pub const MYSQL_FK_CHECKS_OFF: &str = "SET FOREIGN_KEY_CHECKS = 0";
/// MySQL: turns them back on at the end of the same session.
pub const MYSQL_FK_CHECKS_ON: &str = "SET FOREIGN_KEY_CHECKS = 1";

fn qualified(d: Dialect, t: &TableRef) -> String {
    d.qualify(t.schema.as_deref(), &t.name)
}

/// PG and SQLite DDL runs in one transaction (all or nothing); MySQL DDL commits implicitly.
fn plan(d: Dialect, statements: Vec<String>, warnings: Vec<String>) -> DdlPlan {
    DdlPlan {
        statements,
        warnings,
        transactional: d != Dialect::Mysql,
    }
}

fn warn(warnings: &mut Vec<String>, msg: &str) {
    if !warnings.iter().any(|w| w == msg) {
        warnings.push(msg.to_string());
    }
}

fn nothing(what: &str) -> AppError {
    AppError::InvalidInput(format!("no {what} given"))
}

/// The object word of `DROP <word>` / `COMMENT ON <word>` for a table-like kind.
fn object_word(d: Dialect, kind: TableKind) -> Result<&'static str, AppError> {
    Ok(match kind {
        TableKind::Table | TableKind::Partitioned => "TABLE",
        TableKind::View => "VIEW",
        TableKind::MaterializedView if d == Dialect::Postgres => "MATERIALIZED VIEW",
        TableKind::MaterializedView => "VIEW",
        TableKind::ForeignTable if d == Dialect::Postgres => "FOREIGN TABLE",
        TableKind::ForeignTable => "TABLE",
        TableKind::SystemView => {
            return Err(AppError::Unsupported(
                "system views belong to the server and cannot be changed".into(),
            ));
        }
    })
}

/// Option warnings shared by drop and truncate.
fn option_warnings(d: Dialect, cascade: bool, ignore_fk: bool) -> Vec<String> {
    let mut warnings = Vec::new();
    if cascade && d != Dialect::Postgres {
        warn(&mut warnings, "CASCADE is PostgreSQL-only; ignored");
    }
    if ignore_fk && d != Dialect::Mysql {
        warn(
            &mut warnings,
            "Ignoring foreign-key checks is MySQL-only; ignored",
        );
    }
    warnings
}

/// Wraps MySQL statements in `SET FOREIGN_KEY_CHECKS = 0 … = 1` when asked to.
fn with_fk_checks_off(d: Dialect, ignore_fk: bool, statements: Vec<String>) -> Vec<String> {
    if !(ignore_fk && d == Dialect::Mysql) {
        return statements;
    }
    let mut out = Vec::with_capacity(statements.len() + 2);
    out.push(MYSQL_FK_CHECKS_OFF.to_string());
    out.extend(statements);
    out.push(MYSQL_FK_CHECKS_ON.to_string());
    out
}

/// The order DROP statements run in: views first, then what they may read (a view depending on a
/// table would otherwise stop the table's DROP).
const DROP_ORDER: [&str; 4] = ["VIEW", "MATERIALIZED VIEW", "FOREIGN TABLE", "TABLE"];

/// `DROP TABLE / VIEW / MATERIALIZED VIEW / FOREIGN TABLE`. PostgreSQL and MySQL drop every
/// object of a kind in ONE statement (`DROP TABLE a, b`), which the server accepts whatever the
/// foreign keys between them; SQLite drops one object per statement in the order given (the
/// caller puts referencing tables first, see `children_first`). With PG `CASCADE`, statements
/// after the first say `IF EXISTS`: an earlier one may already have dropped their objects.
/// Partitions whose partitioned table is dropped too are left out by the caller
/// (`skip_partitions`).
pub fn drop_tables(
    d: Dialect,
    targets: &[ObjectTarget],
    cascade: bool,
    ignore_fk: bool,
) -> Result<DdlPlan, AppError> {
    if targets.is_empty() {
        return Err(nothing("tables"));
    }
    let warnings = option_warnings(d, cascade, ignore_fk);
    let mut groups: [Vec<String>; DROP_ORDER.len()] = Default::default();
    for t in targets {
        let word = object_word(d, t.kind)?;
        let slot = DROP_ORDER
            .iter()
            .position(|w| *w == word)
            .unwrap_or(DROP_ORDER.len() - 1);
        let name = qualified(d, &t.table);
        if !groups[slot].contains(&name) {
            groups[slot].push(name);
        }
    }
    let pg_cascade = cascade && d == Dialect::Postgres;
    let mut statements = Vec::new();
    for (word, names) in DROP_ORDER.iter().zip(&groups) {
        if names.is_empty() {
            continue;
        }
        if d == Dialect::Sqlite {
            statements.extend(names.iter().map(|n| format!("DROP {word} {n}")));
            continue;
        }
        let if_exists = if pg_cascade && !statements.is_empty() {
            " IF EXISTS"
        } else {
            ""
        };
        let mut sql = format!("DROP {word}{if_exists} {}", names.join(", "));
        if pg_cascade {
            sql.push_str(" CASCADE");
        }
        statements.push(sql);
    }
    Ok(plan(
        d,
        with_fk_checks_off(d, ignore_fk, statements),
        warnings,
    ))
}

/// PostgreSQL: one `TRUNCATE TABLE a, b, c` (tables referencing each other must be truncated
/// together, and a partitioned table together with its partitions is fine). MySQL takes one
/// table per `TRUNCATE`. SQLite has none: `DELETE FROM` per table, in the order given (the
/// caller puts referencing tables first, see `children_first`).
pub fn truncate_tables(
    d: Dialect,
    tables: &[TableRef],
    cascade: bool,
    restart_identity: bool,
    ignore_fk: bool,
) -> Result<DdlPlan, AppError> {
    if tables.is_empty() {
        return Err(nothing("tables"));
    }
    let mut warnings = option_warnings(d, cascade, ignore_fk);
    match d {
        Dialect::Mysql if !restart_identity => {
            warn(&mut warnings, "MySQL TRUNCATE always resets AUTO_INCREMENT")
        }
        Dialect::Sqlite if restart_identity => warn(
            &mut warnings,
            "SQLite keeps AUTOINCREMENT counters on DELETE; restart identity ignored",
        ),
        _ => {}
    }
    let mut names: Vec<String> = Vec::with_capacity(tables.len());
    for t in tables {
        let qt = qualified(d, t);
        if !names.contains(&qt) {
            names.push(qt);
        }
    }
    let statements = match d {
        Dialect::Postgres => {
            let mut sql = format!("TRUNCATE TABLE {}", names.join(", "));
            if restart_identity {
                sql.push_str(" RESTART IDENTITY");
            }
            if cascade {
                sql.push_str(" CASCADE");
            }
            vec![sql]
        }
        Dialect::Mysql => names
            .iter()
            .map(|n| format!("TRUNCATE TABLE {n}"))
            .collect(),
        Dialect::Sqlite => names.iter().map(|n| format!("DELETE FROM {n}")).collect(),
    };
    Ok(plan(
        d,
        with_fk_checks_off(d, ignore_fk, statements),
        warnings,
    ))
}

/// Whether `t` is the table `schema`.`name` (a reference without a schema matches any schema).
fn is_table(t: &TableRef, schema: Option<&str>, name: &str) -> bool {
    t.name == name && (t.schema.is_none() || schema.is_none() || t.schema.as_deref() == schema)
}

/// `targets` without the partitions whose partitioned table (at any level) is among them:
/// dropping the partitioned table drops its partitions, so a statement of their own would fail
/// (`does not exist`). `ancestors` lists (partition, one of its ancestors) pairs. Returns the
/// remaining targets and how many were left out.
pub fn skip_partitions(
    targets: &[ObjectTarget],
    ancestors: &[(TableRef, TableRef)],
) -> (Vec<ObjectTarget>, usize) {
    let selected = |r: &TableRef| {
        targets
            .iter()
            .any(|t| is_table(&t.table, r.schema.as_deref(), &r.name))
    };
    let kept: Vec<ObjectTarget> = targets
        .iter()
        .filter(|t| {
            !ancestors.iter().any(|(part, parent)| {
                is_table(&t.table, part.schema.as_deref(), &part.name) && selected(parent)
            })
        })
        .cloned()
        .collect();
    let skipped = targets.len() - kept.len();
    (kept, skipped)
}

/// The warning of a plan that left partitions out (`skip_partitions`).
pub fn skipped_partitions_warning(n: usize) -> String {
    if n == 1 {
        "1 partition is dropped with its partitioned table".to_string()
    } else {
        format!("{n} partitions are dropped with their partitioned table")
    }
}

/// `tables` with every table that references another one of them (a foreign key, `(child,
/// parent)` in `references`) before the tables it references, so per-table DROP / DELETE
/// statements never trip over a foreign key between them. Otherwise keeps the given order;
/// self-references are ignored and a cycle keeps its tables in the given order.
pub fn children_first(tables: &[TableRef], references: &[(TableRef, TableRef)]) -> Vec<TableRef> {
    let mut rest: Vec<TableRef> = tables.to_vec();
    let mut out = Vec::with_capacity(rest.len());
    while !rest.is_empty() {
        // A table nothing else still waiting references can go now.
        let free = rest.iter().position(|t| {
            !references.iter().any(|(child, parent)| {
                is_table(t, parent.schema.as_deref(), &parent.name)
                    && !is_table(t, child.schema.as_deref(), &child.name)
                    && rest
                        .iter()
                        .any(|r| is_table(r, child.schema.as_deref(), &child.name))
            })
        });
        out.push(rest.remove(free.unwrap_or(0)));
    }
    out
}

/// A PostgreSQL serial column of the source: its default is `nextval` of a sequence the column
/// owns, which `LIKE … INCLUDING ALL` would leave the clone sharing (and dropping the original
/// would then fail, or take the clone's default with it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SerialSource {
    pub column: String,
    /// The source sequence's settings (its name, schema and owner are not used).
    pub sequence: SequenceInfo,
}

/// What cloning needs to know about the source table (from its metadata).
#[derive(Debug, Clone, Default)]
pub struct CloneSource<'a> {
    /// Columns a plain INSERT can write, in table order (generated columns left out).
    pub columns: &'a [String],
    /// PG identity columns: their new sequences continue after the copied values.
    pub identity_columns: &'a [String],
    /// PG serial columns: the clone gets sequences of its own.
    pub serial_columns: &'a [SerialSource],
    /// SQLite: the table's `CREATE TABLE` text as `sqlite_master` stores it.
    pub create_sql: Option<&'a str>,
    /// SQLite: the table's own indexes as `(name, CREATE INDEX text)` (`sqlite_master.sql` not
    /// NULL; the ones behind PRIMARY KEY / UNIQUE come with the `CREATE TABLE` text).
    pub indexes: &'a [(String, String)],
    /// Names already taken in the schema (PG relations, SQLite `sqlite_master` entries): new
    /// sequence / index names avoid them.
    pub taken_names: &'a [String],
    /// The source has triggers, which are never copied.
    pub has_triggers: bool,
}

/// Error for a clone's new name, else the trimmed name.
pub fn clone_name(source: &TableRef, new_name: &str) -> Result<String, AppError> {
    let name = new_name.trim();
    if name.is_empty() {
        return Err(AppError::InvalidInput("the new table needs a name".into()));
    }
    if name == source.name {
        return Err(AppError::InvalidInput(
            "the clone needs a name other than the original's".into(),
        ));
    }
    Ok(name.to_string())
}

/// Copies a table into `new_name` of the same schema. PG: `CREATE TABLE … (LIKE … INCLUDING
/// ALL)`, then a sequence of the clone's own for each serial column (`<table>_<column>_seq`,
/// owned by the clone's column); MySQL: `CREATE TABLE … LIKE …`; SQLite: the original `CREATE
/// TABLE` text under the new name (foreign keys of the table to itself point at the clone), then
/// its indexes under new names. With `with_data` the rows follow (`INSERT INTO … SELECT …` over
/// the writable columns) and the clone's sequences continue after them. Foreign keys are not
/// copied on PG and MySQL (neither `LIKE` copies them), triggers nowhere.
pub fn clone_table(
    d: Dialect,
    source: &TableRef,
    new_name: &str,
    with_data: bool,
    src: &CloneSource<'_>,
) -> Result<DdlPlan, AppError> {
    let name = clone_name(source, new_name)?;
    let target = TableRef {
        schema: source.schema.clone(),
        name,
    };
    let (qs, qt) = (qualified(d, source), qualified(d, &target));
    let mut statements = vec![match d {
        Dialect::Postgres => format!("CREATE TABLE {qt} (LIKE {qs} INCLUDING ALL)"),
        Dialect::Mysql => format!("CREATE TABLE {qt} LIKE {qs}"),
        Dialect::Sqlite => {
            let create = src
                .create_sql
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| {
                    AppError::NotFound(format!("the CREATE TABLE text of {}", source.name))
                })?;
            let renamed = rename_create_table(create, &qt)?;
            retarget_self_references(&renamed, &source.name, &d.quote_ident(&target.name))
        }
    }];
    let mut taken: Vec<String> = src.taken_names.to_vec();
    taken.push(target.name.clone());
    // PG: sequences of the clone's own for its serial columns.
    let mut serials: Vec<(&str, String, bool)> = Vec::new();
    if d == Dialect::Postgres {
        // The identity sequences `LIKE` creates take these names first.
        for c in src.identity_columns {
            taken.push(pg_object_name(&target.name, c, "seq"));
        }
        for s in src.serial_columns {
            let seq = SequenceInfo {
                schema: target.schema.clone(),
                name: choose_pg_name(&target.name, &s.column, "seq", &mut taken),
                owned_by: Some((target.schema.clone(), target.name.clone(), s.column.clone())),
                ..s.sequence.clone()
            };
            let seq_name = d.qualify(seq.schema.as_deref(), &seq.name);
            let seq_lit = d.quote_literal_with(&seq_name, MysqlLiteralMode::default());
            statements.push(sequence_statement(&seq));
            statements.push(format!(
                "ALTER TABLE {qt} ALTER COLUMN {} SET DEFAULT nextval({seq_lit}::regclass)",
                d.quote_ident(&s.column)
            ));
            let descending = integer(&s.sequence.increment).is_some_and(|i| i.starts_with('-'));
            serials.push((s.column.as_str(), seq_lit, descending));
        }
    }
    // SQLite: the table's indexes, renamed.
    if d == Dialect::Sqlite {
        for (index, create) in src.indexes {
            let new_index = choose_sqlite_index_name(index, &source.name, &target.name, &mut taken);
            statements.push(rename_create_index(
                create,
                &d.qualify(target.schema.as_deref(), &new_index),
                &d.quote_ident(&target.name),
            )?);
        }
    }
    if with_data && !src.columns.is_empty() {
        let cols = src
            .columns
            .iter()
            .map(|c| d.quote_ident(c))
            .collect::<Vec<_>>()
            .join(", ");
        let overriding = if d == Dialect::Postgres && !src.identity_columns.is_empty() {
            " OVERRIDING SYSTEM VALUE"
        } else {
            ""
        };
        statements.push(format!(
            "INSERT INTO {qt} ({cols}){overriding} SELECT {cols} FROM {qs}"
        ));
        if d == Dialect::Postgres {
            for c in src.identity_columns {
                statements.push(format!(
                    "SELECT setval(pg_get_serial_sequence({}, {}), COALESCE(MAX({}), 0) + 1, false) FROM {qt}",
                    d.quote_literal_with(&qt, MysqlLiteralMode::default()),
                    d.quote_literal_with(c, MysqlLiteralMode::default()),
                    d.quote_ident(c),
                ));
            }
            for (column, seq_lit, descending) in &serials {
                let agg = if *descending { "MIN" } else { "MAX" };
                let col = d.quote_ident(column);
                statements.push(format!(
                    "SELECT setval({seq_lit}::regclass, {agg}({col})) FROM {qt} HAVING {agg}({col}) IS NOT NULL"
                ));
            }
        }
    }
    let mut warnings = Vec::new();
    if d != Dialect::Sqlite {
        warnings.push("Foreign keys are not copied to the clone".to_string());
    }
    if src.has_triggers {
        warnings.push("Triggers are not copied to the clone".to_string());
    }
    Ok(plan(d, statements, warnings))
}

/// PostgreSQL's `makeObjectName`: `<name1>_<name2>_<label>`, the longer of the two names cut
/// (at a character boundary) until the whole fits in 63 bytes.
pub fn pg_object_name(name1: &str, name2: &str, label: &str) -> String {
    const MAX: usize = 63;
    let overhead = label.len() + 2;
    let (mut n1, mut n2) = (name1.len(), name2.len());
    while n1 + n2 + overhead > MAX && n1 + n2 > 0 {
        if n1 > n2 {
            n1 -= 1;
        } else {
            n2 -= 1;
        }
    }
    let cut = |s: &str, mut n: usize| {
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        s[..n].to_string()
    };
    format!("{}_{}_{label}", cut(name1, n1), cut(name2, n2))
}

/// PostgreSQL's `ChooseRelationName`: `pg_object_name` with label `seq`, then `seq1`, `seq2`…
/// until the name is not taken; the chosen name is added to `taken`.
fn choose_pg_name(name1: &str, name2: &str, label: &str, taken: &mut Vec<String>) -> String {
    let mut pass = 0usize;
    loop {
        let label = if pass == 0 {
            label.to_string()
        } else {
            format!("{label}{pass}")
        };
        let candidate = pg_object_name(name1, name2, &label);
        if !taken.contains(&candidate) {
            taken.push(candidate.clone());
            return candidate;
        }
        pass += 1;
    }
}

/// The clone's name for index `index` of table `source` (SQLite, where index names are unique
/// per database and compared without case): the source's name inside it, as a `_`-separated
/// part, replaced by the clone's (`idx_users_email` → `idx_users_copy_email`), else
/// `<clone>_<index>`; `_2`, `_3`… is appended while the name is taken. The chosen name is added
/// to `taken`.
fn choose_sqlite_index_name(
    index: &str,
    source: &str,
    clone: &str,
    taken: &mut Vec<String>,
) -> String {
    let part = (!source.is_empty())
        .then(|| {
            index.match_indices(source).map(|(i, _)| i).find(|&i| {
                let end = i + source.len();
                (i == 0 || index[..i].ends_with('_'))
                    && (end == index.len() || index[end..].starts_with('_'))
            })
        })
        .flatten();
    let base = match part {
        Some(i) => format!("{}{clone}{}", &index[..i], &index[i + source.len()..]),
        None => format!("{clone}_{index}"),
    };
    let free = |name: &str, taken: &[String]| !taken.iter().any(|t| t.eq_ignore_ascii_case(name));
    let mut candidate = base.clone();
    let mut n = 2;
    while !free(&candidate, taken) {
        candidate = format!("{base}_{n}");
        n += 1;
    }
    taken.push(candidate.clone());
    candidate
}

// --- SQLite CREATE TABLE renaming ------------------------------------------------------------

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Skips whitespace and comments from byte offset `i`.
fn skip_space(s: &str, mut i: usize) -> usize {
    loop {
        let rest = &s[i..];
        let trimmed = rest.trim_start();
        i += rest.len() - trimmed.len();
        if trimmed.starts_with("--") {
            i += trimmed.find('\n').map(|n| n + 1).unwrap_or(trimmed.len());
        } else if let Some(body) = trimmed.strip_prefix("/*") {
            i += body.find("*/").map(|n| n + 4).unwrap_or(trimmed.len());
        } else {
            return i;
        }
    }
}

/// End of keyword `kw` (case-insensitive, whole word) at byte offset `i`, if it is there.
fn keyword_at(s: &str, i: usize, kw: &str) -> Option<usize> {
    let rest = s.get(i..)?;
    let head = rest.get(..kw.len())?;
    if !head.eq_ignore_ascii_case(kw) {
        return None;
    }
    let end = i + kw.len();
    match s[end..].chars().next() {
        Some(c) if is_word_char(c) => None,
        _ => Some(end),
    }
}

/// End of the identifier token at `i`: `"…"` / `` `…` `` / `'…'` (doubled quotes escape),
/// `[…]`, or a bare word.
fn ident_end(s: &str, i: usize) -> Option<usize> {
    let mut chars = s[i..].char_indices();
    let (_, first) = chars.next()?;
    let close = match first {
        '"' | '`' | '\'' => first,
        '[' => ']',
        c if is_word_char(c) => {
            let len = s[i..]
                .char_indices()
                .find(|&(_, c)| !is_word_char(c))
                .map(|(n, _)| n)
                .unwrap_or(s.len() - i);
            return Some(i + len);
        }
        _ => return None,
    };
    let mut iter = s[i + 1..].char_indices().peekable();
    while let Some((n, c)) = iter.next() {
        if c != close {
            continue;
        }
        if close != ']' && iter.peek().is_some_and(|&(_, next)| next == close) {
            iter.next();
            continue;
        }
        return Some(i + 1 + n + c.len_utf8());
    }
    None
}

/// `CREATE TABLE <name> …` (as `sqlite_master` stores it; `TEMP`, `IF NOT EXISTS` and a schema
/// qualifier are accepted too) with the table name replaced by `new_name_sql`, which is already
/// quoted / qualified SQL. The rest of the text (columns, constraints, options) is kept as is.
pub fn rename_create_table(sql: &str, new_name_sql: &str) -> Result<String, AppError> {
    let bad = || AppError::Unsupported("could not read the table's CREATE TABLE text".into());
    let mut i = skip_space(sql, 0);
    i = keyword_at(sql, i, "CREATE").ok_or_else(bad)?;
    i = skip_space(sql, i);
    if let Some(end) = keyword_at(sql, i, "TEMPORARY").or_else(|| keyword_at(sql, i, "TEMP")) {
        i = skip_space(sql, end);
    }
    if keyword_at(sql, i, "VIRTUAL").is_some() {
        return Err(AppError::Unsupported(
            "virtual tables (FTS, R*Tree…) cannot be cloned".into(),
        ));
    }
    i = keyword_at(sql, i, "TABLE").ok_or_else(bad)?;
    i = skip_space(sql, i);
    if let Some(end) = keyword_at(sql, i, "IF") {
        let j = skip_space(sql, end);
        let j = keyword_at(sql, j, "NOT").ok_or_else(bad)?;
        let j = skip_space(sql, j);
        let j = keyword_at(sql, j, "EXISTS").ok_or_else(bad)?;
        i = skip_space(sql, j);
    }
    let start = i;
    let mut end = ident_end(sql, i).ok_or_else(bad)?;
    let after = skip_space(sql, end);
    if sql[after..].starts_with('.') {
        let j = skip_space(sql, after + 1);
        end = ident_end(sql, j).ok_or_else(bad)?;
    }
    Ok(format!("{}{new_name_sql}{}", &sql[..start], &sql[end..]))
}

/// The name an identifier token stands for: quotes removed (`"a""b"` → `a"b`, `[x]` → `x`).
fn unquote_ident(token: &str) -> String {
    let mut chars = token.chars();
    match (chars.next(), token.chars().last()) {
        (Some(open @ ('"' | '`' | '\'')), Some(close)) if close == open && token.len() >= 2 => {
            let inner = &token[1..token.len() - 1];
            let doubled: String = [open, open].iter().collect();
            inner.replace(&doubled, &open.to_string())
        }
        (Some('['), Some(']')) if token.len() >= 2 => token[1..token.len() - 1].to_string(),
        _ => token.to_string(),
    }
}

/// End of the string literal / quoted identifier / comment starting at `i`, if one does.
fn skip_quoted(s: &str, i: usize) -> Option<usize> {
    let rest = &s[i..];
    if rest.starts_with("--") {
        return Some(i + rest.find('\n').map(|n| n + 1).unwrap_or(rest.len()));
    }
    if let Some(body) = rest.strip_prefix("/*") {
        return Some(i + body.find("*/").map(|n| n + 4).unwrap_or(rest.len()));
    }
    match rest.chars().next()? {
        '"' | '`' | '\'' | '[' => Some(ident_end(s, i).unwrap_or(s.len())),
        _ => None,
    }
}

/// SQLite `CREATE TABLE` text with every `REFERENCES <old_name>` (a foreign key of the table to
/// itself; names compare without case, as SQLite does) pointing at `new_name_sql` instead, an
/// unqualified quoted name: a foreign-key clause cannot name a schema. Strings, quoted names
/// and comments are skipped.
pub fn retarget_self_references(sql: &str, old_name: &str, new_name_sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut copied = 0;
    let mut i = 0;
    while i < sql.len() {
        if let Some(end) = skip_quoted(sql, i) {
            i = end;
            continue;
        }
        let Some(c) = sql[i..].chars().next() else {
            break;
        };
        if !is_word_char(c) {
            i += c.len_utf8();
            continue;
        }
        let word_end = ident_end(sql, i).unwrap_or(sql.len());
        if keyword_at(sql, i, "REFERENCES").is_some() {
            let start = skip_space(sql, word_end);
            if let Some(end) = ident_end(sql, start)
                && unquote_ident(&sql[start..end]).eq_ignore_ascii_case(old_name)
            {
                out.push_str(&sql[copied..start]);
                out.push_str(new_name_sql);
                copied = end;
                i = end;
                continue;
            }
        }
        i = word_end;
    }
    out.push_str(&sql[copied..]);
    out
}

/// SQLite `CREATE [UNIQUE] INDEX [IF NOT EXISTS] [schema.]name ON table …` (as `sqlite_master`
/// stores it) with the index name replaced by `new_index_sql` (quoted, may be qualified) and the
/// table by `new_table_sql` (quoted, unqualified, as SQLite wants it after `ON`).
pub fn rename_create_index(
    sql: &str,
    new_index_sql: &str,
    new_table_sql: &str,
) -> Result<String, AppError> {
    let bad = || AppError::Unsupported("could not read an index's CREATE INDEX text".into());
    let mut i = skip_space(sql, 0);
    i = keyword_at(sql, i, "CREATE").ok_or_else(bad)?;
    i = skip_space(sql, i);
    if let Some(end) = keyword_at(sql, i, "UNIQUE") {
        i = skip_space(sql, end);
    }
    i = keyword_at(sql, i, "INDEX").ok_or_else(bad)?;
    i = skip_space(sql, i);
    if let Some(end) = keyword_at(sql, i, "IF") {
        let j = skip_space(sql, end);
        let j = keyword_at(sql, j, "NOT").ok_or_else(bad)?;
        let j = skip_space(sql, j);
        let j = keyword_at(sql, j, "EXISTS").ok_or_else(bad)?;
        i = skip_space(sql, j);
    }
    let name_start = i;
    let mut name_end = ident_end(sql, i).ok_or_else(bad)?;
    let after = skip_space(sql, name_end);
    if sql[after..].starts_with('.') {
        let j = skip_space(sql, after + 1);
        name_end = ident_end(sql, j).ok_or_else(bad)?;
    }
    let on = skip_space(sql, name_end);
    let table_start = skip_space(sql, keyword_at(sql, on, "ON").ok_or_else(bad)?);
    let table_end = ident_end(sql, table_start).ok_or_else(bad)?;
    Ok(format!(
        "{}{new_index_sql}{}{new_table_sql}{}",
        &sql[..name_start],
        &sql[name_end..table_start],
        &sql[table_end..]
    ))
}

// --- routines --------------------------------------------------------------------------------

/// Whether a PG argument list can go into `DROP FUNCTION name(…)` as is: balanced parentheses,
/// and no `;` or comment outside double-quoted names. (The list also has to match the server's
/// own `pg_get_function_identity_arguments` of an existing overload; see `objects`.)
pub fn safe_argument_list(args: &str) -> bool {
    let mut depth = 0i32;
    let mut quoted = false;
    let mut prev = '\0';
    for c in args.chars() {
        if quoted {
            if c == '"' {
                quoted = false;
            }
            prev = c;
            continue;
        }
        match c {
            '"' => quoted = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            ';' => return false,
            '-' if prev == '-' => return false,
            '*' if prev == '/' => return false,
            _ => {}
        }
        prev = c;
    }
    depth == 0 && !quoted
}

fn routine_word(kind: RoutineKind, d: Dialect) -> &'static str {
    match kind {
        RoutineKind::Procedure => "PROCEDURE",
        RoutineKind::Aggregate if d == Dialect::Postgres => "AGGREGATE",
        _ => "FUNCTION",
    }
}

/// `DROP FUNCTION / PROCEDURE / AGGREGATE`, one per routine. PG addresses the overload by its
/// identity arguments.
pub fn drop_routines(
    d: Dialect,
    routines: &[RoutineRef],
    cascade: bool,
) -> Result<DdlPlan, AppError> {
    if d == Dialect::Sqlite {
        return Err(AppError::Unsupported(
            "SQLite has no stored routines".into(),
        ));
    }
    if routines.is_empty() {
        return Err(nothing("routines"));
    }
    let warnings = option_warnings(d, cascade, false);
    let mut statements = Vec::with_capacity(routines.len());
    for r in routines {
        let name = d.qualify(r.schema.as_deref(), &r.name);
        let mut sql = format!("DROP {} {name}", routine_word(r.kind, d));
        if d == Dialect::Postgres {
            if !safe_argument_list(&r.identity_arguments) {
                return Err(AppError::InvalidInput(format!(
                    "unexpected argument list for {}",
                    r.name
                )));
            }
            sql.push_str(&format!("({})", r.identity_arguments));
            if cascade {
                sql.push_str(" CASCADE");
            }
        }
        statements.push(sql);
    }
    Ok(plan(d, statements, warnings))
}

/// `DROP SEQUENCE`, one per sequence (PostgreSQL).
pub fn drop_sequences(
    d: Dialect,
    sequences: &[TableRef],
    cascade: bool,
) -> Result<DdlPlan, AppError> {
    if d != Dialect::Postgres {
        return Err(AppError::Unsupported(
            "sequences are listed for PostgreSQL only".into(),
        ));
    }
    if sequences.is_empty() {
        return Err(nothing("sequences"));
    }
    let statements = sequences
        .iter()
        .map(|s| {
            let mut sql = format!("DROP SEQUENCE {}", qualified(d, s));
            if cascade {
                sql.push_str(" CASCADE");
            }
            sql
        })
        .collect();
    Ok(plan(d, statements, Vec::new()))
}

/// Sets (or, with `None` / empty, removes) the comment of a table, view or materialized view.
/// `mode`: how the MySQL session reads backslashes (`sql_mode`); PostgreSQL uses `E'…'` for text
/// with a backslash, which means the same whatever `standard_conforming_strings` is.
pub fn set_comment(
    d: Dialect,
    target: &ObjectTarget,
    comment: Option<&str>,
    mode: MysqlLiteralMode,
) -> Result<DdlPlan, AppError> {
    let comment = comment.filter(|c| !c.is_empty());
    let qt = qualified(d, &target.table);
    let sql = match d {
        Dialect::Postgres => format!(
            "COMMENT ON {} {qt} IS {}",
            object_word(d, target.kind)?,
            comment.map_or_else(|| "NULL".to_string(), |c| d.quote_literal_with(c, mode))
        ),
        Dialect::Mysql => match target.kind {
            TableKind::Table | TableKind::Partitioned => format!(
                "ALTER TABLE {qt} COMMENT = {}",
                d.quote_literal_with(comment.unwrap_or(""), mode)
            ),
            _ => {
                return Err(AppError::Unsupported(
                    "MySQL views cannot have a comment".into(),
                ));
            }
        },
        Dialect::Sqlite => {
            return Err(AppError::Unsupported(
                "SQLite tables cannot have a comment".into(),
            ));
        }
    };
    Ok(plan(d, vec![sql], Vec::new()))
}

/// `SHOW CREATE FUNCTION | PROCEDURE` for a MySQL routine.
pub fn mysql_show_create(kind: RoutineKind, schema: Option<&str>, name: &str) -> String {
    let what = match kind {
        RoutineKind::Procedure => "PROCEDURE",
        _ => "FUNCTION",
    };
    format!(
        "SHOW CREATE {what} {}",
        Dialect::Mysql.qualify(schema, name)
    )
}

/// What a MySQL routine opens as: a `DROP … IF EXISTS` and the server's `CREATE` text, as two
/// statements the editor runs one after the other (no `DELIMITER`: the editor keeps a
/// `BEGIN … END` body together).
pub fn mysql_routine_script(kind: RoutineKind, name: &str, create: &str) -> String {
    let what = match kind {
        RoutineKind::Procedure => "PROCEDURE",
        _ => "FUNCTION",
    };
    let body = create.trim();
    let body = body.strip_suffix(';').unwrap_or(body).trim_end();
    format!(
        "DROP {what} IF EXISTS {};\n\n{body};\n",
        Dialect::Mysql.quote_ident(name)
    )
}

/// `pg_get_functiondef` output (a `CREATE OR REPLACE …` without terminator) as an editor script.
pub fn pg_routine_script(definition: &str) -> String {
    let body = definition.trim_end();
    if body.ends_with(';') {
        format!("{body}\n")
    } else {
        format!("{body};\n")
    }
}

/// An aggregate as `pg_aggregate` describes it (names are the server's regproc / type text).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AggregateInfo {
    pub schema: Option<String>,
    pub name: String,
    pub arguments: String,
    pub sfunc: String,
    pub stype: String,
    pub finalfunc: Option<String>,
    pub combinefunc: Option<String>,
    pub initcond: Option<String>,
    pub sortop: Option<String>,
}

/// `CREATE OR REPLACE AGGREGATE` text (PG has no `pg_get_functiondef` for aggregates).
pub fn aggregate_ddl(a: &AggregateInfo) -> String {
    let d = Dialect::Postgres;
    let mut opts = vec![
        format!("SFUNC = {}", a.sfunc),
        format!("STYPE = {}", a.stype),
    ];
    if let Some(f) = &a.finalfunc {
        opts.push(format!("FINALFUNC = {f}"));
    }
    if let Some(f) = &a.combinefunc {
        opts.push(format!("COMBINEFUNC = {f}"));
    }
    if let Some(v) = &a.initcond {
        opts.push(format!(
            "INITCOND = {}",
            d.quote_literal_with(v, MysqlLiteralMode::default())
        ));
    }
    if let Some(op) = &a.sortop {
        opts.push(format!("SORTOP = {op}"));
    }
    format!(
        "CREATE OR REPLACE AGGREGATE {}({}) (\n  {}\n);\n",
        d.qualify(a.schema.as_deref(), &a.name),
        a.arguments,
        opts.join(",\n  ")
    )
}

// --- sequences -------------------------------------------------------------------------------

/// A PostgreSQL sequence's settings (`pg_sequence`), numbers as the server prints them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SequenceInfo {
    pub schema: Option<String>,
    pub name: String,
    pub data_type: Option<String>,
    pub start: Option<String>,
    pub increment: Option<String>,
    pub min: Option<String>,
    pub max: Option<String>,
    pub cache: Option<String>,
    pub cycle: bool,
    /// (schema, table, column) of the serial column the sequence is `OWNED BY`.
    pub owned_by: Option<(Option<String>, String, String)>,
}

fn integer(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| {
        let digits = s.strip_prefix('-').unwrap_or(s);
        !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
    })
}

/// `CREATE SEQUENCE <name>` and its options, one per item.
fn sequence_parts(s: &SequenceInfo) -> Vec<String> {
    let d = Dialect::Postgres;
    let name = d.qualify(s.schema.as_deref(), &s.name);
    let mut lines = vec![format!("CREATE SEQUENCE {name}")];
    if let Some(ty) = s
        .data_type
        .as_deref()
        .filter(|t| matches!(*t, "smallint" | "integer" | "bigint"))
    {
        lines.push(format!("AS {ty}"));
    }
    for (label, value) in [
        ("INCREMENT BY", &s.increment),
        ("MINVALUE", &s.min),
        ("MAXVALUE", &s.max),
        ("START WITH", &s.start),
        ("CACHE", &s.cache),
    ] {
        if let Some(v) = integer(value) {
            lines.push(format!("{label} {v}"));
        }
    }
    lines.push(if s.cycle { "CYCLE" } else { "NO CYCLE" }.to_string());
    lines
}

/// `OWNED BY "schema"."table"."column"` of a sequence that has an owner.
fn owned_by_clause(s: &SequenceInfo) -> Option<String> {
    let d = Dialect::Postgres;
    s.owned_by.as_ref().map(|(schema, table, column)| {
        format!(
            "OWNED BY {}.{}",
            d.qualify(schema.as_deref(), table),
            d.quote_ident(column)
        )
    })
}

/// One `CREATE SEQUENCE … [OWNED BY …]` statement (no terminator).
fn sequence_statement(s: &SequenceInfo) -> String {
    let mut parts = sequence_parts(s);
    parts.extend(owned_by_clause(s));
    parts.join(" ")
}

/// `CREATE SEQUENCE …` (+ `ALTER SEQUENCE … OWNED BY …`) for a sequence.
pub fn sequence_ddl(s: &SequenceInfo) -> String {
    let d = Dialect::Postgres;
    let name = d.qualify(s.schema.as_deref(), &s.name);
    let lines = sequence_parts(s);
    let mut out = format!("{};\n", lines.join("\n  "));
    if let Some(owned) = owned_by_clause(s) {
        out.push_str(&format!("\nALTER SEQUENCE {name} {owned};\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlgen::split::split_statements;

    fn t(schema: Option<&str>, name: &str) -> TableRef {
        TableRef::new(schema, name)
    }

    fn target(schema: Option<&str>, name: &str, kind: TableKind) -> ObjectTarget {
        ObjectTarget {
            table: t(schema, name),
            kind,
        }
    }

    fn routine(name: &str, kind: RoutineKind, args: &str) -> RoutineRef {
        RoutineRef {
            schema: Some("public".into()),
            name: name.into(),
            kind,
            identity_arguments: args.into(),
        }
    }

    #[test]
    fn drops_one_statement_per_kind_views_first() {
        let targets = [
            target(Some("public"), "users", TableKind::Table),
            target(Some("public"), "Order Items", TableKind::Partitioned),
            target(Some("public"), "active", TableKind::View),
            target(Some("public"), "counts_mv", TableKind::MaterializedView),
            target(Some("public"), "remote", TableKind::ForeignTable),
            target(Some("sales"), "users", TableKind::Table),
            target(Some("public"), "users", TableKind::Table),
        ];
        let p = drop_tables(Dialect::Postgres, &targets, true, false).unwrap();
        assert_eq!(
            p.statements,
            [
                r#"DROP VIEW "public"."active" CASCADE"#,
                r#"DROP MATERIALIZED VIEW IF EXISTS "public"."counts_mv" CASCADE"#,
                r#"DROP FOREIGN TABLE IF EXISTS "public"."remote" CASCADE"#,
                r#"DROP TABLE IF EXISTS "public"."users", "public"."Order Items", "sales"."users" CASCADE"#,
            ]
        );
        assert!(p.transactional);
        assert!(p.warnings.is_empty());
        // Without CASCADE nothing else can drop them first: no IF EXISTS.
        let p = drop_tables(Dialect::Postgres, &targets[..3], false, false).unwrap();
        assert_eq!(
            p.statements,
            [
                r#"DROP VIEW "public"."active""#,
                r#"DROP TABLE "public"."users", "public"."Order Items""#,
            ]
        );

        // SQLite drops one object per statement, in the given order (tables after views).
        let p = drop_tables(
            Dialect::Sqlite,
            &[
                target(Some("main"), "child", TableKind::Table),
                target(Some("main"), "a\"b", TableKind::Table),
                target(Some("main"), "v", TableKind::View),
            ],
            false,
            true,
        )
        .unwrap();
        assert_eq!(
            p.statements,
            [
                r#"DROP VIEW "main"."v""#,
                r#"DROP TABLE "main"."child""#,
                r#"DROP TABLE "main"."a""b""#,
            ]
        );
        assert_eq!(p.warnings.len(), 1);

        assert!(
            drop_tables(
                Dialect::Postgres,
                &[target(None, "pg_stat", TableKind::SystemView)],
                false,
                false
            )
            .is_err()
        );
        assert!(drop_tables(Dialect::Postgres, &[], false, false).is_err());
    }

    #[test]
    fn mysql_drops_ignoring_foreign_keys_in_one_session() {
        let targets = [
            target(Some("shop"), "orders", TableKind::Table),
            target(Some("shop"), "v`x", TableKind::View),
            target(Some("shop"), "customers", TableKind::Table),
        ];
        let p = drop_tables(Dialect::Mysql, &targets, true, true).unwrap();
        assert_eq!(
            p.statements,
            [
                "SET FOREIGN_KEY_CHECKS = 0",
                "DROP VIEW `shop`.`v``x`",
                "DROP TABLE `shop`.`orders`, `shop`.`customers`",
                "SET FOREIGN_KEY_CHECKS = 1",
            ]
        );
        assert!(!p.transactional);
        assert_eq!(p.warnings, ["CASCADE is PostgreSQL-only; ignored"]);
        let p = drop_tables(Dialect::Mysql, &targets[..1], false, false).unwrap();
        assert_eq!(p.statements, ["DROP TABLE `shop`.`orders`"]);
    }

    #[test]
    fn truncates_per_dialect() {
        let tables = [
            t(Some("public"), "a"),
            t(Some("public"), "b"),
            t(Some("public"), "a"),
        ];
        let p = truncate_tables(Dialect::Postgres, &tables, true, true, false).unwrap();
        assert_eq!(
            p.statements,
            [r#"TRUNCATE TABLE "public"."a", "public"."b" RESTART IDENTITY CASCADE"#]
        );
        let p = truncate_tables(Dialect::Postgres, &tables[..2], false, false, false).unwrap();
        assert_eq!(
            p.statements,
            [r#"TRUNCATE TABLE "public"."a", "public"."b""#]
        );
        let p = truncate_tables(Dialect::Mysql, &tables[..2], false, false, true).unwrap();
        assert_eq!(
            p.statements,
            [
                "SET FOREIGN_KEY_CHECKS = 0",
                "TRUNCATE TABLE `public`.`a`",
                "TRUNCATE TABLE `public`.`b`",
                "SET FOREIGN_KEY_CHECKS = 1",
            ]
        );
        assert_eq!(p.warnings, ["MySQL TRUNCATE always resets AUTO_INCREMENT"]);
        let p = truncate_tables(
            Dialect::Sqlite,
            &[t(None, "child"), t(None, "x")],
            false,
            true,
            false,
        )
        .unwrap();
        assert_eq!(
            p.statements,
            [r#"DELETE FROM "child""#, r#"DELETE FROM "x""#]
        );
        assert!(p.transactional);
        assert_eq!(p.warnings.len(), 1);
    }

    #[test]
    fn orders_referencing_tables_first() {
        let (a, b, c, d) = (
            t(Some("main"), "a"),
            t(Some("main"), "b"),
            t(Some("main"), "c"),
            t(Some("main"), "d"),
        );
        // c → b → a (c references b, b references a); d stands alone; a references itself.
        let refs = [
            (b.clone(), a.clone()),
            (c.clone(), b.clone()),
            (a.clone(), a.clone()),
            (t(Some("main"), "elsewhere"), d.clone()),
        ];
        let order = children_first(&[a.clone(), b.clone(), d.clone(), c.clone()], &refs);
        assert_eq!(order, [d.clone(), c.clone(), b.clone(), a.clone()]);
        // A cycle keeps the given order.
        let cycle = [(a.clone(), b.clone()), (b.clone(), a.clone())];
        assert_eq!(
            children_first(&[a.clone(), b.clone()], &cycle),
            [a.clone(), b.clone()]
        );
        assert!(children_first(&[], &refs).is_empty());
    }

    #[test]
    fn leaves_out_partitions_of_dropped_partitioned_tables() {
        let targets = [
            target(Some("public"), "p", TableKind::Partitioned),
            target(Some("public"), "p1", TableKind::Table),
            target(Some("public"), "p2_1", TableKind::Table),
            target(Some("public"), "fp", TableKind::ForeignTable),
            target(Some("public"), "q1", TableKind::Table),
        ];
        let ancestors = [
            (t(Some("public"), "p1"), t(Some("public"), "p")),
            (t(Some("public"), "p2_1"), t(Some("public"), "p2")),
            (t(Some("public"), "p2_1"), t(Some("public"), "p")),
            (t(Some("public"), "fp"), t(Some("public"), "p")),
            // q1's partitioned table is not dropped: q1 stays.
            (t(Some("public"), "q1"), t(Some("public"), "q")),
        ];
        let (kept, skipped) = skip_partitions(&targets, &ancestors);
        assert_eq!(
            kept.iter()
                .map(|k| k.table.name.as_str())
                .collect::<Vec<_>>(),
            ["p", "q1"]
        );
        assert_eq!(skipped, 3);
        assert_eq!(
            skipped_partitions_warning(3),
            "3 partitions are dropped with their partitioned table"
        );
        let p = drop_tables(Dialect::Postgres, &kept, false, false).unwrap();
        assert_eq!(p.statements, [r#"DROP TABLE "public"."p", "public"."q1""#]);
    }

    #[test]
    fn clones_pg_tables_with_like_and_copies_rows() {
        let cols = ["id".to_string(), "Name".to_string()];
        let identity = ["id".to_string()];
        let src = CloneSource {
            columns: &cols,
            identity_columns: &identity,
            ..CloneSource::default()
        };
        let users = t(Some("public"), "users");
        let p = clone_table(Dialect::Postgres, &users, " users_copy ", true, &src).unwrap();
        assert_eq!(
            p.statements,
            [
                r#"CREATE TABLE "public"."users_copy" (LIKE "public"."users" INCLUDING ALL)"#,
                r#"INSERT INTO "public"."users_copy" ("id", "Name") OVERRIDING SYSTEM VALUE SELECT "id", "Name" FROM "public"."users""#,
                r#"SELECT setval(pg_get_serial_sequence('"public"."users_copy"', 'id'), COALESCE(MAX("id"), 0) + 1, false) FROM "public"."users_copy""#,
            ]
        );
        assert!(p.transactional);

        let plain = CloneSource {
            columns: &cols,
            ..CloneSource::default()
        };
        let p = clone_table(Dialect::Postgres, &users, "u2", false, &plain).unwrap();
        assert_eq!(p.statements.len(), 1);
        let p = clone_table(Dialect::Postgres, &users, "u2", true, &plain).unwrap();
        assert_eq!(
            p.statements[1],
            r#"INSERT INTO "public"."u2" ("id", "Name") SELECT "id", "Name" FROM "public"."users""#
        );

        assert!(clone_table(Dialect::Postgres, &users, "users", true, &src).is_err());
        assert!(clone_table(Dialect::Postgres, &users, "  ", true, &src).is_err());
    }

    #[test]
    fn gives_a_pg_clone_its_own_serial_sequences() {
        let cols = ["id".to_string(), "down".to_string(), "name".to_string()];
        let serial = |column: &str, increment: &str| SerialSource {
            column: column.into(),
            sequence: SequenceInfo {
                data_type: Some("integer".into()),
                start: Some("1".into()),
                increment: Some(increment.into()),
                min: Some("-2147483648".into()),
                max: Some("2147483647".into()),
                cache: Some("1".into()),
                ..SequenceInfo::default()
            },
        };
        let serials = [serial("id", "1"), serial("down", "-1")];
        // `users_copy_id_seq` is taken (by another table's sequence, say): the next one is used.
        let taken = ["users_copy_id_seq".to_string(), "users".to_string()];
        let src = CloneSource {
            columns: &cols,
            serial_columns: &serials,
            taken_names: &taken,
            has_triggers: true,
            ..CloneSource::default()
        };
        let users = t(Some("public"), "users");
        let p = clone_table(Dialect::Postgres, &users, "users_copy", true, &src).unwrap();
        assert_eq!(
            p.statements,
            [
                r#"CREATE TABLE "public"."users_copy" (LIKE "public"."users" INCLUDING ALL)"#,
                r#"CREATE SEQUENCE "public"."users_copy_id_seq1" AS integer INCREMENT BY 1 MINVALUE -2147483648 MAXVALUE 2147483647 START WITH 1 CACHE 1 NO CYCLE OWNED BY "public"."users_copy"."id""#,
                r#"ALTER TABLE "public"."users_copy" ALTER COLUMN "id" SET DEFAULT nextval('"public"."users_copy_id_seq1"'::regclass)"#,
                r#"CREATE SEQUENCE "public"."users_copy_down_seq" AS integer INCREMENT BY -1 MINVALUE -2147483648 MAXVALUE 2147483647 START WITH 1 CACHE 1 NO CYCLE OWNED BY "public"."users_copy"."down""#,
                r#"ALTER TABLE "public"."users_copy" ALTER COLUMN "down" SET DEFAULT nextval('"public"."users_copy_down_seq"'::regclass)"#,
                r#"INSERT INTO "public"."users_copy" ("id", "down", "name") SELECT "id", "down", "name" FROM "public"."users""#,
                r#"SELECT setval('"public"."users_copy_id_seq1"'::regclass, MAX("id")) FROM "public"."users_copy" HAVING MAX("id") IS NOT NULL"#,
                r#"SELECT setval('"public"."users_copy_down_seq"'::regclass, MIN("down")) FROM "public"."users_copy" HAVING MIN("down") IS NOT NULL"#,
            ]
        );
        assert_eq!(
            p.warnings,
            [
                "Foreign keys are not copied to the clone",
                "Triggers are not copied to the clone"
            ]
        );
        // Without rows the new sequences start where the source's started.
        let p = clone_table(Dialect::Postgres, &users, "users_copy", false, &src).unwrap();
        assert_eq!(p.statements.len(), 5);
    }

    #[test]
    fn names_objects_like_postgres() {
        assert_eq!(pg_object_name("users", "id", "seq"), "users_id_seq");
        let long = "a".repeat(70);
        let name = pg_object_name(&long, "id", "seq");
        assert_eq!(name.len(), 63);
        assert!(name.ends_with("_id_seq"));
        // Multi-byte names are cut at a character boundary.
        let name = pg_object_name(&"é".repeat(40), &"ü".repeat(40), "seq");
        assert!(name.len() <= 63 && name.ends_with("_seq"), "{name}");
        let mut taken = vec!["t_c_seq".to_string(), "t_c_seq1".to_string()];
        assert_eq!(choose_pg_name("t", "c", "seq", &mut taken), "t_c_seq2");
        assert_eq!(choose_pg_name("t", "c", "seq", &mut taken), "t_c_seq3");

        // SQLite index names: the table's name as a part of the index's, else a prefix.
        let mut taken = Vec::new();
        let mut sqlite = |index: &str, source: &str, clone: &str| {
            choose_sqlite_index_name(index, source, clone, &mut taken)
        };
        assert_eq!(sqlite("data_idx", "a", "a2"), "a2_data_idx");
        assert_eq!(sqlite("a_idx", "a", "a2"), "a2_idx");
        assert_eq!(
            sqlite("idx_order_items_sku", "order_items", "oi"),
            "idx_oi_sku"
        );
        assert_eq!(sqlite("users", "users", "u2"), "u2");
        assert_eq!(sqlite("A2_IDX", "x", "a2"), "a2_A2_IDX");
        assert_eq!(sqlite("a_idx", "a", "a2"), "a2_idx_2");
    }

    #[test]
    fn clones_mysql_tables_with_create_like() {
        let cols = ["id".to_string(), "total".to_string()];
        let src = CloneSource {
            columns: &cols,
            ..CloneSource::default()
        };
        let p = clone_table(
            Dialect::Mysql,
            &t(Some("shop"), "orders"),
            "orders_copy",
            true,
            &src,
        )
        .unwrap();
        assert_eq!(
            p.statements,
            [
                "CREATE TABLE `shop`.`orders_copy` LIKE `shop`.`orders`",
                "INSERT INTO `shop`.`orders_copy` (`id`, `total`) SELECT `id`, `total` FROM `shop`.`orders`",
            ]
        );
        assert!(!p.transactional);
    }

    #[test]
    fn clones_sqlite_tables_from_their_create_text() {
        let cols = ["id".to_string(), "name".to_string()];
        let create = "CREATE TABLE \"my \"\"items\"\" \" (\n  id INTEGER PRIMARY KEY AUTOINCREMENT,\n  name TEXT -- items table name\n)";
        let src = CloneSource {
            columns: &cols,
            create_sql: Some(create),
            ..CloneSource::default()
        };
        let p = clone_table(
            Dialect::Sqlite,
            &t(Some("main"), "my \"items\" "),
            "items_copy",
            true,
            &src,
        )
        .unwrap();
        assert_eq!(
            p.statements,
            [
                "CREATE TABLE \"main\".\"items_copy\" (\n  id INTEGER PRIMARY KEY AUTOINCREMENT,\n  name TEXT -- items table name\n)",
                r#"INSERT INTO "main"."items_copy" ("id", "name") SELECT "id", "name" FROM "main"."my ""items"" ""#,
            ]
        );
        assert!(p.warnings.is_empty());
        let missing = CloneSource::default();
        assert!(clone_table(Dialect::Sqlite, &t(None, "x"), "y", false, &missing).is_err());
    }

    #[test]
    fn clones_sqlite_self_references_and_indexes() {
        let create = "CREATE TABLE emp (id INTEGER PRIMARY KEY, boss INTEGER REFERENCES \"Emp\"(id), \
                      dept INTEGER REFERENCES dept(id), note TEXT DEFAULT 'REFERENCES emp', \
                      -- REFERENCES emp\n mentor INTEGER CONSTRAINT m REFERENCES [emp] (id) ON DELETE SET NULL)";
        let indexes = [
            (
                "idx_emp_boss".to_string(),
                "CREATE INDEX idx_emp_boss ON emp(boss) WHERE boss IS NOT NULL".to_string(),
            ),
            (
                "by dept".to_string(),
                "CREATE UNIQUE INDEX IF NOT EXISTS main.\"by dept\" ON \"emp\" (dept, id DESC)"
                    .to_string(),
            ),
        ];
        let taken = [
            "emp".to_string(),
            "IDX_EMP2_BOSS".to_string(),
            "dept".to_string(),
        ];
        let src = CloneSource {
            create_sql: Some(create),
            indexes: &indexes,
            taken_names: &taken,
            has_triggers: true,
            ..CloneSource::default()
        };
        let p = clone_table(
            Dialect::Sqlite,
            &t(Some("main"), "emp"),
            "emp2",
            false,
            &src,
        )
        .unwrap();
        assert_eq!(
            p.statements,
            [
                "CREATE TABLE \"main\".\"emp2\" (id INTEGER PRIMARY KEY, boss INTEGER REFERENCES \"emp2\"(id), \
                 dept INTEGER REFERENCES dept(id), note TEXT DEFAULT 'REFERENCES emp', \
                 -- REFERENCES emp\n mentor INTEGER CONSTRAINT m REFERENCES \"emp2\" (id) ON DELETE SET NULL)",
                "CREATE INDEX \"main\".\"idx_emp2_boss_2\" ON \"emp2\"(boss) WHERE boss IS NOT NULL",
                "CREATE UNIQUE INDEX IF NOT EXISTS \"main\".\"emp2_by dept\" ON \"emp2\" (dept, id DESC)",
            ]
        );
        assert_eq!(p.warnings, ["Triggers are not copied to the clone"]);
        assert!(rename_create_index("CREATE VIEW v AS SELECT 1", "\"i\"", "\"t\"").is_err());
        assert!(rename_create_index("CREATE INDEX i (a)", "\"i\"", "\"t\"").is_err());
    }

    #[test]
    fn renames_every_spelling_of_a_create_table_name() {
        let new = "\"n\"";
        for (sql, expected) in [
            ("CREATE TABLE t(a)", "CREATE TABLE \"n\"(a)"),
            ("CREATE TABLE t (a)", "CREATE TABLE \"n\" (a)"),
            ("create table [my t] (a)", "create table \"n\" (a)"),
            ("CREATE TABLE `x``y`(a)", "CREATE TABLE \"n\"(a)"),
            ("CREATE TABLE 'q''t' (a)", "CREATE TABLE \"n\" (a)"),
            ("CREATE TEMP TABLE t (a)", "CREATE TEMP TABLE \"n\" (a)"),
            (
                "CREATE TABLE IF NOT EXISTS main.\"t\" (a)",
                "CREATE TABLE IF NOT EXISTS \"n\" (a)",
            ),
            ("CREATE TABLE \"a.b\" (a)", "CREATE TABLE \"n\" (a)"),
            (
                "  CREATE /* c */ TABLE tbl_ü (a)",
                "  CREATE /* c */ TABLE \"n\" (a)",
            ),
            (
                "CREATE TABLE t AS SELECT 1",
                "CREATE TABLE \"n\" AS SELECT 1",
            ),
        ] {
            assert_eq!(rename_create_table(sql, new).unwrap(), expected, "{sql}");
        }
        assert!(rename_create_table("CREATE VIEW v AS SELECT 1", new).is_err());
        assert!(rename_create_table("CREATE VIRTUAL TABLE f USING fts5(x)", new).is_err());
        assert!(rename_create_table("CREATE TABLE \"open (a)", new).is_err());
        assert!(rename_create_table("CREATE TABLEX t (a)", new).is_err());
    }

    #[test]
    fn drops_routines_by_overload() {
        let p = drop_routines(
            Dialect::Postgres,
            &[
                routine("add", RoutineKind::Function, "a integer, b integer"),
                routine("Archive", RoutineKind::Procedure, ""),
                routine("total", RoutineKind::Aggregate, "numeric"),
                routine("rank_it", RoutineKind::Window, "\"My Type\""),
            ],
            true,
        )
        .unwrap();
        assert_eq!(
            p.statements,
            [
                r#"DROP FUNCTION "public"."add"(a integer, b integer) CASCADE"#,
                r#"DROP PROCEDURE "public"."Archive"() CASCADE"#,
                r#"DROP AGGREGATE "public"."total"(numeric) CASCADE"#,
                r#"DROP FUNCTION "public"."rank_it"("My Type") CASCADE"#,
            ]
        );
        assert!(p.transactional);

        let mut f = routine("f", RoutineKind::Function, "integer); DROP TABLE users; --");
        assert!(drop_routines(Dialect::Postgres, &[f.clone()], false).is_err());
        f.identity_arguments = "x integer DEFAULT 1".into();
        assert!(drop_routines(Dialect::Postgres, &[f], false).is_ok());

        let p = drop_routines(
            Dialect::Mysql,
            &[
                RoutineRef {
                    schema: Some("shop".into()),
                    name: "calc`x".into(),
                    kind: RoutineKind::Function,
                    identity_arguments: String::new(),
                },
                RoutineRef {
                    schema: None,
                    name: "archive".into(),
                    kind: RoutineKind::Procedure,
                    identity_arguments: "ignored".into(),
                },
            ],
            false,
        )
        .unwrap();
        assert_eq!(
            p.statements,
            ["DROP FUNCTION `shop`.`calc``x`", "DROP PROCEDURE `archive`"]
        );
        assert!(!p.transactional);
        assert!(drop_routines(Dialect::Sqlite, &[], false).is_err());
    }

    #[test]
    fn checks_argument_lists() {
        for ok in [
            "",
            "integer",
            "a integer, VARIADIC b text[]",
            "x numeric(10,2)",
            "\"weird;name\" integer",
            "OUT r record",
        ] {
            assert!(safe_argument_list(ok), "{ok}");
        }
        for bad in [
            "a int; drop",
            "a int -- x",
            "a int /* x */",
            "(",
            "a)",
            "\"open",
        ] {
            assert!(!safe_argument_list(bad), "{bad}");
        }
    }

    #[test]
    fn drops_sequences_and_sets_comments() {
        let p = drop_sequences(
            Dialect::Postgres,
            &[t(Some("public"), "users_id_seq")],
            true,
        )
        .unwrap();
        assert_eq!(
            p.statements,
            [r#"DROP SEQUENCE "public"."users_id_seq" CASCADE"#]
        );
        assert!(drop_sequences(Dialect::Mysql, &[t(None, "s")], false).is_err());

        let view = target(Some("public"), "v", TableKind::View);
        let table = target(Some("public"), "users", TableKind::Table);
        let mv = target(Some("public"), "m", TableKind::MaterializedView);
        let esc = MysqlLiteralMode::BackslashEscapes;
        let no_esc = MysqlLiteralMode::NoBackslashEscapes;
        let pg = |o: &ObjectTarget, c: Option<&str>| {
            set_comment(Dialect::Postgres, o, c, esc)
                .unwrap()
                .statements
        };
        assert_eq!(
            pg(&table, Some("it's")),
            [r#"COMMENT ON TABLE "public"."users" IS 'it''s'"#]
        );
        assert_eq!(
            pg(&view, Some("")),
            [r#"COMMENT ON VIEW "public"."v" IS NULL"#]
        );
        assert_eq!(
            pg(&mv, None),
            [r#"COMMENT ON MATERIALIZED VIEW "public"."m" IS NULL"#]
        );
        assert_eq!(
            pg(&table, Some(r"C:\temp")),
            [r#"COMMENT ON TABLE "public"."users" IS E'C:\\temp'"#]
        );
        let my = target(Some("shop"), "orders", TableKind::Table);
        let mysql = |c: Option<&str>, mode| {
            set_comment(Dialect::Mysql, &my, c, mode)
                .unwrap()
                .statements
        };
        assert_eq!(
            mysql(Some(r"C:\temp it's"), esc),
            [r"ALTER TABLE `shop`.`orders` COMMENT = 'C:\\temp it''s'"]
        );
        // NO_BACKSLASH_ESCAPES: the backslash is kept single, the quote doubled.
        assert_eq!(
            mysql(Some(r"C:\temp it's"), no_esc),
            [r"ALTER TABLE `shop`.`orders` COMMENT = 'C:\temp it''s'"]
        );
        assert_eq!(
            mysql(None, esc),
            ["ALTER TABLE `shop`.`orders` COMMENT = ''"]
        );
        assert!(set_comment(Dialect::Mysql, &view, Some("x"), esc).is_err());
        assert!(set_comment(Dialect::Sqlite, &table, Some("x"), esc).is_err());
    }

    #[test]
    fn routine_scripts_split_into_their_statements() {
        let create = "CREATE DEFINER=`root`@`localhost` PROCEDURE `archive`(IN days INT)\nBEGIN\n  DELETE FROM posts WHERE created_at < NOW() - INTERVAL days DAY;\n  IF days > 30 THEN SELECT 'old'; END IF;\nEND";
        let script = mysql_routine_script(RoutineKind::Procedure, "archive", create);
        assert!(script.starts_with("DROP PROCEDURE IF EXISTS `archive`;\n\nCREATE DEFINER"));
        let parts = split_statements(&script, Dialect::Mysql);
        assert_eq!(parts.len(), 2, "{parts:?}");
        assert_eq!(parts[0].sql, "DROP PROCEDURE IF EXISTS `archive`");
        assert_eq!(parts[1].sql, create);
        assert_eq!(
            mysql_show_create(RoutineKind::Function, Some("shop"), "f"),
            "SHOW CREATE FUNCTION `shop`.`f`"
        );

        let def = "CREATE OR REPLACE FUNCTION public.add(a integer, b integer)\n RETURNS integer\n LANGUAGE plpgsql\nAS $function$\nBEGIN\n  RETURN a + b;\nEND;\n$function$\n";
        let script = pg_routine_script(def);
        assert!(script.ends_with("$function$;\n"));
        assert_eq!(split_statements(&script, Dialect::Postgres).len(), 1);
        assert_eq!(pg_routine_script("SELECT 1;\n"), "SELECT 1;\n");
    }

    #[test]
    fn builds_aggregate_and_sequence_scripts() {
        let agg = AggregateInfo {
            schema: Some("public".into()),
            name: "total".into(),
            arguments: "numeric".into(),
            sfunc: "numeric_add".into(),
            stype: "numeric".into(),
            finalfunc: None,
            combinefunc: Some("numeric_add".into()),
            initcond: Some("0".into()),
            sortop: None,
        };
        assert_eq!(
            aggregate_ddl(&agg),
            "CREATE OR REPLACE AGGREGATE \"public\".\"total\"(numeric) (\n  SFUNC = numeric_add,\n  STYPE = numeric,\n  COMBINEFUNC = numeric_add,\n  INITCOND = '0'\n);\n"
        );

        let seq = SequenceInfo {
            schema: Some("public".into()),
            name: "users_id_seq".into(),
            data_type: Some("integer".into()),
            start: Some("1".into()),
            increment: Some("1".into()),
            min: Some("1".into()),
            max: Some("2147483647".into()),
            cache: Some("1".into()),
            cycle: false,
            owned_by: Some((Some("public".into()), "users".into(), "id".into())),
        };
        assert_eq!(
            sequence_ddl(&seq),
            "CREATE SEQUENCE \"public\".\"users_id_seq\"\n  AS integer\n  INCREMENT BY 1\n  MINVALUE 1\n  MAXVALUE 2147483647\n  START WITH 1\n  CACHE 1\n  NO CYCLE;\n\nALTER SEQUENCE \"public\".\"users_id_seq\" OWNED BY \"public\".\"users\".\"id\";\n"
        );
        let odd = SequenceInfo {
            name: "s".into(),
            data_type: Some("text; drop".into()),
            increment: Some("-1".into()),
            max: Some("1; DROP".into()),
            cycle: true,
            ..SequenceInfo::default()
        };
        assert_eq!(
            sequence_ddl(&odd),
            "CREATE SEQUENCE \"s\"\n  INCREMENT BY -1\n  CYCLE;\n"
        );
    }
}
