import { SQLDialect } from "@codemirror/lang-sql";
import type { DialectUi } from "../types";

/**
 * GoogleSQL in the editor: a CodeMirror dialect with BigQuery's words, backticked names, `'…'` and
 * `"…"` strings with backslash escapes, `#` and `--` comments, and the names completion offers;
 * no formatter (the caps do not grant Format). Splitting (scripting blocks included) and Safe
 * Mode come from the backend's lexer profile (`bindings/lex/bigquery.json`), which the editor
 * loads by the session's dialect.
 */

/** Statement and clause words beyond standard SQL's (lower case, as CodeMirror takes them). */
const KEYWORDS = [
  // Standard SQL the dialect needs listed
  "select from where and or not in is null like between as on using join inner left right",
  "full outer cross all distinct group by order having limit offset union intersect except",
  "with recursive case when then else end exists insert into values update delete set create",
  "table view drop alter add column rename truncate if primary key default asc desc nulls first",
  "last true false cast interval over partition window rows range unbounded preceding following",
  "current row merge matched by target source",
  // GoogleSQL's own
  "qualify struct array unnest safe_cast replace except temp temporary materialized external",
  "snapshot clone schema options cluster foreign references not enforced function returns",
  "language remote connection procedure declare begin exception raise return loop while repeat",
  "until for do break leave continue iterate call execute immediate transaction commit rollback",
  "assert export data load overwrite model search index vector row access policy grant revoke",
  "to filter pivot unpivot tablesample system percent at system_time of lateral within",
  "information_schema region project dataset capacity reservation assignment undrop",
].join(" ");

/** Built-in functions completion offers, as GoogleSQL spells them. */
export const BIGQUERY_FUNCTIONS = [
  // Aggregates
  "COUNT",
  "COUNTIF",
  "SUM",
  "AVG",
  "MIN",
  "MAX",
  "ANY_VALUE",
  "ARRAY_AGG",
  "STRING_AGG",
  "LOGICAL_AND",
  "LOGICAL_OR",
  "APPROX_COUNT_DISTINCT",
  "APPROX_QUANTILES",
  "APPROX_TOP_COUNT",
  "MAX_BY",
  "MIN_BY",
  "STDDEV",
  "VARIANCE",
  "CORR",
  // Window
  "ROW_NUMBER",
  "RANK",
  "DENSE_RANK",
  "PERCENT_RANK",
  "NTILE",
  "LAG",
  "LEAD",
  "FIRST_VALUE",
  "LAST_VALUE",
  "NTH_VALUE",
  // Dates and times
  "CURRENT_DATE",
  "CURRENT_DATETIME",
  "CURRENT_TIME",
  "CURRENT_TIMESTAMP",
  "DATE",
  "DATETIME",
  "TIMESTAMP",
  "DATE_ADD",
  "DATE_SUB",
  "DATE_DIFF",
  "DATE_TRUNC",
  "DATETIME_TRUNC",
  "TIMESTAMP_ADD",
  "TIMESTAMP_SUB",
  "TIMESTAMP_DIFF",
  "TIMESTAMP_TRUNC",
  "TIMESTAMP_SECONDS",
  "TIMESTAMP_MILLIS",
  "TIMESTAMP_MICROS",
  "UNIX_SECONDS",
  "UNIX_MILLIS",
  "EXTRACT",
  "FORMAT_DATE",
  "FORMAT_TIMESTAMP",
  "PARSE_DATE",
  "PARSE_TIMESTAMP",
  "GENERATE_DATE_ARRAY",
  "LAST_DAY",
  // Strings
  "CONCAT",
  "LENGTH",
  "CHAR_LENGTH",
  "LOWER",
  "UPPER",
  "TRIM",
  "LTRIM",
  "RTRIM",
  "SUBSTR",
  "STRPOS",
  "REPLACE",
  "SPLIT",
  "STARTS_WITH",
  "ENDS_WITH",
  "CONTAINS_SUBSTR",
  "REGEXP_CONTAINS",
  "REGEXP_EXTRACT",
  "REGEXP_REPLACE",
  "FORMAT",
  "LPAD",
  "RPAD",
  "REVERSE",
  "TO_HEX",
  "FROM_HEX",
  "TO_BASE64",
  "FROM_BASE64",
  "NORMALIZE",
  // Conversion and conditionals
  "CAST",
  "SAFE_CAST",
  "COALESCE",
  "IFNULL",
  "NULLIF",
  "IF",
  "GREATEST",
  "LEAST",
  // Math
  "ABS",
  "ROUND",
  "TRUNC",
  "CEIL",
  "FLOOR",
  "MOD",
  "POW",
  "SQRT",
  "LOG",
  "EXP",
  "SAFE_DIVIDE",
  "RAND",
  "GENERATE_UUID",
  // Arrays
  "ARRAY_LENGTH",
  "ARRAY_CONCAT",
  "ARRAY_TO_STRING",
  "ARRAY_REVERSE",
  "GENERATE_ARRAY",
  "OFFSET",
  "ORDINAL",
  "SAFE_OFFSET",
  // JSON
  "JSON_VALUE",
  "JSON_QUERY",
  "JSON_EXTRACT_SCALAR",
  "JSON_VALUE_ARRAY",
  "JSON_QUERY_ARRAY",
  "JSON_TYPE",
  "PARSE_JSON",
  "TO_JSON",
  "TO_JSON_STRING",
  // Geography
  "ST_GEOGPOINT",
  "ST_GEOGFROMTEXT",
  "ST_ASTEXT",
  "ST_DISTANCE",
  "ST_DWITHIN",
  "ST_AREA",
  "ST_CONTAINS",
  "ST_INTERSECTS",
  // Hashing and the rest
  "FARM_FINGERPRINT",
  "MD5",
  "SHA256",
  "SESSION_USER",
  "ERROR",
] as const;

