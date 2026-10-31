import { JsonDocumentEditor } from "@/features/documents/JsonDocumentEditor";
import {
  DYNAMODB_JSON,
  DYNAMODB_PLAIN_JSON,
  type DocumentFields,
} from "@/features/documents/formats";
import type { InspectorSlotProps } from "../types";

const FORMATS = [DYNAMODB_JSON, DYNAMODB_PLAIN_JSON];

/**
 * The selected item as DynamoDB JSON, or as plain JSON with the toggle (`JsonDocumentEditor`):
 * its changed attributes are staged as one change of the row, a removed one as missing (the
 * backend `REMOVE`s it), and the key attributes stay as they are (a key changes in the grid,
 * which deletes and puts the item in one transaction). Without `onStage` it is read-only.
 */
export function DynamoInspector({ row, columns, onStage }: InspectorSlotProps) {
  if (!row)
    return (
      <p className="text-muted-foreground p-3 text-xs" data-testid="dynamo-no-item">
        Select an item to see it whole.
      </p>
    );
  const keys = columns.filter((c) => c.isPk).map((c) => c.name);
  return (
    <JsonDocumentEditor
      className="p-2"
      document={row as DocumentFields}
      formats={FORMATS}
      readOnlyFields={keys}
      canRemoveFields
      readOnly={!onStage}
      label="Item"
      saveLabel="Stage"
      onSave={onStage}
    />
  );
}
