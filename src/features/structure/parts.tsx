import { useRef, useState, type KeyboardEvent, type MouseEvent, type ReactNode } from "react";
import { FormField } from "@/components/FormField";
import { SectionLabel } from "@/components/SectionLabel";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { actions } from "@/features/palette/actions";
import { formatCombo, isMac } from "@/lib/keys/combo";
import { cn } from "@/lib/utils";
import { MAX_PREFIX_LENGTH } from "./columnEdits";
import { entryBase, entryLength, withEntryLength, type RowStatus } from "./model";

/**
 * Header row of the section tables: sticks right under the section bar while the body scrolls
 * (a box-shadow draws the bottom rule because collapsed borders do not move with sticky cells).
 */
export const HEAD_ROW =
  "text-muted-foreground h-7 text-left text-xs [&>th]:bg-background [&>th]:sticky [&>th]:top-8 [&>th]:z-[5] [&>th]:shadow-[inset_0_-1px_0_var(--color-border)]";

/**
 * A ghost `Button` on a `bg-surface-2` bar (section bars, the structure header, the bottom bar),
 * where the primitive's `hover:bg-muted` is invisible (docs/ui-conventions.md, State tokens).
 */
export const BAR_BUTTON = "hover:bg-accent dark:hover:bg-accent aria-expanded:bg-accent";

/**
 * Compact section bar ("COLUMNS 16 ……… + Column"), sticky inside the scrolling body. It is as
 * wide as the scroll viewport (`100cqw`: the body is a size container) rather than the table, so
 * `left-0` keeps the title and actions in view while the tables scroll sideways.
 */
export function SectionHeader({
  title,
  count,
  children,
}: {
  title: string;
  count?: number;
  children?: ReactNode;
}) {
  return (
    <div
      data-section-header
      className="border-border bg-surface-2 sticky top-0 left-0 z-10 flex h-8 w-[100cqw] max-w-full min-w-0 items-center gap-2 border-y px-3"
    >
      <SectionLabel>{title}</SectionLabel>
      {count !== undefined && (
        <span className="text-muted-foreground text-xs font-normal tabular-nums">{count}</span>
      )}
      <span className="flex-1" />
      {children}
    </div>
  );
}