/** Column types the structure and completion offer, as GoogleSQL writes them. */
export const BIGQUERY_COLUMN_TYPES = [
  "INT64",
  "NUMERIC",
  "NUMERIC(10, 2)",
  "BIGNUMERIC",
  "FLOAT64",
  "BOOL",
  "STRING",
  "STRING(255)",
  "BYTES",
  "DATE",
  "TIME",
  "DATETIME",
  "TIMESTAMP",
  "INTERVAL",
  "JSON",
  "GEOGRAPHY",
  "ARRAY<STRING>",
  "STRUCT<name STRING, value INT64>",
  "RANGE<DATE>",
] as const;

/** `` `name` `` with backslash and backtick escaped, as `spec::backticked_backslash` quotes. */
export const backticked = (name: string): string =>
  `\`${name.replaceAll("\\", "\\\\").replaceAll("`", "\\`")}\``;

/** CodeMirror's dialect (types: the bare names of `BIGQUERY_COLUMN_TYPES`). */
export const BIGQUERY_SQL = SQLDialect.define({
  keywords: KEYWORDS,
  builtin: BIGQUERY_FUNCTIONS.map((f) => f.toLowerCase()).join(" "),
  types: [...new Set(BIGQUERY_COLUMN_TYPES.map((t) => t.replace(/[(<].*$/, "").toLowerCase()))]
    .concat(["int", "integer", "bigint", "smallint", "decimal", "bigdecimal", "float"])
    .join(" "),
  identifierQuotes: "`",
  doubleQuotedStrings: true,
  backslashEscapes: true,
  hashComments: true,
  slashComments: false,
});

export const bigquerySqlUi: DialectUi = {
  label: "GoogleSQL",
  codemirror: () => BIGQUERY_SQL,
  // sql-formatter has a `bigquery` language, but the caps do not grant Format (the backend's
  // brief: not tested yet), so the button stays hidden and nothing would call it.
  formatter: null,
  functions: BIGQUERY_FUNCTIONS,
  columnTypes: BIGQUERY_COLUMN_TYPES,
  quoteIdent: backticked,
};
