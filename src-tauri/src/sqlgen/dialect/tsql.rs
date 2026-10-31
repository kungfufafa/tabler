//! Transact-SQL (the sqlserver lane). Built with `drv-sqlserver` only.
//!
//! `[name]` with `]` doubled, `N'…'` text, `@P1` parameters (sp_executesql, at most 2,100 per
//! request), OFFSET/FETCH paging and TOP, OUTPUT INSERTED for written rows, `0x…` bytes. Batches
//! end at `GO` lines and statements need no `;`, so the classifier treats a write keyword
//! anywhere as a write (`SELECT 1 DELETE FROM t`).
//!
//! Columns reach the grid in their own type (the driver decodes tiberius' values) except where
//! that loses data: money arrives as a float, so it comes as text with its four decimals; CLR
//! types (hierarchyid, spatial) arrive as their serialization, so they come as `ToString()`; the
//! SQL Server 2025 `json` and `vector` types come as text. Pattern filters compare a column's
//! text under a binary collation, so the plain variants are case-sensitive and the `…Case` ones
//! (`LOWER(…) LIKE LOWER(…)`) are not, whatever the column's collation.
//!
//! The statements the driver sends for its own work (session setup, the editor's probe, `KILL`,
//! showplan, Show DDL) are written here too, so its SQL text comes from sqlgen like every other.

use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{
    ClassifyRules, LexProfile, PLAIN, Scan, Token, TokenKind, is_ident_start, scan,
};
use crate::sqlgen::dialect::spec::{
    DialectSpec, LimitOne, NullSafeEq, NullsSort, Paging, PlaceholderStyle, RedactRule, Returning,
    SqlHooks, SqlKnobs, bracketed, explain_unsupported, hex_0x, national_quoted, ph_at_p,
};
use crate::sqlgen::portable::classify;
use crate::types::mutate::{Clock, ServerExpr};
use crate::types::query::ExplainMode;
use crate::types::schema::{ColumnMeta, IndexMeta, TableMeta, ValueKind};

pub static LEX: LexProfile = LexProfile {
    ident_quotes: &[('[', ']'), ('"', '"')],
    string_quotes: &['\''],
    readings: &[PLAIN],
    e_strings: false,
    dollar_quotes: false,
    dollar_tags: false,
    triple_quotes: false,
    raw_prefixes: false,
    byte_prefixes: false,
    hash_comments: false,
    slash_slash_comments: false,
    nested_block_comments: true,
    dash_comment_needs_space: false,
    batch_separator: Some("GO"),
    terminator_optional: true,
    begin_end_bodies: true,
    attribute_calls: false,
};

pub static RULES: ClassifyRules = ClassifyRules {
    queries: &["SELECT", "WITH"],
    inspections: &["USE"],
    // Every statement keyword that is not a read: without terminators, any of them may start a
    // second statement. FETCH and END stay out, since reads use them too (OFFSET … FETCH NEXT,
    // CASE … END); CONVERSATION stands for END CONVERSATION, and the other statements END ends
    // open with BEGIN.
    writes: &[
        "INTO",
        "INSERT",
        "UPDATE",
        "DELETE",
        "MERGE",
        "CREATE",
        "DROP",
        "ALTER",
        "ADD",
        "TRUNCATE",
        "EXEC",
        "EXECUTE",
        "GRANT",
        "REVOKE",
        "DENY",
        "BACKUP",
        "RESTORE",
        "DBCC",
        "KILL",
        "SHUTDOWN",
        "BULK",
        "SET",
        "DECLARE",
        "BEGIN",
        "COMMIT",
        "ROLLBACK",
        "SAVE",
        "WAITFOR",
        "RECONFIGURE",
        "CHECKPOINT",
        "RAISERROR",
        "THROW",
        "PRINT",
        "OPEN",
        "CLOSE",
        "DEALLOCATE",
        "GOTO",
        "RETURN",
        "WHILE",
        "IF",
        "ENABLE",
        "DISABLE",
        "UPDATETEXT",
        "WRITETEXT",
        "READTEXT",
        "SETUSER",
        "REVERT",
        "BREAK",
        "CONTINUE",
        // Service Broker: RECEIVE dequeues (deletes) messages.
        "RECEIVE",
        "SEND",
        "MOVE",
        "GET",
        "CONVERSATION",
        // Azure Synapse RENAME OBJECT.
        "RENAME",
        // Table hints that take update or exclusive locks, or hold shared ones to the end of
        // the transaction or over a whole page or table (`WITH (UPDLOCK)`, the old form
        // `FROM t (HOLDLOCK)`): locks a read-only session should not hold, as PostgreSQL's
        // `FOR UPDATE` takes. Words of hints only, so a batch naming one is a write.
        "UPDLOCK",
        "XLOCK",
        "HOLDLOCK",
        "SERIALIZABLE",
        "REPEATABLEREAD",
        "TABLOCK",
        "TABLOCKX",
        "PAGLOCK",
    ],
    // These run a statement hidden in a string, or reach other servers and the file system.
    write_calls: &["OPENQUERY", "OPENROWSET", "OPENDATASOURCE", "XP_*", "SP_*"],
    table_functions: None,
};

fn classify_tsql(sql: &str) -> StatementClass {
    classify::with(&LEX, &RULES, sql)
}

/// The system type of a column's values (lower case, without its length): the base type of an
/// alias type (`ColumnMeta::base_type`), or the column's own.
pub fn system_type(col: &ColumnMeta) -> String {
    let t = col.base_type.as_deref().unwrap_or(&col.data_type);
    let name = t.split('(').next().unwrap_or(t).trim();
    let name = name.strip_prefix("sys.").unwrap_or(name);
    name.to_ascii_lowercase()
}

/// Binary types: CONVERT would read their bytes as UTF-16.
fn is_binary(col: &ColumnMeta) -> bool {
    col.kind == ValueKind::Bytes
        || matches!(
            system_type(col).as_str(),
            "binary" | "varbinary" | "image" | "timestamp" | "rowversion"
        )
}

/// What the grid gets of a column instead of its value, where its value would lose data on the
/// way (None: the column as it is).
fn projection(e: &str, col: &ColumnMeta) -> Option<String> {
    match system_type(col).as_str() {
        // tiberius reads money as a float; style 2 keeps all four decimals.
        "money" | "smallmoney" => Some(format!("CONVERT(NVARCHAR(40), {e}, 2)")),
        "hierarchyid" | "geometry" | "geography" => Some(format!("{e}.ToString()")),
        "json" | "vector" => Some(format!("CONVERT(NVARCHAR(MAX), {e})")),
        _ if col.kind == ValueKind::Geometry => Some(format!("{e}.ToString()")),
        _ => None,
    }
}

/// Spatial values as WKT drop their SRID: they cannot be written back or used as a key.
fn lossy(col: &ColumnMeta) -> bool {
    col.kind == ValueKind::Geometry || matches!(system_type(col).as_str(), "geometry" | "geography")
}

/// The collation pattern filters compare under: code points, so case and accents count.
pub const BINARY_COLLATION: &str = "Latin1_General_100_BIN2";

/// A column as text, for pattern filters and "Any column". Binary columns have no text form.
/// datetime and money take the styles that keep their milliseconds and decimals (the default
/// styles print `Jan  2 2024 10:11AM` and two decimals).
fn text_expr(e: &str, col: &ColumnMeta) -> Option<String> {
    if is_binary(col) {
        return None;
    }
    let text = match system_type(col).as_str() {
        "hierarchyid" | "geometry" | "geography" => format!("{e}.ToString()"),
        "money" | "smallmoney" => format!("CONVERT(NVARCHAR(40), {e}, 2)"),
        "datetime" | "smalldatetime" => format!("CONVERT(NVARCHAR(30), {e}, 121)"),
        _ if col.kind == ValueKind::Geometry => format!("{e}.ToString()"),
        _ => format!("CONVERT(NVARCHAR(MAX), {e})"),
    };
    Some(format!("{text} COLLATE {BINARY_COLLATION}"))
}

/// T-SQL's clocks. `SYSDATETIME()` keeps datetime2's 100 ns (CURRENT_TIMESTAMP is a datetime,
/// a few milliseconds), and a datetimeoffset takes `SYSDATETIMEOFFSET()`, since a datetime2
/// stored there would read as UTC.
fn server_expr(expr: ServerExpr, kind: ValueKind) -> Option<String> {
    let tz = kind == ValueKind::TimestampTz;
    if expr == ServerExpr::CurrentDate {
        return Some("CAST(SYSDATETIME() AS DATE)".into());
    }
    let (clock, fsp) = expr.clock()?;
    Some(match (clock, fsp) {
        (Clock::CurrentTime, None) => "CAST(SYSDATETIME() AS TIME)".into(),
        (Clock::CurrentTime, Some(n)) => format!("CAST(SYSDATETIME() AS TIME({n}))"),
        (_, None) if tz => "SYSDATETIMEOFFSET()".into(),
        (_, Some(n)) if tz => format!("CAST(SYSDATETIMEOFFSET() AS DATETIMEOFFSET({n}))"),
        (Clock::CurrentTimestamp, None) => "CURRENT_TIMESTAMP".into(),
        (_, None) => "SYSDATETIME()".into(),
        (_, Some(n)) => format!("CAST(SYSDATETIME() AS DATETIME2({n}))"),
    })
}

