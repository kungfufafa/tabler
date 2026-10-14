import { useQuery } from "@tanstack/react-query";
import { ipc } from "@/lib/ipc";

/** Under the schema's `["tables", c, schema]` key, so a schema refresh reloads the diagram too. */
export const diagramKey = (connectionId: string, schema: string) =>
  ["tables", connectionId, schema, "diagram"] as const;

export function useDiagramSchema(connectionId: string, schema: string | null) {
  return useQuery({
    queryKey: diagramKey(connectionId, schema ?? ""),
    queryFn: () => ipc.diagram.schemaRelations(connectionId, schema!),
    enabled: schema !== null,
    staleTime: 60_000,
  });
}
