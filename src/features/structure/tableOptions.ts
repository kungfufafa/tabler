import type { TableOptions } from "@/lib/ipc";

/**
 * The words for the options a table reports: MySQL's (its engine and row format), or those of an
 * engine that distributes or orders its rows (Greenplum, Redshift: their distribution and sort
 * key, their storage in the engine; ClickHouse: its sorting key and engine).
 */
export function tableOptionRows(
  options: TableOptions | null,
  mysql: boolean,
): Array<[label: string, value: string]> {
  if (!options) return [];
  const rows: Array<[string, string | number | null | undefined]> = mysql
    ? [
        ["Engine", options.engine],
        ["Charset", options.charset],
        ["Collation", options.collation],
        ["Auto increment", options.autoIncrement],
        ["Row format", options.rowFormat],
      ]
    : [
        ["Distribution", options.distribution],
        ["Sort key", options.sortKey],
        ["Storage", options.engine],
        ["Collation", options.collation],
      ];
  return rows.flatMap(([label, value]) =>
    value === null || value === undefined || value === ""
      ? []
      : [[label, String(value)] as [string, string]],
  );
}
