import { ChevronDown } from "lucide-react";
import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";
import type { AdminErrorInfo } from "./run";

/** Compact native select (long option lists such as MySQL's collations stay searchable by typing). */
export function NativeSelect({ className, children, ...props }: ComponentProps<"select">) {
  return (
    <div className="relative min-w-0">
      <select
        {...props}
        className={cn(
          "border-input dark:bg-input/30 focus-visible:border-ring focus-visible:ring-ring/50 h-7 w-full min-w-0 appearance-none truncate rounded-md border bg-transparent pr-7 pl-2 text-xs transition-colors outline-none focus-visible:ring-2 disabled:cursor-not-allowed disabled:opacity-50",
          className,
        )}
      >
        {children}
      </select>
      <ChevronDown
        aria-hidden
        className="text-muted-foreground pointer-events-none absolute top-1/2 right-2 size-3.5 -translate-y-1/2"
      />
    </div>
  );
}

/** A server error (or a refusal) under a form. */
export function ErrorLine({ error }: { error: AdminErrorInfo | string | null }) {
  if (!error) return null;
  const info = typeof error === "string" ? { message: error, detail: null, hint: null } : error;
  return (
    <p role="alert" className="text-error selectable text-xs break-words">
      {info.message}
      {info.detail && <span className="block opacity-80">{info.detail}</span>}
      {info.hint && <span className="block opacity-80">{info.hint}</span>}
    </p>
  );
}
