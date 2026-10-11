import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

/** Scrollable section body with a fixed header row (the dialog close button sits at its right). */
export function SectionShell({
  title,
  actions,
  children,
  bodyClassName,
}: {
  title: string;
  actions?: ReactNode;
  children: ReactNode;
  bodyClassName?: string;
}) {
  return (
    <>
      <header className="border-border flex h-11 shrink-0 items-center gap-3 border-b pr-12 pl-5">
        <h2 className="text-[13px] font-semibold">{title}</h2>
        {actions && <div className="ml-auto flex items-center">{actions}</div>}
      </header>
      <div
        className={cn(
          "flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-5 py-4",
          bodyClassName,
        )}
      >
        {children}
      </div>
    </>
  );
}

/** A titled card of setting rows (macOS settings style). */
export function SettingGroup({ title, children }: { title?: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-1.5">
      {title && <h3 className="text-muted-foreground px-1 text-[11px] font-medium">{title}</h3>}
      <div className="border-border bg-surface divide-border divide-y rounded-lg border">
        {children}
      </div>
    </section>
  );
}

/** Label + description on the left, control on the right. `id` is the control's id. */
export function SettingRow({
  id,
  label,
  description,
  children,
}: {
  id?: string;
  label: ReactNode;
  description?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="flex min-h-11 items-center gap-6 px-3 py-2">
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <label id={id ? `${id}-label` : undefined} htmlFor={id} className="text-[13px] leading-5">
          {label}
        </label>
        {description && (
          <div
            id={id ? `${id}-description` : undefined}
            className="text-muted-foreground text-[11px] leading-4"
          >
            {description}
          </div>
        )}
      </div>
      {children && <div className="flex shrink-0 items-center gap-2">{children}</div>}
    </div>
  );
}

export interface SegmentOption<T> {
  value: T;
  label: ReactNode;
  icon?: ReactNode;
  title?: string;
}

/** Compact segmented radio group; arrow keys move the selection (roving tabindex). */
export function Segmented<T extends string | number>({
  id,
  value,
  options,
  onChange,
  className,
}: {
  /** Control id; the group is labelled by `${id}-label` (see SettingRow). */
  id: string;
  value: T;
  options: readonly SegmentOption<T>[];
  onChange: (value: T) => void;
  className?: string;
}) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const selected = options.findIndex((o) => o.value === value);

  const select = (index: number) => {
    const opt = options[index];
    if (!opt) return;
    onChange(opt.value);
    refs.current[index]?.focus();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const n = options.length;
    let next: number | null = null;
    if (e.key === "ArrowRight" || e.key === "ArrowDown") next = (index + 1) % n;
    else if (e.key === "ArrowLeft" || e.key === "ArrowUp") next = (index - 1 + n) % n;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = n - 1;
    if (next === null) return;
    e.preventDefault();
    select(next);
  };

  return (
    <div
      id={id}
      role="radiogroup"
      aria-labelledby={`${id}-label`}
      className={cn(
        "border-border bg-surface-2 inline-flex h-7 items-center gap-0.5 rounded-md border p-0.5",
        className,
      )}
    >
      {options.map((o, i) => {
        const checked = i === selected;
        return (
          <button
            key={String(o.value)}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={checked}
            title={o.title}
            tabIndex={checked || (selected < 0 && i === 0) ? 0 : -1}
            onClick={() => onChange(o.value)}
            onKeyDown={(e) => onKeyDown(e, i)}
            className={cn(
              "text-muted-foreground hover:text-foreground flex h-full min-w-11 items-center justify-center gap-1.5 rounded-[5px] px-2.5 text-xs font-medium tabular-nums transition-colors outline-none [&_svg]:size-3.5",
              // Tailwind v4: `outline-none` sets --tw-outline-style:none, which `outline-2` reads back,
              // so the focus ring needs an explicit style to draw at all.
              "focus-visible:outline-ring/70 focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-solid",
              "aria-checked:bg-surface aria-checked:text-foreground aria-checked:ring-border-strong/70 aria-checked:shadow-xs aria-checked:ring-1",
              "dark:aria-checked:bg-accent",
            )}
          >
            {o.icon}
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

/** Parses a typed integer clamped to [min, max]; `null` when the text is not a number. */
function parseClampedInt(text: string, min: number, max: number): number | null {
  const n = Number.parseInt(text, 10);
  return Number.isFinite(n) ? Math.min(max, Math.max(min, n)) : null;
}

/**
 * Integer input that commits on Enter, blur or unmount, clamped to [min, max]; garbage reverts.
 * Committing on unmount matters: ⌘W / Esc close the dialog with focus still in the field, and a
 * focused node that is removed fires no blur, so the typed value would otherwise be dropped.
 */
export function NumberField({
  id,
  value,
  min,
  max,
  suffix,
  onCommit,
}: {
  id: string;
  value: number;
  min: number;
  max: number;
  suffix?: string;
  onCommit: (value: number) => void;
}) {
  const [draft, setDraftState] = useState<string | null>(null);
  // Read by the unmount commit, which must see the latest draft and props without re-rendering.
  const draftRef = useRef<string | null>(null);
  const latest = useRef({ min, max, onCommit });
  useEffect(() => {
    latest.current = { min, max, onCommit };
  });
  const setDraft = (next: string | null) => {
    draftRef.current = next;
    setDraftState(next);
  };
  const commit = () => {
    const pending = draftRef.current;
    if (pending === null) return;
    setDraft(null);
    const n = parseClampedInt(pending, min, max);
    if (n !== null) onCommit(n);
  };
  useEffect(
    () => () => {
      const pending = draftRef.current;
      if (pending === null) return;
      draftRef.current = null;
      const { min, max, onCommit } = latest.current;
      const n = parseClampedInt(pending, min, max);
      if (n !== null) onCommit(n);
    },
    [],
  );
  return (
    <div className="flex items-center gap-1.5">
      <Input
        id={id}
        type="number"
        inputMode="numeric"
        min={min}
        max={max}
        step={1}
        value={draft ?? String(value)}
        aria-labelledby={`${id}-label`}
        aria-describedby={`${id}-description`}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit();
          }
        }}
        className="h-7 w-16 [appearance:textfield] rounded-md px-2 text-right text-xs tabular-nums [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none"
      />
      {suffix && <span className="text-muted-foreground text-xs">{suffix}</span>}
    </div>
  );
}
