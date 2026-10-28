//! CQL (the cassandra lane): Apache Cassandra 3.0+ and ScyllaDB. Built with `drv-cassandra` only.
//!
//! Double-quoted names (`""` doubled), single-quoted strings (`''` doubled; a backslash is a plain
//! character), `$$…$$` bodies without tags, `--`, `//` and flat `/* */` comments, `?` binds. The
//! server runs one statement per request, and a batch (`BEGIN [UNLOGGED | COUNTER] BATCH …
//! APPLY BATCH`) is one statement whose inner `;` end nothing, so the editor splits with
//! `split_cql`.
//!
//! CQL filters and sorts very little: there is no casting in a WHERE, no NULL ordering, no
//! case-insensitive match, no OR and no offset (later pages need the driver's paging state).
//! Writes are upserts with no row count, so the lane proves them (lightweight transactions)
//! before EditRows is on. The DDL text of `table_meta` (when the server has no DESCRIBE) is
//! rebuilt here from `system_schema` rows.

use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, PLAIN, TokenKind, scan};
use crate::sqlgen::dialect::spec::{
    CiLike, DialectSpec, NullSafeEq, NullsSort, Paging, PlaceholderStyle, RedactRule, SqlHooks,
    SqlKnobs, double_quoted, explain_unsupported, hex_0x, ph_question, single_quoted,
};
use crate::sqlgen::portable::{classify, split};
use crate::sqlgen::split::SplitStatement;
use crate::types::mutate::ServerExpr;
use crate::types::schema::{ColumnMeta, ValueKind};

pub static LEX: LexProfile = LexProfile {
    ident_quotes: &[('"', '"')],
    string_quotes: &['\''],
    // CQL strings take no backslash escapes, on every server.
    readings: &[PLAIN],
    e_strings: false,
    dollar_quotes: true,
    // CQL has only `$$` bodies, and `$a$` is no token at all.
    dollar_tags: false,
    triple_quotes: false,
    raw_prefixes: false,
    byte_prefixes: false,
    hash_comments: false,
    slash_slash_comments: true,
    // Cassandra's and ScyllaDB's lexers end a block comment at its first `*/`.
    nested_block_comments: false,
    dash_comment_needs_space: false,
    batch_separator: None,
    terminator_optional: false,
    // A batch ends with `APPLY BATCH`, not `END`: `split_cql` keeps it whole.
    begin_end_bodies: false,
};

pub static RULES: ClassifyRules = ClassifyRules {
    queries: &["SELECT"],
    // Server-side DESCRIBE (Cassandra 4.0+, ScyllaDB 5.2+) and LIST ROLES / USERS / PERMISSIONS
    // only read; USE switches the keyspace. CONSISTENCY, SERIAL CONSISTENCY, TRACING, PAGING and
    // EXPAND are cqlsh's commands: the session handles them itself (`cassandra::cqlsh`, which
    // refuses trailing text), and the server refuses them as syntax errors.
    // An inspection reads whatever follows it (`USE ks DELETE FROM t` is a Read). That is safe
    // here, for these words and the cqlsh ones alike: Cassandra and ScyllaDB parse exactly one
    // statement per request (`cqlStatement ';'* EOF`), so text after it is a syntax error and
    // nothing runs.
    inspections: &[
        "DESCRIBE",
        "DESC",
        "USE",
        "LIST",
        "CONSISTENCY",
        "SERIAL",
        "TRACING",
        "PAGING",
        "EXPAND",
    ],
    // All reserved in CQL, so no read can use one as a bare name.
    writes: &[
        "INTO", "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "ALTER", "TRUNCATE", "BATCH",
        "APPLY", "BEGIN", "GRANT", "REVOKE",
    ],
    // CQL functions cannot write (UDFs run sandboxed), and there are no table functions.
    write_calls: &[],
    table_functions: None,
};

fn classify_cql(sql: &str) -> StatementClass {
    classify::with(&LEX, &RULES, sql)
}

