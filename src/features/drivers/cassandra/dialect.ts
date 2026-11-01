import { Cassandra, SQLDialect } from "@codemirror/lang-sql";
import type { StatementRange } from "@/features/editor/scanner";
import {
  PLAIN,
  scan,
  split,
  type SplitStatement,
  upper,
} from "@/features/drivers/portable/scanner";
import type { DialectLex } from "@/lib/ipc/bindings/DialectLex";
import CQL_LEX from "@/lib/ipc/bindings/lex/cql.json";
import type { DialectUi } from "../types";

/**
 * CQL in the editor: CodeMirror's Cassandra dialect with more of CQL's words, `$$` bodies and `//`
 * comments, and the names completion offers; no formatter (sql-formatter has no CQL). Safe Mode
 * comes from the backend's lexer profile (`bindings/lex/cql.json`); statements split as
 * `dialect::cql::split_cql` splits them (`splitCql`), a batch kept whole.
 */

const LEX = CQL_LEX as unknown as DialectLex;

/** Words of CQL and cqlsh that CodeMirror's Cassandra dialect lacks. */
const MORE_KEYWORDS = [
  "describe",
  "entries",
  "expand",
  "full",
  "is",
  "json",
  "keys",
  "list",
  "login",
  "materialized",
  "nologin",
  "null",
  "options",
  "paging",
  "partition",
  "replication",
  "role",
  "roles",
  "serial",
  "local_serial",
  "tracing",
  "view",
  "durable_writes",
  "false",
  "true",
].join(" ");

/** The CQL types (native, collections and `vector`). */
export const CQL_COLUMN_TYPES = [
  "ascii",
  "bigint",
  "blob",
  "boolean",
  "counter",
  "date",
  "decimal",
  "double",
  "duration",
  "float",
  "inet",
  "int",
  "smallint",
  "text",
  "time",
  "timestamp",
  "timeuuid",
  "tinyint",
  "uuid",
  "varchar",
  "varint",
  "list<text>",
  "set<text>",
  "map<text, text>",
  "frozen<list<text>>",
  "tuple<int, text>",
  "vector<float, 3>",
] as const;

/**
 * CodeMirror's dialect: its Cassandra one with cqlsh's and newer CQL's words, the CQL types, `$$`
 * bodies and `//` comments.
 */
export const CQL_SQL = SQLDialect.define({
  ...Cassandra.spec,
  keywords: `${Cassandra.spec.keywords ?? ""} ${MORE_KEYWORDS}`,
  types: [
    ...CQL_COLUMN_TYPES.filter((t) => /^\w+$/.test(t)),
    "frozen",
    "list",
    "set",
    "map",
    "tuple",
    "vector",
  ].join(" "),
  doubleDollarQuotedStrings: true,
  slashComments: true,
  backslashEscapes: false,
});

/** Built-in functions completion offers, as CQL spells them. */
export const CQL_FUNCTIONS = [
  // Aggregates
  "count",
  "min",
  "max",
  "sum",
  "avg",
  // Time and uuids
  "now",
  "currentTimestamp",
  "currentDate",
  "currentTime",
  "currentTimeUUID",
  "toDate",
  "toTimestamp",
  "toUnixTimestamp",
  "minTimeuuid",
  "maxTimeuuid",
  "uuid",
  // Cells and partitions
  "token",
  "ttl",
  "writetime",
  "toJson",
  "fromJson",
  // Conversions
  "cast",
  "blobAsText",
  "textAsBlob",
  "blobAsBigint",
  "bigintAsBlob",
] as const;

/** `"name"` with `"` doubled, as `sqlgen::dialect::cql` quotes every identifier. */
export const quoted = (name: string): string => `"${name.replaceAll('"', '""')}"`;

/** The tokens of code of a statement, upper case (strings, names and punctuation as ""). */
function codeWords(sql: string): string[] {
  const s = scan(LEX.lex, PLAIN, sql);
  return s.tokens.flatMap((t) =>
    t.kind === "space" || t.kind === "comment" ? [] : [t.kind === "word" ? upper(s, t) : ""],
  );
}

/** `BEGIN [UNLOGGED | COUNTER] BATCH …` */
function opensBatch(w: string[]): boolean {
  return (
    w[0] === "BEGIN" &&
    (w[1] === "BATCH" || (/^(UNLOGGED|COUNTER)$/.test(w[1] ?? "") && w[2] === "BATCH"))
  );
}

/** `… APPLY BATCH` */
const closesBatch = (w: string[]): boolean => w.at(-2) === "APPLY" && w.at(-1) === "BATCH";

/**
 * `dialect::cql::split_cql`: the portable split, with a batch (`BEGIN [UNLOGGED | COUNTER]
 * BATCH` up to the statement that ends with `APPLY BATCH`, or the end of the text) kept as one
 * statement. Offsets are UTF-16, as the editor's.
 */
export function splitCql(text: string): SplitStatement[] {
  const parts = split(LEX.lex, text);
  const out: SplitStatement[] = [];
  for (let i = 0; i < parts.length; i++) {
    const first = parts[i]!;
    const words = codeWords(first.sql);
    if (!opensBatch(words) || closesBatch(words)) {
      out.push(first);
      continue;
    }
    let end = first.end;
    while (++i < parts.length) {
      end = parts[i]!.end;
      if (closesBatch(codeWords(parts[i]!.sql))) break;
    }
    out.push({ sql: text.slice(first.start, end), start: first.start, end, line: first.line });
  }
  return out;
}

/** The editor's statement ranges of `text` (`splitCql` without the text). */
export const cqlStatements = (text: string): StatementRange[] =>
  splitCql(text).map(({ start, end, line }) => ({ start, end, line }));

/** The lane's dialect; the editor splits with `cqlStatements`, so a batch stays whole. */
export const cqlUi: DialectUi = {
  label: "CQL",
  codemirror: () => CQL_SQL,
  formatter: null,
  functions: CQL_FUNCTIONS,
  columnTypes: CQL_COLUMN_TYPES,
  quoteIdent: quoted,
  split: cqlStatements,
};
