import { insertNewline } from "@codemirror/commands";
import { searchPanelOpen } from "@codemirror/search";
import type { EditorView } from "@codemirror/view";
import {
  ArrowDownAZ,
  Binary,
  Braces,
  ChevronsUpDown,
  Copy,
  Ellipsis,
  KeyRound,
} from "lucide-react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { jsonError, reformatJson } from "@/features/datagrid/editors/json";
import { useLacksFields } from "@/features/datagrid/lacksFields";
import { JsonEditorDialog } from "@/features/datagrid/editors/JsonEditorDialog";
import { CodeView } from "@/features/datagrid/quicklook/CodeView";
import { readSortKeys, writeSortKeys } from "@/features/datagrid/quicklook/jsonPrefs";
import { sortJsonKeys } from "@/features/datagrid/quicklook/jsonTree";
import { detectLanguage, type ValueLanguage } from "@/features/datagrid/quicklook/languages";
import { choiceKeyOf, choicesFor, NULL_CHOICE } from "@/features/datagrid/editors/registry";
import { NOW_KINDS, NOW_LABEL, nowValue } from "@/features/datagrid/setValue";
import { acceptsEmpty, type GridColumn } from "@/features/datagrid/types";
import { actions } from "@/features/palette/actions";
import type { Value } from "@/lib/ipc";
import { comboFromEvent, formatCombo } from "@/lib/keys/combo";
import { cn } from "@/lib/utils";
import { decodeBytesText, editText, formatBytes, hasLineBreak } from "@/lib/values";
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

