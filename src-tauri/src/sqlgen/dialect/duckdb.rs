//! DuckDB SQL (the duckdb lane). A conservative first spec from the research notes; the lane
//! refines it. Built with `drv-duckdb` only. (The `duckdb` crate is `::duckdb` from here.)
//!
//! PostgreSQL-like text (double-quoted names, `E'…'` and `$$…$$` strings), `$1` parameters,
//! RETURNING. DuckDB reads files and other databases from SQL. The classifier calls the plain
//! spellings of that writes: a call of `read_*()`, `glob()` or another scanner of `write_calls`,
//! a table function outside `table_functions`, `COPY`, `ATTACH`, `INSTALL`, `LOAD`, `EXPORT` /
//! `IMPORT DATABASE`.
//!
//! It does not look for a file named where a table goes: a replacement scan (`FROM 'x.csv'`,
//! `FROM "C:/x.csv"`, `FROM x."/path"`, `FROM data.csv`, whose parts the binder joins into a
//! path when no table has that name) reads like a table to it. The engine refuses those reads,
//! so the lane must open read-only sessions (`access_mode=READ_ONLY`), unattended ones
//! (`registry::unattended`, MCP) and plugin-origin ones with `SET enable_external_access =
//! false` followed by `SET lock_configuration = true` (so no statement can turn it back on),
//! extension autoload and autoinstall off, and report `ReadOnlyMode::Server` with
//! `safety.hardened` for them.

use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, PLAIN};
use crate::sqlgen::dialect::spec::{
    CiLike, DialectSpec, PlaceholderStyle, RedactRule, Returning, SqlHooks, SqlKnobs,
    double_quoted, ph_dollar, single_quoted,
};
use crate::sqlgen::portable::classify;
use crate::types::mutate::ServerExpr;
use crate::types::schema::ValueKind;

pub static LEX: LexProfile = LexProfile {
    ident_quotes: &[('"', '"')],
    string_quotes: &['\''],
    readings: &[PLAIN],
    e_strings: true,
    dollar_quotes: true,
    dollar_tags: true,
    triple_quotes: false,
    raw_prefixes: false,
    byte_prefixes: false,
    hash_comments: false,
    slash_slash_comments: false,
    // libpg_query's scanner nests them, as PostgreSQL's does.
    nested_block_comments: true,
    dash_comment_needs_space: false,
    batch_separator: None,
    terminator_optional: false,
    begin_end_bodies: false,
};

pub static RULES: ClassifyRules = ClassifyRules {
    // DuckDB queries may start with FROM; PIVOT_WIDER and PIVOT_LONGER spell PIVOT and UNPIVOT.
    queries: &[
        "SELECT",
        "WITH",
        "VALUES",
        "TABLE",
        "FROM",
        "PIVOT",
        "UNPIVOT",
        "PIVOT_WIDER",
        "PIVOT_LONGER",
    ],
    inspections: &["SHOW", "DESCRIBE", "DESC", "SUMMARIZE", "EXPLAIN", "USE"],
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
        "ATTACH",
        "DETACH",
        "INSTALL",
        "LOAD",
        "EXPORT",
        "IMPORT",
        "CHECKPOINT",
        "VACUUM",
        "CALL",
        "PRAGMA",
        "SET",
        "RESET",
    ],
    write_calls: &[
        "READ_*",
        "GLOB",
        "SNIFF_CSV",
        "PARQUET_*",
        "ICEBERG_*",
        "DELTA_*",
        "SQLITE_*",
        "POSTGRES_*",
        "MYSQL_*",
        "QUERY",
        "QUERY_TABLE",
    ],
    table_functions: Some(&[
        "RANGE",
        "GENERATE_SERIES",
        "UNNEST",
        "DUCKDB_*",
        "PRAGMA_*",
        "JSON_EACH",
        "JSON_TREE",
    ]),
};

fn classify_duckdb(sql: &str) -> StatementClass {
    classify::with(&LEX, &RULES, sql)
}

