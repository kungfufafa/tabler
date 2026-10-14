import type { Dialect, TableRef } from "@/lib/ipc";
import { qualify, quoteIdent } from "@/lib/sql/quote";

/** The foreign key a JOIN follows: `table.column` references `references.column`. */
export interface JoinFk {
  table: TableRef;
  column: string;
  references: TableRef & { column: string };
  /**
   * A composite key: every column pair, joined with AND (`column` and `references.column` stand
   * for the first one and are ignored then).
   */
  pairs?: ReadonlyArray<{ column: string; reference: string }>;
}

/**
 * `SELECT * FROM a JOIN b ON a.fk = b.pk` for a foreign key (TablePlus: "Generate JOIN" in an FK
 * cell's menu): the table that holds the key, joined to the one it references. Tables are
 * qualified by their schema where they have one; a table joined to itself gets an alias for the
 * referenced side.
 */
export function joinSql(dialect: Dialect, fk: JoinFk): string {
  const from = qualify(dialect, fk.table.schema, fk.table.name);
  const target = qualify(dialect, fk.references.schema, fk.references.name);
  const sameTable =
    fk.table.name === fk.references.name &&
    (fk.table.schema ?? "") === (fk.references.schema ?? "");
  const left = quoteIdent(dialect, fk.table.name);
  const alias = sameTable ? quoteIdent(dialect, `${fk.references.name}_ref`) : null;
  const right = alias ?? quoteIdent(dialect, fk.references.name);
  const pairs =
    fk.pairs && fk.pairs.length > 0
      ? fk.pairs
      : [{ column: fk.column, reference: fk.references.column }];
  const on = pairs
    .map(
      (p) =>
        `${left}.${quoteIdent(dialect, p.column)} = ${right}.${quoteIdent(dialect, p.reference)}`,
    )
    .join(" AND ");
  return [
    "SELECT *",
    `FROM ${from}`,
    `JOIN ${target}${alias ? ` AS ${alias}` : ""} ON ${on}`,
    "",
  ].join("\n");
}
