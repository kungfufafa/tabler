//! A view / function / procedure saved from its SQL tab (TablePlus: ⌘S in the tab of a routine
//! drops and recreates it; in the tab of a New ▸ View / Function / Procedure template it
//! creates the object). The definition is the user's own text, sent as ONE statement without
//! `DELIMITER`; the only SQL generated here is the `DROP … IF EXISTS` in front of it (and, on
//! MySQL, the schema in front of the name of a routine that replaces one: see `definition_plan`).

use crate::error::AppError;
use crate::sqlgen::objects::safe_argument_list;
use crate::sqlgen::quote::Dialect;
use crate::types::ddl::DdlPlan;
use crate::types::objects::{RoutineKind, RoutineRef};

fn routine_word(kind: RoutineKind, d: Dialect) -> &'static str {
    match kind {
        RoutineKind::Procedure => "PROCEDURE",
        RoutineKind::Aggregate if d == Dialect::Postgres => "AGGREGATE",
        _ => "FUNCTION",
    }
}

/// `DROP FUNCTION / PROCEDURE IF EXISTS`; PG addresses the overload by its identity arguments.
pub fn drop_routine_if_exists(d: Dialect, r: &RoutineRef) -> Result<String, AppError> {
    if d == Dialect::Sqlite {
        return Err(AppError::Unsupported(
            "SQLite has no stored routines".into(),
        ));
    }
    let name = d.qualify(r.schema.as_deref(), &r.name);
    let mut sql = format!("DROP {} IF EXISTS {name}", routine_word(r.kind, d));
    if d == Dialect::Postgres {
        if !safe_argument_list(&r.identity_arguments) {
            return Err(AppError::InvalidInput(format!(
                "unexpected argument list for {}",
                r.name
            )));
        }
        sql.push_str(&format!("({})", r.identity_arguments));
    }
    Ok(sql)
}

/// The first word of `sql` after whitespace and comments.
fn first_word(sql: &str) -> String {
    let mut rest = sql.trim_start();
    loop {
        if let Some(r) = rest.strip_prefix("--") {
            rest = r
                .split_once('\n')
                .map(|(_, r)| r)
                .unwrap_or("")
                .trim_start();
        } else if let Some(r) = rest.strip_prefix("/*") {
            rest = r
                .split_once("*/")
                .map(|(_, r)| r)
                .unwrap_or("")
                .trim_start();
        } else {
            break;
        }
    }
    rest.split(|c: char| !c.is_ascii_alphabetic())
        .next()
        .unwrap_or("")
        .to_ascii_uppercase()
}

/// The text as the one statement it runs as: trimmed, without its final `;`.
fn definition_text(sql: &str) -> Result<String, AppError> {
    let text = sql.trim();
    let text = text.strip_suffix(';').unwrap_or(text).trim_end();
    if text.is_empty() {
        return Err(AppError::InvalidInput("the definition is empty".into()));
    }
    Ok(text.to_string())
}

fn not_a_definition() -> AppError {
    AppError::InvalidInput(
        "a definition starts with CREATE (CREATE VIEW / FUNCTION / PROCEDURE …); saving drops the \
         old version for you"
            .into(),
    )
}

/// What the head of a `CREATE FUNCTION / PROCEDURE` statement names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutineHeader {
    pub kind: RoutineKind,
    pub or_replace: bool,
    pub schema: Option<String>,
    pub name: String,
    /// The argument list as `DROP` / the catalog's identity arguments spell it: modes and names
    /// kept, `DEFAULT …` removed (PostgreSQL); empty on MySQL, which drops by name.
    pub identity_arguments: String,
}

/// One token of the statement's head.
#[derive(Debug, PartialEq)]
enum Tok {
    /// A bare word, as typed.
    Word(String),
    /// A quoted identifier, unquoted.
    Quoted(String),
    /// A single-quoted string (a `DEFINER = 'user'@'host'` part): its text does not matter.
    Str,
    Punct(char),
}

/// An identifier can hold any non-ASCII character on both servers (MySQL: U+0080 and up).
fn word_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || !c.is_ascii()
}

fn word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$' || !c.is_ascii()
}

/// The quoted text opened at `open` (a `close` quote at that byte; a doubled one stands for
/// itself): its content and the byte after the closing quote (the end when unterminated).
fn quoted_at(sql: &str, open: usize, close: char) -> (String, usize) {
    let mut text = String::new();
    let mut chars = sql[open + close.len_utf8()..].char_indices().peekable();
    let base = open + close.len_utf8();
    while let Some((k, ch)) = chars.next() {
        if ch == close {
            if let Some(&(_, next)) = chars.peek()
                && next == close
            {
                text.push(close);
                chars.next();
                continue;
            }
            return (text, base + k + close.len_utf8());
        }
        text.push(ch);
    }
    (text, sql.len())
}

/// The first `limit` tokens with their byte spans. Walks characters, never bytes: the text is
/// whatever the user typed, and a position inside a multi-byte character is not a boundary.
fn tokens(sql: &str, limit: usize) -> Vec<(Tok, usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while out.len() < limit {
        let rest = &sql[i..];
        let Some(c) = rest.chars().next() else { break };
        if c.is_whitespace() {
            i += c.len_utf8();
        } else if rest.starts_with("--") {
            i = rest.find('\n').map_or(sql.len(), |n| i + n + 1);
        } else if let Some(after) = rest.strip_prefix("/*") {
            i = after.find("*/").map_or(sql.len(), |n| i + 2 + n + 2);
        } else if c == '"' || c == '`' {
            let (text, end) = quoted_at(sql, i, c);
            out.push((Tok::Quoted(text), i, end));
            i = end;
        } else if c == '\'' {
            let (_, end) = quoted_at(sql, i, c);
            out.push((Tok::Str, i, end));
            i = end;
        } else if word_start(c) {
            let end = rest
                .char_indices()
                .find(|&(_, ch)| !word_char(ch))
                .map_or(sql.len(), |(k, _)| i + k);
            out.push((Tok::Word(sql[i..end].to_string()), i, end));
            i = end;
        } else {
            out.push((Tok::Punct(c), i, i + c.len_utf8()));
            i += c.len_utf8();
        }
    }
    out
}