/** Bytes of a blob decoded for the text shown in its read-only box, and how many characters show. */
const BLOB_TEXT_BYTES = 4096;
const BLOB_TEXT_CHARS = 40;

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
  /** Bulk edit: the selected rows hold different values here ("Multiple values"). */
  mixed?: boolean;
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
 * not staged. Every text-like field (text, JSON, XML, enum, set, array, spatial and other values)
 * is a CodeMirror (the Quick Look Editor's, compact): it grows with its text, has syntax colours,
 * multiple carets (⌘-click, ⌃⇧↑ / ⌃⇧↓) and ⌘F search and replace, and a one-line value is
 * committed by a plain ↩ (⌥↩ adds a line) like a native box. Numbers, dates, UUIDs and the like
 * stay plain one-line inputs.
 */
export function FieldBlock({
  col,
  value,
  rowKey,
  focused,
  hidden,
  changed,
  mixed = false,
  onEdit,
  onViewCell,
}: FieldBlockProps) {
  const lacksFields = useLacksFields();
  /** The field's "Pretty JSON" (on: valid JSON shows indented; off: as stored). */
  const [prettyJson, setPrettyJson] = useState(true);
  /** "Sort keys" (remembered): the stored JSON's object entries in key order, for display only. */
  const [sortKeys, setSortKeys] = useState(() => readSortKeys("details"));
  const sortable = useMemo(
    () => !mixed && value.t === "json" && jsonError(value.v) === null,
    [mixed, value],
  );
  const base = useMemo(() => {
    const text = mixed ? "" : prettyJson ? fieldText(value) : editText(value);
    return sortKeys && sortable ? sortJsonKeys(text) : text;
  }, [mixed, prettyJson, value, sortKeys, sortable]);
  const ident = `${rowKey}\u0000${mixed ? "mixed" : value.t}\u0000${base}`;
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
  const isBytes = value.t === "bytes" && !mixed;
  const typeable = editable && col.kind !== "bytes" && !isBytes;
  const text = draft ?? base;
  const dirty = draft !== null && draft !== base;
  const multiline = col.kind === "json" || hasLineBreak(text) || hasLineBreak(base);
  /** The text-like kinds are a CodeMirror; the others a one-line input. */
  const useCode = TEXTAREA_KINDS.has(col.kind);
  const truncatedBlob = value.t === "bytes" && value.v.truncated;
  /** A blob that is text: its start, for the read-only box (the Cell view has all of it). */
  const blobText = useMemo(() => {
    if (value.t !== "bytes" || mixed) return null;
    const decoded = decodeBytesText(value.v.base64, BLOB_TEXT_BYTES, value.v.truncated);
    const flat = decoded?.replace(/\s+/g, " ").trim();
    if (!flat) return null;
    const chars = Array.from(flat);
    const cut =
      chars.length > BLOB_TEXT_CHARS || value.v.truncated || value.v.len > BLOB_TEXT_BYTES;
    return chars.slice(0, BLOB_TEXT_CHARS).join("") + (cut ? "…" : "");
  }, [value, mixed]);
  // JSON / XML are coloured (TablePlus b300), a one-line document too.
  const language = useMemo<ValueLanguage | null>(
    () => (!mixed && useCode ? detectLanguage(col.kind, col.dataType, text) : null),
    [mixed, useCode, col.kind, col.dataType, text],
  );
  /** JSON this field could format / minify (its stored text, or the draft). */
  const jsonText =
    !mixed && (col.kind === "json" || value.t === "json") && jsonError(text) === null;

  const changeSort = (next: boolean) => {
    setSortKeys(next);
    writeSortKeys("details", next);
  };

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
    const result = parseFieldText(col, draft, value, lacksFields);
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
  /** The JSON editor's Save: stages its text (an unchanged one stages nothing); false when invalid. */
  const applyJson = (t: string): boolean => {
    setJsonOpen(false);
    if (t === base) {
      revert();
      return true;
    }
    const result = parseFieldText(col, t, value, lacksFields);
    if (result.ok) {
      stage(result.value);
      return true;
    }
    setError(result.error);
    return false;
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

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
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
      // A one-line input: ↩ (and ⌘↩) stages.
      e.preventDefault();
      commit();
    }
  };

  /** The keys of the CodeMirror box: ⌘S, ⎋ and ↩ as in the native ones (stage, revert, stage). */
  const onCodeKeyDown = (e: KeyboardEvent, view: EditorView): boolean => {
    const combo = comboFromEvent(e);
    if (combo === "Mod+S") {
      e.preventDefault();
      if (commit()) window.setTimeout(() => void actions.run("grid.commit"), 0);
      return true;
    }
    if (e.key === "Escape") {
      // An open search panel and several carets take the first ⎋ (CodeMirror closes / collapses).
      if (searchPanelOpen(view.state) || view.state.selection.ranges.length > 1) return false;
      e.preventDefault();
      if (draft !== null || error) revert();
      else focusGrid();
      return true;
    }
    if (e.key === "Enter") {
      if (combo === "Mod+Enter") {
        e.preventDefault();
        commit();
        return true;
      }
      if (e.altKey && !e.metaKey && !e.ctrlKey) {
        // ⌥↩ — a line break (like a macOS text field); the box turns multi-line.
        e.preventDefault();
        return insertNewline(view);
      }
      if (!multiline) {
        // A one-line value: ↩ stages, like the native boxes. Once it spans lines a plain ↩ is a
        // line break (CodeMirror's) and ⌘↩ stages.
        e.preventDefault();
        commit();
        return true;
      }
    }
    return false;
  };
  const onReadOnlyCodeKeyDown = (e: KeyboardEvent, view: EditorView): boolean => {
    if (e.key !== "Escape") return false;
    if (searchPanelOpen(view.state) || view.state.selection.ranges.length > 1) return false;
    focusGrid();
    return true;
  };

  const placeholder = mixed
    ? "Multiple values"
    : draft !== null && draft !== base
      ? clearedPlaceholder(col)
      : placeholderOf(value);
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
        {blobText !== null && (
          <span
            data-testid="field-blob-text"
            className="selectable min-w-0 flex-1 truncate"
            title={blobText}
          >
            {blobText}
          </span>
        )}
        <span
          className={cn(
            "text-muted-foreground selectable truncate",
            blobText !== null ? "shrink-0 font-sans text-[10px]" : "min-w-0 flex-1",
          )}
        >
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
  } else if (mixed && col.kind === "bytes") {
    box = (
      <div className={cn(BOX, "text-null flex h-[26px] items-center italic")}>Multiple values</div>
    );
  } else if (typeable && isChoiceColumn(col)) {
    box = (
      <ChoiceBox
        col={col}
        value={value}
        mixed={mixed}
        className={boxClass}
        onPick={(v) => v && stage(v)}
      />
    );
  } else {
    box = useCode ? (
      <CodeView
        compact
        text={text}
        wrap
        label={label}
        language={language}
        placeholder={placeholder}
        invalid={!!error}
        readOnly={!typeable}
        // Keeps the grid's shortcuts (⌘Z, ⌘S…) off a box being typed in, like the native ones.
        shortcutScope={typeable ? "grid-edit" : "global"}
        onChange={
          typeable
            ? (t) => {
                // What the user typed; the document following a new value is not an edit.
                if (t === text) return;
                setDraft(t);
                setError(null);
              }
            : undefined
        }
        onKeyDown={typeable ? onCodeKeyDown : onReadOnlyCodeKeyDown}
        onBlur={
          typeable
            ? () => {
                if (draft !== null) commit();
              }
            : undefined
        }
        className={cn(
          "bg-surface focus-within:border-ring focus-within:ring-ring/30 min-h-[26px] w-full min-w-0 font-mono focus-within:ring-2",
          changed && "bg-changed [&_.cm-editor]:bg-transparent",
          error && "border-error focus-within:border-error focus-within:ring-error/30",
        )}
      />
    ) : (
      <input
        value={text}
        aria-label={label}
        aria-invalid={error ? true : undefined}
        readOnly={!typeable}
        placeholder={placeholder}
        spellCheck={false}
        autoComplete="off"
        autoCorrect="off"
        autoCapitalize="off"
        // Keeps grid shortcuts (⌘Z, ⌘S…) off a box being typed in; see `onKeyDown` for ⌘S.
        data-scope={typeable ? "grid-edit" : undefined}
        onChange={(e) => {
          setDraft(e.target.value);
          setError(null);
        }}
        onKeyDown={
          typeable
            ? onKeyDown
            : (e) => {
                if (e.key === "Escape") focusGrid();
              }
        }
        onBlur={
          typeable
            ? () => {
                if (draft !== null) commit();
              }
            : undefined
        }
        className={cn(boxClass, "h-[26px] tabular-nums")}
      />
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
            mixed={mixed}
            canCopy={!truncatedBlob && !mixed}
            onStage={stage}
            onCopy={() => copy(copyText(value))}
            onEditJson={() => setJsonOpen(true)}
            onViewCell={onViewCell}
            json={
              jsonText
                ? {
                    pretty: prettyJson,
                    onPrettyChange: setPrettyJson,
                    sort: sortKeys,
                    onSortChange: sortable ? changeSort : undefined,
                    // Format / Minify change the JSON itself (staged like an edit).
                    onReformat: typeable
                      ? (indent) => {
                          const next = reformatJson(text, indent);
                          if (next === editText(value)) return revert();
                          const result = parseFieldText(col, next, value, lacksFields);
                          if (result.ok) stage(result.value);
                        }
                      : undefined,
                  }
                : undefined
            }
          />
        ) : (
          <>
            {sortable && (
              <button
                type="button"
                aria-label={`Sort keys in ${col.name}`}
                aria-pressed={sortKeys}
                title={sortKeys ? "Show the keys as stored" : "Sort keys"}
                onClick={() => changeSort(!sortKeys)}
                className={cn(
                  "hover:text-foreground hover:bg-accent flex size-4 shrink-0 items-center justify-center rounded opacity-60 group-hover/field:opacity-100 focus-visible:opacity-100",
                  sortKeys ? "text-brand" : "text-muted-foreground",
                )}
              >
                <ArrowDownAZ className="size-3" />
              </button>
            )}
            {jsonText && (
              <button
                type="button"
                aria-label={`Pretty JSON in ${col.name}`}
                aria-pressed={prettyJson}
                title={prettyJson ? "Show the JSON as stored" : "Pretty JSON"}
                onClick={() => setPrettyJson(!prettyJson)}
                className={cn(
                  "hover:text-foreground hover:bg-accent flex size-4 shrink-0 items-center justify-center rounded opacity-60 group-hover/field:opacity-100 focus-visible:opacity-100",
                  prettyJson ? "text-brand" : "text-muted-foreground",
                )}
              >
                <Braces className="size-3" />
              </button>
            )}
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
          </>
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
          onSave={(t) => void applyJson(t)}
          // ⌘S in the dialog: the text is staged, and only then is the tab committed.
          onSaveAndCommit={applyJson}
        />
      )}
    </div>
  );
}

