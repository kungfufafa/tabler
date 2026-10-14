import type { ColumnMeta, Dialect, ForeignKeyMeta, IndexMeta, TableMeta } from "@/lib/ipc";

/**
 * Copy Script As ▸ Laravel Migration (TablePlus's Dump Table plugin, spec §2.4 / §11.4): a
 * migration class (`Schema::create`) rebuilt from a table's metadata. Types Laravel's Blueprint
 * has a method for map onto it; any other type becomes `$table->addColumn('type', 'name')`.
 */

/** A PHP single-quoted string literal. */
export function phpString(s: string): string {
  return `'${s.replace(/\\/g, "\\\\").replace(/'/g, "\\'")}'`;
}

function phpList(items: readonly string[]): string {
  return `[${items.map(phpString).join(", ")}]`;
}

/** `name` or `['a', 'b']`: how Blueprint takes one or several columns. */
function columnsArg(cols: readonly string[]): string {
  return cols.length === 1 ? phpString(cols[0]!) : phpList(cols);
}

/** The type without its parameters, `unsigned` or `zerofill`, lower-case, spaces collapsed. */
export function baseType(dataType: string): string {
  return dataType
    .toLowerCase()
    .replace(/\(.*?\)/g, "")
    .replace(/\b(unsigned|zerofill)\b/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

/** The numbers in a type's parentheses (`numeric(10,2)` → [10, 2]). */
function typeArgs(dataType: string): number[] {
  const m = /\(([^)]*)\)/.exec(dataType);
  if (!m) return [];
  return m[1]!
    .split(",")
    .map((x) => Number(x.trim()))
    .filter((x) => Number.isFinite(x));
}

const INT_METHODS: Record<string, [signed: string, unsigned: string, increments: string]> = {
  tinyint: ["tinyInteger", "unsignedTinyInteger", "tinyIncrements"],
  smallint: ["smallInteger", "unsignedSmallInteger", "smallIncrements"],
  int2: ["smallInteger", "unsignedSmallInteger", "smallIncrements"],
  mediumint: ["mediumInteger", "unsignedMediumInteger", "mediumIncrements"],
  int: ["integer", "unsignedInteger", "increments"],
  integer: ["integer", "unsignedInteger", "increments"],
  int4: ["integer", "unsignedInteger", "increments"],
  bigint: ["bigInteger", "unsignedBigInteger", "bigIncrements"],
  int8: ["bigInteger", "unsignedBigInteger", "bigIncrements"],
};

const SERIALS: Record<string, string> = {
  smallserial: "smallIncrements",
  serial2: "smallIncrements",
  serial: "increments",
  serial4: "increments",
  bigserial: "bigIncrements",
  serial8: "bigIncrements",
};

/** Types whose Blueprint method takes no arguments beyond the name. */
const SIMPLE: Record<string, string> = {
  text: "text",
  tinytext: "tinyText",
  mediumtext: "mediumText",
  longtext: "longText",
  boolean: "boolean",
  bool: "boolean",
  date: "date",
  datetime: "dateTime",
  timestamp: "timestamp",
  "timestamp without time zone": "timestamp",
  "timestamp with time zone": "timestampTz",
  timestamptz: "timestampTz",
  time: "time",
  "time without time zone": "time",
  "time with time zone": "timeTz",
  timetz: "timeTz",
  year: "year",
  json: "json",
  jsonb: "jsonb",
  uuid: "uuid",
  bytea: "binary",
  blob: "binary",
  tinyblob: "binary",
  mediumblob: "binary",
  longblob: "binary",
  binary: "binary",
  varbinary: "binary",
  inet: "ipAddress",
  macaddr: "macAddress",
  geometry: "geometry",
  point: "point",
  polygon: "polygon",
  linestring: "lineString",
  real: "float",
  float4: "float",
  float: "float",
  "double precision": "double",
  float8: "double",
  double: "double",
};

const TIMESTAMPISH = new Set(["timestamp", "timestampTz", "dateTime", "dateTimeTz"]);

function isBool(c: ColumnMeta, dialect: Dialect): boolean {
  const base = baseType(c.dataType);
  if (base === "boolean" || base === "bool") return true;
  return dialect === "mysql" && /^tinyint\(1\)/i.test(c.dataType.trim());
}

function isAutoIncrement(c: ColumnMeta): boolean {
  return (
    c.isAutoIncrement ||
    /^nextval\(/i.test(c.defaultExpr ?? "") ||
    /identity/i.test(c.extra ?? "") ||
    baseType(c.dataType) in SERIALS
  );
}

/** `$table->method(args)` for the column's type, without modifiers. */
function typeCall(
  c: ColumnMeta,
  dialect: Dialect,
  autoIncrement: boolean,
): { method: string; args: string[] } {
  const name = phpString(c.name);
  const base = baseType(c.dataType);
  const args = typeArgs(c.dataType);
  if (isBool(c, dialect)) return { method: "boolean", args: [name] };
  const serial = SERIALS[base];
  if (serial) return { method: serial, args: [name] };
  const ints = INT_METHODS[base];
  if (ints) {
    if (autoIncrement) return { method: ints[2], args: [name] };
    return { method: c.isUnsigned ? ints[1] : ints[0], args: [name] };
  }
  switch (base) {
    case "character varying":
    case "varchar":
    case "nvarchar": {
      const len = c.charMaxLen ?? args[0] ?? null;
      return { method: "string", args: len !== null && len !== 255 ? [name, String(len)] : [name] };
    }
    case "character":
    case "char":
    case "bpchar":
    case "nchar": {
      const len = c.charMaxLen ?? args[0] ?? null;
      return { method: "char", args: len !== null && len !== 255 ? [name, String(len)] : [name] };
    }
    case "numeric":
    case "decimal": {
      const p = c.numericPrecision ?? args[0] ?? null;
      const s = c.numericScale ?? args[1] ?? 0;
      return { method: "decimal", args: p !== null ? [name, String(p), String(s)] : [name] };
    }
    case "enum":
    case "set":
      if (c.enumValues) return { method: base, args: [name, phpList(c.enumValues)] };
      break;
  }
  const simple = SIMPLE[base];
  if (simple) return { method: simple, args: [name] };
  return { method: "addColumn", args: [phpString(c.dataType), name] };
}

/** How the column default reads in PHP, or `undefined` for none. */
type PhpDefault =
  { kind: "value"; php: string } | { kind: "current" } | { kind: "raw"; sql: string };

const NUMBER = /^-?\d+(\.\d+)?$/;

export function phpDefault(c: ColumnMeta, dialect: Dialect): PhpDefault | null {
  const d = c.defaultExpr?.trim();
  if (!d || /^null(::.*)?$/i.test(d) || isAutoIncrement(c)) return null;
  if (/^(current_timestamp(\(\d*\))?|now\(\)|localtimestamp)$/i.test(d)) return { kind: "current" };
  const bool = isBool(c, dialect);
  if (dialect === "mysql") {
    // information_schema.COLUMNS: literals come unquoted; expressions are DEFAULT_GENERATED.
    if (/default_generated/i.test(c.extra ?? "")) return { kind: "raw", sql: d };
    if (bool && (d === "0" || d === "1"))
      return { kind: "value", php: d === "1" ? "true" : "false" };
    const numeric = ["int", "bigInt", "decimal", "float"].includes(c.kind);
    if (numeric && NUMBER.test(d)) return { kind: "value", php: d };
    return { kind: "value", php: phpString(d) };
  }
  // PostgreSQL / SQLite: SQL literal syntax, maybe with a PG cast (`'x'::character varying`).
  const str = /^'((?:[^']|'')*)'(?:::[\w\s."[\]()]+)?$/.exec(d);
  if (str) return { kind: "value", php: phpString(str[1]!.replace(/''/g, "'")) };
  const num = /^\(?(-?\d+(?:\.\d+)?)\)?(?:::[\w\s]+)?$/.exec(d);
  if (num) {
    if (bool && (num[1] === "0" || num[1] === "1"))
      return { kind: "value", php: num[1] === "1" ? "true" : "false" };
    return { kind: "value", php: num[1]! };
  }
  if (/^(true|false)$/i.test(d)) return { kind: "value", php: d.toLowerCase() };
  return { kind: "raw", sql: d };
}

