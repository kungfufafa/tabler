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
//! The statements the driver sends for its own work (session setup, `USE`, `KILL`, showplan) are
//! written here too, so its SQL text comes from sqlgen like every other.

use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, PLAIN, TokenKind, scan};
use crate::sqlgen::dialect::spec::{
    DialectSpec, LimitOne, NullSafeEq, NullsSort, Paging, PlaceholderStyle, RedactRule, Returning,
    SqlHooks, SqlKnobs, bracketed, explain_unsupported, hex_0x, national_quoted, ph_at_p,
};
use crate::sqlgen::portable::classify;
use crate::types::mutate::{Clock, ServerExpr};
use crate::types::query::ExplainMode;
use crate::types::schema::{ColumnMeta, ValueKind};

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
/// terminator has to come before it (`SELECT 1 USE other`). `USE HINT` and `USE PLAN` are query
/// hints.
pub fn switches_database(sql: &str) -> bool {
    let s = scan(&LEX, PLAIN, sql);
    let words: Vec<String> = s
        .tokens
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::Space | TokenKind::Comment))
        .map(|t| match t.kind {
            TokenKind::Word => s.upper(t),
            _ => String::new(),
        })
        .collect();
    words.iter().enumerate().any(|(i, w)| {
        w == "USE" && !matches!(words.get(i + 1).map(String::as_str), Some("HINT" | "PLAN"))
    })
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
        side_effect_functions: &["OPENQUERY", "OPENROWSET", "OPENDATASOURCE", "XP_*", "SP_*"],
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

/// The editor's transaction depth (exact, unlike guessing from the statements).
pub const TRANCOUNT: &str = "SELECT @@TRANCOUNT";

/// Rolls back what an Attention left open, when the session is in a transaction it did not start.
pub const ROLLBACK_IF_OPEN: &str = "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION";

/// A statement followed by its row count, as the last result set. `@@ROWCOUNT` counts the rows
/// of the statement itself, where the DONE counts the client sums also hold its triggers'.
pub fn counted(sql: &str) -> String {
    let mut s = sql.trim_end();
    while let Some(rest) = s.strip_suffix(';') {
        s = rest.trim_end();
    }
    format!("{s};\nSELECT @@ROWCOUNT")
}

/// `USE [db]`: switches a pooled connection to another database (not on Azure SQL Database,
/// where a connection belongs to one database).
pub fn use_database(name: &str) -> String {
    format!("USE {}", bracketed(name))
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
pub fn showplan(mode: ExplainMode, on: bool) -> String {
    let what = match mode {
        ExplainMode::Plan => "SHOWPLAN_XML",
        ExplainMode::Analyze => "STATISTICS XML",
    };
    format!("SET {what} {}", if on { "ON" } else { "OFF" })
}

/// The argument of `OBJECT_ID(@P1)` for a table: its name quoted and qualified, so dots and
/// brackets in names stay names.
pub fn object_name(schema: Option<&str>, name: &str) -> String {
    match schema {
        Some(s) => format!("{}.{}", bracketed(s), bracketed(name)),
        None => bracketed(name),
    }
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
    fn driver_statements() {
        assert_eq!(use_database("a]b"), "USE [a]]b]");
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
        assert_eq!(classify_tsql(TRANCOUNT), StatementClass::Read);
    }
}
