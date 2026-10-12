import type { ResultColumn } from "@/lib/ipc";

/**
 * `sql` with comments and quoted strings blanked out (same length), so keywords and names found
 * in it are code. Backtick- and double-quoted identifiers stay.
 */
function codeOnly(sql: string): string {
  let out = "";
  let i = 0;
  const n = sql.length;
  while (i < n) {
    const c = sql[i]!;
    let end = -1;
    if ((c === "-" && sql[i + 1] === "-") || c === "#") {
      end = sql.indexOf("\n", i);
      end = end < 0 ? n : end;
    } else if (c === "/" && sql[i + 1] === "*") {
      end = sql.indexOf("*/", i + 2);
      end = end < 0 ? n : end + 2;
    } else if (c === "'") {
      end = i + 1;
      while (end < n) {
        if (sql[end] === "\\") end += 2;
        else if (sql[end] === "'" && sql[end + 1] === "'") end += 2;
        else if (sql[end] === "'") break;
        else end++;
      }
      end = Math.min(n, end + 1);
    }
    if (end < 0) {
      out += c;
      i++;
    } else {
      out += " ".repeat(end - i);
      i = end;
    }
  }
  return out;
}

/** A name: `quoted`, "quoted" or bare. */
const NAME = '(?:`[^`]*`|"[^"]*"|[\\p{L}\\p{N}_$]+)';
/** `FROM` / `JOIN` followed by a table name, optionally qualified (`db.table`). */
const TABLE_REF = new RegExp(`\\b(?:FROM|JOIN)\\s+(?:${NAME}\\s*\\.\\s*)?(${NAME})`, "giu");

const unquote = (name: string) =>
  (name.startsWith("`") && name.endsWith("`")) || (name.startsWith('"') && name.endsWith('"'))
    ? name.slice(1, -1)
    : name;

/**
 * The table a SQL editor result can be written back to with INSERTs (the export's default
 * target), or null when that is not known for sure:
 * - every column must come from the same table (a join, or a computed column, has no single
 *   target);
 * - that name must appear in `sql` as a table after FROM / JOIN. MySQL reports a column's
 *   table as written in the query, so `FROM users u` reports `u` and a derived table
 *   `(…) AS t` reports `t`: aliases are not tables to insert into.
 */
export function resultTable(columns: readonly ResultColumn[], sql: string | null): string | null {
  const table = columns[0]?.table ?? null;
  if (!table || !sql || columns.some((c) => c.table !== table)) return null;
  const wanted = table.toLowerCase();
  for (const m of codeOnly(sql).matchAll(TABLE_REF)) {
    if (unquote(m[1]!).toLowerCase() === wanted) return table;
  }
  return null;
}
