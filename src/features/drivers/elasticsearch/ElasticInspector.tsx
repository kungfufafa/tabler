import { JsonDocumentEditor } from "@/features/documents/JsonDocumentEditor";
import { PLAIN_JSON, type DocumentFields } from "@/features/documents/formats";
import type { InspectorSlotProps } from "../types";

/**
 * The document's metadata (`catalog::META_COLUMNS`): the backend edits by them and refuses a
 * change to any of them.
 */
const META_FIELDS = ["_id", "_index", "_routing", "_seq_no", "_primary_term", "_score"];

/**
 * The selected hit as JSON (`JsonDocumentEditor`): its `_source` fields by their flattened
 * paths, with the metadata read-only. Changed fields are staged as one change of the row; a
 * removed field is refused, as the backend refuses unsetting one (a script in the console does
 * it). Without `onStage` (an alias over several indices may still be edited; a system index or
 * a read-only connection may not) it is read-only.
 */
export function ElasticInspector({ row, onStage }: InspectorSlotProps) {
  if (!row)
    return (
      <p className="text-muted-foreground p-3 text-xs" data-testid="es-no-document">
        Select a document to see it whole.
      </p>
    );
  return (
    <JsonDocumentEditor
      className="p-2"
      document={row as DocumentFields}
      formats={[PLAIN_JSON]}
      readOnlyFields={META_FIELDS}
      readOnly={!onStage}
      label="Document"
      saveLabel="Stage"
      onSave={onStage}
    />
  );
}