/** bool / enum box: a native picker; choosing stages at once. */
function ChoiceBox({
  col,
  value,
  mixed = false,
  className,
  onPick,
}: {
  col: GridColumn;
  value: Value;
  /** Bulk edit with differing values: nothing is picked yet. */
  mixed?: boolean;
  className: string;
  onPick: (value: Value | null) => void;
}) {
  const lacksFields = useLacksFields();
  const choices = useMemo(() => choicesFor(col), [col]);
  const key = mixed ? null : choiceKeyOf(value, choices);
  const isNullish = mixed || value.t === "null" || value.t === "default";
  return (
    <div className="relative">
      <select
        aria-label={col.name}
        value={key ?? ""}
        onChange={(e) => {
          const picked = e.target.value;
          if (picked === NULL_CHOICE) return onPick({ t: "null" });
          const result = parseFieldText(col, picked, value, lacksFields);
          onPick(result.ok ? result.value : null);
        }}
        className={cn(className, "h-[26px] appearance-none pr-6", isNullish && "text-null italic")}
      >
        {key === null && (
          <option value="" disabled>
            {mixed
              ? "Multiple values"
              : value.t === "default"
                ? "DEFAULT"
                : value.t === "null"
                  ? "NULL"
                  : editText(value)}
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

/**
 * The ⋯ menu of an editable field: Set NULL / EMPTY / DEFAULT / NOW() (dates and times), JSON
 * editor, copy, Cell view.
 */
function FieldMenu({
  col,
  value,
  mixed = false,
  canCopy,
  onStage,
  onCopy,
  onEditJson,
  onViewCell,
  json,
}: {
  col: GridColumn;
  value: Value;
  /** Bulk edit with differing values: every Set item applies. */
  mixed?: boolean;
  canCopy: boolean;
  onStage: (value: Value) => void;
  onCopy: () => void;
  onEditJson: () => void;
  onViewCell: () => void;
  /** A JSON value: the Pretty JSON toggle, Sort keys, and Format / Minify (editable fields). */
  json?: {
    pretty: boolean;
    onPrettyChange: (pretty: boolean) => void;
    /** Sort keys (display only); the item is offered when there is a handler. */
    sort: boolean;
    onSortChange?: (sort: boolean) => void;
    onReformat?: (indent: number | null) => void;
  };
}) {
  const isEmpty = !mixed && value.t === "text" && value.v === "";
  const now = NOW_KINDS.has(col.kind);
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
            disabled={!mixed && value.t === "null"}
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
            disabled={!mixed && value.t === "default"}
            onSelect={() => onStage({ t: "default" })}
          >
            Set DEFAULT
          </DropdownMenuItem>
        )}
        {now && (
          <DropdownMenuItem
            className="text-xs"
            onSelect={() => {
              const v = nowValue(col.kind);
              if (v) onStage(v);
            }}
          >
            Set {NOW_LABEL}
          </DropdownMenuItem>
        )}
        {col.kind === "json" && (
          <DropdownMenuItem className="text-xs" onSelect={onEditJson}>
            Edit JSON…
          </DropdownMenuItem>
        )}
        {json && (
          <>
            <DropdownMenuCheckboxItem
              className="text-xs"
              checked={json.pretty}
              onCheckedChange={(v) => json.onPrettyChange(v === true)}
            >
              Pretty JSON
            </DropdownMenuCheckboxItem>
            {json.onSortChange && (
              <DropdownMenuCheckboxItem
                className="text-xs"
                checked={json.sort}
                onCheckedChange={(v) => json.onSortChange?.(v === true)}
              >
                Sort keys
              </DropdownMenuCheckboxItem>
            )}
            {json.onReformat && (
              <>
                <DropdownMenuItem className="text-xs" onSelect={() => json.onReformat?.(2)}>
                  Format JSON
                </DropdownMenuItem>
                <DropdownMenuItem className="text-xs" onSelect={() => json.onReformat?.(null)}>
                  Minify JSON
                </DropdownMenuItem>
              </>
            )}
          </>
        )}
        {(col.nullable ||
          acceptsEmpty(col) ||
          col.hasDefault ||
          now ||
          col.kind === "json" ||
          !!json) && <DropdownMenuSeparator />}
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
