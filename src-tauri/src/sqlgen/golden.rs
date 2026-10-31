//! Golden corpus of the core SQL generators: what the builders, scanners and classifiers produce
//! for PostgreSQL, MySQL and SQLite, recorded in `tests/fixtures/sqlgen-golden/*.txt`. New
//! dialects are added next to these three; this test proves that their output stays
//! byte-identical. After an intended change, `TABLER_BLESS=1 cargo test --lib sqlgen::golden`
//! rewrites the files (review the diff).
//!
//! `statements.json` in the same folder is the frozen statement corpus: the SQL of
//! `src/features/safety/classify.cases.json` and the string literals of the split, classify,
//! redact and transaction unit tests as they were when the corpus was made, so it does not move
//! with them.
//!
//! Every value is written as a JSON string (control characters escaped), so the files are plain
//! LF text whatever the SQL holds.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;

use crate::drivers::redact::redact_secrets;
use crate::drivers::{LATE_CANCEL_NOTICE, cancel_safe_on_stop, cap_cancel_notice, tx_effect};
use crate::error::AppError;
use crate::sqlgen::classify::{StatementClass, calls_side_effect_function, classify};
use crate::sqlgen::explain::{analyze_wrapper, explain_offset, wrap_explain};
use crate::sqlgen::mutate::{
    TableSpec, build_change, build_delete, build_insert, build_insert_cells, build_update,
    build_update_cells, cells, ordered, row_key_where, server_expr_sql,
};
use crate::sqlgen::quote::{Dialect, MysqlLiteralMode};
use crate::sqlgen::select::{
    BuiltSql, SelectSpec, build_count, build_fetch_cell, build_select, build_where, escape_glob,
    escape_like, inline_params, literal, pg_cast_type,
};
use crate::sqlgen::split::{code_only, is_single_statement, split_statements};
use crate::sqlgen::transfer::max_params;
use crate::types::browse::{
    Conjunction, FilterGroup, FilterNode, FilterOp, NullsOrder, RowIdentity, RowKey, SortDir,
    SortSpec,
};
use crate::types::mutate::{Clock, ServerExpr, StagedChange};
use crate::types::query::ExplainMode;
use crate::types::schema::{ColumnMeta, TableKind, TableMeta, TableRef, ValueKind};
use crate::types::value::Value;

const DIALECTS: [Dialect; 3] = [Dialect::Postgres, Dialect::Mysql, Dialect::Sqlite];

fn name(d: Dialect) -> &'static str {
    match d {
        Dialect::Postgres => "postgres",
        Dialect::Mysql => "mysql",
        Dialect::Sqlite => "sqlite",
    }
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sqlgen-golden")
}

/// A string as JSON: quoted, with control characters escaped.
fn q(s: &str) -> String {
    serde_json::to_string(s).expect("a string serializes")
}

/// A value in its IPC (JSON) form.
fn js<T: Serialize + ?Sized>(v: &T) -> String {
    serde_json::to_string(v).expect("an IPC value serializes")
}

fn fail(e: &AppError) -> String {
    format!("error {} {}", e.code(), q(&e.to_string()))
}

fn yn(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// A golden file being written, line by line.
struct Golden {
    name: &'static str,
    text: String,
}

impl Golden {
    fn new(name: &'static str, about: &str) -> Golden {
        let mut g = Golden {
            name,
            text: String::new(),
        };
        g.line(format!("# {about}"));
        g.line("# Written by sqlgen::golden from the core code; never edited by hand.");
        g
    }

    fn line(&mut self, line: impl AsRef<str>) {
        self.text.push_str(line.as_ref());
        self.text.push('\n');
    }

    fn section(&mut self, title: impl AsRef<str>) {
        self.text.push('\n');
        self.line(format!("## {}", title.as_ref()));
    }

    /// SQL and its parameters (with the preview that inlines them), or the error.
    fn result(&mut self, indent: &str, d: Dialect, r: Result<BuiltSql, AppError>) {
        match r {
            Ok(b) => {
                self.line(format!("{indent}sql: {}", q(&b.sql)));
                if !b.params.is_empty() {
                    self.line(format!("{indent}params: {}", js(&b.params)));
                    let preview = inline_params(d, &b.sql, &b.params);
                    self.line(format!("{indent}preview: {}", q(&preview)));
                }
            }
            Err(e) => self.line(format!("{indent}{}", fail(&e))),
        }
    }

    fn built(&mut self, label: impl AsRef<str>, d: Dialect, r: Result<BuiltSql, AppError>) {
        self.line(label);
        self.result("  ", d, r);
    }

    /// The WHERE body `build_where` makes of `group`.
    fn filter(
        &mut self,
        label: impl AsRef<str>,
        d: Dialect,
        cols: &[ColumnMeta],
        group: FilterGroup,
    ) {
        self.line(label);
        match where_of(d, cols, &group) {
            Ok(Some(built)) => self.result("  ", d, Ok(built)),
            Ok(None) => self.line("  where: none"),
            Err(e) => self.line(format!("  {}", fail(&e))),
        }
    }

    /// `filter` for a WHERE too long to keep: its size and digest.
    fn filter_digest(&mut self, label: &str, d: Dialect, cols: &[ColumnMeta], group: FilterGroup) {
        self.line(label);
        match where_of(d, cols, &group) {
            Ok(Some(BuiltSql { sql, params })) => {
                let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, sql.as_bytes());
                let hex: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
                self.line(format!(
                    "  {} params, {} bytes, sha256 {hex}",
                    params.len(),
                    sql.len()
                ));
            }
            Ok(None) => self.line("  where: none"),
            Err(e) => self.line(format!("  {}", fail(&e))),
        }
    }

    /// One line (or block) per dialect; `same as <dialect>` where an earlier one rendered the
    /// same. `render` returns `" value"` or `"\n    line…"`.
    fn per_dialect(&mut self, label: &str, render: impl Fn(Dialect) -> String) {
        let mut done: Vec<(Dialect, String)> = Vec::new();
        for d in DIALECTS {
            let body = render(d);
            match done.iter().find(|(_, b)| *b == body) {
                Some((first, _)) => {
                    self.line(format!("  {label} {}: same as {}", name(d), name(*first)))
                }
                None => self.line(format!("  {label} {}:{body}", name(d))),
            }
            done.push((d, body));
        }
    }

    /// Compares with the committed file, or rewrites it under `TABLER_BLESS=1`.
    fn check(self) {
        let path = dir().join(format!("{}.txt", self.name));
        if std::env::var("TABLER_BLESS").as_deref() == Ok("1") {
            std::fs::create_dir_all(dir()).expect("create the golden folder");
            std::fs::write(&path, &self.text).expect("write the golden file");
            return;
        }
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e} (TABLER_BLESS=1 writes it)", path.display()));
        // Values never hold a raw CR, so a checkout with CRLF line ends compares the same.
        let expected = expected.replace("\r\n", "\n");
        if expected == self.text {
            return;
        }
        let want: Vec<&str> = expected.lines().collect();
        let got: Vec<&str> = self.text.lines().collect();
        let at = want
            .iter()
            .zip(&got)
            .position(|(a, b)| a != b)
            .unwrap_or(want.len().min(got.len()));
        let show = |lines: &[&str]| {
            lines
                .get(at)
                .copied()
                .unwrap_or("<end of file>")
                .to_string()
        };
        panic!(
            "{} differs from the core output at line {}:\n  golden: {}\n  now:    {}\nIf the change \
             is intended: TABLER_BLESS=1 cargo test --lib sqlgen::golden, then review the diff.",
            path.display(),
            at + 1,
            show(&want),
            show(&got)
        );
    }
}