const FK_ACTION: Record<string, [onDelete: string, onUpdate: string]> = {
  CASCADE: ["cascadeOnDelete()", "cascadeOnUpdate()"],
  "SET NULL": ["nullOnDelete()", "nullOnUpdate()"],
  RESTRICT: ["restrictOnDelete()", "restrictOnUpdate()"],
  "NO ACTION": ["noActionOnDelete()", "noActionOnUpdate()"],
};

/** `->cascadeOnDelete()`… for a foreign key's actions; the server defaults stay implicit. */
function fkActions(fk: ForeignKeyMeta): string {
  let out = "";
  const del = FK_ACTION[fk.onDelete.toUpperCase()];
  const upd = FK_ACTION[fk.onUpdate.toUpperCase()];
  if (del && fk.onDelete.toUpperCase() !== "NO ACTION") out += `->${del[0]}`;
  if (upd && fk.onUpdate.toUpperCase() !== "NO ACTION") out += `->${upd[1]}`;
  return out;
}

const qualifiedName = (meta: TableMeta, dialect: Dialect): string =>
  dialect === "postgres" && meta.table.schema && meta.table.schema !== "public"
    ? `${meta.table.schema}.${meta.table.name}`
    : meta.table.name;

/** Laravel's default index name (`users_email_unique`), left out of the generated call. */
function defaultIndexName(table: string, cols: readonly string[], type: string): string {
  return `${table}_${cols.join("_")}_${type}`.replace(/[-.]/g, "_").toLowerCase();
}

