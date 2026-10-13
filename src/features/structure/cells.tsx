import { ChevronDown } from "lucide-react";
import { useState, type KeyboardEvent, type ReactNode } from "react";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import { OrderedPicker } from "./parts";

/**
 * A grid cell that is a button: looks like plain text, shows a chevron while hovered or focused,
 * opens a menu on click, Enter, Space or ↓ (keyboard-first, no checkboxes: TablePlus).
 */
const CELL_BUTTON =
  "group/cell hover:border-border focus-visible:border-ring data-[state=open]:border-ring flex h-6 w-full min-w-0 items-center gap-1 rounded-sm border border-transparent px-1.5 text-left text-xs outline-none disabled:cursor-default disabled:hover:border-transparent";

function Chevron() {
  return (
    <ChevronDown
      aria-hidden
      className="text-muted-foreground ml-auto size-3 shrink-0 opacity-0 group-hover/cell:opacity-70 group-focus-visible/cell:opacity-70 group-disabled/cell:hidden"
    />
  );
}

export interface Choice<T extends string> {
  value: T;
  label: string;
  hint?: string;
}

/**
 * One value out of a few (is_nullable YES / NO, is_unique TRUE / FALSE, index algorithm). `keys`
 * maps a plain key to a value, so `y` / `n` set it without opening the menu.
 */