/// Whether a batch switches the session's database: a `USE` anywhere in its code, since no
/// terminator has to come before it (`SELECT 1 USE other`), unless it opens a `USE HINT` or
/// `USE PLAN` query hint (`query_hint_uses`).
pub fn switches_database(sql: &str) -> bool {
    let s = scan(&LEX, PLAIN, sql);
    let code = code_tokens(&s);
    let hints = query_hint_uses(&s, &code);
    code.iter()
        .zip(hints)
        .any(|(t, hint)| !hint && t.kind == TokenKind::Word && s.upper(t) == "USE")
}

/// The tokens of a scan that are code: no space, no comment.
fn code_tokens(s: &Scan) -> Vec<&Token> {
    s.tokens
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::Space | TokenKind::Comment))
        .collect()
}

/// For each of `code`, whether it is the `USE` of a `USE HINT` or `USE PLAN` query hint: one
/// that opens an item of an `OPTION (…)` list. Anywhere else `USE plan` switches to a database
/// named plan.
fn query_hint_uses(s: &Scan, code: &[&Token]) -> Vec<bool> {
    let word = |t: Option<&&Token>, want: &[&str]| {
        t.is_some_and(|t| t.kind == TokenKind::Word && want.contains(&s.upper(t).as_str()))
    };
    let mut hints = vec![false; code.len()];
    // For each open parenthesis, whether it opens an OPTION list.
    let mut parens: Vec<bool> = Vec::new();
    for (i, t) in code.iter().enumerate() {
        let before = i.checked_sub(1).and_then(|k| code.get(k));
        if s.is_punct(t, '(') {
            parens.push(word(before, &["OPTION"]));
        } else if s.is_punct(t, ')') {
            parens.pop();
        } else if parens.last() == Some(&true)
            && word(Some(t), &["USE"])
            && before.is_some_and(|b| s.is_punct(b, '(') || s.is_punct(b, ','))
            && word(code.get(i + 1), &["HINT", "PLAN"])
            && let Some(hint) = hints.get_mut(i)
        {
            *hint = true;
        }
    }
    hints
}

/// Whether a text holds a statement no word of `RULES` names: an ODBC call escape (`{call p}`,
/// `{? = call p}`), which runs a procedure, a CALL, or the old DUMP and LOAD (BACKUP and
/// RESTORE). `RULES` stays the shared test spec's (`portable::testing::TSQL_RULES`), so the
/// session checks these besides the classifier. A word a number may hide (`1e0DUMP`) counts, and
/// so does a scan another reader may end elsewhere.
pub fn hides_statement(sql: &str) -> bool {
    const WORDS: &[&str] = &["CALL", "DUMP", "LOAD"];
    let s = scan(&LEX, PLAIN, sql);
    if s.ambiguous {
        return true;
    }
    let code = code_tokens(&s);
    let digits = |i: usize| {
        code.get(i).is_some_and(|t| {
            t.kind == TokenKind::Word && s.text(t).starts_with(|c: char| c.is_ascii_digit())
        })
    };
    code.iter().enumerate().any(|(i, t)| {
        if s.is_punct(t, '{') {
            // `{call p}` is a CALL word too.
            return code.get(i + 1).is_some_and(|n| s.is_punct(n, '?'));
        }
        if t.kind != TokenKind::Word {
            return false;
        }
        let upper = s.upper(t);
        let folded = s.text(t).to_uppercase();
        let number = digits(i)
            || (i >= 2 && code.get(i - 1).is_some_and(|d| s.is_punct(d, '.')) && digits(i - 2));
        let mut words = vec![upper.as_str(), folded.as_str()];
        if number {
            words.extend(hidden_in_number(&upper));
            words.extend(hidden_in_number(&folded));
        }
        words.into_iter().any(|w| WORDS.contains(&w))
    })
}

/// Whether a batch is one INSERT, UPDATE, DELETE or MERGE, so that `@@ROWCOUNT` after it counts
/// that statement's rows. Statements need no terminator, so every word of `RULES.writes` (and a
/// `USE`) must be one the statement itself has where it stands: its first word, INTO right after
/// an INSERT or MERGE, the one SET outside parentheses an UPDATE must have (`UPDATE STATISTICS t`
/// has none), and a MERGE's `THEN UPDATE SET`, `THEN INSERT` and `THEN DELETE`. So must a read
/// that could follow it outside parentheses (`DELETE FROM t SELECT 1`): a SELECT only as an
/// INSERT's rows (before any VALUES, or after a set operator), a WITH only before a table hint,
/// `TIES`, `ROLLUP` or `CUBE`, a FETCH only after OFFSET's `ROWS`, never a LINENO. A word a
/// number may hide (`1e0SHUTDOWN`), a scan another reader may end elsewhere (`Scan::ambiguous`)
/// or code after a `;` means no, and so does a statement `hides_statement` finds; unsure means
/// no: the batch then reports no count.
pub fn is_lone_dml(sql: &str) -> bool {
    const DML: &[&str] = &["INSERT", "UPDATE", "DELETE", "MERGE"];
    let s = scan(&LEX, PLAIN, sql);
    if s.ambiguous || hides_statement(sql) {
        return false;
    }
    let code = code_tokens(&s);
    let hints = query_hint_uses(&s, &code);
    if let Some(end) = code.iter().position(|t| s.is_punct(t, ';'))
        && code[end..].iter().any(|t| !s.is_punct(t, ';'))
    {
        return false;
    }
    let word = |i: usize| {
        code.get(i)
            .filter(|t| t.kind == TokenKind::Word)
            .map(|t| s.upper(t))
    };
    let Some(first) = word(0).filter(|w| DML.contains(&w.as_str())) else {
        return false;
    };
    let digits = |i: usize| word(i).is_some_and(|w| w.starts_with(|c: char| c.is_ascii_digit()));
    let stop = |w: &str| RULES.writes.contains(&w) || RULES.inspections.contains(&w);
    // Words of a read that may follow the statement, which it may still hold itself.
    let read = |w: &str| RULES.queries.contains(&w) || matches!(w, "FETCH" | "LINENO");
    let mut depth = 0usize;
    let mut set_seen = false;
    let mut rows_seen = false;
    let mut values_seen = false;
    for (i, t) in code.iter().enumerate().skip(1) {
        if s.is_punct(t, '(') {
            depth += 1;
        } else if s.is_punct(t, ')') {
            depth = depth.saturating_sub(1);
        }
        let Some(w) = word(i) else {
            continue;
        };
        // What else the server may read here, as the classifier counts it: a number may end
        // before any letter of its word (`1e0SHUTDOWN`, `1.e5SET`), and a non-ASCII word counts
        // upper-cased by Unicode rules too.
        let number = digits(i) || (i >= 2 && s.is_punct(code[i - 1], '.') && digits(i - 2));
        let text = s.text(t);
        let folded = (!text.is_ascii()).then(|| text.to_uppercase());
        let mut others: Vec<&str> = folded.as_deref().into_iter().collect();
        if number {
            others.extend(hidden_in_number(&w));
            others.extend(folded.as_deref().into_iter().flat_map(hidden_in_number));
        }
        if others
            .into_iter()
            .any(|o| stop(o) || (depth == 0 && read(o)))
        {
            return false;
        }
        let after = |back: usize, want: &str| {
            i.checked_sub(back)
                .is_some_and(|k| word(k).as_deref() == Some(want))
        };
        if depth == 0 && read(&w) {
            let set_operator = after(1, "UNION")
                || after(1, "EXCEPT")
                || after(1, "INTERSECT")
                || (after(1, "ALL") && after(2, "UNION"));
            let owned = match w.as_str() {
                "SELECT" => first == "INSERT" && !values_seen && (!rows_seen || set_operator),
                "WITH" => {
                    code.get(i + 1).is_some_and(|n| s.is_punct(n, '('))
                        || matches!(word(i + 1).as_deref(), Some("TIES" | "ROLLUP" | "CUBE"))
                }
                "FETCH" => after(1, "ROWS") || after(1, "ROW"),
                _ => false,
            };
            if !owned {
                return false;
            }
            rows_seen |= w == "SELECT";
        }
        values_seen |= depth == 0 && w == "VALUES";
        // Query hints, not a switch of database.
        if hints.get(i) == Some(&true) {
            continue;
        }
        if !stop(&w) {
            continue;
        }
        let allowed = match (first.as_str(), w.as_str()) {
            ("INSERT" | "MERGE", "INTO") => i == 1,
            ("UPDATE", "SET") if depth == 0 && !set_seen => {
                set_seen = true;
                true
            }
            ("MERGE", "UPDATE" | "INSERT" | "DELETE") => depth == 0 && after(1, "THEN"),
            ("MERGE", "SET") => depth == 0 && after(1, "UPDATE") && after(2, "THEN"),
            _ => false,
        };
        if !allowed {
            return false;
        }
    }
    first != "UPDATE" || set_seen
}

/// The words `w` may hide where it may be part of a number: each suffix after its first
/// character that starts like a name (`1e0SHUTDOWN` may be `1e0` then SHUTDOWN).
fn hidden_in_number(w: &str) -> impl Iterator<Item = &str> {
    w.char_indices()
        .skip(1)
        .filter(|&(_, c)| is_ident_start(c))
        .map(move |(k, _)| &w[k..])
}

