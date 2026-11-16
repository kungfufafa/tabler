import { CircleAlert } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

/**
 * A form-level or footer error: a 14px alert icon and selectable 11px error text
 * (docs/ui-conventions.md, Forms). `clamp` keeps it to two lines (a dialog footer), with the full
 * text in `title`. An error that belongs to one field goes in `FormField`'s `error` instead.
 */
export function FormError({
  children,
  clamp,
  title,
  className,
}: {
  children: ReactNode;
  clamp?: boolean;
  title?: string;
  className?: string;
}) {
  return (
    <div
      role="alert"
      data-slot="form-error"
      className={cn("text-error flex min-w-0 items-start gap-1.5 text-xs", className)}
    >
      <CircleAlert aria-hidden className="mt-px size-3.5 shrink-0" />
      <span title={title} className={cn("selectable min-w-0 break-words", clamp && "line-clamp-2")}>
        {children}
      </span>
    </div>
  );
}
