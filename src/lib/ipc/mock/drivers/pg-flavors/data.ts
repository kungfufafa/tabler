import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { RoutineSummary } from "../../../bindings/RoutineSummary";
import type { TableKind } from "../../../bindings/TableKind";
import type { TableOptions } from "../../../bindings/TableOptions";
import type { Value } from "../../../bindings/Value";
import type { ValueKind } from "../../../bindings/ValueKind";
import type { Engine } from "./caps";

/**
 * The data of the mock's engines, shaped as the backend's catalog hooks read it
 * (`flavor/imp/*.rs`): CockroachDB's hidden `rowid` key and string session ids, Greenplum's
 * distribution and storage (by version) with a partitioned table, Redshift's DISTSTYLE / SORTKEY,
 * column attributes in `extra`, a SUPER column, sizes, a Spectrum table and its routines.
 */

export interface Col {
  name: string;
  type: string;
  kind: ValueKind;
  nullable?: boolean;
  pk?: boolean;
  auto?: boolean;
  generated?: boolean;
  default?: string;
  extra?: string;
}

export interface Table {
  schema: string;
  name: string;
  kind: TableKind;
  cols: Col[];
  rows: Value[][];
  options: TableOptions | null;
  ddl: string;
  sizeBytes: number | null;
  comment: string | null;
}

export interface Routine extends RoutineSummary {
  /** The definition Show shows; a string starting with "!" is the backend's refusal. */
  definition: string;
}

export interface Process {
  id: string;
  user: string;
  query: string;
  state: string;
  application: string;
}

export interface EngineData {
  databases: string[];
  schemas: string[];
  tables: Table[];
  routines: Routine[];
  processes: Process[];
}

export const NULL: Value = { t: "null" };
const int = (v: number): Value => ({ t: "int", v });
const text = (v: string): Value => ({ t: "text", v });
const dec = (v: string): Value => ({ t: "decimal", v });
const uuid = (i: number): Value => ({
  t: "uuid",
  v: `${(0x1000_0000 + i * 7919).toString(16)}-0000-4000-8000-${String(i).padStart(12, "0")}`,
});
const ts = (i: number): Value => ({
  t: "timestampTz",
  v: `2026-0${(i % 9) + 1}-${String((i % 27) + 1).padStart(2, "0")} 10:${String(i % 60).padStart(2, "0")}:00+00`,
});
const date = (i: number): Value => ({
  t: "date",
  v: `2026-${String((i % 12) + 1).padStart(2, "0")}-${String((i % 27) + 1).padStart(2, "0")}`,
});

const CITIES = ["amsterdam", "boston", "new york", "paris", "rome", "seattle"];
const NAMES = ["Ada", "Grace", "Linus", "Barbara", "Edsger", "Margaret", "Ken", "Frances"];

const range = (n: number) => Array.from({ length: n }, (_, i) => i + 1);

/** `ColumnMeta` of a column (`introspect::table_meta`'s shape). */
export function columnMeta(c: Col, i: number, pk: readonly string[]): ColumnMeta {
  const at = pk.indexOf(c.name);
  return {
    name: c.name,
    ordinal: i + 1,
    dataType: c.type,
    kind: c.kind,
    nullable: c.nullable ?? !c.pk,
    defaultExpr: c.default ?? null,
    isPk: at >= 0,
    pkOrdinal: at >= 0 ? at + 1 : null,
    isAutoIncrement: c.auto ?? false,
    isGenerated: c.generated ?? false,
    isUnsigned: false,
    comment: null,
    enumValues: null,
    charMaxLen:
      /\((\d+)\)$/.exec(c.type) && c.kind === "text" ? Number(/\((\d+)\)$/.exec(c.type)![1]) : null,
    numericPrecision: null,
    numericScale: null,
    collation: null,
    arrayElemType: null,
    extra: c.extra ?? null,
  };
}

export const pkOf = (t: Table): string[] => t.cols.filter((c) => c.pk).map((c) => c.name);