/// Whether read-only Explain may show the plan of `sql`, which it does not run: one INSERT,
/// UPDATE, DELETE or MERGE (`is_lone_dml`) that calls nothing of `RULES.write_calls` or the
/// side-effect functions. Compiling a distributed query opens the linked server or OLE DB
/// provider and has it describe the pass-through text, which some providers do by running it.
pub fn plan_writes_nothing(sql: &str) -> bool {
    is_lone_dml(sql)
        && !classify::calls_listed_function(&LEX, RULES.write_calls, sql)
        && !classify::calls_listed_function(&LEX, SPEC.knobs.side_effect_functions, sql)
}

pub static SPEC: DialectSpec = DialectSpec {
    dialect: SqlDialect::Tsql,
    label: "Transact-SQL",
    quote_ident: bracketed,
    quote_literal: national_quoted,
    quote_literal_exec: national_quoted,
    placeholder: ph_at_p,
    placeholder_style: PlaceholderStyle::AtP,
    placeholders_reusable: true,
    max_params: 2098,
    max_name_parts: 3,
    lex: &LEX,
    rules: &RULES,
    classify: classify_tsql,
    knobs: SqlKnobs {
        bool_literal: ["0", "1"],
        bytes_literal: hex_0x,
        text_projection: Some(projection),
        lossy_text: lossy,
        text_expr,
        paging: Paging::OffsetFetch,
        limit_one: LimitOne::Top,
        count_expr: "COUNT_BIG(*)",
        nulls_order: NullsSort::NullFlag,
        like_wildcards: &['%', '_', '['],
        null_safe_eq: NullSafeEq::OrForm,
        returning: Returning::OutputInserted,
        server_expr,
        // The plan comes from `SET SHOWPLAN_XML ON` in a batch of its own (`showplan`), which no
        // prefix can say: the driver runs it.
        explain: explain_unsupported,
        analyze_tx: Some(("BEGIN TRANSACTION", "ROLLBACK TRANSACTION")),
        switch_db: switches_database,
        catalog_words: &[
            "INFORMATION_SCHEMA",
            "SYS.",
            "OBJECT_DEFINITION",
            "SERVERPROPERTY",
            "DATABASEPROPERTYEX",
            "HAS_DBACCESS",
        ],
        // `NEXT VALUE FOR s` advances a sequence: a read to Safe Mode, like `nextval('s')` in
        // the core dialects, which the Metrics Board and a read-only session refuse.
        side_effect_functions: &[
            "OPENQUERY",
            "OPENROWSET",
            "OPENDATASOURCE",
            "XP_*",
            "SP_*",
            "NEXT VALUE FOR",
        ],
        redact: &[
            RedactRule {
                keyword: "PASSWORD",
            },
            RedactRule {
                keyword: "OLD_PASSWORD",
            },
            RedactRule {
                keyword: "MEDIAPASSWORD",
            },
            // sp_addlinkedsrvlogin @rmtpassword, sp_addlinkedserver @provstr.
            RedactRule {
                keyword: "RMTPASSWORD",
            },
            RedactRule { keyword: "PROVSTR" },
            RedactRule { keyword: "SECRET" },
            RedactRule {
                keyword: "KEY_SOURCE",
            },
            RedactRule {
                keyword: "ENCRYPTBYPASSPHRASE",
            },
            RedactRule {
                keyword: "DECRYPTBYPASSPHRASE",
            },
        ],
        ..SqlKnobs::ANSI
    },
    hooks: SqlHooks::NONE,
    ddl: None,
};

// --- The driver's own statements ---------------------------------------------------------------

/// Sent after every login. tiberius logs in with the ODBC flag (ANSI_NULLS, ANSI_WARNINGS,
/// QUOTED_IDENTIFIER… on) but leaves ARITHABORT off, which DML on indexed views and computed-column
/// indexes refuses (SSMS turns it on), and TEXTSIZE at the server's default, which would cut
/// `(max)`, text and image values.
pub const SESSION_SETUP: &str =
    "SET ARITHABORT ON; SET TEXTSIZE 2147483647; SET IMPLICIT_TRANSACTIONS OFF";

/// Sent on the editor's connection after each batch: the rows its last statement touched (read
/// before anything else resets `@@ROWCOUNT`) and the transaction depth, which is exact where
/// guessing from the statements is not (a bare `BEGIN` opens a block, not a transaction).
pub const AFTER_BATCH: &str = "SELECT @@ROWCOUNT, @@TRANCOUNT";

/// The keep-alive of a worker connection.
pub const PING: &str = "SELECT 1";

/// Rolls back what an Attention left open, when the session is in a transaction it did not start.
pub const ROLLBACK_IF_OPEN: &str = "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION";

/// The statement `sqlgen::portable::select::build_fetch_cell` starts with under this spec
/// (`LimitOne::Top`): the one read whose binary values the driver keeps whole, where every other
/// read keeps the grid's preview of them.
pub fn reads_one_cell(sql: &str) -> bool {
    sql.starts_with("SELECT TOP (1) ")
}

/// A statement followed by its row count, as the last result set. `@@ROWCOUNT` counts the rows
/// of the statement itself, where the DONE counts the client sums also hold its triggers'.
pub fn counted(sql: &str) -> String {
    let mut s = sql.trim_end();
    while let Some(rest) = s.strip_suffix(';') {
        s = rest.trim_end();
    }
    format!("{s};\nSELECT @@ROWCOUNT")
}

/// `KILL n` of a session id the process list showed.
pub fn kill(spid: i64) -> Result<String, AppError> {
    // No floor at 50: since SQL Server 2005 a low id is not always a system session. The process
    // list holds user sessions only (`is_user_process`), and the server refuses to end a system
    // one. Session ids are smallint.
    if spid < 1 || spid > i64::from(i16::MAX) {
        return Err(AppError::InvalidInput(format!(
            "{spid} is not a user session id"
        )));
    }
    Ok(format!("KILL {spid}"))
}

/// The statement that turns plan output on or off for `mode`: `SHOWPLAN_XML` plans without
/// running, `STATISTICS XML` runs the statement and adds the actual plan. Each must be the only
/// statement of its batch.
pub fn showplan(mode: ExplainMode, on: bool) -> &'static str {
    match (mode, on) {
        (ExplainMode::Plan, true) => "SET SHOWPLAN_XML ON",
        (ExplainMode::Plan, false) => "SET SHOWPLAN_XML OFF",
        (ExplainMode::Analyze, true) => "SET STATISTICS XML ON",
        (ExplainMode::Analyze, false) => "SET STATISTICS XML OFF",
    }
}

/// The one column of a result set that holds a plan.
pub const SHOWPLAN_COLUMN: &str = "Microsoft SQL Server 2005 XML Showplan";

/// Sent once `SET SHOWPLAN_XML ON` was answered, before the statement: under plan output it
/// comes back as one plan, and anything else means the server took the SET without honouring it
/// (Babelfish ignores it by default), so the statement would run.
pub const PLAN_PROBE: &str = "SELECT 1 AS plan_probe";

/// What the session's EXPLAIN (`DbSession::explain_form`) puts before a statement. No batch can
/// run it as written (SHOWPLAN_XML must be alone in its batch), so the driver reads it back
/// (`explain_of`) and runs the three batches it stands for. The comment tells it from a user's own
/// `SET STATISTICS XML ON <statement>`, which runs as it is.
pub fn explain_prefix(mode: ExplainMode) -> &'static str {
    match mode {
        ExplainMode::Plan => "SET SHOWPLAN_XML ON /* Tabler Explain */",
        ExplainMode::Analyze => "SET STATISTICS XML ON /* Tabler Explain Analyze */",
    }
}

/// The mode and the statement of a text `explain_prefix` wrapped (the prefix and one space).
pub fn explain_of(sql: &str) -> Option<(ExplainMode, &str)> {
    [ExplainMode::Plan, ExplainMode::Analyze]
        .into_iter()
        .find_map(|mode| {
            let rest = sql.strip_prefix(explain_prefix(mode))?;
            let inner = rest.strip_prefix(' ')?;
            (!inner.trim().is_empty()).then_some((mode, inner))
        })
}

/// The argument of `OBJECT_ID(@P1)` for a table: its name quoted and qualified, so dots and
/// brackets in names stay names.
pub fn object_name(schema: Option<&str>, name: &str) -> String {
    match schema {
        Some(s) => format!("{}.{}", bracketed(s), bracketed(name)),
        None => bracketed(name),
    }
}

// --- Show DDL ------------------------------------------------------------------------------------

