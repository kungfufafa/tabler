import { PostgreSQL, SQLDialect } from "@codemirror/lang-sql";
import type { DialectUi } from "../types";

/**
 * Redshift SQL in the editor: PostgreSQL's highlighting with Redshift's own words, backslash
 * escapes in strings (the server reads them, `sqlgen::dialect::redshift`), sql-formatter's
 * `redshift` language and the names completion offers. Splitting and Safe Mode come from the
 * backend's lexer profile (`bindings/lex/redshift.json`). CockroachDB and Greenplum sessions
 * speak PostgreSQL and use the app's own PostgreSQL support.
 */

/** Words Redshift adds to PostgreSQL 8.0's (DDL attributes, COPY / UNLOAD options, PartiQL). */
const REDSHIFT_KEYWORDS = [
  "approximate",
  "auto",
  "backup",
  "compound",
  "credentials",
  "dateformat",
  "delimiter",
  "diststyle",
  "distkey",
  "encode",
  "even",
  "external",
  "format",
  "gzip",
  "iam_role",
  "identity",
  "interleaved",
  "late",
  "listagg",
  "manifest",
  "parquet",
  "pivot",
  "qualify",
  "sortkey",
  "spectrum",
  "timeformat",
  "top",
  "unload",
  "unpivot",
  "zstd",
].join(" ");

/** Redshift's own types (SUPER, GEOMETRY, HLLSKETCH, VARBYTE) after PostgreSQL's. */
const REDSHIFT_TYPE_WORDS =
  "super geometry geography hllsketch varbyte varbinary timetz timestamptz";

/** CodeMirror's dialect: PostgreSQL's, with backslash escapes and Redshift's words. */
export const REDSHIFT_SQL = SQLDialect.define({
  ...PostgreSQL.spec,
  keywords: `${PostgreSQL.spec.keywords ?? ""} ${REDSHIFT_KEYWORDS}`,
  types: `${PostgreSQL.spec.types ?? ""} ${REDSHIFT_TYPE_WORDS}`,
  backslashEscapes: true,
});

/** Functions completion offers (upper case, as AWS documents them). */
export const REDSHIFT_FUNCTIONS = [
  // Aggregates and windows
  "AVG",
  "COUNT",
  "LISTAGG",
  "MAX",
  "MEDIAN",
  "MIN",
  "PERCENTILE_CONT",
  "STDDEV",
  "SUM",
  "VARIANCE",
  "RATIO_TO_REPORT",
  "ROW_NUMBER",
  "RANK",
  "DENSE_RANK",
  "NTILE",
  "LAG",
  "LEAD",
  "FIRST_VALUE",
  "LAST_VALUE",
  // Strings
  "BTRIM",
  "CHARINDEX",
  "CONCAT",
  "INITCAP",
  "LEFT",
  "LEN",
  "LENGTH",
  "LOWER",
  "LPAD",
  "LTRIM",
  "POSITION",
  "REGEXP_COUNT",
  "REGEXP_INSTR",
  "REGEXP_REPLACE",
  "REGEXP_SUBSTR",
  "REPLACE",
  "REPLICATE",
  "REVERSE",
  "RIGHT",
  "RPAD",
  "RTRIM",
  "SPLIT_PART",
  "STRPOS",
  "SUBSTRING",
  "TRANSLATE",
  "TRIM",
  "UPPER",
  // Dates
  "ADD_MONTHS",
  "CONVERT_TIMEZONE",
  "CURRENT_DATE",
  "DATE_PART",
  "DATE_TRUNC",
  "DATEADD",
  "DATEDIFF",
  "EXTRACT",
  "GETDATE",
  "LAST_DAY",
  "MONTHS_BETWEEN",
  "NEXT_DAY",
  "SYSDATE",
  "TIMEOFDAY",
  "TRUNC",
  // Conversion and logic
  "CAST",
  "COALESCE",
  "CONVERT",
  "DECODE",
  "GREATEST",
  "LEAST",
  "NULLIF",
  "NVL",
  "NVL2",
  "TO_CHAR",
  "TO_DATE",
  "TO_NUMBER",
  "TO_TIMESTAMP",
  // Math and hashes
  "ABS",
  "CEILING",
  "FLOOR",
  "MD5",
  "RANDOM",
  "ROUND",
  "SHA2",
  "SQRT",
  // SUPER, JSON and spatial
  "IS_ARRAY",
  "IS_OBJECT",
  "JSON_EXTRACT_PATH_TEXT",
  "JSON_PARSE",
  "JSON_SERIALIZE",
  "JSON_TYPEOF",
  "ST_AsEWKT",
  "ST_AsText",
  "ST_GeomFromText",
  "TO_HEX",
  // Session and system information
  "CURRENT_SCHEMA",
  "CURRENT_USER",
  "HAS_TABLE_PRIVILEGE",
  "PG_BACKEND_PID",
  "PG_LAST_QUERY_ID",
  "VERSION",
] as const;

/** Column types of the Structure tab and completion (Redshift's, as it spells them). */
export const REDSHIFT_COLUMN_TYPES = [
  "smallint",
  "integer",
  "bigint",
  "decimal",
  "real",
  "double precision",
  "boolean",
  "char",
  "varchar",
  "date",
  "time",
  "timetz",
  "timestamp",
  "timestamptz",
  "interval year to month",
  "interval day to second",
  "super",
  "varbyte",
  "geometry",
  "geography",
  "hllsketch",
] as const;

/** `"name"` with `"` doubled, as `sqlgen::dialect::redshift` quotes (`double_quoted`). */
export const doubleQuoted = (name: string): string => `"${name.replaceAll('"', '""')}"`;

export const redshiftSqlUi: DialectUi = {
  label: "Redshift SQL",
  codemirror: () => REDSHIFT_SQL,
  formatter: "redshift",
  functions: REDSHIFT_FUNCTIONS,
  columnTypes: REDSHIFT_COLUMN_TYPES,
  quoteIdent: doubleQuoted,
};