function cockroach(): EngineData {
  const users: Table = {
    schema: "public",
    name: "users",
    kind: "table",
    cols: [
      {
        name: "id",
        type: "uuid",
        kind: "uuid",
        pk: true,
        auto: true,
        default: "gen_random_uuid()",
      },
      { name: "name", type: "text", kind: "text", nullable: false },
      { name: "city", type: "character varying(32)", kind: "text" },
      {
        name: "created_at",
        type: "timestamp with time zone",
        kind: "timestampTz",
        default: "now()",
      },
    ],
    rows: range(30).map((i) => [
      uuid(i),
      text(NAMES[i % NAMES.length]!),
      text(CITIES[i % CITIES.length]!),
      ts(i),
    ]),
    options: null,
    ddl: "CREATE TABLE public.users (\n\tid UUID NOT NULL DEFAULT gen_random_uuid(),\n\tname STRING NOT NULL,\n\tcity VARCHAR(32) NULL,\n\tcreated_at TIMESTAMPTZ NULL DEFAULT now():::TIMESTAMPTZ,\n\tCONSTRAINT users_pkey PRIMARY KEY (id ASC)\n)",
    sizeBytes: null,
    comment: null,
  };
  // No primary key: CockroachDB keys the table by its hidden, generated `rowid`.
  const rides: Table = {
    schema: "public",
    name: "rides",
    kind: "table",
    cols: [
      { name: "city", type: "text", kind: "text" },
      { name: "revenue", type: "numeric(10,2)", kind: "decimal" },
      {
        name: "rowid",
        type: "bigint",
        kind: "bigInt",
        pk: true,
        auto: true,
        generated: true,
        default: "unique_rowid()",
      },
    ],
    rows: range(45).map((i) => [
      text(CITIES[i % CITIES.length]!),
      dec(`${(i * 3.75).toFixed(2)}`),
      { t: "bigInt", v: String(1_017_458_234_872_201_217n + BigInt(i)) },
    ]),
    options: null,
    ddl: "CREATE TABLE public.rides (\n\tcity STRING NULL,\n\trevenue DECIMAL(10,2) NULL,\n\trowid INT8 NOT VISIBLE NOT NULL DEFAULT unique_rowid(),\n\tCONSTRAINT rides_pkey PRIMARY KEY (rowid ASC)\n)",
    sizeBytes: null,
    comment: null,
  };
  return {
    databases: ["defaultdb", "movr", "postgres", "system"],
    schemas: ["public"],
    tables: [users, rides],
    routines: [
      {
        schema: "public",
        name: "add_one",
        kind: "function",
        arguments: "x bigint",
        identityArguments: "x bigint",
        returnType: "bigint",
        language: "sql",
        comment: null,
        definition:
          "CREATE FUNCTION public.add_one(x INT8)\n\tRETURNS INT8\n\tIMMUTABLE\n\tLANGUAGE SQL\n\tAS $$\n\tSELECT x + 1;\n$$",
      },
      {
        schema: "public",
        name: "archive_rides",
        kind: "procedure",
        arguments: "days bigint",
        identityArguments: "days bigint",
        returnType: null,
        language: "plpgsql",
        comment: null,
        definition:
          "CREATE PROCEDURE public.archive_rides(days INT8)\n\tLANGUAGE plpgsql\n\tAS $$\n\tBEGIN\n\tDELETE FROM rides WHERE revenue = 0;\n\tEND;\n$$",
      },
    ],
    processes: [
      {
        id: "17f1a3c2b4d5e6f70000000000000001",
        user: "root",
        query: "SHOW session_id",
        state: "active",
        application: "tabler",
      },
      {
        id: "17f1a3c2b4d5e6f70000000000000002",
        user: "app",
        query: "SELECT * FROM rides WHERE city = $1",
        state: "active",
        application: "movr",
      },
    ],
  };
}