fn from_hex(hex: &str) -> String {
    format!("from_hex('{hex}')")
}

/// The SQL-standard clocks, without a precision.
fn server_expr(expr: ServerExpr, kind: ValueKind) -> Option<String> {
    match expr.clock() {
        Some((_, Some(_))) => None,
        _ => crate::sqlgen::dialect::spec::ansi_server_expr(expr, kind),
    }
}

pub static SPEC: DialectSpec = DialectSpec {
    dialect: SqlDialect::Duckdb,
    label: "DuckDB SQL",
    quote_ident: double_quoted,
    quote_literal: single_quoted,
    quote_literal_exec: single_quoted,
    placeholder: ph_dollar,
    placeholder_style: PlaceholderStyle::Dollar,
    placeholders_reusable: true,
    max_params: 10_000,
    max_name_parts: 3,
    lex: &LEX,
    rules: &RULES,
    classify: classify_duckdb,
    knobs: SqlKnobs {
        bytes_literal: from_hex,
        ci_like: CiLike::Ilike,
        returning: Returning::Returning,
        set_default: false,
        server_expr,
        catalog_words: &["INFORMATION_SCHEMA", "DUCKDB_"],
        redact: &[
            RedactRule { keyword: "SECRET" },
            RedactRule {
                keyword: "PASSWORD",
            },
            RedactRule {
                keyword: "SESSION_TOKEN",
            },
        ],
        ..SqlKnobs::ANSI
    },
    hooks: SqlHooks::NONE,
    ddl: None,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_bindings_lex_duckdb() {
        crate::sqlgen::dialect::lex::export_lex("duckdb", &LEX, &RULES);
    }

    #[test]
    fn classifies_scanners_and_external_access_as_writes() {
        assert_eq!(classify_duckdb("FROM t SELECT a"), StatementClass::Read);
        for sql in [
            "SELECT * FROM range(10)",
            "PIVOT t ON c USING sum(x)",
            "SHOW TABLES",
            "SELECT * FROM (SELECT * FROM t) b, u",
            "SELECT * FROM \"main\".\"t\" /* c */ -- d\r\n",
            "SELECT * FROM (t CROSS JOIN u)",
            "SELECT * FROM (SELECT * FROM t) b",
            "SELECT * FROM (VALUES (1), (2)) v",
            "SELECT * FROM (FROM t) b",
            "SELECT 1e5, 0x1F FROM t",
            // The clauses that end a FROM list, whose own lists hold calls.
            "SELECT * FROM t WHERE a = 1 ORDER BY a, b",
            "SELECT a, lower(b) FROM t GROUP BY a, lower(b)",
            "SELECT * FROM t UNION ALL SELECT a, lower(b) FROM u",
            // A comparison's FROM names no table.
            "SELECT a IS DISTINCT FROM lower(b) FROM t",
            "SELECT a IS NOT DISTINCT FROM lower(b) FROM t",
            "PIVOT_WIDER t ON c USING sum(x)",
            "SELECT * FROM (PIVOT_LONGER t ON COLUMNS(*))",
            // Qualified and quoted tables, whatever their names look like.
            "SELECT * FROM main.t",
            "SELECT * FROM s.json JOIN \"my.table\" ON true",
            "SELECT t.csv FROM t JOIN u ON t.csv = u.json",
        ] {
            assert_eq!(classify_duckdb(sql), StatementClass::Read, "{sql}");
        }
        for sql in [
            "SELECT content FROM read_text('x')",
            "SELECT * FROM glob('*')",
            "SELECT * FROM main.read_csv('x.csv')",
            "SELECT * FROM st_read('C:/x.shp')",
            "COPY t TO 'x.csv'",
            "ATTACH 'x.db'",
            "INSTALL httpfs",
            "LOAD httpfs",
            "EXPORT DATABASE 'dir'",
            "IMPORT DATABASE 'dir'",
            // A subquery in the FROM list ends, and the list goes on.
            "SELECT * FROM (SELECT * FROM t) b, st_read('x.shp')",
            "SELECT * FROM (SELECT * FROM t) b, evil_fn()",
            "SELECT * FROM t JOIN (SELECT * FROM u) b ON true, st_read('x.shp')",
            // A clause keyword where a name goes leaves the list open: SETTINGS, PREWHERE and
            // FORMAT are not reserved here (an alias, a column, `format()`), and any keyword may
            // follow a `.` or an `AS`.
            "SELECT * FROM t settings, st_read('x.shp')",
            "SELECT * FROM t AS settings, st_read('x.shp')",
            "SELECT * FROM t prewhere, st_read('x.shp')",
            "SELECT * FROM t format, st_read('x.shp')",
            "SELECT * FROM range(3) format, st_read('x.shp')",
            "SELECT * FROM t JOIN u ON t.a = u.order, st_read('x.shp')",
            "SELECT * FROM t JOIN u ON t.a = u.where, st_read('x.shp')",
            "SELECT * FROM t JOIN u ON format('{}', a) = b, st_read('x.shp')",
            "SELECT * FROM t JOIN u ON t.a = settings, st_read('x.shp')",
            // PIVOT, UNPIVOT, SHOW and DESCRIBE take a table, or a table function; so do the
            // DuckDB spellings of PIVOT and UNPIVOT, also inside a query.
            "PIVOT st_read('x.shp') ON column0",
            "UNPIVOT st_read('x.shp') ON COLUMNS(*)",
            "DESCRIBE st_read('x.shp')",
            "PIVOT_WIDER st_read('x.shp') ON a",
            "SELECT * FROM (PIVOT_WIDER st_read('x.shp') ON column0)",
            "WITH p AS (PIVOT_WIDER st_read('x.shp') ON a) SELECT * FROM p",
            "SELECT * FROM t, (PIVOT_LONGER st_read('x.shp') ON COLUMNS(*))",
            // A set operator may be followed by a FROM-first query.
            "SELECT 1 UNION DISTINCT FROM st_read('x.shp')",
            "SELECT 1 EXCEPT DISTINCT FROM read_csv('x.csv')",
            "SELECT * FROM t INTERSECT DISTINCT FROM st_read('C:/x.shp')",
            "SELECT 1 UNION ALL FROM st_read('x.shp')",
            // The first table of a parenthesized join.
            "SELECT * FROM (st_read('C:/x.shp') CROSS JOIN t)",
            "SELECT * FROM t JOIN (st_read('x.shp') CROSS JOIN u) ON true",
            "SELECT * FROM (FROM st_read('x.shp')) b",
            // A number ends before the keyword after it (libpg_query's lexer predates PG 15).
            "SELECT 1INTO t2 FROM t",
            "SELECT 1e0FROM st_read('x.shp')",
            "SELECT 1FROM t, st_read('x.shp')",
            "SELECT 1read_text('x')",
            // Also past a decimal point: it reads `1.e0` as one number.
            "SELECT 1.e0FROM st_read('x.shp')",
            "SELECT 1.e5INTO t2 FROM t",
            // Comments a reader may end elsewhere (libpg_query nests `/* */`).
            "/* /* */ SELECT ' */ DELETE FROM t --'",
            "/* /* */ SELECT 1 */ COMMENT ON TABLE t IS 'x'",
            "SELECT 1 -- x\r' \nDELETE FROM t --'",
        ] {
            assert_eq!(classify_duckdb(sql), StatementClass::Write, "{sql}");
        }
    }

    /// Replacement scans read like tables here: the engine settings of the session refuse them
    /// (see the module doc), so a change to this list is a change to that contract.
    #[test]
    fn leaves_files_named_as_tables_to_the_engine() {
        for sql in [
            "SELECT * FROM 'x.csv'",
            "FROM \"data.parquet\"",
            "SELECT * FROM x.\"/data.csv\"",
            "SELECT * FROM data.csv",
            "DESCRIBE 'data.parquet'",
        ] {
            assert_eq!(classify_duckdb(sql), StatementClass::Read, "{sql}");
        }
    }
}
