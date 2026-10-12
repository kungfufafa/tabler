import { Binary, ChevronsUpDown, Copy, Ellipsis, KeyRound } from "lucide-react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { JsonEditorDialog } from "@/features/datagrid/editors/JsonEditorDialog";
import { choiceKeyOf, choicesFor, NULL_CHOICE } from "@/features/datagrid/editors/registry";
import { insertAtCaret } from "@/features/datagrid/editors/useGrabFocus";
import { acceptsEmpty, type GridColumn } from "@/features/datagrid/types";
import { actions } from "@/features/palette/actions";
import type { Value } from "@/lib/ipc";
import { comboFromEvent, formatCombo } from "@/lib/keys/combo";
import { cn } from "@/lib/utils";
import { editText, formatBytes, hasLineBreak } from "@/lib/values";
import { copy } from "./copy";
import {
  clearedPlaceholder,
  copyText,
  fieldText,
  isChoiceColumn,
  parseFieldText,
  placeholderOf,
  TEXTAREA_KINDS,
} from "./fields";

/** Browsers that size a text area to its content natively (`field-sizing: content`). */
const NATIVE_AUTOSIZE =
  typeof CSS !== "undefined" &&
  typeof CSS.supports === "function" &&
  CSS.supports("field-sizing", "content");

/** The value box: an input-like, monospace 12px field on the panel's darker background. */
const BOX = cn(
  "border-border bg-surface text-foreground block w-full min-w-0 rounded-md border px-2 font-mono text-[12px] leading-[18px] outline-none",
  "placeholder:text-null placeholder:italic",
  "focus:border-ring focus:ring-ring/30 focus:ring-2",
);

export interface FieldBlockProps {
  col: GridColumn;
  value: Value;
  /** Identity of the inspected row: when it changes, an unstaged draft is dropped. */
  rowKey: string;
  /** The grid's focused column (highlighted). */
  focused: boolean;
  /** Hidden in the grid (shown muted). */
  hidden: boolean;
  /** Holds a staged edit (highlighted value). */
  changed: boolean;
  /** Stages a value into this field; absent when the field is read-only. */
  onEdit?: (value: Value) => void;
  /** Shows this field in the Cell view (hex dump, full blob load). */
  onViewCell: () => void;
}

/** Leaves the panel for the active tab's grid (⎋ in a field with nothing to revert). */
function focusGrid() {
  document.querySelector<HTMLElement>('[data-scope="grid"]')?.focus({ preventScroll: true });
}

/**
 * One field of the Details view: the column name (with a key for primary keys) and its type on a
 * header line, the value in an input-like box below. Editable boxes stage their value like an
 * inline grid edit: ↩ (⌘↩ once the text spans lines), blur, or a click elsewhere commits; ⎋
 * reverts; ⌘S commits the field first and then the whole tab. Invalid text shows an error and is
 * not staged.
 */