function greenplum(version6: boolean): EngineData {
  const customers: Table = {
    schema: "public",
    name: "customers",
    kind: "table",
    cols: [
      { name: "id", type: "integer", kind: "int", pk: true },
      { name: "name", type: "text", kind: "text", nullable: false },
      { name: "region", type: "character varying(16)", kind: "text" },
    ],
    rows: range(28).map((i) => [
      int(i),
      text(NAMES[i % NAMES.length]!),
      text(i % 2 ? "EMEA" : "AMER"),
    ]),
    options: {
      engine: "heap",
      charset: null,
      collation: null,
      autoIncrement: null,
      rowFormat: null,
      distribution: "DISTRIBUTED BY (id)",
    },
    ddl: "CREATE TABLE public.customers (\n    id integer NOT NULL,\n    name text NOT NULL,\n    region character varying(16),\n    CONSTRAINT customers_pkey PRIMARY KEY (id)\n)\nDISTRIBUTED BY (id);",
    sizeBytes: null,
    comment: null,
  };
  const storage = version6
    ? "ao_row (appendonly=true, compresstype=zstd)"
    : "ao_row (compresstype=zstd)";
  const orders: Table = {
    schema: "sales",
    name: "orders",
    kind: "partitioned",
    cols: [
      { name: "id", type: "bigint", kind: "bigInt", pk: true },
      { name: "order_date", type: "date", kind: "date", pk: true },
      { name: "customer_id", type: "integer", kind: "int", nullable: false },
      { name: "amount", type: "numeric(12,2)", kind: "decimal", nullable: false },
    ],
    rows: range(60).map((i) => [
      { t: "bigInt", v: String(i) },
      date(i),
      int((i % 28) + 1),
      dec(`${(i * 12.5).toFixed(2)}`),
    ]),
    options: {
      engine: storage,
      charset: null,
      collation: null,
      autoIncrement: null,
      rowFormat: null,
      distribution: "DISTRIBUTED BY (id)",
    },
    ddl: version6
      ? "CREATE TABLE sales.orders (\n    id bigint NOT NULL,\n    order_date date NOT NULL,\n    customer_id integer NOT NULL,\n    amount numeric(12,2) NOT NULL\n)\nWITH (appendonly=true, compresstype=zstd)\nDISTRIBUTED BY (id)\nPARTITION BY RANGE(order_date) (START ('2026-01-01'::date) END ('2027-01-01'::date) EVERY ('1 mon'::interval));"
      : "CREATE TABLE sales.orders (\n    id bigint NOT NULL,\n    order_date date NOT NULL,\n    customer_id integer NOT NULL,\n    amount numeric(12,2) NOT NULL\n) PARTITION BY RANGE (order_date)\nUSING ao_row\nWITH (compresstype=zstd)\nDISTRIBUTED BY (id);",
    sizeBytes: null,
    comment: "Orders by month",
  };
  return {
    databases: ["warehouse", "postgres"],
    schemas: ["public", "sales"],
    tables: [customers, orders],
    routines: [
      {
        schema: "public",
        name: "region_total",
        kind: "function",
        arguments: "r text",
        identityArguments: "r text",
        returnType: "numeric",
        language: "plpgsql",
        comment: null,
        definition:
          "CREATE OR REPLACE FUNCTION public.region_total(r text)\n RETURNS numeric\n LANGUAGE plpgsql\nAS $function$\nBEGIN\n  RETURN (SELECT sum(amount) FROM sales.orders);\nEND;\n$function$\n",
      },
    ],
    processes: [
      {
        id: "4021",
        user: "gpadmin",
        query: "SELECT pid FROM pg_stat_activity",
        state: "active",
        application: "tabler",
      },
      {
        id: "4188",
        user: "etl",
        query: "INSERT INTO sales.orders SELECT * FROM staging.orders",
        state: "active",
        application: "gpload",
      },
    ],
  };
}