/// The WHERE body of `group` with its parameters; `None` when it has no condition.
fn where_of(
    d: Dialect,
    cols: &[ColumnMeta],
    group: &FilterGroup,
) -> Result<Option<BuiltSql>, AppError> {
    let mut params = Vec::new();
    let sql = build_where(d, cols, Some(group), &mut params)?;
    Ok(sql.map(|sql| BuiltSql { sql, params }))
}

/// The frozen statement corpus.
fn corpus() -> Vec<String> {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/sqlgen-golden/statements.json"
    ))
    .expect("statements.json is a list of strings")
}

fn col(name: &str, data_type: &str, kind: ValueKind) -> ColumnMeta {
    serde_json::from_value(serde_json::json!({
        "name": name, "ordinal": 1, "dataType": data_type, "kind": kind, "nullable": true,
        "isPk": false, "isAutoIncrement": false, "isGenerated": false, "isUnsigned": false,
    }))
    .expect("a column")
}

/// A table with one column of each shape the builders treat differently (PostgreSQL type
/// names: only the PostgreSQL casts read them).
fn columns() -> Vec<ColumnMeta> {
    let mut id = col("id", "bigint", ValueKind::BigInt);
    id.is_pk = true;
    id.pk_ordinal = Some(1);
    id.nullable = false;
    // A domain: PostgreSQL casts to its base type.
    let mut code = col("code", "short_code", ValueKind::Text);
    code.base_type = Some("character varying".into());
    vec![
        id,
        col("name", "character varying(50)", ValueKind::Text),
        col("age", "integer", ValueKind::Int),
        col("status", "user_status", ValueKind::Enum),
        col("price", "numeric(10,2)", ValueKind::Decimal),
        col("payload", "jsonb", ValueKind::Json),
        col("data", "bytea", ValueKind::Bytes),
        col("geom", "geometry", ValueKind::Geometry),
        code,
        col("Odd \"Name\"", "text", ValueKind::Text),
        col(
            "created",
            "timestamp(3) with time zone",
            ValueKind::TimestampTz,
        ),
        col("tags", "text[]", ValueKind::Array),
    ]
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn map<const N: usize>(pairs: [(&str, Value); N]) -> BTreeMap<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

fn and(items: Vec<FilterNode>) -> FilterGroup {
    FilterGroup {
        conjunction: Conjunction::And,
        items,
    }
}

fn cond(column: &str, op: FilterOp, values: Vec<Value>) -> FilterNode {
    FilterNode::Condition {
        column: column.into(),
        op,
        values,
    }
}

/// Groups inside groups, a raw fragment, empty parts and an "Any column" condition.
fn nested_filter() -> FilterGroup {
    FilterGroup {
        conjunction: Conjunction::Or,
        items: vec![
            FilterNode::Group(and(vec![
                cond("status", FilterOp::Eq, vec![text("active")]),
                cond("age", FilterOp::Ge, vec![Value::Int(18)]),
            ])),
            cond("name", FilterOp::Like, vec![text("%bob%")]),
            FilterNode::Group(and(vec![])),
            FilterNode::Raw {
                sql: "  age > 1  ".into(),
            },
            FilterNode::Raw { sql: "   ".into() },
            FilterNode::AnyColumn {
                op: FilterOp::Contains,
                values: vec![text("x")],
            },
        ],
    }
}

const OPS: [FilterOp; 25] = [
    FilterOp::Eq,
    FilterOp::Ne,
    FilterOp::Gt,
    FilterOp::Lt,
    FilterOp::Ge,
    FilterOp::Le,
    FilterOp::Like,
    FilterOp::NotLike,
    FilterOp::In,
    FilterOp::NotIn,
    FilterOp::IsNull,
    FilterOp::IsNotNull,
    FilterOp::Between,
    FilterOp::NotBetween,
    FilterOp::Contains,
    FilterOp::NotContains,
    FilterOp::ContainsCase,
    FilterOp::NotContainsCase,
    FilterOp::HasPrefix,
    FilterOp::HasSuffix,
    FilterOp::HasPrefixCase,
    FilterOp::HasSuffixCase,
    FilterOp::Ilike,
    FilterOp::JsonContains,
    FilterOp::JsonNotContains,
];

/// The column and values an operator is recorded with. Exhaustive, so a new operator cannot be
/// left out of the corpus unnoticed (add it to `OPS` too).
fn op_case(op: FilterOp) -> (&'static str, Vec<Value>) {
    use FilterOp as F;
    match op {
        F::Eq | F::Ne | F::Gt | F::Lt | F::Ge | F::Le => ("age", vec![Value::Int(18)]),
        F::Like | F::NotLike | F::Ilike => ("name", vec![text("%bo_b%")]),
        F::In | F::NotIn => ("id", vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
        F::IsNull | F::IsNotNull => ("name", vec![]),
        F::Between | F::NotBetween => (
            "price",
            vec![Value::Decimal("1.50".into()), Value::Decimal("9.99".into())],
        ),
        F::Contains
        | F::NotContains
        | F::ContainsCase
        | F::NotContainsCase
        | F::HasPrefix
        | F::HasSuffix
        | F::HasPrefixCase
        | F::HasSuffixCase => ("name", vec![text("50%_off*[x]?!")]),
        F::JsonContains | F::JsonNotContains => ("payload", vec![text("{\"a\":1}")]),
    }
}

/// One value of every kind, with the edge cases of their literal forms.
fn sample_values() -> Vec<Value> {
    vec![
        Value::Null,
        Value::Default,
        Value::Bool(true),
        Value::Bool(false),
        Value::Int(0),
        Value::Int(-42),
        Value::Int(9_007_199_254_740_991),
        Value::BigInt("9007199254740993".into()),
        Value::BigInt("-18446744073709551615".into()),
        Value::Float(1.5),
        Value::Float(-0.25),
        Value::Float(1e21),
        Value::Float(3e-7),
        Value::Decimal("123.4500".into()),
        text("it's"),
        text("a\\b"),
        text("héllo\nworld"),
        text(""),
        Value::from_bytes(&[0x00, 0xff, b'A', b'B']),
        Value::from_bytes(&[]),
        Value::Date("2024-02-29".into()),
        Value::Time("23:59:59.123456".into()),
        Value::Timestamp("2024-02-29 12:00:00".into()),
        Value::TimestampTz("2024-02-29 12:00:00+07".into()),
        Value::Interval("1 day 02:00:00".into()),
        Value::Uuid("6f1c2e2a-8a43-4c4e-9b1e-1f5f0c1d2e3f".into()),
        Value::Json("{\"a\": [1, 2.50], \"b\": \"it's\"}".into()),
        Value::Array(vec![Value::Int(1), text("x"), Value::Null]),
        Value::Enum("active".into()),
        Value::other("inet", "10.0.0.1/8"),
    ]
}

fn keys() -> Vec<RowKey> {
    vec![
        RowKey::Pk(map([("id", Value::Int(7))])),
        RowKey::Pk(map([("id", Value::Int(7)), ("name", text("a'b"))])),
        RowKey::Pk(map([("id", Value::Null)])),
        RowKey::Pk(map([("code", text("x"))])),
        RowKey::Pk(BTreeMap::new()),
        RowKey::Pk(map([("nope", Value::Int(1))])),
        RowKey::Rowid(42),
        RowKey::Rowid(9_007_199_254_740_993),
        RowKey::AllColumns(map([
            ("name", text("x")),
            ("age", Value::Null),
            ("price", Value::Decimal("1.50".into())),
            ("payload", Value::Json("{\"a\": 1}".into())),
            ("tags", Value::Array(vec![text("a"), text("b c")])),
        ])),
        RowKey::AllColumns(BTreeMap::new()),
    ]
}

fn server_exprs() -> Vec<ServerExpr> {
    let mut out = vec![
        ServerExpr::Now,
        ServerExpr::CurrentTimestamp,
        ServerExpr::CurrentDate,
        ServerExpr::CurrentTime,
        ServerExpr::LocalTimestamp,
        ServerExpr::Default,
    ];
    for clock in [
        Clock::Now,
        Clock::CurrentTimestamp,
        Clock::CurrentTime,
        Clock::LocalTimestamp,
    ] {
        out.push(ServerExpr::Fsp(clock, 0));
        out.push(ServerExpr::Fsp(clock, 6));
    }
    // Above the largest precision: refused.
    out.push(ServerExpr::Fsp(Clock::Now, 7));
    out
}

#[test]
fn quoting() {
    const NAMES: &[&str] = &[
        "users",
        "Order Items",
        "a\"b",
        "a`b",
        "a]b",
        "x\"; DROP TABLE t; --",
        "select",
        "ünïcode",
        "",
        "tab\tline\nbreak",
    ];
    const TEXTS: &[&str] = &[
        "it's",
        "a\\b",
        "C:\\it's",
        "it's a\\b",
        "a\\",
        "a\\'",
        "a\0b",
        "say \"hi\"",
        "héllo",
        "",
        "line\nbreak\r\n",
        "$$x$$",
    ];
    let mut g = Golden::new(
        "quote",
        "Identifier and literal quoting, qualified names and placeholders per dialect.",
    );
    for d in DIALECTS {
        g.section(name(d));
        for s in NAMES {
            g.line(format!("quote_ident {} => {}", q(s), q(&d.quote_ident(s))));
        }
        for s in NAMES {
            for schema in [Some("app"), Some(""), None] {
                let qualified = d.qualify(schema, s);
                g.line(format!(
                    "qualify {} {} => {}",
                    js(&schema),
                    q(s),
                    q(&qualified)
                ));
            }
            g.line(format!(
                "qualify {} \"t\" => {}",
                q(s),
                q(&d.qualify(Some(s), "t"))
            ));
        }
        for s in TEXTS {
            g.line(format!(
                "quote_literal {} => {}",
                q(s),
                q(&d.quote_literal(s))
            ));
            for mode in [
                MysqlLiteralMode::BackslashEscapes,
                MysqlLiteralMode::NoBackslashEscapes,
            ] {
                let quoted = d.quote_literal_with(s, mode);
                g.line(format!(
                    "quote_literal_with {mode:?} {} => {}",
                    q(s),
                    q(&quoted)
                ));
            }
        }
        for n in [1, 2, 10, 65_535] {
            g.line(format!("placeholder {n} => {}", q(&d.placeholder(n))));
        }
    }
    g.check();
}

#[test]
fn literals() {
    // Every placeholder style, in and out of a string, with numbers past the parameter count.
    const MIXED: &str = "SELECT $1, ?1, ?, '$2 ?2 ? it''s', $10, ?10, $11, ?0, $0, $, ?x, \"$3\" \
                         FROM t WHERE a = ? AND b = ?";
    let params = vec![
        Value::Int(1),
        text("a'b"),
        Value::Null,
        Value::Bool(true),
        Value::Float(2.5),
        Value::Decimal("1.10".into()),
        Value::from_bytes(b"\x01"),
        Value::Date("2024-01-02".into()),
        Value::Json("{}".into()),
        text("tenth"),
    ];
    let mut g = Golden::new(
        "literal",
        "Preview literals of every value kind, and placeholders inlined for display, per dialect.",
    );
    for d in DIALECTS {
        g.section(name(d));
        for v in sample_values() {
            g.line(format!("literal {} => {}", js(&v), q(&literal(d, &v))));
        }
        g.line(format!("inline_params {}", q(MIXED)));
        g.line(format!("  with {}", js(&params)));
        g.line(format!("  => {}", q(&inline_params(d, MIXED, &params))));
        g.line(format!(
            "  without params => {}",
            q(&inline_params(d, MIXED, &[]))
        ));
    }
    g.check();
}

#[test]
fn selects() {
    let cols = columns();
    let users = TableRef::new(Some("app"), "users");
    let bare = TableRef::new(None, "users");
    let pk = vec!["id".to_string()];
    let browse = |d: Dialect| SelectSpec {
        dialect: d,
        table: &users,
        columns: &cols,
        selected: None,
        filter: None,
        sort: &[],
        limit: 100,
        offset: 0,
        pk: &pk,
        identity: RowIdentity::Pk,
    };
    let mut g = Golden::new(
        "select",
        "Browse SELECTs, WHERE bodies of every filter operator, counts and their helpers per dialect.",
    );

    g.section("helpers");
    for s in ["plain", "50%_off", "a!b", "*?[x]", ""] {
        g.line(format!(
            "escape_like {} => {}; escape_glob => {}",
            q(s),
            q(&escape_like(s)),
            q(&escape_glob(s))
        ));
    }
    for ty in [
        "integer",
        "character varying(5)",
        "character(5)",
        "char",
        "bit(8)",
        "bit",
        "bit varying(3)",
        "interval day to second(3)",
        "numeric(10,2)[]",
        "character varying(5)[][]",
        "\"My(Type)\"(4)",
        "  timestamp(3)   with time zone  ",
    ] {
        g.line(format!(
            "pg_cast_type {} => {}",
            q(ty),
            q(&pg_cast_type(ty))
        ));
    }

    for d in DIALECTS {
        g.section(format!("{} conditions", name(d)));
        for op in OPS {
            let (column, values) = op_case(op);
            let label = format!("{column} {op:?} {}", js(&values));
            g.filter(label, d, &cols, and(vec![cond(column, op, values.clone())]));
            let label = format!("any column {op:?} {}", js(&values));
            g.filter(
                label,
                d,
                &cols,
                and(vec![FilterNode::AnyColumn { op, values }]),
            );
        }
        let extra: Vec<(&str, FilterNode)> = vec![
            ("empty IN", cond("id", FilterOp::In, vec![])),
            ("empty NOT IN", cond("id", FilterOp::NotIn, vec![])),
            (
                "any column, empty IN",
                FilterNode::AnyColumn {
                    op: FilterOp::In,
                    values: vec![],
                },
            ),
            (
                "any column IN, values bound once where they can repeat",
                FilterNode::AnyColumn {
                    op: FilterOp::In,
                    values: vec![text("a"), Value::Int(2)],
                },
            ),
            (
                "BETWEEN with one value",
                cond("price", FilterOp::Between, vec![Value::Int(1)]),
            ),
            (
                "unknown column",
                cond("nope", FilterOp::Eq, vec![Value::Int(1)]),
            ),
            ("= NULL", cond("age", FilterOp::Eq, vec![Value::Null])),
            ("domain column", cond("code", FilterOp::Eq, vec![text("x")])),
            (
                "timestamptz column",
                cond(
                    "created",
                    FilterOp::Gt,
                    vec![Value::TimestampTz("2024-01-01 00:00:00+00".into())],
                ),
            ),
            (
                "array column",
                cond("tags", FilterOp::Eq, vec![text("{a,b}")]),
            ),
            (
                "enum column",
                cond("status", FilterOp::Ne, vec![text("gone")]),
            ),
            (
                "quoted name",
                cond("Odd \"Name\"", FilterOp::Eq, vec![text("y")]),
            ),
            ("bytes column", cond("data", FilterOp::IsNull, vec![])),
            (
                "pattern on bytes",
                cond("data", FilterOp::Contains, vec![text("ab")]),
            ),
            (
                "JSON contains on a text column",
                cond("name", FilterOp::JsonContains, vec![text("{}")]),
            ),
            (
                "pattern without a value",
                cond("name", FilterOp::Contains, vec![Value::Null]),
            ),
            (
                "raw fragment",
                FilterNode::Raw {
                    sql: "  age > 1  ".into(),
                },
            ),
            ("blank raw fragment", FilterNode::Raw { sql: " ".into() }),
        ];
        for (label, node) in extra {
            g.filter(label, d, &cols, and(vec![node]));
        }
        g.filter("nested groups", d, &cols, nested_filter());
        g.filter("empty group", d, &cols, and(vec![]));

        // Bind-parameter limits: 32,001 values pass PostgreSQL and MySQL only; "Any column" over
        // 10 columns times 6,001 values binds 60,010 on MySQL, which cannot repeat a placeholder.
        let many = |n: i64| (1..=n).map(Value::Int).collect::<Vec<_>>();
        let label = "IN with 32001 values";
        g.filter_digest(
            label,
            d,
            &cols,
            and(vec![cond("id", FilterOp::In, many(32_001))]),
        );
        let any_in = FilterNode::AnyColumn {
            op: FilterOp::In,
            values: many(6_001),
        };
        g.filter_digest(
            "any column IN with 6001 values",
            d,
            &cols,
            and(vec![any_in]),
        );

        g.section(format!("{} select", name(d)));
        g.built(
            "all columns",
            d,
            build_select(&SelectSpec {
                limit: 200,
                offset: 400,
                ..browse(d)
            }),
        );
        let subset = vec!["name".to_string(), "age".to_string()];
        let sort = vec![
            SortSpec {
                column: "name".into(),
                dir: SortDir::Desc,
                nulls: Some(NullsOrder::Last),
            },
            SortSpec {
                column: "age".into(),
                dir: SortDir::Asc,
                nulls: Some(NullsOrder::First),
            },
        ];
        let filter = nested_filter();
        g.built(
            "subset, two sorts, nested filter, no schema",
            d,
            build_select(&SelectSpec {
                table: &bare,
                selected: Some(&subset),
                filter: Some(&filter),
                sort: &sort,
                limit: 50,
                ..browse(d)
            }),
        );
        let only_id = vec!["id".to_string()];
        for dir in [SortDir::Asc, SortDir::Desc] {
            for nulls in [None, Some(NullsOrder::First), Some(NullsOrder::Last)] {
                let sort = vec![SortSpec {
                    column: "age".into(),
                    dir,
                    nulls,
                }];
                g.built(
                    format!("sort {}", js(&sort)),
                    d,
                    build_select(&SelectSpec {
                        selected: Some(&only_id),
                        sort: &sort,
                        ..browse(d)
                    }),
                );
            }
        }
        let sort = vec![
            SortSpec {
                column: "status".into(),
                dir: SortDir::Asc,
                nulls: None,
            },
            SortSpec {
                column: "Odd \"Name\"".into(),
                dir: SortDir::Desc,
                nulls: None,
            },
        ];
        g.built(
            format!("sort {}", js(&sort)),
            d,
            build_select(&SelectSpec {
                selected: Some(&only_id),
                sort: &sort,
                ..browse(d)
            }),
        );
        let composite = vec!["id".to_string(), "name".to_string()];
        let none: Vec<String> = Vec::new();
        let only_name = vec!["name".to_string()];
        for (key_columns, identity) in [
            (&pk, RowIdentity::Pk),
            (&composite, RowIdentity::Pk),
            (&none, RowIdentity::Rowid),
            (&none, RowIdentity::None),
            (&pk, RowIdentity::Rowid),
            (&none, RowIdentity::Pk),
        ] {
            g.built(
                format!("pk {} identity {identity:?}", js(key_columns)),
                d,
                build_select(&SelectSpec {
                    selected: Some(&only_name),
                    pk: key_columns,
                    identity,
                    ..browse(d)
                }),
            );
        }
        let any = and(vec![FilterNode::AnyColumn {
            op: FilterOp::Contains,
            values: vec![text("x")],
        }]);
        g.built(
            "all columns, any column contains",
            d,
            build_select(&SelectSpec {
                filter: Some(&any),
                ..browse(d)
            }),
        );
        let unknown = vec!["nope".to_string()];
        g.built(
            "unknown selected column",
            d,
            build_select(&SelectSpec {
                selected: Some(&unknown),
                ..browse(d)
            }),
        );
        g.built(
            "empty selection",
            d,
            build_select(&SelectSpec {
                selected: Some(&[]),
                ..browse(d)
            }),
        );
        let sort = vec![SortSpec {
            column: "nope".into(),
            dir: SortDir::Asc,
            nulls: None,
        }];
        g.built(
            "unknown sort column",
            d,
            build_select(&SelectSpec {
                sort: &sort,
                ..browse(d)
            }),
        );

        g.section(format!("{} count", name(d)));
        g.built("no filter", d, build_count(d, &users, &cols, None));
        g.built(
            "nested filter, no schema",
            d,
            build_count(d, &bare, &cols, Some(&filter)),
        );
    }
    g.check();
}

#[test]
fn rows() {
    let cols = columns();
    let users = TableRef::new(Some("app"), "users");
    let mut g = Golden::new(
        "rows",
        "Row addressing (row_key_where) and full-cell fetches (build_fetch_cell) per dialect and key.",
    );
    for d in DIALECTS {
        let spec = TableSpec {
            dialect: d,
            table: &users,
            columns: &cols,
        };
        g.section(name(d));
        for key in keys() {
            g.line(format!("key {}", js(&key)));
            let mut params = Vec::new();
            match row_key_where(&spec, &key, &mut params) {
                Ok(w) => {
                    g.line(format!("  where: {}", q(&w.condition)));
                    g.line(format!("  suffix: {}", q(&w.suffix)));
                    if !params.is_empty() {
                        g.line(format!("  params: {}", js(&params)));
                    }
                }
                Err(e) => g.line(format!("  {}", fail(&e))),
            }
            for column in ["name", "data", "payload"] {
                let meta = cols
                    .iter()
                    .find(|c| c.name == column)
                    .expect("a sample column");
                g.line(format!("  fetch {column}"));
                let fetched = build_fetch_cell(&spec, meta, &key);
                g.result(
                    "    ",
                    d,
                    fetched.map(|(sql, params)| BuiltSql { sql, params }),
                );
            }
        }
    }
    g.check();
}

#[test]
fn mutations() {
    let cols = columns();
    let users = TableRef::new(Some("app"), "users");
    let pk7 = RowKey::Pk(map([("id", Value::Int(7))]));
    let all = RowKey::AllColumns(map([("name", text("x")), ("age", Value::Null)]));
    let mut g = Golden::new(
        "mutate",
        "UPDATE / INSERT / DELETE of staged edits, with and without RETURNING, and server \
         expressions per dialect.",
    );
    for d in DIALECTS {
        let spec = TableSpec {
            dialect: d,
            table: &users,
            columns: &cols,
        };

        g.section(format!("{} server expressions", name(d)));
        for expr in server_exprs() {
            for kind in [ValueKind::Timestamp, ValueKind::Date, ValueKind::Time] {
                let sql = match server_expr_sql(d, expr, kind) {
                    Ok(sql) => q(&sql),
                    Err(e) => fail(&e),
                };
                g.line(format!("{} {kind:?} => {sql}", expr.wire()));
            }
        }

        g.section(format!("{} update", name(d)));
        let changes = map([("name", text("Bob")), ("age", Value::Int(30))]);
        for returning in [false, true] {
            g.built(
                format!("name and age by pk, returning {returning}"),
                d,
                build_update(&spec, &pk7, &changes, returning),
            );
        }
        g.built(
            "DEFAULT",
            d,
            build_update(&spec, &pk7, &map([("price", Value::Default)]), false),
        );
        let changes = map([
            ("payload", Value::Json("{\"a\": 1}".into())),
            ("tags", Value::Array(vec![text("a")])),
            ("data", Value::from_bytes(b"\x00\x01")),
        ]);
        for returning in [false, true] {
            g.built(
                format!("json, array and bytes by rowid, returning {returning}"),
                d,
                build_update(&spec, &RowKey::Rowid(42), &changes, returning),
            );
        }
        for returning in [false, true] {
            g.built(
                format!("NULL by every column, returning {returning}"),
                d,
                build_update(&spec, &all, &map([("name", Value::Null)]), returning),
            );
        }
        let changes = map([("created", text("2024-01-01")), ("name", text("x"))]);
        let exprs = BTreeMap::from([
            ("created".to_string(), ServerExpr::Fsp(Clock::Now, 3)),
            ("age".to_string(), ServerExpr::Default),
        ]);
        g.built(
            "server expressions",
            d,
            build_update_cells(&spec, &pk7, &cells(&changes, Some(&exprs)), false),
        );
        g.built(
            "no changes",
            d,
            build_update(&spec, &pk7, &BTreeMap::new(), false),
        );
        g.built(
            "unknown column",
            d,
            build_update(&spec, &pk7, &map([("nope", Value::Int(1))]), false),
        );

        g.section(format!("{} insert", name(d)));
        let values = map([
            ("id", Value::Int(1)),
            ("name", text("a")),
            ("payload", Value::Json("{}".into())),
        ]);
        for returning in [false, true] {
            g.built(
                format!("three columns, returning {returning}"),
                d,
                build_insert(&spec, &values, returning),
            );
            g.built(
                format!("no columns, returning {returning}"),
                d,
                build_insert(&spec, &BTreeMap::new(), returning),
            );
        }
        g.built(
            "DEFAULT is left out",
            d,
            build_insert(
                &spec,
                &map([("id", Value::Default), ("name", text("x"))]),
                false,
            ),
        );
        let values = map([("created", text("2024-01-01")), ("name", text("x"))]);
        let exprs = BTreeMap::from([
            ("created".to_string(), ServerExpr::CurrentTimestamp),
            ("status".to_string(), ServerExpr::Default),
        ]);
        g.built(
            "server expressions",
            d,
            build_insert_cells(&spec, &cells(&values, Some(&exprs)), false),
        );
        g.built(
            "unknown column",
            d,
            build_insert(&spec, &map([("nope", Value::Int(1))]), false),
        );

        g.section(format!("{} delete", name(d)));
        for key in keys() {
            g.built(format!("key {}", js(&key)), d, build_delete(&spec, &key));
        }

        g.section(format!("{} staged changes", name(d)));
        let changes = vec![
            StagedChange::Insert {
                row_id: "i1".into(),
                values: map([("name", text("new"))]),
                exprs: Some(BTreeMap::from([("created".to_string(), ServerExpr::Now)])),
            },
            StagedChange::Update {
                row_id: "u1".into(),
                key: pk7.clone(),
                changes: map([("name", text("changed"))]),
                exprs: None,
            },
            StagedChange::Delete {
                row_id: "d1".into(),
                key: RowKey::Rowid(3),
            },
            StagedChange::Update {
                row_id: "u2".into(),
                key: all.clone(),
                changes: map([("age", Value::Int(1))]),
                exprs: Some(BTreeMap::from([(
                    "created".to_string(),
                    ServerExpr::LocalTimestamp,
                )])),
            },
            StagedChange::Delete {
                row_id: "d2".into(),
                key: pk7.clone(),
            },
        ];
        let order: Vec<&str> = ordered(&changes).iter().map(|c| c.row_id()).collect();
        g.line(format!("ordered => {}", js(&order)));
        for change in ordered(&changes) {
            for returning in [false, true] {
                let built = build_change(&spec, change, returning);
                let rows = built.as_ref().ok().map(|(_, rows)| *rows);
                g.built(
                    format!(
                        "{} {}, returning {returning}, affects {}",
                        change.op_name(),
                        change.row_id(),
                        js(&rows)
                    ),
                    d,
                    built.map(|(sql, _)| sql),
                );
            }
        }
    }
    g.check();
}

#[test]
fn explains() {
    const SQL: &str = "SELECT * FROM t WHERE a = ';' ;  ;\n";
    let mut g = Golden::new(
        "explain",
        "EXPLAIN wrapping, Explain Analyze transactions, parameter limits and cancel notices per \
         dialect.",
    );
    g.line(format!("late cancel notice => {}", q(LATE_CANCEL_NOTICE)));
    for d in DIALECTS {
        g.section(name(d));
        for mode in [ExplainMode::Plan, ExplainMode::Analyze] {
            for mariadb in [false, true] {
                g.line(format!(
                    "wrap_explain {mode:?} mariadb {mariadb} => {} offset {}",
                    q(&wrap_explain(d, SQL, mode, mariadb)),
                    explain_offset(d, mode, mariadb)
                ));
            }
        }
        for in_tx in [None, Some(false), Some(true)] {
            for writes in [false, true] {
                let wrapper = match analyze_wrapper(d, in_tx, writes) {
                    Ok(None) => "as is".to_string(),
                    Ok(Some((open, close))) => format!("{} then {}", q(&open), js(&close)),
                    Err(e) => fail(&e),
                };
                g.line(format!(
                    "analyze_wrapper in_tx {} writes {writes} => {wrapper}",
                    js(&in_tx)
                ));
            }
        }
        g.line(format!("max_params => {}", max_params(d)));
        g.line(format!(
            "cap_cancel_notice 1000 => {}",
            q(&cap_cancel_notice(1000, d))
        ));
    }
    g.check();
}

fn db_error(sqlstate: &str, vendor_code: Option<i64>) -> AppError {
    AppError::Db {
        message: "failed".into(),
        detail: None,
        hint: None,
        sqlstate: Some(sqlstate.into()),
        vendor_code,
        position: None,
        statement_index: None,
    }
}

#[test]
fn statements() {
    let failed = db_error("XX000", None);
    let parse = db_error("42000", Some(1064));
    let tx = |sql: &str, d: Dialect, error: Option<&AppError>| match tx_effect(sql, d, error) {
        Some(true) => "open",
        Some(false) => "closed",
        None => "same",
    };
    let mut g = Golden::new(
        "statements",
        "Per statement and dialect: Safe Mode class, side-effect function call, cancel-safe at the \
         row cap, single statement, and the transaction after it succeeded / failed / failed to \
         parse (MySQL 1064).",
    );
    for sql in corpus() {
        g.line(q(&sql));
        for d in DIALECTS {
            let class = match classify(&sql, d) {
                StatementClass::Read => "read",
                StatementClass::Write => "write",
            };
            g.line(format!(
                "  {}: {class}, side effect {}, cancel-safe {}, single {}, tx {}/{}/{}",
                name(d),
                yn(calls_side_effect_function(&sql, d)),
                yn(cancel_safe_on_stop(&sql, d)),
                yn(is_single_statement(&sql, d)),
                tx(&sql, d, None),
                tx(&sql, d, Some(&failed)),
                tx(&sql, d, Some(&parse)),
            ));
        }
    }
    g.check();
}

#[test]
fn splits() {
    let mut g = Golden::new(
        "split",
        "The editor's statement splitter (char offsets, 1-based line) and the code-only view of \
         each statement of the corpus, per dialect.",
    );
    for sql in corpus() {
        g.line(q(&sql));
        g.per_dialect("split", |d| {
            let parts: Vec<String> = split_statements(&sql, d)
                .iter()
                .map(|s| format!("\n    {}..{} line {} {}", s.start, s.end, s.line, q(&s.sql)))
                .collect();
            if parts.is_empty() {
                " nothing".to_string()
            } else {
                parts.concat()
            }
        });
        g.per_dialect("code_only", |d| format!(" {}", q(&code_only(&sql, d))));
    }
    g.check();
}

#[test]
fn redactions() {
    let corpus = corpus();
    let mut g = Golden::new(
        "redact",
        "Password literals masked in the SQL trace, per dialect, for the statements of the corpus \
         that one dialect changes.",
    );
    let mut unchanged = 0;
    for sql in &corpus {
        let changes = |d: Dialect| {
            let r = redact_secrets(sql, d);
            r.sql != *sql || !r.secrets.is_empty()
        };
        if !DIALECTS.into_iter().any(changes) {
            unchanged += 1;
            continue;
        }
        g.line(q(sql));
        g.per_dialect("redact", |d| {
            let r = redact_secrets(sql, d);
            format!(" {} secrets {}", q(&r.sql), js(&r.secrets))
        });
    }
    g.line(format!(
        "unchanged in every dialect: {unchanged} of {}",
        corpus.len()
    ));
    g.check();
}

/// A table of `columns()` for the export's SELECT (`export::table_query`).
fn export_table(kind: TableKind, keyed: bool) -> TableMeta {
    let mut columns = columns();
    if !keyed {
        for c in &mut columns {
            c.is_pk = false;
            c.pk_ordinal = None;
        }
    }
    // An identity column written as `GENERATED ALWAYS`, and a generated one.
    let mut serial = col("serial", "integer", ValueKind::Int);
    serial.is_auto_increment = true;
    serial.extra = Some("ALWAYS".into());
    let mut total = col("total", "numeric", ValueKind::Decimal);
    total.is_generated = true;
    columns.push(serial);
    columns.push(total);
    TableMeta {
        table: TableRef::new(Some("app"), "users"),
        kind,
        columns,
        primary_key: if keyed { vec!["id".into()] } else { Vec::new() },
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
        triggers: Vec::new(),
        comment: None,
        row_estimate: None,
        ddl: None,
        checks: Vec::new(),
        options: None,
    }
}

#[test]
fn exports() {
    use crate::export::table_query;
    let mut g = Golden::new(
        "export",
        "Table export SELECTs (export::table_query): columns as the export reads them, filter, \
         sort, pages, large blobs and identity columns per dialect.",
    );
    let keyed = export_table(TableKind::Table, true);
    let keyless = export_table(TableKind::Table, false);
    let view = export_table(TableKind::View, false);
    let subset = vec!["name".to_string(), "data".to_string(), "serial".to_string()];
    let filter = nested_filter();
    let sort = vec![SortSpec {
        column: "age".into(),
        dir: SortDir::Desc,
        nulls: Some(NullsOrder::Last),
    }];
    let unknown = vec!["nope".to_string()];
    let empty: Vec<String> = Vec::new();
    /// One export: the table, its columns, filter, sort, `max_rows`, offset and generated
    /// columns left out.
    struct Case<'a> {
        label: &'a str,
        meta: &'a TableMeta,
        selected: Option<&'a [String]>,
        filter: Option<&'a FilterGroup>,
        sort: &'a [SortSpec],
        max_rows: Option<u64>,
        offset: u64,
        skip_generated: bool,
    }
    let all = |label, meta| Case {
        label,
        meta,
        selected: None,
        filter: None,
        sort: &[],
        max_rows: None,
        offset: 0,
        skip_generated: false,
    };
    let cases = [
        all("every column", &keyed),
        Case {
            skip_generated: true,
            ..all("every column but the generated ones", &keyed)
        },
        Case {
            selected: Some(&subset),
            ..all("a subset", &keyed)
        },
        Case {
            filter: Some(&filter),
            sort: &sort,
            ..all("filter and sort", &keyed)
        },
        Case {
            max_rows: Some(10),
            ..all("first rows", &keyed)
        },
        Case {
            sort: &sort,
            max_rows: Some(10),
            offset: 20,
            ..all("a page, sorted", &keyed)
        },
        Case {
            selected: Some(&subset),
            offset: 20,
            ..all("rows after an offset", &keyed)
        },
        Case {
            max_rows: Some(10),
            offset: 20,
            ..all("a page of a table without a key", &keyless)
        },
        Case {
            selected: Some(&subset),
            sort: &sort,
            max_rows: Some(5),
            ..all("a sorted page of a table without a key", &keyless)
        },
        Case {
            selected: Some(&subset),
            max_rows: Some(10),
            ..all("a page of a view", &view)
        },
        all("every row of a table without a key", &keyless),
        Case {
            selected: Some(&unknown),
            ..all("an unknown column", &keyed)
        },
        Case {
            selected: Some(&empty),
            ..all("no column", &keyed)
        },
    ];
    for d in DIALECTS {
        g.section(name(d));
        for c in &cases {
            g.line(c.label);
            let query = table_query(
                d,
                c.meta,
                c.selected,
                c.filter,
                c.sort,
                c.max_rows,
                c.offset,
                c.skip_generated,
            );
            match query {
                Ok(query) => {
                    g.result(
                        "  ",
                        d,
                        Ok(BuiltSql {
                            sql: query.sql,
                            params: query.params,
                        }),
                    );
                    if !query.blob_hex.is_empty() {
                        g.line(format!("  blob hex of: {}", js(&query.blob_hex)));
                    }
                    if query.identity_always {
                        g.line("  overriding system value");
                    }
                }
                Err(e) => g.line(format!("  {}", fail(&e))),
            }
        }
    }
    g.check();
}