export function ChoiceCell<T extends string>({
  value,
  options,
  onChange,
  label,
  disabled,
  keys,
  cellKey,
  className,
  placeholder,
}: {
  value: T | null;
  options: readonly Choice<T>[];
  onChange: (value: T) => void;
  label: string;
  disabled?: boolean;
  keys?: Partial<Record<string, T>>;
  cellKey?: string;
  className?: string;
  placeholder?: string;
}) {
  const current = options.find((o) => o.value === value);
  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>) => {
    if (e.metaKey || e.ctrlKey || e.altKey || !keys) return;
    const to = keys[e.key.toLowerCase()];
    if (to !== undefined && !disabled) {
      e.preventDefault();
      if (to !== value) onChange(to);
    }
  };
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild disabled={disabled}>
        <button
          type="button"
          aria-label={label}
          data-cell-key={cellKey}
          onKeyDown={onKeyDown}
          className={cn(CELL_BUTTON, className)}
        >
          <span className={cn("truncate", !current && "text-muted-foreground/60")}>
            {current?.label ?? value ?? placeholder ?? ""}
          </span>
          <Chevron />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent className="min-w-32">
        <DropdownMenuRadioGroup value={value ?? ""} onValueChange={(v) => onChange(v as T)}>
          {options.map((o) => (
            <DropdownMenuRadioItem key={o.value} value={o.value} className="text-xs">
              <span className="font-mono">{o.label}</span>
              {o.hint && <span className="text-muted-foreground ml-auto pl-3">{o.hint}</span>}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export interface Toggle {
  id: string;
  label: string;
  checked: boolean;
  disabled?: boolean;
}

/**
 * A cell listing flags (MySQL `extra`: auto_increment, on update CURRENT_TIMESTAMP), each
 * toggled from its menu. `readOnly` text (DEFAULT_GENERATED, VIRTUAL GENERATED…) is shown as is.
 */
export function FlagsCell({
  flags,
  readOnly,
  onToggle,
  label,
  disabled,
  cellKey,
}: {
  flags: readonly Toggle[];
  readOnly?: string | null;
  onToggle: (id: string, on: boolean) => void;
  label: string;
  disabled?: boolean;
  cellKey?: string;
}) {
  const text = [...flags.filter((f) => f.checked).map((f) => f.label), readOnly]
    .filter(Boolean)
    .join(" ");
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild disabled={disabled}>
        <button type="button" aria-label={label} data-cell-key={cellKey} className={CELL_BUTTON}>
          <span className="truncate font-mono text-[11px]" title={text || undefined}>
            {text}
          </span>
          <Chevron />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent className="min-w-48">
        {flags.map((f) => (
          <DropdownMenuCheckboxItem
            key={f.id}
            checked={f.checked}
            disabled={f.disabled}
            onCheckedChange={(on) => onToggle(f.id, on === true)}
            className="font-mono text-xs"
          >
            {f.label}
          </DropdownMenuCheckboxItem>
        ))}
        {readOnly && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuLabel className="text-muted-foreground font-mono text-[11px] font-normal">
              {readOnly}
            </DropdownMenuLabel>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export interface MenuEntry {
  id: string;
  label: string;
  hint?: string;
  disabled?: boolean;
}

/** A small chevron button opening a menu of presets (the `column_default` dropdown). */
export function PresetMenu({
  entries,
  onPick,
  label,
  disabled,
}: {
  entries: readonly MenuEntry[];
  /** Returns true when it moved focus itself (the menu then leaves focus alone). */
  onPick: (id: string) => boolean | void;
  label: string;
  disabled?: boolean;
}) {
  const [keepFocus, setKeepFocus] = useState(false);
  return (
    <DropdownMenu onOpenChange={(o) => o && setKeepFocus(false)}>
      <DropdownMenuTrigger asChild disabled={disabled}>
        <button
          type="button"
          tabIndex={-1}
          aria-label={label}
          className="text-muted-foreground hover:text-foreground hover:bg-accent data-[state=open]:bg-accent flex size-5 shrink-0 items-center justify-center rounded-sm opacity-0 outline-none group-focus-within:opacity-100 group-hover:opacity-100 disabled:hidden data-[state=open]:opacity-100"
        >
          <ChevronDown className="size-3" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="end"
        className="min-w-44"
        onCloseAutoFocus={(e) => {
          if (keepFocus) e.preventDefault();
        }}
      >
        {entries.map((e) => (
          <DropdownMenuItem
            key={e.id}
            disabled={e.disabled}
            onSelect={() => setKeepFocus(onPick(e.id) === true)}
            className="text-xs"
          >
            <span className="font-mono">{e.label}</span>
            {e.hint && (
              <span className="text-muted-foreground ml-auto pl-3 text-[11px]">{e.hint}</span>
            )}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** Ordered column tokens (index column_name): click opens the picker. */
export function ColumnsCell({
  value,
  options,
  onChange,
  label,
  disabled,
  cellKey,
  allowEmpty = false,
  placeholder = "pick columns",
  hint = "Columns, in index order",
}: {
  value: readonly string[];
  options: readonly string[];
  onChange: (next: string[]) => void;
  label: string;
  disabled?: boolean;
  cellKey?: string;
  /** An empty pick is a value too (INCLUDE columns); otherwise it is ignored. */
  allowEmpty?: boolean;
  placeholder?: string;
  hint?: string;
}) {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState<string[]>([...value]);
  const text = value.join(", ");
  return (
    <Popover
      open={open}
      onOpenChange={(o) => {
        if (o) setDraft([...value]);
        else if ((allowEmpty || draft.length > 0) && draft.join("\u0000") !== value.join("\u0000"))
          onChange(draft);
        setOpen(o);
      }}
    >
      <PopoverTrigger asChild disabled={disabled}>
        <button
          type="button"
          aria-label={label}
          data-cell-key={cellKey}
          className={cn(CELL_BUTTON, "font-mono text-[11px]")}
        >
          <span className={cn("truncate", !text && "text-muted-foreground/60 italic")} title={text}>
            {text || placeholder}
          </span>
          <Chevron />
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-80 gap-2 p-2.5">
        <p className="text-muted-foreground text-[11px]">{hint}</p>
        <OrderedPicker label={label} options={options} value={draft} onChange={setDraft} />
      </PopoverContent>
    </Popover>
  );
}

/** A popover anchored to a grid cell (foreign key editor, column details). */
export function CellPopover({
  open,
  onOpenChange,
  trigger,
  children,
  className,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  trigger: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      <PopoverTrigger asChild>{trigger}</PopoverTrigger>
      <PopoverContent align="start" className={cn("w-96 p-3", className)}>
        {children}
      </PopoverContent>
    </Popover>
  );
}

export { CELL_BUTTON };