/** Ghost icon button with a tooltip, for row actions. */
export function IconAction({
  label,
  onClick,
  children,
  className,
  disabled,
  tabIndex,
}: {
  label: string;
  onClick: () => void;
  children: ReactNode;
  className?: string;
  disabled?: boolean;
  /** -1 keeps row actions out of the Tab order (Tab moves field to field). */
  tabIndex?: number;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          aria-label={label}
          onClick={onClick}
          disabled={disabled}
          tabIndex={tabIndex}
          className={cn(
            "text-muted-foreground hover:text-foreground hover:bg-accent dark:hover:bg-accent",
            className,
          )}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

/** Moves focus to the same field of the previous / next row of the same grid. */
function moveFocus(from: HTMLInputElement, delta: number) {
  const [row, field] = (from.dataset.cell ?? "").split("|");
  const grid = from.closest("[data-grid]");
  const target = grid?.querySelector<HTMLInputElement>(
    `[data-cell="${Number(row) + delta}|${field}"]`,
  );
  if (target) {
    target.focus();
    target.select();
  }
}

/**
 * Inline text cell: looks like plain text until hovered / focused. Commits on Enter or blur,
 * reverts on Escape; ↑/↓ move between rows; ⌘S commits and then opens the review sheet. Its
 * `grid-edit` scope keeps grid shortcuts (⌘Z…) from stealing native text editing keys.
 */
export function CellInput({
  value,
  onCommit,
  cell,
  cellKey,
  label,
  readOnly,
  placeholder,
  list,
  className,
  onLeave,
}: {
  value: string;
  /** Return false to refuse the edit: the cell then keeps the typed text, marked as invalid. */
  onCommit: (raw: string) => boolean | void;
  /** Called when focus leaves the cell (after any commit), with what it holds. */
  onLeave?: (raw: string) => void;
  /** `<row>|<field>` for ↑/↓ navigation. */
  cell: string;
  /** `<row key>:<field>` for programmatic focus. */
  cellKey: string;
  label: string;
  readOnly?: boolean;
  placeholder?: string;
  list?: string;
  className?: string;
}) {
  const [draft, setDraft] = useState(value);
  const [shown, setShown] = useState(value);
  // Set while ↑/↓ moves focus: that focus change blurs this input synchronously, before React
  // re-renders, and the blur must not commit the same draft a second time.
  const moving = useRef(false);
  // A refused edit keeps the typed text, selected and marked, so it can be fixed rather than
  // retyped; the next keystroke or Escape clears the mark.
  const [invalid, setInvalid] = useState(false);
  if (shown !== value) {
    setShown(value);
    setDraft(value);
    setInvalid(false);
  }
  /** False when the edit was refused and the cell kept the draft. */
  const commit = (input?: HTMLInputElement): boolean => {
    if (draft !== value && onCommit(draft) === false) {
      setInvalid(true);
      input?.select();
      return false;
    }
    setInvalid(false);
    setDraft(value);
    return true;
  };
  const onBlur = () => {
    const raw = draft;
    if (!moving.current) commit();
    onLeave?.(raw);
  };
  // Full text as a tooltip only when the cell clips it.
  const onMouseEnter = (e: MouseEvent<HTMLInputElement>) => {
    const el = e.currentTarget;
    el.title = el.value !== "" && el.scrollWidth > el.clientWidth ? el.value : "";
  };
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    const mod = e.metaKey || e.ctrlKey;
    if (e.key === "Enter" && !e.nativeEvent.isComposing) {
      e.preventDefault();
      commit(e.currentTarget);
    } else if (e.key === "Escape") {
      if (draft !== value) {
        e.preventDefault();
        e.stopPropagation();
        setDraft(value);
        setInvalid(false);
      } else {
        // Leave the cell but stay in the grid so its shortcuts (⌘N, ⌘Z, ⌘S…) keep working.
        const grid = e.currentTarget.closest<HTMLElement>("[data-scope='grid']");
        if (grid) grid.focus({ preventScroll: true });
        else e.currentTarget.blur();
      }
    } else if (mod && !e.shiftKey && e.key.toLowerCase() === "s") {
      e.preventDefault();
      if (commit(e.currentTarget)) void actions.run("grid.commit");
    } else if (
      (e.key === "ArrowUp" || e.key === "ArrowDown") &&
      !list &&
      !mod &&
      !e.altKey &&
      !e.shiftKey
    ) {
      e.preventDefault();
      if (!commit(e.currentTarget)) return;
      moving.current = true;
      try {
        moveFocus(e.currentTarget, e.key === "ArrowUp" ? -1 : 1);
      } finally {
        moving.current = false;
      }
    }
  };
  return (
    <input
      data-scope="grid-edit"
      data-cell={cell}
      data-cell-key={cellKey}
      aria-label={label}
      value={draft}
      onChange={(e) => {
        setDraft(e.target.value);
        setInvalid(false);
      }}
      aria-invalid={invalid || undefined}
      onBlur={onBlur}
      onKeyDown={onKeyDown}
      onMouseEnter={onMouseEnter}
      readOnly={readOnly}
      placeholder={placeholder}
      list={list}
      spellCheck={false}
      autoComplete="off"
      autoCorrect="off"
      autoCapitalize="off"
      className={cn(
        "hover:border-border focus:border-ring focus:bg-surface placeholder:text-muted-foreground h-6 w-full min-w-0 rounded-sm border border-transparent bg-transparent px-1.5 text-xs outline-none read-only:hover:border-transparent",
        readOnly && "cursor-default",
        invalid && "border-error hover:border-error focus:border-error",
        className,
      )}
    />
  );
}

/** The key that drops the focused structure row (`rowDeleteKey` takes either). */
const DELETE_ROW_KEY = isMac ? "Backspace" : "Delete";

/**
 * The keys next to a menu item ("⌘I", "Ctrl+I"). Hidden from the accessible name, so the item is
 * still announced (and found) as "Add Column".
 */
export function MenuKeys({ combo }: { combo: string }) {
  return <ContextMenuShortcut aria-hidden>{formatCombo(combo)}</ContextMenuShortcut>;
}

/** "⌫" / "Delete" next to a Delete / Restore menu item of a structure row. */
export function DeleteRowShortcut() {
  return <MenuKeys combo={DELETE_ROW_KEY} />;
}

/** Copy Name of a structure row's menu. */
export function CopyNameItem({ name }: { name: string }) {
  return <ContextMenuItem onSelect={() => copyName(name)}>Copy Name</ContextMenuItem>;
}

function copyName(name: string): void {
  const clip = typeof navigator !== "undefined" ? navigator.clipboard : undefined;
  if (!clip) {
    toast.error("Could not copy");
    return;
  }
  void clip.writeText(name).then(
    () => toast.success("Copied name", { description: name }),
    () => toast.error("Could not copy"),
  );
}

/**
 * Right-click menu of an index, check, foreign key or trigger row, like the column rows have:
 * Add, Edit, Copy Name, then Delete / Restore, with the keys that do the same on the row.
 * Without any action (a read-only grid with nothing to copy) the row is returned as is.
 */
export function RowContextMenu({
  children,
  what,
  name,
  status,
  onAdd,
  addKeys,
  onEdit,
  editLabel,
  onDrop,
  onRestore,
}: {
  /** The `<tr>`. */
  children: ReactNode;
  /** "Index", "Check", "Foreign Key", "Trigger". */
  what: string;
  name: string | null;
  status: RowStatus;
  onAdd?: () => void;
  /** The shortcut that adds one while the grid has focus (⌘I for indexes). */
  addKeys?: string | null;
  onEdit?: () => void;
  editLabel?: string;
  /** Left out where the row cannot be dropped (read-only, or an unnamed SQLite key). */
  onDrop?: () => void;
  onRestore?: () => void;
}) {
  const dropped = status === "dropped";
  const drop = dropped ? onRestore : onDrop;
  if (!onAdd && !onEdit && !name && !drop) return <>{children}</>;
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
      <ContextMenuContent className="min-w-48" aria-label={`${what} ${name ?? ""}`.trim()}>
        {onAdd && (
          <ContextMenuItem onSelect={onAdd}>
            Add {what}
            {addKeys && <MenuKeys combo={addKeys} />}
          </ContextMenuItem>
        )}
        {onEdit && !dropped && (
          <ContextMenuItem onSelect={onEdit}>
            {editLabel ?? `Edit ${what}…`}
            <MenuKeys combo="Enter" />
          </ContextMenuItem>
        )}
        {name && <CopyNameItem name={name} />}
        {drop && (
          <>
            <ContextMenuSeparator />
            <ContextMenuItem onSelect={drop}>
              {dropped
                ? `Restore ${what}`
                : status === "added"
                  ? `Remove New ${what}`
                  : `Delete ${what}`}
              <DeleteRowShortcut />
            </ContextMenuItem>
          </>
        )}
      </ContextMenuContent>
    </ContextMenu>
  );
}

