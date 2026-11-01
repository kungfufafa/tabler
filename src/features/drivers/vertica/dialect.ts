import { PostgreSQL, SQLDialect } from "@codemirror/lang-sql";
import type { DialectUi } from "../types";

/**
 * Vertica SQL in the editor: PostgreSQL's highlighting (double-quoted names, `E''` strings, dollar
 * quotes, `::` casts) with Vertica's own words, and the names completion offers. Splitting and
 * Safe Mode come from the backend's lexer profile (`bindings/lex/vertica.json`), nested block
 * comments included. No formatter: the session's caps do not grant Format, and sql-formatter has
 * no Vertica language (its PostgreSQL one does not know `SEGMENTED BY` or `AT EPOCH`).
 */

/** Words Vertica adds to PostgreSQL's (projections, historical reads, COPY options, flex). */
const VERTICA_KEYWORDS = [
  "auto",
  "copy",
  "direct",
  "enclosed",
  "encoding",
  "epoch",
  "exceptions",
  "export",
  "flex",
  "gapfill",
  "ilike",
  "interpolate",
  "ksafe",
  "latest",
  "match",
  "merge",
  "nodes",
  "parquet",
  "pattern",
  "profile",
  "projection",
  "rejected",
  "rle",
  "segmented",
  "timeseries",
  "unsegmented",
].join(" ");

/** Vertica's own type words after PostgreSQL's. */
const VERTICA_TYPE_WORDS =
  "binary varbinary long raw timetz timestamptz datetime smalldatetime money number tinyint int8 float8 geometry geography uuid row set map";

/** CodeMirror's dialect: PostgreSQL's, with Vertica's words. */
export const VERTICA_SQL = SQLDialect.define({
  ...PostgreSQL.spec,
  keywords: `${PostgreSQL.spec.keywords ?? ""} ${VERTICA_KEYWORDS}`,
  types: `${PostgreSQL.spec.types ?? ""} ${VERTICA_TYPE_WORDS}`,
});

/**
 * Functions completion offers (upper case, as the SQL reference spells them). Only functions that
 * read: the meta-functions that change state are the classifier's `write_calls`, and
 * `dialect.test.ts` checks that none of these is one.
 */
export const VERTICA_FUNCTIONS = [
  // Aggregates
  "AVG",
  "COUNT",
  "MAX",
  "MIN",
  "SUM",
  "STDDEV",
  "VARIANCE",
  "LISTAGG",
  "APPROXIMATE_COUNT_DISTINCT",
  "APPROXIMATE_MEDIAN",
  "APPROXIMATE_PERCENTILE",
  "BIT_AND",
  "BIT_OR",
  "BOOL_AND",
  "BOOL_OR",
  // Analytics and time series
  "ROW_NUMBER",
  "RANK",
  "DENSE_RANK",
  "NTILE",
  "LAG",
  "LEAD",
  "FIRST_VALUE",
  "LAST_VALUE",
  "MEDIAN",
  "PERCENTILE_CONT",
  "CONDITIONAL_CHANGE_EVENT",
  "CONDITIONAL_TRUE_EVENT",
  "EXPONENTIAL_MOVING_AVERAGE",
  "TIME_SLICE",
  "TS_FIRST_VALUE",
  "TS_LAST_VALUE",
  // Strings
  "LENGTH",
  "CHARACTER_LENGTH",
  "OCTET_LENGTH",
  "SUBSTR",
  "SUBSTRING",
  "POSITION",
  "INSTR",
  "LEFT",
  "RIGHT",
  "LPAD",
  "RPAD",
  "LTRIM",
  "RTRIM",
  "BTRIM",
  "UPPER",
  "LOWER",
  "INITCAP",
  "REPLACE",
  "TRANSLATE",
  "REPEAT",
  "CONCAT",
  "SPLIT_PART",
  "REGEXP_LIKE",
  "REGEXP_REPLACE",
  "REGEXP_SUBSTR",
  "REGEXP_COUNT",
  "MD5",
  "SHA1",
  "SHA256",
  "TO_HEX",
  // Dates and times
  "NOW",
  "SYSDATE",
  "GETDATE",
  "STATEMENT_TIMESTAMP",
  "DATE_TRUNC",
  "DATE_PART",
  "DATEADD",
  "DATEDIFF",
  "TIMESTAMPADD",
  "TIMESTAMPDIFF",
  "ADD_MONTHS",
  "MONTHS_BETWEEN",
  "LAST_DAY",
  "NEXT_DAY",
  "TO_CHAR",
  "TO_DATE",
  "TO_TIMESTAMP",
  "TO_NUMBER",
  "YEAR",
  "MONTH",
  "DAY",
  "HOUR",
  "WEEK",
  "QUARTER",
  // Conversion and logic
  "CAST",
  "COALESCE",
  "NVL",
  "NVL2",
  "NULLIF",
  "NULLIFZERO",
  "ZEROIFNULL",
  "DECODE",
  "GREATEST",
  "LEAST",
  // Math
  "ABS",
  "CEILING",
  "FLOOR",
  "ROUND",
  "TRUNC",
  "MOD",
  "POWER",
  "SQRT",
  "EXP",
  "LN",
  "LOG",
  "HASH",
  // Flex tables and JSON
  "MAPLOOKUP",
  "MAPKEYS",
  "MAPVALUES",
  "MAPTOSTRING",
  "MAPSIZE",
  "MAPCONTAINSKEY",
  // Session and catalog
  "VERSION",
  "CURRENT_DATABASE",
  "CURRENT_SCHEMA",
  "CURRENT_USER",
  "SESSION_USER",
  "LAST_INSERT_ID",
  "GET_COMPLIANCE_STATUS",
] as const;

/** Column types of the Structure tab and completion, as `v_catalog.columns` spells them. */
export const VERTICA_COLUMN_TYPES = [
  "boolean",
  "int",
  "integer",
  "bigint",
  "smallint",
  "tinyint",
  "float",
  "real",
  "double precision",
  "numeric",
  "number",
  "money",
  "char",
  "varchar",
  "long varchar",
  "binary",
  "varbinary",
  "long varbinary",
  "date",
  "time",
  "timetz",
  "timestamp",
  "timestamptz",
  "interval",
  "interval year to month",
  "uuid",
  "geometry",
  "geography",
  "array",
  "set",
  "row",
] as const;

/** `"name"` with `"` doubled, as `sqlgen::dialect::vertica` quotes. */
export const doubleQuoted = (name: string): string => `"${name.replaceAll('"', '""')}"`;

export const verticaSqlUi: DialectUi = {
  label: "Vertica SQL",
  codemirror: () => VERTICA_SQL,
  formatter: null,
  functions: VERTICA_FUNCTIONS,
  columnTypes: VERTICA_COLUMN_TYPES,
  quoteIdent: doubleQuoted,
};
