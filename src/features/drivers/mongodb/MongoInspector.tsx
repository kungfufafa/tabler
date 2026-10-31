import { JsonDocumentEditor } from "@/features/documents/JsonDocumentEditor";
import { EXTENDED_JSON, type DocumentFields } from "@/features/documents/formats";
import type { InspectorSlotProps } from "../types";

/** `_id` addresses the document: the backend edits by it and never changes it. */
const READ_ONLY_FIELDS = ["_id"];

/**
 * The selected document as relaxed Extended JSON (`JsonDocumentEditor`): its changed fields are
 * staged as one change of the row, a removed field as missing (the backend `$unset`s it), and
 * `_id` stays as it is. Without `onStage` (a view, a system collection) it is read-only.
 */
export function MongoInspector({ row, onStage }: InspectorSlotProps) {
  if (!row)
    return (
      <p className="text-muted-foreground p-3 text-xs" data-testid="mongo-no-document">
        Select a document to see it whole.
      </p>
    );
  return (
    <JsonDocumentEditor
      className="p-2"
      document={row as DocumentFields}
      formats={[EXTENDED_JSON]}
      readOnlyFields={READ_ONLY_FIELDS}
      canRemoveFields
      readOnly={!onStage}
      label="Document"
      saveLabel="Stage"
      onSave={onStage}
    />
  );
}