/// No column can be filtered as text: a WHERE takes no casts.
fn no_text(_: &str, _: &ColumnMeta) -> Option<String> {
    None
}

/// The clocks CQL can write: a new time-based UUID for a timeuuid column, and the timestamp and
/// date of one (`toTimestamp(now())`, `toDate(now())`, which Cassandra 2.2+ and ScyllaDB take).
/// CQL timestamps hold milliseconds, so a precision is not offered.
fn cql_server_expr(expr: ServerExpr, kind: ValueKind) -> Option<String> {
    let text = match (expr, kind) {
        (ServerExpr::Now, ValueKind::Uuid) => "now()",
        (
            ServerExpr::Now | ServerExpr::CurrentTimestamp,
            ValueKind::Timestamp | ValueKind::TimestampTz,
        ) => "toTimestamp(now())",
        (ServerExpr::CurrentDate, ValueKind::Date) => "toDate(now())",
        _ => return None,
    };
    Some(text.into())
}

/// Splits editor text into the statements the server runs: the portable split, with a batch
/// (`BEGIN [UNLOGGED | COUNTER] BATCH` up to the statement that ends with `APPLY BATCH`, or the
/// end of the text) kept as one statement.
pub fn split_cql(text: &str) -> Vec<SplitStatement> {
    let parts = split::split(&LEX, text);
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::with_capacity(parts.len());
    let mut parts = parts.into_iter();
    while let Some(first) = parts.next() {
        let words = code_words(&first.sql);
        if !opens_batch(&words) || closes_batch(&words) {
            out.push(first);
            continue;
        }
        let mut end = first.end;
        for next in parts.by_ref() {
            end = next.end;
            if closes_batch(&code_words(&next.sql)) {
                break;
            }
        }
        out.push(SplitStatement {
            sql: chars[first.start..end].iter().collect(),
            start: first.start,
            end,
            line: first.line,
        });
    }
    out
}

/// The tokens of code of a statement, upper case: words as they are, any other token as its
/// kind (strings, quoted names and punctuation never count as keywords).
fn code_words(sql: &str) -> Vec<String> {
    let s = scan(&LEX, PLAIN, sql);
    s.tokens
        .iter()
        .filter_map(|t| match t.kind {
            TokenKind::Space | TokenKind::Comment => None,
            TokenKind::Word => Some(s.upper(t)),
            TokenKind::Ident | TokenKind::Str | TokenKind::Punct => Some(String::new()),
        })
        .collect()
}

/// `BEGIN [UNLOGGED | COUNTER] BATCH …`
fn opens_batch(words: &[String]) -> bool {
    let w = |i: usize| words.get(i).map(String::as_str);
    w(0) == Some("BEGIN")
        && (w(1) == Some("BATCH")
            || (matches!(w(1), Some("UNLOGGED" | "COUNTER")) && w(2) == Some("BATCH")))
}

/// `… APPLY BATCH`
fn closes_batch(words: &[String]) -> bool {
    matches!(words, [.., a, b] if a == "APPLY" && b == "BATCH")
}