/// A table's CREATE TABLE and what completes it (its other indexes, its triggers and the
/// `MS_Description` comments), from what the catalog says of it, since SQL Server has no SHOW
/// CREATE. Batches end at `GO` lines, as SSMS scripts them: a trigger must start its own.
pub fn create_table(meta: &TableMeta) -> String {
    let table = object_name(meta.table.schema.as_deref(), &meta.table.name);
    let mut lines: Vec<String> = meta.columns.iter().map(column_definition).collect();
    let period = |marker: &str| {
        meta.columns
            .iter()
            .find(|c| c.extra.as_deref().is_some_and(|e| e.ends_with(marker)))
    };
    if let (Some(start), Some(end)) = (period("AS ROW START"), period("AS ROW END")) {
        lines.push(format!(
            "PERIOD FOR SYSTEM_TIME ({}, {})",
            bracketed(&start.name),
            bracketed(&end.name)
        ));
    }
    match meta.indexes.iter().find(|i| i.primary) {
        Some(pk) => lines.push(format!(
            "CONSTRAINT {} PRIMARY KEY {} ({})",
            bracketed(&pk.name),
            index_method(pk),
            names(&pk.columns)
        )),
        None if !meta.primary_key.is_empty() => {
            lines.push(format!("PRIMARY KEY ({})", names(&meta.primary_key)));
        }
        None => {}
    }
    for fk in &meta.foreign_keys {
        let mut line = format!(
            "CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
            bracketed(&fk.name),
            names(&fk.columns),
            object_name(fk.ref_table.schema.as_deref(), &fk.ref_table.name),
            names(&fk.ref_columns)
        );
        for (event, action) in [("DELETE", &fk.on_delete), ("UPDATE", &fk.on_update)] {
            if let Some(action) = referential_action(action) {
                line.push_str(&format!(" ON {event} {action}"));
            }
        }
        lines.push(line);
    }
    for check in &meta.checks {
        lines.push(format!(
            "CONSTRAINT {} CHECK ({})",
            bracketed(&check.name),
            check.expression
        ));
    }
    let mut out = format!(
        "CREATE TABLE {table} (\n    {}\n);\n",
        lines.join(",\n    ")
    );
    for index in meta.indexes.iter().filter(|i| !i.primary) {
        out.push_str(&format!("GO\n{};\n", create_index(&table, index)));
    }
    for trigger in &meta.triggers {
        out.push_str(&format!("GO\n{}\n", trigger.definition.trim()));
        if !trigger.enabled {
            out.push_str(&format!(
                "GO\nDISABLE TRIGGER {} ON {table};\n",
                bracketed(&trigger.name)
            ));
        }
    }
    if let Some(schema) = meta.table.schema.as_deref() {
        let level = |column: Option<&str>| {
            let mut s = format!(
                "@level0type = N'SCHEMA', @level0name = {}, @level1type = N'TABLE', \
                 @level1name = {}",
                national_quoted(schema),
                national_quoted(&meta.table.name)
            );
            if let Some(c) = column {
                s.push_str(&format!(
                    ", @level2type = N'COLUMN', @level2name = {}",
                    national_quoted(c)
                ));
            }
            s
        };
        let comments = std::iter::once((None, meta.comment.as_deref())).chain(
            meta.columns
                .iter()
                .map(|c| (Some(c.name.as_str()), c.comment.as_deref())),
        );
        for (column, comment) in comments {
            if let Some(comment) = comment {
                out.push_str(&format!(
                    "GO\nEXEC sys.sp_addextendedproperty @name = N'MS_Description', @value = {}, \
                     {};\n",
                    national_quoted(comment),
                    level(column)
                ));
            }
        }
    }
    out
}

/// A column as CREATE TABLE writes it: a computed one by its expression, the others by type,
/// collation, identity or period, nullability and default.
fn column_definition(c: &ColumnMeta) -> String {
    let name = bracketed(&c.name);
    if let Some(computed) = c.extra.as_deref().filter(|e| e.starts_with("AS (")) {
        return format!("{name} {computed}");
    }
    let mut s = format!("{name} {}", c.data_type);
    if let Some(collation) = c.collation.as_deref() {
        s.push_str(&format!(" COLLATE {collation}"));
    }
    if let Some(extra) = c.extra.as_deref() {
        s.push(' ');
        s.push_str(extra);
    }
    s.push_str(if c.nullable { " NULL" } else { " NOT NULL" });
    if let Some(default) = c.default_expr.as_deref() {
        s.push_str(&format!(" DEFAULT ({default})"));
    }
    s
}

