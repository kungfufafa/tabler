//! Redshift SQL (the pg-flavors lane). Built with `drv-pg-flavors` only.
//!
//! Redshift's leader node is PostgreSQL 8.0.2: `$n` parameters, `$$` and `$tag$` bodies, nested
//! block comments, and plain strings that read a backslash as an escape (there is no
//! `standard_conforming_strings`, and `E'…'` came with PostgreSQL 8.1). It has no RETURNING, no
//! ON CONFLICT, no IS NOT DISTINCT FROM and no savepoints, and `::text` is VARCHAR(256) there. So
//! values go in through casts to the column's own type (`::varchar(65535)` for text), and the
//! grid reads the types the binary protocol carries as they are and the others as text, per
//! column: SUPER through JSON_SERIALIZE, GEOMETRY through ST_AsEWKT and VARBYTE through TO_HEX,
//! which may cut them, so those columns are not editable and not fetched whole; an HLLSKETCH,
//! which no cast turns into text, is read as it comes.
//!
//! The statements the driver sends for itself with a name in them (SHOW TABLE, SHOW VIEW) are
//! built here too.

use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, PLAIN, reading};
use crate::sqlgen::dialect::spec::{
    CiLike, DialectSpec, NullSafeEq, PlaceholderStyle, RedactRule, SqlHooks, SqlKnobs,
    double_quoted, ph_dollar,
};
use crate::sqlgen::dialect::{SqlDialect, qualify_with};
use crate::sqlgen::portable::classify;
use crate::sqlgen::split::leading_keywords;
use crate::types::mutate::ServerExpr;
use crate::types::schema::{ColumnMeta, TableRef, ValueKind};

pub static LEX: LexProfile = LexProfile {
    ident_quotes: &[('"', '"')],
    string_quotes: &['\''],
    // The server reads backslashes as escapes in plain strings (PostgreSQL 8.0 before
    // standard_conforming_strings); the plain reading too, so a read is one either way.
    readings: &[reading(true, false), PLAIN],
    // `E'…'` is PostgreSQL 8.1's. Off, the `E` is a word and the string after it is read both ways,
    // which covers a server that has them (it reads them with escapes) and one that has not.
    e_strings: false,
    // `$$…$$` and `$tag$…$tag$` bodies (CREATE PROCEDURE, CREATE FUNCTION).
    dollar_quotes: true,
    dollar_tags: true,
    triple_quotes: false,
    raw_prefixes: false,
    byte_prefixes: false,
    hash_comments: false,
    slash_slash_comments: false,
    // The PostgreSQL 8.0 lexer Redshift's comes from nests them.
    nested_block_comments: true,
    dash_comment_needs_space: false,
    batch_separator: None,
    terminator_optional: false,
    begin_end_bodies: false,
};

/// The functions a query may call that change the server: AWS's system administration functions
/// (ending or cancelling sessions, settings, WLM priorities that hold for every later query of a
/// user, a cluster reboot), and the large-object, backup, statistics, server file, server signal
/// and sequence functions the leader node's PostgreSQL 8.0 may still have (AWS lists them as
/// unsupported, which on the leader node need not mean an error; superuser only; server files).
const WRITE_CALLS: &[&str] = &[
    "PG_TERMINATE_BACKEND",
    "PG_CANCEL_BACKEND",
    "SET_CONFIG",
    "CHANGE_QUERY_PRIORITY",
    "CHANGE_SESSION_PRIORITY",
    "CHANGE_USER_PRIORITY",
    "REBOOT_CLUSTER",
    "LO_*",
    // `lowrite(fd, data)`; `loread` only reads what `lo_open` opened.
    "LOWRITE",
    "PG_START_BACKUP",
    "PG_STOP_BACKUP",
    "PG_STAT_RESET*",
    // The generic file access functions read server files and list its directories.
    "PG_READ_FILE",
    "PG_READ_BINARY_FILE",
    "PG_STAT_FILE",
    "PG_LS_*",
    "PG_RELOAD_CONF",
    "PG_ROTATE_LOGFILE",
    "NEXTVAL",
    "SETVAL",
];

