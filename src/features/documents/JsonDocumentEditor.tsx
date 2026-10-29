import { useMemo, useRef, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { reformatJson } from "@/features/datagrid/editors/json";
import { insertAtCaret } from "@/features/datagrid/editors/useGrabFocus";
import { formatCombo } from "@/lib/keys/combo";
import { cn } from "@/lib/utils";
import { documentChanges, PLAIN_JSON, type DocumentFields, type DocumentFormat } from "./formats";

/**
 * A document of a document store (a MongoDB document, an Elasticsearch `_source`, a DynamoDB
 * item) as editable JSON, shared by the lanes that have one (critique B4): each configures it
 * through its props, the formats it offers (`formats.ts`: relaxed Extended JSON, plain JSON,
 * DynamoDB JSON with its plain toggle) and the `toolbar` and `footer` slots; the mongodb lane
 * asks for changes to the editor itself.
 *
 * A field the document lacks is not shown, and stays missing unless the user writes it; `null`
 * is NULL. Save hands over only the fields that changed (a removed one as `Other{missing}`, when
 * the lane can unset fields), so the staged change touches nothing else.
 */
export function JsonDocumentEditor({
  document,
  formats = [PLAIN_JSON],
  readOnlyFields,
  canRemoveFields = false,
  readOnly = false,
  label = "Document",
  saveLabel = "Save",
  onSave,
  onCancel,
  toolbar,
  footer,
  className,
}: {
  /** The document as loaded, its missing fields as `Other{missing}`. */
  document: DocumentFields;
  /** The formats offered, the first shown first; more than one shows a toggle. */
  formats?: readonly DocumentFormat[];
  /** Fields that cannot change (`_id`, a key attribute). */
  readOnlyFields?: readonly string[];
  /** Whether removing a field from the text unsets it (else it is refused). */
  canRemoveFields?: boolean;
  readOnly?: boolean;
  /** The text area's accessible name. */
  label?: string;
  saveLabel?: string;
  /** The changed fields; absent when the document cannot be edited. */
  onSave?: (changes: DocumentFields) => void;
  onCancel?: () => void;
  /** The lane's own controls, after the format toggle. */
  toolbar?: ReactNode;
  /** The lane's own lines, above the buttons. */
  footer?: ReactNode;
  className?: string;
}) {
  const first = formats[0] ?? PLAIN_JSON;
  const [format, setFormat] = useState<DocumentFormat>(first);
  const [text, setText] = useState(() => first.write(document));
  // Another document (the selection moved): start over from it.
  const [shown, setShown] = useState(document);
  if (shown !== document) {
    setShown(document);
    setText(format.write(document));
  }
  const ref = useRef<HTMLTextAreaElement>(null);
  const editable = !readOnly && onSave !== undefined;

  const read = useMemo(() => format.read(text, document), [format, text, document]);
  const result = useMemo(
    () =>
      "error" in read
        ? read
        : documentChanges(document, read.fields, { readOnlyFields, canRemoveFields }),
    [read, document, readOnlyFields, canRemoveFields],
  );
  const error = "error" in result ? result.error : null;
  const changed = "changes" in result ? Object.keys(result.changes).length : 0;

  const save = () => {
    if (editable && "changes" in result && changed > 0) onSave?.(result.changes);
  };
  const switchTo = (next: DocumentFormat) => {
    if (next === format || "error" in read) return;
    setText(next.write(read.fields));
    setFormat(next);
  };
  const reformat = (indent: number | null) => {
    if (!("error" in read)) setText(reformatJson(text, indent));
    ref.current?.focus();
  };

  return (
    <div className={cn("flex min-h-0 flex-col gap-2", className)}>
      <div className="flex flex-wrap items-center gap-1.5">
        {formats.length > 1 && (
          <div role="radiogroup" aria-label="Format" className="flex items-center gap-1">
            {formats.map((f) => (
              <Button
                key={f.id}
                type="button"
                size="sm"
                variant={f === format ? "secondary" : "ghost"}
                role="radio"
                aria-checked={f === format}
                disabled={f !== format && "error" in read}
                title={f !== format && "error" in read ? "Fix the document first" : undefined}
                onClick={() => switchTo(f)}
              >
                {f.label}
              </Button>
            ))}
          </div>
        )}
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={"error" in read}
          onClick={() => reformat(2)}
        >
          Format
        </Button>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={"error" in read}
          onClick={() => reformat(null)}
        >
          Minify
        </Button>
        {toolbar}
      </div>
      <Textarea
        ref={ref}
        value={text}
        readOnly={!editable}
        aria-label={label}
        aria-invalid={error ? true : undefined}
        spellCheck={false}
        autoComplete="off"
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.nativeEvent.isComposing || !editable) return;
          if (e.key === "Tab" && !e.shiftKey && !e.metaKey && !e.ctrlKey && !e.altKey) {
            e.preventDefault();
            insertAtCaret(e.currentTarget, "  ");
          } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            save();
          }
        }}
        className="[field-sizing:fixed] min-h-32 flex-1 resize-y font-mono text-xs leading-5 md:text-xs"
      />
      <div
        role={error ? "alert" : undefined}
        className={cn(
          "min-h-4 truncate text-[11px]",
          error ? "text-error" : "text-muted-foreground",
        )}
        title={error ?? undefined}
      >
        {error ??
          (changed === 0 ? "No changes" : `${changed} changed field${changed === 1 ? "" : "s"}`)}
      </div>
      {footer}
      {(editable || onCancel) && (
        <div className="flex justify-end gap-1.5">
          {onCancel && (
            <Button type="button" variant="ghost" size="sm" onClick={onCancel}>
              Cancel
            </Button>
          )}
          {editable && (
            <Button type="button" size="sm" disabled={!!error || changed === 0} onClick={save}>
              {saveLabel}
              <kbd className="ml-1 text-[10px] opacity-70">{formatCombo("Mod+Enter")}</kbd>
            </Button>
          )}
        </div>
      )}
    </div>
  );
}