export function FieldBlock({
  col,
  value,
  rowKey,
  focused,
  hidden,
  changed,
  onEdit,
  onViewCell,
}: FieldBlockProps) {
  const base = fieldText(value);
  const ident = `${rowKey}\u0000${value.t}\u0000${base}`;
  const [seen, setSeen] = useState(ident);
  /** Unstaged text; null while the box shows the value itself. */
  const [draft, setDraft] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [jsonOpen, setJsonOpen] = useState(false);
  // Another row, or a new value for this one (staged here or in the grid): drop the draft.
  if (seen !== ident) {
    setSeen(ident);
    setDraft(null);
    setError(null);
  }
  const blockRef = useRef<HTMLDivElement>(null);

  const editable = !!onEdit;
  const isBytes = value.t === "bytes";
  const typeable = editable && col.kind !== "bytes" && !isBytes;
  const text = draft ?? base;
  const dirty = draft !== null && draft !== base;
  const multiline = col.kind === "json" || hasLineBreak(text) || hasLineBreak(base);
  const truncatedBlob = value.t === "bytes" && value.v.truncated;

  const stage = (v: Value) => {
    setDraft(null);
    setError(null);
    onEdit?.(v);
  };
  /** Stages the draft; false when it does not parse (the error shows and nothing is staged). */
  const commit = (): boolean => {
    if (draft === null) return true;
    if (draft === base) {
      setDraft(null);
      setError(null);
      return true;
    }
    const result = parseFieldText(col, draft, value);
    if (!result.ok) {
      setError(result.error);
      return false;
    }
    stage(result.value);
    return true;
  };
  const revert = () => {
    setDraft(null);
    setError(null);
  };

  // A click outside the field (a grid row, the toolbar's Commit…) stages the draft BEFORE that
  // click's own handlers run: selecting another row re-targets this panel, and blur comes too late.
  const commitRef = useRef(commit);
  useLayoutEffect(() => {
    commitRef.current = commit;
  });
  useEffect(() => {
    if (!dirty) return;
    const onPointerDown = (e: PointerEvent) => {
      if (e.target instanceof Node && blockRef.current?.contains(e.target)) return;
      commitRef.current();
    };
    window.addEventListener("pointerdown", onPointerDown, true);
    return () => window.removeEventListener("pointerdown", onPointerDown, true);
  }, [dirty]);

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement | HTMLTextAreaElement>) => {
    if (e.nativeEvent.isComposing) return;
    const combo = comboFromEvent(e.nativeEvent);
    if (combo === "Mod+S") {
      // The box's `grid-edit` scope keeps the global ⌘S away; stage first, then commit the tab
      // once it has re-rendered with the new change.
      e.preventDefault();
      if (commit()) window.setTimeout(() => void actions.run("grid.commit"), 0);
    } else if (e.key === "Escape") {
      e.preventDefault();
      if (draft !== null || error) revert();
      else focusGrid();
    } else if (e.key === "Enter") {
      if (combo === "Mod+Enter") {
        e.preventDefault();
        commit();
      } else if (multiline) {
        // A plain ↩ adds a line.
      } else if (e.altKey) {
        // ⌥↩ — a line break (like a macOS text field); the box turns multi-line.
        e.preventDefault();
        if (TEXTAREA_KINDS.has(col.kind)) insertAtCaret(e.currentTarget, "\n");
      } else {
        e.preventDefault();
        commit();
      }
    }
  };

  const placeholder =
    draft !== null && draft !== base ? clearedPlaceholder(col) : placeholderOf(value);
  const boxClass = cn(
    BOX,
    changed && "bg-changed",
    error && "border-error focus:border-error focus:ring-error/30",
  );
  const label = col.name;

  let box: React.ReactNode;
  if (isBytes) {
    box = (
      <div
        className={cn(BOX, "flex h-[26px] items-center gap-1.5", changed && "bg-changed")}
        title={truncatedBlob ? "Only a preview is loaded" : undefined}
      >
        <Binary className="text-muted-foreground size-3 shrink-0" />
        <span className="text-muted-foreground selectable min-w-0 flex-1 truncate">
          {formatBytes(value.v.len)} binary{truncatedBlob ? " (preview)" : ""}
        </span>
        <button
          type="button"
          onClick={onViewCell}
          aria-label={`Show ${col.name} in the Cell view`}
          title="Hex dump and full value in the Cell view"
          className="text-brand shrink-0 font-sans text-[11px] hover:underline"
        >
          View
        </button>
      </div>
    );
  } else if (typeable && isChoiceColumn(col)) {
    box = <ChoiceBox col={col} value={value} className={boxClass} onPick={(v) => v && stage(v)} />;
  } else {
    const common = {
      value: text,
      "aria-label": label,
      "aria-invalid": error ? true : undefined,
      readOnly: !typeable,
      placeholder,
      spellCheck: false,
      autoComplete: "off",
      autoCorrect: "off",
      autoCapitalize: "off",
      // Keeps grid shortcuts (⌘Z, ⌘S…) off a box being typed in; see `onKeyDown` for ⌘S.
      "data-scope": typeable ? "grid-edit" : undefined,
      onChange: (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => {
        setDraft(e.target.value);
        setError(null);
      },
      onKeyDown: typeable
        ? onKeyDown
        : (e: React.KeyboardEvent) => {
            if (e.key === "Escape") focusGrid();
          },
      onBlur: typeable
        ? () => {
            if (draft !== null) commit();
          }
        : undefined,
    } as const;
    box = TEXTAREA_KINDS.has(col.kind) ? (
      <AutoTextarea
        {...common}
        className={cn(boxClass, "max-h-60 min-h-[26px] resize-none py-[3px] wrap-anywhere")}
      />
    ) : (
      <input {...common} className={cn(boxClass, "h-[26px] tabular-nums")} />
    );
  }

  const saveKeys = formatCombo(multiline ? "Mod+Enter" : "Enter");
  return (
    <div
      ref={blockRef}
      role="listitem"
      data-focused={focused || undefined}
      className={cn(
        "group/field rounded-md px-1.5 pt-1 pb-1.5",
        focused && "bg-brand-soft",
        hidden && "opacity-60",
      )}
    >
      <div className="flex h-5 items-center gap-1 text-[11px] leading-4">
        {col.isPk && (
          <KeyRound className="text-warning size-3 shrink-0" aria-label="Primary key" role="img" />
        )}
        <span
          className="text-muted-foreground min-w-0 truncate"
          title={`${col.name} · ${col.dataType}${col.nullable ? "" : " · not null"}${col.editable ? "" : " · read-only"}`}
        >
          {col.name}
        </span>
        {hidden && (
          <span className="text-muted-foreground border-border shrink-0 rounded border px-1 text-[9px] leading-3 tracking-wide uppercase">
            hidden
          </span>
        )}
        <span className="flex-1" />
        <span className="text-muted-foreground/80 max-w-[55%] shrink-0 truncate font-mono text-[10px]">
          {col.dataType}
        </span>
        {editable ? (
          <FieldMenu
            col={col}
            value={value}
            canCopy={!truncatedBlob}
            onStage={stage}
            onCopy={() => copy(copyText(value))}
            onEditJson={() => setJsonOpen(true)}
            onViewCell={onViewCell}
          />
        ) : (
          <button
            type="button"
            aria-label={`Copy ${col.name}`}
            title={truncatedBlob ? "Only a preview is loaded" : "Copy value"}
            disabled={truncatedBlob}
            onClick={() => copy(copyText(value))}
            className="text-muted-foreground hover:text-foreground hover:bg-accent flex size-4 shrink-0 items-center justify-center rounded opacity-0 group-hover/field:opacity-100 focus-visible:opacity-100 disabled:opacity-0"
          >
            <Copy className="size-3" />
          </button>
        )}
      </div>
      {box}
      {error ? (
        <p role="alert" className="text-error mt-1 text-[11px] leading-4">
          {error}
        </p>
      ) : (
        dirty && (
          <p className="text-muted-foreground mt-0.5 text-[10px] leading-3">
            {saveKeys} to stage · {formatCombo("Escape")} to revert
          </p>
        )
      )}
      {jsonOpen && (
        <JsonEditorDialog
          col={col}
          initialText={text}
          onCancel={() => setJsonOpen(false)}
          onSetNull={
            col.nullable
              ? () => {
                  setJsonOpen(false);
                  stage({ t: "null" });
                }
              : undefined
          }
          onSave={(t) => {
            setJsonOpen(false);
            if (t === base) return revert();
            const result = parseFieldText(col, t, value);
            if (result.ok) stage(result.value);
            else setError(result.error);
          }}
        />
      )}
    </div>
  );
}

