import type { TableOptions } from "@/lib/ipc";

/**
 * The words for the options a table reports: MySQL's (its engine and row format), or those of a
 * PostgreSQL flavor that distributes its tables (Greenplum, Redshift: their distribution in the
 * row format, their storage in the engine).
 */
export function tableOptionRows(
  options: TableOptions | null,
  mysql: boolean,
): Array<[label: string, value: string]> {
  if (!options) return [];
  const rows: Array<[string, string | number | null]> = mysql
    ? [
        ["Engine", options.engine],
        ["Charset", options.charset],
        ["Collation", options.collation],
        ["Auto increment", options.autoIncrement],
        ["Row format", options.rowFormat],
      ]
    : [
        ["Distribution", options.rowFormat],
        ["Storage", options.engine],
        ["Collation", options.collation],
      ];
  return rows.flatMap(([label, value]) =>
    value === null || value === "" ? [] : [[label, String(value)] as [string, string]],
  );
}