function redshift(): EngineData {
  const sales: Table = {
    schema: "public",
    name: "sales",
    kind: "table",
    cols: [
      {
        name: "salesid",
        type: "integer",
        kind: "int",
        pk: true,
        nullable: false,
        extra: "DISTKEY, SORTKEY 1, ENCODE az64",
      },
      { name: "venue", type: "character varying(64)", kind: "text", extra: "ENCODE lzo" },
      { name: "qtysold", type: "smallint", kind: "int", nullable: false, extra: "ENCODE az64" },
      { name: "pricepaid", type: "numeric(8,2)", kind: "decimal", extra: "ENCODE az64" },
      { name: "details", type: "super", kind: "json", extra: "ENCODE zstd" },
    ],
    rows: range(40).map((i) => [
      int(i),
      text(`${CITIES[i % CITIES.length]!} arena`),
      int((i % 4) + 1),
      dec(`${(i * 21.5).toFixed(2)}`),
      { t: "json", v: `{"channel":"${i % 2 ? "web" : "box office"}"}` },
    ]),
    options: {
      engine: null,
      charset: null,
      collation: null,
      autoIncrement: null,
      rowFormat: null,
      distribution: "DISTSTYLE KEY(salesid)",
      sortKey: "SORTKEY (salesid)",
    },
    ddl: "CREATE TABLE public.sales (\n    salesid integer NOT NULL ENCODE az64 distkey,\n    venue character varying(64) ENCODE lzo,\n    qtysold smallint NOT NULL ENCODE az64,\n    pricepaid numeric(8,2) ENCODE az64,\n    details super ENCODE zstd,\n    PRIMARY KEY (salesid)\n)\nDISTSTYLE KEY\nSORTKEY ( salesid );",
    sizeBytes: 5_242_880,
    comment: null,
  };
  const clicks: Table = {
    schema: "spectrum",
    name: "clicks",
    kind: "foreignTable",
    cols: [
      { name: "ts", type: "timestamp without time zone", kind: "timestamp" },
      { name: "url", type: "character varying(512)", kind: "text" },
    ],
    rows: range(12).map((i) => [
      { t: "timestamp", v: `2026-10-${String(i).padStart(2, "0")} 08:00:00` },
      text(`/page/${i}`),
    ]),
    options: null,
    ddl: "CREATE EXTERNAL TABLE spectrum.clicks (\n    ts timestamp,\n    url character varying(512)\n)\nSTORED AS PARQUET\nLOCATION 's3://example-bucket/clicks/';",
    sizeBytes: null,
    comment: null,
  };
  return {
    databases: ["dev", "sample_data_dev"],
    schemas: ["public", "spectrum"],
    tables: [sales, clicks],
    // `redshift::tests::functions_and_procedures`' routines.
    routines: [
      {
        schema: "public",
        name: "load",
        kind: "procedure",
        arguments: "f1 integer, INOUT f2 character varying",
        identityArguments: "integer, character varying",
        returnType: null,
        language: "plpgsql",
        comment: "Loads a day",
        definition:
          "CREATE OR REPLACE PROCEDURE public.load(f1 integer, INOUT f2 character varying, OUT f3 character varying)\nLANGUAGE plpgsql\nAS $$\nBEGIN NULL; END;\n$$;\n",
      },
      {
        schema: "public",
        name: "f_add",
        kind: "function",
        arguments: "integer, integer",
        identityArguments: "integer, integer",
        returnType: "integer",
        language: "sql",
        comment: null,
        definition:
          'CREATE OR REPLACE FUNCTION "public"."f_add"(integer, integer)\n  RETURNS integer\n  IMMUTABLE\nAS $$ SELECT $1 + $2 $$ LANGUAGE sql;\n',
      },
      {
        schema: "public",
        name: "f_lambda",
        kind: "function",
        arguments: "bigint",
        identityArguments: "bigint",
        returnType: "bigint",
        language: "exfunc",
        comment: null,
        definition: "!the definition of a exfunc function is not kept by Redshift",
      },
    ],
    processes: [
      {
        id: "1073815778",
        user: "awsuser",
        query: "SELECT pg_backend_pid()",
        state: "Running",
        application: "tabler",
      },
      {
        id: "1073946813",
        user: "etl",
        query: "COPY sales FROM 's3://example-bucket/sales/'",
        state: "Running",
        application: "etl",
      },
    ],
  };
}

/** A fresh copy of `engine`'s data for a new session. */
export function engineData(engine: Engine): EngineData {
  switch (engine) {
    case "cockroachdb":
      return cockroach();
    case "greenplum6":
      return greenplum(true);
    case "greenplum7":
      return greenplum(false);
    case "redshift":
      return redshift();
  }
}
