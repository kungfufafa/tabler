import { PostgreSQL, SQLDialect } from "@codemirror/lang-sql";
import type { DialectUi } from "../types";

/**
 * DuckDB SQL in the editor: PostgreSQL's highlighting (double-quoted names, `E''` strings, dollar
 * quotes, `::` casts) with DuckDB's friendly-SQL words, and the names completion offers. Splitting
 * and Safe Mode come from the backend's lexer profile (`bindings/lex/duckdb.json`). No formatter:
 * the session's caps do not grant Format.
 */

/** Words DuckDB adds to PostgreSQL's (friendly SQL, files, extensions, PIVOT). */
const DUCKDB_KEYWORDS = [
  "anti",
  "asof",
  "attach",
  "columns",
  "describe",
  "detach",
  "exclude",
  "export",
  "glob",
  "ilike",
  "import",
  "install",
  "load",
  "macro",
  "pivot",
  "pivot_longer",
  "pivot_wider",
  "positional",
  "pragma",
  "qualify",
  "replace",
  "sample",
  "semi",
  "summarize",
  "unpivot",
  "use",
].join(" ");

/** DuckDB's own type words after PostgreSQL's. */
const DUCKDB_TYPE_WORDS =
  "tinyint utinyint usmallint uinteger ubigint hugeint uhugeint bignum varint blob bit list struct map union enum uuid json timestamptz timetz timestamp_s timestamp_ms timestamp_ns geometry variant";

/** CodeMirror's dialect: PostgreSQL's, with DuckDB's words. */
export const DUCKDB_SQL = SQLDialect.define({
  ...PostgreSQL.spec,
  keywords: `${PostgreSQL.spec.keywords ?? ""} ${DUCKDB_KEYWORDS}`,
  types: `${PostgreSQL.spec.types ?? ""} ${DUCKDB_TYPE_WORDS}`,
});

/**
 * Functions completion offers (as DuckDB's documentation spells them). Only functions that read:
 * the scanners (`read_*`, `glob`), `nextval`, the logger switches and dynamic SQL are the
 * classifier's writes, and `dialect.test.ts` checks that none of these is one.
 */
export const DUCKDB_FUNCTIONS = [
  // Aggregates
  "any_value",
  "approx_count_distinct",
  "arg_max",
  "arg_min",
  "avg",
  "bool_and",
  "bool_or",
  "count",
  "first",
  "histogram",
  "last",
  "list",
  "max",
  "median",
  "min",
  "mode",
  "quantile_cont",
  "quantile_disc",
  "stddev_samp",
  "string_agg",
  "sum",
  "var_samp",
  // Windows
  "dense_rank",
  "lag",
  "lead",
  "ntile",
  "rank",
  "row_number",
  // Text
  "concat",
  "concat_ws",
  "contains",
  "ends_with",
  "format",
  "length",
  "lower",
  "lpad",
  "ltrim",
  "regexp_extract",
  "regexp_matches",
  "regexp_replace",
  "replace",
  "reverse",
  "rpad",
  "rtrim",
  "split_part",
  "starts_with",
  "string_split",
  "strip_accents",
  "substring",
  "trim",
  "upper",
  // Numbers
  "abs",
  "ceil",
  "floor",
  "greatest",
  "least",
  "ln",
  "log10",
  "pow",
  "round",
  "sqrt",
  // Dates and times
  "age",
  "current_date",
  "date_diff",
  "date_part",
  "date_trunc",
  "epoch",
  "epoch_ms",
  "make_date",
  "make_timestamp",
  "now",
  "strftime",
  "strptime",
  "to_timestamp",
  // Lists, structs and maps
  "array_length",
  "flatten",
  "list_aggregate",
  "list_contains",
  "list_distinct",
  "list_filter",
  "list_sort",
  "list_transform",
  "list_value",
  "map_extract",
  "map_keys",
  "map_values",
  "struct_extract",
  "struct_pack",
  "unnest",
  // JSON
  "json_array_length",
  "json_extract",
  "json_extract_string",
  "json_keys",
  "json_object",
  "json_type",
  "json_valid",
  "to_json",
  // Others
  "coalesce",
  "currval",
  "current_database",
  "current_schema",
  "current_setting",
  "gen_random_uuid",
  "hash",
  "ifnull",
  "md5",
  "nullif",
  "random",
  "sha256",
  "try_cast",
  "typeof",
  "uuid",
  "version",
] as const;

/** Column types of the Structure tab and completion, as `duckdb_columns()` spells them. */
export const DUCKDB_COLUMN_TYPES = [
  "BOOLEAN",
  "TINYINT",
  "SMALLINT",
  "INTEGER",
  "BIGINT",
  "HUGEINT",
  "UTINYINT",
  "USMALLINT",
  "UINTEGER",
  "UBIGINT",
  "UHUGEINT",
  "BIGNUM",
  "FLOAT",
  "DOUBLE",
  "DECIMAL",
  "VARCHAR",
  "BLOB",
  "BIT",
  "DATE",
  "TIME",
  "TIME WITH TIME ZONE",
  "TIMESTAMP",
  "TIMESTAMP_S",
  "TIMESTAMP_MS",
  "TIMESTAMP_NS",
  "TIMESTAMP WITH TIME ZONE",
  "INTERVAL",
  "UUID",
  "JSON",
  "VARCHAR[]",
  "INTEGER[]",
  "STRUCT",
  "MAP",
  "UNION",
  "GEOMETRY",
] as const;

/** `"name"` with `"` doubled, as `sqlgen::dialect::duckdb` quotes. */
export const doubleQuoted = (name: string): string => `"${name.replaceAll('"', '""')}"`;

export const duckdbSqlUi: DialectUi = {
  label: "DuckDB SQL",
  codemirror: () => DUCKDB_SQL,
  formatter: null,
  functions: DUCKDB_FUNCTIONS,
  columnTypes: DUCKDB_COLUMN_TYPES,
  quoteIdent: doubleQuoted,
};
