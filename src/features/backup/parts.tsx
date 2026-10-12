import { X } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

/** The small caps title over a column of the window. */
export function ColumnHeader({ children, right }: { children: ReactNode; right?: ReactNode }) {
  return (
    <div className="border-border flex h-8 shrink-0 items-center justify-between gap-2 border-b px-3">
      <span className="text-muted-foreground text-[11px] font-semibold tracking-wide uppercase">
        {children}
      </span>
      {right}
    </div>
  );
}

/** A labelled block of the options column. */
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
    <div className="flex min-w-0 flex-col gap-1.5">
      <label htmlFor={htmlFor} className="text-muted-foreground text-xs leading-4">
        {label}
      </label>
      {children}
      {hint && <div className="text-muted-foreground text-[11px] leading-4">{hint}</div>}
    </div>
  );
}

/** One option chip (`--no-owner ×`). */
export function Chip({
  children,
  onRemove,
  disabled,
}: {
  children: string;
  onRemove: () => void;
  disabled?: boolean;
}) {
  return (
    <span
      className={cn(
        "bg-surface-2 border-border inline-flex h-6 max-w-full items-center gap-0.5 rounded-md border pr-0.5 pl-1.5 font-mono text-[11px]",
        disabled && "opacity-60",
      )}
    >
      <span className="truncate" title={children}>
        {children}
      </span>
      <button
        type="button"
        aria-label={`Remove ${children}`}
        disabled={disabled}
        onClick={onRemove}
        className="text-muted-foreground hover:text-foreground hover:bg-accent inline-flex size-4 shrink-0 items-center justify-center rounded"
      >
        <X className="size-3" />
      </button>
    </span>
  );
}
