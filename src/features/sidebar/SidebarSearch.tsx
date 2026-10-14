import { ChevronDown, Search, X } from "lucide-react";
import type { Ref } from "react";
import { Kbd } from "@/components/Kbd";
import { Input } from "@/components/ui/input";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { MenuEntry } from "@/features/objects/menu";
import { DropdownMenuEntries } from "@/features/objects/MenuEntries";
import { shortcutFor, useKeymapVersion } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";

const ITEMS_LABEL = "Search for item";
/** The item search's syntax (`compileSearch`), as the field's tooltip. */
export const ITEMS_SEARCH_HINT =
  'Fuzzy match. ^name$ anchors the start / end, | or , separates alternatives, "quotes" match exactly.';

/**
 * The sidebar's search field. Esc clears it (and leaves it); ↓ hands focus to the list below
 * (`onArrowDown`). Shows its focus shortcut (`kbdHint`) while empty. With `menu`, the magnifier
 * opens the field's menu (TablePlus: Search in Database…, Matching Strategy ▸, Settings ▸).
 */
export function SidebarSearch({
  value,
  onChange,
  onArrowDown,
  placeholder = "Search for item…",
  label = ITEMS_LABEL,
  hint = label === ITEMS_LABEL ? ITEMS_SEARCH_HINT : undefined,
  kbdHint = true,
  menu,
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
  /** The field's menu (the magnifier becomes its trigger). */
  menu?: () => MenuEntry[];
  className?: string;
  ref?: Ref<HTMLInputElement>;
}) {
  // Read at render: the user can remap it (Settings ▸ Shortcuts).
  useKeymapVersion();
  const focusCombo = shortcutFor("sidebar.focus-search");
  return (
    <div className={cn("relative min-w-0 flex-1", className)}>
      {menu ? (
        <DropdownMenu>
          <DropdownMenuTrigger
            aria-label="Search options"
            title="Search options"
            className="text-muted-foreground hover:text-foreground data-[state=open]:text-foreground focus-visible:ring-ring/50 absolute top-1/2 left-1 z-10 flex h-5 -translate-y-1/2 items-center rounded-sm pr-0.5 pl-1 outline-none focus-visible:ring-2"
          >
            <Search className="size-3.5" />
            <ChevronDown className="size-2.5" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="w-56">
            <SearchMenuEntries build={menu} />
          </DropdownMenuContent>
        </DropdownMenu>
      ) : (
        <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2" />
      )}
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
        className={cn(
          "bg-surface h-6.5 rounded-md pr-10 text-xs md:text-xs",
          menu ? "pl-9" : "pl-7",
        )}
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

/** Builds the menu only while it is open (the content mounts on open). */
function SearchMenuEntries({ build }: { build: () => MenuEntry[] }) {
  return <DropdownMenuEntries entries={build()} />;
}
