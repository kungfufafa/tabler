import { useState } from "react";
import { useDirtyDialogGuard } from "@/hooks/useDirtyDialogGuard";
import { FormError } from "@/components/FormError";
import { KbdHint } from "@/components/KbdHint";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { Dialect } from "@/lib/ipc";
import { StructureSqlEditor } from "./StructureSqlEditor";

/** Which trigger the editor shows: a new one (template) or an existing / staged one. */
export type TriggerEditing = { key: string | null; sql: string; readOnly?: boolean } | null;

/**
 * The raw SQL editor of a trigger (TablePlus: "+ Trigger", or edit a trigger). Staging it replaces
 * the trigger with DROP + CREATE when the changes are applied; the statement is sent as written
 * (no DELIMITER).
 */
export function TriggerEditorDialog({
  editing,
  dialect,
  tableName,
  onClose,
  onStage,
}: {
  editing: TriggerEditing;
  dialect: Dialect;
  tableName: string;
  onClose: () => void;
  /** Returns an error message to show, or null when staged. */
  onStage: (key: string | null, sql: string) => string | null;
}) {
  // The form reports whether its SQL differs from what it opened with, so a stray Escape or a
  // click outside asks before throwing a trigger body away.
  const [dirty, setDirty] = useState(false);
  // Another trigger (or none) opened from outside starts clean.
  const [shownFor, setShownFor] = useState(editing);
  if (shownFor !== editing) {
    setShownFor(editing);
    setDirty(false);
  }
  const close = () => {
    setDirty(false);
    onClose();
  };
  const guard = useDirtyDialogGuard({
    dirty: editing !== null && dirty,
    onClose: close,
    what: "your trigger changes",
  });
  return (
    <>
      <Dialog open={editing !== null} onOpenChange={guard.onOpenChange}>
        <DialogContent
          className="flex h-[min(40rem,calc(100dvh-4rem))] flex-col sm:max-w-[min(56rem,calc(100%-2rem))]"
          {...guard.contentProps}
        >
          {editing && (
            <TriggerForm
              key={editing.key ?? "new"}
              editing={editing}
              dialect={dialect}
              tableName={tableName}
              onClose={close}
              onCancel={guard.requestClose}
              onDirtyChange={setDirty}
              onStage={onStage}
            />
          )}
        </DialogContent>
      </Dialog>
      {guard.confirm}
    </>
  );
}

function TriggerForm({
  editing,
  dialect,
  tableName,
  onClose,
  onCancel,
  onDirtyChange,
  onStage,
}: {
  editing: NonNullable<TriggerEditing>;
  dialect: Dialect;
  tableName: string;
  /** Closes at once: after staging, or for a read-only trigger. */
  onClose: () => void;
  /** Cancel: asks first while the SQL is changed. */
  onCancel: () => void;
  onDirtyChange: (dirty: boolean) => void;
  onStage: (key: string | null, sql: string) => string | null;
}) {
  const [sql, setSql] = useState(editing.sql);
  const [error, setError] = useState<string | null>(null);
  const isNew = editing.key === null;
  const stage = () => {
    if (editing.readOnly) return onClose();
    const err = onStage(editing.key, sql);
    if (err) setError(err);
    else onClose();
  };
  return (
    <>
      <DialogHeader>
        <DialogTitle className="text-sm">
          {editing.readOnly ? "Trigger" : isNew ? "New Trigger" : "Edit Trigger"}
        </DialogTitle>
        <DialogDescription className="text-xs">
          On <span className="font-mono">{tableName}</span>.{" "}
          {editing.readOnly
            ? null
            : isNew
              ? "Staged until you apply: the statement runs as written."
              : "Staged until you apply: the trigger is dropped and created again."}
        </DialogDescription>
      </DialogHeader>
      <StructureSqlEditor
        value={editing.sql}
        onChange={(v) => {
          setSql(v);
          setError(null);
          onDirtyChange(!editing.readOnly && v !== editing.sql);
        }}
        dialect={dialect}
        label="Trigger SQL"
        onSave={stage}
        autoFocus
        readOnly={editing.readOnly}
        className="min-h-40 flex-1"
      />
      {dialect === "postgres" && !editing.readOnly && (
        <p className="text-muted-foreground text-xs">
          PostgreSQL triggers call a function: create it first (it returns <code>trigger</code>).
        </p>
      )}
      {error && <FormError>{error}</FormError>}
      <DialogFooter className="items-center">
        <Button variant="outline" size="sm" onClick={onCancel}>
          {editing.readOnly ? "Close" : "Cancel"}
        </Button>
        {!editing.readOnly && (
          <Button size="sm" onClick={stage}>
            {isNew ? "Add Trigger" : "Stage Changes"}
            <KbdHint combo="Mod+S" />
          </Button>
        )}
      </DialogFooter>
    </>
  );
}
