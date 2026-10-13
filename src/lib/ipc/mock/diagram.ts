// Mock handlers for Generate Diagram…: schema_relations (built from the same table list and
// metadata as the sidebar and the structure tab, so staged DDL shows up in the diagram too) and
// write_binary_file (the PNG export).
import type { DiagramRelation } from "../bindings/DiagramRelation";
import type { DiagramSchema } from "../bindings/DiagramSchema";
import type { DiagramTable } from "../bindings/DiagramTable";
import type { TableMeta } from "../bindings/TableMeta";
import type { TableSummary } from "../bindings/TableSummary";
// The DDL mock registers list_tables / get_table, which this file calls.
import "./ddl";
import { mockError, mockInvoke, registerMock } from "./transport";

/** What the mock "wrote" with write_binary_file, by path (bytes as base64). */
export const mockBinaryFiles = new Map<string, string>();

export async function mockSchemaRelations(
  connectionId: string,
  schema: string,
): Promise<DiagramSchema> {
  const list = await mockInvoke<TableSummary[]>("list_tables", { connectionId, schema });
  const tables: DiagramTable[] = [];
  const relations: DiagramRelation[] = [];
  for (const summary of [...list].sort((a, b) => (a.name < b.name ? -1 : 1))) {
    const ref = { schema, name: summary.name };
    const meta = await mockInvoke<TableMeta>("get_table", { connectionId, table: ref }).catch(
      () => null,
    );
    tables.push({
      ref,
      kind: summary.kind,
      columns: (meta?.columns ?? []).map((c) => ({
        name: c.name,
        dataType: c.dataType,
        isPk: c.isPk,
        nullable: c.nullable,
      })),
      rowEstimate: summary.rowEstimate,
    });
    for (const fk of meta?.foreignKeys ?? [])
      relations.push({
        name: fk.name,
        from: ref,
        fromColumns: fk.columns,
        to: { schema: fk.refTable.schema ?? schema, name: fk.refTable.name },
        toColumns: fk.refColumns,
        onDelete: fk.onDelete,
      });
  }
  return { schema, tables, relations };
}

registerMock("schema_relations", ({ connectionId, schema }) =>
  mockSchemaRelations(connectionId as string, schema as string),
);

registerMock("write_binary_file", ({ path, base64 }) => {
  const p = String(path ?? "");
  if (p.includes("missing")) mockError("IO", `${p}: No such file or directory (os error 2)`);
  try {
    atob(String(base64 ?? ""));
  } catch {
    mockError("INVALID_INPUT", "not valid base64");
  }
  mockBinaryFiles.set(p, String(base64));
  return undefined;
});