pub static RULES: ClassifyRules = ClassifyRules {
    // No VALUES statement: Redshift takes VALUES lists only in an INSERT.
    queries: &["SELECT", "WITH"],
    // SHOW TABLE / VIEW / COLUMNS / GRANTS / … and settings; EXPLAIN never runs its statement.
    inspections: &["SHOW", "EXPLAIN"],
    writes: &[
        "INTO",
        "INSERT",
        "UPDATE",
        "DELETE",
        "MERGE",
        "CREATE",
        "DROP",
        "ALTER",
        "TRUNCATE",
        "COPY",
        "UNLOAD",
        "CALL",
        "GRANT",
        "REVOKE",
        "VACUUM",
        // The calls of `WRITE_CALLS` that take one argument, as words: PostgreSQL 8.0 reads
        // `(1).pg_terminate_backend` as `pg_terminate_backend(1)` (attribute notation), with no
        // `(` after the name for the call check to see. Spelt out where a column may share the
        // prefix (`lo_size` stays a read); a quoted name is a change request
        // (docs/drivers/pg-flavors.md, Limits).
        "PG_TERMINATE_BACKEND",
        "PG_CANCEL_BACKEND",
        "LO_IMPORT",
        "LO_UNLINK",
        "LO_CREAT",
        "LO_CREATE",
        "PG_START_BACKUP",
        "PG_STAT_RESET*",
        "PG_READ_FILE",
        "PG_READ_BINARY_FILE",
        "PG_STAT_FILE",
        "PG_LS_*",
        "NEXTVAL",
    ],
    write_calls: WRITE_CALLS,
    table_functions: None,
};

fn classify_redshift(sql: &str) -> StatementClass {
    classify::with(&LEX, &RULES, sql)
}

/// Quotes doubled, and backslashes too: Redshift reads a backslash in a plain string as an escape
/// (its LIKE escape is written `'\\'`). Previews show the same text that would run.
fn quote_exec(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "''"))
}

fn from_hex(hex: &str) -> String {
    format!("FROM_HEX('{hex}')")
}

/// The type of a column without its modifier, lower case (`character varying`, `numeric`).
fn base_type(col: &ColumnMeta) -> String {
    let ty = col.cast_type().to_ascii_lowercase();
    ty.split('(').next().unwrap_or_default().trim().to_string()
}

fn is_text_type(base: &str) -> bool {
    matches!(
        base,
        "character varying"
            | "varchar"
            | "character"
            | "char"
            | "bpchar"
            | "text"
            | "nvarchar"
            | "nchar"
            | "name"
    )
}

fn is_bytes_type(base: &str) -> bool {
    matches!(base, "varbyte" | "binary varying" | "varbinary")
}

/// The types whose binary form the driver decodes as it is (booleans, integers, floats, dates and
/// text). Times and timestamps come as text: their binary form depends on how the leader node was
/// built (`integer_datetimes`).
fn read_as_is(base: &str) -> bool {
    is_text_type(base)
        || matches!(
            base,
            "boolean"
                | "bool"
                | "smallint"
                | "integer"
                | "bigint"
                | "int2"
                | "int4"
                | "int8"
                | "real"
                | "double precision"
                | "float4"
                | "float8"
                | "date"
        )
}

/// The text a row carries a column in; None to read it as the server sends it.
fn text(e: &str, col: &ColumnMeta) -> Option<String> {
    let base = base_type(col);
    match base.as_str() {
        "super" => Some(format!("JSON_SERIALIZE({e})")),
        "geometry" | "geography" => Some(format!("ST_AsEWKT({e})")),
        b if is_bytes_type(b) => Some(format!("TO_HEX({e})")),
        // No cast turns a sketch into text.
        "hllsketch" => None,
        b if read_as_is(b) => None,
        _ => Some(format!("{e}::varchar(65535)")),
    }
}

/// The projections above that may cut a value (64K of text at most) or show it as other text.
fn lossy(col: &ColumnMeta) -> bool {
    let base = base_type(col);
    matches!(base.as_str(), "super" | "geometry" | "geography") || is_bytes_type(&base)
}

/// A column as text for pattern filters and "Any column"; None leaves it out.
fn text_expr(e: &str, col: &ColumnMeta) -> Option<String> {
    let base = base_type(col);
    match base.as_str() {
        "hllsketch" => None,
        "boolean" | "bool" => Some(format!(
            "CASE WHEN {e} THEN 'true' WHEN NOT {e} THEN 'false' END"
        )),
        b if is_text_type(b) => Some(e.to_string()),
        _ => Some(text(e, col).unwrap_or_else(|| format!("{e}::varchar(65535)"))),
    }
}