fn names(columns: &[String]) -> String {
    columns
        .iter()
        .map(|c| bracketed(c))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `CLUSTERED`, `NONCLUSTERED COLUMNSTORE`…: the index's `type_desc` as DDL spells it.
fn index_method(index: &IndexMeta) -> String {
    index
        .method
        .as_deref()
        .unwrap_or("nonclustered")
        .to_ascii_uppercase()
}

/// An action other than the default, which DDL leaves out.
fn referential_action(action: &str) -> Option<&str> {
    let action = action.trim();
    (!action.is_empty() && !action.eq_ignore_ascii_case("NO ACTION")).then_some(action)
}

fn create_index(table: &str, index: &IndexMeta) -> String {
    let method = index_method(index);
    let unique = if index.unique { "UNIQUE " } else { "" };
    let mut s = format!(
        "CREATE {unique}{method} INDEX {} ON {table}",
        bracketed(&index.name)
    );
    // A clustered columnstore index holds every column and names none.
    if method != "CLUSTERED COLUMNSTORE" {
        s.push_str(&format!(" ({})", names(&index.columns)));
    }
    if !index.include.is_empty() {
        s.push_str(&format!(" INCLUDE ({})", names(&index.include)));
    }
    if let Some(predicate) = index.predicate.as_deref() {
        s.push_str(&format!(" WHERE {predicate}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::sqlgen::portable::testing::{col, map, text};
    use crate::sqlgen::portable::{literal, mutate, select, split};
    use crate::sqlgen::sql::{SelectQuery, TableTarget};
    use crate::types::browse::{
        Conjunction, FilterGroup, FilterNode, FilterOp, NullsOrder, RowIdentity, RowKey, SortDir,
        SortSpec,
    };
    use crate::types::mutate::StagedChange;
    use crate::types::schema::TableRef;
    use crate::types::value::Value;

    #[test]
    fn export_bindings_lex_tsql() {
        crate::sqlgen::dialect::lex::export_lex("tsql", &LEX, &RULES);
    }

    /// The renderer's cases run against `TSQL_LIKE_TEST`, so its rules must be these.
    #[test]
    fn the_test_spec_classifies_alike() {
        use crate::sqlgen::portable::testing::{TSQL_LEX, TSQL_RULES};
        assert_eq!(RULES.queries, TSQL_RULES.queries);
        assert_eq!(RULES.inspections, TSQL_RULES.inspections);
        assert_eq!(RULES.writes, TSQL_RULES.writes);
        assert_eq!(RULES.write_calls, TSQL_RULES.write_calls);
        assert_eq!(
            format!("{LEX:?}"),
            format!("{TSQL_LEX:?}"),
            "the lexer profile the renderer's T-SQL cases run on"
        );
    }

    #[test]
    fn quotes_and_classifies() {
        assert_eq!((SPEC.quote_ident)("a]b"), "[a]]b]");
        assert_eq!((SPEC.quote_literal)("it's"), "N'it''s'");
        assert_eq!(
            classify_tsql("SELECT 1 DELETE FROM t"),
            StatementClass::Write
        );
        assert_eq!(
            classify_tsql("SELECT * FROM OPENQUERY(s, 'DELETE FROM t')"),
            StatementClass::Write
        );
        assert_eq!(
            classify_tsql("SELECT 1 ADD SENSITIVITY CLASSIFICATION TO dbo.t.c WITH (LABEL = 'x')"),
            StatementClass::Write
        );
        assert_eq!(classify_tsql("SELECT [x] FROM t"), StatementClass::Read);
        assert_eq!(
            classify_tsql("SELECT 1e5, 0x1F FROM t"),
            StatementClass::Read
        );
        assert_eq!(
            classify_tsql("SELECT 1xp_cmdshell('dir')"),
            StatementClass::Write
        );
        for sql in [
            // Statements a terminator need not end.
            "SELECT 1 RECEIVE * FROM dbo.q",
            "SELECT 1 MOVE CONVERSATION 'a' TO 'b'",
            "SELECT 1 SETUSER 'dbo'",
            "SELECT 1 SEND ON CONVERSATION @h (N'x')",
            "SELECT 1 GET CONVERSATION GROUP @g FROM dbo.q",
            "SELECT 1 END CONVERSATION @h",
            "SELECT 1 RENAME OBJECT t TO u",
            // A number ends before the keyword after it.
            "SELECT 1DELETE FROM t",
            "SELECT 1e0DELETE FROM t",
            "SELECT $1DELETE FROM t",
            "SELECT 0xINTO t2 FROM t",
            "SELECT 1EXEC sp_who",
            // Also past a decimal point, which the scanner keeps apart (`1`, `.`, `E5DELETE`).
            "SELECT 1.e5DELETE FROM t",
            "SELECT 1.eDELETE FROM t",
            "SELECT 1.E5EXEC sp_who",
        ] {
            assert_eq!(classify_tsql(sql), StatementClass::Write, "{sql}");
            assert!(
                !crate::sqlgen::portable::split::is_single_statement(&LEX, &RULES, sql),
                "{sql}"
            );
        }
        // Comments a reader may end elsewhere: at the lone CR or only at the LF, at the first
        // `*/` or the one that closes the nesting.
        for sql in [
            "SELECT 1 -- x\r' \nDELETE FROM t --'",
            "SELECT 1 /* /* */ DELETE FROM t */",
            "/* /* */ DELETE FROM t --*/ SELECT 1",
        ] {
            assert_eq!(classify_tsql(sql), StatementClass::Write, "{sql:?}");
            assert!(
                !crate::sqlgen::portable::split::is_single_statement(&LEX, &RULES, sql),
                "{sql:?}"
            );
        }
    }

    /// Locking hints take locks a read-only session should not hold, as PostgreSQL's FOR UPDATE
    /// does, so they write; hints that take no more than a read's locks stay reads.
    #[test]
    fn locking_hints_are_writes() {
        for sql in [
            "SELECT * FROM t WITH (UPDLOCK) WHERE id = 1",
            "SELECT * FROM t WITH (UPDLOCK, ROWLOCK)",
            "SELECT * FROM t WITH (XLOCK)",
            "SELECT * FROM t WITH (HOLDLOCK)",
            "SELECT * FROM t WITH (TABLOCKX)",
            "SELECT * FROM t WITH (TABLOCK)",
            "SELECT * FROM t WITH (PAGLOCK)",
            "SELECT * FROM t WITH (SERIALIZABLE)",
            "SELECT * FROM t WITH (REPEATABLEREAD)",
            "SELECT * FROM t WITH (NOLOCK, updlock)",
            // The old form, without WITH.
            "SELECT * FROM t (UPDLOCK)",
            "SELECT * FROM t AS a (HOLDLOCK) JOIN u WITH (NOLOCK) ON a.id = u.id",
            "WITH c AS (SELECT * FROM t WITH (XLOCK)) SELECT * FROM c",
            "SELECT * FROM t WHERE id IN (SELECT id FROM u WITH (UPDLOCK))",
        ] {
            assert_eq!(classify_tsql(sql), StatementClass::Write, "{sql}");
        }
        for sql in [
            "SELECT * FROM t WITH (NOLOCK)",
            "SELECT * FROM t WITH (READPAST, ROWLOCK, INDEX(ix))",
            "SELECT * FROM t WITH (READCOMMITTEDLOCK, NOWAIT)",
            "SELECT [updlock], 'HOLDLOCK' FROM t -- XLOCK",
        ] {
            assert_eq!(classify_tsql(sql), StatementClass::Read, "{sql}");
        }
    }

    /// `NEXT VALUE FOR` advances a sequence, and follows the core dialects' `nextval('s')`: a
    /// read to Safe Mode, refused where side-effect calls are (the Metrics Board, and a read-only
    /// session: `drivers::sqlserver::tests`).
    #[test]
    fn next_value_for_is_a_side_effect_of_a_read() {
        use crate::sqlgen::sql::calls_side_effect_function;
        for sql in [
            "SELECT NEXT VALUE FOR s",
            "SELECT next value for dbo.s",
            "SELECT @x = NEXT VALUE FOR s",
            "WITH c AS (SELECT NEXT VALUE FOR s AS n) SELECT n FROM c",
            "SELECT NEXT /* c */ VALUE\nFOR s",
            "SELECT NEXT VALUE FOR s OVER (ORDER BY id) FROM t",
        ] {
            assert_eq!(classify_tsql(sql), StatementClass::Read, "{sql}");
            assert!(calls_side_effect_function(SqlDialect::Tsql, sql), "{sql}");
        }
        for sql in [
            "SELECT * FROM t ORDER BY id OFFSET 1 ROWS FETCH NEXT 1 ROWS ONLY",
            "SELECT 'NEXT VALUE FOR s', [next] FROM t",
            "SELECT next_value FROM t",
        ] {
            assert!(!calls_side_effect_function(SqlDialect::Tsql, sql), "{sql}");
        }
    }

    // --- Goldens of the builders under this spec ----------------------------------------------

    /// A table with a column of each type the spec treats apart.
    fn columns() -> Vec<ColumnMeta> {
        let mut id = col("id", "bigint", ValueKind::BigInt);
        id.is_pk = true;
        id.pk_ordinal = Some(1);
        id.nullable = false;
        let mut phone = col("phone", "Phone", ValueKind::Text);
        phone.base_type = Some("nvarchar".into());
        let mut wallet = col("wallet", "Wallet", ValueKind::Decimal);
        wallet.base_type = Some("money".into());
        vec![
            id,
            col("name", "nvarchar(50)", ValueKind::Text),
            col("price", "money", ValueKind::Decimal),
            col("tiny", "smallmoney", ValueKind::Decimal),
            col("ratio", "decimal(10,2)", ValueKind::Decimal),
            col("data", "varbinary(max)", ValueKind::Bytes),
            col("stamp", "timestamp", ValueKind::Bytes),
            col("geom", "geography", ValueKind::Geometry),
            col("node", "hierarchyid", ValueKind::Other),
            col("doc", "json", ValueKind::Json),
            col("seen", "datetime", ValueKind::Timestamp),
            col("at", "datetimeoffset(7)", ValueKind::TimestampTz),
            col("Odd ]Name", "nvarchar(10)", ValueKind::Text),
            phone,
            wallet,
        ]
    }

    fn by<'a>(cols: &'a [ColumnMeta], name: &str) -> &'a ColumnMeta {
        cols.iter().find(|c| c.name == name).expect("a test column")
    }

    #[test]
    fn projects_only_what_would_lose_data() {
        let cols = columns();
        let p = |name: &str| projection(&bracketed(name), by(&cols, name));
        assert_eq!(p("id"), None);
        assert_eq!(p("name"), None);
        assert_eq!(p("ratio"), None);
        assert_eq!(p("data"), None);
        assert_eq!(p("seen"), None);
        assert_eq!(p("at"), None);
        assert_eq!(
            p("price").as_deref(),
            Some("CONVERT(NVARCHAR(40), [price], 2)")
        );
        assert_eq!(
            p("tiny").as_deref(),
            Some("CONVERT(NVARCHAR(40), [tiny], 2)")
        );
        // An alias type of money is money.
        assert_eq!(
            p("wallet").as_deref(),
            Some("CONVERT(NVARCHAR(40), [wallet], 2)")
        );
        assert_eq!(p("geom").as_deref(), Some("[geom].ToString()"));
        assert_eq!(p("node").as_deref(), Some("[node].ToString()"));
        assert_eq!(p("doc").as_deref(), Some("CONVERT(NVARCHAR(MAX), [doc])"));
        assert!(lossy(by(&cols, "geom")) && !lossy(by(&cols, "node")));
        assert!(!lossy(by(&cols, "price")));
    }

    #[test]
    fn text_forms_keep_digits_and_compare_by_code_point() {
        let cols = columns();
        let t = |name: &str| text_expr(&bracketed(name), by(&cols, name));
        assert_eq!(t("data"), None);
        assert_eq!(t("stamp"), None);
        assert_eq!(
            t("name").as_deref(),
            Some("CONVERT(NVARCHAR(MAX), [name]) COLLATE Latin1_General_100_BIN2")
        );
        assert_eq!(
            t("seen").as_deref(),
            Some("CONVERT(NVARCHAR(30), [seen], 121) COLLATE Latin1_General_100_BIN2")
        );
        assert_eq!(
            t("price").as_deref(),
            Some("CONVERT(NVARCHAR(40), [price], 2) COLLATE Latin1_General_100_BIN2")
        );
        assert_eq!(
            t("geom").as_deref(),
            Some("[geom].ToString() COLLATE Latin1_General_100_BIN2")
        );
        assert_eq!(system_type(by(&cols, "phone")), "nvarchar");
        assert_eq!(
            system_type(&col("x", "sys.geometry", ValueKind::Geometry)),
            "geometry"
        );
        assert_eq!(
            system_type(&col("x", "NVARCHAR(MAX)", ValueKind::Text)),
            "nvarchar"
        );
    }

    fn query<'a>(
        t: &'a TableRef,
        cols: &'a [ColumnMeta],
        filter: Option<&'a FilterGroup>,
        sort: &'a [SortSpec],
        pk: &'a [String],
    ) -> SelectQuery<'a> {
        SelectQuery {
            table: t,
            columns: cols,
            selected: None,
            filter,
            sort,
            limit: 100,
            offset: 200,
            pk,
            identity: RowIdentity::Pk,
        }
    }

    #[test]
    fn browse_pages_with_offset_fetch() {
        let t = TableRef::new(Some("dbo"), "Or]ders");
        let cols = columns();
        let pk = ["id".to_string()];
        let built = select::build_select(&SPEC, &query(&t, &cols, None, &[], &pk)).unwrap();
        assert_eq!(
            built.sql,
            "SELECT [id], [name], CONVERT(NVARCHAR(40), [price], 2) AS [price], \
             CONVERT(NVARCHAR(40), [tiny], 2) AS [tiny], [ratio], [data], [stamp], \
             [geom].ToString() AS [geom], [node].ToString() AS [node], \
             CONVERT(NVARCHAR(MAX), [doc]) AS [doc], [seen], [at], [Odd ]]Name], [phone], \
             CONVERT(NVARCHAR(40), [wallet], 2) AS [wallet]\nFROM [dbo].[Or]]ders] ORDER BY \
             [id] ASC\nOFFSET 200 ROWS FETCH NEXT 100 ROWS ONLY"
        );
        assert!(built.params.is_empty());
        // Without a key the order is still there, since OFFSET needs one.
        let built = select::build_select(&SPEC, &query(&t, &cols[1..2], None, &[], &[])).unwrap();
        assert_eq!(
            built.sql,
            "SELECT [name]\nFROM [dbo].[Or]]ders] ORDER BY (SELECT NULL)\nOFFSET 200 ROWS \
             FETCH NEXT 100 ROWS ONLY"
        );
        // NULLs first or last through a flag, since T-SQL has no NULLS FIRST.
        let sort = [SortSpec {
            column: "name".into(),
            dir: SortDir::Desc,
            nulls: Some(NullsOrder::Last),
        }];
        let built = select::build_select(&SPEC, &query(&t, &cols[1..2], None, &sort, &pk)).unwrap();
        assert!(
            built.sql.ends_with(
                "ORDER BY CASE WHEN [name] IS NULL THEN 1 ELSE 0 END ASC, [name] DESC\nOFFSET \
                 200 ROWS FETCH NEXT 100 ROWS ONLY"
            ),
            "{}",
            built.sql
        );
    }

    #[test]
    fn filters_bind_at_p_and_escape_brackets() {
        let t = TableRef::new(Some("dbo"), "t");
        let cols = columns();
        let filter = FilterGroup {
            conjunction: Conjunction::And,
            items: vec![
                FilterNode::Condition {
                    column: "name".into(),
                    op: FilterOp::Contains,
                    values: vec![text("50%_[x]")],
                },
                FilterNode::Condition {
                    column: "name".into(),
                    op: FilterOp::ContainsCase,
                    values: vec![text("Bob")],
                },
                FilterNode::Condition {
                    column: "id".into(),
                    op: FilterOp::In,
                    values: vec![Value::Int(1), Value::Int(2)],
                },
                FilterNode::AnyColumn {
                    op: FilterOp::Eq,
                    values: vec![text("a")],
                },
            ],
        };
        let built = select::build_count(&SPEC, &t, &cols, Some(&filter)).unwrap();
        let bin = "COLLATE Latin1_General_100_BIN2";
        assert!(
            built.sql.starts_with(&format!(
                "SELECT COUNT_BIG(*) FROM [dbo].[t] WHERE CONVERT(NVARCHAR(MAX), [name]) {bin} \
                 LIKE @P1 ESCAPE '!' AND LOWER(CONVERT(NVARCHAR(MAX), [name]) {bin}) LIKE \
                 LOWER(@P2) ESCAPE '!' AND [id] IN (@P3, @P4) AND (COALESCE(CONVERT(\
                 NVARCHAR(MAX), [id]) {bin}, '') = @P5 OR "
            )),
            "{}",
            built.sql
        );
        // Binary columns are left out of Any column; every other one reuses @P5.
        assert!(!built.sql.contains("[data]") && !built.sql.contains("[stamp]"));
        assert_eq!(built.sql.matches("@P5").count(), cols.len() - 3);
        assert_eq!(
            built.params,
            vec![
                text("%50!%!_![x]%"),
                text("%Bob%"),
                Value::Int(1),
                Value::Int(2),
                text("a"),
            ]
        );
    }

    #[test]
    fn fetch_cell_takes_top_one() {
        let t = TableRef::new(Some("dbo"), "t");
        let cols = columns();
        let target = TableTarget {
            table: &t,
            columns: &cols,
        };
        let key = RowKey::Pk(map([("id", Value::Int(7))]));
        let (sql, params) =
            select::build_fetch_cell(&SPEC, &target, by(&cols, "price"), &key).unwrap();
        assert_eq!(
            sql,
            "SELECT TOP (1) CONVERT(NVARCHAR(40), [price], 2) FROM [dbo].[t] WHERE [id] = @P1"
        );
        assert_eq!(params, vec![Value::Int(7)]);
        // A row without a key matches every column NULL-safely, reusing each parameter.
        let key = RowKey::AllColumns(map([("name", text("x")), ("ratio", Value::Null)]));
        let (sql, _) = select::build_fetch_cell(&SPEC, &target, by(&cols, "name"), &key).unwrap();
        assert_eq!(
            sql,
            "SELECT TOP (1) [name] FROM [dbo].[t] WHERE ([name] = @P1 OR ([name] IS NULL AND @P1 \
             IS NULL)) AND [ratio] IS NULL"
        );
        // Spatial values are shown as WKT without their SRID: never loaded as the whole value.
        assert!(select::build_fetch_cell(&SPEC, &target, by(&cols, "geom"), &key).is_err());
    }

    #[test]
    fn edits_output_inserted_before_values_and_where() {
        let t = TableRef::new(Some("dbo"), "t");
        let cols = vec![
            by(&columns(), "id").clone(),
            by(&columns(), "name").clone(),
            by(&columns(), "price").clone(),
            by(&columns(), "at").clone(),
        ];
        let target = TableTarget {
            table: &t,
            columns: &cols,
        };
        let update = StagedChange::Update {
            row_id: "r".into(),
            key: RowKey::Pk(map([("id", Value::Int(7))])),
            changes: map([("name", text("it's")), ("price", Value::Default)]),
            exprs: Some(BTreeMap::from([("at".to_string(), ServerExpr::Now)])),
        };
        let (built, expect) = mutate::build_change(&SPEC, &target, &update, true).unwrap();
        assert_eq!(expect, Some(1));
        assert_eq!(
            built.sql,
            "UPDATE [dbo].[t] SET [at] = SYSDATETIMEOFFSET(), [name] = @P1, [price] = DEFAULT \
             OUTPUT INSERTED.[id], INSERTED.[name], CONVERT(NVARCHAR(40), INSERTED.[price], 2) \
             AS [price], INSERTED.[at] WHERE [id] = @P2"
        );
        assert_eq!(
            literal::inline_params(&SPEC, &built.sql, &built.params),
            "UPDATE [dbo].[t] SET [at] = SYSDATETIMEOFFSET(), [name] = N'it''s', [price] = \
             DEFAULT OUTPUT INSERTED.[id], INSERTED.[name], CONVERT(NVARCHAR(40), \
             INSERTED.[price], 2) AS [price], INSERTED.[at] WHERE [id] = 7"
        );
        let insert = StagedChange::Insert {
            row_id: "n".into(),
            values: map([("name", text("a")), ("id", Value::Default)]),
            exprs: None,
        };
        let (built, _) = mutate::build_change(&SPEC, &target, &insert, false).unwrap();
        assert_eq!(built.sql, "INSERT INTO [dbo].[t] ([name]) VALUES (@P1)");
        let defaults = StagedChange::Insert {
            row_id: "n".into(),
            values: BTreeMap::new(),
            exprs: None,
        };
        let (built, _) = mutate::build_change(&SPEC, &target, &defaults, true).unwrap();
        assert!(
            built
                .sql
                .starts_with("INSERT INTO [dbo].[t] OUTPUT INSERTED.[id], ")
                && built.sql.ends_with(" DEFAULT VALUES"),
            "{}",
            built.sql
        );
        let delete = StagedChange::Delete {
            row_id: "r".into(),
            key: RowKey::Pk(map([("id", Value::Int(7))])),
        };
        let (built, _) = mutate::build_change(&SPEC, &target, &delete, false).unwrap();
        assert_eq!(built.sql, "DELETE FROM [dbo].[t] WHERE [id] = @P1");
    }

    #[test]
    fn literals_for_preview() {
        assert_eq!(literal::literal(&SPEC, &Value::Bool(true)), "1");
        assert_eq!(
            literal::literal(&SPEC, &Value::from_bytes(&[0, 255])),
            "0x00ff"
        );
        assert_eq!(literal::literal(&SPEC, &text("a'b")), "N'a''b'");
        assert_eq!(
            literal::literal(&SPEC, &Value::Timestamp("2024-01-02 10:11:12.123".into())),
            "N'2024-01-02 10:11:12.123'"
        );
        // Placeholders in strings, names and comments are not parameters.
        assert_eq!(
            literal::inline_params(
                &SPEC,
                "SELECT @P1, '@P1', [@P1] -- @P1\n, @P10, @p2",
                &[Value::Int(1), text("x")]
            ),
            "SELECT 1, '@P1', [@P1] -- @P1\n, @P10, N'x'"
        );
    }

    #[test]
    fn clocks() {
        let at = |e, k| server_expr(e, k);
        assert_eq!(
            at(ServerExpr::Now, ValueKind::Timestamp).as_deref(),
            Some("SYSDATETIME()")
        );
        assert_eq!(
            at(ServerExpr::CurrentTimestamp, ValueKind::Timestamp).as_deref(),
            Some("CURRENT_TIMESTAMP")
        );
        assert_eq!(
            at(ServerExpr::Fsp(Clock::Now, 3), ValueKind::Timestamp).as_deref(),
            Some("CAST(SYSDATETIME() AS DATETIME2(3))")
        );
        assert_eq!(
            at(ServerExpr::Now, ValueKind::TimestampTz).as_deref(),
            Some("SYSDATETIMEOFFSET()")
        );
        assert_eq!(
            at(
                ServerExpr::Fsp(Clock::LocalTimestamp, 2),
                ValueKind::TimestampTz
            )
            .as_deref(),
            Some("CAST(SYSDATETIMEOFFSET() AS DATETIMEOFFSET(2))")
        );
        assert_eq!(
            at(ServerExpr::CurrentDate, ValueKind::Date).as_deref(),
            Some("CAST(SYSDATETIME() AS DATE)")
        );
        assert_eq!(
            at(ServerExpr::CurrentTime, ValueKind::Time).as_deref(),
            Some("CAST(SYSDATETIME() AS TIME)")
        );
        assert_eq!(
            at(ServerExpr::Fsp(Clock::CurrentTime, 0), ValueKind::Time).as_deref(),
            Some("CAST(SYSDATETIME() AS TIME(0))")
        );
        assert_eq!(at(ServerExpr::Default, ValueKind::Timestamp), None);
    }

    #[test]
    fn batches_split_at_go_lines_only() {
        let parts: Vec<String> = split::split(
            &LEX,
            "CREATE VIEW v AS SELECT 1 AS a;\nGO\nSELECT 'x\nGO\ny'; SELECT [a\ngo\n]\n  go 2  \
             -- twice\r\nEXEC p\n/*\nGO\n*/\nSELECT 2",
        )
        .into_iter()
        .map(|s| s.sql)
        .collect();
        assert_eq!(
            parts,
            [
                "CREATE VIEW v AS SELECT 1 AS a;",
                "SELECT 'x\nGO\ny'; SELECT [a\ngo\n]",
                "EXEC p\n/*\nGO\n*/\nSELECT 2",
            ]
        );
        // `GO` must be alone on its line (a count and a comment may follow).
        assert_eq!(split::split(&LEX, "SELECT 1 GO SELECT 2").len(), 1);
        assert_eq!(split::split(&LEX, "SELECT 1\nGO;\nSELECT 2").len(), 1);
        assert_eq!(split::split(&LEX, "SELECT 1\nGOTO x\nSELECT 2").len(), 1);
    }

    #[test]
    fn use_anywhere_switches_the_database() {
        assert!(switches_database("USE master"));
        assert!(switches_database("SELECT 1 USE master"));
        assert!(switches_database("SELECT 1\nGO\nuse [other]"));
        assert!(!switches_database("SELECT 1 OPTION (USE HINT ('X'))"));
        assert!(!switches_database("SELECT 'USE x', [use] -- USE y"));
        assert!(!switches_database("SELECT used FROM t"));
        assert!(!switches_database(
            "SELECT 1 OPTION (MAXDOP 1, USE PLAN N'<x/>', USE HINT ('X'))"
        ));
        // A database named like the hint, outside an OPTION list.
        assert!(switches_database("SELECT 1 USE plan"));
        assert!(switches_database(
            "SELECT 1 OPTION (USE PLAN N'x') USE plan"
        ));
        assert!(switches_database("SELECT f(1, USE HINT)"));
        assert!(switches_database("SELECT (1) USE hint"));
    }

    /// What `RULES` cannot name without leaving the shared test spec.
    #[test]
    fn statements_the_rules_do_not_name_are_found() {
        for sql in [
            "SELECT 1 {call p}",
            "SELECT 1 {CALL p}",
            "SELECT 1 { ? = call p (1) }",
            "SELECT 1 {/* x */?=call p}",
            "SELECT 1 DUMP TRANSACTION x",
            "SELECT 1 LOAD DATABASE x",
            "SELECT 1 CALL p",
            "SELECT 1 WHERE a = 1e0DUMP TRANSACTION x",
            "SELECT 1.e5LOAD DATABASE x",
            "SELECT 1 /* a /* b */ DUMP DATABASE x */",
        ] {
            assert!(hides_statement(sql), "{sql}");
        }
        for sql in [
            "SELECT 1",
            "SELECT {fn NOW()}, {d '2024-01-01'}",
            "SELECT [call], 'DUMP', \"load\" -- CALL p",
            "SELECT payload, download FROM t",
            "SELECT 1e5",
        ] {
            assert!(!hides_statement(sql), "{sql}");
        }
    }

    #[test]
    fn passwords_stay_out_of_the_trace() {
        use crate::sqlgen::portable::redact::redact;
        for (sql, masked) in [
            (
                "CREATE LOGIN x WITH PASSWORD = N'p''w', CHECK_POLICY = OFF",
                "CREATE LOGIN x WITH PASSWORD = N'********', CHECK_POLICY = OFF",
            ),
            (
                "ALTER LOGIN x WITH PASSWORD = 'new' OLD_PASSWORD = 'old'",
                "ALTER LOGIN x WITH PASSWORD = '********' OLD_PASSWORD = '********'",
            ),
            (
                "EXEC sp_addlinkedsrvlogin @rmtsrvname = 's', @rmtuser = 'u', @rmtpassword = N'pw'",
                "EXEC sp_addlinkedsrvlogin @rmtsrvname = 's', @rmtuser = 'u', @rmtpassword = \
                 N'********'",
            ),
            (
                "CREATE CREDENTIAL c WITH IDENTITY = 'u', SECRET = 'sv=abc'",
                "CREATE CREDENTIAL c WITH IDENTITY = 'u', SECRET = '********'",
            ),
            (
                "SELECT ENCRYPTBYPASSPHRASE('phrase', x) FROM t",
                "SELECT ENCRYPTBYPASSPHRASE('********', x) FROM t",
            ),
        ] {
            assert_eq!(redact(&LEX, SPEC.knobs.redact, sql).sql, masked, "{sql}");
        }
    }

    #[test]
    fn explain_prefixes_read_back() {
        for mode in [ExplainMode::Plan, ExplainMode::Analyze] {
            let wrapped = crate::sqlgen::portable::explain::wrap_form(
                crate::sqlgen::dialect::spec::ExplainForm::Prefix(explain_prefix(mode)),
                "SQL Server",
                "SELECT 1;",
                mode,
            )
            .unwrap()
            .0;
            assert_eq!(explain_of(&wrapped), Some((mode, "SELECT 1")), "{mode:?}");
            assert!(explain_prefix(mode).starts_with(showplan(mode, true)));
        }
        // A user's own statements are not the driver's Explain.
        assert_eq!(explain_of("SET STATISTICS XML ON SELECT 1"), None);
        assert_eq!(explain_of("SET SHOWPLAN_XML ON"), None);
        assert_eq!(
            explain_of(&format!("{}  ", explain_prefix(ExplainMode::Plan))),
            None
        );
        // What the prefix holds is a write to Safe Mode, and never reaches it: the editor reads
        // the statement out first.
        assert_eq!(
            classify_tsql(explain_prefix(ExplainMode::Plan)),
            StatementClass::Write
        );
    }

    #[test]
    fn a_lone_dml_statement_is_told_from_a_batch() {
        for sql in [
            "UPDATE t SET a = 1 WHERE id = 2",
            "update t with (rowlock) set a = case when b = 1 then 2 end;",
            "DELETE FROM t OUTPUT DELETED.id WHERE id IN (SELECT id FROM u)",
            "INSERT INTO t (a) SELECT a FROM u -- SET x\n",
            "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET a = s.a \
             WHEN NOT MATCHED THEN INSERT (id) VALUES (s.id);",
            "UPDATE t SET [exec] = 'DROP x'",
            "UPDATE t SET a = 1e5, b = 0x1F \
             OPTION (USE HINT ('DISABLE_TSQL_SCALAR_UDF_INLINING'))",
            "DELETE TOP (10) FROM t WHERE b IN (SELECT c FROM u ORDER BY c OFFSET 0 ROWS \
             FETCH NEXT 5 ROWS ONLY)",
            "INSERT t DEFAULT VALUES",
            "MERGE t USING s ON t.id = s.id WHEN MATCHED AND s.gone = 1 THEN DELETE \
             WHEN NOT MATCHED BY SOURCE THEN UPDATE SET a = 0;",
            // The reads an INSERT's rows hold, and a table named like a statement.
            "INSERT INTO t (a) SELECT a FROM u UNION ALL SELECT b FROM v EXCEPT SELECT 0",
            "INSERT t SELECT TOP (2) WITH TIES a FROM u WITH (NOLOCK) ORDER BY a",
            "INSERT t SELECT a FROM u ORDER BY a OFFSET 1 ROW FETCH FIRST 1 ROW ONLY",
            "INSERT t SELECT a, COUNT(*) FROM u GROUP BY a WITH ROLLUP",
            "UPDATE statistics SET a = 1",
        ] {
            assert!(is_lone_dml(sql), "{sql}");
        }
        for sql in [
            "SELECT 1",
            "WITH c AS (SELECT 1 AS a) UPDATE c SET a = 2",
            "UPDATE t SET a = 1 UPDATE u SET b = 2",
            "DELETE FROM t\nDROP TABLE u",
            "INSERT INTO t VALUES (1); SELECT 1",
            "INSERT INTO t VALUES (1) SET NOCOUNT ON",
            "INSERT INTO t EXEC p",
            "BEGIN TRAN UPDATE t SET a = 1",
            "",
            // Every word of `RULES.writes` but the statement's own starts another statement.
            "DELETE FROM t SHUTDOWN WITH NOWAIT",
            "DELETE FROM t RECONFIGURE WITH OVERRIDE",
            "INSERT t VALUES (1) CHECKPOINT",
            "UPDATE t SET a = 1 SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
            "UPDATE t SET a = 1 SET CONTEXT_INFO 0x01",
            "UPDATE t SET a = (SELECT 1) WHERE b IN (SELECT c FROM u SET x)",
            "DELETE FROM t REVERT",
            "DELETE FROM t WITH (TABLOCKX)",
            "DELETE FROM t USE master",
            "DELETE FROM t OUTPUT deleted.id INTO log",
            "INSERT t VALUES (1) DISABLE TRIGGER x ON t",
            "MERGE t USING s ON 1 = 1 WHEN MATCHED THEN UPDATE SET a = 1 \
             SET CONTEXT_INFO 0x01;",
            "MERGE t USING s ON 1 = 1 WHEN MATCHED THEN DELETE DELETE FROM u;",
            // A read after the statement, whose `@@ROWCOUNT` would be reported instead.
            "DELETE FROM t SELECT 1",
            "INSERT t VALUES (1) SELECT @@IDENTITY",
            "INSERT t DEFAULT VALUES SELECT 1",
            "INSERT t SELECT 1 SELECT 2",
            "UPDATE t SET a = 1 FETCH NEXT FROM c",
            "DELETE FROM t WITH c AS (SELECT 1 AS a) SELECT a FROM c",
            "DELETE FROM t LINENO 0",
            "MERGE t USING s ON 1 = 1 WHEN MATCHED THEN DELETE SELECT 1;",
            // Not DML at all.
            "UPDATE STATISTICS t",
            "UPDATE STATISTICS t WITH FULLSCAN",
            // Words a number may hide, and comments another reader may end elsewhere.
            "DELETE FROM t WHERE a = 1SELECT 1",
            "UPDATE t SET a = 1 WHERE b = 1e0SHUTDOWN",
            "UPDATE t SET a = 1.e5SET CONTEXT_INFO 0x01",
            "DELETE FROM t /* a /* b */ SHUTDOWN */",
            // A switch to a database named like a hint, and statements `RULES` does not name.
            "DELETE FROM t OPTION (USE PLAN N'x') USE plan",
            "DELETE FROM t USE hint",
            "DELETE FROM t WHERE 1 = 1 {call p}",
            "DELETE FROM t DUMP TRANSACTION x",
        ] {
            assert!(!is_lone_dml(sql), "{sql}");
        }
    }

    /// Read-only Explain plans a lone DML statement only when it reaches no other server.
    #[test]
    fn a_plan_that_calls_out_is_not_let_through() {
        for sql in [
            "DELETE FROM t WHERE id IN (SELECT id FROM u)",
            "UPDATE t SET [openquery] = 1",
        ] {
            assert!(plan_writes_nothing(sql), "{sql}");
        }
        for sql in [
            "DELETE FROM t WHERE id IN (SELECT id FROM \
             OPENQUERY(srv, 'EXEC dbo.purge; SELECT 1 AS id'))",
            "DELETE FROM OPENQUERY(srv, 'SELECT * FROM remote_t')",
            "INSERT t VALUES ((SELECT TOP (1) a FROM OPENROWSET('SQLNCLI', \
             'Server=.;Trusted_Connection=yes', 'SET FMTONLY OFF EXEC dbo.purge SELECT 1 a')))",
            "DELETE FROM t WHERE EXISTS (SELECT 1 FROM OPENDATASOURCE('SQLNCLI', \
             'Data Source=x;Integrated Security=SSPI').db.dbo.t2)",
            "DELETE FROM t WHERE 1 = (SELECT master.dbo.xp_foo())",
            "INSERT t VALUES (NEXT VALUE FOR s)",
            "DELETE FROM t SHUTDOWN",
            "DELETE FROM t WHERE 1 = 1 {call p}",
            "DELETE FROM t OPTION (USE PLAN N'x') USE plan",
        ] {
            assert!(!plan_writes_nothing(sql), "{sql}");
        }
    }

    /// The driver keeps binary values whole for `fetch_cell`'s statement only.
    #[test]
    fn only_fetch_cell_reads_one_cell() {
        let t = TableRef::new(Some("dbo"), "t");
        let cols = columns();
        let pk = ["id".to_string()];
        let target = TableTarget {
            table: &t,
            columns: &cols,
        };
        let key = RowKey::Pk(map([("id", Value::Int(7))]));
        let (fetch, _) = select::build_fetch_cell(&SPEC, &target, by(&cols, "data"), &key).unwrap();
        assert!(reads_one_cell(&fetch), "{fetch}");
        let page = select::build_select(&SPEC, &query(&t, &cols, None, &[], &pk)).unwrap();
        assert!(!reads_one_cell(&page.sql));
        let count = select::build_count(&SPEC, &t, &cols, None).unwrap();
        assert!(!reads_one_cell(&count.sql));
    }

    #[test]
    fn create_table_scripts_what_the_catalog_says() {
        use crate::types::schema::{CheckMeta, ForeignKeyMeta, TableKind, TableMeta, TriggerMeta};
        let mut id = col("Id", "int", ValueKind::Int);
        id.nullable = false;
        id.extra = Some("IDENTITY(1,1)".into());
        id.is_pk = true;
        let mut name = col("Name", "nvarchar(50)", ValueKind::Text);
        name.collation = Some("Latin1_General_CI_AS".into());
        name.default_expr = Some("N'x'".into());
        name.comment = Some("It's the name".into());
        let mut twice = col("Twice", "int", ValueKind::Int);
        twice.extra = Some("AS ([Id]*(2)) PERSISTED".into());
        let mut from = col("From", "datetime2(7)", ValueKind::Timestamp);
        from.nullable = false;
        from.extra = Some("GENERATED ALWAYS AS ROW START".into());
        let mut to = col("To", "datetime2(7)", ValueKind::Timestamp);
        to.nullable = false;
        to.extra = Some("GENERATED ALWAYS AS ROW END".into());
        let index = |name: &str, columns: &[&str], primary: bool, method: &str| IndexMeta {
            name: name.into(),
            columns: columns.iter().map(|c| c.to_string()).collect(),
            unique: primary,
            primary,
            method: Some(method.into()),
            predicate: None,
            definition: None,
            comment: None,
            include: Vec::new(),
        };
        let mut by_name = index("IX_Name", &["Name"], false, "nonclustered");
        by_name.include = vec!["Twice".into()];
        by_name.predicate = Some("[Name]<>N''".into());
        let meta = TableMeta {
            table: TableRef::new(Some("dbo"), "Pe]ople"),
            kind: TableKind::Table,
            columns: vec![id, name, twice, from, to],
            primary_key: vec!["Id".into()],
            indexes: vec![
                index("PK_People", &["Id"], true, "clustered"),
                by_name,
                index("CS", &[], false, "clustered columnstore"),
            ],
            foreign_keys: vec![ForeignKeyMeta {
                name: "FK_Boss".into(),
                columns: vec!["Twice".into()],
                ref_table: TableRef::new(Some("hr"), "Boss"),
                ref_columns: vec!["Id".into()],
                on_update: "NO ACTION".into(),
                on_delete: "SET NULL".into(),
                definition: None,
            }],
            triggers: vec![TriggerMeta {
                name: "trg".into(),
                timing: Some("AFTER".into()),
                events: vec!["INSERT".into()],
                enabled: false,
                definition: "CREATE TRIGGER trg ON dbo.People AFTER INSERT AS SELECT 1\n".into(),
            }],
            comment: None,
            row_estimate: None,
            ddl: None,
            checks: vec![CheckMeta {
                name: "CK_Id".into(),
                expression: "[Id]>(0)".into(),
                columns: vec!["Id".into()],
            }],
            options: None,
        };
        assert_eq!(
            create_table(&meta),
            "CREATE TABLE [dbo].[Pe]]ople] (\n    \
             [Id] int IDENTITY(1,1) NOT NULL,\n    \
             [Name] nvarchar(50) COLLATE Latin1_General_CI_AS NULL DEFAULT (N'x'),\n    \
             [Twice] AS ([Id]*(2)) PERSISTED,\n    \
             [From] datetime2(7) GENERATED ALWAYS AS ROW START NOT NULL,\n    \
             [To] datetime2(7) GENERATED ALWAYS AS ROW END NOT NULL,\n    \
             PERIOD FOR SYSTEM_TIME ([From], [To]),\n    \
             CONSTRAINT [PK_People] PRIMARY KEY CLUSTERED ([Id]),\n    \
             CONSTRAINT [FK_Boss] FOREIGN KEY ([Twice]) REFERENCES [hr].[Boss] ([Id]) ON \
             DELETE SET NULL,\n    \
             CONSTRAINT [CK_Id] CHECK ([Id]>(0))\n\
             );\n\
             GO\n\
             CREATE NONCLUSTERED INDEX [IX_Name] ON [dbo].[Pe]]ople] ([Name]) INCLUDE ([Twice]) \
             WHERE [Name]<>N'';\n\
             GO\n\
             CREATE CLUSTERED COLUMNSTORE INDEX [CS] ON [dbo].[Pe]]ople];\n\
             GO\n\
             CREATE TRIGGER trg ON dbo.People AFTER INSERT AS SELECT 1\n\
             GO\n\
             DISABLE TRIGGER [trg] ON [dbo].[Pe]]ople];\n\
             GO\n\
             EXEC sys.sp_addextendedproperty @name = N'MS_Description', @value = N'It''s the \
             name', @level0type = N'SCHEMA', @level0name = N'dbo', @level1type = N'TABLE', \
             @level1name = N'Pe]ople', @level2type = N'COLUMN', @level2name = N'Name';\n"
        );
    }

    #[test]
    fn driver_statements() {
        assert_eq!(kill(52).unwrap(), "KILL 52");
        assert_eq!(kill(37).unwrap(), "KILL 37");
        assert!(kill(0).is_err() && kill(-5).is_err() && kill(1 << 40).is_err());
        assert_eq!(showplan(ExplainMode::Plan, true), "SET SHOWPLAN_XML ON");
        assert_eq!(
            showplan(ExplainMode::Analyze, false),
            "SET STATISTICS XML OFF"
        );
        assert_eq!(object_name(Some("dbo"), "a.b]"), "[dbo].[a.b]]]");
        assert_eq!(object_name(None, "t"), "[t]");
        assert_eq!(
            counted("UPDATE t SET a = @P1 WHERE id = @P2 ;; "),
            "UPDATE t SET a = @P1 WHERE id = @P2;\nSELECT @@ROWCOUNT"
        );
        // The driver's own statements are writes to Safe Mode, and never reach it as reads.
        for sql in [SESSION_SETUP, ROLLBACK_IF_OPEN] {
            assert_eq!(classify_tsql(sql), StatementClass::Write);
        }
        assert_eq!(classify_tsql(AFTER_BATCH), StatementClass::Read);
    }
}
