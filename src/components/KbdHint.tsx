import { Kbd } from "@/components/Kbd";
import { cn } from "@/lib/utils";

/**
 * A keycap inside a coloured surface, such as a primary or destructive button or a tooltip: it
 * takes that surface's text colour and draws its edge in the same colour at 30%
 * (docs/ui-conventions.md, Dialogs). Plain `Kbd` is for keycaps on the page itself.
 */
export function KbdHint({ combo, className }: { combo: string; className?: string }) {
  return (
    <Kbd combo={combo} className={cn("border-current/30 bg-transparent text-inherit", className)} />
  );
}