/// Whether a catalog type name can be written as a cast as it is: letters, digits, spaces and
/// a modifier (`numeric(10,2)`, `timestamp without time zone`). Anything else is not trusted.
fn plain_type_name(ty: &str) -> bool {
    !ty.is_empty()
        && ty
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '(' | ')' | ','))
}

/// Values are bound as text (`drivers::postgres::params`), so each goes through its column's
/// type: text as the longest VARCHAR (the column checks its own length when it stores it, where a
/// cast to VARCHAR(n) would cut), a SUPER parsed from JSON, the other types by their full name
/// (a DECIMAL keeps its scale; a bare `::numeric` is NUMERIC(18,0) and would round).
fn param_cast(ph: String, col: &ColumnMeta) -> String {
    let base = base_type(col);
    match base.as_str() {
        b if is_text_type(b) => format!("{ph}::varchar(65535)"),
        "super" => format!("JSON_PARSE({ph})"),
        "geometry" => format!("ST_GeomFromEWKT({ph})"),
        "geography" => format!("ST_GeogFromText({ph})"),
        b if is_bytes_type(b) => format!("FROM_HEX({ph})"),
        "hllsketch" => ph,
        _ if plain_type_name(col.cast_type()) => format!("{ph}::{}", col.cast_type()),
        _ => ph,
    }
}

fn server_expr(expr: ServerExpr, _: ValueKind) -> Option<String> {
    match expr {
        // CURRENT_TIMESTAMP runs on the leader node only; GETDATE() runs where the rows are.
        ServerExpr::Now | ServerExpr::CurrentTimestamp => Some("GETDATE()".into()),
        ServerExpr::CurrentDate => Some("CURRENT_DATE".into()),
        _ => None,
    }
}

/// TRUNCATE commits the open transaction; VACUUM cannot run inside one.
fn implicit_commit(sql: &str) -> bool {
    matches!(
        leading_keywords(sql, 1).first().map(String::as_str),
        Some("TRUNCATE" | "VACUUM")
    )
}

pub static SPEC: DialectSpec = DialectSpec {
    dialect: SqlDialect::Redshift,
    label: "Redshift SQL",
    quote_ident: double_quoted,
    quote_literal: quote_exec,
    quote_literal_exec: quote_exec,
    placeholder: ph_dollar,
    placeholder_style: PlaceholderStyle::Dollar,
    placeholders_reusable: true,
    max_params: 32_000,
    max_name_parts: 3,
    lex: &LEX,
    rules: &RULES,
    classify: classify_redshift,
    knobs: SqlKnobs {
        bytes_literal: from_hex,
        text_projection: Some(text),
        lossy_text: lossy,
        text_expr,
        param_cast: Some(param_cast),
        ci_like: CiLike::Ilike,
        null_safe_eq: NullSafeEq::OrForm,
        // Not checked on a server yet: values are entered explicitly.
        set_default: false,
        server_expr,
        implicit_commit,
        catalog_words: &[
            "PG_CATALOG",
            "INFORMATION_SCHEMA",
            "PG_TABLE_DEF",
            "STL_",
            "STV_",
            "SVV_",
            "SVL_",
            "SYS_",
        ],
        // The same calls: the classifier already makes them writes.
        side_effect_functions: WRITE_CALLS,
        redact: &[
            RedactRule {
                keyword: "PASSWORD",
            },
            RedactRule {
                keyword: "CREDENTIALS",
            },
            RedactRule {
                keyword: "SECRET_ACCESS_KEY",
            },
            RedactRule {
                keyword: "SESSION_TOKEN",
            },
            RedactRule {
                keyword: "MASTER_SYMMETRIC_KEY",
            },
        ],
        ..SqlKnobs::ANSI
    },
    hooks: SqlHooks::NONE,
    ddl: None,
};

/// What `SHOW` gives the DDL of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowDdl {
    Table,
    View,
    ExternalTable,
}