pub static SPEC: DialectSpec = DialectSpec {
    dialect: SqlDialect::Cql,
    label: "CQL",
    quote_ident: double_quoted,
    quote_literal: single_quoted,
    quote_literal_exec: single_quoted,
    placeholder: ph_question,
    placeholder_style: PlaceholderStyle::Question,
    placeholders_reusable: false,
    max_params: 1000,
    max_name_parts: 2,
    lex: &LEX,
    rules: &RULES,
    classify: classify_cql,
    knobs: SqlKnobs {
        bool_literal: ["false", "true"],
        bytes_literal: hex_0x,
        text_expr: no_text,
        paging: Paging::LimitOnly,
        nulls_order: NullsSort::Unsupported,
        ci_like: CiLike::Unsupported,
        // SASI's LIKE knows `%` at either end only, and nothing escapes it.
        like_escape: None,
        like_wildcards: &['%'],
        null_safe_eq: NullSafeEq::OrForm,
        insert_all_defaults: None,
        set_default: false,
        server_expr: cql_server_expr,
        explain: explain_unsupported,
        catalog_words: &[
            "SYSTEM_SCHEMA",
            "SYSTEM.",
            "SYSTEM_VIEWS",
            "SYSTEM_VIRTUAL_SCHEMA",
            "SYSTEM_AUTH",
            "SYSTEM_DISTRIBUTED",
            "SYSTEM_TRACES",
        ],
        redact: &[
            // `CREATE ROLE r WITH PASSWORD = '…'`, `CREATE USER u WITH PASSWORD '…'`, and
            // Cassandra 4.1's `HASHED PASSWORD = '…'`.
            RedactRule {
                keyword: "PASSWORD",
            },
        ],
        // cqlsh's `LOGIN user 'password'`, at the start of a statement only: `WHERE login =
        // 'bob'` and `WITH LOGIN = true` are no command.
        redact_commands: &["LOGIN"],
        cancel_notice: "what it changed before it stopped stays changed (Cassandra has no \
                        transactions)",
        ..SqlKnobs::ANSI
    },
    hooks: SqlHooks {
        split: Some(split_cql),
        ..SqlHooks::NONE
    },
    ddl: None,
};

// Schema text rebuilt from `system_schema` rows (the DDL of `table_meta` when the server has no
// DESCRIBE). Names are always quoted; type texts are the server's own (`frozen<list<text>>`).

/// `"ks"."name"`
pub fn qualified(keyspace: &str, name: &str) -> String {
    format!("{}.{}", double_quoted(keyspace), double_quoted(name))
}

/// `DESCRIBE TABLE "ks"."t"` (server-side on Cassandra 4.0+ and ScyllaDB 5.2+).
pub fn describe_table(keyspace: &str, table: &str) -> String {
    format!("DESCRIBE TABLE {}", qualified(keyspace, table))
}

/// `DESCRIBE MATERIALIZED VIEW "ks"."v"`
pub fn describe_view(keyspace: &str, view: &str) -> String {
    format!("DESCRIBE MATERIALIZED VIEW {}", qualified(keyspace, view))
}

/// `{'k': 'v', …}` of text pairs, in the order given.
pub fn map_literal<K: AsRef<str>, V: AsRef<str>>(pairs: &[(K, V)]) -> String {
    let items: Vec<String> = pairs
        .iter()
        .map(|(k, v)| {
            format!(
                "{}: {}",
                single_quoted(k.as_ref()),
                single_quoted(v.as_ref())
            )
        })
        .collect();
    format!("{{{}}}", items.join(", "))
}

/// The part a column plays in its table's primary key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRole {
    Partition,
    Clustering { desc: bool },
    Static,
    Regular,
}

/// A column of a rebuilt `CREATE TABLE`, in table order (partition key, clustering columns, then
/// the rest).
#[derive(Debug, Clone, Copy)]
pub struct DdlColumn<'a> {
    pub name: &'a str,
    /// The server's type text.
    pub cql_type: &'a str,
    pub role: KeyRole,
}

/// A table option of a rebuilt `CREATE TABLE`: its name and its value as CQL (a quoted string,
/// a number, a map literal).
#[derive(Debug, Clone)]
pub struct DdlOption {
    pub name: String,
    pub value: String,
}