/** bool / enum box: a native picker; choosing stages at once. */
function ChoiceBox({
  col,
  value,
  className,
  onPick,
}: {
  col: GridColumn;
  value: Value;
  className: string;
  onPick: (value: Value | null) => void;
}) {
  const choices = useMemo(() => choicesFor(col), [col]);
  const key = choiceKeyOf(value, choices);
  const isNullish = value.t === "null" || value.t === "default";
  return (
    <div className="relative">
      <select
        aria-label={col.name}
        value={key ?? ""}
        onChange={(e) => {
          const picked = e.target.value;
          if (picked === NULL_CHOICE) return onPick({ t: "null" });
          const result = parseFieldText(col, picked, value);
          onPick(result.ok ? result.value : null);
        }}
        className={cn(className, "h-[26px] appearance-none pr-6", isNullish && "text-null italic")}
      >
        {key === null && (
          <option value="" disabled>
            {value.t === "default" ? "DEFAULT" : value.t === "null" ? "NULL" : editText(value)}
          </option>
        )}
        {choices.map((c) => (
          <option key={c.key} value={c.key}>
            {c.label}
          </option>
        ))}
      </select>
      <ChevronsUpDown className="text-muted-foreground pointer-events-none absolute top-1/2 right-1.5 size-3 -translate-y-1/2" />
    </div>
  );
}

