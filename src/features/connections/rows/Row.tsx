import type { UseFormReturn } from "react-hook-form";
import { Label } from "@/components/ui/label";
import type { ConnectionFormValues } from "../schema";

/**
 * The rows of the connection form that every driver may use (`SharedRow` in
 * `features/drivers/types.ts`), copied from `ConnectionDialog.tsx`'s core form and parameterized
 * by what a database calls each field (its descriptor and `KindUi`). With the core values (their
 * defaults) they render what the dialog renders today; F9b moves the dialog onto them.
 */

export type ConnectionForm = UseFormReturn<ConnectionFormValues>;

/** Label column + field column, TablePlus's connection form layout. */
export const GRID = "grid grid-cols-[104px_minmax(0,1fr)] items-start gap-x-3 gap-y-2.5";

/** One line of the form: the label on the left (right-aligned), the field with its message. */
export function Row({
  label,
  htmlFor,
  error,
  hint,
  children,
}: {
  label: string;
  htmlFor?: string;
  error?: string;
  hint?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <>
      <Label
        htmlFor={htmlFor}
        className="text-muted-foreground h-8 justify-end text-right text-xs font-normal"
      >
        {label}
      </Label>
      <div className="flex min-w-0 flex-col gap-1">
        {children}
        {error ? (
          <p className="text-error text-[11px]">{error}</p>
        ) : hint ? (
          <p className="text-muted-foreground text-[11px]">{hint}</p>
        ) : null}
      </div>
    </>
  );
}