/// `SHOW TABLE "s"."t"` (also VIEW, EXTERNAL TABLE): the server's own DDL of the object.
pub fn show_ddl(what: ShowDdl, table: &TableRef) -> String {
    let keyword = match what {
        ShowDdl::Table => "TABLE",
        ShowDdl::View => "VIEW",
        ShowDdl::ExternalTable => "EXTERNAL TABLE",
    };
    format!(
        "SHOW {keyword} {}",
        qualify_with(&SPEC, table.schema.as_deref(), &table.name)
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::sqlgen::portable::mutate::build_change;
    use crate::sqlgen::portable::select::{build_count, build_fetch_cell, build_select, is_lossy};
    use crate::sqlgen::portable::testing::{col, map, text as text_value};
    use crate::sqlgen::sql::{SelectQuery, TableTarget};
    use crate::types::browse::{
        Conjunction, FilterGroup, FilterNode, FilterOp, NullsOrder, RowIdentity, RowKey, SortDir,
        SortSpec,
    };
    use crate::types::mutate::StagedChange;
    use crate::types::value::Value;

    #[test]
    fn export_bindings_lex_redshift() {
        crate::sqlgen::dialect::lex::export_lex("redshift", &LEX, &RULES);
    }

    /// A table with a column of each kind Redshift has.
    fn columns() -> Vec<ColumnMeta> {
        let mut id = col("id", "bigint", ValueKind::BigInt);
        id.is_pk = true;
        id.pk_ordinal = Some(1);
        id.nullable = false;
        vec![
            id,
            col("name", "character varying(300)", ValueKind::Text),
            col("code", "character(3)", ValueKind::Text),
            col("price", "numeric(10,2)", ValueKind::Decimal),
            col("ok", "boolean", ValueKind::Bool),
            col("at", "timestamp without time zone", ValueKind::Timestamp),
            col("doc", "super", ValueKind::Json),
            col("shape", "geometry", ValueKind::Geometry),
            col("sketch", "hllsketch", ValueKind::Other),
            col("raw", "varbyte(64000)", ValueKind::Bytes),
        ]
    }

    #[test]
    fn projects_each_type_the_way_it_reads() {
        let cols = columns();
        let by = |name: &str| cols.iter().find(|c| c.name == name).unwrap().clone();
        let q = |c: &str| format!("\"{c}\"");
        let projected = |name: &str| text(&q(name), &by(name));
        assert_eq!(projected("id"), None);
        assert_eq!(projected("name"), None);
        assert_eq!(projected("ok"), None);
        assert_eq!(projected("sketch"), None);
        assert_eq!(
            projected("price").as_deref(),
            Some("\"price\"::varchar(65535)")
        );
        assert_eq!(projected("at").as_deref(), Some("\"at\"::varchar(65535)"));
        assert_eq!(projected("doc").as_deref(), Some("JSON_SERIALIZE(\"doc\")"));
        assert_eq!(projected("shape").as_deref(), Some("ST_AsEWKT(\"shape\")"));
        assert_eq!(projected("raw").as_deref(), Some("TO_HEX(\"raw\")"));
        for c in &cols {
            let cut = matches!(c.name.as_str(), "doc" | "shape" | "raw");
            assert_eq!(is_lossy(&SPEC, c), cut, "{}", c.name);
        }
        assert_eq!(
            text_expr("\"name\"", &by("name")).as_deref(),
            Some("\"name\"")
        );
        assert_eq!(
            text_expr("\"ok\"", &by("ok")).as_deref(),
            Some("CASE WHEN \"ok\" THEN 'true' WHEN NOT \"ok\" THEN 'false' END")
        );
        assert_eq!(text_expr("\"sketch\"", &by("sketch")), None);
        assert_eq!(
            text_expr("\"id\"", &by("id")).as_deref(),
            Some("\"id\"::varchar(65535)")
        );
    }

    #[test]
    fn values_go_in_through_their_columns_type() {
        let cols = columns();
        let cast = |name: &str| {
            let c = cols.iter().find(|c| c.name == name).unwrap();
            param_cast("$1".into(), c)
        };
        assert_eq!(cast("id"), "$1::bigint");
        assert_eq!(cast("name"), "$1::varchar(65535)");
        assert_eq!(cast("code"), "$1::varchar(65535)");
        assert_eq!(cast("price"), "$1::numeric(10,2)");
        assert_eq!(cast("at"), "$1::timestamp without time zone");
        assert_eq!(cast("doc"), "JSON_PARSE($1)");
        assert_eq!(cast("raw"), "FROM_HEX($1)");
        assert_eq!(cast("sketch"), "$1");
        // A type name the catalog should never give is not written into the SQL.
        let odd = col("x", "int); DROP TABLE t; --", ValueKind::Int);
        assert_eq!(param_cast("$1".into(), &odd), "$1");
        let quoted = col("x", "\"my type\"", ValueKind::Other);
        assert_eq!(param_cast("$1".into(), &quoted), "$1");
    }

    #[test]
    fn quotes_like_the_server_reads() {
        assert_eq!(quote_exec("it's C:\\x"), "'it''s C:\\\\x'");
        assert_eq!((SPEC.quote_literal)("a\\'"), "'a\\\\'''");
        assert_eq!(from_hex("00ff"), "FROM_HEX('00ff')");
        let t = TableRef::new(Some("sales \"eu\""), "orders");
        assert_eq!(
            show_ddl(ShowDdl::Table, &t),
            "SHOW TABLE \"sales \"\"eu\"\"\".\"orders\""
        );
        assert_eq!(
            show_ddl(ShowDdl::View, &TableRef::new(None, "v")),
            "SHOW VIEW \"v\""
        );
        assert_eq!(
            show_ddl(
                ShowDdl::ExternalTable,
                &TableRef::new(Some("spectrum"), "e")
            ),
            "SHOW EXTERNAL TABLE \"spectrum\".\"e\""
        );
        for what in [ShowDdl::Table, ShowDdl::View, ShowDdl::ExternalTable] {
            assert_eq!(classify_redshift(&show_ddl(what, &t)), StatementClass::Read);
        }
        assert!(implicit_commit("truncate t") && implicit_commit("VACUUM DELETE ONLY t"));
        assert!(!implicit_commit("DELETE FROM t"));
        assert_eq!(
            server_expr(ServerExpr::Now, ValueKind::Timestamp).as_deref(),
            Some("GETDATE()")
        );
        assert_eq!(
            server_expr(ServerExpr::LocalTimestamp, ValueKind::Timestamp),
            None
        );
    }

    /// Nothing the portable builders write for Redshift uses what Redshift lacks.
    #[test]
    fn writes_nothing_redshift_lacks() {
        let table = TableRef::new(Some("public"), "orders");
        let cols = columns();
        let target = TableTarget {
            table: &table,
            columns: &cols,
        };
        let mut sql = Vec::new();
        let filter = FilterGroup {
            conjunction: Conjunction::And,
            items: vec![
                FilterNode::Condition {
                    column: "name".into(),
                    op: FilterOp::Ilike,
                    values: vec![text_value("a%")],
                },
                FilterNode::Condition {
                    column: "price".into(),
                    op: FilterOp::Between,
                    values: vec![Value::Decimal("1.5".into()), Value::Decimal("9".into())],
                },
                FilterNode::Condition {
                    column: "code".into(),
                    op: FilterOp::Contains,
                    values: vec![text_value("x_")],
                },
                FilterNode::AnyColumn {
                    op: FilterOp::Contains,
                    values: vec![text_value("q")],
                },
            ],
        };
        let sort = [SortSpec {
            column: "at".into(),
            dir: SortDir::Desc,
            nulls: Some(NullsOrder::Last),
        }];
        let pk = ["id".to_string()];
        let select = build_select(
            &SPEC,
            &SelectQuery {
                table: &table,
                columns: &cols,
                selected: None,
                filter: Some(&filter),
                sort: &sort,
                limit: 100,
                offset: 200,
                pk: &pk,
                identity: RowIdentity::Pk,
            },
        )
        .unwrap();
        assert!(
            select
                .sql
                .contains("\"price\"::varchar(65535) AS \"price\""),
            "{}",
            select.sql
        );
        assert!(select.sql.contains("ILIKE"), "{}", select.sql);
        sql.push(select.sql);
        sql.push(
            build_count(&SPEC, &table, &cols, Some(&filter))
                .unwrap()
                .sql,
        );
        let key = RowKey::Pk(map([("id", Value::Int(7))]));
        let name = cols.iter().find(|c| c.name == "name").unwrap();
        sql.push(build_fetch_cell(&SPEC, &target, name, &key).unwrap().0);
        let all = RowKey::AllColumns(map([
            ("id", Value::Int(7)),
            ("name", Value::Null),
            ("price", Value::Decimal("2.50".into())),
        ]));
        let changes = [
            StagedChange::Update {
                row_id: "r".into(),
                key: key.clone(),
                changes: map([("name", text_value("x")), ("price", Value::Null)]),
                exprs: Some(BTreeMap::from([("at".to_string(), ServerExpr::Now)])),
            },
            StagedChange::Insert {
                row_id: "n".into(),
                values: map([("id", Value::Int(8)), ("name", text_value("y"))]),
                exprs: None,
            },
            StagedChange::Insert {
                row_id: "n".into(),
                values: BTreeMap::new(),
                exprs: None,
            },
            StagedChange::Delete {
                row_id: "r".into(),
                key: key.clone(),
            },
        ];
        for change in &changes {
            let (built, expect) = build_change(&SPEC, &target, change, true).unwrap();
            assert_eq!(expect, Some(1));
            sql.push(built.sql);
        }
        let joined = sql.join("\n");
        for lacking in [
            "RETURNING",
            "ON CONFLICT",
            "DISTINCT FROM",
            "::text",
            "SAVEPOINT",
            "string_agg",
            "unnest",
            "CURRENT_TIMESTAMP",
        ] {
            assert!(
                !joined
                    .to_ascii_lowercase()
                    .contains(&lacking.to_ascii_lowercase()),
                "{lacking} in:\n{joined}"
            );
        }
        assert!(joined.contains("GETDATE()"), "{joined}");
        // Rows are found by their key only: there is no row id, and Redshift keys are not
        // enforced, so a match on every column could hit two rows.
        let by_all = StagedChange::Delete {
            row_id: "r".into(),
            key: all,
        };
        assert!(build_change(&SPEC, &target, &by_all, false).is_err());
        // The lossy columns cannot be written.
        let doc = StagedChange::Update {
            row_id: "r".into(),
            key,
            changes: map([("doc", text_value("{}"))]),
            exprs: None,
        };
        let refused = build_change(&SPEC, &target, &doc, false).unwrap_err();
        assert!(refused.to_string().contains("shown shortened"), "{refused}");
    }

    #[test]
    fn classifies_the_hard_cases() {
        for sql in [
            "SELECT pg_terminate_backend(1)",
            "SELECT pg_cancel_backend(1)",
            "SELECT set_config('search_path', 'x', false)",
            "SELECT REBOOT_CLUSTER()",
            "SELECT CHANGE_USER_PRIORITY('bob', 'lowest')",
            "SELECT CHANGE_QUERY_PRIORITY(123, 'critical')",
            "SELECT CHANGE_SESSION_PRIORITY(30311, 'lowest')",
            "SELECT lo_import('/etc/passwd')",
            "SELECT lowrite(0, 'x')",
            "SELECT pg_read_file('pg_hba.conf')",
            "SELECT pg_ls_dir('.')",
            "SELECT pg_stat_file('postgresql.conf')",
            "SELECT pg_reload_conf()",
            "SELECT pg_rotate_logfile()",
            "SELECT nextval('s')",
            "SELECT setval('s', 1)",
            // Attribute notation: the leader node's parser (PostgreSQL 8.0 transformIndirection)
            // calls `name(expr)` for `(expr).name` when no field of that name is found.
            "SELECT (123).pg_terminate_backend",
            "SELECT (123).pg_cancel_backend",
            "SELECT (process).pg_terminate_backend FROM stv_sessions",
            "SELECT (s.process).pg_cancel_backend FROM stv_sessions s",
            "SELECT ('/etc/passwd').lo_import",
            "SELECT (1).lo_unlink",
            "SELECT (393216).lo_creat",
            "SELECT (1).lo_create",
            "SELECT ('x').pg_start_backup",
            "SELECT (1).PG_TERMINATE_BACKEND",
            "SELECT (SELECT 1).pg_terminate_backend",
            "SELECT (1)\n.pg_terminate_backend",
            "SELECT (1). pg_terminate_backend",
            "SELECT (1)/**/.pg_terminate_backend",
            "SELECT ('pg_hba.conf').pg_read_file",
            "SELECT ('.').pg_ls_dir",
            "SELECT ('s').nextval",
            "SELECT ('shared').pg_stat_reset_shared",
            "WITH k AS (SELECT (process).pg_terminate_backend FROM stv_sessions) SELECT * FROM k",
            "UNLOAD ('SELECT * FROM t') TO 's3://bucket/x' IAM_ROLE 'arn:aws:iam::1:role/r'",
            "COPY t FROM 's3://bucket/x' IAM_ROLE default",
            "CALL p()",
            "CREATE TEMP TABLE x AS SELECT 1",
            "SELECT * INTO #scratch FROM t",
            "SELECT 1; DELETE FROM t",
            "SELECT 'a\\'; DELETE FROM t; --'",
            "SELECT E'a\\'; DELETE FROM t; --'",
            "SELECT 'a\\''; DELETE FROM t",
            "SELECT $$ $$; DELETE FROM t",
            "EXPLAIN ANALYZE DELETE FROM t",
            "CANCEL 123",
            "SET search_path TO x",
            "BEGIN READ ONLY",
            "ANALYZE t",
            "REFRESH MATERIALIZED VIEW mv",
            "VALUES (1)",
        ] {
            assert_eq!(classify_redshift(sql), StatementClass::Write, "{sql}");
        }
        for sql in [
            "SELECT 'UNLOAD (''SELECT 1'') TO ''s3://x''' AS s, $$DELETE FROM t$$",
            "SELECT * FROM svv_table_info WHERE \"schema\" = 'public'",
            "SHOW TABLE public.orders",
            "EXPLAIN DELETE FROM t",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "SELECT current_setting('query_group')",
            "SELECT 'reboot_cluster()' AS a, change_priority FROM t",
            // The attribute notation in a string, quoted names that are only columns, and
            // columns that start like a write call without being one.
            "SELECT '(1).pg_terminate_backend' AS s, $$('x').lo_import$$",
            "SELECT \"pg_terminate_backend\", t.\"lo_import\" FROM t",
            "SELECT lo_owner, lo_size, nextval_at, stats_reset FROM objects",
            "SELECT (doc).items FROM docs",
        ] {
            assert_eq!(classify_redshift(sql), StatementClass::Read, "{sql}");
        }
        // The Metrics Board refuses the same calls.
        for sql in [
            "SELECT reboot_cluster()",
            "SELECT change_user_priority('bob', 'lowest')",
            "SELECT pg_terminate_backend(1)",
            "SELECT lo_unlink(1)",
            "SELECT pg_read_file('pg_hba.conf')",
            "SELECT nextval('s')",
        ] {
            assert!(
                crate::sqlgen::sql::calls_side_effect_function(SqlDialect::Redshift, sql),
                "{sql}"
            );
        }
        assert!(!crate::sqlgen::sql::calls_side_effect_function(
            SqlDialect::Redshift,
            "SELECT current_setting('query_group')"
        ));
    }

    /// The holes the rules cannot close, each under Limits in the brief with its change request
    /// to the shared classifier. They read today; when a change request lands and a case here
    /// turns into a write, move it to the suite's WRITES.
    #[test]
    fn documented_holes_read_until_their_change_requests() {
        for sql in [
            // A quoted name in attribute notation calls the function too, and Redshift folds a
            // quoted name to lower case unless enable_case_sensitive_identifier is on; the words
            // of `writes` never match a quoted name.
            "SELECT (1).\"pg_terminate_backend\"",
            "SELECT (1).\"PG_TERMINATE_BACKEND\"",
            "SELECT ('/etc/passwd').\"lo_import\"",
            "SELECT ('s').\"nextval\"",
        ] {
            assert_eq!(
                classify_redshift(sql),
                StatementClass::Read,
                "{sql} writes now: move it to the suite's WRITES and update the brief's Limits"
            );
        }
        // The Metrics Board's own check sees `name(` only. It refuses these anyway, since the
        // classifier makes them writes and the board refuses writes first.
        for sql in [
            "SELECT (1).pg_terminate_backend",
            "SELECT ('/etc/passwd').lo_import",
        ] {
            assert_eq!(classify_redshift(sql), StatementClass::Write, "{sql}");
            assert!(
                !crate::sqlgen::sql::calls_side_effect_function(SqlDialect::Redshift, sql),
                "{sql} is a side effect call now: drop this case and update the brief's Limits"
            );
        }
    }
}
