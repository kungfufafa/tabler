import { EmptyState } from "@/components/EmptyState";
import { Kbd } from "@/components/Kbd";
import { QuietLoading } from "@/components/QuietLoading";
import { useQuery } from "@tanstack/react-query";
import { useCallback, useMemo } from "react";
import { ipc, type DdlOp, type Dialect, type TableKind, type TableRef } from "@/lib/ipc";
import { ddlKey } from "./invalidate";
import { StructureSqlEditor } from "./StructureSqlEditor";
import { editableViewSql, viewOps } from "./viewEdit";

/**
 * The Structure view of a view / materialized view (TablePlus): its SQL definition, editable.
 * Edits are staged as they are typed; ⌘S reviews and applies them.
 */
export function ViewDefinitionPane({
  connectionId,
  table,
  kind,
  dialect,
  editable,
  resetKey,
  onStage,
  onReview,
}: {
  connectionId: string;
  table: TableRef;
  kind: TableKind;
  dialect: Dialect;
  editable: boolean;
  /** Changes when the staged edit was discarded or applied: the editor shows `original` again. */
  resetKey: number;
  onStage: (ops: DdlOp[]) => void;
  onReview: () => void;
}) {
  const ddl = useQuery({
    queryKey: ddlKey(connectionId, table),
    queryFn: () => ipc.ddl.tableDdl(connectionId, table),
    staleTime: 60_000,
  });
  const original = useMemo(
    () => (ddl.data !== undefined ? editableViewSql(dialect, kind, ddl.data) : null),
    [ddl.data, dialect, kind],
  );
  const onChange = useCallback(
    (sql: string) => {
      if (original !== null) onStage(viewOps(dialect, kind, table, original, sql));
    },
    [original, onStage, dialect, kind, table],
  );
  if (ddl.isPending) return <QuietLoading />;
  if (ddl.isError)
    return (
      <div role="alert" className="h-full">
        <EmptyState title="Could not load the definition" description={ddl.error.message} />
      </div>
    );
  return (
    <section aria-label="Definition" className="flex h-full min-h-0 flex-col gap-1.5 p-3">
      <p className="text-muted-foreground text-xs">
        {editable ? (
          <>
            Edit the definition and press <Kbd combo="Mod+S" className="mx-0.5 align-middle" />:{" "}
            {kind === "materializedView" || dialect === "sqlite"
              ? "the view is dropped and created again."
              : "the view is replaced (CREATE OR REPLACE VIEW)."}
          </>
        ) : (
          "The view's definition"
        )}
      </p>
      <StructureSqlEditor
        key={`${table.schema ?? ""}.${table.name}:${original ?? ""}`}
        value={original ?? ""}
        resetKey={resetKey}
        onChange={onChange}
        dialect={dialect}
        label="View definition"
        readOnly={!editable}
        onSave={editable ? onReview : undefined}
        onPreview={editable ? onReview : undefined}
        className="min-h-48 flex-1"
      />
    </section>
  );
}
