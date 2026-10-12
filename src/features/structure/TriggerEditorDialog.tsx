import { useState } from "react";
import { Kbd } from "@/components/Kbd";
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
  return (
    <Dialog open={editing !== null} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="flex max-h-[85vh] flex-col gap-3 sm:max-w-2xl">
        {editing && (
          <TriggerForm
            key={editing.key ?? "new"}
            editing={editing}
            dialect={dialect}
            tableName={tableName}
            onClose={onClose}
            onStage={onStage}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

function TriggerForm({
  editing,
  dialect,
  tableName,
  onClose,
  onStage,
}: {
  editing: NonNullable<TriggerEditing>;
  dialect: Dialect;
  tableName: string;
  onClose: () => void;
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
          {editing.readOnly ? "Trigger" : isNew ? "New trigger" : "Edit trigger"}
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
        }}
        dialect={dialect}
        label="Trigger SQL"
        onSave={stage}
        autoFocus
        readOnly={editing.readOnly}
        className="h-72"
      />
      {dialect === "postgres" && !editing.readOnly && (
        <p className="text-muted-foreground text-[11px]">
          PostgreSQL triggers call a function: create it first (it returns <code>trigger</code>).
        </p>
      )}
      {error && (
        <p role="alert" className="text-error text-xs">
          {error}
        </p>
      )}
      <DialogFooter className="items-center">
        <Button variant="ghost" size="sm" onClick={onClose}>
          {editing.readOnly ? "Close" : "Cancel"}
        </Button>
        {!editing.readOnly && (
          <Button size="sm" onClick={stage}>
            {isNew ? "Add trigger" : "Stage changes"}
            <Kbd
              combo="Mod+S"
              className="border-primary-foreground/30 ml-1 bg-transparent text-inherit"
            />
          </Button>
        )}
      </DialogFooter>
    </>
  );
}