/// `PRIMARY KEY ((pk…), ck…)` of the columns; None without a partition key.
fn primary_key(columns: &[DdlColumn<'_>]) -> Option<String> {
    let names = |f: fn(&KeyRole) -> bool| -> Vec<String> {
        columns
            .iter()
            .filter(|c| f(&c.role))
            .map(|c| double_quoted(c.name))
            .collect()
    };
    let partition = names(|r| *r == KeyRole::Partition);
    if partition.is_empty() {
        return None;
    }
    let mut key = vec![format!("({})", partition.join(", "))];
    key.extend(names(|r| matches!(r, KeyRole::Clustering { .. })));
    Some(format!("PRIMARY KEY ({})", key.join(", ")))
}

/// `CLUSTERING ORDER BY (ck ASC|DESC, …)`; None without clustering columns.
fn clustering_order(columns: &[DdlColumn<'_>]) -> Option<String> {
    let order: Vec<String> = columns
        .iter()
        .filter_map(|c| match c.role {
            KeyRole::Clustering { desc } => Some(format!(
                "{} {}",
                double_quoted(c.name),
                if desc { "DESC" } else { "ASC" }
            )),
            _ => None,
        })
        .collect();
    (!order.is_empty()).then(|| format!("CLUSTERING ORDER BY ({})", order.join(", ")))
}

/// `CREATE TABLE "ks"."t" (…, PRIMARY KEY ((pk…), ck…)) WITH CLUSTERING ORDER BY (…) AND …;`
pub fn create_table(
    keyspace: &str,
    table: &str,
    columns: &[DdlColumn<'_>],
    options: &[DdlOption],
) -> String {
    let mut lines: Vec<String> = columns
        .iter()
        .map(|c| {
            let fixed = if c.role == KeyRole::Static {
                " static"
            } else {
                ""
            };
            format!("    {} {}{fixed}", double_quoted(c.name), c.cql_type)
        })
        .collect();
    lines.extend(primary_key(columns).map(|k| format!("    {k}")));
    let mut with: Vec<String> = clustering_order(columns).into_iter().collect();
    with.extend(options.iter().map(|o| format!("{} = {}", o.name, o.value)));
    let mut out = format!(
        "CREATE TABLE {} (\n{}\n)",
        qualified(keyspace, table),
        lines.join(",\n")
    );
    if !with.is_empty() {
        out.push_str(" WITH ");
        out.push_str(&with.join("\n    AND "));
    }
    out.push(';');
    out
}

/// What a secondary index covers (`options.target`): a column, or a part of a collection one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexTarget {
    /// `"c"`, or `keys("m")`, `values("m")`, `entries("m")`, `full("l")`.
    Column {
        part: Option<&'static str>,
        column: String,
    },
    /// A ScyllaDB local index: `(("pk"…), "c"…)`.
    Local {
        partition: Vec<String>,
        columns: Vec<String>,
    },
}

impl IndexTarget {
    /// The columns the index names, partition columns first.
    pub fn columns(&self) -> Vec<String> {
        match self {
            IndexTarget::Column { column, .. } => vec![column.clone()],
            IndexTarget::Local { partition, columns } => {
                partition.iter().chain(columns).cloned().collect()
            }
        }
    }

    fn cql(&self) -> String {
        match self {
            IndexTarget::Column { part: None, column } => double_quoted(column),
            IndexTarget::Column {
                part: Some(p),
                column,
            } => format!("{p}({})", double_quoted(column)),
            IndexTarget::Local { partition, columns } => {
                let quoted =
                    |v: &[String]| -> Vec<String> { v.iter().map(|c| double_quoted(c)).collect() };
                let mut parts = vec![format!("({})", quoted(partition).join(", "))];
                parts.extend(quoted(columns));
                parts.join(", ")
            }
        }
    }
}

/// `CREATE INDEX "i" ON "ks"."t" (target)`, or with a class a `CREATE CUSTOM INDEX … USING
/// 'class'` and its other options.
pub fn create_index(
    keyspace: &str,
    table: &str,
    index: &str,
    target: &IndexTarget,
    class: Option<&str>,
    options: &[(String, String)],
) -> String {
    let on = format!(
        "{} ON {} ({})",
        double_quoted(index),
        qualified(keyspace, table),
        target.cql()
    );
    match class {
        None => format!("CREATE INDEX {on};"),
        Some(class) if options.is_empty() => {
            format!("CREATE CUSTOM INDEX {on} USING {};", single_quoted(class))
        }
        Some(class) => format!(
            "CREATE CUSTOM INDEX {on} USING {} WITH OPTIONS = {};",
            single_quoted(class),
            map_literal(options)
        ),
    }
}

/// `CREATE TRIGGER "tr" ON "ks"."t" USING 'class';`
pub fn create_trigger(keyspace: &str, table: &str, trigger: &str, class: &str) -> String {
    format!(
        "CREATE TRIGGER {} ON {} USING {};",
        double_quoted(trigger),
        qualified(keyspace, table),
        single_quoted(class)
    )
}

/// `CREATE MATERIALIZED VIEW "ks"."v" AS SELECT … FROM "ks"."base" WHERE … PRIMARY KEY (…)
/// WITH CLUSTERING ORDER BY (…);`. `where_clause` is the server's text of the view's filter.
pub fn create_view(
    keyspace: &str,
    view: &str,
    base: &str,
    columns: &[DdlColumn<'_>],
    include_all: bool,
    where_clause: &str,
) -> String {
    let selected = if include_all {
        "*".to_string()
    } else {
        columns
            .iter()
            .map(|c| double_quoted(c.name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut out = format!(
        "CREATE MATERIALIZED VIEW {} AS\n    SELECT {selected}\n    FROM {}",
        qualified(keyspace, view),
        qualified(keyspace, base)
    );
    let filter = where_clause.trim();
    if !filter.is_empty() {
        out.push_str("\n    WHERE ");
        out.push_str(filter);
    }
    if let Some(key) = primary_key(columns) {
        out.push_str("\n    ");
        out.push_str(&key);
    }
    if let Some(order) = clustering_order(columns) {
        out.push_str(" WITH ");
        out.push_str(&order);
    }
    out.push(';');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_bindings_lex_cql() {
        crate::sqlgen::dialect::lex::export_lex("cql", &LEX, &RULES);
    }

    #[test]
    fn quotes_and_classifies() {
        assert_eq!((SPEC.quote_ident)("MixedCase\""), "\"MixedCase\"\"\"");
        assert_eq!((SPEC.quote_literal)("it's C:\\"), "'it''s C:\\'");
        let read = |sql: &str| assert_eq!(classify_cql(sql), StatementClass::Read, "{sql:?}");
        let write = |sql: &str| assert_eq!(classify_cql(sql), StatementClass::Write, "{sql:?}");
        read("SELECT * FROM ks.t WHERE k = 1 ALLOW FILTERING");
        read("SELECT JSON k, ttl(v), writetime(v) FROM t");
        read("DESCRIBE KEYSPACES");
        read("LIST ROLES");
        read("CONSISTENCY LOCAL_QUORUM");
        read("SERIAL CONSISTENCY LOCAL_SERIAL");
        read("SELECT $$; DELETE FROM t; $$ FROM t");
        read("SELECT 'a\\' FROM t");
        // An inspection reads whatever follows it; the server refuses such text whole (one
        // statement per request), and the cqlsh commands refuse it too (`cassandra::cqlsh`).
        read("USE ks DELETE FROM t");
        read("SERIAL CONSISTENCY SERIAL\nDELETE FROM t WHERE k = 1");
        read("PAGING 10 INSERT INTO t (k) VALUES (1)");
        read("EXPAND ON\r\nUPDATE t SET v = 1 WHERE k = 1");
        // A duration glued to a keyword is one word.
        read("SELECT * FROM t WHERE d = P1DT2HDELETE");
        write("BEGIN BATCH INSERT INTO t (k) VALUES (1); APPLY BATCH");
        write("BEGIN UNLOGGED BATCH UPDATE t SET v = 1 WHERE k = 1 APPLY BATCH");
        write("INSERT INTO t (k) VALUES (1) IF NOT EXISTS");
        write("DELETE FROM t WHERE k = 1 IF EXISTS");
        write("SELECT 1 FROM t; UPDATE t USING TTL 10 SET v = 1 WHERE k = 1");
        // A backslash does not escape the quote: the DELETE is code.
        write("SELECT 'a\\'; DELETE FROM t; --'");
        write("PRUNE MATERIALIZED VIEW v WHERE k = 1");
        write("COPY t TO 'x.csv'");
        write("SOURCE 'x.cql'");
    }

    #[test]
    fn redacts_passwords_and_cqlsh_login() {
        let redact = |sql: &str| crate::sqlgen::portable::redact::redact_spec(&SPEC, sql);
        let r = redact("LOGIN alice 'se''cret'");
        assert_eq!(r.sql, "LOGIN alice '********'");
        assert_eq!(r.secrets, ["se'cret", "'se''cret'"]);
        assert_eq!(
            redact("SELECT * FROM t WHERE k = 1; LOGIN 'alice' 'pw'").sql,
            "SELECT * FROM t WHERE k = 1; LOGIN 'alice' '********'"
        );
        // cqlsh prompts for the password when the line has none.
        assert_eq!(redact("LOGIN alice").sql, "LOGIN alice");
        // LOGIN is the command only at the start of a statement.
        for sql in [
            "SELECT * FROM users WHERE login = 'bob' ALLOW FILTERING",
            "UPDATE users SET login = 'bob' WHERE k = 1",
        ] {
            assert_eq!(redact(sql).sql, sql, "{sql}");
        }
        assert_eq!(
            redact("CREATE ROLE r WITH PASSWORD = 'x' AND LOGIN = true").sql,
            "CREATE ROLE r WITH PASSWORD = '********' AND LOGIN = true"
        );
        assert_eq!(
            redact("create role r with login = true and password = 'x\\'").sql,
            "create role r with login = true and password = '********'"
        );
        assert_eq!(
            redact("CREATE USER u WITH PASSWORD 'p' NOSUPERUSER").sql,
            "CREATE USER u WITH PASSWORD '********' NOSUPERUSER"
        );
        let r = redact("ALTER ROLE r WITH HASHED PASSWORD = '$2a$10$JSJEMFm6GeaW9XxT5JIheu'");
        assert_eq!(r.sql, "ALTER ROLE r WITH HASHED PASSWORD = '********'");
        assert!(
            r.secrets
                .contains(&"$2a$10$JSJEMFm6GeaW9XxT5JIheu".to_string())
        );
        let r = redact("CREATE ROLE r WITH PASSWORD = $$pw$$ AND LOGIN = true");
        assert_eq!(
            r.sql,
            "CREATE ROLE r WITH PASSWORD = '********' AND LOGIN = true"
        );
        assert_eq!(r.secrets, ["pw", "$$pw$$"]);
        let r = redact("ALTER ROLE r WITH PASSWORD = $$p'w$$");
        assert_eq!(r.secrets, ["p'w", "$$p'w$$"]);
        // A password word in a string, a name or a comment masks nothing.
        for sql in [
            "SELECT 'password' FROM t",
            "SELECT \"login\" FROM t WHERE k = 'a'",
            "SELECT k FROM t -- LOGIN alice 'x'\n",
        ] {
            assert_eq!(redact(sql).sql, sql, "{sql:?}");
        }
    }

    #[test]
    fn batches_split_as_one_statement() {
        let texts =
            |text: &str| -> Vec<String> { split_cql(text).into_iter().map(|s| s.sql).collect() };
        let script = "SELECT 1 FROM t;\nBEGIN BATCH\n  INSERT INTO t (k) VALUES (1);\n  \
                      UPDATE t SET v = ';' WHERE k = 2;\nAPPLY BATCH;\nSELECT 2 FROM t;";
        assert_eq!(
            texts(script),
            [
                "SELECT 1 FROM t",
                "BEGIN BATCH\n  INSERT INTO t (k) VALUES (1);\n  UPDATE t SET v = ';' WHERE \
                 k = 2;\nAPPLY BATCH",
                "SELECT 2 FROM t",
            ]
        );
        let parts = split_cql(script);
        assert_eq!((parts[1].line, parts[2].line), (2, 6));
        let chars: Vec<char> = script.chars().collect();
        for p in &parts {
            assert_eq!(chars[p.start..p.end].iter().collect::<String>(), p.sql);
        }
        // Counter and unlogged batches, a batch closed in its own statement, a batch never
        // closed, and APPLY BATCH glued to the last statement.
        assert_eq!(
            texts("begin counter batch update c set n = n + 1 where k = 1; apply batch; SELECT 1"),
            [
                "begin counter batch update c set n = n + 1 where k = 1; apply batch",
                "SELECT 1"
            ]
        );
        assert_eq!(
            texts("BEGIN BATCH DELETE FROM t WHERE k = 1 APPLY BATCH; SELECT 1"),
            [
                "BEGIN BATCH DELETE FROM t WHERE k = 1 APPLY BATCH",
                "SELECT 1"
            ]
        );
        assert_eq!(
            texts("BEGIN UNLOGGED BATCH INSERT INTO t (k) VALUES (1); SELECT 1"),
            ["BEGIN UNLOGGED BATCH INSERT INTO t (k) VALUES (1); SELECT 1"]
        );
        assert_eq!(
            texts(
                "BEGIN BATCH INSERT INTO t (k) VALUES (1); DELETE FROM t WHERE k = 2 APPLY BATCH; SELECT 1"
            ),
            [
                "BEGIN BATCH INSERT INTO t (k) VALUES (1); DELETE FROM t WHERE k = 2 APPLY BATCH",
                "SELECT 1"
            ]
        );
        // Words in strings, names, bodies and comments open or close nothing.
        assert_eq!(
            texts(
                "SELECT 'BEGIN BATCH' FROM t; SELECT \"apply\", $$APPLY BATCH$$ FROM t -- BEGIN BATCH\n; SELECT 3"
            ),
            [
                "SELECT 'BEGIN BATCH' FROM t",
                "SELECT \"apply\", $$APPLY BATCH$$ FROM t -- BEGIN BATCH",
                "SELECT 3"
            ]
        );
        assert_eq!(
            texts("BEGIN BATCH INSERT INTO t (k) VALUES ('APPLY BATCH'); APPLY BATCH"),
            ["BEGIN BATCH INSERT INTO t (k) VALUES ('APPLY BATCH'); APPLY BATCH"]
        );
        // The facade sends CQL here.
        assert_eq!(
            crate::sqlgen::sql::split_statements(SqlDialect::Cql, "BEGIN BATCH x; APPLY BATCH")
                .len(),
            1
        );
    }

    #[test]
    fn server_expressions_are_the_cql_clocks() {
        let at = |e, k| cql_server_expr(e, k);
        assert_eq!(
            at(ServerExpr::Now, ValueKind::Uuid).as_deref(),
            Some("now()")
        );
        assert_eq!(
            at(ServerExpr::Now, ValueKind::TimestampTz).as_deref(),
            Some("toTimestamp(now())")
        );
        assert_eq!(
            at(ServerExpr::CurrentDate, ValueKind::Date).as_deref(),
            Some("toDate(now())")
        );
        assert_eq!(at(ServerExpr::CurrentTime, ValueKind::Time), None);
        assert_eq!(at(ServerExpr::LocalTimestamp, ValueKind::Timestamp), None);
        assert_eq!(at(ServerExpr::Now, ValueKind::Text), None);
        assert!((SPEC.knobs.switch_db)("use other_ks"));
    }

    #[test]
    fn rebuilds_create_table() {
        let cols = [
            DdlColumn {
                name: "user_id",
                cql_type: "uuid",
                role: KeyRole::Partition,
            },
            DdlColumn {
                name: "Bucket",
                cql_type: "int",
                role: KeyRole::Partition,
            },
            DdlColumn {
                name: "ts",
                cql_type: "timestamp",
                role: KeyRole::Clustering { desc: true },
            },
            DdlColumn {
                name: "seq",
                cql_type: "int",
                role: KeyRole::Clustering { desc: false },
            },
            DdlColumn {
                name: "owner",
                cql_type: "text",
                role: KeyRole::Static,
            },
            DdlColumn {
                name: "tags",
                cql_type: "frozen<list<text>>",
                role: KeyRole::Regular,
            },
        ];
        let options = [
            DdlOption {
                name: "comment".into(),
                value: single_quoted("it's"),
            },
            DdlOption {
                name: "compaction".into(),
                value: map_literal(&[(
                    "class",
                    "org.apache.cassandra.db.compaction.SizeTieredCompactionStrategy",
                )]),
            },
        ];
        assert_eq!(
            create_table("shop", "events", &cols, &options),
            "CREATE TABLE \"shop\".\"events\" (\n    \"user_id\" uuid,\n    \"Bucket\" int,\n    \
             \"ts\" timestamp,\n    \"seq\" int,\n    \"owner\" text static,\n    \"tags\" \
             frozen<list<text>>,\n    PRIMARY KEY ((\"user_id\", \"Bucket\"), \"ts\", \"seq\")\n) \
             WITH CLUSTERING ORDER BY (\"ts\" DESC, \"seq\" ASC)\n    AND comment = 'it''s'\n    \
             AND compaction = {'class': \
             'org.apache.cassandra.db.compaction.SizeTieredCompactionStrategy'};"
        );
        assert_eq!(
            create_table("ks", "kv", &cols[..1], &[]),
            "CREATE TABLE \"ks\".\"kv\" (\n    \"user_id\" uuid,\n    PRIMARY KEY ((\"user_id\"))\n);"
        );
        assert_eq!(
            create_view("ks", "by_ts", "events", &cols[..4], true, "ts IS NOT NULL"),
            "CREATE MATERIALIZED VIEW \"ks\".\"by_ts\" AS\n    SELECT *\n    FROM \
             \"ks\".\"events\"\n    WHERE ts IS NOT NULL\n    PRIMARY KEY ((\"user_id\", \
             \"Bucket\"), \"ts\", \"seq\") WITH CLUSTERING ORDER BY (\"ts\" DESC, \"seq\" ASC);"
        );
        assert_eq!(
            describe_table("Ks", "t\""),
            "DESCRIBE TABLE \"Ks\".\"t\"\"\""
        );
    }

    #[test]
    fn rebuilds_indexes_and_triggers() {
        let col = |part, c: &str| IndexTarget::Column {
            part,
            column: c.into(),
        };
        assert_eq!(
            create_index("ks", "t", "t_email", &col(None, "Email"), None, &[]),
            "CREATE INDEX \"t_email\" ON \"ks\".\"t\" (\"Email\");"
        );
        assert_eq!(
            create_index(
                "ks",
                "t",
                "t_m",
                &col(Some("keys"), "m"),
                Some("org.apache.cassandra.index.sasi.SASIIndex"),
                &[("mode".into(), "CONTAINS".into())]
            ),
            "CREATE CUSTOM INDEX \"t_m\" ON \"ks\".\"t\" (keys(\"m\")) USING \
             'org.apache.cassandra.index.sasi.SASIIndex' WITH OPTIONS = {'mode': 'CONTAINS'};"
        );
        let local = IndexTarget::Local {
            partition: vec!["p".into()],
            columns: vec!["c".into()],
        };
        assert_eq!(local.columns(), ["p", "c"]);
        assert_eq!(
            create_index("ks", "t", "l", &local, None, &[]),
            "CREATE INDEX \"l\" ON \"ks\".\"t\" ((\"p\"), \"c\");"
        );
        assert_eq!(
            create_trigger("ks", "t", "audit", "org.x.Audit'"),
            "CREATE TRIGGER \"audit\" ON \"ks\".\"t\" USING 'org.x.Audit''';"
        );
    }
}
