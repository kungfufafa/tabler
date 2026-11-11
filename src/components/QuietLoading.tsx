import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

/**
 * What a pane shows while its content loads: its own empty surface and nothing else — no
 * skeleton, spinner or "Loading…" text, so the data simply appears in place when it arrives.
 * `header` draws the empty header band of a grid, so a table's frame is already there.
 * Assistive tech still hears that the pane is busy.
 */
export function QuietLoading({
  header,
  className,
  ...props
}: ComponentProps<"div"> & { header?: boolean }) {
  return (
    <div
      role="status"
      aria-busy
      aria-label="Loading"
      className={cn("bg-surface h-full min-h-0 w-full", className)}
      {...props}
    >
      {header && <div className="border-border bg-surface-2 h-6 border-b" />}
    </div>
  );
}

/**
 * The same idea inside a dialog or a list: nothing to look at while a part of it loads, only a
 * status for assistive tech (`children` says what is loading).
 */
export function QuietStatus({ children = "Loading" }: { children?: string }) {
  return (
    <span role="status" className="sr-only">
      {children}
    </span>
  );
}