/// The text between the parenthesis opened at `open` (a `(` at that byte) and its match.
fn paren_group(sql: &str, open: usize) -> Option<&str> {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for (i, c) in sql[open..].char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' | '`' => quote = Some(c),
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&sql[open + 1..open + i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Splits at the commas outside parentheses and quotes.
fn split_top_level(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => quote = Some(c),
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// One PostgreSQL argument as the identity arguments spell it: `DEFAULT expr` / `= expr` cut off,
/// the default `IN` mode dropped, whitespace collapsed. None for an `OUT` argument (not part of a
/// function's identity).
fn identity_argument(arg: &str) -> Option<String> {
    let mut end = arg.len();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let lower = arg.to_ascii_lowercase();
    for (i, c) in arg.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => quote = Some(c),
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '=' if depth == 0 => {
                end = i;
                break;
            }
            'd' | 'D'
                if depth == 0
                    && lower[i..].starts_with("default")
                    && (i == 0 || arg.as_bytes()[i - 1].is_ascii_whitespace())
                    && arg[i + 7..]
                        .chars()
                        .next()
                        .is_none_or(|n| n.is_whitespace() || n == '(' || n == '\'') =>
            {
                end = i;
                break;
            }
            _ => {}
        }
    }
    let words: Vec<&str> = arg[..end].split_whitespace().collect();
    let first = words.first()?.to_ascii_uppercase();
    let words = match first.as_str() {
        "OUT" => return None,
        "IN" => &words[1..],
        _ => &words[..],
    };
    (!words.is_empty()).then(|| words.join(" "))
}

type Toks = [(Tok, usize, usize)];

fn word_at(toks: &Toks, i: usize) -> Option<String> {
    match toks.get(i) {
        Some((Tok::Word(w), ..)) => Some(w.to_ascii_uppercase()),
        _ => None,
    }
}

/// Where the name of a `CREATE [OR REPLACE] [DEFINER = …] FUNCTION | PROCEDURE [IF NOT EXISTS]
/// name` head starts: the routine kind, whether it said `OR REPLACE`, and the index of the first
/// name token. None for anything else (a view, an aggregate…).
fn routine_head(toks: &Toks) -> Option<(RoutineKind, bool, usize)> {
    if word_at(toks, 0).as_deref() != Some("CREATE") {
        return None;
    }
    let mut or_replace = false;
    let mut i = 1;
    let kind = loop {
        match word_at(toks, i).as_deref() {
            Some("FUNCTION") => break RoutineKind::Function,
            Some("PROCEDURE") => break RoutineKind::Procedure,
            Some("AGGREGATE") => return None,
            Some("OR") if word_at(toks, i + 1).as_deref() == Some("REPLACE") => {
                or_replace = true;
                i += 1;
            }
            // `DEFINER = user@host`, `OR REPLACE`'s second word, quoted users…
            _ if i < 14 => {}
            _ => return None,
        }
        i += 1;
    };
    i += 1;
    if word_at(toks, i).as_deref() == Some("IF")
        && word_at(toks, i + 1).as_deref() == Some("NOT")
        && word_at(toks, i + 2).as_deref() == Some("EXISTS")
    {
        i += 3;
    }
    Some((kind, or_replace, i))
}

/// A routine name at token `i`: `name` or `schema.name` (PostgreSQL folds unquoted words to
/// lower case). Returns the schema, the name and the index after it.
fn qualified_name(d: Dialect, toks: &Toks, i: usize) -> Option<(Option<String>, String, usize)> {
    let ident = |at: usize| match toks.get(at) {
        Some((Tok::Word(w), ..)) => Some(if d == Dialect::Postgres {
            w.to_ascii_lowercase()
        } else {
            w.clone()
        }),
        Some((Tok::Quoted(q), ..)) => Some(q.clone()),
        _ => None,
    };
    let first = ident(i)?;
    Some(match toks.get(i + 1) {
        Some((Tok::Punct('.'), ..)) => (Some(first), ident(i + 2)?, i + 3),
        _ => (None, first, i + 1),
    })
}

/// PostgreSQL identity arguments of an argument list as typed (see `identity_argument`); empty
/// elsewhere (MySQL drops by name).
fn identity_arguments_of(d: Dialect, args: &str) -> String {
    if d == Dialect::Postgres {
        split_top_level(args)
            .into_iter()
            .filter_map(identity_argument)
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        String::new()
    }
}

/// The head of a `CREATE [OR REPLACE] [DEFINER = …] FUNCTION | PROCEDURE [IF NOT EXISTS] name
/// (args)` statement; None for anything else (a view, an aggregate…).
pub fn parse_routine_header(d: Dialect, sql: &str) -> Option<RoutineHeader> {
    let toks = tokens(sql, 24);
    let (kind, or_replace, i) = routine_head(&toks)?;
    let (schema, name, next) = qualified_name(d, &toks, i)?;
    let args = match toks.get(next) {
        Some((Tok::Punct('('), at, _)) => paren_group(sql, *at)?,
        _ => return None,
    };
    Some(RoutineHeader {
        kind,
        or_replace,
        schema,
        name,
        identity_arguments: identity_arguments_of(d, args),
    })
}

/// A `DROP FUNCTION | PROCEDURE IF EXISTS name[(args)];` in front of a definition (what opening a
/// MySQL routine puts in its tab: `mysql_routine_script`), and the text after it. Only that exact
/// shape: PostgreSQL needs the argument list (a bare name could mean another overload), MySQL
/// takes none, and anything after the name but the `;` (`CASCADE`, a second name) is not one.
fn leading_drop(d: Dialect, text: &str) -> Option<(RoutineRef, &str)> {
    if d == Dialect::Sqlite {
        return None;
    }
    let toks = tokens(text, 8);
    if word_at(&toks, 0).as_deref() != Some("DROP")
        || word_at(&toks, 2).as_deref() != Some("IF")
        || word_at(&toks, 3).as_deref() != Some("EXISTS")
    {
        return None;
    }
    let kind = match word_at(&toks, 1).as_deref() {
        Some("FUNCTION") => RoutineKind::Function,
        Some("PROCEDURE") => RoutineKind::Procedure,
        _ => return None,
    };
    let (schema, name, next) = qualified_name(d, &toks, 4)?;
    let (args, end) = match toks.get(next) {
        Some((Tok::Punct('('), at, _)) if d == Dialect::Postgres => {
            let args = paren_group(text, *at)?;
            (args, at + args.len() + 2)
        }
        Some((_, at, _)) if d == Dialect::Mysql => ("", *at),
        _ => return None,
    };
    let rest = text[end..].trim_start().strip_prefix(';')?.trim_start();
    let routine = RoutineRef {
        schema,
        name,
        kind,
        identity_arguments: identity_arguments_of(d, args),
    };
    Some((routine, rest))
}

/// The `CREATE` a routine's opened script holds: its leading `DROP … IF EXISTS …;` (what
/// `mysql_routine_script` writes) taken off, the final `;` too. None for text that is not
/// `[DROP …;] CREATE …`.
pub fn create_statement(d: Dialect, script: &str) -> Option<String> {
    let text = definition_text(script).ok()?;
    let create = match leading_drop(d, &text) {
        Some((_, rest)) => rest,
        None => text.as_str(),
    };
    (first_word(create) == "CREATE").then(|| create.to_string())
}

/// Whether two references name the same routine: kind (procedure or not), name, schema when both
/// say one, and on PostgreSQL the overload.
fn same_routine(d: Dialect, a: &RoutineRef, b: &RoutineRef) -> bool {
    let norm = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    // MySQL routine names are not case-sensitive.
    let name = if d == Dialect::Mysql {
        a.name.eq_ignore_ascii_case(&b.name)
    } else {
        a.name == b.name
    };
    name && (a.kind == RoutineKind::Procedure) == (b.kind == RoutineKind::Procedure)
        && (a.schema.is_none() || b.schema.is_none() || a.schema == b.schema)
        && (d != Dialect::Postgres || norm(&a.identity_arguments) == norm(&b.identity_arguments))
}

/// `sql` (a `CREATE … FUNCTION | PROCEDURE name(…)`, as `SHOW CREATE` prints it) with its name
/// qualified by `schema`, so it is created where the old routine lived whatever database the
/// pooled connection happens to have selected. Unchanged when the name already has a schema or the
/// text is not a routine head.
pub fn qualify_routine(d: Dialect, sql: &str, schema: &str) -> String {
    let toks = tokens(sql, 24);
    let Some((_, _, i)) = routine_head(&toks) else {
        return sql.to_string();
    };
    match (toks.get(i), toks.get(i + 1)) {
        (Some((Tok::Word(_) | Tok::Quoted(_), at, _)), next)
            if !matches!(next, Some((Tok::Punct('.'), ..))) =>
        {
            format!("{}{}.{}", &sql[..*at], d.quote_ident(schema), &sql[*at..])
        }
        _ => sql.to_string(),
    }
}

/// `sql` without its `DEFINER = user@host` clause (the account that creates it owns the routine).
/// None when the text names no definer. For re-creating a routine whose definer the connected
/// account may not assume (it needs `SET_USER_ID` / `SUPER`).
pub fn without_definer(sql: &str) -> Option<String> {
    let toks = tokens(sql, 24);
    let at = toks
        .iter()
        .take(6)
        .position(|(t, ..)| matches!(t, Tok::Word(w) if w.eq_ignore_ascii_case("DEFINER")))?;
    if !matches!(toks.get(at + 1), Some((Tok::Punct('='), ..))) {
        return None;
    }
    let account = |i: usize| {
        matches!(
            toks.get(i),
            Some((Tok::Word(_) | Tok::Quoted(_) | Tok::Str, ..))
        )
    };
    let mut last = at + 2;
    if word_at(&toks, last).as_deref() == Some("CURRENT_USER") {
        if matches!(toks.get(last + 1), Some((Tok::Punct('('), ..)))
            && matches!(toks.get(last + 2), Some((Tok::Punct(')'), ..)))
        {
            last += 2;
        }
    } else if account(last)
        && matches!(toks.get(last + 1), Some((Tok::Punct('@'), ..)))
        && account(last + 2)
    {
        last += 2;
    } else {
        return None;
    }
    let (start, end) = (toks[at].1, toks[last].2);
    Some(format!("{}{}", &sql[..start], sql[end..].trim_start()))
}

impl RoutineHeader {
    /// The routine this statement creates, named for a later `DROP` (`default_schema` when the
    /// statement does not qualify its name).
    pub fn routine(&self, default_schema: Option<&str>) -> RoutineRef {
        RoutineRef {
            schema: self.schema.clone().or(default_schema.map(str::to_string)),
            name: self.name.clone(),
            kind: self.kind,
            identity_arguments: self.identity_arguments.clone(),
        }
    }

    /// Whether `CREATE OR REPLACE` of this statement replaces `r` in place: same name (and schema,
    /// when written), same kind and the same identity arguments (names, modes and types).
    pub fn replaces(&self, r: &RoutineRef) -> bool {
        let kind = |k: RoutineKind| k == RoutineKind::Procedure;
        let norm = |s: &str| {
            s.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        self.or_replace
            && self.name == r.name
            && self
                .schema
                .as_ref()
                .is_none_or(|s| Some(s) == r.schema.as_ref())
            && kind(self.kind) == kind(r.kind)
            && norm(&self.identity_arguments) == norm(&r.identity_arguments)
    }
}

/// What saving a definition runs.
#[derive(Debug, Clone, PartialEq)]
pub struct DefinitionPlan {
    pub plan: DdlPlan,
    /// The routine the text creates, so the tab can keep saving it.
    pub created: Option<RoutineRef>,
    /// The routine the plan drops first (None when it replaces in place or drops nothing). MySQL
    /// cannot replace one in a single statement: a failing CREATE after this DROP is undone by
    /// re-creating it (`commands::workspace_commit`).
    pub dropped: Option<RoutineRef>,
}

/// Shown in the Preview SQL sheet of a MySQL replacement.
pub const MYSQL_REPLACE_WARNING: &str = "MySQL cannot replace a routine in one step: the old \
    version is dropped, then the new one created. If the new definition fails, the previous \
    version is created again from the definition read just before.";

/// A definition's statements: `DROP … IF EXISTS` of `drop` (when given), then the definition.
/// A `DROP … IF EXISTS` of the routine in front of the text itself (what opening a MySQL routine
/// shows) is the same drop: it is taken out, never run twice; in a tab that has no `drop` of its
/// own it becomes the drop, and one naming another routine than the tab's is refused.
/// PostgreSQL skips the drop when the text is a `CREATE OR REPLACE` of the same routine and
/// signature (replacing in place keeps the views and triggers that use it); a changed signature
/// drops the old routine first. MySQL always drops and recreates. PG and SQLite run both in one
/// transaction; MySQL commits each DDL statement on its own. Also returns the routine the text
/// creates, so the tab can keep saving it.
pub fn definition_plan(
    d: Dialect,
    drop: Option<&RoutineRef>,
    sql: &str,
) -> Result<DefinitionPlan, AppError> {
    let text = definition_text(sql)?;
    let (typed, definition) = match leading_drop(d, &text) {
        Some((routine, rest)) => (Some(routine), rest),
        None => (None, text.as_str()),
    };
    if first_word(definition) != "CREATE" {
        return Err(not_a_definition());
    }
    let drop = match (drop, typed) {
        (Some(tab), Some(typed)) if !same_routine(d, tab, &typed) => {
            return Err(AppError::InvalidInput(format!(
                "the DROP in front of the definition names {}, but this tab replaces {}: remove \
                 that line (saving drops the old version itself)",
                typed.name, tab.name
            )));
        }
        (Some(tab), _) => Some(tab.clone()),
        (None, typed) => typed,
    };
    let header = parse_routine_header(d, definition);
    let mut statements = Vec::with_capacity(2);
    let mut dropped = None;
    if let Some(r) = &drop {
        let in_place = d == Dialect::Postgres && header.as_ref().is_some_and(|h| h.replaces(r));
        if !in_place {
            statements.push(drop_routine_if_exists(d, r)?);
            dropped = Some(r.clone());
        }
    }
    // MySQL creates an unqualified name in the connection's current database: another one than
    // the routine's (or none: "No database selected", after the DROP already ran). The routine a
    // tab replaces lives in `drop.schema`, so its new text is created there too.
    let qualified = match (d, &drop, &header) {
        (Dialect::Mysql, Some(r), Some(h)) if h.schema.is_none() => r
            .schema
            .as_deref()
            .map(|schema| qualify_routine(d, definition, schema)),
        _ => None,
    };
    statements.push(qualified.as_deref().unwrap_or(definition).to_string());
    let created = header.map(|h| h.routine(drop.as_ref().and_then(|r| r.schema.as_deref())));
    let warnings = match (&dropped, d) {
        (Some(_), Dialect::Mysql) => vec![MYSQL_REPLACE_WARNING.to_string()],
        _ => Vec::new(),
    };
    Ok(DefinitionPlan {
        plan: DdlPlan {
            statements,
            warnings,
            transactional: d != Dialect::Mysql,
        },
        created,
        dropped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routine(kind: RoutineKind, args: &str) -> RoutineRef {
        RoutineRef {
            schema: Some("public".into()),
            name: "add".into(),
            kind,
            identity_arguments: args.into(),
        }
    }

    fn plan(d: Dialect, drop: Option<&RoutineRef>, sql: &str) -> DdlPlan {
        definition_plan(d, drop, sql).unwrap().plan
    }

    #[test]
    fn a_changed_signature_drops_then_creates() {
        let p = plan(
            Dialect::Postgres,
            Some(&routine(RoutineKind::Function, "a integer, b integer")),
            "  CREATE OR REPLACE FUNCTION public.add(a int, b int, c int) RETURNS int AS $$ SELECT a + b; $$ LANGUAGE sql;\n",
        );
        assert_eq!(
            p.statements,
            [
                "DROP FUNCTION IF EXISTS \"public\".\"add\"(a integer, b integer)",
                "CREATE OR REPLACE FUNCTION public.add(a int, b int, c int) RETURNS int AS $$ SELECT a + b; $$ LANGUAGE sql",
            ]
        );
        assert!(p.transactional);
    }

    #[test]
    fn the_same_signature_is_replaced_in_place_on_postgres() {
        let drop = routine(RoutineKind::Function, "a integer, b integer");
        let DefinitionPlan {
            plan: p, created, ..
        } = definition_plan(
            Dialect::Postgres,
            Some(&drop),
            "CREATE OR REPLACE FUNCTION add(a integer, b integer DEFAULT 2) RETURNS int AS $$ SELECT a + b $$ LANGUAGE sql",
        )
        .unwrap();
        // No DROP: views and triggers using the function stay valid.
        assert_eq!(p.statements.len(), 1);
        assert!(p.statements[0].starts_with("CREATE OR REPLACE FUNCTION add("));
        // The tab keeps saving the routine as the text names it (defaults are not identity).
        assert_eq!(
            created,
            Some(routine(RoutineKind::Function, "a integer, b integer"))
        );
        // A plain CREATE needs the old one gone first.
        let p = plan(
            Dialect::Postgres,
            Some(&drop),
            "CREATE FUNCTION add(a integer, b integer) RETURNS int AS 'select 1' LANGUAGE sql",
        );
        assert_eq!(p.statements.len(), 2);
        // Another kind or another name is another routine.
        let p = plan(
            Dialect::Postgres,
            Some(&drop),
            "CREATE OR REPLACE PROCEDURE add(a integer, b integer) AS 'select 1' LANGUAGE sql",
        );
        assert_eq!(p.statements.len(), 2);
        let p = plan(
            Dialect::Postgres,
            Some(&drop),
            "CREATE OR REPLACE FUNCTION plus(a integer, b integer) RETURNS int AS 'select 1' LANGUAGE sql",
        );
        assert_eq!(p.statements.len(), 2);
    }

    #[test]
    fn mysql_always_drops_and_recreates() {
        let DefinitionPlan {
            plan: p, created, ..
        } = definition_plan(
            Dialect::Mysql,
            Some(&routine(RoutineKind::Procedure, "ignored")),
            "CREATE DEFINER=`root`@`localhost` PROCEDURE add()\nBEGIN\n  SELECT 1;\nEND;",
        )
        .unwrap();
        assert_eq!(
            p.statements,
            [
                "DROP PROCEDURE IF EXISTS `public`.`add`",
                // Created where the old one lived, whatever database the connection has selected.
                "CREATE DEFINER=`root`@`localhost` PROCEDURE `public`.add()\nBEGIN\n  SELECT 1;\nEND",
            ]
        );
        assert!(!p.transactional);
        assert_eq!(created.unwrap().name, "add");
    }

    #[test]
    fn creates_without_a_drop_and_refuses_anything_else() {
        let p = plan(
            Dialect::Sqlite,
            None,
            "-- a view\n/* note */ CREATE VIEW v AS SELECT 1",
        );
        assert_eq!(p.statements.len(), 1);
        let created = definition_plan(Dialect::Sqlite, None, "CREATE VIEW v AS SELECT 1")
            .unwrap()
            .created;
        assert_eq!(created, None);
        assert!(definition_plan(Dialect::Postgres, None, "DROP TABLE users").is_err());
        assert!(definition_plan(Dialect::Postgres, None, " ;").is_err());
        assert!(
            definition_plan(
                Dialect::Postgres,
                Some(&routine(RoutineKind::Function, "a int); DROP TABLE x; --")),
                "CREATE FUNCTION f() RETURNS int AS 'select 1' LANGUAGE sql",
            )
            .is_err()
        );
        assert!(
            drop_routine_if_exists(Dialect::Sqlite, &routine(RoutineKind::Function, "")).is_err()
        );
    }

    #[test]
    fn reads_the_head_of_a_routine() {
        let h = parse_routine_header(
            Dialect::Postgres,
            "CREATE OR REPLACE FUNCTION \"Sales\".Total_Of(IN price numeric(10,2) DEFAULT 1, OUT n int, VARIADIC xs int[], tag text = 'a,b') RETURNS int AS $$ SELECT 1 $$",
        )
        .unwrap();
        assert_eq!(h.kind, RoutineKind::Function);
        assert!(h.or_replace);
        assert_eq!(h.schema.as_deref(), Some("Sales"));
        // Unquoted names fold to lower case on PostgreSQL.
        assert_eq!(h.name, "total_of");
        assert_eq!(
            h.identity_arguments,
            "price numeric(10,2), VARIADIC xs int[], tag text"
        );
        let h = parse_routine_header(
            Dialect::Mysql,
            "create procedure if not exists `Do It`(in a int, out b int) begin end",
        )
        .unwrap();
        assert_eq!(h.kind, RoutineKind::Procedure);
        assert_eq!(h.name, "Do It");
        assert_eq!(h.identity_arguments, "");
        for not_one in [
            "CREATE VIEW v AS SELECT 1",
            "CREATE AGGREGATE a(int) (SFUNC = f)",
            "SELECT 1",
            "CREATE FUNCTION f",
        ] {
            assert!(
                parse_routine_header(Dialect::Postgres, not_one).is_none(),
                "{not_one}"
            );
        }
    }

    // --- #47: text with characters outside ASCII ---------------------------------------------

    /// Texts the old byte-walking tokenizer panicked on (a one-byte step landed inside a
    /// character, and the next `sql[i..]` was not on a char boundary).
    const NON_ASCII: &[&str] = &[
        "CREATE VIEW v AS SELECT id FROM t WHERE city = 'São Paulo'",
        "CREATE VIEW v AS SELECT 'é' AS x",
        "CREATE FUNCTION naïve() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql",
        "CREATE PROCEDURE prüfen() BEGIN SELECT 'Zażółć gęślą jaźń'; END",
        "CREATE OR REPLACE FUNCTION public.größe(a integer) RETURNS int AS 'select 1' LANGUAGE sql",
        "CREATE VIEW v AS SELECT '日本語' AS 名前",
        "CREATE VIEW v AS SELECT '😀' AS smile",
        "-- Zażółć gęślą\nCREATE VIEW v AS SELECT 1",
        "/* 日本語 */ CREATE VIEW v AS SELECT 1",
        "CREATE VIEW v AS SELECT 1 -- ünïcödé",
        "CREATE VIEW `café` AS SELECT 1",
        "CREATE VIEW \"café\" AS SELECT 1",
        // A no-break space and a NEL: not whitespace to the servers, whitespace to `char`.
        "CREATE\u{a0}VIEW v AS SELECT 1",
        "CREATE\u{85}VIEW v AS SELECT 1",
    ];

    #[test]
    fn text_outside_ascii_is_planned_instead_of_panicking() {
        for d in [Dialect::Postgres, Dialect::Mysql, Dialect::Sqlite] {
            for sql in NON_ASCII {
                // Whatever the verdict, planning returns it.
                let _ = definition_plan(d, None, sql);
                let _ = parse_routine_header(d, sql);
                let _ = create_statement(d, sql);
                let _ = without_definer(sql);
                let _ = qualify_routine(d, sql, "s");
                // Every prefix ends inside some character or at a boundary: both are fine.
                for (i, _) in sql.char_indices() {
                    let _ = definition_plan(d, None, &sql[..i]);
                    let _ = tokens(&sql[..i], 24);
                }
            }
        }
        // Every character width (1-4 bytes) at every position of a routine head.
        let base =
            "CREATE DEFINER = 'u'@'h' FUNCTION IF NOT EXISTS s.f(a int, \"b c\" text) RETURNS int";
        for ch in ["é", "日", "😀", "\u{a0}", "\u{85}", "\u{2028}"] {
            for (i, _) in base.char_indices() {
                let mut sql = base.to_string();
                sql.insert_str(i, ch);
                for d in [Dialect::Postgres, Dialect::Mysql] {
                    let _ = definition_plan(d, None, &sql);
                    let _ = parse_routine_header(d, &sql);
                    let _ = leading_drop(d, &sql);
                }
            }
        }
    }

    #[test]
    fn non_ascii_views_and_routines_plan_and_are_named_right() {
        // The reported views: planned as written, no routine behind them.
        for sql in [
            "CREATE VIEW v AS SELECT id FROM t WHERE city = 'São Paulo'",
            "CREATE VIEW v AS SELECT 'é' AS x",
        ] {
            let p = definition_plan(Dialect::Mysql, None, sql).unwrap();
            assert_eq!(p.plan.statements, [sql]);
            assert_eq!(p.created, None);
        }
        // A non-ASCII name is an identifier on both servers (PG folds only ASCII letters).
        let p = definition_plan(
            Dialect::Postgres,
            None,
            "CREATE FUNCTION naïve() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql;",
        )
        .unwrap();
        assert_eq!(p.created.unwrap().name, "naïve");
        let p = definition_plan(
            Dialect::Postgres,
            None,
            "CREATE OR REPLACE FUNCTION Größe(Wert integer) RETURNS int AS 'select 1' LANGUAGE sql",
        )
        .unwrap();
        let created = p.created.unwrap();
        // Only the ASCII letter folds, as the server does.
        assert_eq!(created.name, "größe");
        assert_eq!(created.identity_arguments, "Wert integer");
        let p = definition_plan(
            Dialect::Mysql,
            None,
            "CREATE DEFINER = 'rené'@'%' PROCEDURE prüfen(IN größe int) BEGIN SELECT 'Zażółć'; END",
        )
        .unwrap();
        let created = p.created.unwrap();
        assert_eq!(created.name, "prüfen");
        assert_eq!(created.kind, RoutineKind::Procedure);
        // The comment and the quoted identifier keep their text.
        let h = parse_routine_header(
            Dialect::Mysql,
            "-- Zażółć\n/* 日本語 */ CREATE PROCEDURE `日本語`.`prüfen`() BEGIN END",
        )
        .unwrap();
        assert_eq!(h.schema.as_deref(), Some("日本語"));
        assert_eq!(h.name, "prüfen");
        // A quote inside a string no longer swallows what follows, and a non-ASCII definer is
        // one token.
        let toks = tokens("CREATE DEFINER='rené'@'h' PROCEDURE p()", 24);
        assert!(toks.iter().any(|(t, ..)| *t == Tok::Str));
        for (_, from, to) in &toks {
            assert!("CREATE DEFINER='rené'@'h' PROCEDURE p()".is_char_boundary(*from));
            assert!("CREATE DEFINER='rené'@'h' PROCEDURE p()".is_char_boundary(*to));
        }
    }

    // --- #18: the text a routine opens as must save ------------------------------------------

    fn mysql_routine(kind: RoutineKind) -> RoutineRef {
        RoutineRef {
            schema: Some("shop".into()),
            name: "archive".into(),
            kind,
            identity_arguments: String::new(),
        }
    }

    /// What the plan runs for `create` (a name without a schema) in the tab of a routine of `shop`.
    fn in_shop(create: &str) -> String {
        create.replacen("PROCEDURE `archive`", "PROCEDURE `shop`.`archive`", 1)
    }

    fn in_shop_function(create: &str) -> String {
        create.replacen("FUNCTION `archive`", "FUNCTION `shop`.`archive`", 1)
    }

    const MYSQL_PROCEDURE: &str = "CREATE DEFINER=`root`@`localhost` PROCEDURE `archive`(IN days INT)\nBEGIN\n  DELETE FROM posts WHERE created_at < NOW() - INTERVAL days DAY;\n  IF days > 30 THEN SELECT 'old'; END IF;\nEND";

    #[test]
    fn an_opened_mysql_routine_saves() {
        use crate::sqlgen::objects::mysql_routine_script;
        let tab = mysql_routine(RoutineKind::Procedure);
        // What opening a procedure puts in its tab: DROP … IF EXISTS, then the server's CREATE.
        let opened = mysql_routine_script(RoutineKind::Procedure, "archive", MYSQL_PROCEDURE);
        assert!(opened.starts_with("DROP PROCEDURE IF EXISTS `archive`;"));
        let p = definition_plan(Dialect::Mysql, Some(&tab), &opened).unwrap();
        // One DROP (the plan's own, qualified), then the CREATE: the DROP of the text is not run.
        assert_eq!(
            p.plan.statements,
            [
                "DROP PROCEDURE IF EXISTS `shop`.`archive`".to_string(),
                in_shop(MYSQL_PROCEDURE),
            ]
        );
        assert_eq!(p.dropped, Some(tab.clone()));
        assert_eq!(p.created, Some(tab));
        // Edited (the body changed) and with a trailing blank line: still saves.
        let edited = opened.replace("days > 30", "days > 60") + "\n\n";
        let p = definition_plan(
            Dialect::Mysql,
            Some(&mysql_routine(RoutineKind::Procedure)),
            &edited,
        )
        .unwrap();
        assert!(p.plan.statements[1].contains("days > 60"));
        // A function opens the same way.
        let create = "CREATE DEFINER=`root`@`localhost` FUNCTION `archive`(a INT) RETURNS int DETERMINISTIC RETURN a + 1";
        let opened = mysql_routine_script(RoutineKind::Function, "archive", create);
        let p = definition_plan(
            Dialect::Mysql,
            Some(&mysql_routine(RoutineKind::Function)),
            &opened,
        )
        .unwrap();
        assert_eq!(
            p.plan.statements[0],
            "DROP FUNCTION IF EXISTS `shop`.`archive`"
        );
        assert_eq!(p.plan.statements[1], in_shop_function(create));
        // The user deleted the DROP line by hand: same plan.
        let by_hand = opened.split_once("\n\n").unwrap().1;
        let q = definition_plan(
            Dialect::Mysql,
            Some(&mysql_routine(RoutineKind::Function)),
            by_hand,
        )
        .unwrap();
        assert_eq!(q.plan, p.plan);
    }

    #[test]
    fn an_opened_postgres_routine_saves() {
        use crate::sqlgen::objects::pg_routine_script;
        let def = "CREATE OR REPLACE FUNCTION public.add(a integer, b integer)\n RETURNS integer\n LANGUAGE plpgsql\nAS $function$\nBEGIN\n  RETURN a + b;\nEND;\n$function$\n";
        let opened = pg_routine_script(def);
        let tab = routine(RoutineKind::Function, "a integer, b integer");
        let p = definition_plan(Dialect::Postgres, Some(&tab), &opened).unwrap();
        // The same signature: replaced in place, nothing dropped.
        assert_eq!(p.plan.statements.len(), 1);
        assert_eq!(p.dropped, None);
        assert!(p.plan.warnings.is_empty());
        assert!(p.plan.statements[0].ends_with("$function$"));
        assert_eq!(p.created, Some(tab));
    }

    #[test]
    fn a_leading_drop_is_the_tabs_drop() {
        let tab = mysql_routine(RoutineKind::Procedure);
        let create = "CREATE PROCEDURE archive() SELECT 1";
        // Any spelling of the same routine (case, quoting, schema) is the tab's own drop.
        for drop in [
            "DROP PROCEDURE IF EXISTS archive;",
            "drop procedure if exists `ARCHIVE` ;",
            "DROP PROCEDURE IF EXISTS `shop`.`archive`;",
            "-- note\nDROP PROCEDURE IF EXISTS archive ;",
        ] {
            let p = definition_plan(Dialect::Mysql, Some(&tab), &format!("{drop}\n\n{create};"))
                .unwrap();
            assert_eq!(
                p.plan.statements,
                [
                    "DROP PROCEDURE IF EXISTS `shop`.`archive`",
                    "CREATE PROCEDURE `shop`.archive() SELECT 1"
                ],
                "{drop}"
            );
        }
        // A tab without a routine of its own (New ▸ Procedure) takes the typed DROP as its drop.
        let p = definition_plan(
            Dialect::Mysql,
            None,
            &format!("DROP PROCEDURE IF EXISTS archive;\n{create}"),
        )
        .unwrap();
        assert_eq!(
            p.plan.statements,
            ["DROP PROCEDURE IF EXISTS `archive`", create]
        );
        assert_eq!(p.dropped.unwrap().name, "archive");
        // PostgreSQL: with its argument list (a bare name could be another overload).
        let p = definition_plan(
            Dialect::Postgres,
            None,
            "DROP FUNCTION IF EXISTS public.f(a integer, b text DEFAULT 'x');\nCREATE FUNCTION public.f(a integer, b text) RETURNS int AS 'select 1' LANGUAGE sql",
        )
        .unwrap();
        assert_eq!(
            p.plan.statements[0],
            "DROP FUNCTION IF EXISTS \"public\".\"f\"(a integer, b text)"
        );
        assert!(p.plan.transactional);
        for not_a_drop in [
            // Another routine than the tab's: refused rather than dropped behind its back.
            "DROP PROCEDURE IF EXISTS other;\nCREATE PROCEDURE archive() SELECT 1",
            "DROP FUNCTION IF EXISTS archive;\nCREATE PROCEDURE archive() SELECT 1",
            // Not the exact shape.
            "DROP PROCEDURE archive;\nCREATE PROCEDURE archive() SELECT 1",
            "DROP PROCEDURE IF EXISTS archive, other;\nCREATE PROCEDURE archive() SELECT 1",
            "DROP PROCEDURE IF EXISTS archive();\nCREATE PROCEDURE archive() SELECT 1",
            "DROP TABLE archive;\nCREATE PROCEDURE archive() SELECT 1",
            "DROP PROCEDURE IF EXISTS archive;",
            "DROP PROCEDURE IF EXISTS archive; DROP TABLE users",
        ] {
            assert!(
                definition_plan(Dialect::Mysql, Some(&tab), not_a_drop).is_err(),
                "{not_a_drop}"
            );
        }
        assert!(
            definition_plan(
                Dialect::Postgres,
                None,
                "DROP FUNCTION IF EXISTS f; CREATE FUNCTION f() RETURNS int AS 'select 1' LANGUAGE sql",
            )
            .is_err()
        );
        assert!(
            definition_plan(
                Dialect::Postgres,
                None,
                "DROP FUNCTION IF EXISTS f(int) CASCADE; CREATE FUNCTION f(a int) RETURNS int AS 'select 1' LANGUAGE sql",
            )
            .is_err()
        );
        // SQLite has no routines.
        assert!(
            definition_plan(
                Dialect::Sqlite,
                None,
                "DROP FUNCTION IF EXISTS f; CREATE VIEW v AS SELECT 1"
            )
            .is_err()
        );
    }

    #[test]
    fn creates_the_create_an_opened_script_holds() {
        use crate::sqlgen::objects::mysql_routine_script;
        let script = mysql_routine_script(RoutineKind::Procedure, "archive", MYSQL_PROCEDURE);
        assert_eq!(
            create_statement(Dialect::Mysql, &script).as_deref(),
            Some(MYSQL_PROCEDURE)
        );
        assert_eq!(
            create_statement(Dialect::Mysql, "CREATE PROCEDURE p() SELECT 1;\n").as_deref(),
            Some("CREATE PROCEDURE p() SELECT 1")
        );
        assert_eq!(create_statement(Dialect::Mysql, "DROP TABLE t"), None);
        assert_eq!(create_statement(Dialect::Mysql, "  "), None);
    }

    // --- #19: what a MySQL replacement needs to put the old routine back ---------------------

    #[test]
    fn a_mysql_replacement_names_the_routine_it_drops_and_warns() {
        let tab = mysql_routine(RoutineKind::Procedure);
        let p = definition_plan(Dialect::Mysql, Some(&tab), MYSQL_PROCEDURE).unwrap();
        assert_eq!(p.dropped, Some(tab));
        assert!(!p.plan.transactional);
        assert_eq!(p.plan.warnings, [MYSQL_REPLACE_WARNING]);
        // Nothing to put back when nothing is dropped.
        let p = definition_plan(Dialect::Mysql, None, MYSQL_PROCEDURE).unwrap();
        assert_eq!(p.dropped, None);
        assert!(p.plan.warnings.is_empty());
        // PostgreSQL rolls the DROP back itself: a plan with a DROP names it but does not warn.
        let tab = routine(RoutineKind::Function, "a integer");
        let p = definition_plan(
            Dialect::Postgres,
            Some(&tab),
            "CREATE FUNCTION add(a integer, b integer) RETURNS int AS 'select 1' LANGUAGE sql",
        )
        .unwrap();
        assert_eq!(p.dropped, Some(tab));
        assert!(p.plan.transactional && p.plan.warnings.is_empty());
    }

    #[test]
    fn a_mysql_replacement_is_created_in_the_schema_of_the_routine_it_replaces() {
        // An unqualified name would land in the connection's current database: another one, or
        // none ("No database selected", after the DROP already ran).
        let tab = mysql_routine(RoutineKind::Procedure);
        let p = definition_plan(
            Dialect::Mysql,
            Some(&tab),
            "CREATE PROCEDURE archive() SELECT 1",
        )
        .unwrap();
        assert_eq!(
            p.plan.statements,
            [
                "DROP PROCEDURE IF EXISTS `shop`.`archive`",
                "CREATE PROCEDURE `shop`.archive() SELECT 1"
            ]
        );
        let p = definition_plan(
            Dialect::Mysql,
            Some(&tab),
            "CREATE DEFINER=`root`@`%` PROCEDURE IF NOT EXISTS `archive`() SELECT 1",
        )
        .unwrap();
        assert_eq!(
            p.plan.statements[1],
            "CREATE DEFINER=`root`@`%` PROCEDURE IF NOT EXISTS `shop`.`archive`() SELECT 1"
        );
        // The routine the text creates is the one in that schema (what the tab saves next).
        assert_eq!(p.created.unwrap().schema.as_deref(), Some("shop"));
        // A schema the text names itself is the user's choice; no tab routine, nothing to follow.
        let p = definition_plan(
            Dialect::Mysql,
            Some(&tab),
            "CREATE PROCEDURE other.archive() SELECT 1",
        )
        .unwrap();
        assert_eq!(
            p.plan.statements[1],
            "CREATE PROCEDURE other.archive() SELECT 1"
        );
        let p =
            definition_plan(Dialect::Mysql, None, "CREATE PROCEDURE archive() SELECT 1").unwrap();
        assert_eq!(p.plan.statements, ["CREATE PROCEDURE archive() SELECT 1"]);
        // Not a routine (a view): untouched.
        let p = definition_plan(Dialect::Mysql, Some(&tab), "CREATE VIEW v AS SELECT 1").unwrap();
        assert_eq!(p.plan.statements[1], "CREATE VIEW v AS SELECT 1");
        // PostgreSQL texts are qualified by what opens them and run in one transaction.
        let p = definition_plan(
            Dialect::Postgres,
            Some(&routine(RoutineKind::Function, "a integer")),
            "CREATE FUNCTION add(a integer, b integer) RETURNS int AS 'select 1' LANGUAGE sql",
        )
        .unwrap();
        assert!(p.plan.statements[1].starts_with("CREATE FUNCTION add("));
    }

    #[test]
    fn the_old_routine_is_created_again_where_it_lived() {
        let create = "CREATE DEFINER=`root`@`localhost` PROCEDURE `archive`(IN days INT)\nBEGIN\n  SELECT 1;\nEND";
        assert_eq!(
            qualify_routine(Dialect::Mysql, create, "shop"),
            "CREATE DEFINER=`root`@`localhost` PROCEDURE `shop`.`archive`(IN days INT)\nBEGIN\n  SELECT 1;\nEND"
        );
        // An unquoted name, OR REPLACE, IF NOT EXISTS; a quote inside the schema is doubled.
        assert_eq!(
            qualify_routine(
                Dialect::Mysql,
                "CREATE FUNCTION IF NOT EXISTS f(a int) RETURNS int RETURN a",
                "we`ird"
            ),
            "CREATE FUNCTION IF NOT EXISTS `we``ird`.f(a int) RETURNS int RETURN a"
        );
        // Already qualified, or not a routine: unchanged.
        let qualified = "CREATE PROCEDURE `x`.`p`() SELECT 1";
        assert_eq!(
            qualify_routine(Dialect::Mysql, qualified, "shop"),
            qualified
        );
        assert_eq!(
            qualify_routine(Dialect::Mysql, "CREATE VIEW v AS SELECT 1", "shop"),
            "CREATE VIEW v AS SELECT 1"
        );
    }

    #[test]
    fn a_definer_can_be_taken_off() {
        assert_eq!(
            without_definer("CREATE DEFINER=`rv`@`%` PROCEDURE `p`() SELECT 1").as_deref(),
            Some("CREATE PROCEDURE `p`() SELECT 1")
        );
        assert_eq!(
            without_definer("CREATE DEFINER = 'rv'@'localhost' FUNCTION f() RETURNS int RETURN 1")
                .as_deref(),
            Some("CREATE FUNCTION f() RETURNS int RETURN 1")
        );
        assert_eq!(
            without_definer("CREATE DEFINER=CURRENT_USER() PROCEDURE p() SELECT 1").as_deref(),
            Some("CREATE PROCEDURE p() SELECT 1")
        );
        assert_eq!(
            without_definer("CREATE DEFINER=CURRENT_USER PROCEDURE p() SELECT 1").as_deref(),
            Some("CREATE PROCEDURE p() SELECT 1")
        );
        assert_eq!(
            without_definer("CREATE DEFINER=rv@host PROCEDURE p() SELECT 1").as_deref(),
            Some("CREATE PROCEDURE p() SELECT 1")
        );
        // A definer named like a keyword, or no definer: nothing to take off.
        assert_eq!(without_definer("CREATE PROCEDURE p() SELECT 1"), None);
        assert_eq!(
            without_definer("CREATE DEFINER PROCEDURE p() SELECT 1"),
            None
        );
    }
}