/** One migration file (an anonymous `Migration` class) creating `meta`'s table. */
export function laravelMigration(meta: TableMeta, dialect: Dialect): string {
  const table = qualifiedName(meta, dialect);
  const cols = [...meta.columns].sort((a, b) => a.ordinal - b.ordinal);
  const pk = meta.primaryKey;
  const byName = new Map(cols.map((c) => [c.name, c]));
  let usesDb = false;
  const lines: string[] = [];
  const done = new Set<string>();

  // created_at / updated_at → timestamps(), deleted_at → softDeletes(): Laravel's own columns.
  const stamp = (name: string) => {
    const c = byName.get(name);
    if (!c || !c.nullable || phpDefault(c, dialect) !== null) return null;
    const t = typeCall(c, dialect, false).method;
    return t === "timestamp" || t === "dateTime" ? "" : t === "timestampTz" ? "Tz" : null;
  };
  const created = stamp("created_at");
  const timestamps = created !== null && stamp("updated_at") === created ? created : null;
  const deletedAt = stamp("deleted_at");

  // Single-column foreign keys on an id-like column become foreignId()->constrained().
  const constrained = new Map<string, ForeignKeyMeta>();
  const explicitFks: ForeignKeyMeta[] = [];
  for (const fk of meta.foreignKeys) {
    const col = fk.columns.length === 1 ? byName.get(fk.columns[0]!) : undefined;
    const base = col ? baseType(col.dataType) : "";
    const idLike = col && (base === "bigint" || base === "int8" || base === "uuid");
    if (col && idLike && !constrained.has(col.name) && !pk.includes(col.name))
      constrained.set(col.name, fk);
    else explicitFks.push(fk);
  }

  // Single-column unique indexes with Laravel's default name read as ->unique().
  const uniqueCols = new Set<string>();
  const indexes: IndexMeta[] = [];
  for (const ix of meta.indexes) {
    if (ix.primary) continue;
    const fkIndex =
      !ix.unique &&
      dialect === "mysql" &&
      meta.foreignKeys.some(
        (fk) => fk.name === ix.name || fk.columns.join(",") === ix.columns.join(","),
      );
    if (fkIndex) continue;
    if (
      ix.unique &&
      ix.columns.length === 1 &&
      ix.name === defaultIndexName(meta.table.name, ix.columns, "unique")
    )
      uniqueCols.add(ix.columns[0]!);
    else indexes.push(ix);
  }

  const singlePk = pk.length === 1 ? pk[0]! : null;
  for (const c of cols) {
    if (done.has(c.name)) continue;
    if (timestamps !== null && (c.name === "created_at" || c.name === "updated_at")) {
      done.add("created_at").add("updated_at");
      lines.push(`$table->timestamps${timestamps}();`);
      continue;
    }
    if (deletedAt !== null && c.name === "deleted_at") {
      done.add(c.name);
      lines.push(`$table->softDeletes${deletedAt}();`);
      continue;
    }
    done.add(c.name);
    const auto = c.name === singlePk && isAutoIncrement(c);
    const type = typeCall(c, dialect, auto);
    if (auto && c.name === "id" && type.method === "bigIncrements") {
      lines.push(c.comment ? `$table->id()->comment(${phpString(c.comment)});` : "$table->id();");
      continue;
    }
    const fk = constrained.get(c.name);
    let call: string;
    if (fk) {
      const method = baseType(c.dataType) === "uuid" ? "foreignUuid" : "foreignId";
      call = `$table->${method}(${phpString(c.name)})`;
    } else call = `$table->${type.method}(${type.args.join(", ")})`;
    if (c.nullable && !auto) call += "->nullable()";
    const def = phpDefault(c, dialect);
    if (def?.kind === "value") call += `->default(${def.php})`;
    else if (def?.kind === "current")
      call += TIMESTAMPISH.has(type.method)
        ? "->useCurrent()"
        : "->default(DB::raw('CURRENT_TIMESTAMP'))";
    else if (def?.kind === "raw") call += `->default(DB::raw(${phpString(def.sql)}))`;
    if (def?.kind === "raw" || (def?.kind === "current" && !TIMESTAMPISH.has(type.method)))
      usesDb = true;
    if (/on update current_timestamp/i.test(c.extra ?? "")) call += "->useCurrentOnUpdate()";
    if (c.comment) call += `->comment(${phpString(c.comment)})`;
    if (c.name === singlePk && !auto) call += "->primary()";
    if (uniqueCols.has(c.name)) call += "->unique()";
    if (fk) {
      const refCol = fk.refColumns[0] ?? "id";
      const refTable = fk.refTable.name;
      call +=
        refCol === "id"
          ? `->constrained(${phpString(refTable)})`
          : `->constrained(${phpString(refTable)}, ${phpString(refCol)})`;
      call += fkActions(fk);
    }
    lines.push(`${call};`);
  }

  if (pk.length > 1) lines.push(`$table->primary(${phpList(pk)});`);
  for (const ix of indexes) {
    const kind = ix.unique ? "unique" : "index";
    const name =
      ix.name === defaultIndexName(meta.table.name, ix.columns, kind)
        ? ""
        : `, ${phpString(ix.name)}`;
    lines.push(`$table->${kind}(${columnsArg(ix.columns)}${name});`);
  }
  for (const fk of explicitFks)
    lines.push(
      `$table->foreign(${columnsArg(fk.columns)})->references(${columnsArg(fk.refColumns)})->on(${phpString(fk.refTable.name)})${fkActions(fk)};`,
    );
  if (meta.comment) lines.push(`$table->comment(${phpString(meta.comment)});`);

  const uses = [
    "use Illuminate\\Database\\Migrations\\Migration;",
    "use Illuminate\\Database\\Schema\\Blueprint;",
    ...(usesDb ? ["use Illuminate\\Support\\Facades\\DB;"] : []),
    "use Illuminate\\Support\\Facades\\Schema;",
  ];
  const body = lines.map((l) => `            ${l}`).join("\n");
  return `<?php

${uses.join("\n")}

return new class extends Migration
{
    /**
     * Run the migrations.
     */
    public function up(): void
    {
        Schema::create(${phpString(table)}, function (Blueprint $table) {
${body}
        });
    }

    /**
     * Reverse the migrations.
     */
    public function down(): void
    {
        Schema::dropIfExists(${phpString(table)});
    }
};
`;
}

/** One migration per table, each after a `// ----- <table> -----` separator. */
export function laravelMigrations(metas: readonly TableMeta[], dialect: Dialect): string {
  return metas
    .map((m) => `// ----- ${m.table.name} -----\n${laravelMigration(m, dialect)}`)
    .join("\n");
}
