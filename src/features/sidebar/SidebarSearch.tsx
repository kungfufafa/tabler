import { Search, X } from "lucide-react";
import type { Ref } from "react";
import { Kbd } from "@/components/Kbd";
import { Input } from "@/components/ui/input";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";

const FOCUS_COMBO = shortcutFor("sidebar.focus-search");

/**
 * The sidebar's search field. Esc clears it (and leaves it); ↓ hands focus to the list below
 * (`onArrowDown`). Shows its focus shortcut (`kbdHint`) while empty.
 */
export function SidebarSearch({
  value,
  onChange,
  onArrowDown,
  placeholder = "Search for item…",
  label = "Search for item",
  kbdHint = true,
  className,
  ref,
}: {
  value: string;
  onChange: (v: string) => void;
  onArrowDown?: () => void;
  placeholder?: string;
  label?: string;
  kbdHint?: boolean;
  className?: string;
  ref?: Ref<HTMLInputElement>;
}) {
  return (
    <div className={cn("relative min-w-0 flex-1", className)}>
      <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2" />
      <Input
        ref={ref}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            onChange("");
            (e.target as HTMLInputElement).blur();
          } else if (e.key === "ArrowDown" && onArrowDown) {
            e.preventDefault();
            onArrowDown();
          }
        }}
        placeholder={placeholder}
        aria-label={label}
        className="bg-surface h-6.5 rounded-md pr-10 pl-7 text-xs md:text-xs"
        data-scope="sidebar"
      />
      <div className="pointer-events-none absolute top-1/2 right-1.5 -translate-y-1/2">
        {value ? (
          <button
            type="button"
            className="text-muted-foreground hover:text-foreground pointer-events-auto flex"
            onClick={() => onChange("")}
            aria-label="Clear search"
          >
            <X className="size-3.5" />
          </button>
        ) : (
          kbdHint && FOCUS_COMBO && <Kbd combo={FOCUS_COMBO} />
        )}
      </div>
    </div>
  );
}
