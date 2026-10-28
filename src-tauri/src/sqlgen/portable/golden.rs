//! Goldens of the portable builders: what they make of the test specs (`portable::testing`) and
//! of variants covering every `Paging`, `LimitOne`, `PlaceholderStyle`, `NullsSort` and `CiLike`,
//! recorded in `tests/fixtures/sqlgen-portable/*.txt`. After an intended change,
//! `TABLER_BLESS=1 cargo test --lib sqlgen::portable::golden` rewrites them (review the diff).
//! Every value is written as a JSON string, so the files are plain LF text.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;

use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::spec::{DialectSpec, NullsSort, Paging, PlaceholderStyle, RowLocator};
use crate::sqlgen::portable::testing::{
    ANSI_TEST, CI_LIKES, PLACEHOLDER_STYLES, TSQL_LIKE_TEST, ansi_with, columns, map, text,
    with_placeholders,
};
use crate::sqlgen::portable::{explain, literal, mutate, redact, select, split};
use crate::sqlgen::select::BuiltSql;
use crate::sqlgen::sql::{SelectQuery, TableTarget};
use crate::types::browse::{
    Conjunction, FilterGroup, FilterNode, FilterOp, NullsOrder, RowIdentity, RowKey, SortDir,
    SortSpec,
};
use crate::types::mutate::{Clock, ServerExpr, StagedChange};
use crate::types::query::ExplainMode;
use crate::types::schema::{ColumnMeta, TableRef};
use crate::types::value::Value;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sqlgen-portable")
}

fn q(s: &str) -> String {
    serde_json::to_string(s).expect("a string serializes")
}

fn js<T: Serialize + ?Sized>(v: &T) -> String {
    serde_json::to_string(v).expect("an IPC value serializes")
}

fn fail(e: &AppError) -> String {
    format!("error {} {}", e.code(), q(&e.to_string()))
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
        g.line("# Written by sqlgen::portable::golden; never edited by hand.");
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

    fn built(&mut self, label: impl AsRef<str>, spec: &DialectSpec, r: Result<BuiltSql, AppError>) {
        self.line(label);
        match r {
            Ok(b) => {
                self.line(format!("  sql: {}", q(&b.sql)));
                if !b.params.is_empty() {
                    self.line(format!("  params: {}", js(&b.params)));
                    let preview = literal::inline_params(spec, &b.sql, &b.params);
                    self.line(format!("  preview: {}", q(&preview)));
                }
            }
            Err(e) => self.line(format!("  {}", fail(&e))),
        }
    }

    fn check(self) {
        let path = dir().join(format!("{}.txt", self.name));
        if std::env::var("TABLER_BLESS").as_deref() == Ok("1") {
            std::fs::create_dir_all(dir()).expect("create the golden folder");
            std::fs::write(&path, &self.text).expect("write the golden file");
            return;
        }
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e} (TABLER_BLESS=1 writes it)", path.display()));
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
            "{} differs at line {}:\n  golden: {}\n  now:    {}\nIf the change is intended: \
             TABLER_BLESS=1 cargo test --lib sqlgen::portable::golden, then review the diff.",
            path.display(),
            at + 1,
            show(&want),
            show(&got)
        );
    }
}

