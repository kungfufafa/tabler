import { X } from "lucide-react";
import type { ReactNode } from "react";
import { SectionLabel } from "@/components/SectionLabel";
import { cn } from "@/lib/utils";

/** The title over a column of the window (a panel header). */
export function ColumnHeader({ children, right }: { children: ReactNode; right?: ReactNode }) {
  return (
    <div className="border-border flex h-8 shrink-0 items-center justify-between gap-2 border-b px-3 text-xs">
      <SectionLabel as="span" className="truncate">
        {children}
      </SectionLabel>
      {right}
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
        "bg-surface-2 border-border inline-flex h-6 max-w-full items-center gap-0.5 rounded-md border pr-0.5 pl-1.5 font-mono text-xs",
        disabled && "opacity-50",
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
        className="text-muted-foreground hover:text-foreground hover:bg-accent focus-visible:ring-ring/50 inline-flex size-4 shrink-0 items-center justify-center rounded-sm transition-colors outline-none focus-visible:ring-2"
      >
        <X aria-hidden className="size-3" />
      </button>
    </span>
  );
}
