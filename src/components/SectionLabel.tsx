import type { HTMLAttributes } from "react";
import { cn } from "@/lib/utils";

type SectionTag = "h2" | "h3" | "h4" | "p" | "span" | "div" | "legend";

/**
 * The heading of a group inside a form, a list, a panel or a popover: muted, 11px, medium,
 * sentence case (docs/ui-conventions.md, Lists and regions). `as` picks the element so the
 * document outline stays right; it never changes the look.
 */
export function SectionLabel({
  as: Tag = "h3",
  className,
  ...props
}: HTMLAttributes<HTMLElement> & { as?: SectionTag }) {
  return (
    <Tag
      data-slot="section-label"
      className={cn("text-muted-foreground text-xs font-medium", className)}
      {...props}
    />
  );
}
