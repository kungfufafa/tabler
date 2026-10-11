import type { SQLNamespace } from "@codemirror/lang-sql";
import type { CompletionMetadata } from "@/lib/ipc";
import type { SqlDialect } from "./scanner";

/** schema → table → columns, in the shape `@codemirror/lang-sql` completes from. */
export function completionNamespace(
  meta: CompletionMetadata | undefined,
): SQLNamespace | undefined {
  if (!meta) return undefined;
  const out: Record<string, Record<string, string[]>> = {};
  for (const [schema, tables] of Object.entries(meta.schemas)) {
    const t: Record<string, string[]> = {};
    for (const [table, columns] of Object.entries(tables ?? {})) t[table] = [...(columns ?? [])];
    out[schema] = t;
  }
  return out;
}

/** Schema whose tables complete without a prefix: the MySQL database, SQLite `main`, PG default. */
export function completionDefaultSchema(
  dialect: SqlDialect,
  meta: CompletionMetadata | undefined,
  currentDatabase: string | null | undefined,
): string | undefined {
  if (dialect === "sqlite") return "main";
  if (dialect === "mysql") return currentDatabase ?? meta?.defaultSchema ?? undefined;
  return meta?.defaultSchema ?? "public";
}
