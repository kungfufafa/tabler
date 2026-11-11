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
  placeholder = ITEMS_LABEL,
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
    // A size container: the shortcut badge shows only where it fits beside the placeholder.
    <div className={cn("@container/search relative min-w-0 flex-1", className)}>
      {menu ? (
        <DropdownMenu>
          <DropdownMenuTrigger
            aria-label="Search options"
            title="Search options"
            className="text-muted-foreground hover:bg-accent hover:text-foreground data-[state=open]:bg-accent data-[state=open]:text-foreground focus-visible:ring-sidebar-ring absolute top-1/2 left-1 z-10 flex h-6 w-8 -translate-y-1/2 items-center justify-center rounded-sm outline-none focus-visible:ring-1 focus-visible:ring-inset"
          >
            <Search aria-hidden className="size-3.5" />
            <ChevronDown aria-hidden className="size-2.5" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="w-56">
            <SearchMenuEntries build={menu} />
          </DropdownMenuContent>
        </DropdownMenu>
      ) : (
        <Search
          aria-hidden
          className="text-muted-foreground pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2"
        />
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
          // Room on the right for the clear button, or for the badge once it shows.
          "bg-surface h-8 rounded-md pr-8 text-xs md:text-xs",
          kbdHint && !value && "@min-[15rem]/search:pr-24",
          menu ? "pl-10" : "pl-7",
        )}
        data-scope="sidebar"
      />
      <div className="pointer-events-none absolute top-1/2 right-1 -translate-y-1/2">
        {value ? (
          <button
            type="button"
            className="text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:ring-sidebar-ring pointer-events-auto flex size-6 items-center justify-center rounded-sm outline-none focus-visible:ring-1 focus-visible:ring-inset"
            onClick={() => onChange("")}
            aria-label="Clear search"
          >
            <X aria-hidden className="size-3.5" />
          </button>
        ) : (
          kbdHint &&
          focusCombo && (
            <Kbd combo={focusCombo} className="hidden @min-[15rem]/search:inline-flex" />
          )
        )}
      </div>
    </div>
  );
}

/** Builds the menu only while it is open (the content mounts on open). */
function SearchMenuEntries({ build }: { build: () => MenuEntry[] }) {
  return <DropdownMenuEntries entries={build()} />;
}
