import { findParams } from "@/features/editor/params";
import type { Dialect, MetricsQuery, Value } from "@/lib/ipc";

/** Board variables: `{{name}}` (TablePlus's `\{\{[\w.]+\}\}` query-params syntax). */
export const VARIABLE_SYNTAX = "\\{\\{[\\w.]+\\}\\}";

/** An object's SQL cut at its variables: `parts[0] {{names[0]}} parts[1] … parts[n]`. */
export interface Template {
  parts: string[];
  names: string[];
}

/**
 * Finds the `{{variables}}` of `sql` outside comments, strings and quoted identifiers (inside
 * quotes a placeholder cannot go: write `'%' || {{q}} || '%'`, not `'%{{q}}%'`).
 */
export function splitTemplate(sql: string, dialect: Dialect): Template {
  const found = findParams(sql, dialect, { syntax: VARIABLE_SYNTAX, inQuotes: false });
  const parts: string[] = [];
  const names: string[] = [];
  let pos = 0;
  for (const o of found) {
    parts.push(sql.slice(pos, o.start));
    names.push(o.key.slice(2, -2));
    pos = o.end;
  }
  parts.push(sql.slice(pos));
  return { parts, names };
}

/** The distinct variables `sql` uses, in order of first use. */
export function variablesOf(sql: string, dialect: Dialect): string[] {
  return [...new Set(splitTemplate(sql, dialect).names)];
}

/** The template with every variable as NULL: what the safety checks classify. */
export function neutralSql(template: Template): string {
  return template.parts.join(" NULL ");
}

export interface BoundQuery {
  query: Omit<MetricsQuery, "allowWrites" | "maxRows">;
  /** Variables no input or dropdown sets. */
  missing: string[];
}

/**
 * The query to send: the template plus the value of each occurrence. The backend puts the
 * dialect's bind parameters in the gaps (`?` MySQL, `?n` SQLite; quoted literals PostgreSQL).
 */
export function bindVariables(
  template: Template,
  values: Readonly<Record<string, Value>>,
): BoundQuery {
  const missing = new Set<string>();
  const bound = template.names.map((name) => {
    const v = values[name];
    if (v === undefined) missing.add(name);
    return v ?? ({ t: "null" } as const);
  });
  return { query: { parts: template.parts, values: bound }, missing: [...missing] };
}
