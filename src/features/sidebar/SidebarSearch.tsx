import { Search, X } from "lucide-react";
import type { Ref } from "react";
import { Kbd } from "@/components/Kbd";
import { Input } from "@/components/ui/input";
import { shortcutFor, useKeymapVersion } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";

const ITEMS_LABEL = "Search for item";
/** The item search's syntax (`compileSearch`), as the field's tooltip. */
export const ITEMS_SEARCH_HINT =
  'Fuzzy match. ^name$ anchors the start / end, | or , separates alternatives, "quotes" match exactly.';

/**
 * The sidebar's search field. Esc clears it (and leaves it); ↓ hands focus to the list below
 * (`onArrowDown`). Shows its focus shortcut (`kbdHint`) while empty.
 */
export function SidebarSearch({
  value,
  onChange,
  onArrowDown,
  placeholder = "Search for item…",
  label = ITEMS_LABEL,
  hint = label === ITEMS_LABEL ? ITEMS_SEARCH_HINT : undefined,
  kbdHint = true,
  className,
  ref,
}: {
  value: string;
  onChange: (v: string) => void;
  onArrowDown?: () => void;
  placeholder?: string;
  label?: string;
  /** Tooltip of the field (the item search explains its syntax). */
  hint?: string;
  kbdHint?: boolean;
  className?: string;
  ref?: Ref<HTMLInputElement>;
}) {
  // Read at render: the user can remap it (Settings ▸ Shortcuts).
  useKeymapVersion();
  const focusCombo = shortcutFor("sidebar.focus-search");
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
        title={hint}
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
          kbdHint && focusCombo && <Kbd combo={focusCombo} />
        )}
      </div>
    </div>
  );
}