fn base_specs() -> [(&'static str, DialectSpec); 2] {
    [("ansi", ANSI_TEST), ("tsql", TSQL_LIKE_TEST)]
}

fn table() -> TableRef {
    TableRef::new(Some("app"), "users")
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
        offset: 0,
        pk,
        identity: RowIdentity::Pk,
    }
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

fn any(op: FilterOp, values: Vec<Value>) -> FilterNode {
    FilterNode::AnyColumn { op, values }
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

/// The column and values an operator is recorded with (exhaustive: a new operator must be
/// added to `OPS` too).
fn op_case(op: FilterOp) -> (&'static str, Vec<Value>) {
    use FilterOp as F;
    match op {
        F::Eq | F::Ne | F::Gt | F::Lt | F::Ge | F::Le => ("age", vec![Value::Int(18)]),
        F::Like | F::NotLike | F::Ilike => ("name", vec![text("%bo_b%")]),
        F::In | F::NotIn => ("id", vec![Value::Int(1), Value::Int(2)]),
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
        | F::HasSuffixCase => ("name", vec![text("50%_off[x]!\\")]),
        F::JsonContains | F::JsonNotContains => ("payload", vec![text("{\"a\":1}")]),
    }
}

fn op_name(op: FilterOp) -> String {
    js(&op).trim_matches('"').to_string()
}

fn select_section(g: &mut Golden, name: &str, spec: &DialectSpec) {
    let t = table();
    let cols = columns();
    let pk = vec!["id".to_string()];
    g.section(format!("{name}: select"));
    g.built(
        "all columns, first page",
        spec,
        select::build_select(spec, &query(&t, &cols, None, &[], &pk)),
    );
    let picked = ["name".to_string(), "geom".to_string()];
    let mut q = query(&t, &cols, None, &[], &pk);
    q.selected = Some(&picked);
    q.offset = 200;
    q.limit = 50;
    g.built(
        "two columns, third page",
        spec,
        select::build_select(spec, &q),
    );
    let sort = [
        SortSpec {
            column: "name".into(),
            dir: SortDir::Desc,
            nulls: Some(NullsOrder::First),
        },
        SortSpec {
            column: "age".into(),
            dir: SortDir::Asc,
            nulls: Some(NullsOrder::Last),
        },
        SortSpec {
            column: "price".into(),
            dir: SortDir::Asc,
            nulls: None,
        },
    ];
    g.built(
        "sorted with NULLs first and last",
        spec,
        select::build_select(spec, &query(&t, &cols, None, &sort, &pk)),
    );
    let mut q = query(&t, &cols, None, &[], &[]);
    q.identity = RowIdentity::Rowid;
    g.built(
        "no primary key, rowid identity asked",
        spec,
        select::build_select(spec, &q),
    );
    let unknown = [SortSpec {
        column: "nope".into(),
        dir: SortDir::Asc,
        nulls: None,
    }];
    g.built(
        "sort on an unknown column",
        spec,
        select::build_select(spec, &query(&t, &cols, None, &unknown, &pk)),
    );
    let none: [String; 0] = [];
    let mut q = query(&t, &cols, None, &[], &pk);
    q.selected = Some(&none);
    g.built("no columns", spec, select::build_select(spec, &q));

    g.section(format!("{name}: filters"));
    for op in OPS {
        let (column, values) = op_case(op);
        let filter = and(vec![cond(column, op, values)]);
        g.built(
            format!("{} on {column}", op_name(op)),
            spec,
            select::build_count(spec, &t, &cols, Some(&filter)),
        );
    }
    let cases = [
        (
            "in, empty list",
            and(vec![cond("id", FilterOp::In, vec![])]),
        ),
        (
            "not in, empty list",
            and(vec![cond("id", FilterOp::NotIn, vec![])]),
        ),
        (
            "json contains on a non-JSON column",
            and(vec![cond("name", FilterOp::JsonContains, vec![text("{}")])]),
        ),
        (
            "between with one value",
            and(vec![cond("age", FilterOp::Between, vec![Value::Int(1)])]),
        ),
        (
            "contains on a binary column",
            and(vec![cond("data", FilterOp::Contains, vec![text("x")])]),
        ),
        (
            "unknown column",
            and(vec![cond("nope", FilterOp::Eq, vec![Value::Int(1)])]),
        ),
        (
            "nested groups, raw SQL and any column",
            FilterGroup {
                conjunction: Conjunction::Or,
                items: vec![
                    FilterNode::Group(and(vec![
                        cond("age", FilterOp::Ge, vec![Value::Int(18)]),
                        cond("name", FilterOp::IsNotNull, vec![]),
                    ])),
                    FilterNode::Raw {
                        sql: "  age > 1  ".into(),
                    },
                    FilterNode::Group(and(vec![])),
                    any(FilterOp::Contains, vec![text("x")]),
                ],
            },
        ),
        (
            "any column equals",
            and(vec![any(FilterOp::Eq, vec![text("7")])]),
        ),
        (
            "any column in",
            and(vec![any(FilterOp::In, vec![text("a"), text("b")])]),
        ),
        (
            "any column not contains",
            and(vec![any(FilterOp::NotContains, vec![text("x")])]),
        ),
        (
            "any column JSON contains",
            and(vec![any(FilterOp::JsonContains, vec![text("{}")])]),
        ),
    ];
    for (label, filter) in cases {
        g.built(
            label,
            spec,
            select::build_count(spec, &t, &cols, Some(&filter)),
        );
    }
    let wide: Vec<Value> = (0..1200).map(Value::Int).collect();
    let filter = and(vec![cond("id", FilterOp::In, wide)]);
    g.built(
        "IN past the parameter limit",
        spec,
        select::build_count(spec, &t, &cols, Some(&filter)).map(|b| BuiltSql {
            sql: format!("{} characters", b.sql.len()),
            params: Vec::new(),
        }),
    );

    g.section(format!("{name}: fetch cell"));
    let target = TableTarget {
        table: &t,
        columns: &cols,
    };
    let keys = [
        ("pk", RowKey::Pk(map([("id", Value::Int(7))]))),
        ("pk with NULL", RowKey::Pk(map([("id", Value::Null)]))),
        ("empty pk", RowKey::Pk(BTreeMap::new())),
        (
            "pk on an unknown column",
            RowKey::Pk(map([("nope", Value::Int(1))])),
        ),
        (
            "pk on a shortened column",
            RowKey::Pk(map([("geom", text("POINT (1 2)"))])),
        ),
        ("rowid", RowKey::Rowid(42)),
        (
            "all columns",
            RowKey::AllColumns(map([
                ("name", text("x")),
                ("age", Value::Null),
                ("price", Value::Decimal("1.50".into())),
            ])),
        ),
    ];
    for (label, key) in &keys {
        for column in ["name", "data", "geom"] {
            let c = cols.iter().find(|c| c.name == column).expect("a column");
            let r = select::build_fetch_cell(spec, &target, c, key)
                .map(|(sql, params)| BuiltSql { sql, params });
            g.built(format!("{column} by {label}"), spec, r);
        }
    }
}

fn variants_section(g: &mut Golden) {
    let t = table();
    let cols = columns();
    let pk = vec!["id".to_string()];
    let filter = and(vec![
        cond("age", FilterOp::Eq, vec![Value::Int(18)]),
        cond("id", FilterOp::In, vec![Value::Int(1), Value::Int(2)]),
        any(FilterOp::Contains, vec![text("it's")]),
    ]);
    g.section("variants: placeholder styles");
    for style in PLACEHOLDER_STYLES {
        let spec = with_placeholders(style);
        g.built(
            format!("{style:?}"),
            &spec,
            select::build_select(&spec, &query(&t, &cols, Some(&filter), &[], &pk)),
        );
    }

    g.section("variants: paging and limit one");
    for paging in [Paging::LimitOffset, Paging::OffsetFetch, Paging::LimitOnly] {
        let spec = ansi_with(|k| k.paging = paging);
        for offset in [0, 100] {
            let mut q = query(&t, &cols, None, &[], &[]);
            q.offset = offset;
            g.built(
                format!("{paging:?}, no order, offset {offset}"),
                &spec,
                select::build_select(&spec, &q),
            );
        }
    }
    let target = TableTarget {
        table: &t,
        columns: &cols,
    };
    let key = RowKey::Pk(map([("id", Value::Int(7))]));
    for (label, spec) in [("Limit", ANSI_TEST), ("Top", TSQL_LIKE_TEST)] {
        let r = select::build_fetch_cell(&spec, &target, &cols[2], &key)
            .map(|(sql, params)| BuiltSql { sql, params });
        g.built(format!("limit one: {label}"), &spec, r);
    }

    g.section("variants: NULL ordering");
    let sort = [SortSpec {
        column: "name".into(),
        dir: SortDir::Asc,
        nulls: Some(NullsOrder::Last),
    }];
    for nulls in [
        NullsSort::Native,
        NullsSort::NullFlag,
        NullsSort::Unsupported,
    ] {
        let spec = ansi_with(|k| k.nulls_order = nulls);
        g.built(
            format!("{nulls:?}"),
            &spec,
            select::build_select(&spec, &query(&t, &cols, None, &sort, &pk)),
        );
    }

    g.section("variants: case-insensitive matching");
    for ci in CI_LIKES {
        let spec = ansi_with(|k| k.ci_like = ci);
        for op in [FilterOp::ContainsCase, FilterOp::Ilike, FilterOp::Contains] {
            let (column, values) = op_case(op);
            let filter = and(vec![cond(column, op, values)]);
            g.built(
                format!("{ci:?}: {}", op_name(op)),
                &spec,
                select::build_count(&spec, &t, &cols, Some(&filter)),
            );
        }
    }
    let spec = ansi_with(|k| k.like_escape = None);
    let filter = and(vec![cond("name", FilterOp::HasPrefix, vec![text("50%\\")])]);
    g.built(
        "LIKE without an ESCAPE clause",
        &spec,
        select::build_count(&spec, &t, &cols, Some(&filter)),
    );

    g.section("variants: rowid locator");
    let spec = ansi_with(|k| k.row_locator = RowLocator::Rowid);
    let mut q = query(&t, &cols, None, &[], &[]);
    q.identity = RowIdentity::Rowid;
    g.built("select with rowid", &spec, select::build_select(&spec, &q));
    let r = select::build_fetch_cell(&spec, &target, &cols[1], &RowKey::Rowid(9))
        .map(|(sql, params)| BuiltSql { sql, params });
    g.built("fetch by rowid", &spec, r);

    g.section("injection");
    let evil = TableRef::new(Some("s\"]`"), "t\"; DROP TABLE x; --]");
    let evil_cols = vec![crate::sqlgen::portable::testing::col(
        "a\"b]c",
        "int",
        crate::types::schema::ValueKind::Int,
    )];
    let evil_filter = and(vec![cond(
        "a\"b]c",
        FilterOp::Eq,
        vec![text("1' OR '1'='1")],
    )]);
    for (name, spec) in base_specs() {
        g.built(
            name,
            &spec,
            select::build_select(
                &spec,
                &query(&evil, &evil_cols, Some(&evil_filter), &[], &[]),
            ),
        );
    }
}

#[test]
fn select_golden() {
    let mut g = Golden::new(
        "select",
        "Browse SELECT, COUNT with every filter, and fetch of one cell (portable builders).",
    );
    for (name, spec) in base_specs() {
        select_section(&mut g, name, &spec);
    }
    variants_section(&mut g);
    g.check();
}

fn update(key: RowKey, changes: BTreeMap<String, Value>) -> StagedChange {
    StagedChange::Update {
        row_id: "r".into(),
        key,
        changes,
        exprs: None,
    }
}

fn mutate_section(g: &mut Golden, name: &str, spec: &DialectSpec) {
    let t = table();
    let cols = columns();
    let target = TableTarget {
        table: &t,
        columns: &cols,
    };
    let pk = || RowKey::Pk(map([("id", Value::Int(7))]));
    let exprs = BTreeMap::from([
        ("created".to_string(), ServerExpr::Fsp(Clock::Now, 3)),
        ("age".to_string(), ServerExpr::Default),
    ]);
    let changes: Vec<(&str, StagedChange)> = vec![
        (
            "update by pk",
            update(pk(), map([("name", text("Bob")), ("price", Value::Null)])),
        ),
        (
            "update with DEFAULT and a clock",
            StagedChange::Update {
                row_id: "r".into(),
                key: pk(),
                changes: map([("name", Value::Default)]),
                exprs: Some(exprs.clone()),
            },
        ),
        (
            "update by a two-column key with NULL",
            update(
                RowKey::Pk(map([("id", Value::Int(7)), ("name", Value::Null)])),
                map([("age", Value::Int(3))]),
            ),
        ),
        (
            "update by rowid",
            update(RowKey::Rowid(42), map([("age", Value::Int(3))])),
        ),
        (
            "update by all columns",
            update(
                RowKey::AllColumns(map([("name", text("x")), ("age", Value::Null)])),
                map([("age", Value::Int(3))]),
            ),
        ),
        ("update with no change", update(pk(), BTreeMap::new())),
        (
            "update of a shortened column",
            update(pk(), map([("geom", text("POINT (1 2)"))])),
        ),
        (
            "update of an unknown column",
            update(pk(), map([("nope", Value::Int(1))])),
        ),
        (
            "insert",
            StagedChange::Insert {
                row_id: "n".into(),
                values: map([
                    ("name", text("it's")),
                    ("age", Value::Default),
                    ("data", Value::from_bytes(&[0, 255])),
                ]),
                exprs: None,
            },
        ),
        (
            "insert with a clock",
            StagedChange::Insert {
                row_id: "n".into(),
                values: map([("name", text("x"))]),
                exprs: Some(exprs),
            },
        ),
        (
            "insert of defaults only",
            StagedChange::Insert {
                row_id: "n".into(),
                values: map([("age", Value::Default)]),
                exprs: None,
            },
        ),
        (
            "insert of a shortened column",
            StagedChange::Insert {
                row_id: "n".into(),
                values: map([("geom", text("POINT (1 2)"))]),
                exprs: None,
            },
        ),
        (
            "delete by pk",
            StagedChange::Delete {
                row_id: "r".into(),
                key: pk(),
            },
        ),
        (
            "delete by all columns",
            StagedChange::Delete {
                row_id: "r".into(),
                key: RowKey::AllColumns(map([("name", text("x"))])),
            },
        ),
        (
            "delete by an empty key",
            StagedChange::Delete {
                row_id: "r".into(),
                key: RowKey::Pk(BTreeMap::new()),
            },
        ),
    ];
    g.section(format!("{name}: changes"));
    for (label, change) in &changes {
        for returning in [false, true] {
            let r = mutate::build_change(spec, &target, change, returning).map(|(b, n)| {
                assert_eq!(n, Some(1), "every change must affect one row");
                b
            });
            g.built(
                format!("{label}{}", if returning { ", returning" } else { "" }),
                spec,
                r,
            );
        }
    }
}

#[test]
fn mutate_golden() {
    let mut g = Golden::new(
        "mutate",
        "UPDATE / INSERT / DELETE of staged edits (portable builders): rows by key only.",
    );
    for (name, spec) in base_specs() {
        mutate_section(&mut g, name, &spec);
    }
    let t = table();
    let cols = columns();
    let target = TableTarget {
        table: &t,
        columns: &cols,
    };
    g.section("variants");
    let key = RowKey::Pk(map([("id", Value::Int(7))]));
    let spec = ansi_with(|k| k.set_default = false);
    g.built(
        "SET DEFAULT unsupported",
        &spec,
        mutate::build_change(
            &spec,
            &target,
            &update(key.clone(), map([("age", Value::Default)])),
            false,
        )
        .map(|(b, _)| b),
    );
    let spec = ansi_with(|k| k.insert_all_defaults = None);
    let defaults_only = StagedChange::Insert {
        row_id: "n".into(),
        values: BTreeMap::new(),
        exprs: None,
    };
    g.built(
        "insert of defaults only unsupported",
        &spec,
        mutate::build_change(&spec, &target, &defaults_only, true).map(|(b, _)| b),
    );
    fn no_clock(_: ServerExpr, _: crate::types::schema::ValueKind) -> Option<String> {
        None
    }
    let spec = ansi_with(|k| k.server_expr = no_clock);
    let clock = StagedChange::Update {
        row_id: "r".into(),
        key: key.clone(),
        changes: BTreeMap::new(),
        exprs: Some(BTreeMap::from([(
            "created".to_string(),
            ServerExpr::CurrentTimestamp,
        )])),
    };
    g.built(
        "server expression unsupported",
        &spec,
        mutate::build_change(&spec, &target, &clock, false).map(|(b, _)| b),
    );
    let spec = ansi_with(|k| k.row_locator = RowLocator::Rowid);
    g.built(
        "update by rowid with a rowid locator",
        &spec,
        mutate::build_change(
            &spec,
            &target,
            &update(RowKey::Rowid(42), map([("age", Value::Int(1))])),
            false,
        )
        .map(|(b, _)| b),
    );
    for style in PLACEHOLDER_STYLES {
        let spec = with_placeholders(style);
        g.built(
            format!("{style:?}"),
            &spec,
            mutate::build_change(
                &spec,
                &target,
                &update(key.clone(), map([("name", text("a"))])),
                false,
            )
            .map(|(b, _)| b),
        );
    }
    g.check();
}

fn sample_values() -> Vec<Value> {
    vec![
        Value::Null,
        Value::Default,
        Value::Bool(true),
        Value::Bool(false),
        Value::Int(-42),
        Value::BigInt("9007199254740993".into()),
        Value::Float(1.5),
        Value::Decimal("123.4500".into()),
        text("it's"),
        text("a\\b"),
        text("héllo\nworld"),
        Value::from_bytes(&[0x00, 0xff, b'A']),
        Value::Date("2024-02-29".into()),
        Value::TimestampTz("2024-02-29 12:00:00+07".into()),
        Value::Uuid("6f1c2e2a-8a43-4c4e-9b1e-1f5f0c1d2e3f".into()),
        Value::Json("{\"b\": \"it's\"}".into()),
        Value::Array(vec![Value::Int(1), text("x")]),
        Value::other("hierarchyid", "/1/2/"),
    ]
}

#[test]
fn literal_golden() {
    let mut g = Golden::new(
        "literal",
        "Literals and inlined placeholders (portable builders).",
    );
    for (name, spec) in base_specs() {
        g.section(format!("{name}: literals"));
        for v in sample_values() {
            g.line(format!("{} => {}", js(&v), q(&literal::literal(&spec, &v))));
        }
    }
    g.section("inline params");
    let params = [text("a?b"), Value::Int(2), Value::Null];
    let texts: [(PlaceholderStyle, &str); 7] = [
        (
            PlaceholderStyle::Dollar,
            "SELECT '$1' AS x, $1, $2::int, $3, $4 FROM t",
        ),
        (
            PlaceholderStyle::Question,
            "SELECT '?', ?, ? /* ? */, ?, ? FROM t",
        ),
        (
            PlaceholderStyle::QuestionNumbered,
            "SELECT ?1, ?2, ?1, \"?3\", ?3 FROM t",
        ),
        (
            PlaceholderStyle::AtP,
            "SELECT @P1, @P2, @@ROWCOUNT, N'@P3', @P3 FROM t",
        ),
        (
            PlaceholderStyle::AtName,
            "SELECT @p1, @p2, \"@p3\", @p3 FROM t",
        ),
        (
            PlaceholderStyle::Colon,
            "SELECT :1, x::int, :2, ':3', :3 FROM t",
        ),
        (
            PlaceholderStyle::Typed,
            "SELECT {p1:String}, {p2:Nullable(Int64)}, '{p3:String}', {p3:String} FROM t",
        ),
    ];
    for (style, sql) in texts {
        let spec = with_placeholders(style);
        g.line(format!("{style:?}: {}", q(sql)));
        g.line(format!(
            "  => {}",
            q(&literal::inline_params(&spec, sql, &params))
        ));
    }
    g.check();
}

/// Statements every spec is run over: splitting, classifying, redacting and explaining them.
const STATEMENTS: &[&str] = &[
    "SELECT 1",
    "SELECT 1; DELETE FROM t",
    "SELECT 1 DELETE FROM t",
    "SELECT * FROM [delete] WHERE \"a]\" = 'x;y'",
    "SELECT * INTO t2 FROM t",
    "WITH a AS (SELECT 1) SELECT * FROM a;",
    "EXPLAIN ANALYZE UPDATE t SET a = 1",
    "SELECT 'it\\'s; DELETE FROM t'",
    "SELECT $$; DROP TABLE t$$",
    "SELECT 1 /* a /* b */ DELETE FROM t */",
    "SELECT * FROM OPENQUERY(srv, 'DELETE FROM t')",
    "SELECT * FROM generate_series(1, 3), read_parquet('x')",
    "SELECT * FROM t, remote_rows('x')",
    "BEGIN SELECT 1; SELECT 2; END; SELECT 3",
    "SELECT 1\nGO\nSELECT 2\ngo 3\n",
    "CREATE USER u PASSWORD 'p''w'; ALTER USER u IDENTIFIED WITH sha2 BY 'x\\'y'",
    "",
    "-- only a comment",
];

#[test]
fn statements_golden() {
    let mut g = Golden::new(
        "statements",
        "Splitting, Safe Mode classes, trace redaction and EXPLAIN (portable builders).",
    );
    for (name, spec) in base_specs() {
        g.section(name);
        for sql in STATEMENTS {
            g.line(q(sql));
            let parts: Vec<String> = split::split(spec.lex, sql)
                .into_iter()
                .map(|s| format!("{}@{}..{}:{}", q(&s.sql), s.start, s.end, s.line))
                .collect();
            g.line(format!("  split: [{}]", parts.join(", ")));
            let class = match (spec.classify)(sql) {
                StatementClass::Read => "read",
                StatementClass::Write => "write",
            };
            g.line(format!("  class: {class}"));
            let r = redact::redact_spec(&spec, sql);
            if r.sql != *sql {
                g.line(format!("  redacted: {} {}", q(&r.sql), js(&r.secrets)));
            }
        }
        g.section(format!("{name}: explain"));
        for mode in [ExplainMode::Plan, ExplainMode::Analyze] {
            match explain::wrap_explain(&spec, "SELECT 1 ;  ", mode) {
                Ok(sql) => g.line(format!(
                    "{mode:?}: {} (offset {})",
                    q(&sql),
                    explain::explain_offset(&spec, mode)
                )),
                Err(e) => g.line(format!("{mode:?}: {}", fail(&e))),
            }
        }
        for in_tx in [Some(false), Some(true), None] {
            for writes in [false, true] {
                let r = explain::analyze_wrapper(&spec, in_tx, writes);
                g.line(format!(
                    "analyze in_tx={in_tx:?} writes={writes}: {}",
                    match r {
                        Ok(w) => format!("{w:?}"),
                        Err(e) => fail(&e),
                    }
                ));
            }
        }
    }
    let spec = ansi_with(|k| k.analyze_tx = Some(("BEGIN", "ROLLBACK")));
    g.section("variant: an analyze transaction");
    for in_tx in [Some(false), Some(true)] {
        g.line(format!(
            "analyze in_tx={in_tx:?} writes=true: {}",
            match explain::analyze_wrapper(&spec, in_tx, true) {
                Ok(w) => format!("{w:?}"),
                Err(e) => fail(&e),
            }
        ));
    }
    g.check();
}