/** MySQL index prefixes: which columns can take a length, and which cannot go without one. */
export interface PrefixLengths {
  eligible: (column: string) => boolean;
  required: (column: string) => boolean;
}

/**
 * Ordered multi-select as chips: clicking appends to the selection (the badge shows the position),
 * clicking again removes it. With `lengths` (MySQL index columns) every picked char / text / blob
 * column gets a small length input that turns its entry into `name(10)`; entries are matched to
 * the chips by their column name, and a TEXT / BLOB column left without a length is flagged.
 */
export function OrderedPicker({
  options,
  value,
  onChange,
  label,
  empty,
  lengths,
}: {
  options: readonly string[];
  value: readonly string[];
  onChange: (next: string[]) => void;
  label: string;
  empty?: string;
  lengths?: PrefixLengths;
}) {
  if (options.length === 0)
    return <p className="text-muted-foreground text-xs">{empty ?? "Nothing to pick"}</p>;
  const find = (name: string) =>
    lengths ? value.findIndex((e) => entryBase(e) === name) : value.indexOf(name);
  const unsized = lengths
    ? value.flatMap((e) =>
        lengths.required(entryBase(e)) && entryLength(e) === null ? [entryBase(e)] : [],
      )
    : [];
  return (
    <>
      <div role="group" aria-label={label} className="flex flex-wrap gap-1">
        {options.map((name) => {
          const pos = find(name);
          const on = pos >= 0;
          const chip = (
            <button
              key={name}
              type="button"
              aria-pressed={on}
              onClick={() => onChange(on ? value.filter((_, i) => i !== pos) : [...value, name])}
              className={cn(
                "border-border hover:bg-accent focus-visible:ring-ring/50 inline-flex h-6 items-center gap-1 rounded-md border px-1.5 font-mono text-xs transition-colors outline-none focus-visible:ring-2",
                on && "border-brand/60 bg-brand-soft text-brand hover:bg-brand-soft",
              )}
            >
              {on && (
                <span className="bg-brand text-primary-foreground inline-flex size-3.5 items-center justify-center rounded-full font-sans text-[9px] font-medium tabular-nums">
                  {pos + 1}
                </span>
              )}
              {name}
            </button>
          );
          if (!lengths || !on || !lengths.eligible(name)) return chip;
          const length = entryLength(value[pos]!);
          return (
            <span key={name} className="inline-flex items-center gap-0.5">
              {chip}
              <input
                aria-label={`${name} prefix length`}
                title="Index prefix: the first N characters (empty: the whole column)"
                inputMode="numeric"
                placeholder="len"
                value={length === null ? "" : String(length)}
                onChange={(e) => {
                  const digits = e.target.value.replace(/\D/g, "");
                  const n = digits === "" ? 0 : Math.min(MAX_PREFIX_LENGTH, Number(digits));
                  onChange(
                    value.map((entry, i) =>
                      i === pos ? withEntryLength(entry, n || null) : entry,
                    ),
                  );
                }}
                spellCheck={false}
                autoComplete="off"
                className={cn(
                  "border-border bg-background placeholder:text-muted-foreground focus-visible:ring-ring/50 h-6 w-11 rounded-md border px-1 text-center font-mono text-xs outline-none focus-visible:ring-2",
                  lengths.required(name) && length === null && "border-warning",
                )}
              />
            </span>
          );
        })}
      </div>
      {unsized.length > 0 && (
        <p role="status" className="text-warning text-xs">
          {unsized.map((c) => `“${c}”`).join(", ")} {unsized.length === 1 ? "is a" : "are"} TEXT or
          BLOB {unsized.length === 1 ? "column" : "columns"}: MySQL cannot index{" "}
          {unsized.length === 1 ? "it" : "them"} in full, type a length.
        </p>
      )}
    </>
  );
}

/**
 * Label + control row of the small DDL dialogs and popovers: the shared stacked `FormField`
 * (docs/ui-conventions.md, Forms), kept under this name for the dialogs that import it.
 */
export function Field({
  label,
  htmlFor,
  hint,
  error,
  children,
}: {
  label: string;
  htmlFor?: string;
  hint?: ReactNode;
  error?: ReactNode;
  children: ReactNode;
}) {
  return (
    <FormField label={label} htmlFor={htmlFor} hint={hint} error={error}>
      {children}
    </FormField>
  );
}
