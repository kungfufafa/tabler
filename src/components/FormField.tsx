import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

/**
 * A labelled form control (docs/ui-conventions.md, Forms).
 *
 * `stacked` (the default) puts a muted 11px medium label over the control; `inline` puts a muted
 * 11px label in an 88px right-aligned column beside an h-7 control. The hint sits under the
 * control and the error replaces it. With `htmlFor`, the hint and the error get the ids
 * `${htmlFor}-hint` and `${htmlFor}-error`, so the control can point `aria-describedby` at them
 * (and set `aria-invalid` while there is an error).
 */
export function FormField({
  label,
  htmlFor,
  hint,
  error,
  orientation = "stacked",
  children,
  className,
}: {
  label: ReactNode;
  htmlFor?: string;
  hint?: ReactNode;
  error?: ReactNode;
  orientation?: "stacked" | "inline";
  children: ReactNode;
  className?: string;
}) {
  const inline = orientation === "inline";
  const note = error ? (
    <p
      id={htmlFor ? `${htmlFor}-error` : undefined}
      role="alert"
      className="text-error selectable text-xs leading-4 break-words"
    >
      {error}
    </p>
  ) : hint ? (
    <div
      id={htmlFor ? `${htmlFor}-hint` : undefined}
      className="text-muted-foreground text-xs leading-4"
    >
      {hint}
    </div>
  ) : null;

  if (inline) {
    return (
      <div
        data-slot="form-field"
        className={cn(
          "grid min-w-0 grid-cols-[5.5rem_minmax(0,1fr)] items-start gap-x-3",
          className,
        )}
      >
        <label htmlFor={htmlFor} className="text-muted-foreground pt-1.5 text-right text-xs">
          {label}
        </label>
        <div className="flex min-w-0 flex-col gap-1">
          {children}
          {note}
        </div>
      </div>
    );
  }

  return (
    <div data-slot="form-field" className={cn("flex min-w-0 flex-col gap-1", className)}>
      <label htmlFor={htmlFor} className="text-muted-foreground text-xs font-medium">
        {label}
      </label>
      {children}
      {note}
    </div>
  );
}
