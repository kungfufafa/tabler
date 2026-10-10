import { formatCombo } from "@/lib/keys/combo";
import { cn } from "@/lib/utils";

export function Kbd({ combo, className }: { combo: string; className?: string }) {
  return (
    <kbd
      className={cn(
        "border-border bg-surface text-muted-foreground inline-flex h-4.5 min-w-4.5 items-center justify-center rounded border px-1 font-sans text-[10px] font-medium",
        className,
      )}
    >
      {formatCombo(combo)}
    </kbd>
  );
}
