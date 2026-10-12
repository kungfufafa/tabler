import { useRef, useState, type KeyboardEvent, type MouseEvent, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { actions } from "@/features/palette/actions";
import { cn } from "@/lib/utils";

/**
 * Header row of the section tables: sticks right under the section bar while the body scrolls
 * (a box-shadow draws the bottom rule because collapsed borders do not move with sticky cells).
 */
export const HEAD_ROW =
  "text-muted-foreground h-7 text-left text-[11px] [&>th]:bg-background [&>th]:sticky [&>th]:top-8 [&>th]:z-[5] [&>th]:shadow-[inset_0_-1px_0_var(--color-border)]";

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
    <div className="border-border bg-surface-2 sticky top-0 left-0 z-10 flex h-8 w-[100cqw] max-w-full min-w-0 items-center gap-2 border-y px-3">
      <h3 className="text-muted-foreground text-[11px] font-semibold tracking-wide uppercase">
        {title}
      </h3>
      {count !== undefined && (
        <span className="text-muted-foreground/70 text-[11px] tabular-nums">{count}</span>
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
          className={cn("text-muted-foreground hover:text-foreground", className)}
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
  onCommit: (raw: string) => void;
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
  if (shown !== value) {
    setShown(value);
    setDraft(value);
  }
  const commit = () => {
    if (draft !== value) onCommit(draft);
    setDraft(value);
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
      commit();
    } else if (e.key === "Escape") {
      if (draft !== value) {
        e.preventDefault();
        e.stopPropagation();
        setDraft(value);
      } else {
        // Leave the cell but stay in the grid so its shortcuts (⌘N, ⌘Z, ⌘S…) keep working.
        const grid = e.currentTarget.closest<HTMLElement>("[data-scope='grid']");
        if (grid) grid.focus({ preventScroll: true });
        else e.currentTarget.blur();
      }
    } else if (mod && !e.shiftKey && e.key.toLowerCase() === "s") {
      e.preventDefault();
      commit();
      void actions.run("grid.commit");
    } else if (
      (e.key === "ArrowUp" || e.key === "ArrowDown") &&
      !list &&
      !mod &&
      !e.altKey &&
      !e.shiftKey
    ) {
      e.preventDefault();
      commit();
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
      onChange={(e) => setDraft(e.target.value)}
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
        "hover:border-border focus:border-ring focus:bg-surface placeholder:text-muted-foreground/50 h-6 w-full min-w-0 rounded-sm border border-transparent bg-transparent px-1.5 text-xs outline-none read-only:hover:border-transparent",
        readOnly && "cursor-default",
        className,
      )}
    />
  );
}

/**
 * Ordered multi-select as chips: clicking appends to the selection (the badge shows the position),
 * clicking again removes it.
 */
export function OrderedPicker({
  options,
  value,
  onChange,
  label,
  empty,
}: {
  options: readonly string[];
  value: readonly string[];
  onChange: (next: string[]) => void;
  label: string;
  empty?: string;
}) {
  if (options.length === 0)
    return <p className="text-muted-foreground text-[11px]">{empty ?? "Nothing to pick"}</p>;
  return (
    <div role="group" aria-label={label} className="flex flex-wrap gap-1">
      {options.map((name) => {
        const pos = value.indexOf(name);
        const on = pos >= 0;
        return (
          <button
            key={name}
            type="button"
            aria-pressed={on}
            onClick={() => onChange(on ? value.filter((v) => v !== name) : [...value, name])}
            className={cn(
              "border-border hover:bg-muted focus-visible:ring-ring/50 inline-flex h-6 items-center gap-1 rounded-md border px-1.5 font-mono text-[11px] outline-none focus-visible:ring-2",
              on && "border-brand/60 bg-brand-soft text-foreground hover:bg-brand-soft",
            )}
          >
            {on && (
              <span className="bg-brand text-primary-foreground inline-flex size-3.5 items-center justify-center rounded-full font-sans text-[9px] font-semibold tabular-nums">
                {pos + 1}
              </span>
            )}
            {name}
          </button>
        );
      })}
    </div>
  );
}

/** Label + control row used by the small DDL dialogs. */
export function Field({
  label,
  htmlFor,
  hint,
  children,
}: {
  label: string;
  htmlFor?: string;
  hint?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1">
      <label htmlFor={htmlFor} className="text-muted-foreground text-[11px] font-medium">
        {label}
      </label>
      {children}
      {hint && <p className="text-muted-foreground text-[11px]">{hint}</p>}
    </div>
  );
}
