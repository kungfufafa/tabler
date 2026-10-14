import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { CommitHost } from "@/features/commit/CommitHost";
import { TableDialogs, type TableDialogState } from "@/features/structure/TableDialogs";
import { ItemOverviewDialog } from "./ItemOverview";
import { ObjectDialogView } from "./ObjectDialogs";
import { installPendingOps, usePendingOpActions } from "./pendingActions";
import { closeItemOverview, closeObjectDialog, useObjectsStore } from "./store";

/**
 * Mounts the sidebar's object dialogs (drop / truncate / clone, routines, sequences, and the
 * structure package's create / rename table dialogs) and the Item Overview, whichever the sidebar
 * menus and keys opened through `openObjectDialog` / `openItemOverview`; and the workspace commit
 * (every pending change of the active connection, including the staged Truncate… / Delete…): its
 * ⌘S / ⇧⌘P / ⇧⌘⌫ and Preview SQL sheet.
 */
export function ObjectsHost() {
  useEffect(installPendingOps, []);
  const qc = useQueryClient();
  usePendingOpActions(qc);
  const request = useObjectsStore((s) => s.dialog);
  const overview = useObjectsStore((s) => s.overview);
  const tableState: TableDialogState | null =
    request?.dialog.kind === "createTable"
      ? { kind: "create", schema: request.dialog.schema }
      : request?.dialog.kind === "renameTable"
        ? { kind: "rename", table: request.dialog.table }
        : null;
  return (
    <>
      {request && (
        <TableDialogs
          connectionId={request.connectionId}
          dialect={request.dialect}
          state={tableState}
          onClose={closeObjectDialog}
        />
      )}
      {request && tableState === null && (
        <ObjectDialogView
          // A new request (other items) starts the dialog afresh.
          key={JSON.stringify(request.dialog)}
          connectionId={request.connectionId}
          dialect={request.dialect}
          dialog={request.dialog}
          onClose={closeObjectDialog}
        />
      )}
      {overview && (
        <ItemOverviewDialog
          key={`${overview.connectionId}:${overview.schema}`}
          request={overview}
          onClose={closeItemOverview}
        />
      )}
      <CommitHost />
    </>
  );
}
