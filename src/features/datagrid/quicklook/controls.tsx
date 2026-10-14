import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/** The small segmented control of the value viewers (Quick Look, the Cell view): a radio group. */
export function Seg<T extends string>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: ReadonlyArray<{ value: T; label: string }>;
  onChange: (value: T) => void;
}) {
  return (
    <div
      role="radiogroup"
      aria-label={label}
      className="border-border bg-surface-2 dark:bg-background/40 flex h-6 items-center rounded-md border p-px"
    >
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={o.value === value}
          onClick={() => onChange(o.value)}
          className={cn(
            "h-full rounded-[5px] px-2 text-[11px] leading-none",
            o.value === value
              ? "bg-surface text-foreground dark:bg-accent font-medium shadow-xs"
              : "text-muted-foreground hover:text-foreground",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** A small pressed / not pressed button of the value viewers. */
export function Toggle({
  pressed,
  onChange,
  children,
  title,
}: {
  pressed: boolean;
  onChange: (pressed: boolean) => void;
  children: ReactNode;
  title?: string;
}) {
  return (
    <Button
      type="button"
      variant="ghost"
      size="xs"
      aria-pressed={pressed}
      title={title}
      onClick={() => onChange(!pressed)}
      className={cn(
        "text-muted-foreground hover:text-foreground h-6 text-[11px]",
        pressed && "bg-muted text-foreground",
      )}
    >
      {children}
    </Button>
  );
}