/** The ⋯ menu of an editable field: Set NULL / EMPTY / DEFAULT, JSON editor, copy, Cell view. */
function FieldMenu({
  col,
  value,
  canCopy,
  onStage,
  onCopy,
  onEditJson,
  onViewCell,
}: {
  col: GridColumn;
  value: Value;
  canCopy: boolean;
  onStage: (value: Value) => void;
  onCopy: () => void;
  onEditJson: () => void;
  onViewCell: () => void;
}) {
  const isEmpty = value.t === "text" && value.v === "";
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label={`${col.name} options`}
          title="Field options"
          className="text-muted-foreground hover:text-foreground hover:bg-accent data-[state=open]:bg-accent data-[state=open]:text-foreground flex h-4 w-5 shrink-0 items-center justify-center rounded opacity-60 group-hover/field:opacity-100 focus-visible:opacity-100 data-[state=open]:opacity-100"
        >
          <Ellipsis className="size-3.5" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-auto min-w-40 text-xs">
        {col.nullable && (
          <DropdownMenuItem
            className="text-xs"
            disabled={value.t === "null"}
            onSelect={() => onStage({ t: "null" })}
          >
            Set NULL
          </DropdownMenuItem>
        )}
        {acceptsEmpty(col) && (
          <DropdownMenuItem
            className="text-xs"
            disabled={isEmpty}
            onSelect={() => onStage({ t: "text", v: "" })}
          >
            Set EMPTY
          </DropdownMenuItem>
        )}
        {col.hasDefault && (
          <DropdownMenuItem
            className="text-xs"
            disabled={value.t === "default"}
            onSelect={() => onStage({ t: "default" })}
          >
            Set DEFAULT
          </DropdownMenuItem>
        )}
        {col.kind === "json" && (
          <DropdownMenuItem className="text-xs" onSelect={onEditJson}>
            Edit JSON…
          </DropdownMenuItem>
        )}
        {(col.nullable || acceptsEmpty(col) || col.hasDefault || col.kind === "json") && (
          <DropdownMenuSeparator />
        )}
        <DropdownMenuItem className="text-xs" disabled={!canCopy} onSelect={onCopy}>
          Copy Value
        </DropdownMenuItem>
        <DropdownMenuItem className="text-xs" onSelect={onViewCell}>
          Show in Cell View
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** Sizes a text area to its content (where `field-sizing: content` is not supported). */
function fitHeight(el: HTMLTextAreaElement | null) {
  if (!el || NATIVE_AUTOSIZE) return;
  el.style.height = "auto";
  const border = el.offsetHeight - el.clientHeight;
  el.style.height = `${el.scrollHeight + border}px`;
}

/** A text area that grows with its content (up to its CSS max-height, then scrolls). */
function AutoTextarea({ className, ...props }: React.ComponentProps<"textarea">) {
  const ref = useRef<HTMLTextAreaElement>(null);
  useLayoutEffect(() => fitHeight(ref.current), [props.value]);
  useEffect(() => {
    const el = ref.current;
    if (!el || NATIVE_AUTOSIZE) return;
    // Wrapping changes with the panel's width.
    let width = el.clientWidth;
    const ro = new ResizeObserver(() => {
      if (el.clientWidth === width) return;
      width = el.clientWidth;
      fitHeight(el);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return (
    <textarea ref={ref} rows={1} className={cn(className, "field-sizing-content")} {...props} />
  );
}
